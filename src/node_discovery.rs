//! Shared node discovery and identity-probe constants (Seakite-style bootup scan).

use crate::node_scan::NodeScan;
use std::collections::BTreeSet;

/// Boot-up / heartbeat listen window after NMT ResetNode.
pub const SCAN_LISTEN_MS: u64 = 1750;
/// Wait after `ip link` bitrate change before NMT.
pub const SCAN_WAIT_LINK_MS: u64 = 400;
/// Per-SDO response timeout during identity probe.
pub const SCAN_SDO_TIMEOUT_MS: u64 = 450;

/// Full identity read (Scan bus).
pub const IDENTITY_SDO_FULL: &[(u16, u8)] = &[
    (0x1018, 0x01),
    (0x1000, 0x00),
    (0x1018, 0x02),
    (0x1018, 0x03),
    (0x1018, 0x04),
];

/// Vendor SDO only — fallback when boot-up found nothing.
pub const IDENTITY_SDO_VENDOR: &[(u16, u8)] = &[(0x1018, 0x01)];

/// Union boot-up detections at `bitrate` with passive node IDs (pre-scan snapshot).
pub fn union_scan_targets(
    node_scan: &NodeScan,
    bitrate: Option<u32>,
    passive_ids: &[u8],
) -> Vec<u8> {
    let mut ids: BTreeSet<u8> = passive_ids.iter().copied().collect();
    if let Some(bps) = bitrate {
        for id in node_scan.node_ids_sorted() {
            if node_scan
                .get(id)
                .is_some_and(|n| n.detected_bitrates.contains(&bps))
            {
                ids.insert(id);
            }
        }
    }
    ids.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_scan::NodeScan;

    #[test]
    fn union_includes_passive_and_bootup() {
        let mut scan = NodeScan::default();
        scan.mark_detected_at_bitrate(7, 250_000);
        let targets = union_scan_targets(&scan, Some(250_000), &[3]);
        assert!(targets.contains(&3));
        assert!(targets.contains(&7));
    }
}
