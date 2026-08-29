# Rust Edition / Toolchain Stability Policy

## Decision
This repository targets **Rust 2021** compatibility. Do not introduce `async fn main`,
`async move` blocks, or edition-only syntax in binary crates unless the workspace
toolchain is explicitly upgraded and verified across Windows/macOS/Linux.

## Rationale
- Some local build environments fail on doctest/rustdoc for async-heavy crates.
- For 100-year durability, prefer stable, conservative Rust features over edition-specific
  constructs unless the maintenance team agrees to bump the minimum toolchain.

## Implementation rule
- Keep binary startup synchronous where possible.
- Use existing async runtime paths inside libraries only, and guard them with feature
  flags so non-Windows or older toolchains can still compile core crates.
