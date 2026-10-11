# The Aurora RFB profile

Aurora speaks RFB as defined by [RFC 6143](https://www.rfc-editor.org/rfc/rfc6143)
(version 3.8). On top of it, Aurora adds the community extensions that
RFC 6143 leaves to registries (VeNCrypt, ZRLE details, Tight and
pseudo-encodings) and a set of stricter rules. This document lists exactly
what Aurora implements. Anything not listed here is neither sent nor
accepted.

Implementations:

- [`rfb-proto`](../crates/rfb-proto) is the sans-I/O codec and handshake state machines used by both sides.
- [`rfb-tls`](../crates/rfb-tls) handles TLS.
- [`vnc-host`](../crates/vnc-host) is the server.
- [`vnc-client`](../crates/vnc-client) is the client.

Contents:

1. [Protocol versions](#1-protocol-versions)
2. [Security](#2-security)
3. [Initialisation](#3-initialisation)
4. [Client-to-server messages](#4-client-to-server-messages)
5. [Server-to-client messages](#5-server-to-client-messages)
6. [Encodings](#6-encodings)
7. [Limits](#7-limits)
8. [Errors and refusal](#8-errors-and-refusal)
9. [Clipboard](#9-clipboard)
10. [Interoperability](#10-interoperability)

Aurora Video (tiled AV1 for 8K at 240 fps, with its own messages, limits and vectors) is specified separately in [VIDEO.md](VIDEO.md).

## 1. Protocol versions

ProtocolVersion is 12 bytes, `"RFB xxx.yyy\n"` with three ASCII digits on each side of the dot.

- **What Aurora sends:** `RFB 003.008\n` (or the lower version that was agreed).
- **Mapping a received version:**

  | Received | Treated as |
  |---|---|
  | 3.3 | 3.3 |
  | 3.4, 3.5, 3.6 | 3.3 (as RFC 6143 section 7.1.1 requires) |
  | 3.7 | 3.7 |
  | 3.8 or higher | 3.8 |
  | major version above 3 | 3.8 |
  | anything else (major below 3, non-digits, missing `RFB `, `.` or `\n`) | refused |

- **Negotiated version:** the minimum of both sides' versions. The server replies with its own maximum and the client answers with the minimum. Both state machines then follow that version's rules:
  - 3.3: the server chooses the security type and sends it as a `u32`.
  - 3.7: there's no SecurityResult after None, and failures carry no reason.
- **Bad version string:** the server closes without a reply.

Vectors: [handshakes.txt](vectors/handshakes.txt) sections `pair none-3.8`, `pair none-3.7`, `pair vncauth-3.3`, `server not-rfb` and `server future-version`. Tests: [handshake.rs](../crates/rfb-proto/tests/handshake.rs).

## 2. Security

### 2.1 Security types

| Code | Name | Aurora |
|---|---|---|
| 0 | Invalid | used only to signal failure (3.3) |
| 1 | None | supported. A host offers it only when explicitly configured to; the `vnc-host` binary never offers it |
| 2 | VNC Authentication | supported. It's encrypted only inside VeNCrypt; at top level it needs an explicit insecure opt-in (`--insecure-no-tls`) |
| 16 | Tight | recognised, never offered or chosen |
| 19 | VeNCrypt | supported; the default and the only type the `vnc-host` binary offers unless told otherwise |

Rules:

- **Server offer:** the server lists, in its preference order, only the types it can complete. VncAuth needs a password. VeNCrypt needs at least one usable subtype and a TLS configuration.
- **Client choice:** the client picks the first type in *its own* preference order that the server offered and that it can complete. VncAuth needs a password. VeNCrypt needs a usable subtype (see 2.2).
- **Encrypted clients:** an encrypted client (the `vnc-client` default) prefers VeNCrypt only and never downgrades. If VeNCrypt isn't offered, it stops without sending anything (`pair encrypted-client-vs-plain-server`).
- **Choice not offered:** if a client picks a type the server didn't offer, the server sends a failed SecurityResult with reason `security type not offered` and closes (`server type-not-offered`).
- **Nothing usable:** a server with no usable type refuses at once:
  - 3.8 or 3.7: `count:u8 = 0`, then `reason-length:u32` and `reason`;
  - 3.3: `security-type:u32 = 0`, then `reason-length:u32` and `reason`.

  See `server no-usable-type-3.8` and `server no-usable-type-3.3`.

### 2.2 VeNCrypt (security type 19)

VeNCrypt version 0.2 only. Byte layouts after the client chose type 19:

```
S->C  major:u8 = 0, minor:u8 = 2
C->S  major:u8 = 0, minor:u8 = 2
S->C  ack:u8 (0 = version accepted; Aurora sends 0xff and closes for any other version)
S->C  count:u8, subtype:u32 x count     (count <= max_vencrypt_subtypes)
C->S  subtype:u32
S->C  ack:u8 (1 = start TLS; 0 = subtype not offered, then close)
      TLS 1.3 handshake; everything below runs inside TLS
      inner authentication (below), then SecurityResult
```

The client refuses a server version below 0.2. Subtypes:

| Code | Name | Inner authentication | Aurora |
|---|---|---|---|
| 256 | Plain | username/password **without TLS** | never offered or accepted |
| 257, 258, 259 | TLSNone, TLSVnc, TLSPlain | anonymous TLS (no certificate) | off by default on both sides (`allow_anonymous_tls`); the `vnc-host` binary never offers them |
| 260 | X509None | none | supported |
| 261 | X509Vnc | VNC Authentication inside TLS | supported; the host's default first choice |
| 262 | X509Plain | Plain credentials inside TLS | supported when the host has a verifier (`vnc-host --user`) |
| other | | | ignored in an offer; a client never chooses one |

Plain credentials:

```
username-length:u32, password-length:u32, username, password
```

- Both lengths are checked against `max_credential_len` *before* any data is read.
- If a length is over the limit, the server answers with a failed SecurityResult with reason `credentials too long` (`server credentials-too-long`).
- Credentials are compared in constant time.

The client's default subtype preference is X509Plain, X509Vnc, X509None. Each subtype is usable only if:

- `*Vnc`: the client has a password;
- `*Plain`: the client has both a username and a password;
- anonymous subtypes: the client explicitly allows them.

If no offered subtype is usable, the client stops (`client anonymous-tls-only`).

### 2.3 TLS rules

- **TLS 1.3 only.** TLS 1.2 and older aren't compiled in (rustls with the `ring` provider). A peer that can't do TLS 1.3 fails the TLS handshake.
- **Server identity:** the server presents an X.509 certificate. Clients MUST verify it, either against a CA set (`AURORA_VNC_TLS=ca:FILE`) or against a pinned SHA-256 fingerprint of the leaf certificate (`AURORA_VNC_TLS=pin:HEX`; `vnc-host` prints the fingerprint at start). Without a trust setting, an encrypted client refuses to connect. `insecure` (encrypt but accept any certificate) and `off` (no encryption) exist only as explicit opt-ins for testing and legacy peers.
- **TLS starts immediately after the subtype ack.** Bytes already received past the ack belong to TLS.
- **After TLS:** the inner authentication, SecurityResult, ClientInit, ServerInit and the whole message phase run inside TLS.
- **Self-signed certificates** generated by `vnc-host` cover `localhost`, `127.0.0.1` and `::1` (see [VERSIONING.md](VERSIONING.md#on-disk-formats) for the files).

Vectors:

- [handshakes.txt](vectors/handshakes.txt): `pair vencrypt-x509plain`, `pair vencrypt-x509vnc`, `pair vencrypt-plain-wrong-password`, `server vencrypt-version-0.1`, `server vencrypt-subtype-not-offered`, `server credentials-too-long`, `client too-many-subtypes`.

Tests:

- [vencrypt.rs](../crates/rfb-proto/tests/vencrypt.rs);
- TLS in memory and over TCP: [in_memory.rs](../crates/rfb-tls/tests/in_memory.rs), [tcp.rs](../crates/rfb-tls/tests/tcp.rs);
- against `x11vnc -ssl`: [x11vnc_vencrypt.rs](../crates/rfb-tls/tests/x11vnc_vencrypt.rs) and [client_tls_vs_x11vnc.rs](../crates/interop/tests/client_tls_vs_x11vnc.rs).

### 2.4 VNC Authentication

- The server sends a 16-byte challenge from the system CSPRNG.
- The client replies with the challenge DES-encrypted using the first 8 bytes of the password, NUL-padded, with each key byte's bits reversed (RFC 6143 section 7.2.2).
- Passwords longer than 8 bytes are truncated by the algorithm; `vnc-host` warns about this.
- The response is compared in constant time.

### 2.5 SecurityResult

```
status:u32 (0 = OK, 1 = failed)
if failed and version 3.8: reason-length:u32, reason
```

Rules:

- **Reasons:** `authentication failed`, `security type not offered` or `credentials too long`. The reason never reveals which part of a credential was wrong.
- **Lengths:** a received reason longer than `max_reason_len` is a protocol error.
- **When it's sent:** for None in 3.3 and 3.7 no SecurityResult is sent. 3.8 sends one after every type, including None.
- **After a failure:** the server closes the connection. `vnc-host` waits one second before closing, to slow down password guessing.

Vectors: `pair vncauth-wrong-password-3.8` and `pair vncauth-wrong-password-3.3`.

## 3. Initialisation

```
C->S  ClientInit:  shared:u8 (nonzero = share; Aurora clients send 1)
S->C  ServerInit:  width:u16, height:u16, pixel-format (16 bytes),
                   name-length:u32, name (UTF-8 by Aurora; treat as bytes)
```

- **Name length:** must be at most `max_name_len`.
- **Shared flag:** `vnc-host` always shares (several viewers see one desktop). It accepts any value of the flag and never disconnects other viewers.

Pixel format (16 bytes):

```
bits-per-pixel:u8, depth:u8, big-endian:u8, true-colour:u8,
red-max:u16, green-max:u16, blue-max:u16,
red-shift:u8, green-shift:u8, blue-shift:u8, padding:3
```

- **Flag bytes:** decoded as "nonzero is true" and sent as 0 or 1 (x11vnc sends 0xff).
- **Validity:** bits-per-pixel must be 8, 16 or 32, with `1 <= depth <= bits-per-pixel`. Anything else is a protocol error.
- **Aurora's default format** is 32 bpp, depth 24, little-endian, true colour, max 255 and shifts 16/8/0. Its wire bytes are `20 18 00 01 00ff 00ff 00ff 10 08 00 000000`.

Vectors: [messages.txt](vectors/messages.txt) section `server-init`. Tests: [messages.rs](../crates/rfb-proto/tests/messages.rs).

## 4. Client-to-server messages

| Type | Message | Layout after `type:u8` | Host behaviour |
|---|---|---|---|
| 0 | SetPixelFormat | `padding:3, pixel-format` | Accepts true colour at 8/16/32 bpp whose channels fit; colour-map formats only at 8 bpp, answered with SetColourMapEntries for a fixed 3-3-2 map. Anything else is a protocol error. All tiles are resent in the new format. |
| 2 | SetEncodings | `padding:1, count:u16, encoding:s32 x count` | count <= `max_encodings`. Order is preference. Replaces the previous list. Unknown codes are ignored. |
| 3 | FramebufferUpdateRequest | `incremental:u8, x:u16, y:u16, width:u16, height:u16` | One pending request per viewer. A non-incremental request sends the whole region; an incremental one waits until something in it changes. |
| 4 | KeyEvent | `down:u8, padding:2, keysym:u32` | Injected (X11: XTEST), unless view-only |
| 5 | PointerEvent | `button-mask:u8, x:u16, y:u16` | Clamped to the screen, then injected, unless view-only |
| 6 | ClientCutText | `padding:3, length:u32, text` | length <= `max_cut_text_len`; see [Clipboard](#9-clipboard) |
| 6 | ClientCutText, extended | `padding:3, length:s32 < 0, body` (`-length` bytes) | Only after Extended Clipboard is negotiated; see [9.1](#91-extended-clipboard) |
| 251 | SetDesktopSize | `padding:1, width:u16, height:u16, count:u8, padding:1, screen x count` | Only from viewers that listed ExtendedDesktopSize; see [6.8](#68-extendeddesktopsize) |
| 65 | Aurora message | `subtype:u8, reserved:u16, length:u32, body` | Only after the host sent VideoOffer (before that it is a protocol error); see [VIDEO.md](VIDEO.md#3-aurora-messages). |

Any other message type is a protocol error and ends the session; there's no way to skip an unknown message. Vectors: [messages.txt](vectors/messages.txt) sections `client`.

## 5. Server-to-client messages

| Type | Message | Layout after `type:u8` | Notes |
|---|---|---|---|
| 0 | FramebufferUpdate | `padding:1, count:u16`, then `count` rectangles | Each rectangle is a 12-byte header `x:u16, y:u16, width:u16, height:u16, encoding:s32` followed by its payload (section 6) |
| 1 | SetColourMapEntries | `padding:1, first:u16, count:u16, (r:u16, g:u16, b:u16) x count` | count <= `max_colour_map_entries`; sent only for 8-bpp colour-map formats |
| 2 | Bell | nothing | |
| 3 | ServerCutText | `padding:3, length:u32, text` | length <= `max_cut_text_len`; Latin-1 |
| 3 | ServerCutText, extended | `padding:3, length:s32 < 0, body` (`-length` bytes) | Only to viewers that listed Extended Clipboard; see [9.1](#91-extended-clipboard) |
| 65 | Aurora message | `subtype:u8, reserved:u16, length:u32, body` | Only to viewers that listed Aurora Video (0x41555200); see [VIDEO.md](VIDEO.md#3-aurora-messages) |

Host update rules:

- **When updates are sent:** the host only answers a FramebufferUpdateRequest. Updates are never unsolicited.
- **Damage tracking:** damage is tracked in 64x64 tiles against what this viewer was last sent.
- **Too many rectangles:** above 256 rectangles, one bounding rectangle is sent instead.
- **Resizes:** a resize goes alone as a one-rectangle update with a DesktopSize pseudo-rectangle, to viewers that asked for DesktopSize. Others get a cropped or padded screen.

Vectors: [messages.txt](vectors/messages.txt) sections `server` and `rect`.

## 6. Encodings

| Code | Encoding | Host sends | Client accepts |
|---|---|---|---|
| 0 | Raw | yes | yes |
| 1 | CopyRect | yes (scrolls) | yes |
| 7 | Tight | yes (fill + basic: copy and palette filters) | yes (fill, copy, palette, gradient, JPEG) |
| 16 | ZRLE | yes | yes |
| 2, 5, 15 | RRE, Hextile, TRLE | no | no |
| -223 | DesktopSize (pseudo) | yes, if requested | yes |
| -256 .. -247 | CompressLevel 0..9 (pseudo) | honoured: zlib level for new streams, default 2 | sent if configured |
| -32 .. -23 | JPEG quality 0..9 (pseudo) | ignored (the host doesn't produce JPEG) | sent only when the user opts in to lossy JPEG |
| -239 | Cursor (pseudo) | yes, if listed and Cursor With Alpha is not ([6.7](#67-cursor-shapes)) | yes |
| -314 | Cursor With Alpha (pseudo) | yes, if listed (preferred) | not yet |
| -308 | ExtendedDesktopSize (pseudo) | yes, if listed (instead of DesktopSize); SetDesktopSize honoured where the backend can resize ([6.8](#68-extendeddesktopsize)) | yes; the viewer sends SetDesktopSize when its window resizes |
| 0x41555200 | Aurora Video: tiled AV1 (HEVC, H.264 optional) ([VIDEO.md](VIDEO.md)) | sent: AV1 tiles from software rav1e (zero latency, every picture intra); hardware backends not yet | not yet |
| 0xC0A1E5CE | Extended Clipboard (pseudo) | yes, if listed: UTF-8 text with caps, notify, request, peek and provide ([9.1](#91-extended-clipboard)) | not yet |

Encoding selection:

- **Host:** each rectangle uses the first of Raw, ZRLE and Tight in the viewer's SetEncodings order. Raw is used if the list names none of them. CopyRect is used only when listed.
- **Client default list:** CopyRect, ZRLE, Tight, Raw, ExtendedDesktopSize, DesktopSize, Cursor.

Byte-exact vectors for Raw, ZRLE and Tight in ten pixel formats are in [encodings.txt](../crates/rfb-proto/vectors/encodings.txt). Tests: [encodings.rs](../crates/rfb-proto/tests/encodings.rs) (round trips and hand-built payloads), and Aurora's encoders against noVNC's decoders in [novnc_decoders.rs](../crates/rfb-proto/tests/novnc_decoders.rs).

### 6.1 Pixel values on the wire

- **PIXEL:** `bits-per-pixel / 8` bytes in the format's byte order.
- **CPIXEL (ZRLE):** 3 bytes, but only when the format is true colour at 32 bpp, depth <= 24, and every channel's bits fit in either the low 3 bytes or the high 3 bytes. Those 3 bytes are sent in the format's byte order. In every other case CPIXEL is a PIXEL. The high-3-bytes case is where libvncserver has a bug; see [10.2](#102-interop-notes).
- **TPIXEL (Tight):** 3 bytes R, G, B when the format is 32 bpp, depth 24, all maxes 255. Otherwise a PIXEL.

### 6.2 Raw (0)

- **Payload:** `width x height` PIXELs, row by row.

### 6.3 CopyRect (1)

- **Payload:** `src-x:u16, src-y:u16`. The rectangle is copied from that position on the client's current framebuffer.
- **Host use:** the host finds vertical scrolls by row hashing and sends at most one move per run of changed columns. Moves go before any tiles in the same update. They're never sent to a viewer that didn't list CopyRect.

### 6.4 ZRLE (16)

- **Payload:** `length:u32` followed by `length` bytes of one zlib stream that lasts for the whole connection.
- **Flushing:** each rectangle ends with a sync flush.
- **Tiles:** the decompressed data covers the rectangle in 64x64 tiles, row by row. Each tile starts with `subencoding:u8`:

  | Subencoding | Contents |
  |---|---|
  | 0 | raw CPIXELs |
  | 1 | one CPIXEL (solid) |
  | 2-16 | a palette of that many CPIXELs, then packed indices (1, 2 or 4 bits per pixel, each row padded to a byte) |
  | 128 | plain RLE: CPIXEL, run |
  | 130-255 | palette RLE: a palette of `subencoding - 128` CPIXELs, then indices with the top bit marking a following run |

- **Run lengths:** written as `255`-continued bytes; the run is the sum of the bytes plus 1.
- **Host encoder choice:** it picks the smallest subencoding per tile.
- **Decoder bounds:** the inflate is bounded by what the rectangle can need. A length above that, a palette index out of range, or a run past the tile is a protocol error.

### 6.5 Tight (7)

`control:u8`:

- **Low 4 bits:** reset zlib stream N, one bit per stream.
- **High 4 bits:**
  - `0x8` fill: one TPIXEL.
  - `0x9` JPEG: compact length, then a JPEG image. Accepted by the client; never sent by the host.
  - `0x0-0x7` basic compression: bit 6 is the explicit-filter flag and bits 4-5 the zlib stream (0-3).
- **Filters:**

  | Filter | Name | Followed by |
  |---|---|---|
  | 0 | copy | TPIXELs |
  | 1 | palette | `count-1:u8`, then that many TPIXELs; indices are 1 bit for 2 colours, otherwise 8 bits, rows padded to bytes |
  | 2 | gradient | predicted TPIXELs. Accepted; the host doesn't send it |

- **Data size:** data under 12 bytes is sent uncompressed. Otherwise it's a *compact length* (1-3 bytes, 7 bits each, low bits first, high bit = more) followed by that much zlib data.
- **Host streams:** stream 0 carries copy data, stream 1 two-colour palettes and stream 2 other palettes.
- **Rectangle size:** at most 2048 pixels wide and 65536 pixels in area. The host splits larger rectangles.

### 6.6 Pseudo-encodings

- **DesktopSize (-223):** a rectangle header with the new width and height and no payload.
- **CompressLevel and JPEG quality:** carry no rectangles. When several of one kind are listed, the last one wins.
- **Cursor, Cursor With Alpha, ExtendedDesktopSize:** see [6.7](#67-cursor-shapes) and [6.8](#68-extendeddesktopsize). **Extended Clipboard** carries no rectangles; see [9.1](#91-extended-clipboard).

Pseudo-rectangle vectors: [messages.txt](vectors/messages.txt) sections `pseudo-rect`. Codecs: `rfb_proto::pseudo`; fuzz target `pseudo_rect`.

### 6.7 Cursor shapes

Both carry the pointer's shape so the viewer can draw it locally. The rectangle's x and y are the **hotspot**, its width and height the cursor's size; each side is at most `max_cursor_size`. A 0x0 cursor (no payload beyond the fixed parts) hides the pointer.

- **Cursor (-239):** `pixel x width*height` in the session pixel format (section 6.1), then a bitmask of `ceil(width/8) * height` bytes: rows padded to whole bytes, most significant bit = leftmost pixel, 1 = visible. Aurora's host derives the mask from the desktop cursor's alpha (`alpha >= 128` is visible) and sends un-premultiplied colour.
- **Cursor With Alpha (-314):** `encoding:s32`, then the pixels in that encoding, always as 32-bit RGBA bytes (R, G, B, A in that order) with **premultiplied** alpha, regardless of the session pixel format. Aurora sends and accepts only Raw (0) inside, as TigerVNC does; anything else is `Invalid`.
- **Preference:** a host sends only one of the two, Cursor With Alpha if the viewer listed it, otherwise Cursor.

Host behaviour:

- **Source:** the X11 backend reads the desktop's pointer with XFixes GetCursorImage (premultiplied ARGB) and fetches it again only after a DisplayCursorNotify event. The capture thread polls it after each grab, so it follows changes while any viewer is waiting for an update.
- **When it is sent:** with the viewer's next update, after it connects, after the shape changes, after the viewer starts listing a cursor encoding, and (classic Cursor only) after SetPixelFormat. A shape change alone answers a pending incremental request (an update with just the cursor rectangle).
- **Size:** a shape larger than `max_cursor_size` on either side isn't sent; the viewer keeps the previous one.
- **Viewers that list neither** get nothing; the cursor stays wherever the capture draws it (X11 GetImage does not include the pointer).

Tests: [extensions.rs](../crates/vnc-host/tests/extensions.rs) (mode choice, re-sends, mask threshold, pixel formats), `cursor_shape_reaches_vnc_client` in [session.rs](../crates/vnc-host/tests/session.rs) (Aurora's independent client decoder), [tcp.rs](../crates/vnc-host/tests/tcp.rs), `x11_cursor_shape_follows_the_desktop` in [x11_live.rs](../crates/vnc-host/tests/x11_live.rs).

### 6.8 ExtendedDesktopSize

ExtendedDesktopSize (-308) reports the framebuffer size together with its screen layout and answers SetDesktopSize. Rectangle header: `x` = reason, `y` = status, `width`/`height` = framebuffer size. Payload: `count:u8, padding:3, screen x count`, where a screen is 16 bytes: `id:u32, x:u16, y:u16, width:u16, height:u16, flags:u32`.

| Reason (x) | Meaning |
|---|---|
| 0 | Server-side change, or the answer to a non-incremental FramebufferUpdateRequest |
| 1 | Reply to this client's SetDesktopSize |
| 2 | Another client's SetDesktopSize succeeded |

| Status (y) | Meaning |
|---|---|
| 0 | OK |
| 1 | Resize prohibited (by policy or backend) |
| 2 | Out of resources (the backend cannot produce that size) |
| 3 | Invalid screen layout |
| 4 | Request forwarded (a later rectangle carries the outcome) |

Unknown reasons are treated as 0. Status is meaningful only with reason 1.

A **valid layout** (`rfb_proto::pseudo::validate_layout`) has 1 to 255 screens, each non-empty and entirely inside the framebuffer, with unique ids. Flags are preserved as received; new screens use 0.

SetDesktopSize (client type 251) carries the same screen list with the requested framebuffer size. Rules (community specification):

- A server that saw ExtendedDesktopSize in SetEncodings answers **every non-incremental** FramebufferUpdateRequest with an ExtendedDesktopSize rectangle (reason 0) describing the current layout, and uses it instead of DesktopSize for that viewer.
- ExtendedDesktopSize rectangles come **first** in their update. Framebuffer data may follow in the same update only when they don't change the size; an update that changes the size carries no framebuffer data (TigerVNC sends every ExtendedDesktopSize rectangle in an update of its own). Clients must accept both forms and must not answer an ExtendedDesktopSize rectangle with a non-incremental request.
- On success the requester gets reason 1 / status 0 with exactly the layout it asked for, and every other ExtendedDesktopSize viewer gets reason 2 with the same layout (DesktopSize viewers get DesktopSize).
- On failure only the requester hears about it: reason 1 with the status, the current size and layout.
- Replies are never merged or reordered.

Host behaviour:

- **Layout source:** the test pattern reports the layout it was given (one whole screen by default). The X11 backend reports the RandR 1.5 monitors (ids are the monitor-name atoms), refreshed when the size changes and at most once a second otherwise. A layout the server can't express validly (for example a monitor at a negative position) is reported as one whole screen with id 0.
- **Non-incremental requests:** each one from an ExtendedDesktopSize viewer is answered with the layout rectangle (reason 0) followed by the whole screen, in one update, saving the round trip TigerVNC needs. Same-size layout changes and refusals likewise share an update with any pending pixels. A size change goes alone, and the screen at the new size follows on the next request.
- **Server-side changes:** a size change reaches ExtendedDesktopSize viewers as reason 0 (or as reason 2 when it is the result of another viewer's request seen within the last 10 s), and DesktopSize viewers as DesktopSize. A layout change at the same size is sent to ExtendedDesktopSize viewers only.
- **SetDesktopSize:** ignored from a viewer that didn't list ExtendedDesktopSize. View-only viewers get status 1, and invalid layouts status 3 without reaching the backend. The backend then decides:
  - *Test pattern* (`--backend test`): any valid size and layout succeeds.
  - *X11*: refused with status 1 unless the host runs with `--allow-resize`, so remote viewers can't resize a real desktop by default. With the flag, a single-screen request is applied by RandR: the screen is first grown to fit both sizes, the output's CRTC switched to a mode of the requested size (created and added to the output if missing), and the screen then shrunk to fit, all under a server grab, with the old configuration restored on any failure. The CRTC is never disabled, so clients never see a zero-sized screen. Multi-screen requests, X servers with more than one active CRTC, and sizes outside the RandR range get status 2. Xvnc accepts 32..32768 on each side; Xvfb accepts sizes up to its startup size.
- **After a success** the requester gets reason 1 with exactly the requested layout, the other ExtendedDesktopSize viewers reason 2, and DesktopSize viewers DesktopSize. The new screen follows once a frame of the new size has been captured (the host waits up to 2 s for it).

Tests: [extensions.rs](../crates/vnc-host/tests/extensions.rs) (session rules), [tcp_extensions.rs](../crates/vnc-host/tests/tcp_extensions.rs) (three viewers over TLS), `x11_layout_and_randr_resize` in [x11_live.rs](../crates/vnc-host/tests/x11_live.rs) (Xvfb and Xvnc as X servers), and rfb-proto's encoders and decoders against TigerVNC in [tigervnc_extensions.rs](../crates/rfb-proto/tests/tigervnc_extensions.rs). Property tests: [pseudo_properties.rs](../crates/rfb-proto/tests/pseudo_properties.rs).

## 7. Limits

Every attacker-controlled length is bounded before anything is allocated. The defaults are below; embedders may raise them. A value over its limit is a protocol error (`Invalid`), never a large allocation. Data still arriving is `Incomplete` and is never buffered past the limit. The table must match `Limits::default()` exactly (checked by preflight and by [spec.rs](../crates/rfb-proto/tests/spec.rs)).

<!-- BEGIN LIMITS (machine-checked; values are decimal) -->
| Limit | Default | Bounds |
|---|---|---|
| `max_name_len` | 65536 | ServerInit desktop name, bytes |
| `max_reason_len` | 65536 | failure and refusal reasons, bytes |
| `max_cut_text_len` | 16777216 | ClientCutText / ServerCutText, bytes |
| `max_encodings` | 1024 | SetEncodings entries |
| `max_colour_map_entries` | 65535 | SetColourMapEntries entries |
| `max_vencrypt_subtypes` | 32 | VeNCrypt subtypes in one offer (sent or received) |
| `max_credential_len` | 4096 | VeNCrypt Plain username or password, bytes each |
| `max_cursor_size` | 1024 | Cursor / Cursor With Alpha width and height, pixels each |
| `max_aurora_msg_len` | 65536 | Aurora message body (type 65, either direction), bytes |
| `max_video_tiles` | 64 | tiles (independent video streams) in one VideoConfig |
| `max_video_payload` | 67108864 | compressed data in one Aurora Video rectangle, bytes |
<!-- END LIMITS -->

Other fixed bounds:

| Bound | Value | Where |
|---|---|---|
| clipboard text kept and forwarded by `vnc-host` | 1 MiB | `vnc_host::clipboard::DEFAULT_MAX_LEN` |
| client framebuffer | 16384 x 16384 pixels | `vnc-client` `max_framebuffer_pixels` |
| Tight rectangle | 2048 wide, 65536 pixels | section 6.5 |
| ZRLE and Tight inflate | what the rectangle can need | sections 6.4 and 6.5 |

Tests:

- hostile lengths: [regressions.rs](../crates/fuzzing/tests/regressions.rs);
- fuzzing with oracles: [properties.rs](../crates/fuzzing/tests/properties.rs), run with `cargo xtask fuzz`.

## 8. Errors and refusal

| Situation | Server does | Client does |
|---|---|---|
| Malformed ProtocolVersion | closes, no reply | reports `Decode` and closes |
| No usable security type | refuses with reason (section 2.1) | reports `Refused(reason)` |
| No common security type | n/a | sends nothing more, closes (`NoCommonSecurity`) |
| Type or subtype not offered | failed SecurityResult or VeNCrypt ack 0, then closes | n/a |
| VeNCrypt version mismatch | `0xff`, closes | refuses a server below 0.2 |
| Too many VeNCrypt subtypes | never sends more than `max_vencrypt_subtypes` | protocol error, closes |
| TLS failure (no 1.3, bad certificate, wrong pin) | closes after the TLS alert | reports a TLS error before any RFB authentication |
| Wrong password or credentials | failed SecurityResult with reason, a 1 s delay, closes | reports `AuthFailed(reason)` |
| Length over a limit | protocol error, closes | protocol error, closes |
| Unknown message type | closes (messages can't be skipped) | closes |
| Rectangle in an encoding the client didn't list | n/a | protocol error, closes |
| Input from a view-only viewer | ignored silently | n/a |

General rules:

- **No input after failure:** a failed state machine never consumes input again; further calls return `Finished`.
- **Reasons are generic and bounded.** They never echo attacker input or say which credential was wrong.
- **Chunking doesn't matter:** both state machines produce the same output and outcome however the input is split into chunks. The property tests and fuzzers check this at every split point ([properties.rs](../crates/fuzzing/tests/properties.rs)).

Vectors: every `server` and `client` section of [handshakes.txt](vectors/handshakes.txt).

## 9. Clipboard

- **Encoding:** the host keeps the clipboard as UTF-8 with LF line ends and no NUL. Viewers that listed Extended Clipboard (9.1) get and send UTF-8. Other viewers use cut text, which is Latin-1 (RFC 6143), converted at the edge:
  - characters outside Latin-1 become `?`;
  - CRLF becomes LF;
  - NUL is dropped.
- **Size:** it keeps and forwards at most 1 MiB of UTF-8, cut at a character boundary.
- **When a viewer connects** it gets the current text, once it has sent SetEncodings (so the host knows which form to use), or after 2 s for a viewer that sends nothing.
- **Viewer to desktop:** clipboard text from a viewer (either form) becomes the desktop's CLIPBOARD and PRIMARY selections (X11). It's also sent to every *other* viewer, never echoed back to the sender.
- **Desktop to viewers:** a desktop selection change (CLIPBOARD or PRIMARY) is sent to every viewer.
- **Duplicates:** re-sending the current text does nothing.
- **View-only viewers** neither set nor receive the clipboard. `vnc-host --no-clipboard` turns sharing off.
- **Extended Clipboard:** UTF-8 with formats and an explicit request/provide exchange; see 9.1.

- **X11:** the host serves its text as `UTF8_STRING`, as `STRING` (Latin-1), and as `TEXT` (STRING when Latin-1 can hold the text, otherwise UTF8_STRING). It reads desktop selections as UTF8_STRING, falling back to STRING. Large selections arrive by ICCCM INCR. A transfer over the limit is drained to its end, and the first 1 MiB is kept. The host never *sends* with INCR, so the text it serves must fit in one request (16 MiB with BIG-REQUESTS).

Tests: [x11_clipboard.rs](../crates/vnc-host/tests/x11_clipboard.rs) (live X11 with xclip or xsel, UTF-8 both ways, INCR from an x11rb owner, including one over the limit), [tcp.rs](../crates/vnc-host/tests/tcp.rs), [tcp_extensions.rs](../crates/vnc-host/tests/tcp_extensions.rs) (an Extended Clipboard viewer and a Latin-1 viewer sharing it).

### 9.1 Extended Clipboard

Pseudo-encoding 0xC0A1E5CE (-1063131698). Once a viewer has listed it, a ClientCutText or ServerCutText whose length, read as **s32**, is negative carries an extended body of `-length` bytes (`i32::MIN` is `Invalid`). The body starts with `flags:u32`:

| Bits | Meaning |
|---|---|
| 0 text, 1 RTF, 2 HTML, 3 DIB, 4 files (5-15 reserved) | Formats |
| 24 caps, 25 request, 26 peek, 27 notify, 28 provide | Actions |

| Action | After the flags | Meaning |
|---|---|---|
| caps | `size:u32` per set format bit, ascending | Formats and actions the sender supports, and the largest unsolicited provide it accepts per format |
| request | nothing | Send me these formats (provide) |
| peek | nothing | Tell me what you have (notify) |
| notify | nothing | My clipboard now holds these formats (none = empty) |
| provide | zlib stream | For each format bit set, ascending: `size:u32, data` |

- **Text** is UTF-8 with CRLF line ends and a terminating NUL (`text_to_wire` / `text_from_wire`). Receivers stop at the first NUL and turn CRLF into LF.
- **Compression:** each provide uses a fresh zlib stream. TigerVNC and Aurora sync-flush instead of finishing it; decoders must accept either.
- **Limits:** the body is at most `max_cut_text_len + max_cut_text_len/64 + 4096` bytes on the wire, and a provide's **decompressed** data at most `max_cut_text_len` in total. Inflation stops at that bound, so a small zip bomb is refused rather than expanded.
- **Caps:** a server sends caps after every SetEncodings that lists the pseudo-encoding. A peer that never sent caps is assumed to support text, RTF and HTML with request, notify and provide, accepting text up to 20 MiB. Without caps, a message sets exactly one action. A decoder that sees several picks caps, then provide, request, peek, notify; one with no known action is kept as `Other` and ignored.

Host behaviour (`vnc-host`):

- **Caps:** text only, with request, peek, notify and provide; the text size is the host's clipboard limit (1 MiB). The viewer's caps are remembered; until it sends them, the defaults above apply.
- **Desktop or another viewer changed the clipboard:** the host sends Notify (text, or no formats when empty) and answers the viewer's Request with Provide. A viewer whose caps lack notify but include provide gets Provide straight away. Text larger than the viewer's advertised size is not sent: the Provide has no formats, as in TigerVNC.
- **Peek** is answered with Notify.
- **The viewer's Notify** that includes text is answered at once with Request (if the viewer's caps include request). Its Provide becomes the host clipboard. TigerVNC instead waits until a local application reads the selection, and pushes desktop changes as an unsolicited Provide. Clients must handle both: answer Request at any time, and accept Provide without having asked.
- Latin-1 ClientCutText is still accepted from an Extended Clipboard viewer.
- **View-only viewers** get caps but no clipboard, and their Notify and Provide are ignored. Extended messages from viewers that didn't list the pseudo-encoding are ignored.
- **Over the limit:** a provide whose decompressed text exceeds `max_cut_text_len` is a protocol error, like oversized cut text.

Vectors: [messages.txt](vectors/messages.txt) `client` and `server` sections with `ExtendedClipboard`. Codec: `rfb_proto::clipboard`; fuzz target `ext_clipboard`; property tests in [pseudo_properties.rs](../crates/rfb-proto/tests/pseudo_properties.rs). Host tests: [extensions.rs](../crates/vnc-host/tests/extensions.rs), [tcp_extensions.rs](../crates/vnc-host/tests/tcp_extensions.rs). Against TigerVNC: `tigervnc_extended_clipboard` in [tigervnc_extensions.rs](../crates/rfb-proto/tests/tigervnc_extensions.rs).

## 10. Interoperability

### 10.1 Tested peers

These run in every `cargo xtask ci` on GitHub, which sets `AURORA_REQUIRE_INTEROP=1` so a missing tool fails the run instead of skipping.

| Peer | Role | What's checked | Tests |
|---|---|---|---|
| x11vnc / libvncserver | server | None and VncAuth handshakes; Raw, ZRLE and Tight (lossless and JPEG) decoding against Raw in several pixel formats; VeNCrypt X509None and X509Vnc over TLS 1.3 (`-ssl`, OpenSSL) | [x11vnc.rs](../crates/interop/tests/x11vnc.rs), [client_vs_x11vnc.rs](../crates/interop/tests/client_vs_x11vnc.rs), [zrle_vs_x11vnc.rs](../crates/interop/tests/zrle_vs_x11vnc.rs), [tight_vs_x11vnc.rs](../crates/interop/tests/tight_vs_x11vnc.rs), [client_tls_vs_x11vnc.rs](../crates/interop/tests/client_tls_vs_x11vnc.rs), [x11vnc_vencrypt.rs](../crates/rfb-tls/tests/x11vnc_vencrypt.rs), [x11vnc_vector.rs](../crates/rfb-proto/tests/x11vnc_vector.rs) with the recorded [x11vnc-rfb38-none-raw.txt](../crates/interop/vectors/x11vnc-rfb38-none-raw.txt) |
| TigerVNC (`Xvnc`) | server | ZRLE and Tight (lossless and JPEG) against Raw in several pixel formats; an encoder family independent of libvncserver | [tigervnc.rs](../crates/interop/tests/tigervnc.rs) |
| TigerVNC (`Xvnc`) | server | ExtendedDesktopSize and SetDesktopSize (initial layout, success with reason 1 and 2 plus DesktopSize to a legacy viewer, invalid layout to the requester only); Cursor and Cursor With Alpha; Extended Clipboard (caps both ways, UTF-8 text in both directions through xclip). rfb-proto decodes every pseudo-rectangle and its encoders reproduce TigerVNC's bytes exactly | [tigervnc_extensions.rs](../crates/rfb-proto/tests/tigervnc_extensions.rs) |
| noVNC 1.6.0 | client decoders | Aurora's ZRLE and Tight encoder output decoded by noVNC's own `core/decoders` under node (noVNC before 1.4 has no ZRLE decoder, so CI downloads 1.6.0) | [novnc_decoders.rs](../crates/rfb-proto/tests/novnc_decoders.rs) |
| Aurora vnc-client and vnc-viewer vs vnc-host | both | full sessions over pinned TLS 1.3 | [client_vs_vnc_host.rs](../crates/interop/tests/client_vs_vnc_host.rs), [client_zrle.rs](../crates/vnc-host/tests/client_zrle.rs) |

### 10.2 Interop notes

- **libvncserver ZRLE drops red when colours are in the top byte.**
  - Trigger: a client asks for a 32-bit true-colour format with colours in the top 3 bytes (shifts 24/16/8).
  - Cause: libvncserver's "fits in the low 3 bytes" test computes `red-max << red-shift` in a signed `int`, which overflows. It then sends the low 3 bytes, which don't contain red.
  - Result: decoding those bytes as section 6.1 requires gives red = green, green = blue, blue = 0.
  - Aurora's encoder handles this layout correctly (tested in [client_zrle.rs](../crates/vnc-host/tests/client_zrle.rs) and [encodings.txt](../crates/rfb-proto/vectors/encodings.txt)). Aurora's own default format never puts colour in the top byte.
  - Against libvncserver servers, clients SHOULD NOT request such formats with ZRLE. The bug is pinned by `zrle_high_bytes_32_shows_libvncserver_overflow_bug` in [zrle_vs_x11vnc.rs](../crates/interop/tests/zrle_vs_x11vnc.rs), so we notice if it's ever fixed.
- **x11vnc may close before the failure reason.** With `-ssl`, x11vnc can close the TLS connection before sending the SecurityResult reason for a wrong password. The close often comes without a TLS close_notify. Clients MUST treat a close at that point (after sending the VNC auth response) as an authentication failure. Tests: `wrong_password_is_rejected_inside_tls` in [client_tls_vs_x11vnc.rs](../crates/interop/tests/client_tls_vs_x11vnc.rs) and `x509vnc_wrong_password_is_refused_by_x11vnc` in [x11vnc_vencrypt.rs](../crates/rfb-tls/tests/x11vnc_vencrypt.rs).
- **x11vnc and MIT-SHM.** A killed x11vnc leaks its SysV shared-memory segments, so every test fixture runs it with `-noshm`. `cargo xtask` preflight removes orphaned segments.
- **TigerVNC precedes a SetDesktopSize reply with reason-0 rectangles.** After a successful request, Xvnc sends one reason-0 ExtendedDesktopSize rectangle per internal RandR step (three for a plain resize), then the reason-1 reply, all in one update; other viewers likewise get reason 0 before reason 2. Viewers MUST accept any number of reason-0 rectangles before the reply and apply each. Aurora's host sends only the reply. Pinned in [tigervnc_extensions.rs](../crates/rfb-proto/tests/tigervnc_extensions.rs).
- **TigerVNC's Extended Clipboard is lazy one way and eager the other.** After a viewer's Notify, Xvnc takes the X selection but sends Request only when an X application reads it. Desktop changes reach the viewer as an unsolicited Provide, without Notify. Aurora's host requests at once and announces with Notify. Pinned in `tigervnc_extended_clipboard`.
- **TigerVNC sends a hidden (0x0) cursor until a client moves the pointer** after the root cursor is set. Tests set the cursor while connected and keep moving the pointer until a shape arrives.
- **X servers refuse GetCursorImage for an orphaned cursor.** When the displayed cursor was created by a client that has since disconnected (for example `xsetroot -cursor_name`), Xvfb and Xvnc answer XFixes GetCursorImage with BadAccess (the SECURITY extension's resource check finds no owner). The host keeps the previous shape until the cursor changes again. Real desktops are unaffected, since toolkits and window managers stay connected. Tests set cursors from a connected client.
- **x11vnc pixel-format flags:** x11vnc sends 0xff in the pixel-format flag bytes. Aurora accepts any nonzero value (section 3).
