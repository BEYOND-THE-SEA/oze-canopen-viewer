use crate::message_cached::MessageCached;
use egui::{Align, Layout, TextWrapMode};
use oze_canopen::canopen::RxMessageToStringFormat;
use tokio::time::Instant;

/// Fixed column widths for the message table (headers centered in each cell).
struct ColWidths {
    timestamp: f32,
    cob_id: f32,
    data: f32,
    packet_type: f32,
    node_id: f32,
    info: f32,
}

impl ColWidths {
    const fn for_format(format: RxMessageToStringFormat) -> Self {
        let data = match format {
            RxMessageToStringFormat::Binary => 280.0,
            RxMessageToStringFormat::Hex => 200.0,
            RxMessageToStringFormat::Ascii | RxMessageToStringFormat::Utf8 => 160.0,
        };
        Self {
            timestamp: 88.0,
            cob_id: 72.0,
            data,
            packet_type: 100.0,
            node_id: 44.0,
            info: 320.0,
        }
    }
}

#[derive(Debug)]
pub struct MessageRow {
    pub start_time: Instant,
    pub format: RxMessageToStringFormat,
}

impl Default for MessageRow {
    fn default() -> Self {
        Self {
            start_time: Instant::now(),
            format: RxMessageToStringFormat::Hex,
        }
    }
}

impl MessageRow {
    fn separator(ui: &mut egui::Ui) {
        ui.weak("·");
    }

    fn header_cell(ui: &mut egui::Ui, width: f32, text: &str) {
        let h = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(
            egui::vec2(width, h),
            Layout::top_down(Align::Center),
            |ui| {
                ui.label(text);
            },
        );
    }

    fn data_cell(ui: &mut egui::Ui, width: f32, add_label: impl FnOnce(&mut egui::Ui)) {
        let h = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(
            egui::vec2(width, h),
            Layout::top_down(Align::LEFT),
            add_label,
        );
    }

    fn data_label(ui: &mut egui::Ui, width: f32, text: impl Into<egui::WidgetText>) {
        Self::data_cell(ui, width, |ui| {
            ui.add(egui::Label::new(text).wrap_mode(TextWrapMode::Wrap));
        });
    }

    fn info_cell(
        ui: &mut egui::Ui,
        width: f32,
        text: impl Into<egui::WidgetText>,
        tooltip: impl AsRef<str>,
    ) {
        Self::data_cell(ui, width, |ui| {
            ui.add(
                egui::Label::new(text)
                    .wrap_mode(TextWrapMode::Wrap)
                    .sense(egui::Sense::hover()),
            )
            .on_hover_text_at_pointer(tooltip.as_ref());
        });
    }

    fn data_column_title(format: RxMessageToStringFormat) -> &'static str {
        match format {
            RxMessageToStringFormat::Binary => "Binary data",
            RxMessageToStringFormat::Hex => "Hex data",
            RxMessageToStringFormat::Ascii => "ASCII data",
            RxMessageToStringFormat::Utf8 => "UTF8 data",
        }
    }

    pub fn header(&self, ui: &mut egui::Ui) {
        let cols = ColWidths::for_format(self.format);
        Self::header_cell(ui, cols.timestamp, "Timestamp");
        Self::separator(ui);
        Self::header_cell(ui, cols.cob_id, "COB ID");
        Self::separator(ui);
        Self::header_cell(ui, cols.data, Self::data_column_title(self.format));
        Self::separator(ui);
        Self::header_cell(ui, cols.packet_type, "Packet type");
        Self::separator(ui);
        Self::header_cell(ui, cols.node_id, "Node ID");
        Self::separator(ui);
        Self::header_cell(ui, cols.info, "Info");
    }

    pub fn message(&self, ui: &mut egui::Ui, d: &MessageCached) {
        self.message_custom_timestamp(ui, d, &self.start_time);
    }

    pub fn message_custom_timestamp(&self, ui: &mut egui::Ui, d: &MessageCached, time: &Instant) {
        let cols = ColWidths::for_format(self.format);
        let desc = d.msg.parsed_type.to_string();

        let time_secs = d.get_timestamp().duration_since(*time).as_secs_f32();
        let time_str = format!("{time_secs:.6}");
        let cob = &d.cob_str;
        let data = d.get_by_format(self.format);
        let node_id = if let Some(node_id) = d.msg.parsed_node_id {
            format!("{node_id:3}")
        } else {
            "   ".to_owned()
        };
        let info = d.additional.to_string();
        let tooltip = d.additional.get_tooltip();

        Self::data_label(ui, cols.timestamp, time_str);
        Self::separator(ui);
        Self::data_label(ui, cols.cob_id, cob.as_str());
        Self::separator(ui);
        Self::data_cell(ui, cols.data, |ui| {
            ui.add(egui::Label::new(data).wrap_mode(TextWrapMode::Wrap))
                .on_hover_ui(|ui| {
                    ui.label(format!("HEX:   {}", d.hex_str));
                    ui.label(format!("BIN:   {}", d.bin_str));
                    ui.label(format!("ASCII: {}", d.ascii_str));
                });
        });
        Self::separator(ui);
        Self::data_label(ui, cols.packet_type, desc);
        Self::separator(ui);
        Self::data_label(ui, cols.node_id, node_id);
        Self::separator(ui);
        Self::info_cell(ui, cols.info, info, tooltip);
    }
}
