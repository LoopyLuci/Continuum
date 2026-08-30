# Continuum v0.3.0

A peer-to-peer remote desktop platform built with QUIC + Rust.
Encrypted, modular, and designed to evolve for decades.

## Quick Start

```bash
# Build
cargo build --release

# Terminal 1: Start the server
cargo run --release --bin continuum-server -- --listen 0.0.0.0:4433 --pairing-code my-secret

# Terminal 2: Start the client
cargo run --release --bin continuum-client -- --connect 127.0.0.1:4433 --pairing-code my-secret
```

Or use the unified binary (server + GUI in one):
```bash
cargo run --release --bin continuum
```

## Documentation

- **[User Guide](docs/USER_GUIDE.md)** - Installation, usage, troubleshooting
- **[Developer Guide](docs/DEVELOPER_GUIDE.md)** - Architecture, extending, contributing
- **[API Reference](docs/API.md)** - Crate-by-crate API documentation
- **[Security Model](docs/SECURITY.md)** - Encryption, pairing, threats

## Project Structure

```
src/
├── continuum-core/          Core traits (VideoEncoder, Transport, Crypto, etc.)
├── continuum-transport/     QUIC transport, capture, codec, streaming
├── continuum-server/        Server binary
├── continuum-client/        GUI client (egui)
├── continuum/               Unified binary (server + client)
├── continuum-security/      DID handshake, double-ratchet, X25519
├── continuum-ai/            Super-resolution, segmentation, intent
├── continuum-observability/ Metrics, health, audit, OIDC
├── continuum-plugin-sdk/    Plugin manifest and hooks
├── continuum-test/          Testing, benchmarks, replay
├── continuum-ci/            Local CI/CD pipeline
└── relay-server/            QUIC relay for NAT traversal
```

## Testing

```bash
cargo test --workspace
```

## License

MIT
