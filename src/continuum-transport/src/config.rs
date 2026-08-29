use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "continuum-server",
    version,
    about = "Continuum remote desktop server"
)]
pub struct ServerArgs {
    #[arg(short, long, help = "Listen address")]
    pub listen: Option<SocketAddr>,

    #[arg(short, long, help = "Pairing code")]
    pub pairing_code: Option<String>,

    #[arg(short, long, help = "Path to config file")]
    pub config: Option<PathBuf>,

    #[arg(short, long, help = "JPEG quality (1-100)")]
    pub quality: Option<u8>,

    #[arg(short, long, help = "Target FPS")]
    pub fps: Option<u32>,

    #[arg(long, help = "Log level (trace, debug, info, warn, error)")]
    pub log_level: Option<String>,

    #[arg(long, help = "Log format (text, json)")]
    pub log_format: Option<String>,

    #[arg(long, help = "Path to TLS certificate file")]
    pub cert_file: Option<PathBuf>,

    #[arg(long, help = "Path to TLS private key file")]
    pub key_file: Option<PathBuf>,

    #[arg(long, help = "Disable E2E encryption (for local testing only)")]
    pub insecure: bool,

    #[arg(long, help = "Directory for session recordings (default: disabled)")]
    pub record_dir: Option<PathBuf>,

    #[arg(
        long,
        help = "Write frame-level audit log to this JSONL file (disabled if not set)"
    )]
    pub audit_frames: Option<PathBuf>,

    /// Enable verbose/debug logging
    #[arg(long, short)]
    pub verbose: bool,

    #[arg(long, help = "Enable remote WebView2 debugging (requires WebView2 with --remote-debugging-port)")]
    pub remote_debug: bool,

    #[arg(long, default_value = "auto", help = "Encoder: auto, cpu, nvenc, amf, vaapi, videotoolbox")]
    pub encoder: String,

    #[arg(long, default_value_t = false, help = "Require PAKE pairing; disable legacy plaintext pairing code")]
    pub require_pake: bool,
}

#[derive(Parser, Debug, Clone)]
#[command(
    name = "continuum-client",
    version,
    about = "Continuum remote desktop client"
)]
pub struct ClientArgs {
    #[arg(short, long, help = "Server address to connect to")]
    pub connect: Option<SocketAddr>,

    #[arg(short, long, help = "Pairing code")]
    pub pairing_code: Option<String>,

    #[arg(short, long, help = "Path to config file")]
    pub config: Option<PathBuf>,

    #[arg(long, help = "Log level (trace, debug, info, warn, error)")]
    pub log_level: Option<String>,

    #[arg(long, help = "Log format (text, json)")]
    pub log_format: Option<String>,

    #[arg(long, help = "Client display name")]
    pub name: Option<String>,

    #[arg(long, help = "Enable view-only mode")]
    pub view_only: bool,

    #[arg(long, help = "Disable E2E encryption (for local testing only)")]
    pub insecure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: SocketAddr,

    #[serde(default = "default_pairing_code")]
    pub pairing_code: String,

    #[serde(default = "default_quality")]
    pub quality: u8,

    #[serde(default = "default_fps")]
    pub target_fps: u32,

    #[serde(default = "default_max_clients")]
    pub max_clients: usize,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    #[serde(default = "default_log_format")]
    pub log_format: String,

    #[serde(default)]
    pub cert_file: Option<PathBuf>,

    #[serde(default)]
    pub key_file: Option<PathBuf>,

    #[serde(default = "default_true")]
    pub persist_cert: bool,

    #[serde(default = "default_rate_limit_attempts")]
    pub rate_limit_attempts: u32,

    #[serde(default = "default_rate_limit_window_secs")]
    pub rate_limit_window_secs: u64,

    #[serde(default)]
    pub record_dir: Option<PathBuf>,

    #[serde(default)]
    pub audit_frames: Option<PathBuf>,

    #[serde(default)]
    pub insecure: bool,

    #[serde(default)]
    pub remote_debug: bool,

    #[serde(default = "default_encoder")]
    pub encoder: String,

    #[serde(default)]
    pub require_pake: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: default_listen_addr(),
            pairing_code: default_pairing_code(),
            quality: default_quality(),
            target_fps: default_fps(),
            max_clients: default_max_clients(),
            log_level: default_log_level(),
            log_format: default_log_format(),
            cert_file: None,
            key_file: None,
            persist_cert: true,
            rate_limit_attempts: default_rate_limit_attempts(),
            rate_limit_window_secs: default_rate_limit_window_secs(),
            record_dir: None,
            audit_frames: None,
            insecure: false,
            remote_debug: false,
            encoder: default_encoder(),
            require_pake: default_require_pake(),
            }
    }
}

impl ServerConfig {
    pub fn load(args: &ServerArgs) -> Result<Self> {
        let mut config = if let Some(config_path) = &args.config {
            let content = std::fs::read_to_string(config_path).with_context(|| {
                format!("Failed to read config file: {}", config_path.display())
            })?;
            toml::from_str::<ServerConfig>(&content).with_context(|| {
                format!("Failed to parse config file: {}", config_path.display())
            })?
        } else {
            let default_paths = [
                dirs::config_dir().map(|p| p.join("continuum").join("server.toml")),
                Some(PathBuf::from("continuum-server.toml")),
            ];
            let mut loaded = None;
            for path in default_paths.into_iter().flatten() {
                if path.exists() {
                    let content = std::fs::read_to_string(&path)?;
                    loaded = Some(toml::from_str::<ServerConfig>(&content)?);
                    tracing::info!(path = %path.display(), "Loaded config file");
                    break;
                }
            }
            loaded.unwrap_or_default()
        };

        // CLI overrides (highest priority)
        if let Some(addr) = args.listen {
            config.listen_addr = addr;
        }
        if let Some(code) = &args.pairing_code {
            config.pairing_code = code.clone();
        }
        if let Some(q) = args.quality {
            config.quality = q.clamp(1, 100);
        }
        if let Some(fps) = args.fps {
            config.target_fps = fps.clamp(1, 240);
        }
        if let Some(level) = &args.log_level {
            config.log_level = level.clone();
        }
        if let Some(fmt) = &args.log_format {
            config.log_format = fmt.clone();
        }
        if args.cert_file.is_some() {
            config.cert_file = args.cert_file.clone();
        }
        if args.key_file.is_some() {
            config.key_file = args.key_file.clone();
        }
        if args.insecure {
            config.insecure = true;
        }
        if args.remote_debug {
            config.remote_debug = true;
        }
        if let Some(dir) = &args.record_dir {
            config.record_dir = Some(dir.clone());
        }
        if let Some(path) = &args.audit_frames {
            config.audit_frames = Some(path.clone());
        }
        if args.encoder != "auto" {
            config.encoder = args.encoder.clone();
        }
        if args.require_pake {
            config.require_pake = true;
        }

        // Environment variable overrides (no K8s secrets needed)
        if let Ok(code) = std::env::var("CONTINUUM_PAIRING_CODE") {
            config.pairing_code = code;
        }
        if let Ok(addr) = std::env::var("CONTINUUM_LISTEN_ADDR") {
            if let Ok(parsed) = addr.parse() {
                config.listen_addr = parsed;
            }
        }
        if let Ok(q) = std::env::var("CONTINUUM_QUALITY") {
            if let Ok(v) = q.parse::<u8>() {
                config.quality = v.clamp(1, 100);
            }
        }
        if let Ok(fps) = std::env::var("CONTINUUM_FPS") {
            if let Ok(v) = fps.parse::<u32>() {
                config.target_fps = v.clamp(1, 240);
            }
        }
        if let Ok(enc) = std::env::var("CONTINUUM_ENCODER") {
            config.encoder = enc;
        }

        Ok(config)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    #[serde(default = "default_server_addr")]
    pub server_addr: SocketAddr,

    #[serde(default = "default_pairing_code")]
    pub pairing_code: String,

    #[serde(default = "default_client_name")]
    pub client_name: String,

    #[serde(default)]
    pub view_only: bool,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    #[serde(default = "default_log_format")]
    pub log_format: String,

    #[serde(default = "default_true")]
    pub auto_reconnect: bool,

    #[serde(default = "default_max_reconnect_delay")]
    pub max_reconnect_delay_ms: u64,

    #[serde(default = "default_true")]
    pub enable_clipboard: bool,

    #[serde(default = "default_true")]
    pub enable_audio: bool,

    #[serde(default)]
    pub insecure: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_addr: default_server_addr(),
            pairing_code: default_pairing_code(),
            client_name: default_client_name(),
            view_only: false,
            log_level: default_log_level(),
            log_format: default_log_format(),
            auto_reconnect: true,
            max_reconnect_delay_ms: default_max_reconnect_delay(),
            enable_clipboard: true,
            enable_audio: true,
            insecure: false,
        }
    }
}

impl ClientConfig {
    pub fn load(args: &ClientArgs) -> Result<Self> {
        let mut config = if let Some(config_path) = &args.config {
            let content = std::fs::read_to_string(config_path).with_context(|| {
                format!("Failed to read config file: {}", config_path.display())
            })?;
            toml::from_str::<ClientConfig>(&content).with_context(|| {
                format!("Failed to parse config file: {}", config_path.display())
            })?
        } else {
            let default_paths = [
                dirs::config_dir().map(|p| p.join("continuum").join("client.toml")),
                Some(PathBuf::from("continuum-client.toml")),
            ];
            let mut loaded = None;
            for path in default_paths.into_iter().flatten() {
                if path.exists() {
                    let content = std::fs::read_to_string(&path)?;
                    loaded = Some(toml::from_str::<ClientConfig>(&content)?);
                    tracing::info!(path = %path.display(), "Loaded config file");
                    break;
                }
            }
            loaded.unwrap_or_default()
        };

        if let Some(addr) = args.connect {
            config.server_addr = addr;
        }
        if let Some(code) = &args.pairing_code {
            config.pairing_code = code.clone();
        }
        if let Some(name) = &args.name {
            config.client_name = name.clone();
        }
        if args.view_only {
            config.view_only = true;
        }
        if let Some(level) = &args.log_level {
            config.log_level = level.clone();
        }
        if let Some(fmt) = &args.log_format {
            config.log_format = fmt.clone();
        }
        if args.insecure {
            config.insecure = true;
        }

        Ok(config)
    }
}

fn default_listen_addr() -> SocketAddr {
    "0.0.0.0:4433"
        .parse()
        .expect("Hardcoded listen address is valid")
}

fn default_server_addr() -> SocketAddr {
    "127.0.0.1:4433"
        .parse()
        .expect("Hardcoded server address is valid")
}

fn default_pairing_code() -> String {
    "continuum".to_string()
}

fn default_quality() -> u8 {
    85
}

fn default_fps() -> u32 {
    30
}

fn default_max_clients() -> usize {
    16
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "text".to_string()
}

fn default_client_name() -> String {
    "Continuum Client".to_string()
}

fn default_true() -> bool {
    true
}

fn default_max_reconnect_delay() -> u64 {
    30000
}

fn default_rate_limit_attempts() -> u32 {
    5
}

fn default_rate_limit_window_secs() -> u64 {
    60
}

fn default_encoder() -> String {
    "auto".to_string()
}

fn default_require_pake() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(
            config.listen_addr,
            "0.0.0.0:4433".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(config.pairing_code, "continuum");
        assert_eq!(config.quality, 85);
        assert_eq!(config.target_fps, 30);
        assert_eq!(config.max_clients, 16);
    }

    #[test]
    fn test_client_config_default() {
        let config = ClientConfig::default();
        assert_eq!(
            config.server_addr,
            "127.0.0.1:4433".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(config.pairing_code, "continuum");
        assert_eq!(config.client_name, "Continuum Client");
        assert!(!config.view_only);
        assert!(config.auto_reconnect);
    }

    #[test]
    fn test_server_config_serialization() {
        let config = ServerConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let decoded: ServerConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(decoded.quality, 85);
        assert_eq!(decoded.target_fps, 30);
    }

    #[test]
    fn test_client_config_serialization() {
        let config = ClientConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let decoded: ClientConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(decoded.pairing_code, "continuum");
    }

    #[test]
    fn test_default_functions() {
        assert_eq!(
            default_listen_addr(),
            "0.0.0.0:4433".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            default_server_addr(),
            "127.0.0.1:4433".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(default_pairing_code(), "continuum");
        assert_eq!(default_quality(), 85);
        assert_eq!(default_fps(), 30);
        assert_eq!(default_max_clients(), 16);
        assert_eq!(default_log_level(), "info");
        assert_eq!(default_log_format(), "text");
        assert_eq!(default_client_name(), "Continuum Client");
        assert!(default_true());
        assert_eq!(default_max_reconnect_delay(), 30000);
        assert_eq!(default_rate_limit_attempts(), 5);
        assert_eq!(default_rate_limit_window_secs(), 60);
    }
}
