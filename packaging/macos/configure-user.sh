#!/bin/bash
# Materialize and load the packaged per-user LaunchAgent.

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "configure-user.sh must run on macOS." >&2
    exit 1
fi
if [[ "$(id -u)" -eq 0 ]]; then
    echo "Run this helper as the login user, not with sudo." >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE="$SCRIPT_DIR/com.pcloud.pcloud-rs.plist"
BIN_DIR="${PCLOUD_BIN_DIR:-/usr/local/bin}"
PLIST_DIR="$HOME/Library/LaunchAgents"
PLIST="$PLIST_DIR/com.pcloud.pcloud-rs.plist"
DOMAIN="gui/$(id -u)"

if [[ ! -x "$BIN_DIR/pcloudd" || ! -x "$BIN_DIR/pcloudc" ]]; then
    echo "pcloudc and pcloudd were not found in $BIN_DIR." >&2
    exit 66
fi
if [[ ! -f "$TEMPLATE" ]]; then
    echo "LaunchAgent template not found: $TEMPLATE" >&2
    exit 66
fi

mkdir -p \
    "$PLIST_DIR" \
    "$HOME/Library/Logs/pcloud-rs" \
    "$HOME/Library/Application Support/com.pcloud.pcloud-rs" \
    "$HOME/Library/Caches/com.pcloud.pcloud-rs" \
    "$HOME/pCloudDrive"

escape_sed_replacement() {
    printf '%s' "$1" | sed 's/[&|\\]/\\&/g'
}

escaped_home="$(escape_sed_replacement "$HOME")"
escaped_bin="$(escape_sed_replacement "$BIN_DIR")"
tmp="${PLIST}.tmp.$$"
trap 'rm -f "$tmp"' EXIT
sed \
    -e "s|{{USER_HOME}}|$escaped_home|g" \
    -e "s|{{BIN_DIR}}|$escaped_bin|g" \
    "$TEMPLATE" > "$tmp"
plutil -lint "$tmp"
chmod 644 "$tmp"

launchctl bootout "$DOMAIN" "$PLIST" 2>/dev/null || true
mv "$tmp" "$PLIST"
launchctl bootstrap "$DOMAIN" "$PLIST"
launchctl enable "$DOMAIN/com.pcloud.pcloud-rs"
launchctl kickstart -k "$DOMAIN/com.pcloud.pcloud-rs"
trap - EXIT

echo "Configured pcloud-rs for $USER."
echo "LaunchAgent: $PLIST"
echo "Status: pcloudc status"
