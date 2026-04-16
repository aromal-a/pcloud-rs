#!/usr/bin/env bash
# PLATFORM: macOS only.
#
# Sign a macOS binary, bundle, or installer payload with a Developer ID
# Application identity. Enables the hardened runtime and embeds a trusted
# timestamp so the artifact is eligible for notarisation.
#
# Usage:
#   sign-macos.sh <path-to-binary-or-app> <Developer ID Application identity>
#
# Example:
#   sign-macos.sh ./build/pcloud-rs.app \
#     "Developer ID Application: Acme Corp (ABCDE12345)"
#
# NOTE on --deep:
#   --deep recursively signs every nested bundle/executable. It is convenient
#   but dangerous because it will re-sign third-party frameworks that were
#   already signed correctly, potentially breaking their notarisation chain
#   or overwriting a more-restrictive embedded entitlement set.
#   Apple's official guidance (TN3161) says: sign nested code explicitly
#   from the inside out, NOT with --deep, for shipping builds. We use --deep
#   here for simplicity because the current artifact has no embedded
#   third-party frameworks; revisit before adding any.

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "sign-macos.sh must run on macOS (uname=$(uname -s))" >&2
  exit 1
fi

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 <path> <Developer ID Application identity>" >&2
  exit 64
fi

TARGET="$1"
IDENTITY="$2"

if [[ ! -e "$TARGET" ]]; then
  echo "sign-macos.sh: target not found: $TARGET" >&2
  exit 66
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ENTITLEMENTS="$REPO_ROOT/packaging/macos/entitlements.plist"

if [[ ! -f "$ENTITLEMENTS" ]]; then
  echo "sign-macos.sh: entitlements plist missing: $ENTITLEMENTS" >&2
  exit 66
fi

echo "[sign-macos] signing $TARGET with identity: $IDENTITY"

codesign --force --deep --options runtime --timestamp \
  --entitlements "$ENTITLEMENTS" \
  --sign "$IDENTITY" \
  "$TARGET"

echo "[sign-macos] verifying signature"
codesign --verify --deep --strict --verbose=2 "$TARGET"

echo "[sign-macos] gatekeeper assessment"
if ! spctl --assess --type execute --verbose=4 "$TARGET" 2>&1; then
  echo "[sign-macos] WARNING: spctl assessment failed (expected until notarisation)" >&2
fi

echo "[sign-macos] done"
