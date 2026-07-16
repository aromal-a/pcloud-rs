#!/usr/bin/env bash
# DR drill: auth-vault loss.
#
# Plan reference: CLAUDEREV/TIER-PROGRESS.md row T4.2.
#
# Documented runbook procedure (verified at fire 86):
#   OPERATIONS-RUNBOOK.md anchor "Auth vault file deleted (disaster
#   recovery)" — the daemon is documented to come up cleanly with
#   `auth=LoggedOut` when the vault file is missing. Recovery is
#   `pcloud-cli login <user>`, which seeds a fresh vault.
#
# Behavior in code (`crates/pcloud-daemon/src/vault/file.rs:82`):
#   `load_token` returns `Ok(None)` on `ErrorKind::NotFound`, so
#   bootstrap treats a missing vault as the cold no-token state. The
#   daemon DOES start; it does not refuse, and it does not silently
#   create a vault. Re-authentication is required to seed one.
#
# This drill exercises the detection half of the documented
# procedure:
#   1. Seed a fake vault file in an isolated XDG sandbox.
#   2. Delete it (the loss event).
#   3. Boot `pcloudd` (one-shot summary mode, no `serve`) against
#      the empty vault location and assert it (a) exits zero,
#      (b) reports `auth=LoggedOut` in the inline runtime summary.
#      Both are the documented cold-state behavior.
#   4. Confirm `vault.dat` is still absent on disk afterwards
#      (the daemon must NOT silently re-create it).
#
# The recovery half (`pcloud-cli login <user>`) needs live pCloud
# credentials and is covered by the `live-e2e` workflow, not the DR
# drill — see OPERATIONS-RUNBOOK.md anchor "Live E2E account setup".

set -euo pipefail

dr_scenario_name="vault_loss"
# shellcheck source=./_common.sh
. "$(dirname "${BASH_SOURCE[0]}")/_common.sh"
trap dr_teardown EXIT

dr_setup

vault_path="$XDG_DATA_HOME/pcloud-rs/vault.dat"

# Seed a fake "vault was previously present" marker so the loss event
# is observable.
printf 'fake-vault-marker\n' > "$vault_path"
chmod 0600 "$vault_path"
[ -f "$vault_path" ] || dr_fail "vault.dat fixture not created"

# THE LOSS EVENT.
rm -f "$vault_path"
[ ! -e "$vault_path" ] || dr_fail "vault.dat removal failed"

# Locate the daemon binary.
pcloudd_bin="$(dr_find_bin pcloudd)"
if [ -z "$pcloudd_bin" ]; then
    dr_skip "pcloudd binary not available; build the workspace and rerun (set PCLOUD_BIN_DIR or run from target/release/target/debug)."
fi

# Half (a) — daemon-comes-up-cold assertion. Run pcloudd in one-shot
# summary mode (no `serve`); it bootstraps the runtime, prints the
# inline summary, and exits 0 when bootstrap succeeds. The summary
# line must contain `auth=LoggedOut` since no vault was loaded.
out_log="$dr_root/pcloudd.out"
set +e
"$pcloudd_bin" > "$out_log" 2>&1
rc=$?
set -e

if [ "$rc" -ne 0 ]; then
    printf '[dr] pcloudd exited %d; output:\n' "$rc" >&2
    sed -e 's/^/  /' "$out_log" >&2 || true
    dr_fail "pcloudd should bootstrap cleanly with no vault but exited $rc"
fi

if ! grep -Fq 'auth=LoggedOut' "$out_log"; then
    printf '[dr] missing auth=LoggedOut; full output:\n' >&2
    sed -e 's/^/  /' "$out_log" >&2 || true
    dr_fail "pcloudd summary must report auth=LoggedOut after vault loss"
fi

# Half (b) — daemon must NOT silently re-create the vault file. The
# documented recovery is an explicit `pcloud-cli login`, not implicit
# vault provisioning at bootstrap.
if [ -e "$vault_path" ]; then
    dr_fail "vault.dat reappeared after pcloudd bootstrap (must require explicit login)"
fi

printf '[dr] %s: detection half PASS (auth=LoggedOut, no silent re-seed); recovery half (`pcloud-cli login`) requires live credentials per OPERATIONS-RUNBOOK.md "Auth vault file deleted (disaster recovery)".\n' \
    "$dr_scenario_name" >&2
dr_pass
