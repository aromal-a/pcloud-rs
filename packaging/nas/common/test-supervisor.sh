#!/bin/sh

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TMP=${TMPDIR:-/tmp}/pcloud-nas-supervisor-test.$$
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
mkdir -p "$TMP/package/bin" "$TMP/data"

cp "$SCRIPT_DIR/pcloudd-supervisor.sh" "$TMP/package/supervisor.sh"
cat > "$TMP/package/bin/pcloudd" <<'EOF'
#!/bin/sh
trap 'exit 0' TERM INT
printf '%s\n' "${PCLOUD_ROOT:?}" > "${PCLOUD_DATA_DIR:?}/observed-root"
while :; do sleep 1; done
EOF
cat > "$TMP/package/bin/pcloudc" <<'EOF'
#!/bin/sh
kill -TERM "$(cat "$PCLOUD_DATA_DIR/run/pcloudd-manager.pid")"
EOF
chmod 755 "$TMP/package/supervisor.sh" "$TMP/package/bin/pcloudd" "$TMP/package/bin/pcloudc"

export PCLOUD_PACKAGE_DIR="$TMP/package"
export PCLOUD_DATA_DIR="$TMP/data"
export PCLOUD_DAEMON="$TMP/package/bin/pcloudd"
export PCLOUD_CLI="$TMP/package/bin/pcloudc"

"$TMP/package/supervisor.sh" start
"$TMP/package/supervisor.sh" status
test "$(cat "$TMP/data/observed-root")" = "$TMP/data/root"
"$TMP/package/supervisor.sh" stop
if "$TMP/package/supervisor.sh" status 2>/dev/null; then
    echo "status unexpectedly reported running after stop" >&2
    exit 1
else
    test "$?" -eq 3
fi

echo "NAS supervisor test passed"
