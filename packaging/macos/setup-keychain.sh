#!/bin/bash
# Set up pCloud credentials in the macOS Keychain for auto-login.
#
# After running this script, start the daemon with PCLOUD_DURABLE_AUTH_TOKENS=1
# (set in the LaunchAgent plist). The daemon will store auth tokens in the
# Keychain after the first successful login, and retrieve them automatically
# on subsequent startups without requiring interactive login.
#
# Usage:
#   ./packaging/macos/setup-keychain.sh [--check] [--clear]
#
#   --check  Check if credentials are stored in the Keychain
#   --clear  Remove stored credentials from the Keychain

set -euo pipefail

SERVICE="com.pcloud.pcloud-rs"
ACCOUNT="auth-token"

check_keychain() {
    if security find-generic-password \
        -s "$SERVICE" -a "$ACCOUNT" \
        &>/dev/null; then
        echo "Keychain entry found for $SERVICE / $ACCOUNT"
        echo "The daemon will use this token on next startup."
    else
        echo "No Keychain entry found for $SERVICE / $ACCOUNT"
        echo "Log in with 'pcloudc login' to store credentials."
    fi
}

clear_keychain() {
    if security delete-generic-password \
        -s "$SERVICE" -a "$ACCOUNT" \
        &>/dev/null; then
        echo "Removed Keychain entry for $SERVICE / $ACCOUNT"
    else
        echo "No entry to remove (already absent)"
    fi
}

show_status() {
    echo "pcloud-rs Keychain status"
    echo "========================="
    echo ""
    echo "Service:  $SERVICE"
    echo "Account:  $ACCOUNT"
    echo ""
    check_keychain
    echo ""
    echo "Configuration:"
    echo "  PCLOUD_DURABLE_AUTH_TOKENS is controlled by the LaunchAgent plist."
    echo "  When set to '1', the daemon stores the auth token in the Keychain"
    echo "  after login and retrieves it automatically on startup."
    echo ""
    echo "  To log in and store credentials:"
    echo "    pcloudc login"
    echo ""
    echo "  To remove stored credentials:"
    echo "    $0 --clear"
}

case "${1:-}" in
    --check) check_keychain ;;
    --clear) clear_keychain ;;
    *)       show_status ;;
esac
