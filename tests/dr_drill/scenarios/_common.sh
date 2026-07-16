# shellcheck shell=bash
# Common helpers for DR drill scenarios. Sourced by each scenario.
# Provides:
#   dr_setup     — create an isolated XDG sandbox under a temp dir,
#                  export the standard pcloud-rs env vars, and
#                  print a banner.
#   dr_teardown  — best-effort cleanup; trap'd by every scenario.
#   dr_pass      — emit `[PASS] <scenario>` and exit 0.
#   dr_fail      — emit `[FAIL] <scenario>: <reason>` and exit 1.
#   dr_skip      — emit `[SKIP] <scenario>: <reason>` and exit 77.
#                  Use ONLY when a referenced runbook procedure is
#                  not documented (per T4.2 plan constraints).
#
# This file MUST stay POSIX-bashy and not call cargo or pcloudd
# directly — scenarios decide whether to invoke the binaries.

dr_scenario_name="${dr_scenario_name:-unknown}"

dr_setup() {
    dr_root="$(mktemp -d -t pcloud-dr-XXXXXX)"
    export XDG_CONFIG_HOME="$dr_root/config"
    export XDG_DATA_HOME="$dr_root/data"
    export XDG_CACHE_HOME="$dr_root/cache"
    export XDG_RUNTIME_DIR="$dr_root/runtime"
    mkdir -p \
        "$XDG_CONFIG_HOME/pcloud-rs" \
        "$XDG_DATA_HOME/pcloud-rs" \
        "$XDG_CACHE_HOME/pcloud-rs" \
        "$XDG_RUNTIME_DIR/pcloud-rs"
    chmod 0700 "$XDG_DATA_HOME/pcloud-rs" "$XDG_RUNTIME_DIR/pcloud-rs"
    printf '[dr] scenario=%s sandbox=%s\n' "$dr_scenario_name" "$dr_root" >&2
}

dr_teardown() {
    if [ -n "${dr_root:-}" ] && [ -d "$dr_root" ]; then
        rm -rf "$dr_root"
    fi
}

dr_pass() {
    printf '[PASS] %s\n' "$dr_scenario_name"
    exit 0
}

dr_fail() {
    printf '[FAIL] %s: %s\n' "$dr_scenario_name" "${1:-unspecified}"
    exit 1
}

dr_skip() {
    printf '[SKIP] %s: %s\n' "$dr_scenario_name" "${1:-runbook gap}"
    exit 77
}

# Locate a built pcloudd / pcloud-cli binary. Search order:
#   1. $PCLOUD_BIN_DIR (CI sets this)
#   2. ./target/release/<name>
#   3. ./target/debug/<name>
#   4. PATH (`command -v`)
# Returns absolute path on stdout, or empty if not found.
dr_find_bin() {
    local name="$1" repo_root candidate
    repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
    if [ -n "${PCLOUD_BIN_DIR:-}" ] && [ -x "$PCLOUD_BIN_DIR/$name" ]; then
        printf '%s' "$PCLOUD_BIN_DIR/$name"
        return 0
    fi
    for candidate in \
        "$repo_root/target/release/$name" \
        "$repo_root/target/debug/$name"; do
        if [ -x "$candidate" ]; then
            printf '%s' "$candidate"
            return 0
        fi
    done
    if command -v "$name" >/dev/null 2>&1; then
        command -v "$name"
        return 0
    fi
    printf ''
    return 0
}
