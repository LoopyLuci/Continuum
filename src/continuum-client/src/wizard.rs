#![allow(dead_code)]

use continuum_transport::config::ClientConfig;
use eframe::egui;
use std::net::SocketAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Welcome,
    TutorialStep1,
    TutorialStep2,
    TutorialStep3,
    FindServer,
    ManualEntry,
    Pair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionType {
    Auto,
    Manual,
}

pub struct ServerEntry {
    pub name: String,
    pub addr: SocketAddr,
    pub hostname: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub enum WizardAction {
    None,
    GoBack,
    GoHome,
    StartConnecting(ClientConfig),
    ScanNetwork,
}

pub struct WizardState {
    pub current_step: WizardStep,
    pub connection_type: ConnectionType,
    pub discovered_servers: Vec<ServerEntry>,
    pub selected_server: Option<usize>,
    pub manual_address: String,
    pub manual_address_valid: bool,
    pub pairing_code: String,
    pub is_scanning: bool,
    pub pending_action: Option<WizardAction>,
    pub tutorial_complete: bool,
}

impl WizardState {
    pub fn new() -> Self {
        Self {
            current_step: WizardStep::Welcome,
            connection_type: ConnectionType::Auto,
            discovered_servers: Vec::new(),
            selected_server: None,
            manual_address: String::new(),
            manual_address_valid: false,
            pairing_code: "continuum".to_string(),
            is_scanning: false,
            pending_action: None,
            tutorial_complete: false,
        }
    }

    pub fn reset(&mut self) {
        self.current_step = WizardStep::Welcome;
        self.pending_action = None;
    }

    fn selected_addr(&self) -> SocketAddr {
        if let Some(idx) = self.selected_server {
            self.discovered_servers
                .get(idx)
                .map(|s| s.addr)
                .unwrap_or_else(|| "127.0.0.1:4433".parse().unwrap())
        } else if self.connection_type == ConnectionType::Manual {
            self.manual_address
                .parse()
                .unwrap_or_else(|_| "127.0.0.1:4433".parse().unwrap())
        } else {
            "127.0.0.1:4433".parse().unwrap()
        }
    }

    pub fn render(&mut self, ui: &mut egui::Ui) -> WizardAction {
        self.draw_progress(ui);
        ui.add_space(24.0);

        match self.current_step {
            WizardStep::Welcome => self.render_welcome(ui),
            WizardStep::TutorialStep1 => self.render_tutorial1(ui),
            WizardStep::TutorialStep2 => self.render_tutorial2(ui),
            WizardStep::TutorialStep3 => self.render_tutorial3(ui),
            WizardStep::FindServer => self.render_find_server(ui),
            WizardStep::ManualEntry => self.render_manual_entry(ui),
            WizardStep::Pair => self.render_pair(ui),
        }

        if self.current_step != WizardStep::Welcome
            && self.current_step != WizardStep::TutorialStep1
        {
            ui.add_space(20.0);
            if ui.button("Back").clicked() {
                self.go_back();
            }
        }

        self.pending_action.take().unwrap_or(WizardAction::None)
    }

    pub fn go_back(&mut self) {
        self.current_step = match self.current_step {
            WizardStep::Welcome => WizardStep::Welcome,
            WizardStep::TutorialStep1 => WizardStep::Welcome,
            WizardStep::TutorialStep2 => WizardStep::TutorialStep1,
            WizardStep::TutorialStep3 => WizardStep::TutorialStep2,
            WizardStep::FindServer => WizardStep::TutorialStep3,
            WizardStep::ManualEntry => WizardStep::TutorialStep3,
            WizardStep::Pair => WizardStep::FindServer,
        };
    }

    fn draw_progress(&self, ui: &mut egui::Ui) {
        let steps = [
            WizardStep::Welcome,
            WizardStep::TutorialStep1,
            WizardStep::TutorialStep2,
            WizardStep::TutorialStep3,
        ];
        let current = steps
            .iter()
            .position(|s| *s == self.current_step)
            .unwrap_or(0);
        let labels = ["Start", "Connect", "Learn", "Ready"];

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(ui.available_width() / 6.0);
            let sw = ui.available_width() * 0.66 / 4.0;
            for i in 0..4 {
                let done = i <= current;
                let (r, _) =
                    ui.allocate_exact_size(egui::vec2(sw - 4.0, 4.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    r,
                    2.0,
                    if done {
                        egui::Color32::from_rgb(60, 200, 120)
                    } else {
                        egui::Color32::from_rgb(50, 50, 58)
                    },
                );
                if i < 3 {
                    ui.add_space(4.0);
                }
            }
        });
        if let Some(label) = labels.get(current) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() / 2.0 - 40.0);
                ui.label(
                    egui::RichText::new(*label)
                        .color(egui::Color32::from_rgb(160, 160, 170))
                        .size(12.0),
                );
            });
        }
    }

    // ── Step 1: Welcome / Home ──────────────────────────────
    fn render_welcome(&mut self, ui: &mut egui::Ui) {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("Continuum").size(36.0).strong());
            ui.add_space(8.0);
            ui.label("See another computer on your screen. Control it like it's in front of you.");
            ui.add_space(40.0);

            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() / 3.0);
                if ui
                    .add(
                        egui::Button::new("  Connect to a computer  ")
                            .min_size(egui::vec2(200.0, 48.0)),
                    )
                    .clicked()
                {
                    self.pending_action = Some(WizardAction::ScanNetwork);
                    self.current_step = WizardStep::FindServer;
                }
            });

            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() / 3.0);
                if ui
                    .add(
                        egui::Button::new("  Tutorial — show me how  ")
                            .min_size(egui::vec2(200.0, 48.0)),
                    )
                    .clicked()
                {
                    self.current_step = WizardStep::TutorialStep1;
                }
            });

            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() / 3.0);
                if ui
                    .add(
                        egui::Button::new("  Start on this computer  ")
                            .min_size(egui::vec2(200.0, 48.0)),
                    )
                    .clicked()
                {
                    self.pending_action =
                        Some(WizardAction::StartConnecting(ClientConfig::default()));
                }
            });
        });
    }

    // ── Step 2: Tutorial ──────────────────────────────────────
    fn render_tutorial1(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(
                egui::RichText::new("Step 1 — What is Continuum?")
                    .size(24.0)
                    .strong(),
            );
            ui.add_space(16.0);

            ui.label("Continuum lets you see and control another computer from here.");
            ui.add_space(8.0);
            ui.label("You could be in the same room, or across the world.");
            ui.add_space(24.0);

            ui.label(egui::RichText::new("How it works:").strong().size(16.0));
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.add_space(60.0);
                ui.vertical(|ui| {
                    ui.label("1. Install Continuum on the computer you want to control");
                    ui.add_space(4.0);
                    ui.label("2. Open Continuum here and find that computer");
                    ui.add_space(4.0);
                    ui.label("3. Enter the code shown on the other computer");
                    ui.add_space(4.0);
                    ui.label("4. You're connected!");
                });
            });

            ui.add_space(24.0);
            ui.label("That's it. No special setup required on your end.");
            ui.add_space(24.0);

            if ui.add(egui::Button::new("Next: How to connect")).clicked() {
                self.current_step = WizardStep::TutorialStep2;
            }
        });
    }

    fn render_tutorial2(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(
                egui::RichText::new("Step 2 — How to connect")
                    .size(24.0)
                    .strong(),
            );
            ui.add_space(16.0);

            ui.label("The easiest way is to be on the same network:");
            ui.add_space(12.0);

            ui.label(egui::RichText::new("Same network (at home or office)").strong());
            ui.label("Click 'Connect to a computer' on the home screen.");
            ui.label("Continuum will find computers running Continuum automatically.");
            ui.add_space(12.0);

            ui.label(egui::RichText::new("Different network (across the world)").strong());
            ui.label("Enter the address shown on the other computer's screen.");
            ui.add_space(12.0);

            ui.label("The other computer must have Continuum installed and running.");
            ui.add_space(24.0);

            if ui.add(egui::Button::new("Next: Learn more")).clicked() {
                self.current_step = WizardStep::TutorialStep3;
            }
        });
    }

    fn render_tutorial3(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(
                egui::RichText::new("Step 3 — Quick tips")
                    .size(24.0)
                    .strong(),
            );
            ui.add_space(16.0);

            ui.label("While connected:");
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.add_space(60.0);
                ui.vertical(|ui| {
                    ui.label("• Move your mouse to move the other computer's cursor");
                    ui.add_space(4.0);
                    ui.label("• Click to click there too");
                    ui.add_space(4.0);
                    ui.label("• Type to type on the other computer");
                    ui.add_space(4.0);
                    ui.label("• Right-click for right-click menus");
                    ui.add_space(4.0);
                    ui.label("• Scroll with the wheel or trackpad");
                });
            });

            ui.add_space(24.0);
            ui.label(
                egui::RichText::new(
                    "Your connection is secure. Only you can see and control the other computer.",
                )
                .color(egui::Color32::from_rgb(120, 200, 120)),
            );
            ui.add_space(24.0);

            if ui.add(egui::Button::new("Done — let's connect")).clicked() {
                self.tutorial_complete = true;
                self.pending_action = Some(WizardAction::ScanNetwork);
                self.current_step = WizardStep::FindServer;
            }
        });
    }

    // ── Step 4: Find Server ──────────────────────────────────────
    fn render_find_server(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            if self.discovered_servers.is_empty() {
                ui.label("Scanning for computers on your network...");
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Waiting for Continuum to find nearby computers")
                        .color(egui::Color32::from_rgb(140, 140, 150)),
                );

                ui.add_space(20.0);
                ui.label("If no computers appear:");
                ui.add_space(4.0);
                ui.label("1. Check the other computer has Continuum running");
                ui.add_space(4.0);
                ui.label("2. Make sure both are on the same network");
                ui.add_space(4.0);
                ui.label("3. Try entering the address manually");
            } else {
                ui.label("Computers found on your network:");
                ui.add_space(8.0);
                for (i, s) in self.discovered_servers.iter().enumerate() {
                    if ui
                        .add(egui::Button::new(format!("{}  —  {}", s.name, s.addr)))
                        .clicked()
                    {
                        self.selected_server = Some(i);
                        self.current_step = WizardStep::Pair;
                    }
                }
            }

            ui.add_space(16.0);
            ui.label("Or enter an address manually:");
            ui.add_space(4.0);
            if ui.add(egui::Button::new("Enter address")).clicked() {
                self.current_step = WizardStep::ManualEntry;
            }
        });
    }

    // ── Step 4b: Manual Entry ──────────────────────────────
    fn render_manual_entry(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.label("Enter the address:");
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("This is shown on the other computer's Continuum screen.")
                    .color(egui::Color32::from_rgb(140, 140, 150)),
            );

            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.manual_address)
                    .hint_text("192.168.1.100:4433"),
            );
            if resp.changed() {
                self.manual_address_valid = self.manual_address.parse::<SocketAddr>().is_ok();
            }

            if !self.manual_address.is_empty() && !self.manual_address_valid {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 120, 120),
                    "That doesn't look right. Check the address and port.",
                );
            }

            if self.manual_address_valid && ui.button("Connect").clicked() {
                self.current_step = WizardStep::Pair;
            }
        });
    }

    // ── Step 5: Pair ──────────────────────────────────────
    fn render_pair(&mut self, ui: &mut egui::Ui) {
        let addr = self.selected_addr();
        ui.vertical_centered(|ui| {
            ui.label(format!("Computer: {}", addr));
            ui.add_space(20.0);
            ui.label("Look at the other computer's screen for the code.");
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Enter the code shown there.")
                    .color(egui::Color32::from_rgb(140, 140, 150)),
            );

            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.pairing_code).hint_text("Connection code"),
            );
            if resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !self.pairing_code.is_empty()
            {
                self.pending_action = Some(WizardAction::StartConnecting(ClientConfig {
                    server_addr: addr,
                    pairing_code: self.pairing_code.clone(),
                    auto_reconnect: true,
                    ..Default::default()
                }));
            }

            ui.add_space(20.0);
            if !self.pairing_code.is_empty() {
                if ui.add(egui::Button::new("Connect")).clicked() {
                    self.pending_action = Some(WizardAction::StartConnecting(ClientConfig {
                        server_addr: addr,
                        pairing_code: self.pairing_code.clone(),
                        auto_reconnect: true,
                        ..Default::default()
                    }));
                }
            } else {
                ui.colored_label(
                    egui::Color32::from_rgb(140, 140, 150),
                    "Enter the code to continue.",
                );
            }
        });
    }
}
