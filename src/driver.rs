use crate::message_cached::MessageCached;
use oze_canopen::{
    canopen::{self, JoinHandles},
    interface::{CanOpenInfo, CanOpenInterface, Connection},
    proto::nmt::{NmtCommand, NmtCommandSpecifier},
    transmitter::TxPacket,
};
use std::{collections::VecDeque, time::Duration};
use tokio::{signal::ctrl_c, sync::{watch, mpsc}, task::JoinHandle, time::sleep};

/// Enum representing different control commands that can be sent to the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlCommand {
    Stop,
    Kill,
    Process,
}

/// Enum representing different write commands for sending CAN messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteCommand {
    /// Send a SYNC message (COB-ID: 0x080)
    SendSync,
    /// Send an NMT command to a specific node
    SendNmt { node_id: u8, command: NmtCommandSpecifier },
    /// Send a raw CAN frame with COB-ID and data
    SendRaw { cob_id: u32, data: Vec<u8> },
    /// Send a PDO (Process Data Object)
    SendPdo { cob_id: u32, data: Vec<u8> },
    /// Send an SDO Download (write to object dictionary)
    SendSdoDownload { node_id: u8, index: u16, subindex: u8, data: Vec<u8> },
    /// Configure TPDO1 for Statusword on SYNC
    ConfigureTpdo1Statusword { node_id: u8 },
}

/// Struct representing the state of the CAN interface and received messages.
#[derive(Default, Debug, Clone)]
pub struct State {
    pub can_name: String,
    pub bitrate: Option<u32>,
    pub data: VecDeque<MessageCached>,
    pub info: CanOpenInfo,
    pub exit_signal: bool,
}

/// Struct representing control data including the command and connection details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    pub command: ControlCommand,
    pub connection: Connection,
}

/// Struct representing the driver responsible for processing CAN messages and handling control commands.
pub struct Driver {
    sender: watch::Sender<State>,
    receiver: watch::Receiver<Control>,
    write_receiver: mpsc::Receiver<WriteCommand>,
    state: State,
    pub co: CanOpenInterface,
    control: Control,
    index: u64,
    handles: JoinHandles,
}

const MAX_MESSAGES_IN_STATE: usize = 512;

impl Driver {
    pub fn new(
        sender: watch::Sender<State>,
        receiver: watch::Receiver<Control>,
        write_receiver: mpsc::Receiver<WriteCommand>,
    ) -> Self {
        // Initialize the CANopen interface with the initial connection details.
        let initial_connection = receiver.borrow().connection.clone();
        let (co, handles) = canopen::start(initial_connection.can_name, initial_connection.bitrate);

        // Create the driver and start running it.
        let control = receiver.borrow().clone();
        Driver {
            co,
            sender,
            control,
            receiver,
            write_receiver,
            index: 0,
            state: State::default(),
            handles,
        }
    }

    /// Asynchronously processes incoming CAN messages and control commands.
    async fn process(&mut self) {
        // Wait for a message, timeout, ctrl_c signal, or write command.
        let rcv = tokio::select! {
            rcv = self.co.rx.recv() => Some(rcv),
            () = sleep(Duration::from_millis(100)) => None,
            _ = ctrl_c() => {
                self.control.command = ControlCommand::Kill;
                return;
            },
            Some(write_cmd) = self.write_receiver.recv() => {
                self.handle_write_command(write_cmd).await;
                None
            }
        };

        // Get the latest control data if it has changed.
        if self.receiver.has_changed().unwrap() {
            self.control = self.receiver.borrow_and_update().clone();
            // Update connection details if they have changed.
            self.co
                .connection
                .lock()
                .await
                .clone_from(&self.control.connection);
        }

        // Set information from the CANopen stack to the state.
        let info = self.co.info.lock().await.clone();
        self.state.info = info;

        // Handle control commands.
        match self.control.command {
            ControlCommand::Stop | ControlCommand::Kill => {
                return;
            }
            ControlCommand::Process => {}
        }

        // If no message has been received, return.
        let Some(Ok(d)) = rcv else {
            return;
        };

        // Parse and cache the received message.
        let d = MessageCached::new(self.index, d);
        self.index += 1;

        // Add the new message to the state, ensuring the state does not exceed the max size.
        while self.state.data.len() > MAX_MESSAGES_IN_STATE {
            self.state.data.pop_front();
        }
        self.state.data.push_back(d);
    }

    /// Handles write commands to send CAN messages.
    async fn handle_write_command(&mut self, cmd: WriteCommand) {
        match cmd {
            WriteCommand::SendSync => {
                if let Err(e) = self.co.send_sync().await {
                    log::error!("Failed to send SYNC message: {:?}", e);
                } else {
                    log::info!("SYNC message sent successfully");
                }
            }
            WriteCommand::SendNmt { node_id, command } => {
                let nmt_cmd = NmtCommand::new(command, node_id);
                if let Err(e) = self.co.send_nmt(nmt_cmd).await {
                    log::error!("Failed to send NMT message: {:?}", e);
                } else {
                    log::info!("NMT message sent successfully: {:?} to node {}", command, node_id);
                }
            }
            WriteCommand::SendRaw { cob_id, data } => {
                let cob_id_u16 = (cob_id & 0x7FF) as u16;
                let packet = TxPacket { cob_id: cob_id_u16, data };
                if let Err(e) = self.co.tx.send(packet).await {
                    log::error!("Failed to send raw CAN message: {:?}", e);
                } else {
                    log::info!("Raw CAN message sent successfully: COB-ID=0x{:03X}", cob_id);
                }
            }
            WriteCommand::SendPdo { cob_id, data } => {
                let cob_id_u16 = (cob_id & 0x7FF) as u16;
                let packet = TxPacket { cob_id: cob_id_u16, data };
                if let Err(e) = self.co.tx.send(packet).await {
                    log::error!("Failed to send PDO message: {:?}", e);
                } else {
                    log::info!("PDO message sent successfully: COB-ID=0x{:03X}", cob_id);
                }
            }
            WriteCommand::SendSdoDownload { node_id, index, subindex, data } => {
                self.send_sdo_download(node_id, index, subindex, &data).await;
            }
            WriteCommand::ConfigureTpdo1Statusword { node_id } => {
                log::info!("Configuring TPDO1 for Statusword (0x6041) on node {}", node_id);
                
                // Étape 1: NMT Pre-Operational
                let nmt_pre_op = NmtCommand::new(NmtCommandSpecifier::EnterPreOperational, node_id);
                if let Err(e) = self.co.send_nmt(nmt_pre_op).await {
                    log::error!("Failed to send NMT Pre-Operational: {:?}", e);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                
                // Étape 2: Désactiver TPDO1 (COB-ID avec bit 31 = 1)
                let cob_id_disabled = 0x80000180u32 + u32::from(node_id);
                self.send_sdo_download(node_id, 0x1800, 0x01, &cob_id_disabled.to_le_bytes().to_vec()).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
                
                // Étape 3: Effacer le mapping (mettre le nombre d'objets à 0)
                self.send_sdo_download(node_id, 0x1A00, 0x00, &[0x00]).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
                
                // Étape 4: Configurer le mapping pour Statusword (0x6041, 32 bits)
                // Format: 0xIIIISSLL (Index + Subindex + Length en bits)
                let mapping: u32 = 0x60410020; // 0x6041 subindex 0x00, 32 bits (0x20)
                self.send_sdo_download(node_id, 0x1A00, 0x01, &mapping.to_le_bytes().to_vec()).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
                
                // Étape 5: Activer le mapping (1 objet mappé)
                self.send_sdo_download(node_id, 0x1A00, 0x00, &[0x01]).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
                
                // Étape 6: Activer TPDO1 (COB-ID sans bit 31)
                let cob_id_enabled = 0x00000180u32 + u32::from(node_id);
                self.send_sdo_download(node_id, 0x1800, 0x01, &cob_id_enabled.to_le_bytes().to_vec()).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
                
                // Étape 7: NMT Operational
                let nmt_op = NmtCommand::new(NmtCommandSpecifier::StartRemoteNode, node_id);
                if let Err(e) = self.co.send_nmt(nmt_op).await {
                    log::error!("Failed to send NMT Operational: {:?}", e);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                
                // Étape 8: Configurer le type de transmission (0x01 = SYNC cyclique à chaque SYNC)
                self.send_sdo_download(node_id, 0x1800, 0x02, &[0x01]).await;
                
                log::info!("TPDO1 configured successfully for node {}", node_id);
            }
        }
    }
    
    async fn send_sdo_download(&mut self, node_id: u8, index: u16, subindex: u8, data: &[u8]) {
        let sdo_tx_cob_id = 0x600 + u16::from(node_id);
        let mut sdo_data = Vec::with_capacity(8);
        
        // SDO Download Expedited (for data <= 4 bytes)
        if data.len() <= 4 {
            // Command byte: 0x23 = Initiate download expedited, 4 bytes specified
            let n = (4 - data.len()) as u8;
            let ccs = 0x20 | (n << 2) | 0x03; // Expedited + size indicated + size
            
            sdo_data.push(ccs);
            sdo_data.extend_from_slice(&index.to_le_bytes());
            sdo_data.push(subindex);
            sdo_data.extend_from_slice(data);
            // Pad to 8 bytes
            while sdo_data.len() < 8 {
                sdo_data.push(0);
            }
        } else {
            log::error!("SDO segmented transfer not implemented yet. Data size: {} bytes", data.len());
            return;
        }
        
        let packet = TxPacket { cob_id: sdo_tx_cob_id, data: sdo_data };
        if let Err(e) = self.co.tx.send(packet).await {
            log::error!("Failed to send SDO Download: {:?}", e);
        } else {
            log::info!("SDO Download sent to node {}: index=0x{:04X}, subindex=0x{:02X}, data={:02X?}", 
                node_id, index, subindex, data);
        }
    }

    /// Asynchronously runs the driver, continuously processing messages and sending state updates.
    async fn run(&mut self) {
        self.state.data.reserve_exact(MAX_MESSAGES_IN_STATE);
        loop {
            self.process().await;
            if self.control.command == ControlCommand::Kill {
                self.state.exit_signal = true;
            }

            self.sender.send(self.state.clone()).unwrap();
            // Exit the loop if a Kill command is received.
            if self.control.command == ControlCommand::Kill {
                break;
            }
        }
    }

    /// Starts the driver with the given state and control channels.
    pub fn start_thread(mut self) -> JoinHandle<()> {
        tokio::spawn(async move {
            self.run().await;
            self.handles.close_and_join().await;
        })
    }
}
