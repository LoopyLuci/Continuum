# Contributing to Continuum

Thank you for your interest in contributing to Continuum!

## Getting Started

1. Fork the repository
2. Clone your fork
3. Install Rust 1.75+ (https://rustup.rs)
4. Run `cargo build` to verify the build works
5. Create a feature branch: `git checkout -b feature/my-feature`

## Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test --workspace

# Run clippy
cargo clippy --workspace --all-targets

# Check formatting
cargo fmt --all -- --check
```

## Pull Request Process

1. Ensure all tests pass: `cargo test --workspace`
2. Run clippy with no warnings: `cargo clippy --workspace --all-targets -- -D warnings`
3. Format your code: `cargo fmt --all`
4. Update documentation if needed
5. Write a clear PR description explaining what and why

## Code Style

- Follow standard Rust conventions
- Use `anyhow::Result` for error propagation
- Use structured logging via `tracing`
- Never use `unwrap()` in production code paths
- Add tests for new functionality

## Security

If you find a security vulnerability, please see [SECURITY.md](SECURITY.md) for reporting instructions.

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
