#!/bin/sh
# Shared NAS process supervisor. Vendor lifecycle scripts provide the paths.

set -eu

: "${PCLOUD_PACKAGE_DIR:?PCLOUD_PACKAGE_DIR is required}"
: "${PCLOUD_DATA_DIR:?PCLOUD_DATA_DIR is required}"

PCLOUD_RUN_DIR="${PCLOUD_RUN_DIR:-$PCLOUD_DATA_DIR/run}"
PCLOUD_LOG_DIR="${PCLOUD_LOG_DIR:-$PCLOUD_DATA_DIR/log}"
PCLOUD_DAEMON="${PCLOUD_DAEMON:-$PCLOUD_PACKAGE_DIR/bin/pcloudd}"
PCLOUD_CLI="${PCLOUD_CLI:-$PCLOUD_PACKAGE_DIR/bin/pcloudc}"
PID_FILE="${PCLOUD_PID_FILE:-$PCLOUD_RUN_DIR/pcloudd-manager.pid}"
LOCK_DIR="$PCLOUD_RUN_DIR/pcloudd-manager.lock"
LOG_FILE="${PCLOUD_LOG_FILE:-$PCLOUD_LOG_DIR/pcloudd.log}"

export PCLOUD_ROOT="${PCLOUD_ROOT:-$PCLOUD_DATA_DIR/root}"
export PCLOUD_ENV="${PCLOUD_ENV:-prod}"
export PCLOUD_LOG_LEVEL="${PCLOUD_LOG_LEVEL:-info}"
export PCLOUD_DURABLE_AUTH_TOKENS="${PCLOUD_DURABLE_AUTH_TOKENS:-1}"
# Headless NAS appliances do not normally provide a desktop Secret Service.
export PCLOUD_VAULT="${PCLOUD_VAULT:-file}"

umask 077
mkdir -p "$PCLOUD_RUN_DIR" "$PCLOUD_LOG_DIR" "$PCLOUD_ROOT"

read_pid() {
    [ -f "$PID_FILE" ] || return 1
    pid=$(sed -n '1p' "$PID_FILE")
    case "$pid" in
        ''|*[!0-9]*) return 1 ;;
    esac
    printf '%s\n' "$pid"
}

is_pcloudd_pid() {
    candidate="$1"
    kill -0 "$candidate" 2>/dev/null || return 1
    if [ -L "/proc/$candidate/exe" ]; then
        executable=$(readlink "/proc/$candidate/exe" 2>/dev/null || true)
        case "$executable" in
            */pcloudd|*/pcloudd\ \(deleted\)) return 0 ;;
            *) ;;
        esac
    fi
    if [ -r "/proc/$candidate/cmdline" ] &&
       tr '\000' '\n' < "/proc/$candidate/cmdline" | grep -Fqx "$PCLOUD_DAEMON"; then
        return 0
    fi
    # All supported NAS targets are Linux and expose procfs. Refuse to signal
    # an unverified PID if a vendor image unexpectedly hides /proc.
    return 1
}

running_pid() {
    candidate=$(read_pid) || return 1
    is_pcloudd_pid "$candidate" || return 1
    printf '%s\n' "$candidate"
}

acquire_lock() {
    if ! mkdir "$LOCK_DIR" 2>/dev/null; then
        echo "pcloudd lifecycle operation is already in progress" >&2
        return 1
    fi
    trap 'rmdir "$LOCK_DIR" 2>/dev/null || true' EXIT HUP INT TERM
}

release_lock() {
    rmdir "$LOCK_DIR" 2>/dev/null || true
    trap - EXIT HUP INT TERM
}

start_daemon() {
    acquire_lock
    if pid=$(running_pid); then
        echo "pcloudd is already running (pid $pid)"
        release_lock
        return 0
    fi
    rm -f "$PID_FILE"
    if [ ! -x "$PCLOUD_DAEMON" ] || [ ! -x "$PCLOUD_CLI" ]; then
        echo "pcloudd or pcloudc is missing from $PCLOUD_PACKAGE_DIR/bin" >&2
        release_lock
        return 1
    fi

    echo "starting pcloudd; log: $LOG_FILE"
    nohup "$PCLOUD_DAEMON" serve >>"$LOG_FILE" 2>&1 </dev/null &
    pid=$!
    printf '%s\n' "$pid" > "$PID_FILE"
    chmod 600 "$PID_FILE"

    attempts=0
    while [ "$attempts" -lt 5 ]; do
        if is_pcloudd_pid "$pid"; then
            echo "pcloudd started (pid $pid)"
            release_lock
            return 0
        fi
        attempts=$((attempts + 1))
        sleep 1
    done
    rm -f "$PID_FILE"
    echo "pcloudd failed to start; inspect $LOG_FILE" >&2
    release_lock
    return 1
}

wait_for_exit() {
    wait_pid="$1"
    wait_seconds="$2"
    elapsed=0
    while [ "$elapsed" -lt "$wait_seconds" ]; do
        is_pcloudd_pid "$wait_pid" || return 0
        sleep 1
        elapsed=$((elapsed + 1))
    done
    return 1
}

stop_daemon() {
    acquire_lock
    if ! pid=$(running_pid); then
        rm -f "$PID_FILE"
        echo "pcloudd is not running"
        release_lock
        return 0
    fi

    echo "requesting a durable daemon drain (pid $pid)"
    if ! "$PCLOUD_CLI" stop >>"$LOG_FILE" 2>&1; then
        echo "pcloudc stop failed; falling back to SIGTERM" >&2
    fi
    if ! wait_for_exit "$pid" 30; then
        echo "drain timed out after 30 seconds; sending SIGTERM" >&2
        kill -TERM "$pid" 2>/dev/null || true
    fi
    if ! wait_for_exit "$pid" 15; then
        echo "pcloudd ignored SIGTERM; sending SIGKILL" >&2
        kill -KILL "$pid" 2>/dev/null || true
        wait_for_exit "$pid" 5 || {
            echo "unable to stop pcloudd pid $pid" >&2
            release_lock
            return 1
        }
    fi
    rm -f "$PID_FILE"
    echo "pcloudd stopped"
    release_lock
}

status_daemon() {
    if pid=$(running_pid); then
        echo "pcloudd is running (pid $pid)"
        return 0
    fi
    [ ! -f "$PID_FILE" ] || echo "pcloudd is stopped (stale pid file)" >&2
    return 3
}

case "${1:-}" in
    start) start_daemon ;;
    stop) stop_daemon ;;
    restart)
        stop_daemon
        start_daemon
        ;;
    status) status_daemon ;;
    *)
        echo "usage: $0 {start|stop|restart|status}" >&2
        exit 64
        ;;
esac
