#![allow(dead_code)]

// Continuum — One app. Always on. Zero setup.
// =====================================================
// Server always runs. Shows YOUR ID + Password on launch.
// Click Connect to enter someone else's ID + Password.
// =====================================================

mod app;
mod cdp;
mod cdp_tests;
mod tray;

use clap::Parser;
use continuum_transport::ServerConfig;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "continuum", version, about = "Remote access for everyone")]
struct Cli {
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Run in client-only mode (no server, for connecting to another instance)
    #[arg(long)]
    client_only: bool,

    #[arg(long, help = "Directory for session recordings")]
    record_dir: Option<PathBuf>,

    #[arg(long, help = "Write frame-level audit log to this JSONL file")]
    audit_frames: Option<PathBuf>,

    #[arg(long, help = "Disable E2E encryption (for local testing only)")]
    insecure: bool,

    /// Enable verbose/debug logging
    #[arg(long, short)]
    verbose: bool,

    /// Replay a recorded session file
    #[arg(long)]
    replay: Option<PathBuf>,

    /// Enable CDP debug API on this port (0 = disabled)
    #[arg(long, default_value = "0")]
    debug: u16,
}

#[tokio::main]
async fn main() {
    let args = Cli::parse();

    if let Some(ref replay_path) = args.replay {
        run_replay_gui(replay_path);
        return;
    }

    if args.insecure {
        eprintln!("WARNING: Running with --insecure flag. E2E encryption is DISABLED.");
        eprintln!("This is intended for local testing ONLY. Do not use in production.");
        eprintln!("All media frames will be transmitted in PLAINTEXT.");
        eprintln!();
    }

    let log_level = if args.verbose {
        "debug"
    } else {
        &args.log_level
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    // Load or create persistent 6-character identity
    let (machine_id, pairing_code) = if args.client_only {
        // Client-only mode: generate fresh random identity each time
        (generate_machine_id(), generate_pairing_code())
    } else {
        load_or_create_config()
    };

    // Start server only if NOT in client-only mode
    if !args.client_only {
        let server_config = ServerConfig {
            pairing_code: pairing_code.clone(),
            record_dir: args.record_dir.clone(),
            audit_frames: args.audit_frames.clone(),
            insecure: args.insecure,
            ..Default::default()
        };
        tokio::spawn(async move {
            if let Err(e) = continuum_transport::run_server(server_config).await {
                tracing::error!(error = %e, "Server error");
            }
        });
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        if args.debug > 0 {
            let debug_addr: std::net::SocketAddr = format!("127.0.0.1:{}", args.debug)
                .parse()
                .expect("Invalid debug port");
            let debug_state = cdp::DebugState {
                state: Arc::new(parking_lot::RwLock::new(cdp::AppStateSnapshot::default())),
                event_tx: tokio::sync::broadcast::channel(256).0,
            };
            tokio::spawn(async move {
                cdp::start_cdp_server(debug_state, debug_addr).await;
            });
            eprintln!("CDP Debug API: http://127.0.0.1:{}/json", args.debug);
            eprintln!("WebSocket: ws://127.0.0.1:{}/devtools/browser", args.debug);
        }
    }

    // Check for updates in the background
    tokio::spawn(async {
        check_for_updates().await;
    });

    // Start GUI
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 700.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("Continuum"),
        ..Default::default()
    };

    if let Err(err) = eframe::run_native(
        "Continuum",
        options,
        Box::new(move |cc| {
            setup_visuals(&cc.egui_ctx);
            Ok(Box::new(app::ContinuumApp::new(
                machine_id.clone(),
                pairing_code.clone(),
            )))
        }),
    ) {
        tracing::error!(error = %err, "GUI error");
    }
}

/// Generate a 6-character hex ID (e.g. "a1b2c3")
pub fn generate_machine_id() -> String {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(&rand::random::<[u8; 8]>());
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    hasher.write(hostname.as_bytes());
    let full = format!("{:08x}", hasher.finish());
    full[..6].to_string()
}

/// Generate a 6-character pairing code (letters+digits, no ambiguous chars)
pub fn generate_pairing_code() -> String {
    use rand::Rng;
    let charset = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    let mut code = String::with_capacity(6);
    for _ in 0..6 {
        code.push(char::from(charset[rng.gen_range(0..charset.len())]));
    }
    code
}

/// Persistent identity config location
fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("continuum")
}

/// Load saved identity or create new one
fn load_or_create_config() -> (String, String) {
    let dir = config_dir();
    let file = dir.join("identity.toml");

    if let Ok(content) = std::fs::read_to_string(&file) {
        if let Ok(config) = toml::from_str::<PersistedIdentity>(&content) {
            if config.machine_id.len() == 6 && config.pairing_code.len() == 6 {
                tracing::info!(
                    "Loaded saved identity: id={}, code={}",
                    config.machine_id,
                    config.pairing_code
                );
                return (config.machine_id, config.pairing_code);
            }
        }
    }

    let id = generate_machine_id();
    let code = generate_pairing_code();
    save_config(&id, &code);
    tracing::info!("Created new identity: id={}, code={}", id, code);
    (id, code)
}

/// Save identity to disk
pub fn save_config(machine_id: &str, pairing_code: &str) {
    let dir = config_dir();
    let file = dir.join("identity.toml");
    let config = PersistedIdentity {
        machine_id: machine_id.to_string(),
        pairing_code: pairing_code.to_string(),
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(content) = toml::to_string_pretty(&config) {
        let _ = std::fs::write(&file, content);
    }
}

#[derive(Serialize, Deserialize)]
struct PersistedIdentity {
    machine_id: String,
    pairing_code: String,
}

fn setup_visuals(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    ctx.set_style(style);

    let mut vis = egui::Visuals::dark();
    vis.override_text_color = Some(egui::Color32::from_rgb(225, 225, 230));
    vis.widgets.inactive.bg_fill = egui::Color32::from_rgb(40, 40, 48);
    vis.widgets.hovered.bg_fill = egui::Color32::from_rgb(55, 55, 65);
    vis.widgets.active.bg_fill = egui::Color32::from_rgb(60, 65, 85);
    vis.selection.bg_fill = egui::Color32::from_rgb(60, 100, 200);
    vis.extreme_bg_color = egui::Color32::from_rgb(14, 14, 18);
    vis.panel_fill = egui::Color32::from_rgb(18, 18, 24);
    ctx.set_visuals(vis);
}

fn run_replay_gui(path: &PathBuf) {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: Cannot open recording file: {}", e);
            std::process::exit(1);
        }
    };

    let frames = read_csr_frames(file);
    if frames.is_empty() {
        eprintln!("Error: No frames found in recording");
        std::process::exit(1);
    }

    eprintln!("Loaded {} frames from {}", frames.len(), path.display());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_title(format!("Continuum Replay — {}", path.display())),
        ..Default::default()
    };

    let frames_clone = frames;
    if let Err(err) = eframe::run_native(
        "Continuum Replay",
        options,
        Box::new(move |cc| {
            setup_visuals(&cc.egui_ctx);
            Ok(Box::new(ReplayApp::new(frames_clone)))
        }),
    ) {
        eprintln!("Error: {}", err);
    }
}

struct ReplayApp {
    frames: Vec<(Vec<u8>, std::time::Duration)>,
    current_frame: usize,
    playing: bool,
    texture: Option<egui::TextureHandle>,
    last_frame_time: std::time::Instant,
}

impl ReplayApp {
    fn new(frames: Vec<(Vec<u8>, std::time::Duration)>) -> Self {
        Self {
            frames,
            current_frame: 0,
            playing: false,
            texture: None,
            last_frame_time: std::time::Instant::now(),
        }
    }
}

impl eframe::App for ReplayApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("replay_controls").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button(if self.playing {
                        "⏸ Pause"
                    } else {
                        "▶ Play"
                    })
                    .clicked()
                {
                    self.playing = !self.playing;
                    if self.playing {
                        self.last_frame_time = std::time::Instant::now();
                    }
                }

                if ui.button("⏮ Reset").clicked() {
                    self.current_frame = 0;
                    self.texture = None;
                }

                let slider = egui::Slider::new(
                    &mut self.current_frame,
                    0..=self.frames.len().saturating_sub(1),
                )
                .text("Frame");
                ui.add(slider);

                ui.label(format!("{}/{}", self.current_frame + 1, self.frames.len()));
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.current_frame < self.frames.len() {
                let (ref data, _) = self.frames[self.current_frame];
                if let Ok(img) = image::load_from_memory(data) {
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let pixels: Vec<egui::Color32> = rgba
                        .pixels()
                        .map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
                        .collect();
                    let color_image = egui::ColorImage { size, pixels };
                    self.texture = Some(ui.ctx().load_texture(
                        "replay_frame",
                        color_image,
                        egui::TextureOptions::default(),
                    ));
                }

                if let Some(ref tex) = self.texture {
                    let avail = ui.available_size();
                    let aspect = tex.size_vec2().x / tex.size_vec2().y;
                    let (w, h) = if avail.x / avail.y > aspect {
                        (avail.y * aspect, avail.y)
                    } else {
                        (avail.x, avail.x / aspect)
                    };
                    ui.image((tex.id(), egui::vec2(w, h)));
                }
            }
        });

        if self.playing && self.current_frame < self.frames.len().saturating_sub(1) {
            let (_, ref duration) = self.frames[self.current_frame];
            if self.last_frame_time.elapsed() >= *duration {
                self.current_frame += 1;
                self.texture = None;
                self.last_frame_time = std::time::Instant::now();
            }
            ctx.request_repaint();
        }
    }
}

async fn check_for_updates() {
    let current_version = env!("CARGO_PKG_VERSION");

    let url = "https://api.github.com/repos/continuum-remote/continuum/releases/latest";
    match reqwest::Client::new()
        .get(url)
        .header("User-Agent", format!("continuum/{}", current_version))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(tag) = json.get("tag_name").and_then(|v| v.as_str()) {
                    let latest = tag.trim_start_matches('v');
                    if latest != current_version {
                        eprintln!();
                        eprintln!("╔════════════════════════════════════════════════════════════╗");
                        eprintln!(
                            "║  A new version of Continuum is available: {:<15} ║",
                            latest
                        );
                        eprintln!("║  You are running: {:<41} ║", current_version);
                        eprintln!(
                            "║  Download: https://github.com/continuum-remote/continuum/releases ║"
                        );
                        eprintln!("╚════════════════════════════════════════════════════════════╝");
                        eprintln!();
                    }
                }
            }
        }
        Err(_) => {
            // Silently ignore — network might not be available
        }
    }
}

fn read_csr_frames(file: std::fs::File) -> Vec<(Vec<u8>, std::time::Duration)> {
    use std::io::{BufReader, Read};
    let mut reader = BufReader::new(file);
    let mut frames = Vec::new();
    let mut last_ts: Option<chrono::DateTime<chrono::Utc>> = None;

    loop {
        let mut len_buf = [0u8; 4];
        match reader.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(_) => break,
        }

        let header_len = u32::from_be_bytes(len_buf) as usize;

        // Skip input records (marker: 0xFFFFFFFF)
        if header_len == 0xFFFFFFFF || header_len > 1024 * 1024 {
            if header_len == 0xFFFFFFFF {
                let mut input_len_buf = [0u8; 4];
                if reader.read_exact(&mut input_len_buf).is_err() {
                    break;
                }
                let input_len = u32::from_be_bytes(input_len_buf) as usize;
                let mut skip = vec![0u8; input_len];
                if reader.read_exact(&mut skip).is_err() {
                    break;
                }
            }
            continue;
        }

        let mut header_data = vec![0u8; header_len];
        if reader.read_exact(&mut header_data).is_err() {
            break;
        }

        let mut data_len_buf = [0u8; 4];
        if reader.read_exact(&mut data_len_buf).is_err() {
            break;
        }
        let data_len = u32::from_be_bytes(data_len_buf) as usize;
        if data_len > 64 * 1024 * 1024 {
            break;
        }

        let mut frame_data = vec![0u8; data_len];
        if reader.read_exact(&mut frame_data).is_err() {
            break;
        }

        let duration = if let Ok(header) = serde_json::from_slice::<serde_json::Value>(&header_data)
        {
            if let Some(ts_str) = header.get("timestamp").and_then(|v| v.as_str()) {
                if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                    let ts_utc = ts.with_timezone(&chrono::Utc);
                    let dur = if let Some(prev) = last_ts {
                        (ts_utc - prev)
                            .to_std()
                            .unwrap_or(std::time::Duration::from_millis(33))
                    } else {
                        std::time::Duration::from_millis(33)
                    };
                    last_ts = Some(ts_utc);
                    dur
                } else {
                    std::time::Duration::from_millis(33)
                }
            } else {
                std::time::Duration::from_millis(33)
            }
        } else {
            std::time::Duration::from_millis(33)
        };

        frames.push((frame_data, duration));
    }

    frames
}
