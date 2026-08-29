#![allow(dead_code)]

use continuum_transport::ClientConfig;
use eframe::egui;

pub struct Settings {
    pub config: ClientConfig,
    pub show_window: bool,
    pub section: usize,
}

impl Settings {
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config,
            show_window: false,
            section: 0,
        }
    }

    pub fn render(&mut self, ctx: &egui::Context) {
        if !self.show_window {
            return;
        }
        let mut open = self.show_window;
        let section = &mut self.section;
        let config = &mut self.config;

        egui::Window::new("Settings")
            .collapsible(false)
            .resizable(true)
            .default_width(480.0)
            .default_height(480.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let names = ["Picture", "Performance", "Privacy", "Recording", "About"];
                    for (i, name) in names.iter().enumerate() {
                        if ui.selectable_label(*section == i, *name).clicked() {
                            *section = i;
                        }
                    }
                });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                match *section {
                    0 => render_picture(ui, config),
                    1 => render_performance(ui, config),
                    2 => render_privacy(ui, config),
                    3 => render_recording(ui, config),
                    _ => render_about(ui),
                }
            });

        self.show_window = open;
    }
}

fn render_picture(ui: &mut egui::Ui, _config: &mut ClientConfig) {
    ui.heading("How the Picture Looks");
    ui.add_space(8.0);
    ui.label("Continuum adjusts picture quality automatically for the best experience.");
    ui.add_space(12.0);
    ui.colored_label(
        egui::Color32::from_rgb(60, 200, 120),
        "✓  Auto-adjustment is on",
    );
}

fn render_performance(ui: &mut egui::Ui, config: &mut ClientConfig) {
    ui.heading("How Fast It Feels");
    ui.add_space(8.0);
    ui.label("Continuum adjusts automatically.");
    ui.add_space(12.0);
    ui.colored_label(
        egui::Color32::from_rgb(60, 200, 120),
        "✓  Automatic adjustment is on",
    );
    ui.add_space(16.0);
    ui.label("If the connection drops:");
    let mut reconnect = config.auto_reconnect;
    if ui
        .checkbox(&mut reconnect, "Try to reconnect automatically")
        .clicked()
    {
        config.auto_reconnect = reconnect;
    }
    ui.add_space(4.0);
    ui.colored_label(
        egui::Color32::from_rgb(160, 160, 170),
        "When turned on, Continuum will try again if the connection is lost.",
    );
}

fn render_privacy(ui: &mut egui::Ui, config: &mut ClientConfig) {
    ui.heading("Privacy & Security");
    ui.add_space(8.0);
    ui.colored_label(
        egui::Color32::from_rgb(60, 200, 120),
        "✓  Encrypted connection",
    );
    ui.add_space(4.0);
    ui.label("All data is scrambled so no one else can read it.");
    ui.add_space(12.0);
    let mut view_only = config.view_only;
    if ui
        .checkbox(&mut view_only, "View only — don't allow control")
        .clicked()
    {
        config.view_only = view_only;
    }
    ui.add_space(4.0);
    ui.colored_label(
        egui::Color32::from_rgb(160, 160, 170),
        "You can see the other computer but can't move the mouse or type.",
    );
    ui.add_space(12.0);
    let mut clipboard = config.enable_clipboard;
    if ui
        .checkbox(&mut clipboard, "Share clipboard between computers")
        .clicked()
    {
        config.enable_clipboard = clipboard;
    }
    ui.add_space(4.0);
    ui.colored_label(
        egui::Color32::from_rgb(160, 160, 170),
        "Copying text on one computer makes it available on the other.",
    );
}

fn render_recording(ui: &mut egui::Ui, _config: &mut ClientConfig) {
    ui.heading("Recording");
    ui.add_space(8.0);
    ui.label("Record what happens during a connection.");
    ui.add_space(12.0);
    let mut on = false;
    ui.checkbox(&mut on, "Record this session").clicked();
    ui.add_space(4.0);
    ui.colored_label(
        egui::Color32::from_rgb(160, 160, 170),
        "Recordings are saved to your computer.",
    );
    ui.add_space(16.0);
    ui.colored_label(egui::Color32::from_rgb(140, 140, 150), "No recordings yet.");
}

fn render_about(ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.heading("Continuum");
        ui.add_space(8.0);
        ui.colored_label(
            egui::Color32::from_rgb(140, 140, 150),
            format!("Version {}", env!("CARGO_PKG_VERSION")),
        );
        ui.add_space(12.0);
        ui.label("Remote access for everyone");
        ui.add_space(4.0);
        ui.label("Connect to another computer from this one.");
        ui.add_space(16.0);
        ui.colored_label(egui::Color32::from_rgb(100, 100, 110), "Built with Rust");
    });
}
