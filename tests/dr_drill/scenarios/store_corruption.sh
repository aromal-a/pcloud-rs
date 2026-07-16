#!/usr/bin/env bash
# DR drill: SQLite store corruption.
#
# Plan reference: CLAUDEREV/TIER-PROGRESS.md row T4.2.
#
# Documented runbook procedure (verified at fire 86):
#   OPERATIONS-RUNBOOK.md anchor "Store file corrupted (disaster
#   recovery)" — the daemon is documented to FAIL bootstrap cleanly
#   (no auto-delete, no auto-repair) when the SQLite store is
#   structurally damaged. Recovery is operator-driven: move the file
#   aside (`store.sqlite3.corrupt-<ts>`) and either restore from
#   backup or bootstrap from cold and re-add sync roots.
#
# Behavior in code:
#   `pcloud_store::bootstrap_profile` (`crates/pcloud-store/src/lib.rs:205`)
#   calls `Connection::open` then runs
#   `evaluate_connection_integrity` (`crates/pcloud-store/src/integrity.rs:31`)
#   which issues `PRAGMA quick_check`. A header-with-garbage layout
#   fails at `Connection::open`-time with "file is not a database";
#   a corrupt body fails the quick_check. Either failure surfaces as
#   `BootstrapError::Store` and the daemon refuses to bind the IPC
#   socket. There is intentionally no `pcloud-cli store repair`
#   command in this fork.
#
# This drill exercises the detection half of the documented
# procedure:
#   1. Fabricate a SQLite-header-with-garbage file at the store path.
#   2. Boot `pcloudd` and assert it (a) exits non-zero, (b) emits a
#      bootstrap-failure message that names the store / sqlite, and
#      (c) does NOT auto-delete or auto-replace the corrupt file
#      (the operator-move-aside policy).
#
# The recovery half (move-aside + re-bootstrap + re-login) requires
# live pCloud credentials to verify end-to-end and is covered by the
# `live-e2e` workflow, not the unattended DR drill.

set -euo pipefail

dr_scenario_name="store_corruption"
# shellcheck source=./_common.sh
. "$(dirname "${BASH_SOURCE[0]}")/_common.sh"
trap dr_teardown EXIT

dr_setup

# IMPORTANT: the production store filename is `store.sqlite3` (with
# a trailing `3`), not `store.sqlite`. Mirrors `state_dir.join(
# "store.sqlite3")` in `crates/pcloud-daemon/src/bootstrap.rs:839`.
# Using the wrong name lets bootstrap silently create a fresh empty
# store next to our fixture, which would render the drill a no-op.
store_path="$XDG_DATA_HOME/pcloud-rs/store.sqlite3"

# Fabricate a SQLite-header-with-garbage layout: the magic prefix is
# present so the file is recognised as a SQLite candidate, but the
# subsequent header pages are random bytes so `Connection::open` /
# `PRAGMA quick_check` rejects it.
printf 'SQLite format 3\x00' > "$store_path"
dd if=/dev/urandom of="$store_path" bs=1 count=1024 conv=notrunc \
    seek=16 status=none
chmod 0600 "$store_path"
[ -s "$store_path" ] || dr_fail "store.sqlite3 fixture not created"

# Snapshot the corrupt file content + size + sha so we can confirm
# the daemon left it alone (per the "do not delete the store" policy
# documented in OPERATIONS-RUNBOOK.md "Store migration failed").
fixture_sha_before="$(sha256sum "$store_path" | awk '{print $1}')"
fixture_size_before="$(wc -c < "$store_path")"

pcloudd_bin="$(dr_find_bin pcloudd)"
if [ -z "$pcloudd_bin" ]; then
    dr_skip "pcloudd binary not available; build the workspace and rerun (set PCLOUD_BIN_DIR or run from target/release/target/debug)."
fi

out_log="$dr_root/pcloudd.out"
set +e
"$pcloudd_bin" > "$out_log" 2>&1
rc=$?
set -e

# Half (a) — bootstrap must fail. Refusing to bind the IPC socket on
# a corrupt store is the documented behavior; auto-delete or
# auto-replace would be a disaster.
if [ "$rc" -eq 0 ]; then
    printf '[dr] pcloudd unexpectedly exited 0; output:\n' >&2
    sed -e 's/^/  /' "$out_log" >&2 || true
    dr_fail "pcloudd must refuse to start against a corrupt store"
fi

# Half (b) — error message must name the store / sqlite so an
# operator can route the alert to the right runbook anchor. The
# canonical string emitted by `BootstrapError::Store` wrapping
# `StoreError::Sqlite` is `store bootstrap failed: ...`.
if ! grep -Fq 'store bootstrap failed' "$out_log"; then
    printf '[dr] missing structured store-bootstrap-failure message; output:\n' >&2
    sed -e 's/^/  /' "$out_log" >&2 || true
    dr_fail "pcloudd error output must mention 'store bootstrap failed'"
fi

# Half (c) — the daemon must not have rewritten the corrupt file in
# any way (no auto-delete, no auto-replace, no truncate-and-rebuild).
# `chmod 0600` may be applied at `Connection::open` time before the
# error surfaces; we therefore check size + sha rather than mtime.
fixture_sha_after="$(sha256sum "$store_path" | awk '{print $1}')"
fixture_size_after="$(wc -c < "$store_path")"
if [ "$fixture_sha_before" != "$fixture_sha_after" ] \
   || [ "$fixture_size_before" != "$fixture_size_after" ]; then
    printf '[dr] store file content changed: size %s->%s sha %s->%s\n' \
        "$fixture_size_before" "$fixture_size_after" \
        "${fixture_sha_before:0:16}" "${fixture_sha_after:0:16}" >&2
    dr_fail "pcloudd must not modify a corrupt store on bootstrap failure"
fi

printf '[dr] %s: detection half PASS (bootstrap refused, file untouched); operator recovery (`mv aside` + restore-from-backup or cold rebootstrap) is documented at OPERATIONS-RUNBOOK.md "Store file corrupted (disaster recovery)".\n' \
    "$dr_scenario_name" >&2
dr_pass
