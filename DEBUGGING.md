# Continuum Debugging Guide

## Local Debug API (CDP-Compatible)

Continuum includes a built-in debug API compatible with the Chrome DevTools Protocol (CDP). This API is designed for **agent-based testing and automation** — not for remote WebView2 inspection.

### Enabling the Debug API

```bash
# Start with debug API on port 9094
continuum --debug 9094

# Or with other flags
continuum --debug 9094 --record-dir ./recordings
```

The debug API binds to `127.0.0.1` only — it is not accessible from other machines.

### HTTP Endpoints

| Endpoint | Method | Description |
|---|---|---|
| `/json` | GET | List available debug targets (CDP format) |
| `/json/version` | GET | Browser version info |
| `/json/protocol` | GET | Supported CDP domains |
| `/` | GET | API info |

### WebSocket Endpoint

| Endpoint | Description |
|---|---|
| `/devtools/browser` | WebSocket for CDP commands and events |

### Supported CDP Methods

| Method | Description |
|---|---|
| `Page.enable` | Enable page events |
| `Page.captureScreenshot` | Get frame count info |
| `Runtime.enable` | Enable runtime events |
| `Runtime.evaluate` | Returns "Continuum Runtime" |
| `Runtime.getProperties` | Returns empty properties |
| `DOM.enable` | Enable DOM events |
| `DOM.getDocument` | Get UI element tree |
| `DOM.getBoxModel` | Get element bounds |
| `Input.dispatchMouseEvent` | Simulate mouse event |
| `Input.dispatchKeyEvent` | Simulate keyboard event |

### Example Usage

```bash
# Get debug targets
curl http://127.0.0.1:9094/json

# Get version info
curl http://127.0.0.1:9094/json/version

# Get UI elements via CDP
curl -X POST http://127.0.0.1:9094/devtools/browser \
  -d '{"id":1,"method":"DOM.getDocument"}'
```

### What This API Is

- A local testing interface for automated agents
- A CDP-compatible protocol for UI inspection
- A way to simulate input events programmatically
- A state query interface for connection status, frame count, etc.

### What This API Is NOT

- **Not remote WebView2 debugging** — it does not connect to a running WebView2 instance
- **Not a replacement for Chrome DevTools** — it exposes Continuum's own state, not a web page
- **Not accessible remotely** — localhost only for security

### Remote WebView2 Debugging (Planned for v1.1)

A future version will support tunneling CDP traffic over the QUIC connection to inspect WebView2 applications running on the remote machine. This requires:

1. Server-side WebView2 runtime detection
2. CDP WebSocket proxying over QUIC
3. Client-side local WebSocket endpoint for DevTools frontend
4. PAKE-encrypted tunnel for all debug traffic

See [ROADMAP.md](ROADMAP.md) for details.
