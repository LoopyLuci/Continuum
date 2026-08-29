# Changelog

All notable changes to Continuum will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - Unreleased

### Added
- Relay server with real NAT traversal handshake: register, connect, and bidirectional stream relay.
- Clipboard and file transfer stream handlers on the server side, gated by paired session state.
- Resume-token eviction and periodic cleanup on the server to prevent unbounded growth.
- `--require-pake` CLI flag to enforce PAKE-based pairing only.
- Non-Windows capture/input scaffolding:
  - macOS capture via `scrap` + optional `core-graphics`
  - Linux capture via `scrap` + optional `libxdo`
- Workspace-level metadata: `rust-version`, `license`, `repository`.

### Changed
- Workspace metadata standardized across crates via `workspace.package`.
- `continuum-ai` marked deprecated until it is wired into runtime and Windows rustdoc toolchain issue is resolved.

### Fixed
- Client app struct initialization now matches declared fields after feature additions.

## [0.2.0] - 2026-08-28

### Added
- Initial public release candidate with transport, client, server, security, observability, test harness, and CI tooling.
