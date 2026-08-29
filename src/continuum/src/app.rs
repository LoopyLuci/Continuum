#![allow(dead_code)]

use continuum_transport::ClientConfig;
use eframe::egui;
use std::net::SocketAddr;
use std::time::Duration;

pub struct ContinuumApp {
    machine_id: String,
    pairing_code: String,
    state: AppState,
    renderer: FrameRenderer,
    connect_id: String,
    connect_password: String,
    error_msg: String,
    sas_words: Vec<String>,
    show_sas_modal: bool,
    frame_count: u64,
    recording_active: bool,
}

enum AppState {
    Home,
    Connecting {
        id: String,
        password: String,
        rx: std::sync::mpsc::Receiver<WorkerEvent>,
    },
    Connected {
        config: ClientConfig,
        rx: std::sync::mpsc::Receiver<WorkerEvent>,
        _thread: std::thread::JoinHandle<()>,
    },
    Disconnected {
        message: String,
    },
}

struct FrameRenderer {
    texture: Option<egui::TextureHandle>,
    last_frame: Option<egui::ColorImage>,
}

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

enum WorkerEvent {
    Frame(Vec<u8>),
    Status(String),
    Clipboard(Vec<u8>),
    PairingResult {
        accepted: bool,
        #[allow(dead_code)]
        message: String,
        sas_words: Vec<String>,
    },
    Error(String),
}

fn start_connection(addr: SocketAddr, password: String) -> std::sync::mpsc::Receiver<WorkerEvent> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .unwrap();
        rt.block_on(async move {
            let (tokio_tx, mut tokio_rx) =
                tokio::sync::mpsc::channel::<continuum_transport::client::ConnectionEvent>(256);
            let tx2 = tx.clone();
            tokio::spawn(async move {
                while let Some(e) = tokio_rx.recv().await {
                    let out = match e {
                        continuum_transport::client::ConnectionEvent::Frame { data, .. } => {
                            WorkerEvent::Frame(data)
                        }
                        continuum_transport::client::ConnectionEvent::Status(s) => {
                            WorkerEvent::Status(s.to_string())
                        }
                        continuum_transport::client::ConnectionEvent::Clipboard(d) => {
                            WorkerEvent::Clipboard(d.data)
                        }
                        continuum_transport::client::ConnectionEvent::Error(e) => {
                            WorkerEvent::Error(e)
                        }
                        continuum_transport::client::ConnectionEvent::PairingResult(r) => {
                            let evt = WorkerEvent::PairingResult {
                                accepted: r.accepted,
                                message: r.message.clone(),
                                sas_words: r.sas_words.clone(),
                            };
                            if tx2.send(evt).is_err() {
                                break;
                            }
                            if r.accepted {
                                let _ = tx2.send(WorkerEvent::Status("Streaming".into()));
                            } else {
                                let _ = tx2.send(WorkerEvent::Error(format!(
                                    "Pairing failed: {}",
                                    r.message
                                )));
                            }
                            continue;
                        }
                        continuum_transport::client::ConnectionEvent::Metrics(_) => continue,
                        _ => continue,
                    };
                    if tx2.send(out).is_err() {
                        break;
                    }
                }
            });
            let (_, cmd_rx) =
                tokio::sync::mpsc::channel::<continuum_transport::client::ControlCommand>(32);
            let config = ClientConfig {
                server_addr: addr,
                pairing_code: password,
                auto_reconnect: true,
                ..Default::default()
            };
            let _ =
                continuum_transport::client::maintain_connection(&config, tokio_tx, cmd_rx).await;
        });
    });
    rx
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
            error_msg: String::new(),
            sas_words: Vec::new(),
            show_sas_modal: false,
            frame_count: 0,
            recording_active: false,
        }
    }
}

impl eframe::App for ContinuumApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut next_state = None;

        if let AppState::Home = &self.state {
            self.render_home(ctx, &mut next_state);
        }

        if let AppState::Connecting { rx, .. } = &self.state {
            let mut disconnect_err = None;
            while let Ok(evt) = rx.try_recv() {
                match evt {
                    WorkerEvent::Frame(data) => {
                        self.renderer.update(&data);
                        self.frame_count += 1;
                    }
                    WorkerEvent::Status(s) => {
                        if s.contains("Streaming") {
                            // Will transition below
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
                        disconnect_err = Some(e);
                    }
                    _ => {}
                }
            }
            if let Some(e) = disconnect_err {
                self.state = AppState::Disconnected { message: e };
            }
            // Transition to Connected if we have a frame
            if self.renderer.has_frame() {
                if let AppState::Connecting { rx, .. } =
                    std::mem::replace(&mut self.state, AppState::Home)
                {
                    let addr = resolve_id_to_address(&self.connect_id);
                    let config = ClientConfig {
                        server_addr: addr,
                        pairing_code: self.connect_password.clone(),
                        auto_reconnect: true,
                        ..Default::default()
                    };
                    self.state = AppState::Connected {
                        config,
                        rx,
                        _thread: std::thread::Builder::new()
                            .name("conn".into())
                            .spawn(|| {})
                            .unwrap(),
                    };
                }
            }
        }

        if let AppState::Connected { rx, config, .. } = &self.state {
            while let Ok(evt) = rx.try_recv() {
                match evt {
                    WorkerEvent::Frame(data) => {
                        self.renderer.update(&data);
                        self.frame_count += 1;
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
                        next_state = Some(AppState::Disconnected { message: e });
                    }
                    _ => {}
                }
            }
            let config = config.clone();
            self.render_streaming(ctx, &config);
        }

        if let AppState::Disconnected { ref message } = self.state {
            let msg = message.clone();
            self.render_disconnected(ctx, &msg);
        }

        if let Some(s) = next_state {
            self.state = s;
        }
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

impl ContinuumApp {
    fn render_home(&mut self, ctx: &egui::Context, _next: &mut Option<AppState>) {
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
                            .hint_text("e.g. a1b2c3d4")
                            .font(egui::TextStyle::Monospace),
                    );

                    ui.add_space(8.0);

                    ui.label("Their password:");
                    ui.add_space(2.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.connect_password)
                            .hint_text("e.g. bravo-echo-5832")
                            .font(egui::TextStyle::Monospace)
                            .password(true),
                    );

                    ui.add_space(20.0);

                    let can_connect =
                        !self.connect_id.is_empty() && !self.connect_password.is_empty();
                    if ui
                        .add_enabled(
                            can_connect,
                            egui::Button::new("Connect").min_size(egui::vec2(200.0, 40.0)),
                        )
                        .clicked()
                    {
                        let addr = resolve_id_to_address(&self.connect_id);
                        let _config = ClientConfig {
                            server_addr: addr,
                            pairing_code: self.connect_password.clone(),
                            auto_reconnect: true,
                            ..Default::default()
                        };
                        let rx = start_connection(addr, self.connect_password.clone());
                        self.sas_words.clear();
                        self.show_sas_modal = false;
                        self.state = AppState::Connecting {
                            id: self.connect_id.clone(),
                            password: self.connect_password.clone(),
                            rx,
                        };
                        return;
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

    fn render_connecting_screen(&self, ctx: &egui::Context) {
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

    fn render_streaming(&mut self, ctx: &egui::Context, _config: &ClientConfig) {
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

        egui::TopBottomPanel::top("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Encryption indicator
                ui.colored_label(egui::Color32::from_rgb(60, 220, 120), "🔒 E2E Encrypted");

                ui.separator();

                // Audio indicator
                ui.label("🔊 Audio");

                ui.separator();

                // Clipboard indicator
                ui.label("📋 Clipboard");

                ui.separator();

                // Recording indicator
                if self.recording_active {
                    ui.colored_label(egui::Color32::from_rgb(255, 80, 80), "⏺ Recording");
                    ui.separator();
                }

                // SAS verify button
                if !self.sas_words.is_empty() {
                    if ui.small_button("Verify SAS").clicked() {
                        self.show_sas_modal = true;
                    }
                    ui.separator();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Disconnect").clicked() {
                        self.state = AppState::Home;
                    }
                    ui.label(format!("{} frames", self.frame_count));
                });
            });
        });
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
}

fn resolve_id_to_address(id: &str) -> std::net::SocketAddr {
    let id = id.trim();

    // Direct IP:port
    if let Ok(addr) = id.parse::<std::net::SocketAddr>() {
        return addr;
    }

    // Direct IP (add default port)
    if let Ok(ip) = id.parse::<std::net::IpAddr>() {
        return std::net::SocketAddr::new(ip, 4433);
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
                return std::net::SocketAddr::new(ip, port);
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
