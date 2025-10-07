use crate::{
    bitrate::RatesData,
    bus_stats::BusStats,
    chart::{self, Chart},
    driver::{Control, ControlCommand, State, WriteCommand},
    filter::GlobalFilter,
    filter_panel::FilterPanel,
    message_cached::MessageCached,
    message_sender::MessageSender,
    pinned_filter::PinnedFilters,
    theme::{theme, OZON_GRAY, OZON_PINK},
    viewer::Viewer,
};
use egui::{emath::Numeric, Button, Layout, TextEdit, Ui};
use oze_canopen::{
    canopen::RxMessageToStringFormat,
    interface::{CanOpenInfo, Connection},
};
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Arc};
use tokio::{
    sync::{watch, mpsc, Mutex},
    time::Instant,
};

const MESSAGES_COUNT: usize = 4096;

pub struct Gui {
    data: VecDeque<MessageCached>,
    driver: watch::Receiver<State>,
    pinned_filters: PinnedFilters,
    viewer: Viewer,
    chart: chart::Chart,
    last: Instant,
    fps: VecDeque<f64>,
    bus_load_history: VecDeque<f64>,
    bus_stats: BusStats,
    global_filter: Rc<RefCell<GlobalFilter>>,
    filter_panel: FilterPanel,
    message_sender: MessageSender,

    format: RxMessageToStringFormat,

    can_name_raw: String,
    bitrate_raw: String,

    info: CanOpenInfo,

    connection: Connection,
    stopped: bool,
    driver_ctrl: watch::Sender<Control>,
    write_sender: mpsc::Sender<WriteCommand>,
    bitrate: Arc<Mutex<RatesData>>,
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
        let can_name_raw = connection_data.can_name.clone();
        let bitrate_raw = connection_data
            .bitrate
            .map(|b| b.to_string())
            .unwrap_or_default();

        Self {
            fps: VecDeque::new(),
            bus_load_history: VecDeque::new(),
            bus_stats: BusStats::new(),
            data: VecDeque::new(),
            pinned_filters: PinnedFilters::default(),
            info: CanOpenInfo::default(),
            connection: connection_data,
            format: RxMessageToStringFormat::Hex,
            viewer: Viewer::new(global_filter.clone()),
            filter_panel: FilterPanel::new(global_filter.clone()),
            message_sender: MessageSender::new(write_sender.clone()),
            last: Instant::now(),
            chart: Chart::new(bitrate.clone()),
            stopped: false,
            global_filter,
            can_name_raw,
            bitrate_raw,
            driver_ctrl,
            driver,
            write_sender,
            bitrate,
        }
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
            if let Some(last) = self.data.front() {
                if i.index <= last.index {
                    continue;
                }
            }

            // Update bus statistics
            self.bus_stats.on_message(i.msg.msg.cob_id, now);
            
            self.pinned_filters.push_data(i);
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
        
        if let Some(configured_bitrate) = self.connection.bitrate {
            let rates = Handle::current().block_on(async {
                self.bitrate.lock().await.clone()
            });
            
            if let Some(last_rate) = rates.last() {
                let current_bps = last_rate[1];
                let percentage = (current_bps / f64::from(configured_bitrate)) * 100.0;
                let clamped_percentage = percentage.min(100.0).max(0.0);
                
                // Ajouter à l'historique
                self.bus_load_history.push_back(clamped_percentage);
                
                // Garder une fenêtre glissante de 50 échantillons
                while self.bus_load_history.len() > 50 {
                    self.bus_load_history.pop_front();
                }
                
                // Calculer la moyenne glissante
                if !self.bus_load_history.is_empty() {
                    let avg = self.bus_load_history.iter().sum::<f64>() / self.bus_load_history.len() as f64;
                    
                    // Update bus statistics
                    self.bus_stats.update_load(avg);
                    self.bus_stats.calculate_msg_rate();
                    self.bus_stats.calculate_cob_id_rates(Instant::now());
                    
                    return Some(avg);
                }
            }
        }
        None
    }
    
    fn show_dashboard(&self, ui: &mut Ui) {
        use egui::Color32;
        
        ui.group(|ui| {
            ui.heading("📊 Bus Statistics");
            ui.separator();
            
            ui.horizontal(|ui| {
                // Occupation section
                ui.vertical(|ui| {
                    ui.label("🔋 Bus Occupation");
                    ui.horizontal(|ui| {
                        ui.label("Current:");
                        let color = if self.bus_stats.current_load() > 80.0 {
                            Color32::RED
                        } else if self.bus_stats.current_load() > 50.0 {
                            Color32::YELLOW
                        } else {
                            Color32::GREEN
                        };
                        ui.colored_label(color, format!("{:.1}%", self.bus_stats.current_load()));
                    });
                    ui.label(format!("Peak: {:.1}%", self.bus_stats.peak_load()));
                    ui.label(format!("Average: {:.1}%", self.bus_stats.avg_load()));
                });
                
                ui.separator();
                
                // Message rate section
                ui.vertical(|ui| {
                    ui.label("📬 Message Rate");
                    ui.label(format!("Current: {:.0} msg/s", self.bus_stats.current_msg_rate()));
                    ui.label(format!("Peak: {:.0} msg/s", self.bus_stats.peak_msg_rate()));
                    ui.label(format!("Average: {:.1} msg/s", self.bus_stats.avg_msg_rate()));
                });
                
                ui.separator();
                
                // Timing analysis section
                ui.vertical(|ui| {
                    ui.label("⏱️ Inter-Frame Timing");
                    if let Some(min_gap) = self.bus_stats.min_gap() {
                        ui.label(format!("Min: {:.2} ms", min_gap));
                    } else {
                        ui.label("Min: --");
                    }
                    if let Some(max_gap) = self.bus_stats.max_gap() {
                        ui.label(format!("Max: {:.1} ms", max_gap));
                    } else {
                        ui.label("Max: --");
                    }
                    if let Some(avg_gap) = self.bus_stats.avg_gap() {
                        ui.label(format!("Avg: {:.2} ms", avg_gap));
                    } else {
                        ui.label("Avg: --");
                    }
                });
                
                ui.separator();
                
                // Total messages
                ui.vertical(|ui| {
                    ui.label("📊 Totals");
                    ui.label(format!("Messages: {}", self.bus_stats.total_messages()));
                    if let Some(jitter) = self.bus_stats.jitter() {
                        ui.label(format!("Jitter: ±{:.2} ms", jitter));
                    } else {
                        ui.label("Jitter: --");
                    }
                });
            });
        });
    }
    
    fn show_stats_panel(&self, ui: &mut Ui) {
        ui.vertical(|ui| {
            ui.heading("📈 Detailed Stats");
            ui.separator();
            
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

    fn show_connect_ui(&mut self, ui: &mut Ui) {
        ui.add(
            TextEdit::singleline(&mut self.can_name_raw)
                .hint_text("can name")
                .desired_width(100.0),
        );

        ui.add(
            TextEdit::singleline(&mut self.bitrate_raw)
                .hint_text("bitrate")
                .desired_width(100.0),
        );
        let bitrate = self.bitrate_raw.parse::<u32>().ok();
        let button_enbled = !self.can_name_raw.is_empty()
            && ((bitrate.is_some()
                && bitrate.unwrap_or_default() <= 1_000_000
                && bitrate.unwrap_or_default() > 0)
                || self.bitrate_raw.is_empty());
        if ui
            .add_enabled(button_enbled, Button::new("🔌Connect"))
            .clicked()
        {
            self.connection.can_name = self.can_name_raw.clone();
            self.connection.bitrate = bitrate;
            self.send_driver_control();
        }
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

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                self.show_connect_ui(ui);
                ui.separator();

                self.show_format_ui(ui);
                ui.separator();

                ui.label(format!(
                    "rx {} tx {}",
                    self.info.receiver_socket, self.info.transmitter_socket,
                ));

                ui.separator();
                ui.label(format!("packets={}", self.data.len()));

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
                });
            });

            if !connected {
                Self::show_connection_help(ui);
            }
        });

        self.viewer.message_row.format = self.format;
        self.pinned_filters.message_row.format = self.format;
        
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
        
        // Right side panel for detailed stats
        egui::SidePanel::right("stats_panel")
            .resizable(true)
            .default_width(250.0)
            .min_width(200.0)
            .show(ctx, |ui| {
                ui.add_enabled_ui(connected, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.show_stats_panel(ui);
                    });
                });
            });
        
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_enabled_ui(connected, |ui| {
                // Dashboard at the top
                self.show_dashboard(ui);
                ui.separator();
                
                // Chart in the middle
                self.chart.ui(ui);
                ui.separator();
                
                // Filter panel
                let to_pin = self.filter_panel.update(ui);
                if self.stopped != self.filter_panel.stop {
                    self.stopped = self.filter_panel.stop;
                    self.send_driver_control();
                }
                if let Some(to_pin) = to_pin {
                    self.pinned_filters.pin_filter(to_pin, &self.data);
                }

                ui.separator();
                self.pinned_filters.update(ui);
                ui.separator();
                self.viewer.update(ui, &self.data);
            });
        });

        ctx.request_repaint();
    }
}
