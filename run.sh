#!/bin/bash
# Continuum — Self-managing launcher (Linux/macOS)
# ================================================
# Usage:
#   ./run.sh              # Start server + client
#   ./run.sh server       # Start server only
#   ./run.sh client       # Start client only
#   ./run.sh stop         # Stop all
#   ./run.sh status       # Check status
#   ./run.sh watch        # Start server + CI watcher
# ================================================

set -e
ROOT="$(cd "$(dirname "$0")" && pwd)"

stop_all() {
    pkill -f continuum-server 2>/dev/null || true
    pkill -f continuum-client 2>/dev/null || true
    echo "All Continuum processes stopped."
}

status() {
    echo ""
    echo "  Continuum Status"
    echo "  ================="
    pgrep -x continuum-server > /dev/null && echo "  Server:   RUNNING" || echo "  Server:   STOPPED"
    pgrep -x continuum-client > /dev/null && echo "  Client:   RUNNING" || echo "  Client:   STOPPED"
    echo "  ================="
    echo ""
}

start_server() {
    EXE="$ROOT/continuum-server"
    if [ ! -f "$EXE" ]; then
        echo "Building server..."
        cargo build --release --bin continuum-server
    fi
    echo "Starting server on 0.0.0.0:4433..."
    nohup "$EXE" --listen 0.0.0.0:4433 > "$ROOT/server.log" 2>&1 &
    echo "Server PID: $!"
    sleep 2
}

start_client() {
    EXE="$ROOT/continuum-client"
    if [ ! -f "$EXE" ]; then
        echo "Building client..."
        cargo build --release --bin continuum-client
    fi
    echo "Starting client..."
    "$EXE" &
}

ACTION="${1:-}"
case "$ACTION" in
    stop)   stop_all ;;
    status) status ;;
    server)
        start_server
        status ;;
    client)
        start_client
        sleep 2
        status ;;
    watch)
        start_server
        sleep 3
        status
        echo "Starting CI watcher..."
        cargo run --release -p continuum-ci -- watch
        ;;
    *)
        start_server
        sleep 3
        status
        start_client
        echo ""
        echo "Both server and client are running."
        echo "  Use ./run.sh status to check status"
        echo "  Use ./run.sh stop to stop all"
        ;;
esac
