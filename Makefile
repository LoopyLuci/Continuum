# Continuum — Makefile
# =====================
# No Docker or Kubernetes required. Everything runs as native binaries.
# Docker support is optional — only the `docker-*` targets need it.
#
# Usage:
#   make build          # Build server + client
#   make run-server     # Start server locally
#   make run-client     # Start client
#   make test           # Run all tests
#   make ci             # Full CI pipeline

# === Configuration (overridable) ===
CARGO ?= cargo
RUST_LOG ?= info
TARGET ?= release
SERVER_PORT ?= 4433

# === Build ===
.PHONY: build build-all check

build:
	$(CARGO) build --$(TARGET) --bin continuum-server --bin continuum-client

build-all:
	$(CARGO) build --$(TARGET) --workspace

check:
	$(CARGO) check --workspace

# === Run ===
.PHONY: run-server run-client run-relay run-ci

run-server:
	RUST_LOG=$(RUST_LOG) $(CARGO) run --$(TARGET) --bin continuum-server -- --listen 0.0.0.0:$(SERVER_PORT)

run-client:
	$(CARGO) run --$(TARGET) --bin continuum-client

run-relay:
	$(CARGO) run --$(TARGET) --bin relay-server

run-ci:
	$(CARGO) run --$(TARGET) -p continuum-ci -- run

# === Test ===
.PHONY: test test-quick ci

test:
	$(CARGO) test --workspace

test-quick:
	$(CARGO) test --workspace --no-fail-fast -q

ci: check test
	@echo "CI pipeline complete"

# === Docker (optional) ===
.PHONY: docker-build docker-up docker-down docker-logs

docker-build:
	docker compose build

docker-up:
	docker compose up -d

docker-down:
	docker compose down

docker-logs:
	docker compose logs -f

# === Lint ===
.PHONY: lint fmt

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

fmt:
	$(CARGO) fmt --all -- --check

fmt-fix:
	$(CARGO) fmt --all

# === Config ===
.PHONY: init-config

init-config:
	$(CARGO) run -p continuum-ci -- init continuum-ci.toml

# === Clean ===
.PHONY: clean

clean:
	$(CARGO) clean
	rm -rf target/ci-release recordings/

# === Help ===
.PHONY: help

help:
	@echo "Continuum — build targets"
	@echo ""
	@echo "  make build          Build release binaries"
	@echo "  make run-server     Start server"
	@echo "  make run-client     Start client (GUI)"
	@echo "  make test           Run all tests"
	@echo "  make ci             Full CI pipeline"
	@echo "  make docker-up      Docker Compose (optional)"
	@echo "  make fmt            Check formatting"
	@echo "  make lint           Run clippy"
