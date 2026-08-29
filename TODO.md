# Continuum TODO

## Completed

- [x] Error handling: replace all unwrap/expect with proper error propagation
- [x] Configuration: CLI args (clap), config file (toml), env vars
- [x] Structured logging: tracing + tracing-subscriber
- [x] Security: gate media/input behind pairing
- [x] Security: persist TLS certificate across restarts
- [x] Security: TOFU certificate pinning
- [x] Security: rate-limit pairing attempts
- [x] Reconnection: exponential backoff with jitter
- [x] Heartbeat: connection health monitoring
- [x] Adaptive bitrate: dynamic JPEG quality based on encode time
- [x] Frame diffing: skip unchanged frames
- [x] Keyframe interval: periodic keyframes + change detection
- [x] Multi-monitor: enumerate and select displays
- [x] Full keyboard: modifiers, function keys, arrow keys, combos
- [x] Mouse scroll: horizontal and vertical scroll support
- [x] Mouse drag: track mouse-down state
- [x] Client architecture: modular app/connection/rendering/input/settings/widgets
- [x] Modern UI: dark theme, toolbar, side panel, status bar
- [x] CI/CD: GitHub Actions for build, test, clippy, fmt
- [x] Testing: unit tests for types, codec, security, AI

## Next Steps

### P0 — Critical
- [ ] Verify client GUI opens and streams in real interactive session
- [ ] End-to-end integration test with real QUIC connection
- [ ] Performance benchmarking: capture-to-display latency

### P1 — High Priority
- [ ] Clipboard sync: bidirectional text+image clipboard transfer
- [ ] Audio streaming: capture, Opus encode/decode, playback
- [ ] File transfer: drag-drop, chunked transfer, progress, resume
- [ ] Wire up continuum-security DID handshake for auth
- [ ] Session tokens for reconnection without re-pairing

### P2 — Medium Priority
- [ ] NAT traversal: real QUIC relay with registration
- [ ] Multi-client support: broadcast frames, per-client state
- [ ] Cross-platform capture: macOS (core-graphics), Linux (xdotool)
- [ ] H.264/AV1 hardware encoding (NVENC, QuickSync, VAAPI)
- [ ] Host as service: Windows Service, systemd, launchd
- [ ] Session recording and playback

### P3 — Nice to Have
- [ ] Super-resolution via ONNX Runtime (Real-ESRGAN)
- [ ] Semantic segmentation for smart compression
- [ ] QR code pairing
- [ ] Auto-update mechanism
- [ ] Packaging: MSI, DMG, DEB, Flatpak
- [ ] TOTP-based pairing
- [ ] Multi-window/multi-session support
