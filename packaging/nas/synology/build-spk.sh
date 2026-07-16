#!/bin/sh

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)
VERSION=""
ARCH=""
BUILD_NUMBER="0001"
PCLOUDD=""
PCLOUDC=""
OUTPUT_DIR="$ROOT_DIR/target/nas/synology"

usage() {
    echo "usage: $0 --version X.Y.Z --arch x86_64|armv8 --pcloudd PATH --pcloudc PATH [--build-number NNNN] [--output DIR]" >&2
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) VERSION=$2; shift 2 ;;
        --arch) ARCH=$2; shift 2 ;;
        --build-number) BUILD_NUMBER=$2; shift 2 ;;
        --pcloudd) PCLOUDD=$2; shift 2 ;;
        --pcloudc) PCLOUDC=$2; shift 2 ;;
        --output) OUTPUT_DIR=$2; shift 2 ;;
        *) usage; exit 64 ;;
    esac
done

case "$VERSION" in ''|*[!0-9.]*) usage; exit 64 ;; esac
case "$ARCH" in x86_64|armv8) ;; *) usage; exit 64 ;; esac
case "$BUILD_NUMBER" in ''|*[!0-9]*) usage; exit 64 ;; esac
[ -x "$PCLOUDD" ] && [ -x "$PCLOUDC" ] || { echo "prebuilt binaries must be executable" >&2; exit 66; }

STAGE="$OUTPUT_DIR/stage-$ARCH"
PAYLOAD="$STAGE/payload"
rm -rf "$STAGE"
mkdir -p "$PAYLOAD/bin" "$PAYLOAD/share/pcloud-rs" "$STAGE/scripts" "$STAGE/conf" "$OUTPUT_DIR"
install -m 755 "$PCLOUDD" "$PAYLOAD/bin/pcloudd"
install -m 755 "$PCLOUDC" "$PAYLOAD/bin/pcloudc"
install -m 755 "$SCRIPT_DIR/../common/pcloudd-supervisor.sh" "$PAYLOAD/share/pcloud-rs/pcloudd-supervisor.sh"
install -m 755 "$SCRIPT_DIR/scripts/start-stop-status" "$STAGE/scripts/start-stop-status"
install -m 755 "$SCRIPT_DIR/scripts/postinst" "$STAGE/scripts/postinst"
install -m 644 "$SCRIPT_DIR/conf/privilege" "$STAGE/conf/privilege"

SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-0}
tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
    -C "$PAYLOAD" -czf "$STAGE/package.tgz" .
cat > "$STAGE/INFO" <<EOF
package="pcloud-rs"
version="${VERSION}-${BUILD_NUMBER}"
os_min_ver="7.0-40000"
description="pCloud command-line client and durable sync daemon"
displayname="pcloud-rs"
arch="$ARCH"
maintainer="pcloud-rs contributors"
maintainer_url="https://github.com/ezechiel203/pcloud-rs"
ctl_stop="yes"
silent_install="yes"
silent_upgrade="yes"
beta="no"
EOF

SPK="$OUTPUT_DIR/pcloud-rs-${VERSION}-${BUILD_NUMBER}-${ARCH}.spk"
tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
    -C "$STAGE" -cf "$SPK" INFO package.tgz scripts conf
tar -tf "$SPK" | grep -qx 'INFO'
tar -tf "$SPK" | grep -qx 'package.tgz'
echo "$SPK"
