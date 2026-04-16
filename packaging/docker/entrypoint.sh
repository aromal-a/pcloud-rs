#!/usr/bin/env bash
# PLATFORM: Linux (container runtime)
#
# pcloud-rs container entrypoint.
#
# Behaviour:
#   - No arguments, or first argument starts with "-": run pcloudd with them.
#   - First argument is "pcloudd" or "pcloudc": exec that binary with the rest.
#   - First argument resolves to an executable on PATH: exec it directly.
#   - Otherwise, treat the whole argv as arguments to pcloudd.
#
# This matches the conventional Docker pattern where `docker run <image> bash`
# drops you into a shell, but the default command keeps the daemon running.

set -euo pipefail

# Ensure a writable runtime dir exists for the daemon's local IPC socket.
: "${XDG_RUNTIME_DIR:=/run/pcloud-rs}"
if [ ! -d "${XDG_RUNTIME_DIR}" ]; then
    mkdir -p "${XDG_RUNTIME_DIR}" || true
    chmod 0700 "${XDG_RUNTIME_DIR}" || true
fi
export XDG_RUNTIME_DIR

if [ "$#" -eq 0 ]; then
    exec /usr/local/bin/pcloudd
fi

case "$1" in
    pcloudd)
        shift
        exec /usr/local/bin/pcloudd "$@"
        ;;
    pcloudc)
        shift
        exec /usr/local/bin/pcloudc "$@"
        ;;
    -*)
        # Flag form: forward to pcloudd.
        exec /usr/local/bin/pcloudd "$@"
        ;;
    *)
        if command -v "$1" >/dev/null 2>&1; then
            exec "$@"
        fi
        exec /usr/local/bin/pcloudd "$@"
        ;;
esac
