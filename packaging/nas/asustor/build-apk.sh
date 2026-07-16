#!/bin/sh

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)
VERSION=""
ARCH="x86-64"
PCLOUDD=""
PCLOUDC=""
ICON="$SCRIPT_DIR/icon.png"
OUTPUT_DIR="$ROOT_DIR/target/nas/asustor"
APKG_TOOL=${APKG_TOOL:-apkg-tool.py}

usage() {
    echo "usage: $0 --version X.Y.Z --arch x86-64|arm64 --pcloudd PATH --pcloudc PATH [--icon 90x90.png] [--output DIR]" >&2
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) VERSION=$2; shift 2 ;;
        --arch) ARCH=$2; shift 2 ;;
        --pcloudd) PCLOUDD=$2; shift 2 ;;
        --pcloudc) PCLOUDC=$2; shift 2 ;;
        --icon) ICON=$2; shift 2 ;;
        --output) OUTPUT_DIR=$2; shift 2 ;;
        *) usage; exit 64 ;;
    esac
done

case "$VERSION" in ''|*[!0-9.]*) usage; exit 64 ;; esac
case "$ARCH" in x86-64|arm64) ;; *) usage; exit 64 ;; esac
[ -x "$PCLOUDD" ] && [ -x "$PCLOUDC" ] || { echo "prebuilt binaries must be executable" >&2; exit 66; }
[ -f "$ICON" ] || { echo "a 90x90 PNG icon is required by the ADM 5 guide" >&2; exit 66; }
if command -v identify >/dev/null 2>&1; then
    [ "$(identify -format '%m %wx%h' "$ICON")" = "PNG 90x90" ] || {
        echo "--icon must be a 90x90 PNG" >&2
        exit 65
    }
fi
command -v "$APKG_TOOL" >/dev/null 2>&1 || { echo "ASUSTOR apkg-tool.py is required" >&2; exit 69; }
[ "$(id -u)" -eq 0 ] || {
    echo "ASUSTOR's official APKG 2.0 tool calls chown(2); run this builder through sudo" >&2
    exit 77
}

STAGE="$OUTPUT_DIR/pcloud-rs_${VERSION}_${ARCH}"
rm -rf "$STAGE"
mkdir -p "$STAGE/CONTROL" "$STAGE/bin" "$STAGE/share/pcloud-rs" "$OUTPUT_DIR"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@ARCH@/$ARCH/g" \
    "$SCRIPT_DIR/config.json.in" > "$STAGE/CONTROL/config.json"
install -m 755 "$SCRIPT_DIR/start-stop.sh" "$STAGE/CONTROL/start-stop.sh"
install -m 644 "$SCRIPT_DIR/description.txt" "$STAGE/CONTROL/description.txt"
install -m 644 "$ICON" "$STAGE/CONTROL/icon.png"
install -m 755 "$SCRIPT_DIR/../common/pcloudd-supervisor.sh" "$STAGE/share/pcloud-rs/pcloudd-supervisor.sh"
install -m 755 "$PCLOUDD" "$STAGE/bin/pcloudd"
install -m 755 "$PCLOUDC" "$STAGE/bin/pcloudc"

"$APKG_TOOL" create "$STAGE" --destination "$OUTPUT_DIR"
APK=$(find "$OUTPUT_DIR" -maxdepth 1 -name "*${VERSION}*${ARCH}*.apk" -print | head -n 1)
[ -n "$APK" ] || { echo "apkg-tool.py produced no APK" >&2; exit 1; }
echo "$APK"
