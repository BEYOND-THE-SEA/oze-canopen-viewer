use crate::{
    bitrate::RatesData,
    bus_stats::BusStats,
    chart::{self, Chart},
    driver::{Control, ControlCommand, State, WriteCommand},
    filter::GlobalFilter,
    filter_panel::FilterPanel,
    message_cached::MessageCached,
    message_sender::MessageSender,
    node_scan::NodeScan,
    remote_connection::{CleanupInfo, LocalCannelloniClient, RemoteConnection, RemoteSetupStatus, DEFAULT_CANNELLONI_PORT},
    theme::{theme, OZON_GRAY, OZON_PINK},
    viewer::Viewer,
};
use egui::{emath::Numeric, Button, Layout, TextEdit, Ui};
use oze_canopen::{
    canopen::RxMessageToStringFormat,
    interface::{CanOpenInfo, Connection},
};
use std::{cell::RefCell, collections::VecDeque, io::Write, process::{Command, Stdio}, rc::Rc, sync::Arc, sync::mpsc as std_mpsc};
use tokio::{
    sync::{watch, mpsc, Mutex},
    time::Instant,
};
use tokio::time::Duration;

const MESSAGES_COUNT: usize = 4096;

/// Standard CANopen bitrates
const CANOPEN_BITRATES: &[(u32, &str)] = &[
    (10_000, "10 kbit/s"),
    (20_000, "20 kbit/s"),
    (50_000, "50 kbit/s"),
    (125_000, "125 kbit/s"),
    (250_000, "250 kbit/s"),
    (500_000, "500 kbit/s"),
    (800_000, "800 kbit/s"),
    (1_000_000, "1 Mbit/s"),
];

/// Connection mode: local CAN interface or remote via SSH
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionMode {
    #[default]
    Local,
    Remote,
}

/// Action pending password confirmation (for local connections only)
#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingAction {
    Connect,
    Disconnect,
}

#[derive(Clone, Debug)]
struct InterfaceDetails {
    scope: &'static str,
    name: String,
    state: String,
    interface_type: String,
    bitrate: Option<String>,
    extras: Vec<String>,
}

pub struct Gui {
    data: VecDeque<MessageCached>,
    driver: watch::Receiver<State>,
    viewer: Viewer,
    chart: chart::Chart,
    last: Instant,
    fps: VecDeque<f64>,
    bus_load_history: VecDeque<f64>,
    bus_stats: BusStats,
    global_filter: Rc<RefCell<GlobalFilter>>,
    filter_panel: FilterPanel,
    message_sender: MessageSender,
    node_scan: NodeScan,
    active_scan_running: bool,
    active_scan_started_at: Option<Instant>,
    active_scan_expected_end_at: Option<Instant>,

    format: RxMessageToStringFormat,

    can_name_raw: String,
    selected_bitrate: Option<u32>,

    info: CanOpenInfo,

    connection: Connection,
    stopped: bool,
    driver_ctrl: watch::Sender<Control>,
    write_sender: mpsc::Sender<WriteCommand>,
    bitrate: Arc<Mutex<RatesData>>,
    last_seen_index: Option<u64>,

    // Connection state
    is_interface_up: bool,

    // Password popup state
    show_password_popup: bool,
    password_input: String,
    config_status: Option<Result<String, String>>,
    pending_action: Option<PendingAction>,

    // Local sudo password (for CAN and vcan0 operations)
    local_sudo_password: String,

    // Interface diagnostics
    local_interface_details: Option<InterfaceDetails>,
    remote_interface_details: Option<InterfaceDetails>,

    // Remote connection state
    connection_mode: ConnectionMode,
    remote_ssh_host: String,
    remote_ssh_user: String,
    remote_ssh_password: String,
    remote_can_interface: String,
    remote_setup_status: RemoteSetupStatus,
    is_remote_connected: bool,
    remote_status_receiver: Option<std_mpsc::Receiver<RemoteSetupStatus>>,
    remote_starting_client_at: Option<Instant>,
    
    // UI toggles
    show_stats_panel: bool,
}

impl Gui {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        driver: watch::Receiver<State>,
        driver_ctrl: watch::Sender<Control>,
        bitrate: Arc<Mutex<RatesData>>,
        write_sender: mpsc::Sender<WriteCommand>,
    ) -> Self {
        theme(&cc.egui_ctx);

        let global_filter = Rc::new(RefCell::new(GlobalFilter::default()));
        let connection_data = driver_ctrl.subscribe().borrow().connection.clone();
        
        // Default to "can0" if no interface name is provided
        let can_name_raw = if connection_data.can_name.is_empty() {
            "can0".to_string()
        } else {
            connection_data.can_name.clone()
        };
        
        // Default to 250 kbit/s if no bitrate is provided (most common CANopen bitrate)
        let selected_bitrate = connection_data.bitrate.or(Some(250_000));

        let local_interface_details = Self::query_local_interface_details(&can_name_raw);

        Self {
            fps: VecDeque::new(),
            bus_load_history: VecDeque::new(),
            bus_stats: BusStats::new(),
            data: VecDeque::new(),
            info: CanOpenInfo::default(),
            connection: connection_data,
            format: RxMessageToStringFormat::Hex,
            viewer: Viewer::new(global_filter.clone()),
            filter_panel: FilterPanel::new(global_filter.clone()),
            message_sender: MessageSender::new(write_sender.clone()),
            node_scan: NodeScan::default(),
            active_scan_running: false,
            active_scan_started_at: None,
            active_scan_expected_end_at: None,
            last: Instant::now(),
            chart: Chart::new(bitrate.clone()),
            stopped: false,
            global_filter,
            can_name_raw,
            selected_bitrate,
            driver_ctrl,
            driver,
            write_sender,
            bitrate,
            last_seen_index: None,
            is_interface_up: false,
            show_password_popup: false,
            password_input: String::new(),
            config_status: None,
            pending_action: None,
            local_sudo_password: String::new(),
            local_interface_details,
            remote_interface_details: None,
            // Remote connection state
            connection_mode: ConnectionMode::Local,
            remote_ssh_host: String::new(),
            remote_ssh_user: String::new(),
            remote_ssh_password: String::new(),
            remote_can_interface: "can0".to_string(),
            remote_setup_status: RemoteSetupStatus::Idle,
            is_remote_connected: false,
            remote_status_receiver: None,
            remote_starting_client_at: None,
            // UI toggles
            show_stats_panel: false,
        }
    }

    fn apply_remote_connected_state(&mut self) {
        self.is_remote_connected = true;

        // Store cleanup info for signal handlers
        CleanupInfo::store(CleanupInfo {
            ssh_host: self.remote_ssh_host.clone(),
            ssh_user: self.remote_ssh_user.clone(),
            ssh_password: self.remote_ssh_password.clone(),
            port: DEFAULT_CANNELLONI_PORT,
        });

        // Connect the viewer to vcan0
        // Use selected bitrate for stats calculation (bus occupation)
        self.connection.can_name = "vcan0".to_string();
        self.connection.bitrate = self.selected_bitrate;
        self.send_driver_control();
        self.refresh_local_interface_details("vcan0");
        self.refresh_remote_interface_details();

        self.remote_setup_status = RemoteSetupStatus::Connected;
        self.remote_starting_client_at = None;
        self.remote_status_receiver = None;
    }

    fn send_driver_control(&self) {
        let _ = self.driver_ctrl.send(Control {
            command: if self.stopped {
                ControlCommand::Stop
            } else {
                ControlCommand::Process
            },
            connection: self.connection.clone(),
        });
    }

    fn get_data_from_driver(&mut self) -> bool {
        let driver = self.driver.borrow();
        let now = Instant::now();
        
        for i in &driver.data {
            // Use last_seen_index to filter already processed messages
            if let Some(last_index) = self.last_seen_index {
                if i.index <= last_index {
                    continue;
                }
            }

            // Update last seen index
            self.last_seen_index = Some(i.index);

            // Update bus statistics
            self.bus_stats.on_message(i.msg.msg.cob_id, now);

            // Passive node scan should not depend on UI filters
            self.node_scan.update_from_message(i);
            
            if !self.global_filter.borrow().filter(i) {
                self.data.push_front(i.clone());
            }
        }

        while self.data.len() > MESSAGES_COUNT {
            self.data.pop_back();
        }

        self.info = driver.info.clone();

        driver.exit_signal
    }

    fn calc_fps(&mut self) -> f64 {
        let fps = 1.0 / self.last.elapsed().as_secs_f64();
        self.last = Instant::now();

        self.fps.push_back(fps);

        let fps = self.fps.iter().sum::<f64>() / self.fps.len().to_f64();
        while self.fps.len() > usize::from_f64(fps.round()) * 5 {
            self.fps.pop_front();
        }

        fps.round()
    }

    fn calc_bus_load(&mut self) -> Option<f64> {
        use tokio::runtime::Handle;
        
        // Always update message rate and COB-ID rates (independent of bitrate)
        self.bus_stats.calculate_msg_rate();
        self.bus_stats.calculate_cob_id_rates(Instant::now());
        
        // Bus load calculation requires configured bitrate
        if let Some(configured_bitrate) = self.connection.bitrate {
            let rates = Handle::current().block_on(async {
                self.bitrate.lock().await.clone()
            });
            
            if let Some(last_rate) = rates.last() {
                let current_bps = last_rate[1];
                let percentage = (current_bps / f64::from(configured_bitrate)) * 100.0;
                let clamped_percentage = percentage.min(100.0).max(0.0);
                
                // Add to history
                self.bus_load_history.push_back(clamped_percentage);
                
                // Keep a sliding window of 50 samples
                while self.bus_load_history.len() > 50 {
                    self.bus_load_history.pop_front();
                }
                
                // Calculate sliding average
                if !self.bus_load_history.is_empty() {
                    let avg = self.bus_load_history.iter().sum::<f64>() / self.bus_load_history.len() as f64;
                    
                    // Update bus statistics with load
                    self.bus_stats.update_load(avg);
                    
                    return Some(avg);
                }
            }
        }
        None
    }
    
    
    fn show_stats_content(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            
            // Top COB-IDs
            ui.label("🏆 Most Frequent COB-IDs:");
            ui.separator();
            
            let top_cobs = self.bus_stats.get_top_cob_ids(10);
            if top_cobs.is_empty() {
                ui.label("No data yet");
            } else {
                egui::Grid::new("top_cob_ids")
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label("COB-ID");
                        ui.label("Rate");
                        ui.end_row();
                        
                        for (cob_id, rate) in top_cobs {
                            ui.label(format!("0x{:03X}", cob_id));
                            if rate >= 1.0 {
                                ui.label(format!("{:.1} Hz", rate));
                            } else {
                                ui.label(format!("{:.2} Hz", rate));
                            }
                            ui.end_row();
                        }
                    });
            }
            
            ui.separator();

            // Observed nodes + active scan
            ui.horizontal(|ui| {
                ui.label("🧭 Observed nodes:");

                if self.active_scan_running {
                    ui.separator();
                    ui.weak("Bus scan running...");
                }

                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    let scan_btn = ui.add_enabled(!self.active_scan_running, Button::new("Scan bus"));
                    if scan_btn
                        .on_hover_text("Sends SDO requests (0x1000/0x1018) to discover silent nodes")
                        .clicked()
                    {
                        self.start_active_scan();
                    }
                });
            });

            // Stop scan when expected time passed (best-effort).
            if self.active_scan_running {
                if let Some(end_at) = self.active_scan_expected_end_at {
                    if Instant::now() >= end_at {
                        self.active_scan_running = false;
                        self.active_scan_started_at = None;
                        self.active_scan_expected_end_at = None;
                    }
                }
            }

            egui::Grid::new("observed_nodes")
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Node ID");
                    ui.label("Frames");
                    ui.label("Last seen");
                    ui.label("Types");
                    ui.label("State");
                    ui.label("Profile");
                    ui.label("Vendor");
                    ui.label("Product");
                    ui.end_row();

                    let now = Instant::now();
                    let mut any = false;
                    for n in self.node_scan.values() {
                        any = true;
                        ui.monospace(format!("{}", n.node_id));
                        ui.monospace(format!("{}", n.frames));
                        let last_seen = n.last_seen.map(|t| now.duration_since(t).as_secs_f32());
                        if let Some(s) = last_seen {
                            ui.monospace(format!("{s:.1}s"));
                        } else {
                            ui.monospace("-");
                        }
                        ui.label(n.types_string());
                        ui.label(
                            n.heartbeat_state
                                .map(|s| s.as_str().to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        );
                        ui.label(n.profile_string());
                        ui.label(n.vendor_string());
                        ui.monospace(n.product_string());
                        ui.end_row();
                    }

                    if !any {
                        ui.weak("No nodes observed yet");
                        ui.end_row();
                    }
                });
            
            ui.separator();

            // Bus occupation details
            ui.label("🔋 Bus Occupation Details:");
            ui.separator();
            ui.label(format!("• Current: {:.2}%", self.bus_stats.current_load()));
            ui.label(format!("• Peak: {:.2}%", self.bus_stats.peak_load()));
            ui.label(format!("• Average: {:.2}%", self.bus_stats.avg_load()));
            
            ui.separator();
            
            // Timing details
            ui.label("⏱️ Timing Details:");
            ui.separator();
            if let Some(min_gap) = self.bus_stats.min_gap() {
                ui.label(format!("• Min gap: {:.3} ms", min_gap));
            }
            if let Some(max_gap) = self.bus_stats.max_gap() {
                ui.label(format!("• Max gap: {:.1} ms", max_gap));
            }
            if let Some(avg_gap) = self.bus_stats.avg_gap() {
                ui.label(format!("• Avg gap: {:.3} ms", avg_gap));
            }
            if let Some(jitter) = self.bus_stats.jitter() {
                ui.label(format!("• Jitter (σ): ±{:.3} ms", jitter));
            }
            
            ui.separator();
            
            // Message rate details
            ui.label("📬 Message Rate Details:");
            ui.separator();
            ui.label(format!("• Current: {:.1} msg/s", self.bus_stats.current_msg_rate()));
            ui.label(format!("• Peak: {:.1} msg/s", self.bus_stats.peak_msg_rate()));
            ui.label(format!("• Average: {:.2} msg/s", self.bus_stats.avg_msg_rate()));
            ui.label(format!("• Total: {}", self.bus_stats.total_messages()));
        });
    }

    fn start_active_scan(&mut self) {
        const THROTTLE_MS: u64 = 8;
        const NODES: u64 = 127;
        const REQS_PER_NODE: u64 = 5; // 0x1000:00 and 0x1018:01..04
        let total_reqs = NODES * REQS_PER_NODE;
        let estimated_ms = total_reqs * THROTTLE_MS + 500;

        self.node_scan.clear();
        self.active_scan_running = true;
        let started_at = Instant::now();
        self.active_scan_started_at = Some(started_at);
        self.active_scan_expected_end_at = Some(started_at + Duration::from_millis(estimated_ms));

        let sender = self.write_sender.clone();
        tokio::spawn(async move {
            for node_id in 1u8..=127u8 {
                let cob_id = 0x600u32 + u32::from(node_id);
                let requests: &[(u16, u8)] = &[
                    (0x1000, 0x00),
                    (0x1018, 0x01),
                    (0x1018, 0x02),
                    (0x1018, 0x03),
                    (0x1018, 0x04),
                ];

                for (index, sub) in requests {
                    let data = vec![
                        0x40,
                        (*index & 0x00FF) as u8,
                        (*index >> 8) as u8,
                        *sub,
                        0,
                        0,
                        0,
                        0,
                    ];
                    let _ = sender
                        .send(WriteCommand::SendRaw { cob_id, data })
                        .await;
                    tokio::time::sleep(Duration::from_millis(THROTTLE_MS)).await;
                }
            }
        });
    }

    fn sudo_error_message(stderr: &str) -> String {
        let lowercase = stderr.to_lowercase();
        let filtered_error: String = stderr
            .lines()
            .filter(|line| !line.contains("[sudo]") && !line.contains("password"))
            .collect::<Vec<_>>()
            .join("\n");

        if lowercase.contains("incorrect password")
            || lowercase.contains("sorry, try again")
            || lowercase.contains("a password is required")
            || lowercase.contains("authentication")
        {
            "Local sudo authentication failed".to_string()
        } else if filtered_error.trim().is_empty() {
            "Local sudo command failed".to_string()
        } else {
            filtered_error
        }
    }

    fn is_sudo_auth_error(error: &str) -> bool {
        let lowercase = error.to_lowercase();
        lowercase.contains("sudo authentication failed")
            || lowercase.contains("incorrect password")
            || lowercase.contains("sorry, try again")
            || lowercase.contains("authentication")
    }

    fn is_interface_missing_error(error: &str) -> bool {
        let lowercase = error.to_lowercase();
        lowercase.contains("cannot find device")
            || lowercase.contains("no such device")
            || lowercase.contains("does not exist")
    }

    fn query_local_interface_details(interface_name: &str) -> Option<InterfaceDetails> {
        if interface_name.is_empty() {
            return None;
        }

        let output = Command::new("ip")
            .args(["-details", "link", "show", interface_name])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let details = String::from_utf8_lossy(&output.stdout).to_string();
        Some(Self::parse_interface_details("Local", interface_name, &details))
    }

    fn parse_interface_details(
        scope: &'static str,
        interface_name: &str,
        details: &str,
    ) -> InterfaceDetails {
        let mut state = "UNKNOWN".to_string();
        let mut bitrate = None;
        let mut extras = Vec::new();
        let mut interface_type = if interface_name.starts_with("vcan") {
            "vcan".to_string()
        } else {
            "can".to_string()
        };

        if let Some(first_line) = details.lines().next() {
            if let Some(flags) = first_line.split('<').nth(1).and_then(|part| part.split('>').next())
            {
                extras.push(format!("flags {}", flags));
            }

            let tokens: Vec<_> = first_line.split_whitespace().collect();
            for window in tokens.windows(2) {
                match window {
                    ["mtu", value] => extras.push(format!("mtu {}", value)),
                    ["qdisc", value] => extras.push(format!("qdisc {}", value)),
                    ["state", value] => state = (*value).to_string(),
                    _ => {}
                }
            }
        }

        for line in details.lines().skip(1) {
            let tokens: Vec<_> = line.split_whitespace().collect();
            for window in tokens.windows(2) {
                match window {
                    ["bitrate", value] => bitrate = Some((*value).to_string()),
                    ["link/can", _] if interface_name.starts_with("vcan") => {
                        interface_type = "vcan".to_string();
                    }
                    ["link/can", _] => interface_type = "can".to_string(),
                    _ => {}
                }
            }
        }

        InterfaceDetails {
            scope,
            name: interface_name.to_string(),
            state,
            interface_type,
            bitrate,
            extras,
        }
    }

    fn refresh_local_interface_details(&mut self, interface_name: &str) {
        self.local_interface_details = Self::query_local_interface_details(interface_name);
    }

    fn refresh_remote_interface_details(&mut self) {
        let remote = RemoteConnection::new(
            self.remote_ssh_host.clone(),
            self.remote_ssh_user.clone(),
            self.remote_ssh_password.clone(),
            self.remote_can_interface.clone(),
            self.selected_bitrate,
            DEFAULT_CANNELLONI_PORT,
        );

        let details = tokio::runtime::Handle::current()
            .block_on(async { remote.get_interface_details().await.ok() });
        self.remote_interface_details = details
            .map(|raw| Self::parse_interface_details("Remote", &self.remote_can_interface, &raw));
    }

    fn show_interface_details(ui: &mut Ui, details: &InterfaceDetails) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{} {}:", details.scope, details.name));
            ui.monospace(format!(
                "{} | type {}",
                details.state, details.interface_type
            ));
            if let Some(bitrate) = &details.bitrate {
                ui.monospace(format!("bitrate {}", bitrate));
            }
            for extra in &details.extras {
                ui.monospace(extra);
            }
        });
    }

    fn show_connection_feedback(&self, ui: &mut Ui) {
        let has_interface_details =
            self.local_interface_details.is_some() || self.remote_interface_details.is_some();
        let show_remote_status =
            !matches!(self.remote_setup_status, RemoteSetupStatus::Idle | RemoteSetupStatus::Connected);

        if !has_interface_details && !show_remote_status {
            return;
        }

        ui.separator();

        if let Some(details) = &self.local_interface_details {
            Self::show_interface_details(ui, details);
        }

        if let Some(details) = &self.remote_interface_details {
            Self::show_interface_details(ui, details);
        }

        if show_remote_status {
            ui.add_space(4.0);
            match &self.remote_setup_status {
                RemoteSetupStatus::Failed(msg) => {
                    ui.colored_label(egui::Color32::RED, "Remote error:");
                    ui.horizontal_wrapped(|ui| {
                        ui.label(msg);
                    });
                }
                status => {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(egui::Color32::YELLOW, "Remote status:");
                        ui.label(status.to_string());
                    });
                }
            }
        }
    }

    fn open_local_password_popup(&mut self, action: PendingAction) {
        self.show_password_popup = true;
        self.password_input.clear();
        self.config_status = None;
        self.pending_action = Some(action);
    }

    fn start_local_action(&mut self, action: PendingAction) {
        if self.local_sudo_password.is_empty() {
            self.open_local_password_popup(action);
            return;
        }

        self.password_input = self.local_sudo_password.clone();
        self.config_status = None;
        self.pending_action = Some(action);
        self.execute_pending_action();
        if self.config_status.is_some() && !self.show_password_popup {
            self.show_password_popup = true;
        }
    }

    /// Run a command with sudo using password via stdin
    fn run_sudo_command(password: &str, args: &[&str]) -> Result<(), String> {
        let mut child = Command::new("sudo")
            .arg("-S") // Read password from stdin
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn sudo: {}", e))?;

        // Write password to stdin
        if let Some(mut stdin) = child.stdin.take() {
            writeln!(stdin, "{}", password)
                .map_err(|e| format!("Failed to write password: {}", e))?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| format!("Failed to wait for command: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Self::sudo_error_message(&stderr));
        }

        Ok(())
    }

    /// Configure the CAN interface using ip link commands with password
    fn configure_can_interface(can_name: &str, bitrate: Option<u32>, password: &str) -> Result<(), String> {
        // First, bring the interface down (ignore errors if already down)
        let _ = Self::run_sudo_command(password, &["ip", "link", "set", "down", can_name]);

        // Set the CAN bitrate if provided
        if let Some(br) = bitrate {
            let bitrate_str = br.to_string();
            Self::run_sudo_command(
                password,
                &["ip", "link", "set", can_name, "type", "can", "bitrate", &bitrate_str],
            ).map_err(|e| format!("Failed to set bitrate: {}", e))?;
            log::info!("CAN interface {} configured with bitrate {}", can_name, br);
        }

        // Bring the interface up
        Self::run_sudo_command(password, &["ip", "link", "set", "up", can_name])
            .map_err(|e| format!("Failed to bring interface up: {}", e))?;

        log::info!("CAN interface {} is now up", can_name);
        Ok(())
    }

    fn show_connect_ui(&mut self, ui: &mut Ui) {
        let is_connected = self.is_interface_up || self.is_remote_connected;
        
        // Connection mode selector (disabled when connected)
        ui.add_enabled_ui(!is_connected, |ui| {
            egui::ComboBox::from_id_salt("connection_mode")
                .selected_text(match self.connection_mode {
                    ConnectionMode::Local => "🖥️ Local",
                    ConnectionMode::Remote => "🌐 Remote",
                })
                .width(90.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(self.connection_mode == ConnectionMode::Local, "🖥️ Local").clicked() {
                        self.connection_mode = ConnectionMode::Local;
                    }
                    if ui.selectable_label(self.connection_mode == ConnectionMode::Remote, "🌐 Remote").clicked() {
                        self.connection_mode = ConnectionMode::Remote;
                    }
                });
        });

        ui.separator();

        match self.connection_mode {
            ConnectionMode::Local => self.show_local_connect_ui(ui),
            ConnectionMode::Remote => self.show_remote_connect_ui(ui),
        }
    }

    fn show_local_connect_ui(&mut self, ui: &mut Ui) {
        // Disable interface name and bitrate when connected
        let can_name_response = ui.add_enabled(
            !self.is_interface_up,
            TextEdit::singleline(&mut self.can_name_raw)
                .hint_text("can name")
                .desired_width(80.0),
        );

        // Bitrate dropdown (disabled when connected)
        let current_label = self.selected_bitrate
            .and_then(|br| CANOPEN_BITRATES.iter().find(|(val, _)| *val == br))
            .map(|(_, label)| *label)
            .unwrap_or("Select bitrate");
        
        ui.add_enabled_ui(!self.is_interface_up, |ui| {
            egui::ComboBox::from_id_salt("bitrate_selector")
                .selected_text(current_label)
                .width(100.0)
                .show_ui(ui, |ui| {
                    for (value, label) in CANOPEN_BITRATES {
                        let is_selected = self.selected_bitrate == Some(*value);
                        if ui.selectable_label(is_selected, *label).clicked() {
                            self.selected_bitrate = Some(*value);
                        }
                    }
                });
        });

        if self.is_interface_up {
            // Disconnect button
            if ui.button("🔌 Disconnect").clicked() {
                self.start_local_action(PendingAction::Disconnect);
            }
        } else {
            // Connect button
            let button_enabled = !self.can_name_raw.is_empty() && self.selected_bitrate.is_some();
            if ui
                .add_enabled(button_enabled, Button::new("🔌 Connect"))
                .clicked()
            {
                self.start_local_action(PendingAction::Connect);
            }

            if button_enabled
                && can_name_response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
            {
                self.start_local_action(PendingAction::Connect);
            }
        }
    }

    fn show_remote_connect_ui(&mut self, ui: &mut Ui) {
        let is_connecting = matches!(
            self.remote_setup_status,
            RemoteSetupStatus::TestingConnection
                | RemoteSetupStatus::CheckingCannelloni
                | RemoteSetupStatus::DeployingCannelloni
                | RemoteSetupStatus::ConfiguringCanInterface
                | RemoteSetupStatus::StartingServer
                | RemoteSetupStatus::CreatingVcan
                | RemoteSetupStatus::StartingClient
        );

        // SSH Host
        let ssh_host_response = ui.add_enabled(
            !self.is_remote_connected && !is_connecting,
            TextEdit::singleline(&mut self.remote_ssh_host)
                .hint_text("Host (IP or name)")
                .desired_width(160.0),
        ).on_hover_text("IP address or hostname of the remote machine (e.g., 192.168.0.166 or pc-bts3.local)");

        // SSH User
        let ssh_user_response = ui.add_enabled(
            !self.is_remote_connected && !is_connecting,
            TextEdit::singleline(&mut self.remote_ssh_user)
                .hint_text("User")
                .desired_width(100.0),
        ).on_hover_text("SSH username");

        // SSH Password
        let ssh_password_response = ui.add_enabled(
            !self.is_remote_connected && !is_connecting,
            TextEdit::singleline(&mut self.remote_ssh_password)
                .hint_text("Password")
                .password(true)
                .desired_width(120.0),
        ).on_hover_text("SSH password (also used for sudo on remote)");

        // Local sudo password (for vcan0 and local cannelloni client)
        let local_sudo_hint = if self.local_sudo_password.is_empty() {
            "Local sudo"
        } else {
            "Local sudo (cached)"
        };
        let local_sudo_response = ui.add_enabled(
            !self.is_remote_connected && !is_connecting,
            TextEdit::singleline(&mut self.local_sudo_password)
                .hint_text(local_sudo_hint)
                .password(true)
                .desired_width(120.0),
        ).on_hover_text("Local sudo password (used to create vcan0 and start the local cannelloni client)");

        // Remote CAN interface
        let remote_can_response = ui.add_enabled(
            !self.is_remote_connected && !is_connecting,
            TextEdit::singleline(&mut self.remote_can_interface)
                .hint_text("can0")
                .desired_width(60.0),
        ).on_hover_text("CAN interface on the remote machine (e.g., can0)");

        // Bitrate dropdown
        let current_label = self.selected_bitrate
            .and_then(|br| CANOPEN_BITRATES.iter().find(|(val, _)| *val == br))
            .map(|(_, label)| *label)
            .unwrap_or("Bitrate");
        
        ui.add_enabled_ui(!self.is_remote_connected && !is_connecting, |ui| {
            egui::ComboBox::from_id_salt("remote_bitrate_selector")
                .selected_text(current_label)
                .width(100.0)
                .show_ui(ui, |ui| {
                    for (value, label) in CANOPEN_BITRATES {
                        let is_selected = self.selected_bitrate == Some(*value);
                        if ui.selectable_label(is_selected, *label).clicked() {
                            self.selected_bitrate = Some(*value);
                        }
                    }
                });
        });

        // Connect/Disconnect button
        if self.is_remote_connected {
            if ui.button("🔌 Disconnect").clicked() {
                self.try_remote_disconnect();
            }
        } else if is_connecting {
            ui.add_enabled(false, Button::new("⏳ Connecting..."));
        } else {
            let button_enabled = !self.remote_ssh_host.is_empty()
                && !self.remote_ssh_user.is_empty()
                && !self.remote_ssh_password.is_empty()
                && !self.remote_can_interface.is_empty()
                && self.selected_bitrate.is_some()
                && !self.local_sudo_password.is_empty();

            if ui.add_enabled(button_enabled, Button::new("🔌 Connect")).clicked() {
                self.try_remote_connect();
            }

            let submit_from_keyboard = ssh_host_response.lost_focus()
                || ssh_user_response.lost_focus()
                || ssh_password_response.lost_focus()
                || local_sudo_response.lost_focus()
                || remote_can_response.lost_focus();

            if button_enabled
                && submit_from_keyboard
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
            {
                self.try_remote_connect();
            }
        }
    }

    fn show_password_popup(&mut self, ctx: &egui::Context) {
        if !self.show_password_popup {
            return;
        }

        let (title, description, action_label, password_hint) = match self.pending_action {
            Some(PendingAction::Connect) => (
                "🔐 Connect Interface",
                "Enter sudo password to configure CAN interface:",
                "✅ Connect",
                "Sudo password",
            ),
            Some(PendingAction::Disconnect) => (
                "🔐 Disconnect Interface",
                "Enter sudo password to bring down CAN interface:",
                "✅ Disconnect",
                "Sudo password",
            ),
            None => return,
        };

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(description);
                    ui.add_space(10.0);

                    // Only show password field if needed
                    if !password_hint.is_empty() {
                        ui.horizontal(|ui| {
                            if self.config_status.is_none()
                                && self.password_input.is_empty()
                                && !self.local_sudo_password.is_empty()
                            {
                                self.password_input = self.local_sudo_password.clone();
                            }

                            ui.label(format!("{}:", password_hint));
                            let response = ui.add(
                                TextEdit::singleline(&mut self.password_input)
                                    .password(true)
                                    .desired_width(200.0),
                            );
                            
                            // Focus the password field when popup opens
                            if self.config_status.is_none() {
                                response.request_focus();
                            }

                            let _ = response;
                        });
                    }

                    ui.add_space(10.0);

                    // Show status message if any
                    if let Some(ref status) = self.config_status {
                        match status {
                            Ok(msg) => {
                                ui.colored_label(egui::Color32::GREEN, msg);
                            }
                            Err(msg) => {
                                ui.colored_label(egui::Color32::RED, format!("❌ {}", msg));
                            }
                        }
                        ui.add_space(5.0);
                    }

                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.execute_pending_action();
                    }

                    ui.horizontal(|ui| {
                        if ui.button(action_label).clicked() {
                            self.execute_pending_action();
                        }

                        if ui.button("❌ Cancel").clicked() {
                            self.show_password_popup = false;
                            self.password_input.clear();
                            self.config_status = None;
                            self.pending_action = None;
                        }
                    });
                });
            });
    }

    fn execute_pending_action(&mut self) {
        match self.pending_action {
            Some(PendingAction::Connect) => {
                self.try_configure_and_connect();
            }
            Some(PendingAction::Disconnect) => {
                self.try_disconnect();
            }
            None => {}
        }
    }

    fn try_disconnect(&mut self) {
        match Self::disconnect_interface(&self.can_name_raw, &self.password_input) {
            Ok(()) => {
                log::info!("CAN interface disconnected successfully");
                self.is_interface_up = false;

                // Remember local sudo password for future operations (e.g. remote vcan0)
                self.local_sudo_password = self.password_input.clone();

                // Close popup and clear password
                self.show_password_popup = false;
                self.password_input.clear();
                self.config_status = None;
                self.pending_action = None;
                let can_name = self.can_name_raw.clone();
                self.refresh_local_interface_details(&can_name);
            }
            Err(e) => {
                if Self::is_interface_missing_error(&e) {
                    log::info!(
                        "CAN interface {} is missing; treating as disconnected",
                        self.can_name_raw
                    );
                    self.is_interface_up = false;

                    // Remember local sudo password for future operations (e.g. remote vcan0)
                    self.local_sudo_password = self.password_input.clone();

                    // Close popup and clear password
                    self.show_password_popup = false;
                    self.password_input.clear();
                    self.config_status = None;
                    self.pending_action = None;
                    let can_name = self.can_name_raw.clone();
                    self.refresh_local_interface_details(&can_name);
                    return;
                }

                log::error!("Failed to disconnect CAN interface: {}", e);
                let needs_reauth = Self::is_sudo_auth_error(&e);
                if needs_reauth {
                    self.local_sudo_password.clear();
                }

                if !self.show_password_popup && needs_reauth {
                    self.open_local_password_popup(PendingAction::Disconnect);
                    self.config_status = Some(Err(
                        "Stored sudo password was rejected. Please enter it again.".to_string(),
                    ));
                } else {
                    self.config_status = Some(Err(e));
                }
            }
        }
    }

    /// Bring down the CAN interface
    fn disconnect_interface(can_name: &str, password: &str) -> Result<(), String> {
        Self::run_sudo_command(password, &["ip", "link", "set", "down", can_name])
            .map_err(|e| format!("Failed to bring interface down: {}", e))?;

        log::info!("CAN interface {} is now down", can_name);
        Ok(())
    }

    fn try_configure_and_connect(&mut self) {
        let bitrate = self.selected_bitrate;

        // Configure the CAN interface with the provided password
        match Self::configure_can_interface(&self.can_name_raw, bitrate, &self.password_input) {
            Ok(()) => {
                log::info!("CAN interface configured successfully, connecting...");
                self.is_interface_up = true;
                
                // Connect after successful configuration
                self.connection.can_name = self.can_name_raw.clone();
                self.connection.bitrate = bitrate;
                self.send_driver_control();

                // Remember local sudo password for future operations (e.g. remote vcan0)
                self.local_sudo_password = self.password_input.clone();

                // Close the popup and clear password
                self.show_password_popup = false;
                self.password_input.clear();
                self.config_status = None;
                self.pending_action = None;
                let can_name = self.can_name_raw.clone();
                self.refresh_local_interface_details(&can_name);
            }
            Err(e) => {
                log::error!("Failed to configure CAN interface: {}", e);
                let needs_reauth = Self::is_sudo_auth_error(&e);
                if needs_reauth {
                    self.local_sudo_password.clear();
                }

                if !self.show_password_popup && needs_reauth {
                    self.open_local_password_popup(PendingAction::Connect);
                    self.config_status = Some(Err(
                        "Stored sudo password was rejected. Please enter it again.".to_string(),
                    ));
                } else {
                    self.config_status = Some(Err(e));
                }
            }
        }
    }

    fn try_remote_connect(&mut self) {
        // Create a channel for status updates
        let (tx, rx) = std_mpsc::channel();
        self.remote_status_receiver = Some(rx);
        self.remote_interface_details = None;
        
        // Start the remote connection process
        self.remote_setup_status = RemoteSetupStatus::TestingConnection;
        
        // Clone values for the async task
        let ssh_host = self.remote_ssh_host.clone();
        let ssh_user = self.remote_ssh_user.clone();
        let ssh_password = self.remote_ssh_password.clone();
        let can_interface = self.remote_can_interface.clone();
        let bitrate = self.selected_bitrate;
        let port = DEFAULT_CANNELLONI_PORT;
        let local_sudo_password = self.local_sudo_password.clone();
        
        // Spawn the async setup task (non-blocking)
        tokio::spawn(async move {
            let result = Self::setup_remote_connection_async_with_status(
                ssh_host,
                ssh_user,
                ssh_password,
                can_interface,
                bitrate,
                port,
                local_sudo_password,
                tx.clone(),
            )
            .await;
            
            // Send final status
            match result {
                Ok(()) => {
                    let _ = tx.send(RemoteSetupStatus::Connected);
                }
                Err(e) => {
                    let _ = tx.send(RemoteSetupStatus::Failed(e));
                }
            }
        });
        // Note: Don't clear SSH password so user can retry connection if needed
    }
    
    /// Poll the remote connection status channel and update state
    fn poll_remote_connection_status(&mut self) {
        // Collect status updates first (to avoid borrow issues)
        let mut updates = Vec::new();
        let mut should_clear_receiver = false;
        
        if let Some(ref rx) = self.remote_status_receiver {
            while let Ok(status) = rx.try_recv() {
                let is_final = matches!(&status, RemoteSetupStatus::Connected | RemoteSetupStatus::Failed(_));
                if is_final {
                    should_clear_receiver = true;
                }
                updates.push(status);
            }
        }
        
        // Process updates
        for status in updates {
            log::info!("Remote connection status: {:?}", status);
            
            match &status {
                RemoteSetupStatus::Connected => {
                    self.apply_remote_connected_state();
                }
                RemoteSetupStatus::Failed(msg) => {
                    self.is_remote_connected = false;
                    CleanupInfo::clear();
                    self.remote_interface_details = None;
                    if Self::is_sudo_auth_error(msg) {
                        self.local_sudo_password.clear();
                    }
                    self.remote_starting_client_at = None;
                }
                RemoteSetupStatus::StartingClient => {
                    self.remote_starting_client_at = Some(Instant::now());
                }
                _ => {}
            }
            
            self.remote_setup_status = status;
        }
        
        // Clear receiver if we got a final status
        if should_clear_receiver {
            self.remote_status_receiver = None;
        }

        // Watchdog: if we get stuck at "StartingClient" but the local client is running,
        // treat the setup as connected to avoid UI getting stuck forever.
        if !self.is_remote_connected
            && self.remote_setup_status == RemoteSetupStatus::StartingClient
        {
            if let Some(started_at) = self.remote_starting_client_at {
                if started_at.elapsed().as_secs_f32() > 2.0 {
                    let remote_host = self.remote_ssh_host.clone();
                    let port = DEFAULT_CANNELLONI_PORT;
                    let rt = tokio::runtime::Handle::current();
                    let client_running = rt.block_on(async {
                        LocalCannelloniClient::check_client_running(&remote_host).await
                    });
                    match client_running {
                        Ok(true) => {
                            log::warn!(
                                "Remote setup stuck at StartingClient, but cannelloni client is running; promoting to Connected"
                            );
                            // Best-effort: verify the remote port is reachable (non-fatal)
                            let _ = rt.block_on(async {
                                let remote = RemoteConnection::new(
                                    self.remote_ssh_host.clone(),
                                    self.remote_ssh_user.clone(),
                                    self.remote_ssh_password.clone(),
                                    self.remote_can_interface.clone(),
                                    self.selected_bitrate,
                                    port,
                                );
                                remote.check_cannelloni_server_running().await
                            });
                            self.apply_remote_connected_state();
                        }
                        Ok(false) => {}
                        Err(e) => {
                            log::debug!("Failed to check cannelloni client: {}", e);
                        }
                    }
                }
            }
        }
    }

    async fn setup_remote_connection_async_with_status(
        ssh_host: String,
        ssh_user: String,
        ssh_password: String,
        can_interface: String,
        bitrate: Option<u32>,
        port: u16,
        local_sudo_password: String,
        status_tx: std_mpsc::Sender<RemoteSetupStatus>,
    ) -> Result<(), String> {
        // Create remote connection handler
        let remote = RemoteConnection::new(
            ssh_host.clone(),
            ssh_user,
            ssh_password,
            can_interface,
            bitrate,
            port,
        );

        // Step 1: Test SSH connection
        log::info!("Step 1/6: Testing SSH connection...");
        let _ = status_tx.send(RemoteSetupStatus::TestingConnection);
        remote.test_connection().await
            .map_err(|e| format!("Step 1 - SSH connection failed: {}", e))?;

        // Step 2: Check/install cannelloni
        log::info!("Step 2/6: Checking cannelloni on remote...");
        let _ = status_tx.send(RemoteSetupStatus::CheckingCannelloni);
        if !remote.check_cannelloni_installed().await
            .map_err(|e| format!("Step 2 - Failed to check cannelloni: {}", e))? 
        {
            log::info!("Step 2b/6: Deploying cannelloni to remote...");
            let _ = status_tx.send(RemoteSetupStatus::DeployingCannelloni);
            remote.deploy_cannelloni().await
                .map_err(|e| format!("Step 2 - Failed to deploy cannelloni: {}", e))?;
        }

        // Step 3: Configure CAN interface
        log::info!("Step 3/6: Configuring CAN interface on remote...");
        let _ = status_tx.send(RemoteSetupStatus::ConfiguringCanInterface);
        remote.setup_can_interface().await
            .map_err(|e| format!("Step 3 - CAN interface config failed: {}", e))?;

        // Step 4: Start cannelloni server
        log::info!("Step 4/6: Starting cannelloni server on remote...");
        let _ = status_tx.send(RemoteSetupStatus::StartingServer);
        remote.start_cannelloni_server().await
            .map_err(|e| format!("Step 4 - Cannelloni server failed: {}", e))?;

        // Step 5: Create local vcan0
        log::info!("Step 5/6: Creating local vcan0 interface...");
        let _ = status_tx.send(RemoteSetupStatus::CreatingVcan);
        LocalCannelloniClient::create_vcan_interface_with_password(&local_sudo_password)
            .await
            .map_err(|e| format!("Step 5 - Failed to create vcan0: {}", e))?;

        // Step 6: Start cannelloni client
        log::info!("Step 6/6: Starting cannelloni client...");
        let _ = status_tx.send(RemoteSetupStatus::StartingClient);
        LocalCannelloniClient::start_client_with_password(
            &ssh_host,
            port,
            &local_sudo_password,
        )
        .await
        .map_err(|e| format!("Step 6 - Cannelloni client failed: {}", e))?;

        Ok(())
    }

    fn try_remote_disconnect(&mut self) {
        log::info!("Disconnecting remote connection");
        
        let rt = tokio::runtime::Handle::current();
        
        // Stop local cannelloni client
        let cleanup_result = rt.block_on(async {
            LocalCannelloniClient::cleanup_with_password(false, &self.local_sudo_password).await
        });
        if let Err(err) = cleanup_result {
            if Self::is_sudo_auth_error(&err) {
                self.local_sudo_password.clear();
            }
        }
        
        // Stop remote cannelloni server
        let remote = RemoteConnection::new(
            self.remote_ssh_host.clone(),
            self.remote_ssh_user.clone(),
            self.remote_ssh_password.clone(),
            self.remote_can_interface.clone(),
            self.selected_bitrate,
            DEFAULT_CANNELLONI_PORT,
        );
        let _ = rt.block_on(async {
            remote.stop_cannelloni_server().await
        });
        
        // Reset state
        self.is_remote_connected = false;
        self.remote_setup_status = RemoteSetupStatus::Idle;
        self.remote_interface_details = None;
        self.refresh_local_interface_details("vcan0");
        
        // Clear cleanup info (signal handlers no longer need to cleanup)
        CleanupInfo::clear();

        // Note: Don't clear password so user can reconnect easily

        log::info!("Remote connection disconnected");
    }

    fn show_format_ui(&mut self, ui: &mut Ui) {
        if ui
            .selectable_label(self.format == RxMessageToStringFormat::Hex, "hex")
            .on_hover_text("Use HEX format to show message data")
            .clicked()
        {
            self.format = RxMessageToStringFormat::Hex;
        }
        if ui
            .selectable_label(self.format == RxMessageToStringFormat::Binary, "bin")
            .on_hover_text("Use binary format to show message data")
            .clicked()
        {
            self.format = RxMessageToStringFormat::Binary;
        }
        if ui
            .selectable_label(self.format == RxMessageToStringFormat::Ascii, "ascii")
            .on_hover_text("Use ASCII encoding to show message data")
            .clicked()
        {
            self.format = RxMessageToStringFormat::Ascii;
        }
    }

    fn show_connection_help(ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
                    ui.colored_label(OZON_PINK, "↑ You need to enter can name, i.e.");
                    ui.colored_label(OZON_GRAY, "can0");
                    ui.colored_label(OZON_PINK, "and optionally bitrate. If bitrate is set then link will go down, bitrate will be changed and then link will be set up.");
                });
        ui.colored_label(OZON_PINK, "Or your CAN interface is not connected properly");
        ui.label("Or you can execute program with arguments default values, for help execute:");
        ui.colored_label(OZON_GRAY, "oze-canopen-viewer --help");
    }
}

impl eframe::App for Gui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let fps = self.calc_fps();
        let connected =
            self.info.receiver_socket || self.info.transmitter_socket || self.info.rx_bits > 0;
        if self.get_data_from_driver() {
            println!("Gracefull shutdown");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            ctx.request_repaint();
            return;
        }

        // Poll remote connection status (non-blocking)
        self.poll_remote_connection_status();
        
        // Show password popup if needed
        self.show_password_popup(ctx);

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    self.show_connect_ui(ui);
                    ui.separator();

                    self.show_format_ui(ui);
                    ui.separator();

                    let rx_state = if self.info.receiver_socket {
                        "active"
                    } else {
                        "inactive"
                    };
                    ui.label(format!("CAN rx: {rx_state}")).on_hover_text(
                        "CAN receive socket is open; active after at least one frame was read. \
                         This is not a message counter.",
                    );

                    ui.separator();

                    let tx_state = if self.info.transmitter_socket {
                        "active"
                    } else {
                        "inactive"
                    };
                    ui.label(format!("CAN tx: {tx_state}")).on_hover_text(
                        "CAN transmit socket is open for sending frames. \
                         This is not a recent-send indicator.",
                    );

                    ui.separator();
                    ui.label(format!("Frames: {}", self.data.len())).on_hover_text(format!(
                        "Number of CAN frames kept in the list (max {MESSAGES_COUNT}). \
                         Cleared by CLEAR."
                    ));

                    ui.separator();
                    if let Some(bus_load) = self.calc_bus_load() {
                        let color = if bus_load > 80.0 {
                            egui::Color32::RED
                        } else if bus_load > 50.0 {
                            egui::Color32::YELLOW
                        } else {
                            egui::Color32::GREEN
                        };
                        ui.colored_label(color, format!("Bus: {:.1}%", bus_load));
                    }

                    ui.with_layout(Layout::right_to_left(egui::Align::RIGHT), |ui| {
                        ui.label(format!("{fps} FPS",));
                        ui.separator();
                        if ui.selectable_label(self.show_stats_panel, "📈")
                            .on_hover_text(if self.show_stats_panel { "Hide statistics" } else { "Show statistics" })
                            .clicked()
                        {
                            self.show_stats_panel = !self.show_stats_panel;
                        }
                    });
                });

                self.show_connection_feedback(ui);
            });

            if !connected {
                Self::show_connection_help(ui);
            }
        });

        self.viewer.message_row.format = self.format;

        // Left side panel for message sender
        egui::SidePanel::left("message_sender_panel")
            .resizable(true)
            .default_width(350.0)
            .min_width(300.0)
            .show(ctx, |ui| {
                ui.add_enabled_ui(connected, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.message_sender.ui(ui);
                    });
                });
            });
        
        // Right side panel for detailed stats (toggleable)
        if self.show_stats_panel {
            egui::SidePanel::right("stats_panel")
                .resizable(true)
                .default_width(250.0)
                .min_width(200.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("📈 Stats");
                        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✖").on_hover_text("Hide statistics").clicked() {
                                self.show_stats_panel = false;
                            }
                        });
                    });
                    ui.separator();
                    ui.add_enabled_ui(connected, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            self.show_stats_content(ui);
                        });
                    });
                });
        }
        
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_enabled_ui(connected, |ui| {
                // Chart at the top
                self.chart.ui(ui);
                ui.separator();
                
                // Filter panel
                self.filter_panel.update(ui);
                if self.stopped != self.filter_panel.stop {
                    self.stopped = self.filter_panel.stop;
                    self.send_driver_control();
                }
                if self.filter_panel.clear_requested {
                    self.filter_panel.clear_requested = false;
                    self.data.clear();
                    self.bus_stats.reset();
                    self.bus_load_history.clear();
                    self.node_scan.clear();
                }

                ui.separator();
                self.viewer.update(ui, &self.data);
            });
        });

        ctx.request_repaint();
    }
    
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Cleanup remote connection on app close
        if self.is_remote_connected {
            log::info!("App closing - cleaning up remote connection");
            self.try_remote_disconnect();
        }
    }
}
