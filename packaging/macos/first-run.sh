#!/bin/bash
# Interactive first-run setup for pcloud-rs on macOS.
# Guides the user through installation, login, and first mount.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo ""
echo "pcloud-rs macOS Setup"
echo "====================="
echo ""

# Step 1: Check fuse-t
echo "Step 1: Checking for fuse-t..."
FUSET_FOUND=false
for lib in \
    "/usr/local/lib/libfuse-t.dylib" \
    "/opt/homebrew/lib/libfuse-t.dylib" \
    "/Library/Application Support/fuse-t/lib/libfuse-t.dylib"; do
    if [[ -f "$lib" ]]; then
        FUSET_FOUND=true
        echo "  fuse-t found at: $lib"
        break
    fi
done
if [[ "$FUSET_FOUND" == false ]]; then
    echo "  fuse-t not found."
    echo "  Install it with: brew install --cask fuse-t"
    echo "  Or download from: https://www.fuse-t.org/"
    echo ""
    read -p "Continue without fuse-t? (mount will not work) [y/N] " yn
    [[ "${yn,,}" == "y" ]] || exit 1
fi
echo ""

# Step 2: Install
echo "Step 2: Installing pcloud-rs..."
if command -v pcloudc &>/dev/null && command -v pcloudd &>/dev/null; then
    echo "  Binaries already installed: $(which pcloudc), $(which pcloudd)"
else
    read -p "  Build and install now? [Y/n] " yn
    if [[ "${yn,,}" != "n" ]]; then
        bash "$SCRIPT_DIR/install.sh" --build
    fi
fi
echo ""

# Step 3: LaunchAgent
echo "Step 3: LaunchAgent status..."
if launchctl list com.pcloud.pcloud-rs &>/dev/null; then
    echo "  LaunchAgent is already loaded."
else
    read -p "  Install LaunchAgent (auto-start on login)? [Y/n] " yn
    if [[ "${yn,,}" != "n" ]]; then
        bash "$SCRIPT_DIR/install.sh"
    fi
fi
echo ""

# Step 4: Login
echo "Step 4: pCloud login..."
if pcloudc status 2>/dev/null | grep -q "authenticated"; then
    echo "  Already authenticated."
else
    read -p "  Log in to pCloud now? [Y/n] " yn
    if [[ "${yn,,}" != "n" ]]; then
        pcloudc login
    fi
fi
echo ""

# Step 5: Mount
echo "Step 5: Mount pCloud drive..."
MOUNT_POINT="$HOME/pCloudDrive"
mkdir -p "$MOUNT_POINT"
read -p "  Mount at $MOUNT_POINT? [Y/n] " yn
if [[ "${yn,,}" != "n" ]]; then
    pcloudc mount "$MOUNT_POINT" && echo "  Mounted at $MOUNT_POINT" || echo "  Mount failed — check logs with: ./launchd-status.sh"
fi
echo ""

echo "Setup complete."
echo "  Status:    pcloudc status"
echo "  Logs:      tail -f ~/Library/Logs/pcloud-rs/pcloud-rs.err.log"
echo "  Unmount:   pcloudc unmount"
echo ""
