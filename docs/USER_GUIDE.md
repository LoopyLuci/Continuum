# Continuum User Guide

## Installation

### Requirements
- Rust 1.75+ (stable)
- Windows (full capture/input), macOS or Linux (capture via `scrap`, input via `libxdo`/`core-graphics`)

### Build

```bash
cargo build --release --workspace --exclude continuum-ai --exclude continuum-plugin-sdk
```

### Feature Flags

| Flag | Purpose |
|------|---------|
| `audio` | Audio capture/playback (requires `cpal`) |
| `wasm-plugins` | WASM plugin support (requires `wasmtime`) |
| `nvenc` | NVIDIA hardware encoding |
| `vaapi` | VAAPI hardware encoding |
| `platform-macos` | macOS capture via `core-graphics` + `scrap` |
| `platform-linux` | Linux capture via `xdo` + `scrap` |

## Basic Usage

### Starting the Server

The server waits for incoming client connections and streams the desktop.

```bash
# Basic
cargo run --release --bin continuum-server -- --listen 0.0.0.0:4433

# With pairing code
cargo run --release --bin continuum-server -- --listen 0.0.0.0:4433 --pairing-code my-secret

# With recording
cargo run --release --bin continuum-server -- --listen 0.0.0.0:4433 --record-dir ./recordings

# With audit logging
cargo run --release --bin continuum-server -- --listen 0.0.0.0:4433 --audit-frames audit.jsonl

# Require PAKE pairing only
cargo run --release --bin continuum-server -- --listen 0.0.0.0:4433 --require-pake

# Insecure mode (testing only - no E2E encryption)
cargo run --release --bin continuum-server -- --listen 0.0.0.0:4433 --insecure
```

### Starting the Client

```bash
# Basic
cargo run --release --bin continuum-client -- --connect 127.0.0.1:4433

# With pairing code
cargo run --release --bin continuum-client -- --connect 127.0.0.1:4433 --pairing-code my-secret

# Headless mode (service/background)
cargo run --release --bin continuum-client -- --connect 127.0.0.1:4433 --headless

# Enable debug API
cargo run --release --bin continuum-client -- --connect 127.0.0.1:4433 --debug 9090
```

### Unified Binary

The `continuum` binary combines server + GUI client:

```bash
cargo run --release --bin continuum
```

## Pairing

Continuum uses a 6-character pairing code + SAS (Short Authentication String) verification.

1. Server displays ID (machine ID) and password (pairing code)
2. Client enters ID and password
3. Both sides display SAS words during pairing
4. Verify words match on both screens, then confirm

If SAS words don't match, disconnect immediately — the connection may be compromised.

## Relay Server

For NAT traversal without port forwarding:

```bash
# Start relay server (public IP)
cargo run --release --bin relay-server -- --listen 0.0.0.0:4434

# Both clients connect to relay with same session ID
cargo run --release --bin continuum-client -- --connect RELAY_IP:4434 --session-id abc123
```

## Session Recording

### Start Recording
```bash
cargo run --release --bin continuum-server -- --record-dir ./recordings
```

### Replay Recording
```bash
cargo run --release -p continuum-test -- replay -f recordings/session-*.csr
```

### Recording Format (.csr)
Binary format with frame index, timestamps, and JPEG data. Each entry:
- 4-byte header length + JSON metadata
- 4-byte data length + raw JPEG bytes

## Audit Logging

```bash
cargo run --release --bin continuum-server -- --audit-frames audit.jsonl
```

Output: one JSON line per event with hash chain verification:
```json
{"timestamp":"...","event_type":"frame_sent","client_addr":"...","frame_number":42,"size_bytes":14230,"quality":85,"encode_time_us":3200,"encrypted":true,"hash":"...","previous_hash":"..."}
```

## Configuration

Configuration is layered (later overrides earlier):
1. Defaults
2. Config file (`~/.config/continuum/server.toml`)
3. Environment variables (`CONTINUUM_PAIRING_CODE`, `CONTINUUM_LISTEN_ADDR`, etc.)
4. CLI flags

## Troubleshooting

See [TROUBLESHOOTING.md](TROUBLESHOOTING.md)
