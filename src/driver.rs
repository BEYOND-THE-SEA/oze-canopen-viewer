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
    /// Send an Emergency message
    SendEmcy { node_id: u8, error_code: u16, error_register: u8, data: [u8; 5] },
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
            WriteCommand::SendEmcy { node_id, error_code, error_register, data } => {
                let emcy_cob_id = 0x080 + u16::from(node_id);
                let mut emcy_data = Vec::with_capacity(8);
                emcy_data.extend_from_slice(&error_code.to_le_bytes());
                emcy_data.push(error_register);
                emcy_data.extend_from_slice(&data);
                
                let packet = TxPacket { cob_id: emcy_cob_id, data: emcy_data };
                if let Err(e) = self.co.tx.send(packet).await {
                    log::error!("Failed to send EMCY message: {:?}", e);
                } else {
                    log::info!("EMCY message sent successfully from node {}: error_code=0x{:04X}", node_id, error_code);
                }
            }
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
