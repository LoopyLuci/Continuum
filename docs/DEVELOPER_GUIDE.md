# Continuum Developer Guide

## Architecture Overview

Continuum is organized as a Rust workspace with clear separation of concerns:

```
continuum-core/          ← Swappable traits (no dependencies)
    ↓
continuum-security/      ← Crypto implementations
continuum-transport/     ← QUIC, capture, codec (depends on core + security)
    ↓
continuum-server/        ← Server binary (depends on transport)
continuum-client/        ← Client binary (depends on transport)
continuum/               ← Unified binary (server + client)
continuum-observability/ ← Metrics, audit, health
continuum-ai/            ← ML-based enhancements
continuum-ci/            ← CI/CD pipeline
continuum-test/          ← Testing tools
continuum-plugin-sdk/    ← Plugin types
relay-server/            ← NAT traversal relay
```

## Core Traits (continuum-core)

Every component is behind a swappable interface defined in `continuum-core`:

### VideoEncoder
```rust
pub trait VideoEncoder: Send + Sync {
    fn encode(&mut self, frame: &VideoFrame, is_keyframe: bool, quality: u8) -> ContinuumResult<EncodedFrame>;
    fn notify_network(&mut self, rtt_ms: f32, packet_loss: f32);
    fn bitrate_estimate_kbps(&self) -> u32;
    fn request_keyframe(&mut self);
    fn reset(&mut self);
}
```

### Transport
```rust
pub trait Transport: Send + Sync {
    fn connect(&mut self, addr: &str) -> ContinuumResult<Box<dyn TransportConnection>>;
    fn accept(&mut self) -> ContinuumResult<Box<dyn TransportConnection>>;
    fn close(&mut self);
}
```

### CryptoProvider
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

### CaptureBackend
```rust
pub trait CaptureBackend: Send + Sync {
    fn enumerate_monitors(&self) -> ContinuumResult<Vec<MonitorInfo>>;
    fn capture_frame(&mut self, monitor_id: u32) -> ContinuumResult<CapturedFrame>;
    fn name(&self) -> &str;
    fn is_available(&self) -> bool;
}
```

### AudioProcessor
```rust
pub trait AudioProcessor: Send + Sync {
    fn capture(&mut self) -> ContinuumResult<AudioFrame>;
    fn play(&mut self, frame: &AudioFrame);
    fn is_active(&self) -> bool;
    fn name(&self) -> &str;
}
```

### InputInjector
```rust
pub trait InputInjector: Send + Sync {
    fn apply(&mut self, event: &RemoteInputEvent) -> ContinuumResult<()>;
    fn name(&self) -> &str;
    fn is_available(&self) -> bool;
}
```

### ProtocolNegotiator
```rust
pub trait ProtocolNegotiator: Send + Sync {
    fn negotiate(&mut self, peer_versions: &[ProtocolVersion]) -> ContinuumResult<ProtocolVersion>;
    fn supported_versions(&self) -> &[ProtocolVersion];
    fn supports_version(&self, version: &ProtocolVersion) -> bool;
}
```

## Transport Layer (continuum-transport)

### QUIC Implementation
- Uses `quinn` crate with TLS 1.3
- ALPN protocol: `continuum-relay-1`
- Bidirectional streams for media, intent, audio, debug, clipboard, file transfer
- Automatic reconnection with exponential backoff

### Stream Types (ApqStreamType)
| Type | ID | Purpose |
|------|----|---------|
| Media | 1 | Video frames |
| Intent | 2 | Input, clipboard, file transfer |
| Audio | 5 | Audio streams |
| Debug | 6 | CDP-compatible debug tunnel |
| Clipboard | 7 | Clipboard sync |
| FileTransfer | 4 | File transfer |

### Frame Pipeline
```
Capture → Encode (JPEG) → E2E Encrypt → QUIC Send
QUIC Recv → E2E Decrypt → Decode (JPEG) → Render
```

### Adaptive Rate Control
- Monitors encode time, frame size, RTT, packet loss
- Adjusts quality (30-95) and FPS (10-60) dynamically
- Skip frame decision when bandwidth is limited

## Security Layer (continuum-security)

### E2E Encryption
- X25519 Diffie-Hellman key exchange
- AES-256-GCM via double-ratchet
- PAKE (Password-Authenticated Key Exchange) option
- SAS (Short Authentication String) verification

### Pairing Flow
1. Client generates X25519 keypair
2. Client sends public key in PairingHandshake
3. Server generates X25519 keypair
4. Server computes shared secret, returns public key
5. Client computes same shared secret
6. Both sides encrypt/decrypt with AES-256-GCM

### DID Handshake
- Decentralized identity verification
- DidHandshake struct for identity verification
- Supports multiple DID methods

## Server (continuum-server)

### Main Loop
1. Listen for QUIC connections
2. Accept bidirectional streams
3. Route streams by type (Media, Intent, Audio, etc.)
4. Handle each stream concurrently

### Session Management
- Session tokens with 1-hour expiry
- Resume capability via resume tokens
- Per-client permissions (view, control, clipboard, file transfer)

## Client (continuum-client)

### UI Framework
- egui-based immediate mode GUI
- Dark theme with customizable visuals
- Wizard-based setup flow

### Connection Flow
1. Resolve ID to address (direct IP, hostname, or relay)
2. Establish QUIC connection
3. Pair with server (SAS verification)
4. Open media stream
5. Render frames + capture input

### Features
- Multi-monitor selection
- Quality settings
- Clipboard sync
- File transfer
- Session recording

## Relay Server (relay-server)

### NAT Traversal
1. Host registers with relay (session ID + secret)
2. Client connects to relay with same session ID
3. Relay forwards bidirectional streams between host and client
4. Session expires after 1 hour of inactivity

## AI Layer (continuum-ai)

### Features
- **Super-Resolution**: Upscale low-res content (ONNX-based)
- **Segmentation**: Detect semantic regions for quality allocation
- **Anomaly Detection**: Detect encoding anomalies
- **Network Prediction**: Predict optimal bitrate
- **Intent Agent**: Parse and predict user intents

### ONNX Engine
- Load ONNX models for inference
- Classify regions (text, image, background)
- Determine quality tier per region

## Observability (continuum-observability)

### Metrics
- Encode time, frame size, bitrate
- RTT, packet loss, bandwidth estimate
- Per-client and per-session metrics

### Audit Logging
- JSON-Lines format
- Hash chain for tamper evidence
- Events: connection, pairing, input, frame, session

### Health Server
- HTTP endpoint for health checks
- Prometheus-compatible metrics

## CI/CD (continuum-ci)

### Pipeline
1. Watch for file changes
2. Run stages (build, test, lint, deploy)
3. Report results to dashboard

### Dashboard
- Web UI at `http://127.0.0.1:8787`
- Build history, stage output, metrics

## Testing (continuum-test)

### Test Types
- **Unit tests**: Per-crate tests
- **Integration tests**: Cross-crate tests
- **E2E tests**: Full client-server tests
- **Chaos tests**: Disconnect/reconnect/frame stress
- **Benchmarks**: Performance benchmarks

### Running Tests
```bash
# All tests
cargo test --workspace

# Specific crate
cargo test -p continuum-transport

# E2E tests
cargo run --release -p continuum-test -- e2e

# Benchmarks
cargo run --release -p continuum-test -- bench

# Chaos tests
cargo run --release -p continuum-test -- chaos
```

## Plugin System (continuum-plugin-sdk)

### Plugin Types
- Native plugins (compiled into binary)
- WASM plugins (sandboxed via wasmtime)

### Plugin Hooks
- `on_frame_encoded`: Modify frame before send
- `on_session_start`: Session initialization
- `on_session_end`: Session cleanup

## Building and Extending

### Adding a New Video Encoder
1. Implement `VideoEncoder` trait
2. Register in encoder factory
3. Add feature flag if optional

### Adding a New Transport
1. Implement `Transport` + `TransportConnection` + `BiStream`
2. Register in transport factory
3. Add feature flag if optional

### Adding a New Crypto Provider
1. Implement `CryptoProvider` trait
2. Register in crypto registry
3. Add algorithm ID for negotiation

## Code Style
- `RUSTFLAGS="-D warnings"` enforced in CI
- `cargo fmt` for formatting
- `cargo clippy` for lints
- Edition 2021
