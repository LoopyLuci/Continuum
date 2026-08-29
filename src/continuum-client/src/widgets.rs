#![allow(dead_code)]

use continuum_transport::ConnectionStatus;
use eframe::egui::{self, Color32, RichText, Ui};

pub fn status_indicator(ui: &mut Ui, status: &ConnectionStatus, label: &str) {
    let (color, text) = match status {
        ConnectionStatus::Disconnected => (
            Color32::from_rgb(180, 60, 60),
            format!("{}: Disconnected", label),
        ),
        ConnectionStatus::Connecting => (
            Color32::from_rgb(220, 180, 60),
            format!("{}: Connecting...", label),
        ),
        ConnectionStatus::Connected => (
            Color32::from_rgb(60, 180, 120),
            format!("{}: Connected", label),
        ),
        ConnectionStatus::Pairing => (
            Color32::from_rgb(220, 180, 60),
            format!("{}: Pairing...", label),
        ),
        ConnectionStatus::Paired => (
            Color32::from_rgb(60, 200, 140),
            format!("{}: Paired", label),
        ),
        ConnectionStatus::Streaming => (
            Color32::from_rgb(60, 255, 140),
            format!("{}: Streaming", label),
        ),
        ConnectionStatus::Reconnecting => (
            Color32::from_rgb(220, 140, 60),
            format!("{}: Reconnecting...", label),
        ),
    };

    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, color);
        ui.label(RichText::new(text).color(Color32::from_rgb(200, 200, 210)));
    });
}

pub fn stat_row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(Color32::from_rgb(140, 140, 155)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).color(Color32::from_rgb(220, 220, 225)));
        });
    });
}

pub fn section_header(ui: &mut Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(title)
            .strong()
            .color(Color32::from_rgb(180, 180, 195))
            .size(14.0),
    );
    ui.add_space(2.0);
}

pub fn toolbar_button(ui: &mut Ui, icon: &str, tooltip: &str, enabled: bool) -> bool {
    let response = ui.add_enabled(enabled, egui::Button::new(RichText::new(icon).size(16.0)));
    let clicked = response.clicked();
    response.on_hover_text(tooltip);
    clicked
}

pub fn monitor_selector(ui: &mut Ui, monitors: &[String], selected: &mut usize) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Monitor:");
        egui::ComboBox::from_id_salt("monitor_select")
            .selected_text(monitors.get(*selected).unwrap_or(&"None".to_string()))
            .show_ui(ui, |ui| {
                for (i, name) in monitors.iter().enumerate() {
                    if ui.selectable_value(selected, i, name).changed() {
                        changed = true;
                    }
                }
            });
    });
    changed
}

pub fn connection_panel(
    ui: &mut Ui,
    status: &ConnectionStatus,
    fps: f32,
    latency: f32,
    bandwidth: f32,
) {
    egui::Frame::new()
        .fill(Color32::from_rgb(25, 25, 32))
        .corner_radius(6.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            section_header(ui, "Connection");
            status_indicator(ui, status, "Status");
            ui.add_space(4.0);
            stat_row(ui, "FPS", &format!("{:.1}", fps));
            stat_row(ui, "Latency", &format!("{:.1} ms", latency));
            stat_row(ui, "Bandwidth", &format!("{:.0} kbps", bandwidth));
        });
}

pub fn performance_bar(ui: &mut Ui, label: &str, value: f32, max: f32, unit: &str) {
    let fraction = (value / max).clamp(0.0, 1.0);
    let color = if fraction < 0.5 {
        Color32::from_rgb(60, 200, 120)
    } else if fraction < 0.8 {
        Color32::from_rgb(220, 180, 60)
    } else {
        Color32::from_rgb(220, 80, 60)
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(Color32::from_rgb(140, 140, 155)));
        let bar_width = 100.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(bar_width, 12.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 3.0, Color32::from_rgb(40, 40, 50));
        let fill_rect =
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction, rect.height()));
        ui.painter().rect_filled(fill_rect, 3.0, color);
        ui.label(
            RichText::new(format!("{:.0}{}", value, unit))
                .color(Color32::from_rgb(200, 200, 210))
                .size(11.0),
        );
    });
}
