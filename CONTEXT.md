# Continuum — Project Context

Read this before touching code. It exists so an agent (or a human) picking
this project back up doesn't have to re-derive history from `Chat.txt` or
guess at what's real versus aspirational.

## What this project actually is right now

Continuum started as `Continuum.txt` — an aspirational design document
describing a "next-generation remote desktop platform" with QUIC transport,
AI-driven segmentation/super-resolution, DID-style security handshakes, and
plugin support. **None of that document was implemented when the repo was
created; it was pure spec.**

A Copilot session (transcript in `Chat.txt`) then built an actual working
prototype from scratch, iterating through many Rust compile-error and QUIC
handshake fixes. That prototype is what exists in `src/` today. A follow-up
Claude Code session (this one) fixed leftover branding, cleaned build
warnings, verified the build, and reorganized the repo (moved crates into
`src/`, added this documentation set).

**The gap between `Continuum.txt`'s ambition and the current code is large.**
Treat `Continuum.txt` as inspiration/vision, not as a spec the code follows.

## What's real vs. stub, crate by crate

| Crate | Wired into a binary? | What it actually does |
|---|---|---|
| `continuum-transport` | Yes — used by server, client | QUIC transport (quinn/rustls), wire types, JPEG codec, **real Windows screen capture** via the `screenshots` crate, **real Windows input injection** via `enigo`. Non-Windows falls back to a synthetic gradient demo frame (`codec.rs::synthesize_demo_frame`). |
| `continuum-server` | Yes (binary) | ~10-line wrapper that calls `continuum_transport::run_server`. All real logic lives in the transport crate. |
| `continuum-client` | Yes (binary) | egui/eframe GUI. Runs a background tokio thread that connects, opens a media stream + intent stream, renders frames, and forwards mouse clicks as `RemoteInputEvent`s. |
| `continuum-ai` | **No** — not referenced anywhere outside its own crate | Segmentation, super-resolution, intent agent, network predictor. All stub/placeholder logic from the original scaffold. Nothing calls into this crate. |
| `continuum-security` | **No** — not referenced anywhere outside its own crate | Double-ratchet and DID-handshake primitives. The actual transport uses plain rustls/TLS via QUIC (self-signed cert, no real identity verification) — this crate's E2E crypto is not in the data path at all. |
| `relay-server` | **No** — standalone binary, nothing connects to it | A bare TCP echo server on port 4434. Not a QUIC relay, not used by client or server. Effectively dead code kept from the original scaffold. |

If you're asked to "wire up AI features" or "add E2E encryption," know that
the scaffolding exists but is completely disconnected — this is greenfield
integration work, not a bug fix.

## Protocol as implemented (not as originally envisioned)

- Transport: QUIC via `quinn` 0.11 + `rustls` 0.23 (ring provider), ALPN
  `apq-1`, self-signed cert generated fresh on every server start (no
  persistence, no real trust — client currently accepts it unconditionally,
  see `continuum-transport/src/client.rs`).
- Two bidirectional stream types, selected by a 4-byte big-endian
  `ApqStreamType` header (`Media = 1`, `Intent = 2`). See
  `continuum-transport/src/types.rs`.
- **Media stream**: server pushes `(len, semantics_json)(len, jpeg_bytes)`
  frames continuously, ~30 fps target (`sleep(33ms)` in the capture loop).
  No backpressure, no adaptive bitrate, no keyframe/delta logic — every
  frame is a full JPEG.
- **Intent stream**: opened fresh per-message (not reused) for pairing
  handshakes and input events, length-prefixed JSON. Server infers which of
  `PairingHandshake` / `RemoteInputEvent` / free-text intent it received by
  trying each `serde_json` deserialization in order — see
  `handle_intent_stream` in `continuum-transport/src/server.rs`.
- Pairing is **not** authentication in any real sense: the server compares a
  plaintext string (`CONTINUUM_PAIRING_CODE`, default `"continuum"`) sent
  over the stream. Anyone who can reach the QUIC endpoint and guess/know the
  code can send input events — there's no session/token gating after
  pairing succeeds, and pairing success isn't even checked before the
  server accepts subsequent intent/input streams. **This is not secure.**

## Known-good verification (as of this session)

- `cargo build --bin continuum-server --bin continuum-client` — clean, zero
  warnings, zero errors.
- Server run standalone via `cargo run --bin continuum-server`: binds
  `127.0.0.1:4433`, prints pairing code, accepts a real QUIC connection from
  the client (`New connection: ...` logged).
- Client run standalone via `cargo run --bin continuum-client` from a real
  interactive terminal has **not** been visually confirmed working in this
  session — every attempt to launch it through the automated tooling here
  exited silently within seconds with **no error output at all** (not even
  the `eprintln!` that wraps `eframe::run_native`'s `Result`). This smells
  like a headless/no-display environment issue specific to the automation
  sandbox, not a code defect, but it has not been proven either way. **The
  next agent's first job should be verifying the client GUI actually opens
  and streams in a real interactive session** — see
  [docs/KNOWN_ISSUES.md](docs/KNOWN_ISSUES.md).

## Naming

The product name is **Continuum**. An earlier scaffold briefly used
"OmniHarness" in one UI heading; that's been removed. If you find
"OmniHarness" anywhere outside `Chat.txt`/`Continuum.txt` (which are
historical records and shouldn't be edited), treat it as a bug and rename it.

## Documentation map

- [README.md](README.md) — quickstart, build/run.
- [TODO.md](TODO.md) — prioritized roadmap, what to build next and why.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — crate/module map, data flow.
- [docs/TRANSPORT.md](docs/TRANSPORT.md) — wire protocol detail.
- [docs/SECURITY.md](docs/SECURITY.md) — current security posture and gaps.
- [docs/KNOWN_ISSUES.md](docs/KNOWN_ISSUES.md) — open bugs/unknowns, including the client-launch mystery above.
