use crate::{
    message_cached::{MessageCached, NmtState, RxMessageAdditional},
    vendor_ids::vendor_company_name,
};
use oze_canopen::{
    canopen::RxMessageType,
    proto::sdo::{ResponseData, UploadResponseData},
};
use std::collections::BTreeMap;
use tokio::time::Instant;

#[derive(Debug, Default, Clone)]
pub struct NodeIdentity {
    pub device_type: Option<u32>,
    pub cia_profile: Option<u16>,
    pub vendor_id: Option<u32>,
    pub vendor_name: Option<&'static str>,
    pub product_code: Option<u32>,
    pub revision: Option<u32>,
    pub serial: Option<u32>,
    // Manufacturer Device Name (0x1008) may be segmented; keep optional for later.
    pub device_name: Option<String>,
}

impl NodeIdentity {
    fn update_from_sdo_upload(&mut self, index: u16, subindex: u8, data: &[u8]) {
        let Some(v) = le_bytes_to_u32(data) else {
            return;
        };

        match (index, subindex) {
            (0x1000, 0x00) => {
                self.device_type = Some(v);
                self.cia_profile = Some((v & 0xFFFF) as u16);
            }
            (0x1018, 0x01) => {
                self.vendor_id = Some(v);
                self.vendor_name = vendor_company_name(v);
            }
            (0x1018, 0x02) => self.product_code = Some(v),
            (0x1018, 0x03) => self.revision = Some(v),
            (0x1018, 0x04) => self.serial = Some(v),
            _ => {}
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ObservedNode {
    pub node_id: u8,
    pub frames: u64,
    pub last_seen: Option<Instant>,
    pub last_cob_id: Option<u16>,
    pub types_mask: u16,
    pub heartbeat_state: Option<NmtState>,
    pub identity: NodeIdentity,
    /// Host CAN bitrates at which this node was seen (boot-up scan or traffic).
    pub detected_bitrates: Vec<u32>,
}

impl ObservedNode {
    fn mark_type(&mut self, t: RxMessageType) {
        let bit = match t {
            RxMessageType::Sync => 0,
            RxMessageType::Pdo => 1,
            RxMessageType::SdoRx => 2,
            RxMessageType::SdoTx => 3,
            RxMessageType::Nmt => 4,
            RxMessageType::Emcy => 5,
            RxMessageType::Guarding => 6,
            RxMessageType::Lss => 7,
            RxMessageType::Unknown => 15,
        };
        self.types_mask |= 1u16 << bit;
    }

    pub fn types_string(&self) -> String {
        let mut out = Vec::new();
        let push = |out: &mut Vec<&'static str>, bit: u16, name: &'static str| {
            if (self.types_mask & (1u16 << bit)) != 0 {
                out.push(name);
            }
        };
        push(&mut out, 0, "SYNC");
        push(&mut out, 1, "PDO");
        push(&mut out, 2, "SDO-RX");
        push(&mut out, 3, "SDO-TX");
        push(&mut out, 4, "NMT");
        push(&mut out, 5, "EMCY");
        push(&mut out, 6, "HB");
        push(&mut out, 7, "LSS");
        push(&mut out, 15, "UNK");
        out.join(",")
    }

    pub fn profile_string(&self) -> String {
        let Some(p) = self.identity.cia_profile else {
            return "Unknown".to_string();
        };
        match p {
            401 => "CiA 401".to_string(),
            402 => "CiA 402".to_string(),
            404 => "CiA 404".to_string(),
            406 => "CiA 406".to_string(),
            _ => format!("CiA {p}"),
        }
    }

    pub fn vendor_string(&self) -> String {
        if let Some(name) = self.identity.vendor_name {
            return name.to_string();
        }
        if let Some(id) = self.identity.vendor_id {
            return format!("Vendor 0x{id:08X}");
        }
        "-".to_string()
    }

    pub fn product_string(&self) -> String {
        if let Some(p) = self.identity.product_code {
            format!("0x{p:08X}")
        } else {
            "-".to_string()
        }
    }

    pub fn detected_bitrates_string(&self) -> String {
        if self.detected_bitrates.is_empty() {
            return "-".to_string();
        }
        self.detected_bitrates
            .iter()
            .map(|bps| format_bitrate_short(*bps))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn selection_label(&self) -> String {
        format!(
            "Node {} — {} — {}",
            self.node_id,
            self.vendor_string(),
            self.profile_string()
        )
    }
}

/// Bitrate to use for SDO/config when host rate does not match detection.
///
/// Returns `None` if host can stay as-is (no detection data, or host already in list).
pub fn preferred_operation_bitrate(node: &ObservedNode, host_bps: Option<u32>) -> Option<u32> {
    if node.detected_bitrates.is_empty() {
        return None;
    }
    if let Some(host) = host_bps {
        if node.detected_bitrates.contains(&host) {
            return None;
        }
    }
    Some(pick_detected_bitrate(&node.detected_bitrates))
}

fn pick_detected_bitrate(detected: &[u32]) -> u32 {
    if detected.len() == 1 {
        return detected[0];
    }
    if detected.contains(&250_000) {
        return 250_000;
    }
    *detected.iter().min().unwrap_or(&250_000)
}

fn format_bitrate_short(bps: u32) -> String {
    if bps >= 1_000_000 {
        format!("{}M", bps / 1_000_000)
    } else if bps >= 1000 {
        format!("{}k", bps / 1000)
    } else {
        format!("{bps}")
    }
}

/// COB-ID 0x700 + node_id (boot-up / heartbeat).
pub fn node_id_from_heartbeat_cob(cob_id: u16) -> Option<u8> {
    if (0x700..=0x77F).contains(&cob_id) {
        let nid = cob_id - 0x700;
        if (1..=127).contains(&nid) {
            return Some(nid as u8);
        }
    }
    None
}

#[derive(Debug, Default)]
pub struct NodeScan {
    nodes: BTreeMap<u8, ObservedNode>,
    /// While set (e.g. active SDO bus scan), tag every seen node with this host CAN bitrate.
    pub assumed_bitrate: Option<u32>,
    /// During discovery listen: tag boot-up frames with this bitrate.
    pub discovery_listen_bps: Option<u32>,
}

impl NodeScan {
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.assumed_bitrate = None;
        self.discovery_listen_bps = None;
    }

    /// Snapshot node IDs already seen from passive traffic (before an active scan).
    pub fn passive_node_ids(&self) -> Vec<u8> {
        self.node_ids_sorted()
    }

    /// Multi-bitrate scan: keep nodes, clear per-node bitrate tags only.
    pub fn prepare_multi_bitrate_scan(&mut self) {
        for node in self.nodes.values_mut() {
            node.detected_bitrates.clear();
        }
        self.assumed_bitrate = None;
        self.discovery_listen_bps = None;
    }

    pub fn set_discovery_listen(&mut self, bps: Option<u32>) {
        self.discovery_listen_bps = bps;
    }

    pub fn node_ids_sorted(&self) -> Vec<u8> {
        self.nodes.keys().copied().collect()
    }

    pub fn values(&self) -> impl Iterator<Item = &ObservedNode> {
        self.nodes.values()
    }

    pub fn get(&self, node_id: u8) -> Option<&ObservedNode> {
        self.nodes.get(&node_id)
    }

    pub fn mark_detected_at_bitrate(&mut self, node_id: u8, bitrate: u32) {
        let entry = self.nodes.entry(node_id).or_insert_with(|| ObservedNode {
            node_id,
            ..ObservedNode::default()
        });
        if !entry.detected_bitrates.contains(&bitrate) {
            entry.detected_bitrates.push(bitrate);
            entry.detected_bitrates.sort_unstable();
        }
    }

    /// Record boot-up / heartbeat on 0x700+n while scanning at `bitrate`.
    pub fn observe_bootup_frame(&mut self, cob_id: u16, bitrate: u32, nmt_state: Option<NmtState>) {
        let Some(node_id) = node_id_from_heartbeat_cob(cob_id) else {
            return;
        };
        self.mark_detected_at_bitrate(node_id, bitrate);
        if let Some(entry) = self.nodes.get_mut(&node_id) {
            entry.last_cob_id = Some(cob_id);
            if let Some(state) = nmt_state {
                entry.heartbeat_state = Some(state);
            }
            entry.mark_type(RxMessageType::Guarding);
        }
    }

    pub fn update_from_message(&mut self, msg: &MessageCached) {
        if !is_node_emitted_message(msg.msg.parsed_type) {
            return;
        }

        let Some(node_id) = msg.msg.parsed_node_id else {
            return;
        };

        if let Some(bps) = self.assumed_bitrate {
            self.mark_detected_at_bitrate(node_id, bps);
        }

        let entry = self.nodes.entry(node_id).or_insert_with(|| ObservedNode {
            node_id,
            ..ObservedNode::default()
        });

        entry.frames += 1;
        entry.last_seen = Some(msg.get_timestamp());
        entry.last_cob_id = Some(msg.msg.msg.cob_id);
        entry.mark_type(msg.msg.parsed_type);

        if let Some(nid) = node_id_from_heartbeat_cob(msg.msg.msg.cob_id) {
            if nid == node_id {
                if let RxMessageAdditional::Heartbeat(hb) = &msg.additional {
                    if hb.state == NmtState::BootUp {
                        entry.mark_type(RxMessageType::Guarding);
                    }
                }
            }
        }

        match &msg.additional {
            RxMessageAdditional::Heartbeat(hb) => {
                entry.heartbeat_state = Some(hb.state);
            }
            RxMessageAdditional::SdoTx(resp) => {
                if let ResponseData::Upload(upload) = &resp.resp {
                    if let UploadResponseData::DataExpedited(bytes) = &upload.data {
                        entry
                            .identity
                            .update_from_sdo_upload(upload.index, upload.subindex, bytes);
                    }
                }
            }
            _ => {}
        }
    }
}

fn is_node_emitted_message(t: RxMessageType) -> bool {
    matches!(
        t,
        RxMessageType::Pdo | RxMessageType::SdoTx | RxMessageType::Emcy | RxMessageType::Guarding
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_bitrate_empty_detection() {
        let node = ObservedNode::default();
        assert_eq!(preferred_operation_bitrate(&node, Some(250_000)), None);
    }

    #[test]
    fn preferred_bitrate_host_matches() {
        let mut node = ObservedNode::default();
        node.detected_bitrates = vec![125_000, 250_000];
        assert_eq!(preferred_operation_bitrate(&node, Some(250_000)), None);
    }

    #[test]
    fn preferred_bitrate_picks_when_mismatch() {
        let mut node = ObservedNode::default();
        node.detected_bitrates = vec![125_000];
        assert_eq!(preferred_operation_bitrate(&node, Some(250_000)), Some(125_000));
    }

    #[test]
    fn preferred_bitrate_prefers_250k_among_many() {
        let mut node = ObservedNode::default();
        node.detected_bitrates = vec![500_000, 125_000, 250_000];
        assert_eq!(preferred_operation_bitrate(&node, Some(50_000)), Some(250_000));
    }
}

fn le_bytes_to_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || bytes.len() > 4 {
        return None;
    }
    let mut tmp = [0u8; 4];
    tmp[..bytes.len()].copy_from_slice(bytes);
    Some(u32::from_le_bytes(tmp))
}

