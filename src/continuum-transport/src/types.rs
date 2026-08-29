use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const PROTOCOL_VERSION: u32 = 2;
pub const ALPN: &[u8] = b"apq-2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum ApqStreamType {
    Media = 1,
    Intent = 2,
    Clipboard = 3,
    FileTransfer = 4,
    Audio = 5,
    Debug = 6,
}

impl From<u32> for ApqStreamType {
    fn from(value: u32) -> Self {
        match value {
            1 => Self::Media,
            2 => Self::Intent,
            3 => Self::Clipboard,
            4 => Self::FileTransfer,
            5 => Self::Audio,
            6 => Self::Debug,
            _ => Self::Media,
        }
    }
}

impl fmt::Display for ApqStreamType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Media => write!(f, "Media"),
            Self::Intent => write!(f, "Intent"),
            Self::Clipboard => write!(f, "Clipboard"),
            Self::FileTransfer => write!(f, "FileTransfer"),
            Self::Audio => write!(f, "Audio"),
            Self::Debug => write!(f, "Debug"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameSemantics {
    pub content_type: ContentType,
    pub width: u32,
    pub height: u32,
    pub quality: u8,
    pub frame_number: u64,
    pub timestamp: DateTime<Utc>,
    pub is_keyframe: bool,
    pub monitor_id: u32,
    pub encode_time_us: u64,
}

impl Default for FrameSemantics {
    fn default() -> Self {
        Self {
            content_type: ContentType::Jpeg,
            width: 0,
            height: 0,
            quality: 85,
            frame_number: 0,
            timestamp: Utc::now(),
            is_keyframe: true,
            monitor_id: 0,
            encode_time_us: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentType {
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/png")]
    Png,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub label: String,
    pub importance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingHandshake {
    pub pairing_code: String,
    pub client_name: String,
    pub protocol_version: u32,
    #[serde(default)]
    pub client_e2e_public: Vec<u8>,
    #[serde(default)]
    pub pake_encrypted_key: Vec<u8>,
}

impl Default for PairingHandshake {
    fn default() -> Self {
        Self {
            pairing_code: String::new(),
            client_name: "Continuum Client".to_string(),
            protocol_version: PROTOCOL_VERSION,
            client_e2e_public: Vec::new(),
            pake_encrypted_key: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingResponse {
    pub accepted: bool,
    pub message: String,
    pub session_token: Option<String>,
    pub permissions: Permissions,
    #[serde(default)]
    pub server_e2e_public: Vec<u8>,
    #[serde(default)]
    pub pake_encrypted_key: Vec<u8>,
    #[serde(default)]
    pub resume_token: Option<String>,
    #[serde(default)]
    pub sas_words: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permissions {
    pub can_view: bool,
    pub can_control: bool,
    pub can_clipboard: bool,
    pub can_file_transfer: bool,
    pub can_audio: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            can_view: true,
            can_control: true,
            can_clipboard: true,
            can_file_transfer: true,
            can_audio: true,
        }
    }
}

impl Permissions {
    pub fn view_only() -> Self {
        Self {
            can_view: true,
            can_control: false,
            can_clipboard: false,
            can_file_transfer: false,
            can_audio: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputAction {
    #[serde(rename = "mouse_move")]
    MouseMove,
    #[serde(rename = "mouse_click")]
    MouseClick,
    #[serde(rename = "mouse_down")]
    MouseDown,
    #[serde(rename = "mouse_up")]
    MouseUp,
    #[serde(rename = "mouse_scroll")]
    MouseScroll,
    #[serde(rename = "key_press")]
    KeyPress,
    #[serde(rename = "key_down")]
    KeyDown,
    #[serde(rename = "key_up")]
    KeyUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    #[serde(rename = "left")]
    Left,
    #[serde(rename = "right")]
    Right,
    #[serde(rename = "middle")]
    Middle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteInputEvent {
    pub action: InputAction,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub button: Option<MouseButton>,
    pub key: Option<String>,
    pub modifiers: Option<ModifierKeys>,
    pub scroll_x: Option<f32>,
    pub scroll_y: Option<f32>,
    pub monitor_id: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModifierKeys {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardData {
    pub content_type: ClipboardContentType,
    pub data: Vec<u8>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardContentType {
    #[serde(rename = "text/plain")]
    Text,
    #[serde(rename = "image/png")]
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTransferRequest {
    pub filename: String,
    pub file_size: u64,
    pub file_hash: String,
    pub direction: TransferDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferDirection {
    #[serde(rename = "upload")]
    Upload,
    #[serde(rename = "download")]
    Download,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    pub transfer_id: String,
    pub offset: u64,
    pub data: Vec<u8>,
    pub is_last: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
    pub scale_factor: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub version: String,
    pub hostname: String,
    pub monitors: Vec<MonitorInfo>,
    pub capabilities: ServerCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    pub max_fps: u32,
    pub supports_audio: bool,
    pub supports_clipboard: bool,
    pub supports_file_transfer: bool,
    pub supports_multi_monitor: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub timestamp: DateTime<Utc>,
    pub sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IntentMessage {
    #[serde(rename = "pairing")]
    Pairing(PairingHandshake),
    #[serde(rename = "input")]
    Input(RemoteInputEvent),
    #[serde(rename = "clipboard")]
    Clipboard(ClipboardData),
    #[serde(rename = "file_request")]
    FileRequest(FileTransferRequest),
    #[serde(rename = "file_chunk")]
    FileChunk(FileChunk),
    #[serde(rename = "heartbeat")]
    Heartbeat(Heartbeat),
    #[serde(rename = "select_monitor")]
    SelectMonitor(u32),
    #[serde(rename = "set_quality")]
    SetQuality(u8),
    #[serde(rename = "set_fps")]
    SetFps(u32),
    #[serde(rename = "text_intent")]
    TextIntent(String),
    #[serde(rename = "list_monitors")]
    ListMonitors,
    #[serde(rename = "resume")]
    Resume(ResumeRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeRequest {
    pub token: String,
    pub client_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IntentResponse {
    #[serde(rename = "pairing_result")]
    PairingResult(PairingResponse),
    #[serde(rename = "heartbeat_ack")]
    HeartbeatAck(Heartbeat),
    #[serde(rename = "server_info")]
    ServerInfo(ServerInfo),
    #[serde(rename = "file_response")]
    FileResponse(FileTransferRequest),
    #[serde(rename = "error")]
    Error(ErrorMessage),
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "monitors")]
    Monitors(Vec<MonitorInfo>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMessage {
    pub code: u32,
    pub message: String,
}

impl fmt::Display for ErrorMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Pairing,
    Paired,
    Streaming,
    Reconnecting,
}

impl fmt::Display for ConnectionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => write!(f, "Disconnected"),
            Self::Connecting => write!(f, "Connecting"),
            Self::Connected => write!(f, "Connected"),
            Self::Pairing => write!(f, "Pairing"),
            Self::Paired => write!(f, "Paired"),
            Self::Streaming => write!(f, "Streaming"),
            Self::Reconnecting => write!(f, "Reconnecting"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FrameStats {
    pub fps: f32,
    pub latency_ms: f32,
    pub bandwidth_kbps: f32,
    pub frames_received: u64,
    pub frames_dropped: u64,
    pub bytes_received: u64,
    pub current_quality: u8,
    pub resolution: (u32, u32),
}

impl Default for FrameStats {
    fn default() -> Self {
        Self {
            fps: 0.0,
            latency_ms: 0.0,
            bandwidth_kbps: 0.0,
            frames_received: 0,
            frames_dropped: 0,
            bytes_received: 0,
            current_quality: 85,
            resolution: (0, 0),
        }
    }
}
