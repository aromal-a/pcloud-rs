#!/bin/sh

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TMP=${TMPDIR:-/tmp}/pcloud-nas-package-validation.$$
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
mkdir -p "$TMP"

for script in \
    "$SCRIPT_DIR/common/pcloudd-supervisor.sh" \
    "$SCRIPT_DIR/common/test-supervisor.sh" \
    "$SCRIPT_DIR/synology/build-spk.sh" \
    "$SCRIPT_DIR/synology/scripts/start-stop-status" \
    "$SCRIPT_DIR/synology/scripts/postinst" \
    "$SCRIPT_DIR/qnap/build-qpkg.sh" \
    "$SCRIPT_DIR/qnap/pcloud-rs.sh" \
    "$SCRIPT_DIR/asustor/build-apk.sh" \
    "$SCRIPT_DIR/asustor/start-stop.sh"; do
    sh -n "$script"
done

python3 -m json.tool "$SCRIPT_DIR/synology/conf/privilege" >/dev/null
sed -e 's/@VERSION@/0.1.0/g' -e 's/@ARCH@/x86-64/g' \
    "$SCRIPT_DIR/asustor/config.json.in" | python3 -m json.tool >/dev/null
python3 -c 'import struct,sys; p=open(sys.argv[1], "rb").read(24); assert p[:8] == b"\x89PNG\r\n\x1a\n" and struct.unpack(">II", p[16:24]) == (90, 90)' \
    "$SCRIPT_DIR/asustor/icon.png"
grep -q '^QPKG_NAME="pcloud-rs"$' "$SCRIPT_DIR/qnap/qpkg.cfg.in"
grep -q '^QPKG_SERVICE_PROGRAM="pcloud-rs.sh"$' "$SCRIPT_DIR/qnap/qpkg.cfg.in"

"$SCRIPT_DIR/common/test-supervisor.sh"
SPK=$(
    "$SCRIPT_DIR/synology/build-spk.sh" \
        --version 0.1.0 \
        --arch x86_64 \
        --pcloudd /bin/true \
        --pcloudc /bin/true \
        --output "$TMP/synology"
)
tar -xf "$SPK" -C "$TMP"
tar -tzf "$TMP/package.tgz" | grep -qx './bin/pcloudd'
tar -tzf "$TMP/package.tgz" | grep -qx './bin/pcloudc'
echo "NAS packaging validation passed"
