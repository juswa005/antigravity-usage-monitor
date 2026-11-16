#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX_BIN="${XDG_BIN_HOME:-$HOME/.local/bin}"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/agmonitor"
SYSTEMD_USER_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

if ! command -v cargo >/dev/null 2>&1; then
    echo "Error: cargo not found. Install Rust (rustup or your distro's cargo package) first." >&2
    exit 1
fi

echo "==> Building agmonitor (release)..."
(cd "$REPO_DIR" && cargo build --release --locked)

echo "==> Installing binary to $PREFIX_BIN/agmonitor"
install -Dm755 "$REPO_DIR/target/release/agmonitor" "$PREFIX_BIN/agmonitor"

if [ ! -f "$CONFIG_DIR/config.toml" ]; then
    echo "==> Writing default config to $CONFIG_DIR/config.toml"
    install -Dm644 "$REPO_DIR/config/config.toml" "$CONFIG_DIR/config.toml"
else
    echo "==> Existing config preserved at $CONFIG_DIR/config.toml"
fi

if [ "${1:-}" = "--with-timer" ]; then
    echo "==> Installing systemd user service & 5-minute refresh timer"
    install -Dm644 "$REPO_DIR/packaging/systemd/agmonitor-refresh.service" "$SYSTEMD_USER_DIR/agmonitor-refresh.service"
    install -Dm644 "$REPO_DIR/packaging/systemd/agmonitor-refresh.timer" "$SYSTEMD_USER_DIR/agmonitor-refresh.timer"
    sed -i "s|%h/.local/bin/agmonitor|$PREFIX_BIN/agmonitor|g" "$SYSTEMD_USER_DIR/agmonitor-refresh.service"
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload
        systemctl --user enable --now agmonitor-refresh.timer
    fi
fi

echo ""
echo "Done! Ensure $PREFIX_BIN is in your PATH, then run:"
echo "  agmonitor doctor"
echo "  agmonitor"
