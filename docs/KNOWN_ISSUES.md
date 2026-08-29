# Continuum Known Issues

## Resolved

- ~~**Client GUI exits silently under headless launch**~~ — Fixed with proper error handling and logging
- ~~**Hardcoded loopback address**~~ — Fixed with CLI args and config file support
- ~~**Self-signed cert regenerated every restart**~~ — Fixed with cert persistence
- ~~**No access gating**~~ — Fixed: media/input streams require successful pairing
- ~~**No reconnection logic**~~ — Fixed with exponential backoff reconnection
- ~~**Intent stream per message**~~ — Fixed: intent streams are now properly managed

## Active Issues

### Platform Support

- **Windows-only real capture**: Screen capture and input injection only work on Windows. macOS and Linux fall back to synthetic demo frames.
- **Input injection on macOS/Linux**: Not implemented. Requires platform-specific backends (core-graphics, xdotool).

### Performance

- **JPEG-only encoding**: No H.264/H.265/AV1 hardware encoding. JPEG is CPU-bound and less efficient for video content.
- **No GPU acceleration**: Frame encoding/decoding is CPU-only. GPU encode (NVENC, QuickSync) would significantly improve performance.
- **Full-frame JPEG**: No delta encoding or region-of-interest compression. Every frame is a complete JPEG.

### Network

- **No NAT traversal**: Direct connection only. Internet access requires manual port forwarding or a relay server.
- **No audio streaming**: Audio capture/playback is stubbed but not implemented.
- **No file transfer**: File transfer protocol is defined but not implemented.

### GUI

- **No drag-and-drop**: File drag-and-drop onto the client window is not implemented.
- **No multi-window**: Single window only. No support for multiple remote sessions.
- **No session recording**: Recording/playback is stubbed but not implemented.

## Workarounds

### Headless Server

The server can run headless (no display required). Only the client requires an interactive desktop session.

### Non-Windows Platforms

On macOS/Linux, the server will run but capture synthetic demo frames. Input injection is a no-op. This is useful for testing the transport layer.

### Internet Access

For internet access, either:
1. Port forward 4433 UDP to the server
2. Use the relay server (not yet implemented)
3. Use a VPN (Tailscale, WireGuard) between client and server
