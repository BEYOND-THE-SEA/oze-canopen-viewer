use std::sync::Arc;

use crate::{bitrate::RatesData, theme::OZON_PINK};
use egui::Vec2b;
use egui_plot::{Line, Plot, PlotPoints};
use tokio::{runtime::Handle, sync::Mutex};

#[derive(Debug)]
pub struct Chart {
    channel: Arc<Mutex<RatesData>>,
}

impl Chart {
    pub fn new(channel: Arc<Mutex<RatesData>>) -> Chart {
        Chart { channel }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        // Display Y-axis label manually on the left with spacing
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.add_space(120.0); // Center the label vertically
                ui.label(
                    egui::RichText::new("Bitrate\n(bits/s)")
                        .size(11.0)
                );
            });
            
            ui.add_space(5.0); // Space between label and plot
            
            let plot = Plot::new("plot")
                .height(250.0)
                .allow_drag(false)
                .allow_boxed_zoom(false)
                .allow_scroll(false)
                .allow_zoom(false)
                .show_axes(Vec2b::new(true, true))
                .x_axis_label("Time (s)")
                .label_formatter(|name, value| {
                    if !name.is_empty() {
                        format!("{}: {:.1} s, {:.0} bps", name, value.x, value.y)
                    } else {
                        format!("Time: {:.1} s\nBitrate: {:.0} bps", value.x, value.y)
                    }
                });

            Handle::current().block_on(async {
                let data: Vec<[f64; 2]> = self.channel.lock().await.clone();
                // There is no Borrowed PlotPoints so we need to copy every time
                plot.show(ui, |plot_ui| {
                    plot_ui.line(Line::new(PlotPoints::new(data)).color(OZON_PINK).name("CAN Bitrate"));
                })
            });
        });
    }
}
