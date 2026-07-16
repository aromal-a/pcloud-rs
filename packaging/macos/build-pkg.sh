#!/bin/bash
# Build a macOS .pkg installer for pcloud-rs.
#
# Prerequisites:
#   - Rust toolchain (stable)
#   - Xcode command line tools (pkgbuild, productbuild)
#   - fuse-t installed (optional — needed for runtime, not build time)
#
# Usage:
#   ./packaging/macos/build-pkg.sh \
#       [--application-sign "Developer ID Application: ..."] \
#       [--installer-sign "Developer ID Installer: ..."] \
#       [--notarize]
#
# Output: target/pkg/pcloud-rs-<version>-macos.pkg

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$ROOT_DIR"

# ── Parse args ────────────────────────────────────────────────────────────────
APPLICATION_SIGN_ID=""
INSTALLER_SIGN_ID=""
DO_NOTARIZE=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --application-sign) APPLICATION_SIGN_ID="$2"; shift 2 ;;
        --installer-sign) INSTALLER_SIGN_ID="$2"; shift 2 ;;
        --notarize) DO_NOTARIZE=true; shift ;;
        --help|-h)
            sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "Unknown arg: $1" >&2; exit 1 ;;
    esac
done

if [[ -n "$APPLICATION_SIGN_ID" && -z "$INSTALLER_SIGN_ID" ]] || \
   [[ -z "$APPLICATION_SIGN_ID" && -n "$INSTALLER_SIGN_ID" ]]; then
    echo "Both --application-sign and --installer-sign are required for a signed package." >&2
    exit 64
fi
if [[ "$DO_NOTARIZE" == true && -z "$APPLICATION_SIGN_ID" ]]; then
    echo "--notarize requires both signing identities." >&2
    exit 64
fi

# ── Version ───────────────────────────────────────────────────────────────────
VERSION=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c "import sys,json; pkgs=json.load(sys.stdin)['packages']; \
      print([p['version'] for p in pkgs if p['name']=='pcloud-cli'][0])")
echo "Building pcloud-rs $VERSION for macOS"
ARCH="$(uname -m)"

PKG_OUT="$ROOT_DIR/target/pkg"
STAGE="$PKG_OUT/stage"
COMPONENT_PKG="$PKG_OUT/pcloud-rs-component.pkg"
rm -rf "$STAGE"
rm -f "$COMPONENT_PKG"
mkdir -p "$STAGE/usr/local/bin"
mkdir -p "$STAGE/usr/local/share/pcloud-rs/macos"
mkdir -p "$PKG_OUT"

# ── Build release binaries ────────────────────────────────────────────────────
echo "Building release binaries..."
cargo build --release --locked -p pcloud-cli -p pcloud-daemon

# ── Stage binaries ────────────────────────────────────────────────────────────
cp "$ROOT_DIR/target/release/pcloudc" "$STAGE/usr/local/bin/pcloudc"
cp "$ROOT_DIR/target/release/pcloudd" "$STAGE/usr/local/bin/pcloudd"

# A package installer does not have a reliable target GUI-user context.  Stage
# the per-user template and helper instead of installing a broken system-wide
# LaunchAgent containing unresolved home-directory placeholders.
cp "$SCRIPT_DIR/com.pcloud.pcloud-rs.plist" \
   "$STAGE/usr/local/share/pcloud-rs/macos/com.pcloud.pcloud-rs.plist"
cp "$SCRIPT_DIR/configure-user.sh" \
   "$STAGE/usr/local/share/pcloud-rs/macos/configure-user.sh"
chmod 755 "$STAGE/usr/local/share/pcloud-rs/macos/configure-user.sh"

# ── Sign binaries (optional) ──────────────────────────────────────────────────
if [[ -n "$APPLICATION_SIGN_ID" ]]; then
    echo "Signing binaries with $APPLICATION_SIGN_ID..."
    for bin in pcloudc pcloudd; do
        codesign --force --options runtime --timestamp \
            --entitlements "$SCRIPT_DIR/entitlements.plist" \
            --sign "$APPLICATION_SIGN_ID" \
            "$STAGE/usr/local/bin/$bin"
        codesign --verify --strict --verbose=2 "$STAGE/usr/local/bin/$bin"
    done
fi

# ── Build .pkg ────────────────────────────────────────────────────────────────
echo "Building component package..."
pkgbuild \
    --root "$STAGE" \
    --identifier "com.pcloud.pcloud-rs" \
    --version "$VERSION" \
    --install-location "/" \
    "$COMPONENT_PKG"

FINAL_PKG="$PKG_OUT/pcloud-rs-${VERSION}-macos-${ARCH}.pkg"
echo "Building product archive..."
productbuild \
    --package "$COMPONENT_PKG" \
    --identifier "com.pcloud.pcloud-rs" \
    --version "$VERSION" \
    "$FINAL_PKG"

# ── Sign the .pkg (optional) ──────────────────────────────────────────────────
if [[ -n "$INSTALLER_SIGN_ID" ]]; then
    echo "Signing .pkg..."
    productsign \
        --sign "$INSTALLER_SIGN_ID" \
        "$FINAL_PKG" \
        "${FINAL_PKG%.pkg}-signed.pkg"
    mv "${FINAL_PKG%.pkg}-signed.pkg" "$FINAL_PKG"
    pkgutil --check-signature "$FINAL_PKG"
fi

# ── Notarize (optional) ───────────────────────────────────────────────────────
if [[ "$DO_NOTARIZE" == "true" ]]; then
    echo "Notarizing..."
    bash "$ROOT_DIR/packaging/signing/notarize-macos.sh" "$FINAL_PKG"
fi

pkgutil --payload-files "$COMPONENT_PKG" | grep -q '^./usr/local/bin/pcloudd$'
pkgutil --payload-files "$COMPONENT_PKG" | grep -q '^./usr/local/bin/pcloudc$'

echo "Done: $FINAL_PKG"
echo "After installation, each user enables auto-start with:"
echo "  /usr/local/share/pcloud-rs/macos/configure-user.sh"
