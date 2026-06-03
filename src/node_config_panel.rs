use crate::{
    driver::WriteCommand,
    message_cached::{MessageCached, RxMessageAdditional},
    node_config::{
        build_change_bitrate_steps, build_change_node_id_steps, build_read_config_steps,
        resolve_profile, spawn_config_script, write_command_raw, ConfigStep, DeviceProfile,
        CLI_BITRATES,
    },
    node_scan::{NodeScan, ObservedNode},
};
use egui::{Button, ComboBox, Layout, ScrollArea, TextEdit, Ui};
use oze_canopen::proto::sdo::ResponseData;
use std::collections::VecDeque;
use tokio::{sync::mpsc, time::Duration, time::Instant};

const LOG_MAX: usize = 80;
const OP_TIMEOUT_SECS: f32 = 2.0;
const ACTIVE_SCAN_THROTTLE_MS: u64 = 8;
/// Fixed height for the operation log strip at the bottom of the panel.
const OPERATION_LOG_HEIGHT: f32 = 88.0;

#[derive(Debug, Clone)]
struct PendingOp {
    node_id: u8,
    index: u16,
    subindex: u8,
    started_at: Instant,
    label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftPanelTab {
    Messages,
    NodesConfig,
}

pub struct NodeConfigPanel {
    pub left_tab: LeftPanelTab,
    pub selected_node_id: Option<u8>,
    profile: DeviceProfile,
    new_node_id: String,
    new_bitrate: u32,
    confirm_writes: bool,
    generic_index: String,
    generic_subindex: String,
    generic_data: String,
    operation_log: VecDeque<String>,
    pending_ops: Vec<PendingOp>,
    status_message: Option<String>,
    active_scan_running: bool,
    active_scan_expected_end_at: Option<Instant>,
    /// Set by UI when user clicks "Scan all bitrates"; consumed by Gui.
    pub request_multi_bitrate_scan: bool,
    /// Progress label while Gui runs multi-bitrate scan.
    pub multi_bitrate_status: Option<String>,
    /// When true, any scan is in progress (active SDO or multi-bitrate).
    pub any_scan_running: bool,
}

impl Default for NodeConfigPanel {
    fn default() -> Self {
        Self {
            left_tab: LeftPanelTab::Messages,
            selected_node_id: None,
            profile: DeviceProfile::Auto,
            new_node_id: String::from("2"),
            new_bitrate: 250_000,
            confirm_writes: false,
            generic_index: String::from("2000"),
            generic_subindex: String::from("00"),
            generic_data: String::from("02"),
            operation_log: VecDeque::new(),
            pending_ops: Vec::new(),
            status_message: None,
            active_scan_running: false,
            active_scan_expected_end_at: None,
            request_multi_bitrate_scan: false,
            multi_bitrate_status: None,
            any_scan_running: false,
        }
    }
}

impl NodeConfigPanel {
    pub fn show_left_tabs(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.left_tab, LeftPanelTab::Messages, "Messages");
            ui.selectable_value(
                &mut self.left_tab,
                LeftPanelTab::NodesConfig,
                "Nodes & Config",
            );
        });
        ui.separator();
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        connected: bool,
        node_scan: &mut NodeScan,
        write_sender: &mpsc::Sender<WriteCommand>,
        local_can_scan_enabled: bool,
    ) {
        self.poll_active_scan();
        self.any_scan_running =
            self.active_scan_running || self.multi_bitrate_status.is_some();

        let total_h = ui.available_height().max(200.0);
        let top_max = (total_h - OPERATION_LOG_HEIGHT - 28.0).max(120.0);

        ui.vertical(|ui| {
            ui.set_min_height(total_h);
            ui.set_max_height(total_h);

            ScrollArea::vertical()
                .id_salt("nodes_config_top")
                .max_height(top_max)
                .show(ui, |ui| {
                    self.draw_top_section(
                        ui,
                        connected,
                        node_scan,
                        write_sender,
                        local_can_scan_enabled,
                    );
                });

            self.show_operation_log(ui);
        });
    }

    fn draw_top_section(
        &mut self,
        ui: &mut Ui,
        connected: bool,
        node_scan: &mut NodeScan,
        write_sender: &mpsc::Sender<WriteCommand>,
        local_can_scan_enabled: bool,
    ) {
        ui.heading("Nodes & Config");
        ui.separator();

        if self.any_scan_running {
            let status = if let Some(ref st) = self.multi_bitrate_status {
                st.as_str()
            } else if self.active_scan_running {
                "Bus scan running…"
            } else {
                ""
            };
            if !status.is_empty() {
                ui.add(
                    egui::Label::new(egui::RichText::new(status).weak().size(11.0))
                        .wrap()
                        .truncate(),
                );
            }
        }

        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        local_can_scan_enabled
                            && connected
                            && !self.any_scan_running,
                        Button::new("Scan all bitrates"),
                    )
                    .on_hover_text(
                        "Cycle 50k–500k: reconfigure can0, NMT ResetNode broadcast, boot-up listen, SDO identity read. Disturbs the bus briefly.",
                    )
                    .clicked()
                {
                    self.request_multi_bitrate_scan = true;
                }
                if ui
                    .add_enabled(connected && !self.any_scan_running, Button::new("Scan bus"))
                    .on_hover_text("SDO read 0x1000 / 0x1018 for nodes 1–127 (non-destructive)")
                    .clicked()
                {
                    self.start_active_scan(write_sender.clone(), node_scan);
                }
            });
        });

        self.draw_node_selector(ui, node_scan);

        ui.separator();

        let Some(node_id) = self.selected_node_id else {
            ui.weak("Select a node to configure.");
            return;
        };

        let resolved = resolve_profile(
            self.profile,
            node_scan
                .get(node_id)
                .and_then(|n| n.identity.vendor_id),
        );

        if let Some(n) = node_scan.get(node_id) {
            self.show_node_details(ui, n);
        }

        ui.separator();
        ui.label("Device profile:");
        ComboBox::from_id_salt("device_profile_combo")
            .selected_text(self.profile.label())
            .show_ui(ui, |ui| {
                for p in DeviceProfile::ALL {
                    ui.selectable_value(&mut self.profile, p, p.label());
                }
            });
        ui.weak(format!("Resolved for actions: {}", resolved.label()));
        if resolved.is_dangerous() {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Warning: LSS global affects all LSS-capable nodes on the bus.",
            );
        }

        ui.separator();

        let node_id_steps = new_node_id_parsed(&self.new_node_id)
            .and_then(|new_id| build_change_node_id_steps(resolved, node_id, new_id));

        ui.horizontal(|ui| {
            ui.label("Change node ID to:");
            ui.add(
                TextEdit::singleline(&mut self.new_node_id)
                    .desired_width(48.0)
                    .hint_text("1-127"),
            );
            if ui
                .add_enabled(connected && self.confirm_writes, Button::new("Apply"))
                .on_hover_text("Requires confirmation checkbox")
                .clicked()
            {
                if let Ok(steps) = node_id_steps.clone() {
                    self.run_steps(write_sender, steps, node_id);
                } else if let Err(e) = &node_id_steps {
                    self.push_log(format!("Error: {e}"));
                }
            }
        });
        let node_preview_summary = match &node_id_steps {
            Ok(_) => format!("node {node_id} → {}", self.new_node_id.trim()),
            Err(e) => format!("invalid: {e}"),
        };
        show_steps_collapsing(ui, "preview_node_id", &node_preview_summary, node_id_steps);

        let bitrate_steps = build_change_bitrate_steps(resolved, node_id, self.new_bitrate);

        ui.horizontal(|ui| {
            ui.label("Change device bitrate to:");
            ComboBox::from_id_salt("config_bitrate_combo")
                .selected_text(format_bitrate_option(self.new_bitrate))
                .width(110.0)
                .show_ui(ui, |ui| {
                    for (bps, label) in CLI_BITRATES {
                        ui.selectable_value(&mut self.new_bitrate, *bps, *label);
                    }
                    ui.selectable_value(&mut self.new_bitrate, 800_000, "800 kbit/s");
                    ui.selectable_value(&mut self.new_bitrate, 1_000_000, "1 Mbit/s");
                    ui.selectable_value(&mut self.new_bitrate, 20_000, "20 kbit/s");
                });
            if ui
                .add_enabled(connected && self.confirm_writes, Button::new("Apply"))
                .on_hover_text("Requires confirmation checkbox")
                .clicked()
            {
                match bitrate_steps.clone() {
                    Ok(steps) => self.run_steps(write_sender, steps, node_id),
                    Err(e) => self.push_log(format!("Error: {e}")),
                }
            }
        });
        ui.weak("After bitrate change, set the PC CAN interface to the same rate.");

        let bitrate_preview_summary = match &bitrate_steps {
            Ok(_) => format_bitrate_option(self.new_bitrate),
            Err(e) => format!("invalid: {e}"),
        };
        show_steps_collapsing(ui, "preview_bitrate", &bitrate_preview_summary, bitrate_steps);

        ui.checkbox(
            &mut self.confirm_writes,
            "I confirm persistent device writes",
        );

        if ui
            .add_enabled(connected, Button::new("Read device config (SDO)"))
            .clicked()
        {
            let steps = build_read_config_steps(resolved, node_id);
            self.run_steps(write_sender, steps, node_id);
        }

        ui.separator();
        ui.collapsing("Expert: generic SDO download", |ui| {
            ui.horizontal(|ui| {
                ui.label("Index:");
                ui.add(TextEdit::singleline(&mut self.generic_index).desired_width(70.0));
                ui.label("Sub:");
                ui.add(TextEdit::singleline(&mut self.generic_subindex).desired_width(40.0));
            });
            ui.horizontal(|ui| {
                ui.label("Data hex:");
                ui.add(TextEdit::singleline(&mut self.generic_data).desired_width(160.0));
            });
            if ui
                .add_enabled(connected && self.confirm_writes, Button::new("Send SDO download"))
                .clicked()
            {
                if let (Ok(index), Ok(sub), Ok(data)) = (
                    u16::from_str_radix(self.generic_index.trim().trim_start_matches("0x"), 16),
                    u8::from_str_radix(self.generic_subindex.trim().trim_start_matches("0x"), 16),
                    parse_hex(&self.generic_data),
                ) {
                    if data.len() <= 4 {
                        let _ = write_sender.try_send(WriteCommand::SendSdoDownload {
                            node_id,
                            index,
                            subindex: sub,
                            data,
                        });
                        self.track_op(node_id, index, sub, "Generic SDO download");
                        self.push_log(format!(
                            "Sent SDO download 0x{index:04X}:{sub:02X} to node {node_id}"
                        ));
                    } else {
                        self.push_log("Error: max 4 bytes for expedited download".to_string());
                    }
                } else {
                    self.push_log("Error: invalid index/subindex/data".to_string());
                }
            }
        });

        if let Some(msg) = &self.status_message {
            ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
        }
    }

    fn draw_node_selector(&mut self, ui: &mut Ui, node_scan: &NodeScan) {
        let ids = node_scan.node_ids_sorted();

        if ids.len() == 1 && self.selected_node_id != Some(ids[0]) {
            self.select_node(ids[0]);
        }

        ui.label("Observed node:");
        ui.horizontal(|ui| {
            let selected_label = self
                .selected_node_id
                .and_then(|id| node_scan.get(id).map(ObservedNode::selection_label))
                .unwrap_or_else(|| "— select —".to_string());

            ComboBox::from_id_salt("observed_node_combo")
                .selected_text(selected_label)
                .width(ui.available_width().min(320.0))
                .show_ui(ui, |ui| {
                    for id in &ids {
                        if let Some(n) = node_scan.get(*id) {
                            ui.selectable_value(
                                &mut self.selected_node_id,
                                Some(*id),
                                n.selection_label(),
                            );
                        }
                    }
                });

            ui.add_enabled_ui(!ids.is_empty(), |ui| {
                if ui.small_button("◀").on_hover_text("Previous node").clicked() {
                    self.cycle_node(&ids, -1);
                }
                if ui.small_button("▶").on_hover_text("Next node").clicked() {
                    self.cycle_node(&ids, 1);
                }
            });
        });

        ScrollArea::vertical()
            .id_salt("nodes_config_table")
            .max_height(100.0)
            .show(ui, |ui| {
                egui::Grid::new("nodes_config_observed")
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label("ID");
                        ui.label("Vendor");
                        ui.label("Profile");
                        ui.label("Bitrates");
                        ui.end_row();

                        if ids.is_empty() {
                            ui.weak("No nodes yet — wait for traffic or run a scan");
                            ui.end_row();
                            return;
                        }

                        for id in ids {
                            let Some(n) = node_scan.get(id) else {
                                continue;
                            };
                            let selected = self.selected_node_id == Some(id);
                            let row = ui.selectable_label(selected, format!("{id}"));
                            if row.clicked() {
                                self.select_node(id);
                            }
                            ui.add(egui::Label::new(n.vendor_string()).truncate());
                            ui.add(egui::Label::new(n.profile_string()).truncate());
                            ui.add(egui::Label::new(n.detected_bitrates_string()).truncate());
                            ui.end_row();
                        }
                    });
            });
    }

    fn select_node(&mut self, id: u8) {
        self.selected_node_id = Some(id);
        self.new_node_id = format!("{}", id.saturating_add(1).min(127));
    }

    fn cycle_node(&mut self, ids: &[u8], dir: i32) {
        if ids.is_empty() {
            return;
        }
        let current = self.selected_node_id.and_then(|s| ids.iter().position(|&x| x == s));
        let next = match current {
            None => 0,
            Some(i) => {
                let n = ids.len() as i32;
                (i as i32 + dir).rem_euclid(n) as usize
            }
        };
        self.select_node(ids[next]);
    }

    fn show_node_details(&self, ui: &mut Ui, n: &ObservedNode) {
        ui.label(format!("Vendor: {}", n.vendor_string()));
        ui.label(format!("Profile: {}", n.profile_string()));
        ui.label(format!("Product: {}", n.product_string()));
        ui.label(format!("Seen at: {}", n.detected_bitrates_string()));
        if let Some(r) = n.identity.revision {
            ui.label(format!("Revision: 0x{r:08X}"));
        }
        if let Some(s) = n.identity.serial {
            ui.label(format!("Serial: 0x{s:08X}"));
        }
    }

    fn show_operation_log(&self, ui: &mut Ui) {
        ui.separator();
        ui.label("Operation log:");
        ScrollArea::vertical()
            .id_salt("nodes_config_op_log")
            .auto_shrink([false, false])
            .max_height(OPERATION_LOG_HEIGHT)
            .show(ui, |ui| {
                if self.operation_log.is_empty() {
                    ui.weak("(no operations yet)");
                } else {
                    for line in self.operation_log.iter() {
                        ui.label(line);
                    }
                }
            });
    }

    fn run_steps(
        &mut self,
        write_sender: &mpsc::Sender<WriteCommand>,
        steps: Vec<ConfigStep>,
        node_id: u8,
    ) {
        for step in &steps {
            if let WriteCommand::SendSdoDownload {
                index,
                subindex,
                ..
            } = &step.command
            {
                self.track_op(node_id, *index, *subindex, &step.label);
            }
        }
        self.push_log(format!("Running {} step(s) on node {node_id}", steps.len()));
        spawn_config_script(write_sender.clone(), steps);
        self.status_message = Some("Configuration script started".to_string());
    }

    fn track_op(&mut self, node_id: u8, index: u16, subindex: u8, label: &str) {
        self.pending_ops.push(PendingOp {
            node_id,
            index,
            subindex,
            started_at: Instant::now(),
            label: label.to_string(),
        });
    }

    pub fn push_log(&mut self, line: String) {
        self.operation_log.push_front(line);
        while self.operation_log.len() > LOG_MAX {
            self.operation_log.pop_back();
        }
    }

    pub fn on_messages(&mut self, messages: &[MessageCached]) {
        let now = Instant::now();
        for msg in messages {
            if let RxMessageAdditional::SdoTx(resp) = &msg.additional {
                let node_id = msg.msg.parsed_node_id.unwrap_or(0);
                match &resp.resp {
                    ResponseData::Download(d) => {
                        self.resolve_pending(node_id, d.index, d.subindex, true, None, now);
                    }
                    ResponseData::Abort(a) => {
                        let reason = a.reason.to_str().to_string();
                        self.resolve_pending(
                            node_id,
                            a.index,
                            a.subindex,
                            false,
                            Some(reason),
                            now,
                        );
                    }
                    ResponseData::Upload(u) => {
                        if let oze_canopen::proto::sdo::UploadResponseData::DataExpedited(bytes) =
                            &u.data
                        {
                            let val = le_bytes_to_u32(bytes);
                            self.push_log(format!(
                                "SDO upload OK node {node_id} 0x{:04X}:{:02X} = 0x{val:08X}",
                                u.index, u.subindex
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }

        let mut timed_out: Vec<PendingOp> = Vec::new();
        self.pending_ops.retain(|op| {
            if now.duration_since(op.started_at).as_secs_f32() >= OP_TIMEOUT_SECS {
                timed_out.push(op.clone());
                false
            } else {
                true
            }
        });
        for op in timed_out {
            self.push_log(format!(
                "Timeout waiting for SDO response: {} (node {})",
                op.label, op.node_id
            ));
        }
    }

    fn resolve_pending(
        &mut self,
        node_id: u8,
        index: u16,
        subindex: u8,
        ok: bool,
        abort_reason: Option<String>,
        now: Instant,
    ) {
        if let Some(pos) = self
            .pending_ops
            .iter()
            .position(|op| op.node_id == node_id && op.index == index && op.subindex == subindex)
        {
            let op = self.pending_ops.remove(pos);
            let elapsed = now.duration_since(op.started_at).as_secs_f32();
            if ok {
                self.push_log(format!(
                    "OK: {} (node {}, {:.2}s)",
                    op.label, node_id, elapsed
                ));
                self.status_message = Some(format!("Success: {}", op.label));
            } else {
                self.push_log(format!(
                    "SDO abort: {} — {} (node {}, {:.2}s)",
                    op.label,
                    abort_reason.unwrap_or_else(|| "?".to_string()),
                    node_id,
                    elapsed
                ));
            }
        }
    }

    pub fn start_active_scan(
        &mut self,
        write_sender: mpsc::Sender<WriteCommand>,
        node_scan: &mut NodeScan,
    ) {
        const NODES: u64 = 127;
        const REQS_PER_NODE: u64 = 5;
        let estimated_ms = NODES * REQS_PER_NODE * ACTIVE_SCAN_THROTTLE_MS + 500;

        node_scan.clear();
        self.active_scan_running = true;
        self.active_scan_expected_end_at =
            Some(Instant::now() + Duration::from_millis(estimated_ms));
        self.push_log("Active bus scan started".to_string());

        tokio::spawn(async move {
            for node_id in 1u8..=127u8 {
                let requests: &[(u16, u8)] = &[
                    (0x1000, 0x00),
                    (0x1018, 0x01),
                    (0x1018, 0x02),
                    (0x1018, 0x03),
                    (0x1018, 0x04),
                ];
                for (index, sub) in requests {
                    let _ = write_sender
                        .send(WriteCommand::SendSdoUpload {
                            node_id,
                            index: *index,
                            subindex: *sub,
                        })
                        .await;
                    tokio::time::sleep(Duration::from_millis(ACTIVE_SCAN_THROTTLE_MS)).await;
                }
            }
        });
    }

    fn poll_active_scan(&mut self) {
        if self.active_scan_running {
            if let Some(end) = self.active_scan_expected_end_at {
                if Instant::now() >= end {
                    self.active_scan_running = false;
                    self.active_scan_expected_end_at = None;
                    self.push_log("Active bus scan finished".to_string());
                }
            }
        }
    }

    pub fn clear_on_buffer_clear(&mut self) {
        self.pending_ops.clear();
    }

    pub fn take_multi_bitrate_request(&mut self) -> bool {
        let v = self.request_multi_bitrate_scan;
        self.request_multi_bitrate_scan = false;
        v
    }
}

fn new_node_id_parsed(s: &str) -> Result<u8, String> {
    let id: u8 = s
        .trim()
        .parse()
        .map_err(|_| "Invalid node ID".to_string())?;
    if crate::node_config::is_valid_node_id(id) {
        Ok(id)
    } else {
        Err("Node ID must be between 1 and 127".to_string())
    }
}

fn parse_hex(s: &str) -> Result<Vec<u8>, ()> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.len() % 2 != 0 {
        return Err(());
    }
    let mut out = Vec::new();
    for i in (0..cleaned.len()).step_by(2) {
        out.push(u8::from_str_radix(&cleaned[i..i + 2], 16).map_err(|_| ())?);
    }
    Ok(out)
}

fn le_bytes_to_u32(bytes: &[u8]) -> u32 {
    let mut tmp = [0u8; 4];
    let n = bytes.len().min(4);
    tmp[..n].copy_from_slice(&bytes[..n]);
    u32::from_le_bytes(tmp)
}

fn format_bitrate_option(bps: u32) -> String {
    for (rate, label) in CLI_BITRATES {
        if *rate == bps {
            return (*label).to_string();
        }
    }
    match bps {
        800_000 => "800 kbit/s".to_string(),
        1_000_000 => "1 Mbit/s".to_string(),
        20_000 => "20 kbit/s".to_string(),
        _ => format!("{bps} bit/s"),
    }
}

fn show_steps_collapsing(
    ui: &mut egui::Ui,
    id_salt: &str,
    target_summary: &str,
    steps: Result<Vec<ConfigStep>, String>,
) {
    let header = match &steps {
        Ok(s) => format!("Commands ({target_summary}) — {} step(s)", s.len()),
        Err(e) => format!("Commands ({target_summary}) — {e}"),
    };

    egui::CollapsingHeader::new(header)
        .id_salt(id_salt)
        .show(ui, |ui| match steps {
            Ok(s) => {
                for (i, step) in s.iter().enumerate() {
                    let raw = write_command_raw(&step.command);
                    let line = if step.delay_ms > 0 {
                        format!("{raw} — wait {} ms", step.delay_ms)
                    } else {
                        raw
                    };
                    ui.horizontal(|ui| {
                        ui.monospace(format!("{}.", i + 1));
                        ui.vertical(|ui| {
                            ui.monospace(line);
                            ui.weak(&step.label);
                        });
                    });
                }
            }
            Err(e) => {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
        });
}
