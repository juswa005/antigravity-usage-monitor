#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX_BIN="${XDG_BIN_HOME:-$HOME/.local/bin}"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/agmonitor"
SYSTEMD_USER_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

WITH_TIMER=0
INSTALL_DEPS=0

for arg in "$@"; do
    case "$arg" in
        --with-timer) WITH_TIMER=1 ;;
        --install-deps) INSTALL_DEPS=1 ;;
        -h|--help)
            echo "Usage: $0 [--install-deps] [--with-timer]"
            exit 0
            ;;
        *)
            echo "Unknown option: $arg" >&2
            exit 1
            ;;
    esac
done

install_distro_deps() {
    if [ -f /etc/arch-release ]; then
        echo "==> Installing Arch Linux dependencies via pacman..."
        sudo pacman -S --needed --noconfirm rust base-devel pkgconf sqlite procps-ng git
    elif [ -f /etc/debian_version ]; then
        echo "==> Installing Debian/Ubuntu dependencies via apt..."
        sudo apt-get update
        sudo apt-get install -y build-essential pkg-config libsqlite3-dev procps git curl
        if ! command -v cargo >/dev/null 2>&1; then
            echo "==> cargo not found in apt or PATH; please install Rust via rustup (https://rustup.rs) or 'sudo apt install -y cargo rustc'." >&2
            exit 1
        fi
    elif [ -f /etc/fedora-release ]; then
        echo "==> Installing Fedora dependencies via dnf..."
        sudo dnf install -y rust cargo gcc pkgconf-pkg-config sqlite-devel procps-ng git
    else
        echo "Unsupported distro for --install-deps; please install rust/cargo, gcc, pkg-config, and sqlite dev headers manually." >&2
        exit 1
    fi
}

if [ "$INSTALL_DEPS" -eq 1 ]; then
    install_distro_deps
fi

missing=()
command -v cargo >/dev/null 2>&1 || missing+=("cargo")
command -v rustc >/dev/null 2>&1 || missing+=("rustc")
if ! command -v cc >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1; then
    missing+=("gcc/cc")
fi

if [ "${#missing[@]}" -gt 0 ]; then
    echo "Error: missing required build tool(s)/libraries: ${missing[*]}" >&2
    if [ -f /etc/arch-release ]; then
        echo "On Arch Linux, run: sudo pacman -S --needed rust base-devel pkgconf sqlite procps-ng" >&2
    elif [ -f /etc/debian_version ]; then
        echo "On Debian/Ubuntu, run: sudo apt install -y build-essential pkg-config libsqlite3-dev procps cargo rustc" >&2
    fi
    echo "Or re-run with: $0 --install-deps" >&2
    exit 1
fi

# Prefer system libsqlite3 when pkg-config + sqlite3 headers are available;
# otherwise fall back cleanly to rusqlite's bundled sqlite3 compilation.
if command -v pkg-config >/dev/null 2>&1 && pkg-config --exists sqlite3 2>/dev/null; then
    export LIBSQLITE3_SYS_USE_PKG_CONFIG=1
    echo "==> Building agmonitor (release, linked against system sqlite3)..."
else
    echo "==> Building agmonitor (release, bundled sqlite3)..."
fi

(cd "$REPO_DIR" && cargo build --release --locked)

echo "==> Installing binary to $PREFIX_BIN/agmonitor"
install -Dm755 "$REPO_DIR/target/release/agmonitor" "$PREFIX_BIN/agmonitor"

if [ ! -f "$CONFIG_DIR/config.toml" ]; then
    echo "==> Writing default config to $CONFIG_DIR/config.toml"
    install -Dm644 "$REPO_DIR/config/config.toml" "$CONFIG_DIR/config.toml"
else
    echo "==> Existing config preserved at $CONFIG_DIR/config.toml"
fi

if [ "$WITH_TIMER" -eq 1 ]; then
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
