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
}

#[derive(Debug, Default)]
pub struct NodeScan {
    nodes: BTreeMap<u8, ObservedNode>,
}

impl NodeScan {
    pub fn clear(&mut self) {
        self.nodes.clear();
    }

    pub fn values(&self) -> impl Iterator<Item = &ObservedNode> {
        self.nodes.values()
    }

    pub fn update_from_message(&mut self, msg: &MessageCached) {
        if !is_node_emitted_message(msg.msg.parsed_type) {
            return;
        }

        let Some(node_id) = msg.msg.parsed_node_id else {
            return;
        };

        let entry = self.nodes.entry(node_id).or_insert_with(|| ObservedNode {
            node_id,
            ..ObservedNode::default()
        });

        entry.frames += 1;
        entry.last_seen = Some(msg.get_timestamp());
        entry.last_cob_id = Some(msg.msg.msg.cob_id);
        entry.mark_type(msg.msg.parsed_type);

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

fn le_bytes_to_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || bytes.len() > 4 {
        return None;
    }
    let mut tmp = [0u8; 4];
    tmp[..bytes.len()].copy_from_slice(bytes);
    Some(u32::from_le_bytes(tmp))
}

