# Continuum

Next-generation remote desktop platform. Built with QUIC transport and Rust.
**No Docker or Kubernetes required** — runs as native binaries on Windows, macOS, and Linux.

## Quick Start

### Prerequisites

- Rust 1.75+ (stable)
- Windows (primary: real screen capture + input injection)
- macOS/Linux (demo mode with synthetic frames)

### Build and Run

```bash
# Build everything
cargo build --release

# Terminal 1 — Start the server
cargo run --release --bin continuum-server

# Terminal 2 — Start the client
cargo run --release --bin continuum-client
```

### Or use the unified binary (server + GUI in one)

```bash
cargo run --release --bin continuum
# Server starts automatically, GUI opens with your ID/Password
```

### Connect

The client connects to `127.0.0.1:4433` by default. Enter the 6-character ID and password shown on the server's home screen and click **Connect**.

## Features

- **QUIC Transport**: Low-latency, encrypted streaming over QUIC/TLS 1.3
- **E2E Encryption**: Real Diffie-Hellman key exchange + AES-256-GCM double-ratchet on all frames
- **Real-time Screen Capture**: Windows screen capture with adaptive quality
- **Input Injection**: Full keyboard, mouse, scroll, and modifier key support
- **Multi-Monitor**: Select and stream any connected display
- **Adaptive Bitrate**: Dynamic quality and frame rate based on encode time
- **Session Recording**: Record and replay remote sessions (.csr format)
- **Frame-Level Audit**: JSON-Lines audit log with tamper-evident hash chain
- **Plugin System**: Native compiled plugins with lifecycle hooks
- **GPU Encoding**: NVENC/VAAPI hardware acceleration (with `--features nvenc`)
- **AI Super-Resolution**: Smart upscaling and region classification
- **Local CI/CD**: Built-in `continuum-ci` with web dashboard

## CLI Flags

### Server

```bash
continuum-server \
  --listen 0.0.0.0:4433 \
  --pairing-code my-secret \
  --record-dir ./recordings \
  --audit-frames audit.jsonl \
  --insecure  # Disable E2E encryption (testing only)
```

### Client

```bash
continuum-client \
  --connect 127.0.0.1:4433 \
  --pairing-code my-secret \
  --name "My Workstation"
```

### Unified Binary

```bash
# Server + GUI mode (default)
continuum

# Client only (for testing two instances on one machine)
continuum --client-only

# With recording + audit
continuum --record-dir ./recordings --audit-frames audit.jsonl
```

## Configuration

Set via CLI flags, config file, or environment variables:

```bash
# Environment variables
export CONTINUUM_PAIRING_CODE=my-secret
export CONTINUUM_LISTEN_ADDR=0.0.0.0:4433
export CONTINUUM_QUALITY=90

# Config file (~/.config/continuum/server.toml)
```

## Architecture

```
Client ←——QUIC/TLS——→ Server
         (APQ-2 ALPN)

Frame pipeline:
  Capture → Encode (JPEG) → E2E Encrypt → QUIC Send
  QUIC Recv → E2E Decrypt → Decode (JPEG) → Render

E2E Key Exchange:
  Client generates X25519 keypair
  → Sends public key in PairingHandshake
  → Server generates X25519 keypair
  → Server computes shared secret, returns its public key
  → Client computes same shared secret
  → Both sides encrypt/decrypt with AES-256-GCM via double-ratchet
```

## Session Recording

```bash
# Start server with recording enabled
cargo run --release --bin continuum-server -- --record-dir ./recordings

# Replay a recording
cargo run --release -p continuum-test -- replay -f recordings/session-*.csr
```

Recording format (.csr): Binary format with frame index, timestamps, and JPEG data. Each frame entry contains:
- 4-byte header length + JSON metadata (timestamp, semantics, dimensions)
- 4-byte data length + raw JPEG bytes

## Audit Logging

```bash
# Start server with frame-level audit
cargo run --release --bin continuum-server -- --audit-frames audit.jsonl

# Output: one JSON line per event with hash chain verification
{"timestamp":"...","event_type":"frame_sent","client_addr":"...","frame_number":42,"size_bytes":14230,"quality":85,"encode_time_us":3200,"encrypted":true,"hash":"...","previous_hash":"..."}
```

## Project Structure

```
src/
├── continuum-transport/     Core: QUIC, capture, codec, E2E, streaming, plugins
├── continuum-server/        Server binary (thin wrapper)
├── continuum-client/        GUI client (egui)
├── continuum/               Unified binary (server + client GUI)
├── continuum-security/      DID handshake, double-ratchet, X25519
├── continuum-ai/            Super-resolution, segmentation, intent
├── continuum-observability/ Metrics, health, audit, OIDC
├── continuum-plugin-sdk/    Plugin manifest and hook types
├── continuum-test/          Testing suite, benchmarks, replay tool
├── continuum-ci/            Local CI/CD pipeline
└── relay-server/            QUIC relay for NAT traversal
```

## Testing

```bash
# Run all tests (99+ tests)
cargo test --workspace

# Run benchmarks
cargo run --release -p continuum-test -- bench

# Run two-instance connection test
cargo run --release -p continuum-test -- two-test

# Run smoke tests
cargo run --release -p continuum-test -- smoke
```

## Debugging

Continuum includes a CDP-compatible debug API for automated testing:

```bash
# Enable debug API
continuum --debug 9094

# Query state
curl http://127.0.0.1:9094/json
```

See [DEBUGGING.md](DEBUGGING.md) for the full API reference.

## License

MIT
