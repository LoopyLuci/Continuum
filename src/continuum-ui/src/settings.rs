use serde::{Deserialize, Serialize};

/// General settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    pub auto_start: bool,
    pub minimize_to_tray: bool,
    pub show_notifications: bool,
    pub language: String,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            auto_start: false,
            minimize_to_tray: true,
            show_notifications: true,
            language: "en".to_string(),
        }
    }
}

/// Network settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSettings {
    pub listen_address: String,
    pub port: u16,
    pub enable_upnp: bool,
    pub stun_servers: Vec<String>,
    pub turn_servers: Vec<String>,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            listen_address: "0.0.0.0".to_string(),
            port: 4433,
            enable_upnp: true,
            stun_servers: vec![
                "stun:stun1.l.google.com:19302".to_string(),
                "stun:stun2.l.google.com:19302".to_string(),
            ],
            turn_servers: Vec::new(),
        }
    }
}

/// Video settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSettings {
    pub codec: String,
    pub quality: u8,
    pub fps: u32,
    pub hardware_acceleration: bool,
    pub max_bitrate_kbps: u32,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            codec: "jpeg".to_string(),
            quality: 80,
            fps: 30,
            hardware_acceleration: true,
            max_bitrate_kbps: 0,
        }
    }
}

/// Security settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySettings {
    pub require_pairing: bool,
    pub auto_accept: bool,
    pub session_timeout_secs: u64,
    pub enable_audit_log: bool,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            require_pairing: true,
            auto_accept: false,
            session_timeout_secs: 3600,
            enable_audit_log: true,
        }
    }
}

/// Settings panel
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsPanel {
    pub general: GeneralSettings,
    pub network: NetworkSettings,
    pub video: VideoSettings,
    pub security: SecuritySettings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = SettingsPanel::default();
        assert_eq!(settings.general.language, "en");
        assert_eq!(settings.network.port, 4433);
        assert_eq!(settings.video.codec, "jpeg");
        assert!(settings.security.require_pairing);
    }
}
