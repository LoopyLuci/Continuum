# Continuum Wire Protocol

## Transport Layer

- **Protocol**: QUIC (quinn 0.25 + rustls 0.23)
- **ALPN**: `"apq-2"` (must match on both sides)
- **Certificate**: Self-signed via rcgen, persisted to disk
- **Trust**: TOFU (Trust-On-First-Use) fingerprint pinning on client

### Transport Tuning

| Parameter | Value |
|-----------|-------|
| Max concurrent bidi streams | 128 |
| Max concurrent uni streams | 64 |
| Datagram receive buffer | 1 MB |
| Keep-alive interval | 10s |
| Max idle timeout | 30s |

## Stream Types

Every bidirectional stream starts with a 4-byte big-endian `u32` header:

| Value | Type | Direction | Lifecycle |
|-------|------|-----------|-----------|
| 1 | Media | Server → Client | Long-lived |
| 2 | Intent | Client → Server | Per-message |
| 3 | Clipboard | Bidirectional | Per-message |
| 4 | FileTransfer | Bidirectional | Per-transfer |
| 5 | Audio | Server → Client | Long-lived |

## Media Stream

Server pushes frames continuously. Each frame:

```
[u32 sem_len]       Length of FrameSemantics JSON
[bytes sem_json]     FrameSemantics as JSON
[u32 frame_len]     Length of JPEG payload
[bytes jpeg_data]   JPEG-encoded frame
```

### FrameSemantics

```json
{
  "content_type": "image/jpeg",
  "width": 1920,
  "height": 1080,
  "quality": 85,
  "frame_number": 42,
  "timestamp": "2026-07-07T12:00:00Z",
  "is_keyframe": true,
  "monitor_id": 0,
  "encode_time_us": 5000
}
```

### Adaptive Behavior

- Quality adjusts between 30-95 based on encode time
- Frames with >98% similarity to previous frame are skipped (empty payload)
- Keyframes sent every 30 frames or on >15% pixel change
- Non-keyframe quality reduced to 70% of base quality

## Intent Stream

One message per stream, then `finish()`:

```
[u32 len]           Length of IntentMessage JSON
[bytes json]        IntentMessage as JSON
```

### IntentMessage Types

#### Pairing
```json
{
  "type": "pairing",
  "pairing_code": "continuum",
  "client_name": "My Laptop",
  "protocol_version": 2
}
```

Response:
```json
{
  "type": "pairing_result",
  "accepted": true,
  "message": "Pairing successful",
  "session_token": "session-1",
  "permissions": {
    "can_view": true,
    "can_control": true,
    "can_clipboard": true,
    "can_file_transfer": true,
    "can_audio": true
  }
}
```

#### Input
```json
{
  "type": "input",
  "action": "key_press",
  "x": null,
  "y": null,
  "button": null,
  "key": "enter",
  "modifiers": { "ctrl": false, "alt": false, "shift": false, "super_key": false },
  "scroll_x": null,
  "scroll_y": null,
  "monitor_id": 0
}
```

#### Heartbeat
```json
{
  "type": "heartbeat",
  "timestamp": "2026-07-07T12:00:00Z",
  "sequence": 42
}
```

Response:
```json
{
  "type": "heartbeat_ack",
  "timestamp": "2026-07-07T12:00:00Z",
  "sequence": 42
}
```

## Clipboard Stream

```
[u32 len]           Length of ClipboardData JSON
[bytes json]        ClipboardData as JSON
```

### ClipboardData
```json
{
  "content_type": "text/plain",
  "data": [72, 101, 108, 108, 111],
  "timestamp": "2026-07-07T12:00:00Z"
}
```

## Error Handling

IntentResponse::Error:
```json
{
  "type": "error",
  "code": 1002,
  "message": "Not authorized for input control"
}
```

Error codes:
- 1001: Input injection failed
- 1002: Not authorized
- 1003: Rate limited
- 1004: Invalid message format
- 1005: Server full
