#!/bin/bash
# macOS install script for pcloud-rs.
#
# Usage:
#   ./packaging/macos/install.sh [--prefix /usr/local] [--no-launchd] [--build]
#
# By default, copies binaries from target/release/ and installs the LaunchAgent.
# Pass --build to compile first.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

PREFIX="/usr/local"
INSTALL_LAUNCHD=true
DO_BUILD=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix) PREFIX="$2"; shift 2 ;;
        --no-launchd) INSTALL_LAUNCHD=false; shift ;;
        --build) DO_BUILD=true; shift ;;
        --help|-h)
            echo "Usage: $0 [--prefix DIR] [--no-launchd] [--build]"
            exit 0
            ;;
        *) echo "Unknown arg: $1" >&2; exit 1 ;;
    esac
done

BIN_DIR="$PREFIX/bin"

info()  { echo "  [info]  $*"; }
ok()    { echo "  [ ok ]  $*"; }
warn()  { echo "  [warn]  $*"; }
error() { echo "  [err!]  $*" >&2; }

# ── Build ─────────────────────────────────────────────────────────────────────
if [[ "$DO_BUILD" == true ]]; then
    info "Building release binaries..."
    cd "$ROOT_DIR"
    cargo build --release --locked -p pcloud-cli -p pcloud-daemon
    ok "Build complete"
fi

# ── Verify binaries exist ─────────────────────────────────────────────────────
for bin in pcloudc pcloudd; do
    if [[ ! -f "$ROOT_DIR/target/release/$bin" ]]; then
        error "Binary not found: target/release/$bin"
        error "Run with --build or build manually first."
        exit 1
    fi
done

# ── Install binaries ──────────────────────────────────────────────────────────
info "Installing binaries to $BIN_DIR..."
sudo mkdir -p "$BIN_DIR"
sudo install -o root -g wheel -m 755 "$ROOT_DIR/target/release/pcloudc" "$BIN_DIR/pcloudc"
sudo install -o root -g wheel -m 755 "$ROOT_DIR/target/release/pcloudd" "$BIN_DIR/pcloudd"
ok "Installed pcloudc and pcloudd to $BIN_DIR"

# ── Create user directories ───────────────────────────────────────────────────
info "Creating user directories..."
mkdir -p "$HOME/Library/Logs/pcloud-rs"
mkdir -p "$HOME/Library/Application Support/com.pcloud.pcloud-rs"
mkdir -p "$HOME/Library/Caches/com.pcloud.pcloud-rs"
mkdir -p "$HOME/pCloudDrive"
ok "Directories created"

# ── Install LaunchAgent ───────────────────────────────────────────────────────
if [[ "$INSTALL_LAUNCHD" == true ]]; then
    LAUNCH_AGENTS="$HOME/Library/LaunchAgents"
    mkdir -p "$LAUNCH_AGENTS"

    PLIST_SRC="$SCRIPT_DIR/com.pcloud.pcloud-rs.plist"
    PLIST_DST="$LAUNCH_AGENTS/com.pcloud.pcloud-rs.plist"

    info "Installing LaunchAgent plist..."
    # Substitute {{USER_HOME}} placeholder with actual $HOME
    sed "s|{{USER_HOME}}|$HOME|g; s|{{BIN_DIR}}|$BIN_DIR|g" \
        "$PLIST_SRC" > "$PLIST_DST"
    chmod 644 "$PLIST_DST"
    ok "Installed plist to $PLIST_DST"

    # Check if already loaded and unload first
    if launchctl list com.pcloud.pcloud-rs &>/dev/null; then
        info "Unloading existing LaunchAgent..."
        launchctl bootout "gui/$(id -u)" "$PLIST_DST" 2>/dev/null || true
    fi

    info "Loading LaunchAgent..."
    launchctl bootstrap "gui/$(id -u)" "$PLIST_DST"
    ok "LaunchAgent loaded — daemon will start now and at every login"

    echo ""
    echo "Installation complete."
    echo ""
    echo "  Check status: launchctl list com.pcloud.pcloud-rs"
    echo "  View logs:    tail -f ~/Library/Logs/pcloud-rs/pcloud-rs.err.log"
    echo "  Stop daemon:  launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist"
    echo "  CLI status:   pcloudc status"
else
    echo ""
    echo "Installation complete (launchd skipped)."
    echo "  Start daemon manually: pcloudd serve"
fi
