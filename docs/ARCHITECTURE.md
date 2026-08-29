# Continuum Architecture

## Workspace Layout

| Crate | Type | Purpose |
|-------|------|---------|
| `continuum-transport` | Library | Core transport, capture, input, codec, TLS, configuration |
| `continuum-server` | Binary | Host application, thin wrapper around transport |
| `continuum-client` | Binary | GUI client with modular egui architecture |
| `continuum-security` | Library | DID handshake, double-ratchet encryption |
| `continuum-ai` | Library | Super-resolution, segmentation, intent prediction |
| `relay-server` | Binary | QUIC relay for NAT traversal |

## Data Flow

### Media Stream (Host → Client)

```
Server                          Client
  │                               │
  ├─ capture_monitor()            │
  ├─ AdaptiveEncoder::encode()    │
  ├─ write [sem_len][sem_json]    │
  ├─ write [frame_len][jpeg]  ────┼─ read_frame()
  │                               ├─ decode_jpeg()
  │                               ├─ FrameRenderer::update()
  │                               └─ egui texture upload
  └─ (repeat at adaptive FPS)
```

### Intent Stream (Client → Server)

```
Client                          Server
  │                               │
  ├─ InputHandler::handle()       │
  ├─ open Intent stream           │
  ├─ write [len][IntentMessage]───┼─ handle_intent_stream()
  │                               ├─ match IntentMessage
  │                               │   ├─ Pairing → auth check
  │                               │   ├─ Input → InputInjector::apply()
  │                               │   ├─ Clipboard → store
  │                               │   └─ FileRequest → handle
  │                               ├─ write [len][IntentResponse]
  │   read IntentResponse  ←──────┘
  └─ close stream
```

## Module Map

### continuum-transport

```
src/
├── lib.rs          Module declarations, re-exports
├── types.rs        All wire types: enums, structs, serde impls
├── config.rs       CLI args (clap), config file (toml), env vars
├── capture.rs      Screen capture, monitor enumeration
├── codec.rs        Adaptive JPEG encoder, frame diff detection
├── input.rs        OS-level input injection (enigo)
├── tls.rs          Cert generation/persistence, TOFU pinning
├── server.rs       QUIC server, connection handling, media/intent streams
└── client.rs       QUIC client, connection management, reconnection
```

### continuum-client

```
src/
├── main.rs         Entry point, logging setup, egui launcher
├── app.rs          ContinuumApp state machine, panel rendering
├── connection.rs   Network thread, event/command channels
├── rendering.rs    Frame decoding, texture management, FPS tracking
├── input.rs        Keyboard/mouse capture, coordinate mapping
├── settings.rs     Settings window, config display
└── widgets.rs      Custom egui widgets: status, stats, toolbars
```

## Security Model

1. **Transport**: QUIC mandates TLS 1.3 encryption
2. **Server Auth**: TOFU certificate pinning (first connect trusts, warns on change)
3. **Client Auth**: Pairing code with rate limiting (5 attempts/60s)
4. **Access Control**: Media/input/clipboard/file streams require successful pairing
5. **Session Tokens**: Generated on successful pairing for future verification

## Configuration Hierarchy

1. CLI arguments (highest priority)
2. Environment variables
3. Config file (continuum-{server,client}.toml)
4. Default values (lowest priority)

Config file search order:
1. Path specified by `--config` flag
2. `~/.config/continuum/{server,client}.toml`
3. `./continuum-{server,client}.toml`

## Adaptive Encoding

The `AdaptiveEncoder` dynamically adjusts quality based on:
- Encode time vs target frame time
- Frame similarity (skip unchanged frames)
- Keyframe interval (every 30 frames or on significant change)
- Quality range: 30-95 JPEG quality

## Reconnection Strategy

Exponential backoff with jitter:
- Base delay: 500ms
- Multiplier: 1.5x
- Max delay: 30s
- Jitter: ±25% of delay
- Reset on successful connection
