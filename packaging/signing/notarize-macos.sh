#!/usr/bin/env bash
# PLATFORM: macOS only.
#
# Submit a previously-signed artifact to Apple's notary service, wait for
# the result, and staple the notarisation ticket into the artifact.
#
# Usage:
#   notarize-macos.sh <path-to-signed-artifact>
#
# Environment:
#   APPLE_ID                      Apple ID email.
#   APPLE_APP_SPECIFIC_PASSWORD   App-specific password (NOT the Apple ID password).
#   APPLE_TEAM_ID                 10-char Team ID.
#
# Only .pkg / .dmg / .zip are accepted by notarytool. Raw binaries must be
# wrapped in a zip first (use `ditto -c -k --keepParent`).

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "notarize-macos.sh must run on macOS (uname=$(uname -s))" >&2
  exit 1
fi

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <path-to-signed-artifact>" >&2
  exit 64
fi

TARGET="$1"

: "${APPLE_ID:?APPLE_ID not set}"
: "${APPLE_APP_SPECIFIC_PASSWORD:?APPLE_APP_SPECIFIC_PASSWORD not set}"
: "${APPLE_TEAM_ID:?APPLE_TEAM_ID not set}"

if [[ ! -f "$TARGET" ]]; then
  echo "notarize-macos.sh: target not found: $TARGET" >&2
  exit 66
fi

echo "[notarize-macos] submitting $TARGET"

xcrun notarytool submit "$TARGET" \
  --apple-id "$APPLE_ID" \
  --team-id "$APPLE_TEAM_ID" \
  --password "$APPLE_APP_SPECIFIC_PASSWORD" \
  --wait

case "$TARGET" in
  *.pkg|*.dmg|*.app)
    echo "[notarize-macos] stapling ticket"
    xcrun stapler staple "$TARGET"
    xcrun stapler validate "$TARGET"
    ;;
  *.zip)
    # zip archives cannot be stapled; consumers re-validate via Gatekeeper
    # on extraction. We still validate that the notarisation succeeded via
    # notarytool's exit code above.
    echo "[notarize-macos] zip artifact: stapling skipped (not supported)"
    ;;
  *)
    echo "[notarize-macos] WARNING: unknown artifact type, skipping staple" >&2
    ;;
esac

echo "[notarize-macos] done"
