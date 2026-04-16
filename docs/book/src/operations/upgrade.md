# Upgrade

## 1. Purpose

This chapter is the authoritative upgrade reference for a host or fleet
running `pcloud-daemon` / `pcloud-cli`. It exists so that an operator
executing an upgrade — whether a single workstation or a thousand-seat
rollout — has one place to look up:

- what a semver bump actually commits the daemon to (IPC protocol, store
  schema, vault format, config envelope),
- how to perform a **graceful drain** of an active daemon before
  restarting it under a new binary,
- the **2-wave rolling procedure** for fleets,
- the **zero-downtime two-daemon handoff** design (documented as a
  design note — not yet code-backed; see the honesty callout below),
- the supported migration path from the legacy C client via
  `pcloudc migrate-from-c --from <PATH> [--dry-run] [--force-overwrite]`,
- CLI-flag deprecation discipline,
- service-manager restart semantics for systemd / launchd / SCM / rc.d.

Related:

- [Deployment](./deployment.md) — fleet-wide rollout, mTLS agents,
  Prometheus + OTel.
- [Runbook Playbook 2 / 3](./runbook.md#playbook-2-upgrade-pinned---latest)
  — canonical single-host upgrade and rollback commands.
- [Packaging matrix](./packaging-matrix.md) — per-channel install paths
  and service-manager entries touched by an upgrade.

## 2. Prereqs

Before you start an upgrade you must have:

- a **frozen release version** with its sha256, release notes URL, and
  signature artefact (never upgrade to `latest`),
- the **current version fingerprint** recorded for rollback triage:

  ```bash
  pcloudc version --json
  pcloudc --json status > /var/log/pcloud-rs/pre-upgrade-status.json
  ```

- a **recent vault backup** (UID-bound; owner-only on mode `0600`):

  ```bash
  install -d -m 0700 /var/backups/pcloud-rs/$(date +%F)
  install -m 0600 ~/.config/pcloud-rs/auth_token* \
    /var/backups/pcloud-rs/$(date +%F)/
  ```

- a **recent store snapshot** (see
  [Backup snapshots](./backup-snapshots.md); GPG is mandatory),
- privilege to stop/start the service manager unit on the host
  (systemd user/system, launchd, SCM, or rc.d),
- for fleet upgrades: MDM/inventory tags (`pcloud-rs_wave=<canary|A|B>`)
  already assigned so reporting can distinguish cohorts.

## 3. Conceptual background

### Semver, literally

The Rust workspace publishes `pcloud-daemon`, `pcloud-cli`, and the
`pcloud-sdk` surface under a **single coordinated version**. Treat the
version as a *tuple* of fingerprints, not a single integer.

- **MAJOR (`X.y.z`)** — backwards-incompatible change in any of:
  - the IPC wire protocol between `pcloud-cli` and `pcloud-daemon`,
  - the `pcloud-sdk` public Rust API,
  - the on-disk store schema (forward migration exists but older
    daemons cannot open a newer store),
  - the **vault file format**,
  - the config envelope schema beyond what the loader transparently
    migrates.

  MAJOR upgrades are **not drop-in**. Read the release notes.

- **MINOR (`x.Y.z`)** — backwards-compatible additions:
  - new CLI subcommands / flags,
  - new SDK functions,
  - new optional config fields,
  - new IPC request variants older daemons simply do not send,
  - new forward-compatible store migrations.

  MINOR upgrades are drop-in within a major series. Newer daemon
  accepts requests from an older CLI; newer CLI sending a new request
  variant to an older daemon receives a clean `UnsupportedRequest`
  error rather than a crash.

- **PATCH (`x.y.Z`)** — bug, documentation, internal refactor, security
  update. No visible behavior changes.

### State surfaces that migrate on an upgrade

| Surface            | Migration owner                              | Forward-compat within major? | Backward-compat across major? |
|--------------------|----------------------------------------------|------------------------------|-------------------------------|
| Config envelope    | `pcloud-config` loader (in-memory upgrade)   | yes (`v0` → current)         | **documented by release note** |
| Store schema       | `pcloud-store` migrations                    | yes                          | one-way                        |
| Vault format       | `pcloud-daemon/src/auth_vault.rs`            | yes                          | requires explicit procedure    |
| Plugin registry    | `pcloud-plugin-api`                          | yes                          | signatures re-validated        |
| Audit chain        | `pcloud-observability`                       | yes (append-only)            | re-verifies tail hash          |

### CLI flag deprecation policy

Deprecated flags print a `warning:` line to stderr for **one full minor
cycle** before removal. A flag removed in `X.Y.0` was emitting the
warning since `X.(Y-1).0`. Machine parsers should key off exit code
(stable) plus `--json` output (stable per-major), never stderr text.

### Service-manager restart semantics

| Platform  | Supervisor | Restart command                                              | Signal / guarantees                                                                              |
|-----------|------------|--------------------------------------------------------------|--------------------------------------------------------------------------------------------------|
| Linux     | systemd    | `systemctl --user restart pcloud-rs-daemon` (or system)       | `SIGTERM` → `TimeoutStopSec` (default 90s) → `SIGKILL`. Daemon drains on `SIGTERM`.              |
| macOS     | launchd    | `launchctl kickstart -k gui/$(id -u)/com.pcloud.pcloudd`     | `SIGTERM` + grace; re-exec under the new LaunchAgent plist.                                     |
| Windows   | SCM        | `sc stop pcloudd && sc start pcloudd`                        | SCM `SERVICE_CONTROL_STOP` → daemon acknowledges within `ServiceStopTimeoutMs` (default 30s).    |
| FreeBSD   | rc.d       | `service pcloudd restart`                                    | `SIGTERM` then `SIGKILL` after `daemon_stop_wait` (default 30s).                                 |
| OpenBSD   | rc.d       | `rcctl restart pcloudd`                                      | `SIGTERM`, 30 s grace, `SIGKILL`.                                                                |
| NetBSD    | rc.d       | `/etc/rc.d/pcloudd restart`                                  | Same semantics as FreeBSD.                                                                       |

### Graceful drain

The daemon treats `SIGTERM` (or the platform equivalent) as a **drain
request**:

1. transition the drain state machine from `Running` → `Draining` and
   stamp `drain_started_at`,
2. reject new non-status IPC requests with
   `ResponseStatus::Unavailable("daemon draining, retry")`; the
   explicitly-admitted methods (`DrainStatus`, `Shutdown`, `GetHealth`,
   `Health`) continue to answer so operators and supervisors can poll
   progress without racing the socket,
3. wait for in-flight requests to complete, up to
   `[upgrade].drain_timeout_secs` (default 30 s),
4. flush in-flight upload sidecars (H5) and fsync the staging
   directory (see [partial transfers](./partial-transfers.md)),
5. close the page cache, commit pending store writes through the SQLite
   online-backup API, flush the audit index,
6. release mount handles via RAII (`pcloud-fs` unmount; the FUSE
   runtime's signal-aware cleanup also kicks in on SIGINT/SIGHUP),
7. release the auth-vault and store locks, remove the pidfile, unbind
   the socket, and exit `0`.

A second `SIGTERM` during drain promotes it to **force stop**: the
drain deadline is treated as already expired, in-flight IPC requests
receive `Unavailable` on their next syscall, and the daemon exits as
soon as on-disk state is consistent.

#### Operator drain recipe

The `pcloudc drain` subcommand encapsulates the full recipe. It reads
the pidfile at `<state_dir>/daemon.pid`, dispatches `SIGTERM`, and
polls `Method::DrainStatus` every 500 ms until the daemon reports
`state == "stopped"` or `[upgrade].handoff_timeout_secs` (default 30 s)
expires.

```bash
# Supervised drain with default timeouts
pcloudc drain

# JSON output — the final line is the last DrainStatus payload
pcloudc --json drain

# Tighten the poll window for fast test harnesses
PCLOUD_ROOT=/run/pcloud-ci pcloudc drain
```

The returned JSON payload conforms to `DrainStatusPayload`:

```json
{"state": "draining", "in_flight": 2, "elapsed_drain_ms": 4128}
```

Exit codes:

| Exit | Meaning                                            |
|------|----------------------------------------------------|
| 0    | daemon reported `state == "stopped"`               |
| 6    | drain timed out (`Unavailable`); check daemon log  |
| 1    | pidfile missing / unreadable / `kill(2)` failed    |

For service managers that own the signal themselves
(`systemctl --user stop pcloud-rs-daemon`), `pcloudc drain` is not
required — the daemon performs the same drain on SIGTERM regardless of
who dispatched it. Operators drive `pcloudc drain` when they want the
exit code to reflect drain success rather than the supervisor's
"command accepted" return.

#### Config

The `[upgrade]` section (all fields optional; defaults shown):

```toml
[upgrade]
# Seconds a new daemon instance waits for the previous daemon's lease
# and socket to release during a rolling upgrade.
handoff_timeout_secs = 30

# Seconds the serve loop waits for in-flight requests to complete
# after SIGTERM before forcing a shutdown.
drain_timeout_secs = 30
```

Both knobs accept 0–600. Values above 600 are capped to 600 by the
config loader. Setting either to 0 means "exit as soon as no
dispatch is executing" — useful in test harnesses, unsafe in
production because an in-flight upload finalise can take several
seconds.

### Daemon-upgrade handoff protocol

The shipped handoff is deliberately simpler than full socket-activation
fd-passing. It leverages the Tier-2 HA lease
([`pcloud-daemon::ha_lease`]) and the drain state machine above:

1. the operator installs the new binary in-place (via the platform
   package manager) or under a versioned path, then invokes
   `pcloudc drain` against the running daemon,
2. `pcloudc drain` polls `Method::DrainStatus` until the old daemon
   reports `state == "stopped"`, which only happens after the vault
   and store locks are released and the socket is unbound,
3. the new daemon is started (by the service manager or manually). On
   bootstrap it tries to acquire the HA lease; if the lease is still
   held it waits up to `[upgrade].handoff_timeout_secs` for the old
   daemon to exit,
4. once the lease is free, the new daemon binds the socket and begins
   serving. Operators poll `pcloudc status` or the systemd unit to
   confirm.

This is **code-backed today** — both ends of the handshake are
exercised by `tests/graceful_drain.rs` and the lease-polling logic in
`crates/pcloud-daemon/src/ha_lease.rs`. What is still deliberately
missing is socket-activation fd-passing across the handoff: the new
daemon creates a fresh `UnixListener`, so clients connected
mid-handoff observe a brief reconnect window. The `pcloudc drain`
documentation in §4 reflects that reality rather than hiding it.

## 4. Step-by-step procedure

This is the **mandatory** procedure for any MINOR or PATCH upgrade
across more than ~10 seats. For MAJOR upgrades, pair it with the fleet
procedure in [Deployment](./deployment.md) and the release-note
migration steps.

### 4.1 Wave plan

| Wave        | Fraction | Hold time | Purpose                                  |
|-------------|----------|-----------|------------------------------------------|
| Canary      | 1–2%     | 48 hours  | Detect regressions before broad rollout  |
| Wave A      | 20%      | 48 hours  | Surface issues at moderate scale         |
| Wave B      | 78%      | 72 hours  | Complete the rollout                     |

Canaries must include ≥1 host per OS/arch/init-system/FUSE-runtime
combination in the fleet.

### 4.2 Per-host procedure

```bash
# 1. Pre-flight capture (on-host, automated)
pcloudc --version > /var/log/pcloud-rs/pre-upgrade-version.txt
pcloudc --json status > /var/log/pcloud-rs/pre-upgrade-status.json
sha256sum "$(command -v pcloud-daemon)" \
  > /var/log/pcloud-rs/pre-upgrade-daemon.sha

# 2. Vault snapshot (UID-bound, 0600 storage)
install -d -m 0700 /var/backups/pcloud-rs/$(date +%F)
install -m 0600 ~/.config/pcloud-rs/auth_token* \
  /var/backups/pcloud-rs/$(date +%F)/

# 3. Graceful drain
#    Linux
systemctl --user stop pcloud-rs-daemon
#    macOS
launchctl bootout gui/$(id -u) \
  ~/Library/LaunchAgents/com.pcloud.pcloudd.plist
#    Windows
sc stop pcloudd
#    BSDs
service pcloudd stop

# 4. Verify signature before installing
sha256sum -c SHA256SUMS.txt
cosign verify-blob --key release.pub \
  --signature pcloud-daemon.sig pcloud-daemon

# 5. Install the binary via the platform-native package manager.
#    Do NOT mix package sources across an upgrade.

# 6. Start the daemon
#    Linux
systemctl --user start pcloud-rs-daemon
#    macOS
launchctl bootstrap gui/$(id -u) \
  ~/Library/LaunchAgents/com.pcloud.pcloudd.plist
#    Windows
sc start pcloudd
#    BSDs
service pcloudd start
```

### 4.3 Expected output selectors

Post-restart the daemon must produce the following; parse with native
JSON selectors (`jq`, `ConvertFrom-Json`, `json_reformat`, etc.):

```bash
pcloudc --json status | jq '.auth.state, .sync.root_count, .mount.state'
# "Authenticated" 3 "active"

pcloudc doctor --json | jq '.checks[] | select(.level=="error")'
# (empty)

pcloudc version --json | jq '{daemon,ipc_protocol,store_schema,vault_format}'
# { "daemon":"X.Y.Z", "ipc_protocol":"N",
#   "store_schema":"M","vault_format":"v1" }
```

The human-readable equivalents for ad-hoc checks:

```bash
pcloudc status          # inline auth=, sync=, crypto=, engine summary
pcloudc status auth     # selector-extracted view
pcloudc doctor          # full health bundle
```

## 5. Verification

A host is **only** considered upgraded if all of the following are true:

1. `pcloudc --version` reports the pinned version exactly,
2. `pcloudc --json status` matches the pre-upgrade JSON on:
   - `auth.state == "Authenticated"`,
   - `sync.root_count`,
   - `mount.state`,
3. `pcloudc doctor --json` reports zero `level == "error"` checks,
4. `curl --unix-socket $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock
   http://localhost/health` returns HTTP 200,
5. `curl --unix-socket $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock
   http://localhost/slo` shows the post-restart counters resetting
   cleanly.

Diff the raw JSON snapshots:

```bash
diff /var/log/pcloud-rs/pre-upgrade-status.json \
     /var/log/pcloud-rs/post-upgrade-status.json
```

Only the `daemon.uptime_secs` and `version` fields should differ.

### Wave gate criteria

Hold the wave (do not advance) if during the hold window any of:

- `daemon.stopped` with non-zero exit > 0.5% of upgraded seats,
- `auth.login.failed` rate > 2× pre-upgrade baseline,
- `journal.entry.quarantined` on > 1% of upgraded seats,
- any `ipc.peer.rejected` not classified as a known false positive,
- mount EIO rate > pre-upgrade baseline.

A held wave triggers a cohort rollback, a bead filing, and a
post-incident review before the next attempt. **Do not paper over a
held wave by adjusting thresholds.**

## 6. Rollback

Use [runbook Playbook 3](./runbook.md#playbook-3-rollback) when any of:

- SEV-1/SEV-2 incident declared,
- store fails to open on > 0.1% of upgraded seats (suspected schema
  regression),
- vault ownership/mode validation failure on any host not explained by
  a known UID/hostname change,
- confirmed secret leak (see
  [log analysis](./runbook.md#log-analysis-guide)).

Rollback procedure summary:

```bash
# 1. Drain new daemon
systemctl --user stop pcloud-rs-daemon    # or platform equivalent

# 2. Restore old binary from the previous package
<platform-package-manager> install pcloud-daemon=<previous-version>
sha256sum -c SHA256SUMS.prev.txt

# 3. Restore the vault if and only if it was touched
install -m 0600 /var/backups/pcloud-rs/<date>/auth_token* \
  ~/.config/pcloud-rs/

# 4. Restart old daemon
systemctl --user start pcloud-rs-daemon

# 5. Verify
pcloudc version --json
pcloudc doctor --json | jq '.checks[] | select(.level=="error")'
```

Store rollback across a MAJOR line is **not supported**; restore from
a pre-upgrade snapshot (see [Backup snapshots](./backup-snapshots.md)).

## 7. Tradeoffs / tuning

| Knob                          | Default | Tradeoff                                                                   |
|-------------------------------|---------|----------------------------------------------------------------------------|
| Canary fraction               | 1–2%    | Larger → faster rollouts, more blast radius if bad.                        |
| Canary hold                   | 48 h    | Shorter hold misses diurnal-pattern regressions (backup windows).          |
| Wave A fraction               | 20%     | Larger wave A accelerates delivery but leaves less triage headroom.        |
| `TimeoutStopSec` (systemd)    | 90 s    | Shorter means quicker upgrades but risks killing drain mid-flush.          |
| Graceful drain retry          | 2       | More retries delay rollouts; fewer risk mounting under uncommitted state.  |
| Structured log level (canary) | `debug` | `debug` produces verbose logs; revert to `info` after the hold period.    |

## 8. Common failure modes

1. **Vault ownership/mode check fails after restart.**
   - Symptom: `doctor` reports `vault.mode_invalid` or
     `vault.owner_mismatch`; daemon refuses to start.
   - Cause: upgrade ran as a different UID (frequent under Ansible
     `become_user`) or mode drifted to `0644`.
   - Fix: `chown $(id -u):$(id -g) ~/.config/pcloud-rs/auth_token*` and
     `chmod 0600`. Restart. Do not disable the check.

2. **Store migration fails on open (MAJOR upgrade).**
   - Symptom: `daemon.started` never fires; `journalctl` shows
     `store.migration.failed`.
   - Cause: missing migration step or pre-existing corruption.
   - Fix: restore the last verified snapshot
     (`pcloudc backup snapshot-verify <path>` first, then
     `pcloudc backup snapshot-restore <path> --yes`), then re-attempt
     the upgrade.

3. **`upload_status` probe failures after restart.**
   - Symptom: H5 replay emits `BackendError` for every sidecar.
   - Cause: network unreachable at daemon boot; auth not yet
     re-established.
   - Fix: non-fatal; rescheduled on next replay tick. If persistent,
     check `pcloudc doctor --json | jq '.checks[] | select(.id=="net")'`.

4. **Service-manager kill before drain completes.**
   - Symptom: exit code 143 (SIGTERM + grace exceeded) followed by
     `SIGKILL`; next boot shows `Unparseable` sidecars.
   - Cause: `TimeoutStopSec` too short for the workload.
   - Fix: raise `TimeoutStopSec=180s` in the unit override; keep under
     your MDM's job-kill window.

5. **Migrate-from-C refuses to proceed (`--force-overwrite` guard).**
   - Symptom: `migrate-from-c` exits non-zero with "Rust state
     directory is not empty".
   - Cause: a previous `.pclouddb` (store) exists under the Rust data
     directory and the guard is active.
   - Fix: take a vault + store backup, then re-run with
     `pcloudc migrate-from-c --from <PATH> --force-overwrite`. Only
     do this after `--dry-run` confirms the plan.

## 9. Security / compliance notes

- **Release-key rotation.** Every upgrade cycle is a rehearsal for
  rotation. If you cannot verify the signature on the new binary
  against the pinned release key, abort. Do not disable
  `cosign verify-blob`.
- **Vault never travels off-host in cleartext.** If your MDM pushes
  state it must push ciphertext only. `auth_vault.bin` is already
  encrypted at rest but loses ownership semantics when copied through
  a generic agent — always use the vault backup helpers.
- **Password persistence is NOT migrated.** `migrate-from-c`
  deliberately drops C-client-stored passwords. The Rust vault is
  UID-bound with ownership/mode validation; the C client did not
  enforce this. Users re-authenticate. This is by design.
- **Telemetry stays opt-in across upgrades.** A minor/patch bump MUST
  NOT flip telemetry to `enabled = true`. If a user opted out, their
  choice survives every upgrade. Surface the state in
  `pcloudc --json status`.
- **Transport policy is non-negotiable.** The `production` config
  profile rejects downgrade away from TLS. Do not carry forward
  `allow_plaintext = true` from an earlier staging test.
- **Audit events emitted during the drain** are chained into the same
  hash-linked audit log; a held wave’s audit must be retained for the
  length of your compliance regime.

## 10. Migrating from legacy C `pcloud-rs`

`pcloudc migrate-from-c` is shipped and **idempotent**. It has three
verified flags: `--from <PATH>`, `--dry-run`, `--force-overwrite`
(confirmed in
`crates/pcloud-cli/src/main.rs::run_migrate_from_c` and
`crates/pcloud-cli/src/app.rs`). There is no `--purge-legacy` flag —
any cleanup of the legacy installation is a separate, manual step the
operator performs after the Rust daemon is verified healthy.

### Three safeguards (enforced by the migrator)

1. **Refuse-overwrite by default.** If a Rust state directory already
   contains a `.pclouddb`, the migration exits non-zero. Pass
   `--force-overwrite` only after a vault + store backup.
2. **Copy, never move.** Legacy files under `--from <PATH>` are read
   and copied into the Rust state tree. The migrator never moves,
   renames, or deletes legacy files. Operators clean up the legacy
   installation manually after verifying the Rust daemon is healthy.
3. **Secret redaction in preview.** `--dry-run` routes through the
   daemon’s redaction filter; auth tokens, saved passwords, crypto
   hints, and any `SecretString`-typed field render as `<redacted>`.
   Preview output is safe to attach to bug reports.

### What it does

1. Discovers legacy state (SQLite DB, sync roots, crypto settings,
   cached auth tokens, active mount).
2. Validates preconditions (Rust daemon installed and startable; new
   config paths empty or absent; each sync root acceptable to the
   runtime; legacy C daemon not running).
3. Writes `~/.config/pcloud-rs/config.toml` under the `production`
   profile with TLS enforced; imports sync-root registrations over
   authenticated IPC (requires an unlocked session).
4. Does **not** copy: the C-client password store, the C-client auth
   token blob, a running C-daemon mount. Those are either
   re-established by the user or intentionally dropped.

### Procedure

```bash
# 1. Stop the legacy C daemon
pkill -TERM pcloud-rs || true

# 2. Unmount any legacy FUSE mount
fusermount3 -u -z /path/to/legacy/mount || true

# 3. Dry-run (no writes)
pcloudc migrate-from-c --from ~/.pcloud --dry-run

# 4. Execute
pcloudc migrate-from-c --from ~/.pcloud
#   add --force-overwrite *only* if the refuse-overwrite guard tripped
#   and you just took a backup.

# 5. Start the Rust daemon and authenticate
systemctl --user enable --now pcloud-rs-daemon
pcloudc login <user>

# 6. Verify
pcloudc doctor --json
pcloudc sync list
pcloudc status
```

### Limitations

- Password persistence **not** migrated.
- Crypto shell state **not** migrated — unlock on the Rust client with
  the existing crypto password; key material is fetched fresh.
- Telemetry preferences **not** migrated — telemetry is opt-in on the
  Rust path and starts disabled regardless of legacy state.

File a bead against `bd-1du.10` if you encounter a legacy state shape
`migrate-from-c` cannot translate.

## 11. Cross-references

- [Runbook](./runbook.md) — live playbooks (drain, rollback, IPC
  triage).
- [Deployment](./deployment.md) — fleet rollout, supply-chain gate,
  telemetry opt-in.
- [Backup snapshots](./backup-snapshots.md) — pre-upgrade snapshot +
  GFS retention.
- [Partial transfers](./partial-transfers.md) — H5 sidecar / H6 resume
  behavior during a daemon restart.
- [Packaging matrix](./packaging-matrix.md) — install paths and
  service-manager entries per channel.
- [CLI reference — `migrate-from-c`](../reference/cli.md).
- [Config reference — envelope migration](../reference/config.md).
