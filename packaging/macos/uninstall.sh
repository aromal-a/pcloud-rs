#!/bin/bash
# Uninstall pcloud-rs from macOS.
set -euo pipefail

PLIST="$HOME/Library/LaunchAgents/com.pcloud.pcloud-rs.plist"

echo "Stopping and removing LaunchAgent..."
if launchctl list com.pcloud.pcloud-rs &>/dev/null; then
    launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
    echo "  Stopped daemon"
fi
rm -f "$PLIST"
echo "  Removed plist"

echo "Removing binaries..."
sudo rm -f /usr/local/bin/pcloudc /usr/local/bin/pcloudd
echo "  Removed /usr/local/bin/pcloudc and pcloudd"

echo ""
echo "Note: User data directories are NOT removed:"
echo "  ~/Library/Application Support/com.pcloud.pcloud-rs  (config, state, vault)"
echo "  ~/Library/Caches/com.pcloud.pcloud-rs               (cache, runtime socket)"
echo "  ~/Library/Logs/pcloud-rs                             (logs)"
echo ""
echo "Remove them manually if you want a clean uninstall:"
echo "  rm -rf ~/Library/Application\ Support/com.pcloud.pcloud-rs"
echo "  rm -rf ~/Library/Caches/com.pcloud.pcloud-rs"
echo "  rm -rf ~/Library/Logs/pcloud-rs"
