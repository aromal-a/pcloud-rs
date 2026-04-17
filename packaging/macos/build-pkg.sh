#!/bin/bash
# Build a macOS .pkg installer for pcloud-rs.
#
# Prerequisites:
#   - Rust toolchain (stable)
#   - Xcode command line tools (pkgbuild, productbuild)
#   - fuse-t installed (optional — needed for runtime, not build time)
#
# Usage:
#   ./packaging/macos/build-pkg.sh [--sign DEVELOPER_ID] [--notarize]
#
# Output: target/pkg/pcloud-rs-<version>-macos.pkg

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$ROOT_DIR"

# ── Parse args ────────────────────────────────────────────────────────────────
SIGN_ID=""
DO_NOTARIZE=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --sign) SIGN_ID="$2"; shift 2 ;;
        --notarize) DO_NOTARIZE=true; shift ;;
        *) echo "Unknown arg: $1" >&2; exit 1 ;;
    esac
done

# ── Version ───────────────────────────────────────────────────────────────────
VERSION=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c "import sys,json; pkgs=json.load(sys.stdin)['packages']; \
      print([p['version'] for p in pkgs if p['name']=='pcloud-cli'][0])")
echo "Building pcloud-rs $VERSION for macOS"

PKG_OUT="$ROOT_DIR/target/pkg"
STAGE="$PKG_OUT/stage"
mkdir -p "$STAGE/usr/local/bin"
mkdir -p "$STAGE/Library/LaunchAgents"
mkdir -p "$PKG_OUT"

# ── Build release binaries ────────────────────────────────────────────────────
echo "Building release binaries..."
cargo build --release -p pcloud-cli -p pcloud-daemon

# ── Stage binaries ────────────────────────────────────────────────────────────
cp "$ROOT_DIR/target/release/pcloudc" "$STAGE/usr/local/bin/pcloudc"
cp "$ROOT_DIR/target/release/pcloudd" "$STAGE/usr/local/bin/pcloudd"

# ── Stage LaunchAgent plist ───────────────────────────────────────────────────
cp "$SCRIPT_DIR/com.pcloud.pcloud-rs.plist" \
   "$STAGE/Library/LaunchAgents/com.pcloud.pcloud-rs.plist"

# ── Sign binaries (optional) ──────────────────────────────────────────────────
if [[ -n "$SIGN_ID" ]]; then
    echo "Signing binaries with $SIGN_ID..."
    for bin in pcloudc pcloudd; do
        codesign --force --options runtime \
            --entitlements "$SCRIPT_DIR/entitlements.plist" \
            --sign "$SIGN_ID" \
            "$STAGE/usr/local/bin/$bin"
    done
fi

# ── Build .pkg ────────────────────────────────────────────────────────────────
COMPONENT_PKG="$PKG_OUT/pcloud-rs-component.pkg"
echo "Building component package..."
pkgbuild \
    --root "$STAGE" \
    --identifier "com.pcloud.pcloud-rs" \
    --version "$VERSION" \
    --install-location "/" \
    "$COMPONENT_PKG"

FINAL_PKG="$PKG_OUT/pcloud-rs-${VERSION}-macos.pkg"
echo "Building product archive..."
productbuild \
    --package "$COMPONENT_PKG" \
    --identifier "com.pcloud.pcloud-rs" \
    --version "$VERSION" \
    "$FINAL_PKG"

# ── Sign the .pkg (optional) ──────────────────────────────────────────────────
if [[ -n "$SIGN_ID" ]]; then
    echo "Signing .pkg..."
    productsign \
        --sign "$SIGN_ID" \
        "$FINAL_PKG" \
        "${FINAL_PKG%.pkg}-signed.pkg"
    mv "${FINAL_PKG%.pkg}-signed.pkg" "$FINAL_PKG"
fi

# ── Notarize (optional) ───────────────────────────────────────────────────────
if [[ "$DO_NOTARIZE" == "true" ]]; then
    echo "Notarizing..."
    bash "$ROOT_DIR/packaging/signing/notarize-macos.sh" "$FINAL_PKG"
fi

echo "Done: $FINAL_PKG"
