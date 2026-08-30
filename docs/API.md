# Continuum API Reference

## Core Types

### ContinuumResult<T>
```rust
pub type ContinuumResult<T> = std::result::Result<T, ContinuumError>;
```

### ContinuumError
```rust
pub enum ContinuumError {
    Transport(String),
    Video(String),
    Crypto(String),
    Capture(String),
    Audio(String),
    Input(String),
    Protocol(String),
    NotSupported(String),
    Internal(String),
}
```

## VideoEncoder Trait

```rust
pub trait VideoEncoder: Send + Sync {
    fn encode(&mut self, frame: &VideoFrame, is_keyframe: bool, quality: u8) 
        -> ContinuumResult<EncodedFrame>;
    fn notify_network(&mut self, rtt_ms: f32, packet_loss: f32);
    fn bitrate_estimate_kbps(&self) -> u32;
    fn request_keyframe(&mut self);
    fn reset(&mut self);
}
```

### VideoFrame
```rust
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub timestamp_us: i64,
}
```

### EncodedFrame
```rust
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub quality: u8,
    pub frame_number: u64,
    pub timestamp_us: i64,
    pub is_keyframe: bool,
    pub encode_time_us: u64,
}
```

## Transport Trait

```rust
pub trait Transport: Send + Sync {
    fn connect(&mut self, addr: &str) -> ContinuumResult<Box<dyn TransportConnection>>;
    fn accept(&mut self) -> ContinuumResult<Box<dyn TransportConnection>>;
    fn close(&mut self);
}

pub trait TransportConnection: Send + Sync {
    fn open_bi(&mut self) -> ContinuumResult<Box<dyn BiStream>>;
    fn accept_bi(&mut self) -> ContinuumResult<Box<dyn BiStream>>;
    fn close(&mut self);
    fn stats(&self) -> ConnectionStats;
}

pub trait BiStream: Send + Sync {
    fn send(&mut self, data: &[u8]) -> ContinuumResult<()>;
    fn recv(&mut self, buf: &mut [u8]) -> ContinuumResult<usize>;
    fn recv_exact(&mut self, buf: &mut [u8]) -> ContinuumResult<()>;
    fn close(&mut self);
}
```

### ConnectionStats
```rust
pub struct ConnectionStats {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub rtt_ms: f32,
    pub packet_loss: f32,
}
```

## CryptoProvider Trait

```rust
pub trait CryptoProvider: Send + Sync {
    fn init_with_secret(&mut self, secret: &[u8]);
    fn is_active(&self) -> bool;
    fn encrypt(&mut self, plaintext: &[u8]) -> ContinuumResult<Vec<u8>>;
    fn decrypt(&mut self, ciphertext: &[u8]) -> ContinuumResult<Vec<u8>>;
    fn algorithm_id(&self) -> u16;
    fn algorithm_name(&self) -> &str;
}
```

## CaptureBackend Trait

```rust
pub trait CaptureBackend: Send + Sync {
    fn enumerate_monitors(&self) -> ContinuumResult<Vec<MonitorInfo>>;
    fn capture_frame(&mut self, monitor_id: u32) -> ContinuumResult<CapturedFrame>;
    fn name(&self) -> &str;
    fn is_available(&self) -> bool;
}
```

### MonitorInfo
```rust
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}
```

### CapturedFrame
```rust
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub timestamp_us: i64,
    pub monitor_id: u32,
}
```

## AudioProcessor Trait

```rust
pub trait AudioProcessor: Send + Sync {
    fn capture(&mut self) -> ContinuumResult<AudioFrame>;
    fn play(&mut self, frame: &AudioFrame);
    fn is_active(&self) -> bool;
    fn name(&self) -> &str;
}
```

### AudioFrame
```rust
pub struct AudioFrame {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub timestamp_us: i64,
}
```

## InputInjector Trait

```rust
pub trait InputInjector: Send + Sync {
    fn apply(&mut self, event: &RemoteInputEvent) -> ContinuumResult<()>;
    fn name(&self) -> &str;
    fn is_available(&self) -> bool;
}
```

### RemoteInputEvent
```rust
pub struct RemoteInputEvent {
    pub action: InputAction,
    pub x: Option<u32>,
    pub y: Option<u32>,
    pub button: Option<MouseButton>,
    pub key: Option<String>,
    pub modifiers: Option<ModifierKeys>,
    pub scroll_x: Option<f32>,
    pub scroll_y: Option<f32>,
    pub monitor_id: Option<u32>,
}
```

## ProtocolNegotiator Trait

```rust
pub trait ProtocolNegotiator: Send + Sync {
    fn negotiate(&mut self, peer_versions: &[ProtocolVersion]) 
        -> ContinuumResult<ProtocolVersion>;
    fn supported_versions(&self) -> &[ProtocolVersion];
    fn supports_version(&self, version: &ProtocolVersion) -> bool;
}
```

### ProtocolVersion
```rust
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(1, 0, 0);
pub const MINIMUM_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(1, 0, 0);
```

## Transport Layer Types (continuum-transport)

### ApqStreamType
```rust
pub enum ApqStreamType {
    Media = 1,
    Intent = 2,
    Audio = 5,
    Debug = 6,
    Clipboard = 7,
    FileTransfer = 4,
}
```

### FrameSemantics
```rust
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
```

### Permissions
```rust
pub struct Permissions {
    pub can_control: bool,
    pub can_clipboard: bool,
    pub can_file_transfer: bool,
}

impl Permissions {
    pub fn view_only() -> Self { ... }
    pub fn default() -> Self { ... }
}
```

### ClipboardData
```rust
pub struct ClipboardData {
    pub data: Vec<u8>,
    pub content_type: ClipboardContentType,
}

pub enum ClipboardContentType {
    Text,
    Image,
}
```

### TransferDirection
```rust
pub enum TransferDirection {
    Upload,
    Download,
}
```

### FileTransferRequest
```rust
pub struct FileTransferRequest {
    pub filename: String,
    pub file_size: u64,
    pub file_hash: String,
    pub direction: TransferDirection,
}
```

### FileChunk
```rust
pub struct FileChunk {
    pub transfer_id: String,
    pub offset: u64,
    pub data: Vec<u8>,
    pub is_last: bool,
}
```

### IntentMessage
```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IntentMessage {
    Pairing(PairingHandshake),
    Input(RemoteInputEvent),
    Clipboard(ClipboardData),
    FileRequest(FileTransferRequest),
    FileChunk(FileChunk),
    Heartbeat(Heartbeat),
    SelectMonitor(u32),
    SetQuality(u8),
    SetFps(u32),
    TextIntent(String),
    ListMonitors,
    Resume(ResumeRequest),
}
```

### PairingHandshake
```rust
pub struct PairingHandshake {
    pub pairing_code: String,
    pub client_name: String,
    pub client_e2e_public: Vec<u8>,
    pub pake_encrypted_key: Vec<u8>,
}
```

### PairingResponse
```rust
pub struct PairingResponse {
    pub accepted: bool,
    pub message: String,
    pub session_token: Option<String>,
    pub permissions: Permissions,
    pub server_e2e_public: Vec<u8>,
    pub pake_encrypted_key: Vec<u8>,
    pub resume_token: Option<String>,
    pub sas_words: Vec<String>,
}
```

## Security Types (continuum-security)

### Key Exchange
```rust
pub fn generate_dh_keypair() -> (Vec<u8>, Vec<u8>);
pub fn compute_shared_secret(secret: &[u8], public: &[u8]) -> Vec<u8>;
```

### Double Ratchet
```rust
pub struct RatchetState {
    // Internal state for double-ratchet
}

impl RatchetState {
    pub fn new(shared_secret: Vec<u8>) -> Self;
    pub fn encrypt(&mut self, plaintext: &[u8]) -> EncryptedFrame;
    pub fn decrypt(&mut self, frame: &EncryptedFrame) -> Vec<u8>;
}
```

## Configuration Types

### ServerConfig
```rust
pub struct ServerConfig {
    pub listen: String,
    pub pairing_code: String,
    pub record_dir: Option<PathBuf>,
    pub audit_frames: Option<PathBuf>,
    pub insecure: bool,
    pub encoder: String,
    pub require_pake: bool,
}
```

### ClientConfig
```rust
pub struct ClientConfig {
    pub server_addr: SocketAddr,
    pub pairing_code: String,
    pub client_name: String,
    pub auto_reconnect: bool,
    pub max_reconnect_delay_ms: u64,
    pub view_only: bool,
    pub enable_clipboard: bool,
}
```

## AI Types (continuum-ai)

### AnomalyDetector
```rust
pub struct AnomalyDetector {
    // Window-based anomaly detection
}

impl AnomalyDetector {
    pub fn new(window_size: usize, threshold_sigma: f64) -> Self;
    pub fn record_encode_time(&mut self, time_us: f64);
    pub fn record_frame_size(&mut self, size_bytes: f64);
    pub fn warnings(&self) -> &[AnomalyWarning];
    pub fn is_anomaly(&self, metric: &str, value: f64) -> bool;
}
```

### NetworkForecaster
```rust
pub struct NetworkForecaster;

impl NetworkForecaster {
    pub fn new() -> Self;
    pub fn record_sample(&mut self, sample: NetworkSample);
    pub fn predict(&self) -> Option<NetworkPrediction>;
    pub fn predict_bitrate_kbps(&self, jitter_ms: f32, packet_loss: f32) -> u32;
}
```

## Observability Types (continuum-observability)

### MetricsRegistry
```rust
pub struct MetricsRegistry;

impl MetricsRegistry {
    pub fn new() -> Self;
    pub fn record_encode(&mut self, time_us: u64, size_bytes: u64);
    pub fn record_frame_received(&mut self, size_bytes: u64);
    pub fn record_frame_dropped(&mut self);
}
```

### AuditLogger
```rust
pub struct AuditLogger;

impl AuditLogger {
    pub fn new() -> Self;
    pub fn new_with_file(path: &Path) -> anyhow::Result<Self>;
    pub fn log(&self, event: AuditEvent, client_addr: &str, session_id: &str, message: &str);
}
```

## Testing Types (continuum-test)

### TestResult
```rust
pub struct TestResult {
    pub name: String,
    pub category: TestCategory,
    pub status: TestStatus,
    pub duration_ms: u64,
    pub details: String,
}
```

### TestCategory
```rust
pub enum TestCategory {
    Unit,
    Integration,
    E2E,
    Chaos,
    Performance,
    Smoke,
    Packaging,
}
```

### TestStatus
```rust
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
    Error,
    Timeout,
}
```
