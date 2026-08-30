# Continuum v1.0 Implementation Plan

## Executive Summary

This document provides a comprehensive, actionable plan to transform Continuum from a functional prototype into a production-grade, next-generation remote desktop platform that surpasses all competitors. The plan is organized into 12 workstreams, each designed to be implemented independently while contributing to a cohesive whole.

**Goal:** A remote desktop platform that is:
- **Zero-friction:** Connect two machines with a 6-character code, no networking knowledge needed
- **Universal:** Works on any device, any network, any platform
- **Performant:** Gaming-grade latency, efficient codecs, hardware acceleration
- **Secure:** Post-quantum ready, fully audited, privacy-preserving
- **Modular:** Every component swappable, every feature optional
- **Durable:** Designed to evolve for 100 years

---

## Workstream 1: NAT Traversal & Network Resilience

### 1.1 Problem
Continuum currently cannot traverse NATs or firewalls without manual relay configuration. This is the single biggest barrier to adoption.

### 1.2 Solution: Full ICE Implementation

**Architecture:**
```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        ICE CONNECTION ESTABLISHMENT                          │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌──────────┐         ┌──────────┐         ┌──────────┐                     │
│  │  STUN    │         │  TURN    │         │  Relay   │                     │
│  │  Server  │         │  Server  │         │  Server  │                     │
│  └────┬─────┘         └────┬─────┘         └────┬─────┘                     │
│       │                    │                    │                           │
│       └────────────────────┼────────────────────┘                           │
│                            │                                                │
│  ┌─────────────────────────┴─────────────────────────┐                      │
│  │              ICE Agent (continuum-ice)             │                      │
│  ├───────────────────────────────────────────────────┤                      │
│  │ 1. Gather host candidates (local IPs)             │                      │
│  │ 2. Query STUN for server reflexive candidates     │                      │
│  │ 3. Allocate TURN relay candidates                 │                      │
│  │ 4. Exchange candidates via signaling              │                      │
│  │ 5. Perform connectivity checks                    │                      │
│  │ 6. Select best candidate pair                     │                      │
│  └───────────────────────────────────────────────────┘                      │
│                            │                                                │
│  ┌─────────────────────────┴─────────────────────────┐                      │
│  │              QUIC Transport (continuum-transport)  │                      │
│  └───────────────────────────────────────────────────┘                      │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 1.3 Implementation

**New Crate:** `continuum-ice`

```toml
# src/continuum-ice/Cargo.toml
[package]
name = "continuum-ice"
version.workspace = true
edition.workspace = true

[dependencies]
continuum-core = { path = "../continuum-core" }
stun = "0.7"
turn = "0.8"
webrtc-ice = "0.1"  # or custom implementation
tokio = { workspace = true }
serde = { workspace = true }
```

**Core Types:**

```rust
/// ICE agent for NAT traversal
pub struct IceAgent {
    config: IceConfig,
    candidates: Vec<IceCandidate>,
    state: IceState,
    stun_client: StunClient,
    turn_client: Option<TurnClient>,
}

/// ICE candidate types
#[derive(Debug, Clone)]
pub enum IceCandidateType {
    Host,           // Local IP:port
    ServerReflexive, // Public IP:port (from STUN)
    PeerReflexive,  // Discovered during connectivity check
    Relayed,        // TURN relay
}

#[derive(Debug, Clone)]
pub struct IceCandidate {
    pub candidate_type: IceCandidateType,
    pub address: SocketAddr,
    pub priority: u32,
    pub foundation: String,
    pub protocol: TransportProtocol,
}

impl IceAgent {
    /// Create a new ICE agent with the given configuration
    pub fn new(config: IceConfig) -> Self { ... }
    
    /// Gather all possible candidates
    pub async fn gather_candidates(&mut self) -> ContinuumResult<Vec<IceCandidate>> {
        // 1. Gather host candidates (all local IPs)
        let host = self.gather_host_candidates();
        
        // 2. Query STUN servers for server reflexive candidates
        let srflx = self.stun_client.discover(&self.config.stun_servers).await?;
        
        // 3. Allocate TURN relay (if configured)
        let relay = if let Some(turn) = &self.config.turn_server {
            Some(self.turn_client.allocate(turn).await?)
        } else {
            None
        };
        
        Ok([host, srflx, relay].concat())
    }
    
    /// Perform connectivity checks with remote candidates
    pub async fn connectivity_check(
        &self,
        local: &IceCandidate,
        remote: &IceCandidate,
    ) -> bool { ... }
    
    /// Select the best candidate pair
    pub fn select_best_pair(
        &self,
        local: &[IceCandidate],
        remote: &[IceCandidate],
    ) -> Option<(IceCandidate, IceCandidate)> { ... }
}
```

**STUN Implementation:**

```rust
pub struct StunClient {
    servers: Vec<String>,
}

impl StunClient {
    /// Discover public IP:port via STUN
    pub async fn discover(&self, servers: &[String]) -> ContinuumResult<Vec<IceCandidate>> {
        let mut candidates = Vec::new();
        
        for server in servers {
            let response = self.send_binding_request(server).await?;
            if let Some(mapped) = response.mapped_address {
                candidates.push(IceCandidate {
                    candidate_type: IceCandidateType::ServerReflexive,
                    address: mapped,
                    priority: self.calculate_priority(&mapped),
                    foundation: format!("srflx-{}", server),
                    protocol: TransportProtocol::Udp,
                });
            }
        }
        
        Ok(candidates)
    }
}
```

**TURN Implementation:**

```rust
pub struct TurnClient {
    server: String,
    username: String,
    password: String,
}

impl TurnClient {
    /// Allocate a relay address on the TURN server
    pub async fn allocate(&self, server: &str) -> ContinuumResult<IceCandidate> {
        // Send ALLOCATE request
        // Receive relayed address
        // Return as ICE candidate
    }
    
    /// Send data through the relay
    pub async fn send(&self, data: &[u8], peer: SocketAddr) -> ContinuumResult<()> { ... }
    
    /// Receive data from the relay
    pub async fn recv(&self) -> ContinuumResult<(Vec<u8>, SocketAddr)> { ... }
}
```

### 1.4 Configuration

```toml
[network.ice]
enabled = true
stun_servers = [
    "stun:stun1.l.google.com:19302",
    "stun:stun2.l.google.com:19302",
    "stun:stun.continuum.local:3478",
]
turn_servers = [
    "turn:turn.continuum.local:3478",
]
turn_username = "continuum"
turn_password = ""
# Aggressive nomination for faster connection
aggressive_nomination = true
# Keep-alive interval for NAT binding
keep_alive_interval = "15s"
```

### 1.5 Testing

```rust
#[tokio::test]
async fn test_stun_discovery() {
    let client = StunClient::new();
    let candidates = client.discover(&[
        "stun:stun1.l.google.com:19302".to_string()
    ]).await.unwrap();
    assert!(!candidates.is_empty());
}

#[tokio::test]
async fn test_ice_full_flow() {
    let mut agent = IceAgent::new(IceConfig::default());
    let candidates = agent.gather_candidates().await.unwrap();
    assert!(candidates.len() >= 2); // At least host + srflx
}
```

---

## Workstream 2: Modern Codec Pipeline

### 2.1 Problem
Continuum uses JPEG encoding, which is 5-10x less efficient than modern video codecs. This results in high bandwidth usage and poor video quality.

### 2.2 Solution: Multi-Codec Pipeline with Hardware Acceleration

**Architecture:**
```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        VIDEO CODEC PIPELINE                                 │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐              │
│  │ Capture  │───▶│  Encode  │───▶│  Packet  │───▶│  Send    │              │
│  └──────────┘    └──────────┘    └──────────┘    └──────────┘              │
│                       │                                                     │
│                       ▼                                                     │
│  ┌──────────────────────────────────────────────────────────────────────┐  │
│  │                    Codec Selection Logic                               │  │
│  ├──────────────────────────────────────────────────────────────────────┤  │
│  │ 1. Check hardware encoder availability (NVENC, AMF, VAAPI, VideoToolbox)│ │
│  │ 2. Check content type (screen vs video vs gaming)                     │  │
│  │ 3. Check bandwidth estimate                                           │  │
│  │ 4. Select optimal codec + parameters                                  │  │
│  └──────────────────────────────────────────────────────────────────────┘  │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 2.3 Implementation

**New Crate:** `continuum-codec`

```toml
# src/continuum-codec/Cargo.toml
[package]
name = "continuum-codec"
version.workspace = true
edition.workspace = true

[dependencies]
continuum-core = { path = "../continuum-core" }
openh264 = "0.6"  # Software H.264
nvenc = { version = "0.1", optional = true }  # NVIDIA
amf = { version = "0.1", optional = true }  # AMD
vaapi = { version = "0.1", optional = true }  # Intel/Linux
videotoolbox = { version = "0.1", optional = true }  # macOS

[features]
default = ["software"]
software = ["openh264"]
nvenc = ["dep:nvenc"]
amf = ["dep:amf"]
vaapi = ["dep:vaapi"]
videotoolbox = ["dep:videotoolbox"]
```

**Core Types:**

```rust
/// Codec types supported by Continuum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecType {
    H264,
    H265,
    Av1,
    Vp9,
}

/// Codec capabilities
#[derive(Debug, Clone)]
pub struct CodecCapabilities {
    pub codec_type: CodecType,
    pub hardware_accelerated: bool,
    pub max_resolution: (u32, u32),
    pub max_fps: u32,
    pub supports_b_frames: bool,
}

/// Video encoder implementation
pub struct VideoEncoder {
    inner: Box<dyn VideoEncoderImpl>,
    config: EncoderConfig,
}

trait VideoEncoderImpl: Send + Sync {
    fn encode(&mut self, frame: &VideoFrame) -> ContinuumResult<EncodedFrame>;
    fn set_bitrate(&mut self, kbps: u32);
    fn set_fps(&mut self, fps: u32);
    fn request_keyframe(&mut self);
    fn capabilities(&self) -> CodecCapabilities;
}

impl VideoEncoder {
    /// Create the best available encoder for the current system
    pub fn create_best(config: EncoderConfig) -> ContinuumResult<Self> {
        // Try hardware encoders first
        #[cfg(feature = "nvenc")]
        if let Ok(enc) = NvencEncoder::new(&config) {
            return Ok(Self::new(Box::new(enc)));
        }
        
        #[cfg(feature = "amf")]
        if let Ok(enc) = AmfEncoder::new(&config) {
            return Ok(Self::new(Box::new(enc)));
        }
        
        #[cfg(feature = "vaapi")]
        if let Ok(enc) = VaapiEncoder::new(&config) {
            return Ok(Self::new(Box::new(enc)));
        }
        
        // Fall back to software
        SoftwareEncoder::new(&config).map(|enc| Self::new(Box::new(enc)))
    }
}
```

**Hardware Encoder Support:**

```rust
// NVIDIA NVENC
#[cfg(feature = "nvenc")]
pub struct NvencEncoder {
    encoder: nvenc::Encoder,
}

#[cfg(feature = "nvenc")]
impl VideoEncoderImpl for NvencEncoder {
    fn encode(&mut self, frame: &VideoFrame) -> ContinuumResult<EncodedFrame> {
        // Zero-copy from GPU texture
        // Encode with NVENC
        // Return encoded bitstream
    }
}

// AMD AMF
#[cfg(feature = "amf")]
pub struct AmfEncoder {
    encoder: amf::Encoder,
}

// Intel VAAPI
#[cfg(feature = "vaapi")]
pub struct VaapiEncoder {
    encoder: vaapi::Encoder,
}

// macOS VideoToolbox
#[cfg(feature = "videotoolbox")]
pub struct VtEncoder {
    encoder: videotoolbox::Encoder,
}
```

**Content-Aware Encoding:**

```rust
pub struct ContentAnalyzer {
    // Detects content type for optimal encoding
}

impl ContentAnalyzer {
    pub fn analyze(&self, frame: &VideoFrame) -> ContentType {
        // Analyze frame characteristics:
        // - High frequency detail → text/UI (needs high quality)
        // - Motion vectors → video (needs temporal compression)
        // - Flat regions → desktop (can be lossy)
    }
}

pub enum ContentType {
    Text,      // High quality, no B-frames
    Desktop,   // Balanced quality
    Video,     // Temporal compression, B-frames
    Gaming,    // Low latency, high FPS
}

impl ContentType {
    pub fn optimal_params(&self) -> EncoderParams {
        match self {
            Self::Text => EncoderParams {
                codec: CodecType::H264,
                quality: 95,
                b_frames: 0,
                bitrate_kbps: 5000,
            },
            Self::Desktop => EncoderParams {
                codec: CodecType::H264,
                quality: 85,
                b_frames: 2,
                bitrate_kbps: 3000,
            },
            Self::Video => EncoderParams {
                codec: CodecType::H264,
                quality: 80,
                b_frames: 3,
                bitrate_kbps: 8000,
            },
            Self::Gaming => EncoderParams {
                codec: CodecType::H264,
                quality: 75,
                b_frames: 0,
                bitrate_kbps: 15000,
            },
        }
    }
}
```

### 2.4 Configuration

```toml
[video]
# Preferred codec (auto-detect if not specified)
codec = "auto"  # auto, h264, h265, av1, vp9

# Hardware acceleration
hardware_acceleration = true

# Default quality (1-100)
quality = 85

# Default FPS
fps = 30

# Maximum bitrate (kbps, 0 = unlimited)
max_bitrate = 0

# Content-aware encoding
content_aware = true

# Keyframe interval (seconds)
keyframe_interval = 2

# B-frames (0 for low latency)
b_frames = 2
```

---

## Workstream 3: Local Discovery (mDNS)

### 3.1 Problem
Users must know the IP address or pairing code to connect. No automatic discovery of peers on the local network.

### 3.2 Solution: mDNS + DNS-SD Implementation

**New Crate:** `continuum-discovery`

```toml
# src/continuum-discovery/Cargo.toml
[package]
name = "continuum-discovery"
version.workspace = true
edition.workspace = true

[dependencies]
continuum-core = { path = "../continuum-core" }
mdns-sd = "0.12"
tokio = { workspace = true }
serde = { workspace = true }
```

**Core Types:**

```rust
/// mDNS service discovery
pub struct MdnsDiscovery {
    service_type: String,
    daemon: mdns_sd::ServiceDaemon,
    hostname: String,
    port: u16,
}

/// A discovered peer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPeer {
    pub machine_id: String,
    pub display_name: String,
    pub address: String,
    pub port: u16,
    pub pairing_code: Option<String>,
    pub protocol_version: u16,
    pub capabilities: Vec<String>,
}

impl MdnsDiscovery {
    /// Create a new mDNS discovery service
    pub fn new(port: u16) -> ContinuumResult<Self> {
        let daemon = mdns_sd::ServiceDaemon::new()?;
        Ok(Self {
            service_type: "_continuum._tcp.local.".to_string(),
            daemon,
            hostname: "continuum.local.".to_string(),
            port,
        })
    }
    
    /// Advertise this machine on the local network
    pub fn advertise(
        &self,
        machine_id: &str,
        display_name: &str,
        pairing_code: Option<&str>,
    ) -> ContinuumResult<()> {
        let mut properties = vec![
            ("machine_id", machine_id),
            ("display_name", display_name),
            ("protocol_version", "1"),
        ];
        
        if let Some(code) = pairing_code {
            properties.push(("pairing_code", code));
        }
        
        let service_info = mdns_sd::ServiceInfo::new(
            &self.service_type,
            &self.hostname,
            &self.hostname,
            self.port,
            Some(&properties),
        )?;
        
        self.daemon.register(service_info)?;
        Ok(())
    }
    
    /// Browse for available peers
    pub fn browse(&self) -> ContinuumResult<Vec<DiscoveredPeer>> {
        let receiver = self.daemon.browse(&self.service_type)?;
        let mut peers = Vec::new();
        
        while let Ok(event) = receiver.recv_timeout(Duration::from_millis(100)) {
            match event {
                mdns_sd::ServiceEvent::ServiceResolved(info) => {
                    peers.push(DiscoveredPeer {
                        machine_id: info.get_property("machine_id")
                            .map(|p| p.val().to_string())
                            .unwrap_or_default(),
                        display_name: info.get_property("display_name")
                            .map(|p| p.val().to_string())
                            .unwrap_or_default(),
                        address: info.get_addresses()
                            .iter()
                            .next()
                            .map(|a| a.to_string())
                            .unwrap_or_default(),
                        port: info.get_port(),
                        pairing_code: info.get_property("pairing_code")
                            .map(|p| p.val().to_string()),
                        protocol_version: info.get_property("protocol_version")
                            .and_then(|p| p.val().parse().ok())
                            .unwrap_or(1),
                        capabilities: Vec::new(),
                    });
                }
                _ => {}
            }
        }
        
        Ok(peers)
    }
}
```

---

## Workstream 4: Web Client

### 4.1 Problem
Continuum requires a native client installation. No way to connect from a browser.

### 4.2 Solution: WebRTC-based Web Client

**Architecture:**
```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        WEB CLIENT ARCHITECTURE                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        Browser (TypeScript)                          │   │
│  ├─────────────────────────────────────────────────────────────────────┤   │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐            │   │
│  │  │ WebRTC   │  │ WebCodecs│  │ WebGL    │  │ WebSockets│            │   │
│  │  │ Transport│  │ Decoder  │  │ Renderer │  │ Signaling │            │   │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────┘            │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│                                    ▼                                        │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        Signaling Server (Rust)                       │   │
│  ├─────────────────────────────────────────────────────────────────────┤   │
│  │  - WebSocket connections                                            │   │
│  │  - Session negotiation                                              │   │
│  │  - ICE candidate exchange                                           │   │
│  │  - Pairing code routing                                             │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│                                    ▼                                        │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        Native Server (Rust)                          │   │
│  ├─────────────────────────────────────────────────────────────────────┤   │
│  │  - QUIC transport                                                   │   │
│  │  - H.264/H.265 encoding                                             │   │
│  │  - Screen capture                                                   │   │
│  │  - Input injection                                                  │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 4.3 Implementation

**New Crate:** `continuum-web`

```toml
# src/continuum-web/Cargo.toml
[package]
name = "continuum-web"
version.workspace = true
edition.workspace = true

[dependencies]
continuum-core = { path = "../continuum-core" }
axum = { version = "0.8", features = ["ws"] }
tokio = { workspace = true }
serde = { workspace = true }
tower-http = { version = "0.6", features = ["fs", "cors"] }
```

**Signaling Server:**

```rust
pub struct SignalingServer {
    state: Arc<SignalingState>,
}

struct SignalingState {
    sessions: RwLock<HashMap<String, Session>>,
    peers: RwLock<HashMap<String, Peer>>,
}

struct Session {
    id: String,
    pairing_code: String,
    host: Option<Peer>,
    peer: Option<Peer>,
}

struct Peer {
    id: String,
    socket: WebSocket,
}

impl SignalingServer {
    pub fn new() -> Self {
        Self {
            state: Arc::new(SignalingState::new()),
        }
    }
    
    pub fn router(self) -> Router {
        Router::new()
            .route("/ws", get(Self::websocket_handler))
            .route("/health", get(|| async { "ok" }))
            .layer(CorsLayer::permissive())
            .with_state(self.state)
    }
    
    async fn websocket_handler(
        ws: WebSocketUpgrade,
        State(state): State<Arc<SignalingState>>,
    ) -> Response {
        ws.on_upgrade(|socket| Self::handle_socket(socket, state))
    }
    
    async fn handle_socket(socket: WebSocket, state: Arc<SignalingState>) {
        // Handle WebSocket messages:
        // - join_session { pairing_code }
        // - ice_candidate { candidate }
        // - sdp_offer { sdp }
        // - sdp_answer { sdp }
    }
}
```

**Web Client (TypeScript):**

```typescript
// web-client/src/continuum.ts
export class ContinuumClient {
    private pc: RTCPeerConnection;
    private dc: RTCDataChannel;
    private ws: WebSocket;
    
    async connect(pairingCode: string): Promise<void> {
        // Connect to signaling server
        this.ws = new WebSocket(`wss://relay.continuum.local/ws`);
        
        // Join session
        this.ws.send(JSON.stringify({
            type: 'join_session',
            pairing_code: pairingCode,
        }));
        
        // Create WebRTC peer connection
        this.pc = new RTCPeerConnection({
            iceServers: [
                { urls: 'stun:stun1.l.google.com:19302' },
                { urls: 'stun:stun2.l.google.com:19302' },
            ],
        });
        
        // Handle incoming video stream
        this.pc.ontrack = (event) => {
            const video = document.getElementById('remote-video') as HTMLVideoElement;
            video.srcObject = event.streams[0];
        };
        
        // Create data channel for input
        this.dc = this.pc.createDataChannel('input');
    }
    
    sendInput(event: InputEvent): void {
        this.dc.send(JSON.stringify(event));
    }
}
```

---

## Workstream 5: Mobile Clients

### 5.1 Problem
No mobile support. Can't use Continuum from a phone or tablet.

### 5.2 Solution: Native iOS/Android with Shared Rust Core

**Architecture:**
```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        MOBILE ARCHITECTURE                                  │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        Shared Rust Core                              │   │
│  ├─────────────────────────────────────────────────────────────────────┤   │
│  │  continuum-core (traits)                                            │   │
│  │  continuum-transport (QUIC)                                         │   │
│  │  continuum-codec (H.264)                                            │   │
│  │  continuum-crypto (encryption)                                      │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│                    ┌───────────────┴───────────────┐                        │
│                    ▼                               ▼                        │
│  ┌─────────────────────────────────┐ ┌─────────────────────────────────┐   │
│  │        iOS (Swift)               │ │        Android (Kotlin)          │   │
│  ├─────────────────────────────────┤ ├─────────────────────────────────┤   │
│  │  - SwiftUI interface            │ │  - Jetpack Compose interface    │   │
│  │  - FFI bindings to Rust         │ │  - JNI bindings to Rust         │   │
│  │  - Touch input handling         │ │  - Touch input handling         │   │
│  │  - Camera for QR scanning       │ │  - Camera for QR scanning       │   │
│  │  - Network framework            │ │  - OkHttp/QUIC                  │   │
│  └─────────────────────────────────┘ └─────────────────────────────────┘   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 5.3 Implementation

**FFI Bindings:**

```rust
// src/continuum-ffi/src/lib.rs
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

/// Create a new Continuum client (FFI)
#[no_mangle]
pub extern "C" fn continuum_client_new() -> *mut Client {
    let client = Client::new();
    Box::into_raw(Box::new(client))
}

/// Connect to a server (FFI)
#[no_mangle]
pub extern "C" fn continuum_client_connect(
    client: *mut Client,
    pairing_code: *const c_char,
) -> i32 {
    let client = unsafe { &mut *client };
    let code = unsafe { CStr::from_ptr(pairing_code) }.to_str().unwrap();
    
    match client.connect(code) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// Send input event (FFI)
#[no_mangle]
pub extern "C" fn continuum_client_send_input(
    client: *mut Client,
    event: *const c_char,
) -> i32 {
    let client = unsafe { &mut *client };
    let event_json = unsafe { CStr::from_ptr(event) }.to_str().unwrap();
    
    match client.send_input(event_json) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}
```

**iOS (Swift):**

```swift
// ios/Sources/Continuum/Continuum.swift
import Foundation

public class ContinuumClient {
    private var client: OpaquePointer?
    
    public init() {
        client = continuum_client_new()
    }
    
    public func connect(pairingCode: String) -> Bool {
        return pairingCode.withCString { ptr in
            continuum_client_connect(client, ptr) == 0
        }
    }
    
    public func sendInput(_ event: InputEvent) -> Bool {
        let json = event.toJSON()
        return json.withCString { ptr in
            continuum_client_send_input(client, ptr) == 0
        }
    }
    
    deinit {
        continuum_client_free(client)
    }
}
```

**Android (Kotlin):**

```kotlin
// android/src/main/java/app/continuum/ContinuumClient.kt
package app.continuum

class ContinuumClient {
    private var nativeHandle: Long = 0
    
    init {
        nativeHandle = nativeNew()
    }
    
    fun connect(pairingCode: String): Boolean {
        return nativeConnect(nativeHandle, pairingCode) == 0
    }
    
    fun sendInput(event: InputEvent): Boolean {
        return nativeSendInput(nativeHandle, event.toJSON()) == 0
    }
    
    protected fun finalize() {
        nativeFree(nativeHandle)
    }
    
    private external fun nativeNew(): Long
    private external fun nativeConnect(handle: Long, code: String): Int
    private external fun nativeSendInput(handle: Long, json: String): Int
    private external fun nativeFree(handle: Long)
}
```

---

## Workstream 6: Enterprise Features

### 6.1 Problem
No SSO, LDAP, audit logging, or policy management. Can't deploy in enterprise environments.

### 6.2 Solution: Enterprise Auth & Audit Module

**New Crate:** `continuum-enterprise`

```toml
# src/continuum-enterprise/Cargo.toml
[package]
name = "continuum-enterprise"
version.workspace = true
edition.workspace = true

[dependencies]
continuum-core = { path = "../continuum-core" }
continuum-observability = { path = "../continuum-observability" }
openidconnect = "4"
ldap3 = "0.11"
serde = { workspace = true }
tokio = { workspace = true }
```

**SSO/OIDC:**

```rust
pub struct OidcProvider {
    client: openidconnect::core::CoreClient,
    config: OidcConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
}

impl OidcProvider {
    pub async fn new(config: OidcConfig) -> ContinuumResult<Self> {
        let issuer = openidconnect::IssuerUrl::new(config.issuer_url.clone())?;
        let client = openidconnect::core::CoreClient::from_provider_metadata(
            // Fetch OIDC provider metadata
        );
        Ok(Self { client, config })
    }
    
    pub fn authorize_url(&self) -> (String, String) {
        // Generate authorization URL for user login
    }
    
    pub async fn exchange_code(&self, code: &str) -> ContinuumResult<IdToken> {
        // Exchange authorization code for ID token
    }
}
```

**LDAP:**

```rust
pub struct LdapProvider {
    pool: ldap3::LdapConnPool,
    config: LdapConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapConfig {
    pub url: String,
    pub bind_dn: String,
    pub bind_password: String,
    pub base_dn: String,
    pub user_filter: String,
}

impl LdapProvider {
    pub async fn new(config: LdapConfig) -> ContinuumResult<Self> {
        let (conn, mut ldap) = ldap3::LdapConnAsync::new(&config.url).await?;
        ldap.simple_bind(&config.bind_dn, &config.bind_password).await?;
        Ok(Self { pool: conn, config })
    }
    
    pub async fn authenticate(&self, username: &str, password: &str) -> ContinuumResult<bool> {
        // Search for user DN
        // Attempt bind with user credentials
        // Return success/failure
    }
    
    pub async fn get_groups(&self, username: &str) -> ContinuumResult<Vec<String>> {
        // Query user's group memberships
    }
}
```

**Audit Logging:**

```rust
pub struct AuditLogger {
    sink: Box<dyn AuditSink>,
}

pub trait AuditSink: Send + Sync {
    fn log(&self, event: &AuditEvent) -> ContinuumResult<()>;
}

pub struct FileAuditSink {
    path: PathBuf,
    file: Mutex<File>,
}

pub struct JsonAuditSink {
    path: PathBuf,
}

impl AuditSink for JsonAuditSink {
    fn log(&self, event: &AuditEvent) -> ContinuumResult<()> {
        let json = serde_json::to_string(event)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", json)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    pub actor: String,
    pub target: String,
    pub details: serde_json::Value,
    pub hash: String,
    pub previous_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditEventType {
    ConnectionRequested,
    ConnectionAccepted,
    ConnectionRejected,
    ConnectionClosed,
    PairingCompleted,
    PairingFailed,
    FileTransfer,
    InputEvent,
    SettingsChanged,
    UserAuthenticated,
    UserAccessDenied,
}
```

---

## Workstream 7: UI/UX Redesign

### 7.1 Problem
Current UI is functional but basic. Missing address book, connection history, settings, and modern design.

### 7.2 Solution: Redesigned egui Interface with New Components

**New Components:**

```rust
/// Address book for saved connections
pub struct AddressBook {
    entries: Vec<AddressBookEntry>,
    selected: Option<usize>,
}

pub struct AddressBookEntry {
    pub machine_id: String,
    pub display_name: String,
    pub last_connected: DateTime<Utc>,
    pub last_address: String,
    pub status: ConnectionStatus,
    pub avatar: Option<egui::TextureId>,
}

/// Connection history
pub struct ConnectionHistory {
    entries: Vec<HistoryEntry>,
}

pub struct HistoryEntry {
    pub timestamp: DateTime<Utc>,
    pub peer_name: String,
    pub peer_id: String,
    pub duration: Duration,
    pub bytes_transferred: u64,
    pub direction: ConnectionDirection,
}

/// Settings panel
pub struct SettingsPanel {
    pub general: GeneralSettings,
    pub network: NetworkSettings,
    pub video: VideoSettings,
    pub security: SecuritySettings,
    pub enterprise: EnterpriseSettings,
}

/// Quick connect bar
pub struct QuickConnectBar {
    input: String,
    recent_connections: Vec<String>,
}
```

**Redesigned Main Screen:**

```rust
impl eframe::App for ContinuumApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Top bar with status and quick actions
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            self.render_top_bar(ui);
        });
        
        // Left sidebar with address book
        egui::SidePanel::left("sidebar").show(ctx, |ui| {
            self.render_sidebar(ui);
        });
        
        // Central content area
        egui::CentralPanel::default().show(ctx, |ui| {
            match &self.state {
                AppState::Home => self.render_home(ui),
                AppState::Connecting { worker, .. } => self.render_connecting(ui),
                AppState::Connected { worker, frame_count } => self.render_streaming(ui),
                AppState::Disconnected { message } => self.render_disconnected(ui),
            }
        });
        
        // Bottom status bar
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            self.render_status_bar(ui);
        });
    }
}
```

---

## Workstream 8: Plugin System

### 8.1 Problem
No extensibility. Can't add custom features without modifying core.

### 8.2 Solution: WASM-based Plugin Runtime

**New Crate:** `continuum-plugins`

```toml
# src/continuum-plugins/Cargo.toml
[package]
name = "continuum-plugins"
version.workspace = true
edition.workspace = true

[dependencies]
continuum-core = { path = "../continuum-core" }
wasmtime = "25"
serde = { workspace = true }
```

**Plugin Trait:**

```rust
/// Plugin interface
pub trait Plugin: Send + Sync {
    /// Get plugin metadata
    fn metadata(&self) -> PluginMetadata;
    
    /// Initialize the plugin
    fn init(&mut self, ctx: &mut PluginContext) -> ContinuumResult<()>;
    
    /// Handle a frame event
    fn on_frame(&mut self, frame: &mut VideoFrame) -> ContinuumResult<()>;
    
    /// Handle a session event
    fn on_session_event(&mut self, event: &SessionEvent) -> ContinuumResult<()>;
    
    /// Handle an input event
    fn on_input(&mut self, event: &InputEvent) -> ContinuumResult<()>;
    
    /// Shutdown the plugin
    fn shutdown(&mut self) -> ContinuumResult<()>;
}

pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub capabilities: Vec<PluginCapability>,
}

pub enum PluginCapability {
    FrameProcessing,
    InputProcessing,
    SessionManagement,
    UiExtension,
    NetworkExtension,
}
```

**WASM Plugin Loader:**

```rust
pub struct WasmPluginLoader {
    engine: wasmtime::Engine,
    store: wasmtime::Store<PluginState>,
}

impl WasmPluginLoader {
    pub fn load(&self, wasm_bytes: &[u8]) -> ContinuumResult<Box<dyn Plugin>> {
        let module = wasmtime::Module::new(&self.engine, wasm_bytes)?;
        let instance = wasmtime::Instance::new(&mut self.store, &module, &[])?;
        
        // Extract plugin interface from WASM exports
        let metadata = instance.get_typed_func::<(), PluginMetadata>(&mut self.store, "plugin_metadata")?;
        let process_frame = instance.get_typed_func::<*mut u8, i32>(&mut self.store, "process_frame")?;
        
        Ok(Box::new(WasmPlugin {
            metadata: metadata.call(&mut self.store, ())?,
            process_frame,
        }))
    }
}
```

---

## Workstream 9: Session Management

### 9.1 Problem
No session persistence. Can't resume sessions or manage multiple viewers.

### 9.2 Solution: Session Store with Multi-Viewer Support

**New Crate:** `continuum-session`

```toml
# src/continuum-session/Cargo.toml
[package]
name = "continuum-session"
version.workspace = true
edition.workspace = true

[dependencies]
continuum-core = { path = "../continuum-core" }
tokio = { workspace = true }
serde = { workspace = true }
uuid = { version = "1", features = ["v4"] }
```

**Core Types:**

```rust
/// A session between two or more peers
pub struct Session {
    pub id: String,
    pub host: SessionPeer,
    pub viewers: Vec<SessionPeer>,
    pub state: SessionState,
    pub created_at: DateTime<Utc>,
    pub pairing_code: String,
    pub capabilities: SessionCapabilities,
}

pub struct SessionPeer {
    pub machine_id: String,
    pub display_name: String,
    pub connection: ConnectionId,
    pub permissions: Permissions,
    pub joined_at: DateTime<Utc>,
}

pub enum SessionState {
    WaitingForPeer,
    Active,
    Paused,
    Closed,
}

pub struct SessionCapabilities {
    pub max_viewers: usize,
    pub allow_input: bool,
    pub allow_clipboard: bool,
    pub allow_file_transfer: bool,
    pub allow_audio: bool,
}

/// Session store for managing active sessions
pub struct SessionStore {
    sessions: RwLock<HashMap<String, Session>>,
    by_pairing_code: RwLock<HashMap<String, String>>,
    by_machine: RwLock<HashMap<String, Vec<String>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            by_pairing_code: RwLock::new(HashMap::new()),
            by_machine: RwLock::new(HashMap::new()),
        }
    }
    
    /// Create a new session
    pub async fn create(&self, host: SessionPeer, pairing_code: String) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let session = Session {
            id: id.clone(),
            host,
            viewers: Vec::new(),
            state: SessionState::WaitingForPeer,
            created_at: Utc::now(),
            pairing_code: pairing_code.clone(),
            capabilities: SessionCapabilities::default(),
        };
        
        self.sessions.write().await.insert(id.clone(), session);
        self.by_pairing_code.write().await.insert(pairing_code, id.clone());
        id
    }
    
    /// Join an existing session
    pub async fn join(&self, session_id: &str, viewer: SessionPeer) -> ContinuumResult<()> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(session_id)
            .ok_or(crate::ContinuumError::NotFound("Session not found".into()))?;
        
        if session.viewers.len() >= session.capabilities.max_viewers {
            return Err(crate::ContinuumError::NotSupported("Session is full".into()));
        }
        
        session.viewers.push(viewer);
        Ok(())
    }
    
    /// Get session by pairing code
    pub async fn find_by_code(&self, code: &str) -> Option<Session> {
        let by_code = self.by_pairing_code.read().await;
        if let Some(id) = by_code.get(code) {
            self.sessions.read().await.get(id).cloned()
        } else {
            None
        }
    }
    
    /// Close a session
    pub async fn close(&self, session_id: &str) {
        if let Some(session) = self.sessions.write().await.remove(session_id) {
            self.by_pairing_code.write().await.remove(&session.pairing_code);
        }
    }
}
```

---

## Workstream 10: Security Hardening

### 10.1 Problem
Current security is good but needs hardening for production use.

### 10.2 Solution: Defense in Depth

**Security Layers:**

1. **Transport Security:** QUIC with TLS 1.3
2. **Session Security:** SPAKE2+ for pairing
3. **E2E Security:** Double-ratchet encryption
4. **Identity Security:** Ed25519 signing keys
5. **Application Security:** Sandboxed plugins
6. **Audit Security:** Tamper-evident logs

**Post-Quantum Readiness:**

```rust
pub enum KeyExchangeAlgorithm {
    X25519,
    Kyber512,
    Kyber768,
    Kyber1024,
    X25519Kyber768,  // Hybrid
}

pub struct CryptoProvider {
    algorithm: KeyExchangeAlgorithm,
    // ...
}

impl CryptoProvider {
    /// Negotiate algorithm with peer
    pub fn negotiate(&self, peer_algorithms: &[KeyExchangeAlgorithm]) -> KeyExchangeAlgorithm {
        // Prefer hybrid, then strongest shared
        let preference = [
            KeyExchangeAlgorithm::X25519Kyber768,
            KeyExchangeAlgorithm::X25519,
            KeyExchangeAlgorithm::Kyber768,
        ];
        
        for alg in &preference {
            if peer_algorithms.contains(alg) {
                return alg.clone();
            }
        }
        
        // Fall back to X25519
        KeyExchangeAlgorithm::X25519
    }
}
```

---

## Workstream 11: Performance Optimization

### 11.1 Problem
Need to match or exceed competitor performance.

### 11.2 Solution: Zero-Copy Pipeline + Adaptive Quality

**Zero-Copy Capture → Encode Pipeline:**

```rust
pub struct GpuPipeline {
    // Windows: DXGI → NVENC (zero-copy)
    // macOS: CGDisplay → VideoToolbox (zero-copy)
    // Linux: PipeWire → VAAPI (zero-copy)
}

impl GpuPipeline {
    pub fn capture_and_encode(&mut self) -> ContinuumResult<EncodedFrame> {
        // 1. Capture directly to GPU texture (no CPU readback)
        // 2. Encode from GPU texture (no GPU→CPU transfer)
        // 3. Return encoded bitstream
    }
}
```

**Adaptive Quality Controller:**

```rust
pub struct AdaptiveQualityController {
    target_latency_ms: f32,
    current_quality: u8,
    current_fps: u32,
    bandwidth_estimator: BandwidthEstimator,
}

impl AdaptiveQualityController {
    pub fn update(&mut self, stats: ConnectionStats) -> QualityDecision {
        // Adjust quality based on:
        // - Network RTT
        // - Packet loss
        // - Encode time
        // - Content type
        
        if stats.rtt_ms > self.target_latency_ms * 1.5 {
            // Reduce quality
            self.current_quality = (self.current_quality - 5).max(30);
        } else if stats.rtt_ms < self.target_latency_ms * 0.5 {
            // Increase quality
            self.current_quality = (self.current_quality + 5).min(95);
        }
        
        QualityDecision {
            quality: self.current_quality,
            fps: self.current_fps,
        }
    }
}
```

---

## Workstream 12: Testing & Quality Assurance

### 12.1 Problem
Need comprehensive testing for production reliability.

### 12.2 Solution: Multi-Layer Testing Strategy

**Test Layers:**

1. **Unit Tests:** Per-module tests (existing)
2. **Integration Tests:** Cross-module tests
3. **E2E Tests:** Full client-server tests
4. **Compatibility Tests:** Old client vs new server
5. **Performance Tests:** Latency, bandwidth, FPS
6. **Chaos Tests:** Network drops, CPU exhaustion
7. **Fuzz Tests:** Protocol parsers, file formats
8. **Security Tests:** Penetration testing

**Compatibility Test Framework:**

```rust
/// Test that version N client can connect to version N+1 server
#[tokio::test]
async fn test_backward_compatibility() {
    let old_client = Client::new("1.0.0");
    let new_server = Server::new("1.1.0");
    
    let result = old_client.connect(&new_server).await;
    assert!(result.is_ok());
}
```

**Chaos Testing:**

```rust
pub struct ChaosEngine {
    network_conditions: NetworkConditions,
    cpu_load: f32,
    memory_pressure: f32,
}

impl ChaosEngine {
    pub async fn run_with_chaos<F, T>(&self, f: F) -> T
    where
        F: FnOnce() -> T,
    {
        // Apply network latency
        // Apply packet loss
        // Apply CPU load
        // Apply memory pressure
        // Run test function
        f()
    }
}
```

---

## Implementation Timeline

### Phase 1: Foundation (Weeks 1-4)
- [ ] ICE implementation (STUN/TURN)
- [ ] mDNS discovery
- [ ] H.264 codec with hardware acceleration
- [ ] Address book UI

### Phase 2: Reach (Weeks 5-8)
- [ ] Web client (WebRTC)
- [ ] Mobile client shells (iOS/Android)
- [ ] Unattended access mode
- [ ] Session recording improvements

### Phase 3: Enterprise (Weeks 9-12)
- [ ] SSO/OIDC integration
- [ ] LDAP/AD support
- [ ] Audit logging
- [ ] Policy management

### Phase 4: Polish (Weeks 13-16)
- [ ] UI/UX redesign
- [ ] Plugin system
- [ ] Performance optimization
- [ ] Comprehensive testing

### Phase 5: Launch (Weeks 17-20)
- [ ] Security audit
- [ ] Documentation
- [ ] Community building
- [ ] v1.0 release

---

## Summary

This plan transforms Continuum from a functional prototype into a production-grade, next-generation remote desktop platform. Each workstream is designed to be implemented independently, with clear interfaces between components.

**Key Principles:**
1. **Modularity:** Every component is swappable
2. **Durability:** Designed to evolve for 100 years
3. **Privacy:** Self-hosted by design, no vendor lock-in
4. **Performance:** Gaming-grade latency, efficient codecs
5. **Security:** Post-quantum ready, fully auditable

The result will be a remote desktop platform that rivals or exceeds all competitors while remaining fully open source and privacy-preserving.
