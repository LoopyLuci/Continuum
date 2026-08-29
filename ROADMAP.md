# Continuum Roadmap

## v1.0.0 (Current)
- QUIC transport with TLS 1.3
- E2E encryption (PAKE + X25519 + AES-256-GCM + DH ratchet)
- Real-time screen capture (Windows, synthetic fallback on Linux/macOS)
- Input injection (keyboard, mouse, scroll, modifiers)
- Multi-monitor selection
- Session recording (.csr format) with replay tool
- Clipboard sync (bidirectional text)
- Audio forwarding (cpal-based capture/playback)
- File transfer (chunked upload/download)
- Frame-level audit logging (JSONL with hash chain)
- Connection quality metrics (latency, packet loss, bandwidth)
- Native plugin system
- Adaptive bitrate control
- LAN discovery
- Trusted identity persistence
- Session resumption with token-based reconnection

## v1.1.0 (Planned)
- AI-powered region classification and super-resolution
- WASM plugin runtime
- GPU encoding (NVENC, AMF, VAAPI, VideoToolbox)
- System tray integration
- MSI/DMG/AppImage installers
- Auto-update mechanism
- NAT traversal via TURN relay
- Multi-user sessions (view-only guests)
- Mobile client (iOS/Android)
- End-to-end encrypted clipboard (rich content)
- Drag-and-drop file transfer with GUI progress
- Audio device selection (choose which output to capture)
- Remote WebView2 debugging via QUIC tunnel (CDP proxy)

## v2.0.0 (Future)
- WebRTC transport option
- Cloud relay infrastructure
- Enterprise SSO integration
- Session recording playback in browser
- Collaborative annotations
- Remote printing
