use crate::{
    driver::WriteCommand,
    message_cached::{MessageCached, RxMessageAdditional},
    node_config::{
        build_change_bitrate_steps, build_change_node_id_steps, build_read_config_steps,
        resolve_profile, spawn_config_script, write_command_raw, ConfigStep, DeviceProfile,
        CLI_BITRATES,
    },
    node_scan::{preferred_operation_bitrate, NodeScan, ObservedNode},
};
use egui::{Button, ComboBox, Layout, ScrollArea, TextEdit, Ui};
use oze_canopen::proto::sdo::ResponseData;
use std::collections::VecDeque;
use tokio::{sync::mpsc, time::Duration, time::Instant};

const LOG_MAX: usize = 80;
const OP_TIMEOUT_SECS: f32 = 2.0;
const ACTIVE_SCAN_THROTTLE_MS: u64 = 8;
/// Fixed height for the operation log strip at the bottom of the panel.
const OPERATION_LOG_HEIGHT: f32 = 300.0;

/// Left side panel width (must match [`gui`](crate::gui) `SidePanel` settings).
pub const LEFT_PANEL_DEFAULT_WIDTH: f32 = 300.0;
pub const LEFT_PANEL_MIN_WIDTH: f32 = 100.0;
/// Soft cap for inner layout (combos/tables); SidePanel itself has no max_width.
pub const LEFT_PANEL_LAYOUT_MAX_WIDTH: f32 = 480.0;

/// Closed node combo width — fixed so content never pulls the panel to full window width.
const NODE_COMBO_WIDTH: f32 = 100.0;
const NODE_COMBO_MIN_WIDTH: f32 = 52.0;
const NODE_NAV_BUTTONS_WIDTH: f32 = 44.0;

/// Observed-node table column widths as fractions of table width (ID, Vendor, Profile, Bitrates).
const NODE_TABLE_COL_FRACS: [f32; 4] = [0.10, 0.44, 0.24, 0.22];

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

/// Host CAN context for node operations (bitrate alignment).
pub struct NodeConfigHostCtx {
    pub host_bitrate: Option<u32>,
    /// Local CAN up and sudo password available — required to change host bitrate.
    pub can_align_host_bitrate: bool,
}

/// Actions queued during UI; drained by [`Gui`](crate::gui::Gui) after the panel is drawn.
#[derive(Debug, Clone)]
pub enum NodeConfigAction {
    SetHostBitrate {
        bps: u32,
        node_id: u8,
        detected_summary: String,
    },
    RunConfigScript {
        steps: Vec<ConfigStep>,
        node_id: u8,
    },
    SendGenericSdoDownload {
        node_id: u8,
        index: u16,
        subindex: u8,
        data: Vec<u8>,
    },
}

pub struct NodeConfigPanel {
    pub left_tab: LeftPanelTab,
    pub selected_node_id: Option<u8>,
    profile: DeviceProfile,
    new_node_id: String,
    new_bitrate: u32,
    /// Last node ID value for which Apply was enabled or applied (debounce baseline).
    node_id_apply_baseline: String,
    bitrate_apply_baseline: u32,
    node_id_apply_after: Option<Instant>,
    bitrate_apply_after: Option<Instant>,
    /// Value last seen when the node-ID debounce timer was started (re-arm on edit).
    node_id_apply_armed_for: Option<String>,
    bitrate_apply_armed_for: Option<u32>,
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
    pending_actions: Vec<NodeConfigAction>,
}

impl Default for NodeConfigPanel {
    fn default() -> Self {
        Self {
            left_tab: LeftPanelTab::Messages,
            selected_node_id: None,
            profile: DeviceProfile::Auto,
            new_node_id: String::from("2"),
            new_bitrate: 250_000,
            node_id_apply_baseline: String::from("2"),
            bitrate_apply_baseline: 250_000,
            node_id_apply_after: None,
            bitrate_apply_after: None,
            node_id_apply_armed_for: None,
            bitrate_apply_armed_for: None,
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
            pending_actions: Vec::new(),
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

    pub fn take_pending_actions(&mut self) -> Vec<NodeConfigAction> {
        std::mem::take(&mut self.pending_actions)
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        connected: bool,
        node_scan: &mut NodeScan,
        write_sender: &mpsc::Sender<WriteCommand>,
        local_can_scan_enabled: bool,
        host: &NodeConfigHostCtx,
    ) {
        self.poll_active_scan(node_scan);
        self.any_scan_running =
            self.active_scan_running || self.multi_bitrate_status.is_some();

        let panel_w = ui.available_width().min(LEFT_PANEL_LAYOUT_MAX_WIDTH);

        ui.vertical(|ui| {
            let top_max = (ui.available_height() - OPERATION_LOG_HEIGHT - 12.0).max(80.0);

            ScrollArea::vertical()
                .id_salt("nodes_config_top")
                .auto_shrink([false, false])
                .max_height(top_max)
                .show(ui, |ui| {
                    self.draw_top_section(
                        ui,
                        panel_w,
                        connected,
                        node_scan,
                        write_sender,
                        local_can_scan_enabled,
                        host,
                    );
                });

            self.show_operation_log(ui);
        });
    }

    fn draw_top_section(
        &mut self,
        ui: &mut Ui,
        panel_w: f32,
        connected: bool,
        node_scan: &mut NodeScan,
        write_sender: &mpsc::Sender<WriteCommand>,
        local_can_scan_enabled: bool,
        host: &NodeConfigHostCtx,
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
                ui.label(egui::RichText::new(status).weak().size(11.0));
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
                    self.start_active_scan(write_sender.clone(), node_scan, host.host_bitrate);
                }
            });
        });

        self.draw_node_selector(ui, panel_w, node_scan);

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
            self.show_node_extra_details(ui, n, host.host_bitrate);
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Device profile:");
            ComboBox::from_id_salt("device_profile_combo")
                .selected_text(self.profile.label())
                .width(160.0)
                .show_ui(ui, |ui| {
                    for p in DeviceProfile::ALL {
                        ui.selectable_value(&mut self.profile, p, p.label());
                    }
                });
        });
        ui.weak(format!("Resolved for actions: {}", resolved.label()));
        if resolved.is_dangerous() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "Warning: LSS global affects all LSS-capable nodes on the bus.",
                    )
                    .color(egui::Color32::YELLOW),
                )
                .wrap(),
            );
        }

        if ui
            .add_enabled(connected, Button::new("Read device config (SDO)"))
            .on_hover_text(
                "Non-destructive SDO read of node ID / bitrate (profile-specific OD). Does not change the profile selector.",
            )
            .clicked()
        {
            let steps = build_read_config_steps(resolved, node_id);
            self.queue_with_bitrate_align(
                node_scan,
                node_id,
                host,
                NodeConfigAction::RunConfigScript { steps, node_id },
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
            let now = Instant::now();
            self.update_apply_debounce(now);
            let node_id_apply = connected
                && self.node_id_apply_ready(now)
                && matches!(&node_id_steps, Ok(s) if !s.is_empty());
            if ui
                .add_enabled(node_id_apply, Button::new("Apply"))
                .on_hover_text("Enabled 1 s after you change the node ID")
                .clicked()
            {
                if let Ok(steps) = node_id_steps.clone() {
                    self.queue_with_bitrate_align(node_scan, node_id, host, NodeConfigAction::RunConfigScript {
                        steps,
                        node_id,
                    });
                    self.commit_node_id_apply_baseline();
                } else if let Err(e) = &node_id_steps {
                    self.push_log(format!("Error: {e}"));
                }
            }
        });
        let node_preview_summary = format!("node {node_id} → {}", self.new_node_id.trim());
        show_steps_collapsing(ui, "preview_node_id", &node_preview_summary, node_id_steps);

        let current_bitrate = node_scan
            .get(node_id)
            .and_then(|n| inferred_device_bitrate(n, host.host_bitrate));
        let bitrate_steps =
            build_change_bitrate_steps(resolved, node_id, self.new_bitrate, current_bitrate);

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
            let now = Instant::now();
            self.update_apply_debounce(now);
            let bitrate_apply = connected
                && self.bitrate_apply_ready(now)
                && matches!(&bitrate_steps, Ok(s) if !s.is_empty());
            if ui
                .add_enabled(bitrate_apply, Button::new("Apply"))
                .on_hover_text("Enabled 1 s after you change the bitrate")
                .clicked()
            {
                match bitrate_steps.clone() {
                    Ok(steps) => {
                        self.queue_with_bitrate_align(
                            node_scan,
                            node_id,
                            host,
                            NodeConfigAction::RunConfigScript { steps, node_id },
                        );
                        self.commit_bitrate_apply_baseline();
                    }
                    Err(e) => self.push_log(format!("Error: {e}")),
                }
            }
        });
        ui.weak("After bitrate change, set the PC CAN interface to the same rate.");

        let bitrate_preview_summary = format_bitrate_option(self.new_bitrate);
        show_steps_collapsing(ui, "preview_bitrate", &bitrate_preview_summary, bitrate_steps);

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
                ui.add(TextEdit::singleline(&mut self.generic_data).desired_width(90.0));
            });
            if ui
                .add_enabled(connected, Button::new("Send SDO download"))
                .clicked()
            {
                if let (Ok(index), Ok(sub), Ok(data)) = (
                    u16::from_str_radix(self.generic_index.trim().trim_start_matches("0x"), 16),
                    u8::from_str_radix(self.generic_subindex.trim().trim_start_matches("0x"), 16),
                    parse_hex(&self.generic_data),
                ) {
                    if data.len() <= 4 {
                        self.queue_with_bitrate_align(
                            node_scan,
                            node_id,
                            host,
                            NodeConfigAction::SendGenericSdoDownload {
                                node_id,
                                index,
                                subindex: sub,
                                data,
                            },
                        );
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

    fn draw_node_selector(&mut self, ui: &mut Ui, panel_w: f32, node_scan: &NodeScan) {
        let ids = node_scan.node_ids_sorted();

        if ids.len() == 1 && self.selected_node_id != Some(ids[0]) {
            self.select_node(ids[0]);
        }

        ui.horizontal(|ui| {
            ui.label("Observed node:");
            let selected_label = self
                .selected_node_id
                .map(|id| format!("Node {id}"))
                .unwrap_or_else(|| "— select —".to_string());

            let spacing = ui.spacing().item_spacing.x;
            let reserved = NODE_NAV_BUTTONS_WIDTH + spacing + 4.0;
            let combo_width = (panel_w - reserved - 90.0)
                .clamp(NODE_COMBO_MIN_WIDTH, NODE_COMBO_WIDTH);

            ComboBox::from_id_salt("observed_node_combo")
                .selected_text(selected_label)
                .width(combo_width)
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
                let table_w = panel_w.max(1.0);
                let col_w = node_table_column_widths(table_w);
                let row_h = ui.spacing().interact_size.y;

                if ids.is_empty() {
                    grid_empty_row(ui, table_w);
                    return;
                }

                ui.horizontal(|ui| {
                    for (header, w) in ["ID", "Vendor", "Profile", "Bitrates"]
                        .into_iter()
                        .zip(col_w)
                    {
                        table_cell(ui, w, row_h, |ui| ui.strong(header));
                    }
                });
                ui.separator();

                for id in ids {
                    let Some(n) = node_scan.get(id) else {
                        continue;
                    };
                    let selected = self.selected_node_id == Some(id);
                    let frame = if selected {
                        egui::Frame::none()
                            .fill(ui.visuals().selection.bg_fill)
                            .inner_margin(egui::Margin::symmetric(2.0, 1.0))
                    } else {
                        egui::Frame::none()
                    };

                    let row = frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            table_cell(ui, col_w[0], row_h, |ui| {
                                ui.label(format!("{id}"));
                            });
                            table_cell(ui, col_w[1], row_h, |ui| {
                                ui.add(egui::Label::new(n.vendor_string()).truncate());
                            });
                            table_cell(ui, col_w[2], row_h, |ui| {
                                ui.add(egui::Label::new(n.profile_string()).truncate());
                            });
                            table_cell(ui, col_w[3], row_h, |ui| {
                                ui.add(egui::Label::new(n.detected_bitrates_string()).truncate());
                            });
                        });
                    });

                    if row.response.interact(egui::Sense::click()).clicked() {
                        self.select_node(id);
                    }
                    row.response.context_menu(|ui| {
                        if ui.button("Select").clicked() {
                            self.select_node(id);
                            ui.close_menu();
                        }
                    });
                }
            });
    }

    fn select_node(&mut self, id: u8) {
        self.selected_node_id = Some(id);
        self.new_node_id = format!("{id}");
        self.reset_apply_baselines();
    }

    fn reset_apply_baselines(&mut self) {
        self.node_id_apply_baseline = self.new_node_id.clone();
        self.bitrate_apply_baseline = self.new_bitrate;
        self.node_id_apply_after = None;
        self.bitrate_apply_after = None;
        self.node_id_apply_armed_for = None;
        self.bitrate_apply_armed_for = None;
    }

    fn update_apply_debounce(&mut self, now: Instant) {
        const DELAY: Duration = Duration::from_secs(1);
        if self.new_node_id != self.node_id_apply_baseline {
            if self.node_id_apply_armed_for.as_deref() != Some(self.new_node_id.as_str()) {
                self.node_id_apply_armed_for = Some(self.new_node_id.clone());
                self.node_id_apply_after = Some(now + DELAY);
            }
        } else {
            self.node_id_apply_armed_for = None;
            self.node_id_apply_after = None;
        }
        if self.new_bitrate != self.bitrate_apply_baseline {
            if self.bitrate_apply_armed_for != Some(self.new_bitrate) {
                self.bitrate_apply_armed_for = Some(self.new_bitrate);
                self.bitrate_apply_after = Some(now + DELAY);
            }
        } else {
            self.bitrate_apply_armed_for = None;
            self.bitrate_apply_after = None;
        }
    }

    fn node_id_apply_ready(&self, now: Instant) -> bool {
        self.node_id_apply_after
            .is_some_and(|t| now >= t)
    }

    fn bitrate_apply_ready(&self, now: Instant) -> bool {
        self.bitrate_apply_after
            .is_some_and(|t| now >= t)
    }

    fn commit_node_id_apply_baseline(&mut self) {
        self.node_id_apply_baseline = self.new_node_id.clone();
        self.node_id_apply_after = None;
        self.node_id_apply_armed_for = None;
    }

    fn commit_bitrate_apply_baseline(&mut self) {
        self.bitrate_apply_baseline = self.new_bitrate;
        self.bitrate_apply_after = None;
        self.bitrate_apply_armed_for = None;
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

    /// SDO fields and hints not shown in the observed-node table (Vendor/Profile/Bitrates are there).
    fn show_node_extra_details(&self, ui: &mut Ui, n: &ObservedNode, host_bitrate: Option<u32>) {
        let has_revision = n.identity.revision.is_some();
        let has_serial = n.identity.serial.is_some();
        let has_product = n.identity.product_code.is_some();
        let bitrate_hint = preferred_operation_bitrate(n, host_bitrate).is_some();

        if !has_revision && !has_serial && !has_product && !bitrate_hint {
            return;
        }

        ui.collapsing("More from SDO (0x1018 / 0x1000)", |ui| {
            if has_product {
                ui.label(format!("Product code: {}", n.product_string()));
            }
            if let Some(r) = n.identity.revision {
                ui.label(format!("Revision: 0x{r:08X}"));
            }
            if let Some(s) = n.identity.serial {
                ui.label(format!("Serial: 0x{s:08X}"));
            }
            if let Some(target) = preferred_operation_bitrate(n, host_bitrate) {
                ui.weak(format!(
                    "Apply / SDO will set host CAN to {} first.",
                    format_bitrate_option(target)
                ));
            }
        });
    }

    fn queue_with_bitrate_align(
        &mut self,
        node_scan: &NodeScan,
        node_id: u8,
        host: &NodeConfigHostCtx,
        action: NodeConfigAction,
    ) {
        if let Some(n) = node_scan.get(node_id) {
            if let Some(target) = preferred_operation_bitrate(n, host.host_bitrate) {
                if !host.can_align_host_bitrate {
                    self.push_log(
                        "Error: node was detected at another bitrate; local CAN with sudo password is required to align host bitrate"
                            .to_string(),
                    );
                    return;
                }
                self.pending_actions.push(NodeConfigAction::SetHostBitrate {
                    bps: target,
                    node_id,
                    detected_summary: n.detected_bitrates_string(),
                });
            }
        }
        self.pending_actions.push(action);
    }

    pub(crate) fn execute_action(
        &mut self,
        action: NodeConfigAction,
        write_sender: &mpsc::Sender<WriteCommand>,
    ) {
        match action {
            NodeConfigAction::SetHostBitrate { .. } => {}
            NodeConfigAction::RunConfigScript { steps, node_id } => {
                self.run_steps(write_sender, steps, node_id);
            }
            NodeConfigAction::SendGenericSdoDownload {
                node_id,
                index,
                subindex,
                data,
            } => {
                let _ = write_sender.try_send(WriteCommand::SendSdoDownload {
                    node_id,
                    index,
                    subindex,
                    data: data.clone(),
                });
                self.track_op(node_id, index, subindex, "Generic SDO download");
                self.push_log(format!(
                    "Sent SDO download 0x{index:04X}:{subindex:02X} to node {node_id}"
                ));
            }
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
        host_bitrate: Option<u32>,
    ) {
        const NODES: u64 = 127;
        const REQS_PER_NODE: u64 = 5;
        let estimated_ms = NODES * REQS_PER_NODE * ACTIVE_SCAN_THROTTLE_MS + 500;

        node_scan.clear();
        node_scan.assumed_bitrate = host_bitrate;
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

    fn poll_active_scan(&mut self, node_scan: &mut NodeScan) {
        if self.active_scan_running {
            if let Some(end) = self.active_scan_expected_end_at {
                if Instant::now() >= end {
                    self.active_scan_running = false;
                    self.active_scan_expected_end_at = None;
                    node_scan.assumed_bitrate = None;
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

fn node_table_column_widths(table_w: f32) -> [f32; 4] {
    let id_w = (table_w * NODE_TABLE_COL_FRACS[0]).clamp(24.0, 36.0);
    let mut vendor_w = table_w * NODE_TABLE_COL_FRACS[1];
    let mut profile_w = table_w * NODE_TABLE_COL_FRACS[2];
    let mut bitrates_w = table_w * NODE_TABLE_COL_FRACS[3];
    let used = id_w + vendor_w + profile_w + bitrates_w;
    if used > table_w {
        let scale = table_w / used;
        vendor_w *= scale;
        profile_w *= scale;
        bitrates_w *= scale;
    } else {
        vendor_w += table_w - used;
    }
    [id_w, vendor_w, profile_w, bitrates_w]
}

fn table_cell<R>(ui: &mut egui::Ui, width: f32, height: f32, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Center),
        add_contents,
    )
    .inner
}

fn grid_empty_row(ui: &mut egui::Ui, width: f32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("No nodes yet — wait for traffic or run a scan").weak(),
                )
                .wrap(),
            );
        },
    );
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

fn inferred_device_bitrate(node: &ObservedNode, host_bps: Option<u32>) -> Option<u32> {
    match node.detected_bitrates.as_slice() {
        [one] => Some(*one),
        [] => host_bps,
        _ => host_bps.filter(|h| node.detected_bitrates.contains(h)),
    }
}

fn show_steps_collapsing(
    ui: &mut egui::Ui,
    id_salt: &str,
    target_summary: &str,
    steps: Result<Vec<ConfigStep>, String>,
) {
    let header = match &steps {
        Ok(s) if s.is_empty() => format!("Commands ({target_summary}) — no changes"),
        Ok(s) => format!("Commands ({target_summary}) — {} step(s)", s.len()),
        Err(_) => format!("Commands ({target_summary})"),
    };

    egui::CollapsingHeader::new(header)
        .id_salt(id_salt)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match steps {
                Ok(s) if s.is_empty() => {
                    ui.weak("Nothing to apply — target already matches the current value.");
                }
                Ok(s) => {
                    for (i, step) in s.iter().enumerate() {
                        let raw = write_command_raw(&step.command);
                        let line = if step.delay_ms > 0 {
                            format!("{raw} — wait {} ms", step.delay_ms)
                        } else {
                            raw
                        };
                        ui.horizontal_top(|ui| {
                            ui.set_width(ui.available_width());
                            ui.monospace(format!("{}.", i + 1));
                            ui.vertical(|ui| {
                                ui.set_width(ui.available_width());
                                ui.add(
                                    egui::Label::new(egui::RichText::new(line).monospace())
                                        .wrap(),
                                );
                                ui.weak(&step.label);
                            });
                        });
                    }
                }
                Err(e) => {
                    ui.add(
                        egui::Label::new(egui::RichText::new(e).color(egui::Color32::LIGHT_RED))
                            .wrap(),
                    );
                }
            }
        });
}
