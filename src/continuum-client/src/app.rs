use crate::clipboard::ClipboardMonitor;
use crate::connection::{NetworkWorker, WorkerEvent};
use continuum_transport::client::ControlCommand;
use eframe::egui;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

#[allow(dead_code)]
pub struct ContinuumApp {
    machine_id: String,
    pairing_code: String,
    state: AppState,
    renderer: FrameRenderer,
    connect_id: String,
    connect_password: String,
    clipboard: ClipboardMonitor,
    sas_words: Vec<String>,
    show_sas_modal: bool,
    audio_enabled: bool,
    pending_file_path: Option<PathBuf>,
}

#[allow(dead_code)]
enum AppState {
    Home,
    Connecting {
        worker: NetworkWorker,
        #[allow(dead_code)]
        id: String,
        #[allow(dead_code)]
        password: String,
    },
    Connected {
        worker: NetworkWorker,
        frame_count: u64,
    },
    Disconnected {
        message: String,
    },
}

#[allow(dead_code)]
struct FrameRenderer {
    texture: Option<egui::TextureHandle>,
    last_frame: Option<egui::ColorImage>,
}

#[allow(dead_code)]
impl FrameRenderer {
    fn new() -> Self {
        Self {
            texture: None,
            last_frame: None,
        }
    }

    fn update(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if let Ok(img) = image::load_from_memory(data) {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let pixels: Vec<egui::Color32> = rgba
                .pixels()
                .map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
                .collect();
            self.last_frame = Some(egui::ColorImage { size, pixels });
            self.texture = None;
        }
    }

    fn render(&mut self, ui: &mut egui::Ui) {
        if let Some(frame) = &self.last_frame {
            if self.texture.is_none() {
                let tex =
                    ui.ctx()
                        .load_texture("frame", frame.clone(), egui::TextureOptions::default());
                self.texture = Some(tex);
            }
            if let Some(texture) = &self.texture {
                let avail = ui.available_size();
                let aspect = texture.size_vec2().x / texture.size_vec2().y;
                let (w, h) = if avail.x / avail.y > aspect {
                    (avail.y * aspect, avail.y)
                } else {
                    (avail.x, avail.x / aspect)
                };
                ui.image((texture.id(), egui::vec2(w, h)));
            }
        }
    }

    fn has_frame(&self) -> bool {
        self.last_frame.is_some()
    }
}

impl ContinuumApp {
    pub fn new(machine_id: String, pairing_code: String) -> Self {
        Self {
            machine_id,
            pairing_code,
            state: AppState::Home,
            renderer: FrameRenderer::new(),
            connect_id: String::new(),
            connect_password: String::new(),
            clipboard: ClipboardMonitor::new(),
            sas_words: Vec::new(),
            show_sas_modal: false,
            audio_enabled: true,
            pending_file_path: None,
        }
    }
}

impl eframe::App for ContinuumApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Process network events in Connecting/Connected states
        match &mut self.state {
            AppState::Connecting { worker, .. } => {
                while let Ok(evt) = worker.event_rx.try_recv() {
                    match evt {
                        WorkerEvent::Frame(data) => {
                            self.renderer.update(&data);
                            let worker = match std::mem::replace(&mut self.state, AppState::Home) {
                                AppState::Connecting { worker, .. } => worker,
                                _ => {
                                    tracing::warn!(
                                        "Unexpected state transition during frame receive"
                                    );
                                    return;
                                }
                            };
                            self.state = AppState::Connected {
                                worker,
                                frame_count: 0,
                            };
                            return;
                        }
                        WorkerEvent::Status(s) => {
                            if s.contains("Streaming") {
                                let worker =
                                    match std::mem::replace(&mut self.state, AppState::Home) {
                                        AppState::Connecting { worker, .. } => worker,
                                        _ => {
                                            tracing::warn!(
                                            "Unexpected state transition during streaming status"
                                        );
                                            return;
                                        }
                                    };
                                self.state = AppState::Connected {
                                    worker,
                                    frame_count: 0,
                                };
                                return;
                            }
                        }
                        WorkerEvent::Clipboard(_) => {}
                        WorkerEvent::PairingResult {
                            accepted,
                            sas_words,
                            message: _,
                        } => {
                            if accepted && !sas_words.is_empty() {
                                self.sas_words = sas_words;
                                self.show_sas_modal = true;
                            }
                        }
                        WorkerEvent::Error(e) => {
                            self.state = AppState::Disconnected { message: e };
                            return;
                        }
                    }
                }
            }
            AppState::Connected {
                worker,
                frame_count,
            } => {
                if let Some(clip_data) = self.clipboard.poll() {
                    let _ = worker.command_tx.send(ControlCommand::Clipboard(clip_data));
                }
                while let Ok(evt) = worker.event_rx.try_recv() {
                    match evt {
                        WorkerEvent::Frame(data) => {
                            self.renderer.update(&data);
                            *frame_count += 1;
                        }
                        WorkerEvent::Clipboard(data) => {
                            if let Ok(text) = String::from_utf8(data) {
                                self.clipboard.set_text(&text);
                            }
                        }
                        WorkerEvent::PairingResult {
                            accepted,
                            sas_words,
                            message: _,
                        } => {
                            if accepted && !sas_words.is_empty() {
                                self.sas_words = sas_words;
                                self.show_sas_modal = true;
                            }
                        }
                        WorkerEvent::Error(e) => {
                            self.state = AppState::Disconnected { message: e };
                            return;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        // Render current state
        let _state_name = match &self.state {
            AppState::Home => {
                self.render_home(ctx);
                "home".to_string()
            }
            AppState::Connecting { .. } => {
                self.render_connecting(ctx);
                "connecting".to_string()
            }
            AppState::Connected { frame_count, .. } => {
                let fc = *frame_count;
                self.ui_heading_bar(ctx, &format!("Connected — {} frames", fc));
                self.render_streaming(ctx);
                "connected".to_string()
            }
            AppState::Disconnected { .. } => "disconnected".to_string(),
        };
        if let AppState::Disconnected { message } = &self.state {
            let msg = message.clone();
            self.render_disconnected(ctx, &msg);
        }

        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

#[allow(dead_code)]
impl ContinuumApp {
    fn render_home(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("Continuum").size(20.0).strong());
                ui.label(
                    egui::RichText::new("Remote access for everyone")
                        .color(egui::Color32::from_rgb(140, 140, 150))
                        .size(12.0),
                );
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(30.0);
            ui.columns(2, |cols| {
                cols[0].vertical(|ui| {
                    ui.heading("Your Machine");
                    ui.add_space(8.0);

                    ui.label("This computer's ID:");
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.machine_id.clone())
                                .font(egui::TextStyle::Monospace)
                                .interactive(false),
                        );
                        if ui.small_button("Copy").clicked() {
                            ui.ctx().copy_text(self.machine_id.clone());
                        }
                        if ui.small_button("Generate New").clicked() {
                            self.machine_id = super::generate_machine_id();
                            super::save_config(&self.machine_id, &self.pairing_code);
                        }
                    });

                    ui.add_space(12.0);
                    ui.label("This computer's password:");
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.pairing_code.clone())
                                .font(egui::TextStyle::Monospace)
                                .interactive(false),
                        );
                        if ui.small_button("Copy").clicked() {
                            ui.ctx().copy_text(self.pairing_code.clone());
                        }
                        if ui.small_button("Generate New").clicked() {
                            self.pairing_code = super::generate_pairing_code();
                            super::save_config(&self.machine_id, &self.pairing_code);
                        }
                    });

                    ui.add_space(16.0);
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 180, 120),
                        "Share these with someone to let them connect to this computer.",
                    );
                });

                cols[1].vertical(|ui| {
                    ui.heading("Connect to someone");
                    ui.add_space(8.0);
                    ui.label("Their ID:");
                    ui.add_space(2.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.connect_id)
                            .hint_text("6 characters")
                            .font(egui::TextStyle::Monospace),
                    );

                    ui.add_space(8.0);
                    ui.label("Their password:");
                    ui.add_space(2.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.connect_password)
                            .hint_text("6 characters")
                            .font(egui::TextStyle::Monospace),
                    );

                    ui.add_space(20.0);
                    let can_connect =
                        !self.connect_id.is_empty() && !self.connect_password.is_empty();
                    if ui
                        .add_enabled(can_connect, egui::Button::new("Connect"))
                        .clicked()
                    {
                        let addr = resolve_id_to_address(&self.connect_id);
                        let worker = NetworkWorker::spawn(addr, self.connect_password.clone());
                        self.sas_words.clear();
                        self.show_sas_modal = false;
                        self.state = AppState::Connecting {
                            worker,
                            id: self.connect_id.clone(),
                            password: self.connect_password.clone(),
                        };
                    }
                    if !can_connect {
                        ui.colored_label(
                            egui::Color32::from_rgb(140, 140, 150),
                            "Enter the ID and Password shown on the other computer.",
                        );
                    }
                });
            });
        });
    }

    fn render_connecting(&self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.add(egui::Spinner::new().size(48.0));
                ui.add_space(16.0);
                ui.heading("Connecting...");
                ui.add_space(8.0);
                ui.colored_label(
                    egui::Color32::from_rgb(160, 160, 170),
                    "This usually takes a few seconds",
                );
            });
        });
    }

    fn render_streaming(&mut self, ctx: &egui::Context) {
        // SAS verification modal
        if self.show_sas_modal && !self.sas_words.is_empty() {
            egui::Window::new("Verify Connection")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(egui::RichText::new("Verify these words match on both screens:").size(16.0));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        for word in &self.sas_words {
                            ui.label(egui::RichText::new(word).size(24.0).strong().color(egui::Color32::from_rgb(100, 200, 255)));
                        }
                    });
                    ui.add_space(12.0);
                    ui.colored_label(egui::Color32::from_rgb(220, 180, 60), "If the words don't match, disconnect immediately — the connection may be compromised.");
                    ui.add_space(8.0);
                    if ui.button("Words Match — Continue").clicked() {
                        self.show_sas_modal = false;
                    }
                    if ui.button("Words Don't Match — Disconnect").clicked() {
                        self.state = AppState::Home;
                    }
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.renderer.has_frame() {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(8, 8, 12))
                    .inner_margin(2.0)
                    .show(ui, |ui| {
                        self.renderer.render(ui);
                    });
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.add(egui::Spinner::new().size(32.0));
                    ui.label("Waiting for first frame...");
                });
            }
        });
    }

    fn render_disconnected(&mut self, ctx: &egui::Context, message: &str) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.heading(
                    egui::RichText::new("Disconnected")
                        .color(egui::Color32::from_rgb(200, 140, 60)),
                );
                ui.label(message);
                ui.add_space(16.0);
                if ui.button("Back to Home").clicked() {
                    self.state = AppState::Home;
                }
            });
        });
    }

    fn ui_heading_bar(&mut self, ctx: &egui::Context, text: &str) {
        egui::TopBottomPanel::top("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(60, 220, 120), text);
                if !self.sas_words.is_empty() && ui.small_button("Verify SAS").clicked() {
                    self.show_sas_modal = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label("Press Escape to disconnect");
                });
            });
        });
    }
}

#[allow(dead_code)]
fn resolve_id_to_address(id: &str) -> SocketAddr {
    let id = id.trim();

    // Direct IP:port
    if let Ok(addr) = id.parse::<SocketAddr>() {
        return addr;
    }

    // Direct IP (add default port)
    if let Ok(ip) = id.parse::<std::net::IpAddr>() {
        return SocketAddr::new(ip, 4433);
    }

    // IPv6 with brackets: [::1]:port
    if id.starts_with('[') {
        if let Some(end) = id.find(']') {
            let ip_str = &id[1..end];
            if let Ok(ip) = ip_str.parse::<std::net::IpAddr>() {
                let port = if id.len() > end + 1 && id.as_bytes()[end + 1] == b':' {
                    id[end + 2..].parse::<u16>().unwrap_or(4433)
                } else {
                    4433
                };
                return SocketAddr::new(ip, port);
            }
        }
    }

    // Hostname:port
    if let Some((host, port_str)) = id.rsplit_once(':') {
        if let Ok(port) = port_str.parse::<u16>() {
            if let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(&(host, port)) {
                if let Some(addr) = addrs.into_iter().next() {
                    return addr;
                }
            }
        }
    }

    // Just hostname with default port
    if let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(&(id, 4433u16)) {
        if let Some(addr) = addrs.into_iter().next() {
            return addr;
        }
    }

    // Fallback to localhost
    "127.0.0.1:4433".parse().unwrap()
}
