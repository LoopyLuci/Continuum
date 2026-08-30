## What's New

### Core Architecture
- **continuum-core crate**: 7 swappable trait families (VideoEncoder, Transport, CryptoProvider, CaptureBackend, AudioProcessor, InputInjector, ProtocolNegotiator)
- Foundation for 100-year durability: every component behind a swappable interface

### Security
- **E2E encryption**: X25519 Diffie-Hellman + AES-256-GCM double-ratchet
- **PAKE pairing**: Password-Authenticated Key Exchange option
- **SAS verification**: Short Authentication String for MITM detection
- **Audit logging**: JSON-Lines with hash chain for tamper evidence
- **Resume tokens**: Session resumption with 1-hour expiry

### Networking
- **QUIC transport**: Low-latency, encrypted streaming with TLS 1.3
- **Relay server**: NAT traversal without port forwarding
- **Adaptive bitrate**: Dynamic quality/FPS based on network conditions
- **Multi-monitor**: Select and stream any connected display

### Features
- **Session recording**: Record and replay (.csr format)
- **Clipboard sync**: Bidirectional clipboard synchronization
- **File transfer**: Send files between machines
- **AI enhancements**: Super-resolution, segmentation, anomaly detection
- **CI/CD**: Built-in local pipeline with web dashboard

### Testing
- 240+ tests across all crates
- E2E, chaos, smoke, and benchmark tests
- CI with check, test, clippy, fmt, doctest, audit, performance jobs

### Documentation
- User Guide: installation, usage, troubleshooting
- Developer Guide: architecture, extending, contributing
- API Reference: all public types and functions

## Binaries
- `continuum-server`: Server only
- `continuum-client`: GUI client
- `continuum`: Unified binary (server + client)
- `relay-server`: NAT traversal relay
- `continuum-test`: Testing tools

## Quick Start
```bash
cargo build --release
cargo run --release --bin continuum-server -- --listen 0.0.0.0:4433 --pairing-code my-secret
cargo run --release --bin continuum-client -- --connect 127.0.0.1:4433 --pairing-code my-secret
```

Full documentation: https://github.com/LoopyLuci/Continuum/tree/main/docs
