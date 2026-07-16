#!/bin/bash
# Build a macOS .dmg disk image for pcloud-rs.
#
# Prerequisites:
#   - Rust toolchain (stable)
#   - hdiutil (built-in on macOS)
#   - Optional: create-dmg (brew install create-dmg) for a prettier DMG
#
# Usage:
#   ./packaging/macos/build-dmg.sh [--sign DEVELOPER_ID]
#
# Output: target/pkg/pcloud-rs-<version>-macos.dmg

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$ROOT_DIR"

SIGN_ID=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --sign) SIGN_ID="$2"; shift 2 ;;
        *) echo "Unknown arg: $1" >&2; exit 1 ;;
    esac
done

VERSION=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c "import sys,json; pkgs=json.load(sys.stdin)['packages']; \
      print([p['version'] for p in pkgs if p['name']=='pcloud-cli'][0])")

PKG_OUT="$ROOT_DIR/target/pkg"
DMG_STAGE="$PKG_OUT/dmg-stage"
mkdir -p "$DMG_STAGE"
mkdir -p "$PKG_OUT"

echo "Building release binaries..."
cargo build --release --locked -p pcloud-cli -p pcloud-daemon

cp "$ROOT_DIR/target/release/pcloudc" "$DMG_STAGE/pcloudc"
cp "$ROOT_DIR/target/release/pcloudd" "$DMG_STAGE/pcloudd"
cp "$ROOT_DIR/docs/MACOS.md" "$DMG_STAGE/README-macOS.md"

if [[ -n "$SIGN_ID" ]]; then
    for bin in pcloudc pcloudd; do
        codesign --force --options runtime \
            --entitlements "$SCRIPT_DIR/entitlements.plist" \
            --sign "$SIGN_ID" \
            "$DMG_STAGE/$bin"
    done
fi

DMG_PATH="$PKG_OUT/pcloud-rs-${VERSION}-macos.dmg"

if command -v create-dmg >/dev/null 2>&1; then
    create-dmg \
        --volname "pcloud-rs $VERSION" \
        --window-size 600 400 \
        --icon-size 80 \
        "$DMG_PATH" \
        "$DMG_STAGE"
else
    hdiutil create \
        -volname "pcloud-rs $VERSION" \
        -srcfolder "$DMG_STAGE" \
        -ov \
        -format UDZO \
        "$DMG_PATH"
fi

echo "Done: $DMG_PATH"
