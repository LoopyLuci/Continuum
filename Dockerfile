# =============================================================================
# Continuum — Multi-stage Docker build
# =============================================================================
# Build:          docker build --target server -t continuum-server .
#                 docker build --target relay -t continuum-relay .
#                 docker build --target ci -t continuum-ci .
# Run:            docker compose up -d
# No K8s needed:  docker compose handles everything
# =============================================================================

# ---- Builder stage ----
FROM rust:1.81-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
COPY src/continuum-transport/Cargo.toml src/continuum-transport/
COPY src/continuum-server/Cargo.toml src/continuum-server/
COPY src/continuum-client/Cargo.toml src/continuum-client/
COPY src/continuum-ai/Cargo.toml src/continuum-ai/
COPY src/continuum-security/Cargo.toml src/continuum-security/
COPY src/continuum-ci/Cargo.toml src/continuum-ci/
COPY src/continuum-observability/Cargo.toml src/continuum-observability/
COPY src/continuum-plugin-sdk/Cargo.toml src/continuum-plugin-sdk/
COPY src/relay-server/Cargo.toml src/relay-server/

RUN mkdir -p src/continuum-transport/src \
    src/continuum-server/src \
    src/continuum-client/src \
    src/continuum-ai/src \
    src/continuum-security/src \
    src/continuum-ci/src \
    src/continuum-observability/src \
    src/continuum-plugin-sdk/src \
    src/relay-server/src \
    && echo "fn main() {}" > src/continuum-server/src/main.rs \
    && echo "fn main() {}" > src/continuum-client/src/main.rs \
    && echo "fn main() {}" > src/relay-server/src/main.rs \
    && echo "fn main() {}" > src/continuum-ci/src/main.rs \
    && touch src/continuum-transport/src/lib.rs \
    && touch src/continuum-ai/src/lib.rs \
    && touch src/continuum-security/src/lib.rs \
    && touch src/continuum-observability/src/lib.rs \
    && touch src/continuum-plugin-sdk/src/lib.rs \
    && cargo build --release --workspace 2>/dev/null || true

# Now build for real with all source
COPY . .
RUN cargo build --release \
    --bin continuum-server \
    --bin continuum-client \
    --bin relay-server \
    --bin continuum-ci

# ---- Server image ----
FROM debian:bookworm-slim AS server
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl libc6 && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/continuum-server /usr/local/bin/
EXPOSE 4433/udp 9091
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl -f http://localhost:9091/health || exit 1
ENTRYPOINT ["continuum-server"]

# ---- Relay image ----
FROM debian:bookworm-slim AS relay
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libc6 && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/relay-server /usr/local/bin/
EXPOSE 4434/udp
ENTRYPOINT ["relay-server"]

# ---- CI image ----
FROM debian:bookworm-slim AS ci
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git libc6 && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/continuum-ci /usr/local/bin/
EXPOSE 9090
ENTRYPOINT ["continuum-ci"]
