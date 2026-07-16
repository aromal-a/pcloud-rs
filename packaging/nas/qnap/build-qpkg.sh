#!/bin/sh

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)
VERSION=""
ARCH=""
PCLOUDD=""
PCLOUDC=""
OUTPUT_DIR="$ROOT_DIR/target/nas/qnap"
QBUILD=${QBUILD:-qbuild}

usage() {
    echo "usage: $0 --version X.Y.Z --arch x86_64|arm_64 --pcloudd PATH --pcloudc PATH [--output DIR]" >&2
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) VERSION=$2; shift 2 ;;
        --arch) ARCH=$2; shift 2 ;;
        --pcloudd) PCLOUDD=$2; shift 2 ;;
        --pcloudc) PCLOUDC=$2; shift 2 ;;
        --output) OUTPUT_DIR=$2; shift 2 ;;
        *) usage; exit 64 ;;
    esac
done

case "$VERSION" in ''|*[!0-9.]*) usage; exit 64 ;; esac
[ "${#VERSION}" -le 10 ] || { echo "QPKG versions are limited to 10 characters" >&2; exit 64; }
case "$ARCH" in x86_64|arm_64) ;; *) usage; exit 64 ;; esac
[ -x "$PCLOUDD" ] && [ -x "$PCLOUDC" ] || { echo "prebuilt binaries must be executable" >&2; exit 66; }
command -v "$QBUILD" >/dev/null 2>&1 || { echo "QDK qbuild is required; install QDK 2.5.3 or newer" >&2; exit 69; }

STAGE="$OUTPUT_DIR/stage-$ARCH"
rm -rf "$STAGE"
mkdir -p "$STAGE/shared" "$STAGE/$ARCH/bin" "$STAGE/build" "$OUTPUT_DIR"
sed "s/@VERSION@/$VERSION/g" "$SCRIPT_DIR/qpkg.cfg.in" > "$STAGE/qpkg.cfg"
install -m 755 "$SCRIPT_DIR/pcloud-rs.sh" "$STAGE/shared/pcloud-rs.sh"
install -m 755 "$SCRIPT_DIR/../common/pcloudd-supervisor.sh" "$STAGE/shared/pcloudd-supervisor.sh"
install -m 755 "$PCLOUDD" "$STAGE/$ARCH/bin/pcloudd"
install -m 755 "$PCLOUDC" "$STAGE/$ARCH/bin/pcloudc"
: > "$STAGE/package_routines"

(cd "$STAGE" && QDK_BUILD_ARCH="$ARCH" QDK_BUILD_VERSION="$VERSION" "$QBUILD")
QPKG=$(find "$STAGE/build" -maxdepth 1 -name '*.qpkg' -print | head -n 1)
[ -n "$QPKG" ] || { echo "qbuild produced no QPKG" >&2; exit 1; }
FINAL="$OUTPUT_DIR/pcloud-rs-${VERSION}-${ARCH}.qpkg"
cp "$QPKG" "$FINAL"
echo "$FINAL"
