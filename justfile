# Continuum — justfile (https://github.com/casey/just)
# ====================================================
# No Docker or Kubernetes required.
#
# Usage:
#   just build        # Build server + client
#   just run-server   # Start server locally
#   just run-client   # Start client (requires display)
#   just test         # Run all tests
#   just ci           # Run local CI pipeline
#
# With Docker:
#   just docker-up    # Start all services via compose
#   just docker-logs  # Follow logs

# === Build ===
build:
    cargo build --release --bin continuum-server --bin continuum-client --bin relay-server

build-all:
    cargo build --release --workspace

check:
    cargo check --workspace

# === Run ===
run-server:
    RUST_LOG=info cargo run --release --bin continuum-server -- --listen 0.0.0.0:4433

run-client:
    cargo run --release --bin continuum-client

run-relay:
    cargo run --release --bin relay-server

run-ci:
    cargo run --release -p continuum-ci -- run

# === Test ===
test:
    cargo test --workspace

test-quick:
    cargo test --workspace --no-fail-fast -q

test-integration:
    cargo test --workspace --test protocol_tests

# === CI ===
ci: check test
    @echo "CI pipeline complete"

# === Docker (optional — works without) ===
docker-build:
    docker compose build

docker-up:
    docker compose up -d
    docker compose logs -f

docker-up-ci:
    docker compose --profile ci up -d

docker-logs:
    docker compose logs -f

docker-down:
    docker compose down

docker-clean:
    docker compose down -v

# === Clean ===
clean:
    cargo clean

clean-all: clean
    rm -rf recordings/ target/ci-release/

# === Lint ===
lint:
    cargo clippy --workspace --all-targets -- -D warnings

fmt:
    cargo fmt --all -- --check

fmt-fix:
    cargo fmt --all

# === Perf ===
bench:
    cargo bench

profile:
    cargo build --release --features "profiling"
    # Run with: CARGO_PROFILE_RELEASE_DEBUG=true

# === Generate default config ===
init-config:
    cargo run -p continuum-ci -- init continuum-ci.toml

# === Help ===
help:
    @echo "Continuum — build system"
    @echo ""
    @echo "Targets:"
    @echo "  build         Build release binaries"
    @echo "  run-server    Start the server"
    @echo "  run-client    Start the GUI client"
    @echo "  test          Run all tests"
    @echo "  ci            Run CI pipeline (check + test)"
    @echo "  docker-up     Start via Docker Compose (optional)"
    @echo "  fmt           Check formatting"
    @echo "  lint          Run clippy"
