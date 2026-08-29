use serde::{Deserialize, Serialize};

pub type PluginResult = Result<Vec<u8>, PluginError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginError {
    pub code: u32,
    pub message: String,
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for PluginError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub hooks: Vec<String>,
    pub permissions: Vec<String>,
    pub wasm_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameData {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub timestamp_us: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Rgba,
    Bgra,
    Nv12,
    I420,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputEventData {
    pub event_type: String,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub button: Option<String>,
    pub key: Option<String>,
    pub modifiers: Vec<String>,
    pub timestamp_us: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    pub client_name: String,
    pub pairing_code: String,
    pub client_public_key: Option<Vec<u8>>,
    pub metadata: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub accepted: bool,
    pub session_token: Option<String>,
    pub permissions: PluginPermissions,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginPermissions {
    pub can_view: bool,
    pub can_control: bool,
    pub can_clipboard: bool,
    pub can_audio: bool,
    pub can_file_transfer: bool,
    pub custom_tags: Vec<String>,
}

impl Default for PluginPermissions {
    fn default() -> Self {
        Self {
            can_view: true,
            can_control: false,
            can_clipboard: false,
            can_audio: false,
            can_file_transfer: false,
            custom_tags: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "hook")]
pub enum PluginHook {
    #[serde(rename = "on_frame_captured")]
    OnFrameCaptured(FrameData),
    #[serde(rename = "on_frame_received")]
    OnFrameReceived(FrameData),
    #[serde(rename = "on_input_event")]
    OnInputEvent(InputEventData),
    #[serde(rename = "on_authenticate")]
    OnAuthenticate(AuthRequest),
    #[serde(rename = "on_connection_opened")]
    OnConnectionOpened,
    #[serde(rename = "on_connection_closed")]
    OnConnectionClosed,
    #[serde(rename = "on_settings_changed")]
    OnSettingsChanged { key: String, value: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "hook")]
pub enum PluginHookResult {
    #[serde(rename = "on_frame_captured")]
    OnFrameCaptured(FrameData),
    #[serde(rename = "on_frame_received")]
    OnFrameReceived(FrameData),
    #[serde(rename = "on_input_event")]
    OnInputEvent(InputEventData),
    #[serde(rename = "on_authenticate")]
    OnAuthenticate(AuthResponse),
    #[serde(rename = "on_connection_opened")]
    OnConnectionOpened,
    #[serde(rename = "on_connection_closed")]
    OnConnectionClosed,
    #[serde(rename = "on_settings_changed")]
    OnSettingsChanged,
}

pub trait ContinuumPlugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;
    fn handle_hook(&self, hook: PluginHook) -> PluginResult;
    fn on_load(&self) -> PluginResult {
        Ok(Vec::new())
    }
    fn on_unload(&self) -> PluginResult {
        Ok(Vec::new())
    }
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.name.is_empty() {
            errors.push("Plugin name is required".to_string());
        }
        if semver_version_is_valid(&self.version) {
            // valid
        } else {
            errors.push(format!("Invalid version: {}", self.version));
        }
        if self.wasm_path.is_empty() {
            errors.push("wasm_path is required".to_string());
        }

        let valid_hooks = [
            "on_frame_captured",
            "on_frame_received",
            "on_input_event",
            "on_authenticate",
            "on_connection_opened",
            "on_connection_closed",
            "on_settings_changed",
        ];

        for hook in &self.hooks {
            if !valid_hooks.contains(&hook.as_str()) {
                errors.push(format!("Unknown hook: {}", hook));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

fn semver_version_is_valid(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| p.parse::<u32>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_error_display() {
        let err = PluginError {
            code: 404,
            message: "Not found".to_string(),
        };
        assert_eq!(format!("{}", err), "[404] Not found");
    }

    #[test]
    fn test_plugin_manifest_valid() {
        let manifest = PluginManifest {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            author: Some("Test".to_string()),
            description: Some("A test plugin".to_string()),
            hooks: vec!["on_frame_captured".to_string()],
            permissions: vec!["view".to_string()],
            wasm_path: "plugin.wasm".to_string(),
        };
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn test_plugin_manifest_empty_name_invalid() {
        let manifest = PluginManifest {
            name: "".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            description: None,
            hooks: vec![],
            permissions: vec![],
            wasm_path: "plugin.wasm".to_string(),
        };
        let result = manifest.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("name")));
    }

    #[test]
    fn test_plugin_manifest_invalid_version() {
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "not-a-version".to_string(),
            author: None,
            description: None,
            hooks: vec![],
            permissions: vec![],
            wasm_path: "plugin.wasm".to_string(),
        };
        let result = manifest.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_plugin_manifest_empty_wasm_path() {
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            description: None,
            hooks: vec![],
            permissions: vec![],
            wasm_path: "".to_string(),
        };
        let result = manifest.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_plugin_manifest_unknown_hook() {
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            description: None,
            hooks: vec!["on_fake_hook".to_string()],
            permissions: vec![],
            wasm_path: "plugin.wasm".to_string(),
        };
        let result = manifest.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_semver_version_valid() {
        assert!(semver_version_is_valid("1.0.0"));
        assert!(semver_version_is_valid("0.1.0"));
        assert!(semver_version_is_valid("10.20.30"));
    }

    #[test]
    fn test_semver_version_invalid() {
        assert!(!semver_version_is_valid(""));
        assert!(!semver_version_is_valid("1.0"));
        assert!(!semver_version_is_valid("1.0.0.0"));
        assert!(!semver_version_is_valid("a.b.c"));
        assert!(!semver_version_is_valid("1.0.x"));
    }

    #[test]
    fn test_plugin_permissions_default() {
        let perms = PluginPermissions::default();
        assert!(perms.can_view);
        assert!(!perms.can_control);
        assert!(!perms.can_clipboard);
        assert!(!perms.can_audio);
        assert!(!perms.can_file_transfer);
        assert!(perms.custom_tags.is_empty());
    }

    #[test]
    fn test_pixel_format_serialization() {
        let rgba = PixelFormat::Rgba;
        let json = serde_json::to_string(&rgba).unwrap();
        assert_eq!(json, "\"rgba\"");

        let nv12 = PixelFormat::Nv12;
        let json = serde_json::to_string(&nv12).unwrap();
        assert_eq!(json, "\"nv12\"");
    }

    #[test]
    fn test_frame_data_serialization() {
        let frame = FrameData {
            data: vec![1, 2, 3],
            width: 640,
            height: 480,
            format: PixelFormat::Rgba,
            timestamp_us: 12345,
        };
        let json = serde_json::to_string(&frame).unwrap();
        let decoded: FrameData = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.width, 640);
        assert_eq!(decoded.height, 480);
        assert_eq!(decoded.format, PixelFormat::Rgba);
    }

    #[test]
    fn test_auth_request_serialization() {
        let req = AuthRequest {
            client_name: "test-client".to_string(),
            pairing_code: "code123".to_string(),
            client_public_key: Some(vec![1, 2, 3]),
            metadata: std::collections::HashMap::new(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: AuthRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.client_name, "test-client");
    }

    #[test]
    fn test_auth_response_serialization() {
        let resp = AuthResponse {
            accepted: true,
            session_token: Some("token".to_string()),
            permissions: PluginPermissions::default(),
            error_message: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: AuthResponse = serde_json::from_str(&json).unwrap();
        assert!(decoded.accepted);
    }

    #[test]
    fn test_input_event_data_serialization() {
        let event = InputEventData {
            event_type: "click".to_string(),
            x: Some(100),
            y: Some(200),
            button: Some("left".to_string()),
            key: None,
            modifiers: vec!["ctrl".to_string()],
            timestamp_us: 999,
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: InputEventData = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.event_type, "click");
        assert_eq!(decoded.x, Some(100));
    }

    #[test]
    fn test_plugin_hook_serialization() {
        let hook = PluginHook::OnConnectionOpened;
        let json = serde_json::to_string(&hook).unwrap();
        assert!(json.contains("\"hook\":\"on_connection_opened\""));

        let hook = PluginHook::OnConnectionClosed;
        let json = serde_json::to_string(&hook).unwrap();
        assert!(json.contains("\"hook\":\"on_connection_closed\""));
    }

    #[test]
    fn test_plugin_hook_settings_changed() {
        let hook = PluginHook::OnSettingsChanged {
            key: "quality".to_string(),
            value: "85".to_string(),
        };
        let json = serde_json::to_string(&hook).unwrap();
        assert!(json.contains("\"hook\":\"on_settings_changed\""));
        assert!(json.contains("\"key\":\"quality\""));
    }
}
