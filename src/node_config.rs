use crate::driver::WriteCommand;
use oze_canopen::proto::nmt::NmtCommandSpecifier;

/// Known vendor IDs from Seakite CANopen CLI bootup mapping.
pub const VENDOR_RFC4800: u32 = 0x0000_0182;
pub const VENDOR_SENSY: u32 = 0x0000_017E;

/// CiA 301 store signature (`"save"` little-endian).
pub const STORE_SIGNATURE_SAVE: u32 = 0x6576_6173;

pub const OD_STORE_PARAMS: u16 = 0x1010;
pub const OD_SUBINDEX_STORE_ALL: u8 = 0x01;

pub const LSS_COB_ID: u32 = 0x7E5;
pub const LSS_SWITCH_MODE_GLOBAL: u8 = 0x04;
pub const LSS_SET_NODE_ID: u8 = 0x11;
pub const LSS_SET_BIT_RATE: u8 = 0x13;
pub const LSS_STORE_CONFIGURATION: u8 = 0x17;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceProfile {
    Auto,
    Rfc4800,
    BaumerEam300b,
    Sensy,
    GenericSdo,
    ExpertLssGlobal,
}

impl DeviceProfile {
    pub const ALL: [DeviceProfile; 6] = [
        DeviceProfile::Auto,
        DeviceProfile::Rfc4800,
        DeviceProfile::BaumerEam300b,
        DeviceProfile::Sensy,
        DeviceProfile::GenericSdo,
        DeviceProfile::ExpertLssGlobal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DeviceProfile::Auto => "Auto",
            DeviceProfile::Rfc4800 => "RFC4800 (LSS global)",
            DeviceProfile::BaumerEam300b => "Baumer EAM300B",
            DeviceProfile::Sensy => "Sensy LC",
            DeviceProfile::GenericSdo => "Generic SDO",
            DeviceProfile::ExpertLssGlobal => "Expert LSS (global)",
        }
    }

    pub fn is_dangerous(self) -> bool {
        matches!(
            self,
            DeviceProfile::Rfc4800 | DeviceProfile::ExpertLssGlobal
        )
    }
}

/// Resolve profile from auto-detect + user override.
pub fn resolve_profile(selected: DeviceProfile, vendor_id: Option<u32>) -> DeviceProfile {
    if selected != DeviceProfile::Auto {
        return selected;
    }
    match vendor_id {
        Some(VENDOR_RFC4800) => DeviceProfile::Rfc4800,
        Some(VENDOR_SENSY) => DeviceProfile::Sensy,
        _ => DeviceProfile::GenericSdo,
    }
}

pub fn is_valid_node_id(id: u8) -> bool {
    (1..=127).contains(&id)
}

/// Bitrates supported by the Seakite CANopen CLI.
pub const CLI_BITRATES: &[(u32, &str)] = &[
    (50_000, "50 kbit/s"),
    (100_000, "100 kbit/s"),
    (125_000, "125 kbit/s"),
    (250_000, "250 kbit/s"),
    (500_000, "500 kbit/s"),
];

pub fn is_valid_cli_bitrate(bps: u32) -> bool {
    CLI_BITRATES.iter().any(|(v, _)| *v == bps)
}

/// One step in a configuration script: delay after previous step, then command.
#[derive(Debug, Clone)]
pub struct ConfigStep {
    pub delay_ms: u64,
    pub label: String,
    pub command: WriteCommand,
}

fn sdo_download(node_id: u8, index: u16, subindex: u8, data: Vec<u8>) -> WriteCommand {
    WriteCommand::SendSdoDownload {
        node_id,
        index,
        subindex,
        data,
    }
}

fn sdo_upload(node_id: u8, index: u16, subindex: u8) -> WriteCommand {
    WriteCommand::SendSdoUpload {
        node_id,
        index,
        subindex,
    }
}

fn nmt(node_id: u8, command: NmtCommandSpecifier) -> WriteCommand {
    WriteCommand::SendNmt { node_id, command }
}

fn lss(command: u8, data: Vec<u8>) -> WriteCommand {
    WriteCommand::SendLssCommand { command, data }
}

fn baumer_save_all(node_id: u8) -> WriteCommand {
    sdo_download(
        node_id,
        OD_STORE_PARAMS,
        OD_SUBINDEX_STORE_ALL,
        STORE_SIGNATURE_SAVE.to_le_bytes().to_vec(),
    )
}

fn baumer_bitrate_code(bps: u32) -> Option<u8> {
    match bps {
        50_000 => Some(0x02),
        100_000 => Some(0x03),
        125_000 => Some(0x04),
        250_000 => Some(0x05),
        500_000 => Some(0x06),
        800_000 => Some(0x07),
        1_000_000 => Some(0x08),
        _ => None,
    }
}

fn sensy_bitrate_code(bps: u32) -> Option<u8> {
    match bps {
        20_000 => Some(0x00),
        50_000 => Some(0x01),
        100_000 => Some(0x02),
        125_000 => Some(0x03),
        250_000 => Some(0x04),
        500_000 => Some(0x05),
        800_000 => Some(0x06),
        1_000_000 => Some(0x07),
        _ => None,
    }
}

fn rfc4800_lss_bitrate_code(bps: u32) -> Option<u8> {
    match bps {
        50_000 => Some(0x06),
        100_000 => Some(0x05),
        125_000 => Some(0x04),
        250_000 => Some(0x03),
        500_000 => Some(0x02),
        800_000 => Some(0x01),
        1_000_000 => Some(0x00),
        _ => None,
    }
}

/// Build steps to change node ID for the given profile (target = current node on bus).
pub fn build_change_node_id_steps(
    profile: DeviceProfile,
    current_node_id: u8,
    new_node_id: u8,
) -> Result<Vec<ConfigStep>, String> {
    if !is_valid_node_id(new_node_id) {
        return Err("Node ID must be between 1 and 127".to_string());
    }
    if new_node_id == current_node_id {
        return Ok(vec![]);
    }

    let mut steps = Vec::new();

    match profile {
        DeviceProfile::BaumerEam300b => {
            steps.push(ConfigStep {
                delay_ms: 0,
                label: format!("SDO download 0x2101:00 = {new_node_id}"),
                command: sdo_download(current_node_id, 0x2101, 0x00, vec![new_node_id]),
            });
            steps.push(ConfigStep {
                delay_ms: 100,
                label: "Store all parameters (0x1010:01)".to_string(),
                command: baumer_save_all(current_node_id),
            });
            steps.push(ConfigStep {
                delay_ms: 100,
                label: "NMT Reset Communication".to_string(),
                command: nmt(current_node_id, NmtCommandSpecifier::ResetCommunication),
            });
        }
        DeviceProfile::Sensy => {
            steps.push(ConfigStep {
                delay_ms: 0,
                label: format!("SDO download 0x2000:00 = {new_node_id} (auto-save on device)"),
                command: sdo_download(current_node_id, 0x2000, 0x00, vec![new_node_id]),
            });
            steps.push(ConfigStep {
                delay_ms: 100,
                label: "NMT Reset Communication".to_string(),
                command: nmt(current_node_id, NmtCommandSpecifier::ResetCommunication),
            });
        }
        DeviceProfile::Rfc4800 | DeviceProfile::ExpertLssGlobal => {
            steps.push(ConfigStep {
                delay_ms: 0,
                label: "LSS: switch all nodes to configuration mode".to_string(),
                command: lss(LSS_SWITCH_MODE_GLOBAL, vec![]),
            });
            steps.push(ConfigStep {
                delay_ms: 50,
                label: "LSS: enter configuration state".to_string(),
                command: lss(LSS_SWITCH_MODE_GLOBAL, vec![0x01]),
            });
            steps.push(ConfigStep {
                delay_ms: 50,
                label: format!("LSS: set node ID = {new_node_id} (GLOBAL — all LSS nodes)"),
                command: lss(LSS_SET_NODE_ID, vec![new_node_id]),
            });
            steps.push(ConfigStep {
                delay_ms: 50,
                label: "LSS: store configuration".to_string(),
                command: lss(LSS_STORE_CONFIGURATION, vec![]),
            });
            steps.push(ConfigStep {
                delay_ms: 100,
                label: "NMT Reset Communication (broadcast node 0)".to_string(),
                command: nmt(0, NmtCommandSpecifier::ResetCommunication),
            });
        }
        DeviceProfile::Auto | DeviceProfile::GenericSdo => {
            return Err(
                "Select a device profile (Baumer, Sensy, RFC4800, or LSS expert) to change node ID"
                    .to_string(),
            );
        }
    }

    Ok(steps)
}

/// Build steps to change device CAN bitrate (persistent on device).
pub fn build_change_bitrate_steps(
    profile: DeviceProfile,
    current_node_id: u8,
    new_bitrate: u32,
    current_bitrate: Option<u32>,
) -> Result<Vec<ConfigStep>, String> {
    if current_bitrate == Some(new_bitrate) {
        return Ok(vec![]);
    }

    let mut steps = Vec::new();

    match profile {
        DeviceProfile::BaumerEam300b => {
            let code = baumer_bitrate_code(new_bitrate)
                .ok_or_else(|| format!("Baumer does not support {new_bitrate} bps"))?;
            steps.push(ConfigStep {
                delay_ms: 0,
                label: format!("SDO download 0x2100:00 = 0x{code:02X}"),
                command: sdo_download(current_node_id, 0x2100, 0x00, vec![code]),
            });
            steps.push(ConfigStep {
                delay_ms: 100,
                label: "Store all parameters (0x1010:01)".to_string(),
                command: baumer_save_all(current_node_id),
            });
            steps.push(ConfigStep {
                delay_ms: 100,
                label: "NMT Reset Node".to_string(),
                command: nmt(current_node_id, NmtCommandSpecifier::ResetNode),
            });
        }
        DeviceProfile::Sensy => {
            let code = sensy_bitrate_code(new_bitrate)
                .ok_or_else(|| format!("Sensy does not support {new_bitrate} bps"))?;
            steps.push(ConfigStep {
                delay_ms: 0,
                label: format!("SDO download 0x2001:00 = 0x{code:02X}"),
                command: sdo_download(current_node_id, 0x2001, 0x00, vec![code]),
            });
            steps.push(ConfigStep {
                delay_ms: 100,
                label: "NMT Reset Node".to_string(),
                command: nmt(current_node_id, NmtCommandSpecifier::ResetNode),
            });
        }
        DeviceProfile::Rfc4800 | DeviceProfile::ExpertLssGlobal => {
            let code = rfc4800_lss_bitrate_code(new_bitrate)
                .ok_or_else(|| format!("RFC4800 LSS does not support {new_bitrate} bps"))?;
            steps.push(ConfigStep {
                delay_ms: 0,
                label: "LSS: switch all nodes to configuration mode".to_string(),
                command: lss(LSS_SWITCH_MODE_GLOBAL, vec![]),
            });
            steps.push(ConfigStep {
                delay_ms: 50,
                label: "LSS: enter configuration state".to_string(),
                command: lss(LSS_SWITCH_MODE_GLOBAL, vec![0x01]),
            });
            steps.push(ConfigStep {
                delay_ms: 50,
                label: format!("LSS: set bit rate code 0x{code:02X} (GLOBAL)"),
                command: lss(LSS_SET_BIT_RATE, vec![0x00, code]),
            });
            steps.push(ConfigStep {
                delay_ms: 50,
                label: "LSS: store configuration".to_string(),
                command: lss(LSS_STORE_CONFIGURATION, vec![]),
            });
            steps.push(ConfigStep {
                delay_ms: 100,
                label: "NMT Reset Node (broadcast)".to_string(),
                command: nmt(0, NmtCommandSpecifier::ResetNode),
            });
        }
        DeviceProfile::Auto | DeviceProfile::GenericSdo => {
            return Err(
                "Select a device profile to change bitrate, or use Generic SDO manually"
                    .to_string(),
            );
        }
    }

    Ok(steps)
}

/// Read current node ID / bitrate from device (profile-specific OD).
pub fn build_read_config_steps(profile: DeviceProfile, node_id: u8) -> Vec<ConfigStep> {
    let mut steps = Vec::new();
    match profile {
        DeviceProfile::BaumerEam300b => {
            steps.push(ConfigStep {
                delay_ms: 0,
                label: "Read 0x2101:00 node ID".to_string(),
                command: sdo_upload(node_id, 0x2101, 0x00),
            });
            steps.push(ConfigStep {
                delay_ms: 8,
                label: "Read 0x2100:00 baudrate".to_string(),
                command: sdo_upload(node_id, 0x2100, 0x00),
            });
        }
        DeviceProfile::Sensy => {
            steps.push(ConfigStep {
                delay_ms: 0,
                label: "Read 0x2000:00 node ID".to_string(),
                command: sdo_upload(node_id, 0x2000, 0x00),
            });
            steps.push(ConfigStep {
                delay_ms: 8,
                label: "Read 0x2001:00 baudrate".to_string(),
                command: sdo_upload(node_id, 0x2001, 0x00),
            });
        }
        DeviceProfile::Rfc4800 => {
            steps.push(ConfigStep {
                delay_ms: 0,
                label: "Read 0x2000:00 node ID".to_string(),
                command: sdo_upload(node_id, 0x2000, 0x00),
            });
            steps.push(ConfigStep {
                delay_ms: 8,
                label: "Read 0x2001:00 bit rate".to_string(),
                command: sdo_upload(node_id, 0x2001, 0x00),
            });
        }
        _ => {
            steps.push(ConfigStep {
                delay_ms: 0,
                label: "Read 0x1018:01 vendor ID".to_string(),
                command: sdo_upload(node_id, 0x1018, 0x01),
            });
        }
    }
    steps
}

/// CAN frame preview for a [`WriteCommand`] (matches `driver` encoding).
pub fn write_command_raw(cmd: &WriteCommand) -> String {
    match cmd {
        WriteCommand::SendSync => "0x080 SYNC".to_string(),
        WriteCommand::SendNmt { node_id, command } => {
            format_can_frame(0x000, &[*command as u8, *node_id])
        }
        WriteCommand::SendRaw { cob_id, data } | WriteCommand::SendPdo { cob_id, data } => {
            format_can_frame((cob_id & 0x7FF) as u16, data)
        }
        WriteCommand::SendSdoDownload {
            node_id,
            index,
            subindex,
            data,
        } => {
            let cob_id = 0x600 + u16::from(*node_id);
            match build_sdo_download_bytes(*index, *subindex, data) {
                Some(bytes) => format_can_frame(cob_id, &bytes),
                None => format!(
                    "SDO download node {node_id} 0x{index:04X}:{subindex:02X} (segmented, >4 B)"
                ),
            }
        }
        WriteCommand::SendSdoUpload {
            node_id,
            index,
            subindex,
        } => {
            let cob_id = 0x600 + u16::from(*node_id);
            format_can_frame(
                cob_id,
                &[
                    0x40,
                    (index & 0x00FF) as u8,
                    (index >> 8) as u8,
                    *subindex,
                    0,
                    0,
                    0,
                    0,
                ],
            )
        }
        WriteCommand::SendLssCommand { command, data } => {
            let mut msg = vec![0u8; 8];
            msg[0] = *command;
            let copy_len = data.len().min(7);
            msg[1..1 + copy_len].copy_from_slice(&data[..copy_len]);
            format_can_frame(0x7E5, &msg)
        }
        WriteCommand::ConfigureTpdo1Statusword { node_id } => {
            format!("TPDO1 setup script for node {node_id} (multiple SDO/NMT frames)")
        }
    }
}

fn format_can_frame(cob_id: u16, data: &[u8]) -> String {
    let mut bytes = data.to_vec();
    while bytes.len() < 8 {
        bytes.push(0);
    }
    bytes.truncate(8);
    format!(
        "0x{cob_id:03X} [{}]",
        bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn build_sdo_download_bytes(index: u16, subindex: u8, data: &[u8]) -> Option<[u8; 8]> {
    if data.len() > 4 {
        return None;
    }
    let n = (4 - data.len()) as u8;
    let ccs = 0x20 | (n << 2) | 0x03;
    let mut out = [0u8; 8];
    out[0] = ccs;
    out[1..3].copy_from_slice(&index.to_le_bytes());
    out[3] = subindex;
    out[4..4 + data.len()].copy_from_slice(data);
    Some(out)
}

/// Human-readable preview of steps.
pub fn steps_preview(steps: &[ConfigStep]) -> String {
    steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s.label))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Spawn async execution of a configuration script.
pub fn spawn_config_script(
    write_sender: tokio::sync::mpsc::Sender<WriteCommand>,
    steps: Vec<ConfigStep>,
) {
    tokio::spawn(async move {
        for step in steps {
            if step.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(step.delay_ms)).await;
            }
            if write_sender.send(step.command).await.is_err() {
                log::error!("Config script: channel closed at step {}", step.label);
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdo_upload_raw_preview() {
        let raw = write_command_raw(&WriteCommand::SendSdoUpload {
            node_id: 5,
            index: 0x1018,
            subindex: 1,
        });
        assert!(raw.starts_with("0x605"));
        assert!(raw.contains("40"));
    }

    #[test]
    fn baumer_node_id_steps() {
        let steps = build_change_node_id_steps(DeviceProfile::BaumerEam300b, 5, 10).unwrap();
        assert_eq!(steps.len(), 3);
        assert!(steps[0].label.contains("0x2101"));
    }

    #[test]
    fn node_id_no_change_returns_empty() {
        let steps = build_change_node_id_steps(DeviceProfile::BaumerEam300b, 7, 7).unwrap();
        assert!(steps.is_empty());
    }

    #[test]
    fn bitrate_no_change_returns_empty() {
        let steps =
            build_change_bitrate_steps(DeviceProfile::Sensy, 3, 250_000, Some(250_000)).unwrap();
        assert!(steps.is_empty());
    }

    #[test]
    fn sensy_bitrate_mapping() {
        assert_eq!(sensy_bitrate_code(250_000), Some(0x04));
        let steps =
            build_change_bitrate_steps(DeviceProfile::Sensy, 3, 250_000, Some(125_000)).unwrap();
        assert!(steps[0].label.contains("0x2001"));
    }

    #[test]
    fn resolve_auto_vendor() {
        assert_eq!(
            resolve_profile(DeviceProfile::Auto, Some(VENDOR_SENSY)),
            DeviceProfile::Sensy
        );
    }
}
