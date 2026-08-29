// Continuum Client — Designed for everyone, not just engineers.
// ============================================================
// Zero setup: just run it and it finds the server automatically.
// ============================================================

mod app;
mod clipboard;
mod connection;
mod debug_api;
mod input;
mod rendering;
mod settings;
mod widgets;
mod wizard;

use clap::Parser;
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

/// Continuum — Remote access for everyone
#[derive(Parser, Debug)]
#[command(
    name = "continuum-client",
    version,
    about = "Remote access for everyone"
)]
struct Cli {
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Enable debug API on this port (0 = disabled)
    #[arg(long = "debug", default_value = "0", help = "Debug API port")]
    debug_port: u16,

    /// Auto-connect to any discovered server (skip wizard)
    #[arg(long = "auto")]
    auto_connect: bool,

    /// Connect directly to this address
    #[arg(long = "connect")]
    connect_to: Option<String>,

    /// Use this pairing code
    #[arg(long = "pairing-code", default_value = "continuum")]
    pairing_code: String,

    /// Run without GUI (headless/service mode)
    #[arg(long = "headless")]
    headless: bool,
}

#[tokio::main]
async fn main() {
    let args = Cli::parse();

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    // Start debug API server if requested
    if args.debug_port > 0 {
        let debug_addr: std::net::SocketAddr = format!("127.0.0.1:{}", args.debug_port)
            .parse()
            .expect("Invalid debug port");
        let debug_state = debug_api::DebugState {
            app_state: Arc::new(parking_lot::RwLock::new(debug_api::AppDebugState::default())),
        };
        tokio::spawn(async move {
            debug_api::start_debug_server(debug_state, debug_addr).await;
        });
        eprintln!("Debug API: http://127.0.0.1:{}/api/state", args.debug_port);
    }

    if args.headless {
        let machine_id = generate_machine_id();
        let pairing_code = generate_pairing_code();
        eprintln!("Headless mode active — id={machine_id} password={pairing_code}");
        eprintln!("GUI is disabled; connect via debug API when enabled.");
        std::future::pending::<()>().await;
        return;
    }

    // Generate local identity
    let machine_id = generate_machine_id();
    let pairing_code = generate_pairing_code();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([640.0, 480.0])
            .with_title("Continuum"),
        ..Default::default()
    };

    if let Err(err) = eframe::run_native(
        "Continuum",
        options,
        Box::new(move |cc| {
            setup_visual_style(&cc.egui_ctx);
            Ok(Box::new(app::ContinuumApp::new(
                machine_id.clone(),
                pairing_code.clone(),
            )))
        }),
    ) {
        eprintln!("Sorry, something went wrong: {err}");
    }
}

fn setup_visual_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.button_padding = egui::vec2(16.0, 8.0);
    style.spacing.indent = 24.0;
    style.interaction.tooltip_delay = 0.3;
    ctx.set_style(style);

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(egui::Color32::from_rgb(230, 230, 235));
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(40, 40, 48);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(55, 55, 65);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(60, 65, 85);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(22, 22, 28);
    visuals.selection.bg_fill = egui::Color32::from_rgb(60, 100, 200);
    visuals.extreme_bg_color = egui::Color32::from_rgb(14, 14, 18);
    visuals.faint_bg_color = egui::Color32::from_rgb(18, 18, 24);
    visuals.window_fill = egui::Color32::from_rgb(22, 22, 28);
    visuals.panel_fill = egui::Color32::from_rgb(18, 18, 24);
    ctx.set_visuals(visuals);
}

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

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedIdentity {
    machine_id: String,
    pairing_code: String,
}

pub fn save_config(machine_id: &str, pairing_code: &str) {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("continuum");
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

pub fn trust_server(fingerprint: &str, addr: &str) {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("continuum");
    let _ = std::fs::create_dir_all(&config_dir);
    let trust_file = config_dir.join("trusted-servers.json");

    let mut trusted: serde_json::Value = if trust_file.exists() {
        std::fs::read_to_string(&trust_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if let Some(obj) = trusted.as_object_mut() {
        obj.insert(
            fingerprint.to_string(),
            serde_json::json!({
                "addr": addr,
                "trusted_at": chrono::Utc::now().to_rfc3339(),
            }),
        );
    }

    let _ = std::fs::write(
        &trust_file,
        serde_json::to_string_pretty(&trusted).unwrap_or_default(),
    );
}

pub fn is_trusted_server(fingerprint: &str) -> bool {
    let trust_file = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("continuum")
        .join("trusted-servers.json");

    if !trust_file.exists() {
        return false;
    }

    let content = std::fs::read_to_string(&trust_file).unwrap_or_default();
    let trusted: serde_json::Value =
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}));

    trusted
        .as_object()
        .map(|o| o.contains_key(fingerprint))
        .unwrap_or(false)
}
