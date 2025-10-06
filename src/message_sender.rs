use crate::driver::WriteCommand;
use egui::{ComboBox, TextEdit, Ui};
use oze_canopen::proto::nmt::NmtCommandSpecifier;
use tokio::sync::mpsc;

/// Panel for sending CAN messages
#[derive(Debug)]
pub struct MessageSender {
    selected_type: MessageType,
    
    // SYNC - no parameters needed
    
    // NMT parameters
    nmt_node_id: String,
    nmt_command: NmtCommandSpecifier,
    
    // Raw/PDO parameters
    raw_cob_id: String,
    raw_data: String,
    
    // EMCY parameters
    emcy_node_id: String,
    emcy_error_code: String,
    emcy_error_register: String,
    emcy_data: String,
    
    write_sender: mpsc::Sender<WriteCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageType {
    Sync,
    Nmt,
    Pdo,
    Raw,
    Emcy,
}

impl MessageType {
    fn as_str(&self) -> &str {
        match self {
            MessageType::Sync => "SYNC",
            MessageType::Nmt => "NMT",
            MessageType::Pdo => "PDO",
            MessageType::Raw => "Raw CAN",
            MessageType::Emcy => "EMCY",
        }
    }
    
    fn all() -> [MessageType; 5] {
        [
            MessageType::Sync,
            MessageType::Nmt,
            MessageType::Pdo,
            MessageType::Raw,
            MessageType::Emcy,
        ]
    }
}

impl MessageSender {
    pub fn new(write_sender: mpsc::Sender<WriteCommand>) -> Self {
        Self {
            selected_type: MessageType::Sync,
            nmt_node_id: String::from("1"),
            nmt_command: NmtCommandSpecifier::StartRemoteNode,
            raw_cob_id: String::from("180"),
            raw_data: String::from("00 00 00 00 00 00 00 00"),
            emcy_node_id: String::from("1"),
            emcy_error_code: String::from("1000"),
            emcy_error_register: String::from("00"),
            emcy_data: String::from("00 00 00 00 00"),
            write_sender,
        }
    }
    
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.group(|ui| {
            ui.heading("📤 Send CAN Message");
            ui.separator();
            
            // Message type selector
            ui.horizontal(|ui| {
                ui.label("Message Type:");
                ComboBox::from_id_salt("msg_type_combo")
                    .selected_text(self.selected_type.as_str())
                    .show_ui(ui, |ui| {
                        for msg_type in MessageType::all() {
                            ui.selectable_value(&mut self.selected_type, msg_type, msg_type.as_str());
                        }
                    });
            });
            
            ui.separator();
            
            // Message-specific fields
            match self.selected_type {
                MessageType::Sync => {
                    self.show_sync_ui(ui);
                }
                MessageType::Nmt => {
                    self.show_nmt_ui(ui);
                }
                MessageType::Pdo => {
                    self.show_pdo_ui(ui);
                }
                MessageType::Raw => {
                    self.show_raw_ui(ui);
                }
                MessageType::Emcy => {
                    self.show_emcy_ui(ui);
                }
            }
        });
    }
    
    fn show_sync_ui(&self, ui: &mut Ui) {
        ui.label("SYNC message (COB-ID: 0x080)");
        ui.label("No parameters required");
        ui.separator();
        
        if ui.button("📤 Send SYNC").clicked() {
            let _ = self.write_sender.try_send(WriteCommand::SendSync);
        }
    }
    
    fn show_nmt_ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Node ID:");
            ui.add(TextEdit::singleline(&mut self.nmt_node_id)
                .desired_width(60.0)
                .hint_text("0-127"));
        });
        
        ui.horizontal(|ui| {
            ui.label("Command:");
            ComboBox::from_id_salt("nmt_cmd_combo")
                .selected_text(format!("{:?}", self.nmt_command))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.nmt_command, NmtCommandSpecifier::StartRemoteNode, "Start Remote Node (0x01)");
                    ui.selectable_value(&mut self.nmt_command, NmtCommandSpecifier::StopRemoteNode, "Stop Remote Node (0x02)");
                    ui.selectable_value(&mut self.nmt_command, NmtCommandSpecifier::EnterPreOperational, "Enter Pre-Operational (0x80)");
                    ui.selectable_value(&mut self.nmt_command, NmtCommandSpecifier::ResetNode, "Reset Node (0x81)");
                    ui.selectable_value(&mut self.nmt_command, NmtCommandSpecifier::ResetCommunication, "Reset Communication (0x82)");
                });
        });
        
        ui.separator();
        
        if ui.button("📤 Send NMT").clicked() {
            if let Ok(node_id) = self.nmt_node_id.parse::<u8>() {
                if node_id <= 127 {
                    let _ = self.write_sender.try_send(WriteCommand::SendNmt {
                        node_id,
                        command: self.nmt_command,
                    });
                } else {
                    log::error!("Invalid node ID: must be 0-127");
                }
            } else {
                log::error!("Invalid node ID format");
            }
        }
    }
    
    fn show_pdo_ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("COB-ID (hex):");
            ui.add(TextEdit::singleline(&mut self.raw_cob_id)
                .desired_width(100.0)
                .hint_text("180"));
        });
        
        ui.horizontal(|ui| {
            ui.label("Data (hex):");
            ui.add(TextEdit::singleline(&mut self.raw_data)
                .desired_width(250.0)
                .hint_text("00 11 22 33 44 55 66 77"));
        });
        
        ui.label("ℹ️ PDO COB-IDs: TPDO1=0x180+NodeID, RPDO1=0x200+NodeID");
        ui.separator();
        
        if ui.button("📤 Send PDO").clicked() {
            if let Ok(cob_id) = u32::from_str_radix(&self.raw_cob_id, 16) {
                if let Ok(data) = parse_hex_data(&self.raw_data) {
                    if data.len() <= 8 {
                        let _ = self.write_sender.try_send(WriteCommand::SendPdo { cob_id, data });
                    } else {
                        log::error!("Data too long: max 8 bytes");
                    }
                } else {
                    log::error!("Invalid data format");
                }
            } else {
                log::error!("Invalid COB-ID format");
            }
        }
    }
    
    fn show_raw_ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("COB-ID (hex):");
            ui.add(TextEdit::singleline(&mut self.raw_cob_id)
                .desired_width(100.0)
                .hint_text("123"));
        });
        
        ui.horizontal(|ui| {
            ui.label("Data (hex):");
            ui.add(TextEdit::singleline(&mut self.raw_data)
                .desired_width(250.0)
                .hint_text("00 11 22 33 44 55 66 77"));
        });
        
        ui.label("ℹ️ Send any raw CAN frame");
        ui.separator();
        
        if ui.button("📤 Send Raw CAN").clicked() {
            if let Ok(cob_id) = u32::from_str_radix(&self.raw_cob_id, 16) {
                if let Ok(data) = parse_hex_data(&self.raw_data) {
                    if data.len() <= 8 {
                        let _ = self.write_sender.try_send(WriteCommand::SendRaw { cob_id, data });
                    } else {
                        log::error!("Data too long: max 8 bytes");
                    }
                } else {
                    log::error!("Invalid data format");
                }
            } else {
                log::error!("Invalid COB-ID format");
            }
        }
    }
    
    fn show_emcy_ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Node ID:");
            ui.add(TextEdit::singleline(&mut self.emcy_node_id)
                .desired_width(60.0)
                .hint_text("1"));
        });
        
        ui.horizontal(|ui| {
            ui.label("Error Code (hex):");
            ui.add(TextEdit::singleline(&mut self.emcy_error_code)
                .desired_width(100.0)
                .hint_text("1000"));
        });
        
        ui.horizontal(|ui| {
            ui.label("Error Register (hex):");
            ui.add(TextEdit::singleline(&mut self.emcy_error_register)
                .desired_width(60.0)
                .hint_text("00"));
        });
        
        ui.horizontal(|ui| {
            ui.label("Manufacturer Data (hex):");
            ui.add(TextEdit::singleline(&mut self.emcy_data)
                .desired_width(150.0)
                .hint_text("00 00 00 00 00"));
        });
        
        ui.label("ℹ️ EMCY COB-ID: 0x080 + Node ID");
        ui.separator();
        
        if ui.button("📤 Send EMCY").clicked() {
            if let Ok(node_id) = self.emcy_node_id.parse::<u8>() {
                if let Ok(error_code) = u16::from_str_radix(&self.emcy_error_code, 16) {
                    if let Ok(error_register) = u8::from_str_radix(&self.emcy_error_register, 16) {
                        if let Ok(data_vec) = parse_hex_data(&self.emcy_data) {
                            if data_vec.len() == 5 {
                                let mut data = [0u8; 5];
                                data.copy_from_slice(&data_vec);
                                let _ = self.write_sender.try_send(WriteCommand::SendEmcy {
                                    node_id,
                                    error_code,
                                    error_register,
                                    data,
                                });
                            } else {
                                log::error!("Manufacturer data must be exactly 5 bytes");
                            }
                        } else {
                            log::error!("Invalid manufacturer data format");
                        }
                    } else {
                        log::error!("Invalid error register format");
                    }
                } else {
                    log::error!("Invalid error code format");
                }
            } else {
                log::error!("Invalid node ID format");
            }
        }
    }
}

/// Parse hex data string like "00 11 22" or "001122" into Vec<u8>
fn parse_hex_data(s: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    
    if cleaned.len() % 2 != 0 {
        return Err("Hex string must have even number of characters".to_string());
    }
    
    let mut result = Vec::new();
    for i in (0..cleaned.len()).step_by(2) {
        match u8::from_str_radix(&cleaned[i..i+2], 16) {
            Ok(byte) => result.push(byte),
            Err(_) => return Err(format!("Invalid hex at position {}", i)),
        }
    }
    
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_hex_data() {
        assert_eq!(parse_hex_data("00 11 22"), Ok(vec![0x00, 0x11, 0x22]));
        assert_eq!(parse_hex_data("001122"), Ok(vec![0x00, 0x11, 0x22]));
        assert_eq!(parse_hex_data("FF"), Ok(vec![0xFF]));
        assert!(parse_hex_data("0").is_err());
        assert!(parse_hex_data("GG").is_err());
    }
}

