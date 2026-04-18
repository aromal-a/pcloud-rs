#!/bin/bash
# Show the status of the pcloud-rs LaunchAgent and daemon.

LABEL="com.pcloud.pcloud-rs"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"

echo "pcloud-rs launchd status"
echo "========================"
echo ""

# launchctl status
echo "LaunchAgent:"
if launchctl list "$LABEL" 2>/dev/null; then
    echo "  (exit code 0 = loaded)"
else
    echo "  Not loaded — install with: ./packaging/macos/install.sh"
fi
echo ""

# Plist check
echo "Plist: $PLIST"
if [[ -f "$PLIST" ]]; then
    echo "  Present"
    plutil -lint "$PLIST" && echo "  Valid XML" || echo "  INVALID — fix before loading"
else
    echo "  Missing — run install.sh"
fi
echo ""

# Daemon connectivity
echo "Daemon socket:"
if pcloudc status &>/dev/null; then
    pcloudc status
else
    echo "  Not responding (daemon may not be running)"
fi
echo ""

# Logs
LOG_DIR="$HOME/Library/Logs/pcloud-rs"
echo "Recent logs ($LOG_DIR):"
if [[ -f "$LOG_DIR/pcloud-rs.err.log" ]]; then
    tail -20 "$LOG_DIR/pcloud-rs.err.log"
else
    echo "  No log file found"
fi
