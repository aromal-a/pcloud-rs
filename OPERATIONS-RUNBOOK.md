# OPERATIONS RUNBOOK

Operational guide for the Rust daemon (`pcloud-daemon`) and CLI
(`pcloud-cli`). This is not a claim of production readiness — the Rust
path still has open parity gaps (`bd-1du`, `bd-1du.4`, `bd-1du.10`; see
`STATUS.md` for the current `Partial` / `Missing` row counts).

## Startup

Build:

```bash
cd /path/to/pcloud-rs
cargo build --release -p pcloud-daemon -p pcloud-cli
```

Start the daemon (foreground, for debugging):

```bash
PCLOUD_ROOT=~/.config/pcloud-rs target/release/pcloudd serve
```

Start the daemon (background, production shape):

```bash
PCLOUD_ROOT=/etc/pcloud-rs PCLOUD_LOG_LEVEL=info \
  target/release/pcloudd serve &
```

Note: `pcloudd` does not accept `--config`, `--log-format`, or `--log-level` flags.
Configuration is via environment variables (`PCLOUD_ROOT`, `PCLOUD_ENV`,
`PCLOUD_API_MODE`, `PCLOUD_LOG_LEVEL`). See `pcloudd --help` for the full list.

On startup, the daemon will:

1. Load the config profile (`pcloud-config`).
2. Open the SQLite store; run pending migrations.
3. Open the auth vault if token persistence is enabled.
4. Bind the IPC socket at `$XDG_RUNTIME_DIR/pcloud-rs/daemon.sock`
   (mode `0600`).
5. Start backend runtimes and the dispatch loop.
6. Emit a `daemon.started` audit event.

Successful startup log line (json format):

```
{"level":"info","event":"daemon.started","socket":"/run/user/1000/pcloud-rs/daemon.sock"}
```

## Shutdown

Graceful:

```bash
pcloud-cli shutdown
# or
kill -TERM <pid>
```

The daemon handles `SIGTERM` / `SIGINT` via a cancellation token:

1. Stops accepting new IPC connections.
2. Drains in-flight requests (bounded timeout).
3. Flushes engine schedulers.
4. Commits open store transactions.
5. Zeroizes any resident `SecretBytes` (crypto lock).
6. Removes the IPC socket file.
7. Emits `daemon.stopped`.

Force stop (only when graceful fails):

```bash
kill -KILL <pid>
```

After a kill -9, the next startup will:

- Roll forward the journal (`pcloud-fs::journal`, `pcloud-store::tx`).
- Re-validate staged cache consistency (`pcloud-cache::staging`).
- Re-verify vault file ownership and mode.

## Health checks

CLI:

```bash
pcloud-cli status
```

Returns: auth state, sync roots (count + paused flag), pending transfers,
crypto shell state, mount state, last audit event, daemon uptime.

Readiness probe (script-friendly):

```bash
# Stable JSON envelope (status/message/exit_code) for scripts.
pcloud-cli --json status

# Or pull individual fields out of the inline status summary with
# the built-in selector (no external parser required):
pcloud-cli status auth sync crypto
```

A ready daemon reports `auth=Authenticated`, a healthy engine
summary, and no outstanding shutdown request. Readiness requires:

- store is open and migrations current,
- IPC socket bound,
- at minimum one successful auth heartbeat if credentials are persisted.

## Common failures and remediation

### IPC socket already in use

Symptom: `bind: Address already in use` on startup.

Cause: stale socket from an unclean shutdown.

Fix:

```bash
ls -l $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock
# If no pcloud-daemon process, remove it:
rm $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock
```

### Auth vault rejected (ownership / mode)

Symptom: `auth_vault: ownership mismatch` or `mode not 0600`.

Fix: do **not** chmod blindly. Verify the file belongs to the running
UID and has not been copied from another account. If compromised, delete
and re-authenticate:

```bash
rm ~/.local/share/pcloud-rs/vault.dat
pcloud-cli login <user>
```

### Auth vault file deleted (disaster recovery)

Symptom: the vault file (`~/.local/share/pcloud-rs/vault.dat` on the
default file-vault backend) was deleted or never seeded — for example
the operator wiped state to recover from a suspected compromise, or
the file was lost in a partial backup restore.

Behavior in code (`crates/pcloud-daemon/src/vault/file.rs`,
`load_token` at line 82): a missing vault file is **not** an error.
`load_token` returns `Ok(None)` on `ErrorKind::NotFound`, and bootstrap
treats that as the cold "no persisted token" state. The daemon comes
up with `auth=LoggedOut` in the inline status summary; no sync /
crypto / mount work runs until the operator re-authenticates.
Verified by booting `pcloudd` against an empty `XDG_DATA_HOME` (the
`tests/dr_drill/scenarios/vault_loss.sh` drill exercises this path).

This means there is no "refuse to start" gate to clear — the recovery
procedure is simply to log in again:

```bash
# 1. Confirm the daemon is up and reports LoggedOut.
pcloud-cli status              # auth=LoggedOut in the inline summary

# 2. Re-authenticate. The interactive REPL prompts for password and,
#    if 2FA is enabled on the account, for the TFA code.
pcloud-cli login <user>
```

After a successful login the daemon writes a fresh vault (mode `0600`,
parent `0700`, UID-bound) via the same `vault.store(token)` path
`apply_bootstrap_credentials_with_vault` uses on first install. Token
persistence remains opt-in (`config.features.durable_auth_tokens_enabled`);
if you want the new token kept across restarts, pass `--save-password`
to `pcloud-cli login` so the seed write is enabled.

The drill `tests/dr_drill/scenarios/vault_loss.sh` exercises only the
detection half — boot the daemon against a missing vault and assert
the `auth=LoggedOut` cold state. The login half requires live
credentials and is covered by the `live-e2e` workflow, not the DR
drill.

### TFA required but never prompted

Symptom: login returns `TfaRequired` but CLI exited without prompting.

Fix: re-run with the explicit TFA flag:

```bash
pcloud-cli login <user> --tfa
```

or submit directly:

```bash
pcloud-cli tfa <6-digit-code>
pcloud-cli tfa --recovery <recovery-code>
pcloud-cli tfa --resend-sms
pcloud-cli tfa --resend-notification
```

### Sync root rejected

Symptom: `sync add` returns `NestedRoot`, `IgnoredMount`, or
`DuplicateLocal`.

Fix: pick a local path that is **not**:

- already a registered sync root,
- nested inside another sync root,
- on `/proc`, `/sys`, `/dev`, `/run`, `/snap`, flatpak runtime dirs,
- on a pCloud-drive or other virtual-filesystem mount.

Check: `pcloud-cli sync list` and inspect `/proc/self/mountinfo`.

### Store migration failed

Symptom: `store.open: migration v<N> failed`.

Fix: **do not delete the store**. Capture the log and file a bead. As a
temporary fallback, set `PCLOUD_ENV=development` and stop any active sync
to reduce writes while extracting state before repair. (`pcloudd` has no
`--read-only` flag; rely on environment-level configuration instead.)

### Store file corrupted (disaster recovery)

Symptom: on startup, `pcloudd` exits non-zero before binding the IPC
socket and prints (to stderr):

```
daemon bootstrap failed: store bootstrap failed: sqlite operation failed: file is not a database
```

or, when the file opens but the on-disk pages are damaged:

```
daemon bootstrap failed: store bootstrap failed: ...
```

Behavior in code: `pcloud_store::bootstrap_profile`
(`crates/pcloud-store/src/lib.rs:205`) calls `Connection::open`,
applies pending migrations, and then runs
`integrity::evaluate_connection_integrity`
(`crates/pcloud-store/src/integrity.rs:31`) which issues
`PRAGMA quick_check`. A header-with-garbage layout fails at
`Connection::open`-time with `SqliteFailure { ... } "file is not a
database"`; a structurally-corrupt body fails the `quick_check` and
yields `IntegrityStatus::RepairRequired`. Either failure surfaces as
`BootstrapError::Store` and the daemon refuses to bind the IPC socket
— there is **no auto-delete and no auto-repair** by design.

Recovery is operator-driven. There is intentionally no
`pcloud-cli store repair` command in this fork; the store contains
sync-root metadata and audit-chain entries that an automated rebuild
cannot reconstruct without re-walking pCloud, so an operator must
decide whether to discard the local state or restore from backup.

```bash
# 1. Stop any lingering daemon process.
systemctl --user stop pcloudd 2>/dev/null || true
pkill -TERM -x pcloudd       2>/dev/null || true

# 2. Move the corrupt store aside (do NOT delete — the file may still
#    contain recoverable rows the upstream sqlite tooling can extract
#    via `.dump`/`.recover`).
ts=$(date +%s)
mv ~/.local/share/pcloud-rs/store.sqlite3 \
   ~/.local/share/pcloud-rs/store.sqlite3.corrupt-$ts
# WAL/SHM siblings are no longer valid against a different store file;
# move them aside too so a fresh bootstrap does not reuse them.
mv ~/.local/share/pcloud-rs/store.sqlite3-wal \
   ~/.local/share/pcloud-rs/store.sqlite3-wal.corrupt-$ts 2>/dev/null || true
mv ~/.local/share/pcloud-rs/store.sqlite3-shm \
   ~/.local/share/pcloud-rs/store.sqlite3-shm.corrupt-$ts 2>/dev/null || true

# 3a. Preferred: restore from a known-good backup taken via the
#     `Backup and restore of local state` playbook.
#     tar -xzf pcloud-rs-state-<date>.tgz -C ~

# 3b. Fallback: bootstrap from cold. The daemon will create a fresh
#     empty store on next start; sync roots, conflict state, and
#     audit chain are lost and must be re-added via `pcloud-cli`.
systemctl --user start pcloudd
pcloud-cli status               # confirm `auth=LoggedOut` cold state
pcloud-cli login <user>         # re-authenticate
# Re-add sync roots:
# pcloud-cli sync add <local> <remote>

# 4. Capture the corrupt store and the bootstrap log for triage:
sha256sum ~/.local/share/pcloud-rs/store.sqlite3.corrupt-$ts
journalctl --user -u pcloudd -n 200 --since "10 minutes ago" \
  > /tmp/pcloud-store-corruption-$ts.log
# File a bead and attach BOTH the moved store and the log; never
# attach the vault file (see "Backup and restore of local state").
```

The `pcloudc verify` command (`crates/pcloud-cli/src/verify.rs`) is a
**separate** tool — it walks a synced *tree* and cross-checks local
SHA256 digests against server digests. It does not inspect or repair
the SQLite metadata store. Do not confuse the two: `pcloudc verify`
needs a healthy store + authenticated daemon to function and will
itself fail to start against a corrupt store.

The drill `tests/dr_drill/scenarios/store_corruption.sh` exercises the
detection half: corrupt the file in-place, attempt `pcloudd`
bootstrap, and assert it exits non-zero with a structured error
mentioning the store. The recovery half (move-aside + re-bootstrap +
re-login) requires live credentials and is exercised manually under
the procedure above.

### Crypto locked — requested op needs unlocked shell

Symptom: `CryptoLocked` on operations that require the crypto shell.

Fix:

```bash
pcloud-cli crypto start
```

Note the active crypto path is gated (`bd-1du.5`) and may be disabled in
the running build.

## Log analysis guide

Log format: structured JSON via `pcloud-observability::logging`.

Key event names:

- `daemon.started` / `daemon.stopped`
- `auth.login.ok` / `auth.login.failed` / `auth.tfa.required`
- `auth.vault.persisted` / `auth.vault.removed`
- `sync.root.added` / `sync.root.removed` / `sync.root.paused`
- `transfer.upload.ok` / `transfer.download.ok` / `transfer.*.failed`
- `publink.created` / `publink.changed` / `publink.deleted`
- `crypto.setup` / `crypto.start` / `crypto.stop` / `crypto.mkdir`
- `ipc.peer.rejected` (UID mismatch)
- `store.migration.applied`

Filter failed operations (any JSON-aware log shipper works; a plain
grep on the NDJSON log file is enough for interactive triage):

```bash
grep '"event":"[^"]*\.failed"' daemon.log
```

Find secret leaks (should return **nothing**):

```bash
grep -E '"(password|token|master_key|temppass)":"[^"]' daemon.log
```

If that grep ever prints, treat it as a security incident: rotate the
credential and file a bead.

### Log rotation (audit-06 LOW deployment / pcloud-rs-ncx.87-a)

`pcloud-daemon` writes structured JSON to stderr by default; most
installers tee this to `/var/log/pcloud-rs/daemon.log` or a systemd
journal namespace. If you rotate the file externally (logrotate,
`newsyslog`, or an operator script) you have two safe options:

1. **`copytruncate` (recommended for logrotate).** Rotates by
   copying the file and truncating the original in place. Requires
   no daemon restart and no signal handling. The in-flight fd the
   daemon writes through keeps appending to the truncated file.
   This is the policy we ship in `packaging/linux/logrotate/` and
   the one we test in CI.

2. **`postrotate kill -HUP`.** The daemon listens for `SIGHUP` and
   reopens all log sinks on receipt (see
   `crates/pcloud-daemon/src/signals.rs`). This form is slightly
   more efficient (no interim `cp` on large logs) but requires the
   PID file to be correct. On systemd units use
   `ExecReload=/bin/kill -HUP $MAINPID` or
   `postrotate systemctl reload pcloud-daemon`.

Do NOT use `create` rotation without `copytruncate` or `HUP` — the
daemon's fd would continue writing to the rotated (renamed) inode
and the new file would stay empty until the next daemon restart.

## Backup and restore of local state

> Checklist cross-reference (audit-06 LOW deployment /
> pcloud-rs-ncx.87-e): for disaster-recovery backup/restore of
> page-cache and staging data (separate from config/vault), see
> [`docs/book/src/operations/backup-snapshots.md`](docs/book/src/operations/backup-snapshots.md).
> The procedures below cover operator-owned files; the `backup-snapshots`
> doc covers daemon-managed transient state.

State locations (default XDG paths):

- Config: `~/.config/pcloud-rs/config.json`
- Store: `~/.local/share/pcloud-rs/store.sqlite` (+ `-wal`, `-shm`)
- Auth vault: `~/.local/share/pcloud-rs/vault.dat`
- Page cache / staging: `~/.cache/pcloud-rs/`
- Journal: `~/.local/share/pcloud-rs/journal/`

Backup (daemon must be **stopped** first):

```bash
pcloud-cli shutdown
tar --acls --xattrs -czf pcloud-rs-state-$(date +%F).tgz \
  -C ~/.local/share pcloud-rs \
  -C ~/.config pcloud-rs
chmod 0600 pcloud-rs-state-*.tgz
```

Cache (`~/.cache/pcloud-rs/`) is **disposable** — exclude it from backups.

Restore:

```bash
pcloud-cli shutdown || true
rm -rf ~/.local/share/pcloud-rs ~/.config/pcloud-rs
tar -xzf pcloud-rs-state-<date>.tgz -C ~
chmod 0700 ~/.local/share/pcloud-rs
chmod 0600 ~/.local/share/pcloud-rs/vault.dat
systemctl --user start pcloudd  # or launch manually
pcloud-cli status
```

The vault is UID-bound; restoring on a different UID will be rejected —
you must re-authenticate instead.

## Troubleshooting: Sync Queue Stuck

If `pcloudc status` shows queue depth > 0 but no transfers are completing:

1. Check for active conflicts: `pcloudc conflict list`
2. Check daemon logs: `journalctl -u pcloudd -n 100`
3. Check network connectivity to pCloud API: `pcloudc health`
4. Check available disk space (downloads may pause on low disk): `df -h`
5. Check upload sessions for stuck sessions: `pcloudc upload list`
6. Force a full re-scan: `pcloudc sync status` (shows per-root state)
7. If a stall is suspected, restart the daemon: `systemctl restart pcloudd`

## Escalation

1. Capture: daemon log (json), `pcloud-cli --json status`,
   `bd list --status=open`.
2. Check open beads under `bd-1du.*` — your issue may already be known.
3. File a new bead with reproduction steps; never attach secret material.

## Playbook: First install

Fresh single-host install. Do not reuse a vault copied from another UID.

1. Install the binaries:

   > **Note:** Distribution packages are not yet published. Install from source:
   > ```bash
   > git clone https://github.com/ezechiel203/pcloud-rs
   > cd pcloud-rs
   > cargo build --release -p pcloud-daemon -p pcloud-cli
   > sudo cp target/release/pcloudd /usr/local/bin/
   > sudo cp target/release/pcloudc /usr/local/bin/
   > ```
   >
   > **Aspirational (repos not yet published):** Once distribution packages are available:
   > ```bash
   > # Debian/Ubuntu:   sudo apt install pcloud-rs
   > # Fedora/RHEL:     sudo dnf install pcloud-rs
   > # Arch (AUR):      yay -S pcloud-rs-git
   > # Nix:             nix profile install github:ezechiel203/pcloud-rs#pcloud-rs
   > ```
2. Create the dedicated state directories with correct perms:
   ```bash
   install -d -m 0700 ~/.config/pcloud-rs ~/.local/share/pcloud-rs ~/.cache/pcloud-rs
   ```
3. Drop a minimal production config at `~/.config/pcloud-rs/config.json`
   (profile = `production`, TLS enforced, vault persistence explicitly
   opted in if desired).
4. Enable the systemd user unit:
   ```bash
   systemctl --user daemon-reload
   systemctl --user enable --now pcloudd
   ```
5. Verify:
   ```bash
   pcloudc status
   pcloudc --json status        # stable envelope for scripts
   ```
   `auth=Authenticated` in the status summary plus a `daemon.started`
   event in the journal (`journalctl --user -u pcloudd`) confirm
   a clean install.
6. Log in: `pcloudc login <user>` and complete TFA if prompted.
7. Capture the install fingerprint:
   ```bash
   pcloudc --version > ~/pcloud-rs-install.txt
   sha256sum "$(command -v pcloud-daemon)" >> ~/pcloud-rs-install.txt
   ```
   Keep this next to your vault backup — it is the anchor for later
   rollbacks.

If step 5 fails, stop and triage before logging in. Do not attempt to
create sync roots against a daemon that does not report
`auth=Authenticated` and a healthy engine summary.

## Playbook: Upgrade (pinned -> latest)

Target: upgrade from a pinned version to the latest release **without**
touching the on-disk vault, store, or journal formats (there are no
DB migrations in scope for routine upgrades).

1. Record the current state:
   ```bash
   pcloudc --version
   pcloudc --json status > /tmp/pre-upgrade.json
   sha256sum "$(command -v pcloud-daemon)" > /tmp/pre-upgrade.sha
   ```
2. Snapshot the vault (see "Vault backup / restore" below) **before**
   touching binaries.
3. Stop the daemon cleanly:
   ```bash
   systemctl --user stop pcloudd
   ```
   Confirm exit via `daemon.stopped` in the journal.
4. Install the new binaries (same package manager as first install).
   Do not mix package sources.
5. Restart:
   ```bash
   systemctl --user start pcloudd
   ```
6. Verify:
   ```bash
   pcloudc status               # inline summary: auth, sync, crypto, engine
   pcloudc --version            # confirm daemon version banner matches
   curl --unix-socket $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock \
     http://localhost/health
   curl --unix-socket $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock \
     http://localhost/slo
   ```
   `/health` must return `ok`; `/slo` must show error budget within
   policy.
7. Diff `pcloudc --json status` against `/tmp/pre-upgrade.json`: auth
   state, sync root count, mount state must match.

**Config migration notes.** The config loader transparently migrates
envelope versions v0 and v1 to the current schema in-memory; your
on-disk `config.json` is not rewritten. If you added new fields to
control a new feature, copy them into `config.json` explicitly — the
upgrade will not invent them for you. If `/health` is not `ok` after
restart, roll back immediately (next section) rather than continuing
to investigate with users online.

## Playbook: Rollback

Use only when the upgrade playbook fails verification. Scope: binary +
vault rollback. There are **no** DB schema rollbacks — the store format
is forward-compatible within a minor series.

1. Stop the failing daemon:
   ```bash
   systemctl --user stop pcloudd
   ```
2. Reinstall the previously pinned version. Confirm by sha256:
   ```bash
   sha256sum "$(command -v pcloud-daemon)"
   diff - /tmp/pre-upgrade.sha
   ```
3. Restore the vault snapshot captured before the upgrade:
   ```bash
   install -m 0600 /secure/backup/auth_token.dat \
     ~/.config/pcloud-rs/auth_token.dat
   install -m 0600 /secure/backup/auth_token.meta \
     ~/.config/pcloud-rs/auth_token.meta
   ```
   Ensure the parent directory is `0700` and owned by the target UID.
4. Start the daemon and verify:
   ```bash
   systemctl --user start pcloudd
   pcloudc status
   ```
5. If the status summary does not report `auth=Authenticated`, delete
   the vault and re-authenticate — do **not** leave a mismatched vault
   in place. File a bead with the upgrade log.

Never attempt a rollback by copying the store file across versions. If
the store refuses to open after a rollback, that is a release bug — stop
and escalate.

## Playbook: Vault backup / restore

The vault holds durable auth tokens (opt-in). It is UID-bound, mode
`0600`, and its parent directory must be `0700`. Handle it like a
private key.

Backup:

1. Stop the daemon to avoid copying a half-written vault:
   ```bash
   systemctl --user stop pcloudd
   ```
2. Copy the vault with perms preserved:
   ```bash
   install -m 0600 ~/.config/pcloud-rs/auth_token* /secure/backup/
   ```
   Use a directory that is itself `0700` and on encrypted storage.
3. Restart the daemon.

Restore:

1. Stop the daemon.
2. Place the files back:
   ```bash
   install -d -m 0700 ~/.config/pcloud-rs
   install -m 0600 /secure/backup/auth_token*  ~/.config/pcloud-rs/
   ```
3. Start the daemon and run `pcloudc status`. If the vault metadata
   fails ownership or mode validation, re-authenticate instead — the
   daemon will refuse a mismatched vault by design.

**Cautions.**

- Never commit vault files to version control or ticket attachments.
- Never restore a vault taken from a different UID, hostname, or
  machine image — the daemon rejects cross-UID restores.
- Treat a leaked vault as a full credential compromise: revoke the
  token server-side, rotate, and replace the snapshot.
- Backups older than the current token TTL are useless; rotate snapshot
  schedules with your token lifetime policy.

## Playbook: Certificate rotation

The Rust client trusts the system CA bundle via `webpki-roots` /
`rustls-native-certs`. **There is no application-level certificate
pinning** in the retained Rust path. Rotation is therefore a system-CA
concern, not an application concern.

1. On Debian/Ubuntu, refresh the CA bundle:
   ```bash
   sudo apt update && sudo apt install --reinstall ca-certificates
   sudo update-ca-certificates
   ```
2. On Fedora/RHEL: `sudo update-ca-trust extract`.
3. On Arch: `sudo trust extract-compat`.
4. Restart the daemon so `rustls` picks up the refreshed root store:
   ```bash
   systemctl --user restart pcloudd
   ```
5. Verify TLS continues to negotiate:
   ```bash
   pcloudc status           # healthy engine summary implies recent API calls
   ```
   A recent successful call, visible in the summary and the journal,
   confirms the new roots are accepted.

If a pCloud-side certificate rotation breaks your clients, the remedy
is upstream CA refresh — do **not** patch the binary to disable
verification, and do **not** introduce a local pin. File a bead if you
find yourself tempted.

## Playbook: Incident triage checklist

Run these in order when `pcloudc status` is failing or users report the
daemon is down.

1. **Daemon down?**
   ```bash
   systemctl --user status pcloudd
   journalctl --user -u pcloudd -n 200
   ```
   Look for a `daemon.stopped` with non-zero exit or a panic.
2. **IPC socket orphaned?** If the journal shows `bind: Address already
   in use`, remove the stale socket:
   ```bash
   ls -l $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock
   rm $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock
   systemctl --user start pcloudd
   ```
3. **Mount orphaned?** A prior crash may have left a FUSE mount
   attached. Use the built-in recovery path:
   ```bash
   pcloudc mount --force-umount
   # or, equivalently, via env before restart:
   PCLOUD_FORCE_UMOUNT=1 systemctl --user restart pcloudd
   ```
4. **Journal corrupted?** On startup, journal replay errors are logged
   as `journal.replay.failed`. Capture the log, stop the daemon, and
   move the journal aside:
   ```bash
   mv ~/.local/share/pcloud-rs/journal \
      ~/.local/share/pcloud-rs/journal.bad-$(date +%s)
   ```
   Restart; uncommitted writeback work will be lost but the daemon will
   come up. File a bead with the moved journal attached (redact paths).
5. **Secret leak?** Re-run the grep in "Log analysis guide". Any match
   is a P0.
6. **Still broken?** Capture `pcloudc --json status`, the journal, and
   `bd list --status=open`, then escalate.

## Playbook: Kernel-mount recovery

When a FUSE mount is wedged and a graceful unmount fails:

1. Identify the mountpoint:
   ```bash
   mount | grep pcloud
   ```
2. Lazy-unmount via FUSE:
   ```bash
   fusermount3 -u -z /path/to/mount
   ```
   `-z` detaches immediately and lets in-flight fs ops drain. Expect
   `EIO` to be returned to any process still holding file handles.
3. If `fusermount3` is unavailable, fall back to:
   ```bash
   sudo umount -l /path/to/mount
   ```
4. Restart the daemon:
   ```bash
   PCLOUD_FORCE_UMOUNT=1 systemctl --user restart pcloudd
   ```
5. On startup the journal replay will roll forward any staged writes
   that had been committed locally but not yet uploaded. Entries that
   failed integrity checks are quarantined under
   `~/.local/share/pcloud-rs/journal/quarantine/` and logged as
   `journal.entry.quarantined`. They are not retried automatically —
   inspect and either replay manually or discard.

Mounted-drive parity is still tracked under `bd-1du.4`; expect rough
edges and file beads for anything surprising.

## Playbook: Crypto password rotation

`change_crypto_pass` (and the related `send_change_user_private`)
parity is **still pending** per `STATUS.md` (see the "Crypto parity
progress" section of `CLAUDE.md`). Until those rows land, the supported
rotation procedure is:

1. Unlock the crypto shell with the current password:
   ```bash
   pcloudc crypto start
   ```
2. Export or copy out any locally-held crypto-folder data you need to
   preserve, while unlocked.
3. Use the official pCloud web UI to change the crypto password. The
   Rust client does not yet drive `change_crypto_pass` end-to-end.
4. Stop the daemon:
   ```bash
   systemctl --user stop pcloudd
   ```
5. Restart and re-unlock with the new password:
   ```bash
   systemctl --user start pcloudd
   pcloudc crypto start
   ```
6. Verify via `pcloudc status crypto` (prints the `crypto=` inline
   substring of the status summary) or `pcloudc crypto status`; the
   fingerprint should match the new key material.

A follow-up bead should track first-class CLI support for
`change_crypto_pass` so this playbook can be replaced by a single
command. Until that bead closes, treat the web UI as the source of
truth for crypto password changes.

## Live E2E account setup

The weekly / on-demand `live-e2e` CI job (defined in
`.github/workflows/ci.yml`) runs the suite under
`crates/pcloud-live-e2e/` against a dedicated soak pCloud account.
The job is a hard gate (CLAUDEREV iter-1 TEST-H-1, fire 25): a flake
or outage causes the workflow to fail rather than silently pass. The
operator response is to investigate and re-run, never to mute.

### Provisioning the soak account

1. Create a dedicated pCloud account that will hold no production
   data. Free-tier is sufficient for the current matrix of tests.
2. Disable TFA for this account. The live suite drives TFA-specific
   verbs against a separate fixture path; the main soak account must
   not require an interactive code at login time.
3. Generate a strong password and rotate it on a 90-day cadence (or
   sooner on any signal of credential leak). The current credential
   pair is held only in the GitHub repository secret store as
   `PCLOUD_TEST_USER` / `PCLOUD_TEST_PASSWORD`.
4. Pre-create the test fixture folders the suite expects (the suite
   creates its own ephemeral subfolders per-test, but the parent
   path must exist). See `crates/pcloud-live-e2e/README.md` if a
   bootstrap helper exists; otherwise the suite errors out with a
   `parent folder missing` message that names the path to create.

### Rotating the credentials

1. From the pCloud web UI, change the soak account password.
2. In the repository's GitHub Settings → Secrets and variables →
   Actions, update `PCLOUD_TEST_PASSWORD`.
3. Trigger a manual `workflow_dispatch` run of the `live-e2e` job to
   confirm the new credential is accepted before the next scheduled
   weekly run fires.
4. Record the rotation in the operator log.

### Reading a failed weekly run

1. Open the workflow run in the GitHub Actions UI.
2. Download the `live-e2e-logs-${run_id}` artifact (uploaded by the
   `Upload test artifacts on failure` step). Logs are kept for 7
   days from the run timestamp.
3. The artifact bundles `target/debug/build/**/output` and any
   `/tmp/pcloud-live-e2e-*` scratch files written by the suite. No
   secrets are written to those paths by design.
4. If the failure is a transient pCloud outage (5xx burst, rate-limit
   exhaustion), file a brief operator note and re-run via
   `workflow_dispatch`. If two consecutive runs fail with the same
   non-pCloud-side signature, treat it as a real regression and open
   an issue against the suite.

### Rate-limit and isolation knobs

* `--test-threads=1` is mandatory; the suite must not parallelise
  API access against the soak account.
* `PCLOUD_RATE_LIMIT_*` env vars (consumed by `pcloud-resilience`)
  are sized for one full suite-pass to fit comfortably inside a
  24-hour pCloud per-user quota window. A second pass within the
  same window is fine; a third may begin to throttle and is the
  signal to wait or escalate.

## Release key rotation

The `release-packaging.yml` workflow signs every `.deb`, `.rpm`, and
the `SHA256SUMS` digest file with the project release GPG key
(CLAUDEREV iter-1 DEPLOY-H-11.2). Downstream verifiers fetch the
artifacts plus the matching `.sig` files from the GitHub release page
and run `gpg --verify <artifact>.sig <artifact>` against the
project's published public key.

When any of the three signing secrets is unset the workflow prints
a structured "skipping signing" message and uploads the artifacts
unsigned. That is the dry-run / fork posture, not the production
posture.

### Required secrets

Configured under repo Settings → Secrets and variables → Actions:

* `RELEASE_GPG_PRIVATE_KEY` — ASCII-armored private key block
  (output of `gpg --armor --export-secret-keys $KEY_ID`). Must
  carry the signing-capable subkey.
* `RELEASE_GPG_PASSPHRASE` — passphrase that unlocks the private
  key. Stored as a separate secret so a key-only leak does not
  trivially yield signing capability.
* `RELEASE_GPG_KEY_ID` — long key fingerprint or short id. Passed
  to `gpg --local-user` so a multi-key keyring cannot pick the
  wrong subkey at sign time.

### Provisioning (first-time setup)

1. Generate a release-only keypair on a hardened operator workstation:
   ```
   gpg --quick-generate-key "pcloud-rs Release <release@pcloud-rs.invalid>" \
     ed25519 cert,sign 2y
   ```
2. Note the long fingerprint:
   ```
   gpg --list-secret-keys --keyid-format LONG
   ```
3. Export the secret block (this is what populates the GitHub
   secret; treat the output as a high-value credential):
   ```
   gpg --armor --export-secret-keys $KEY_ID > release-key.asc
   ```
4. Paste the contents of `release-key.asc` into the
   `RELEASE_GPG_PRIVATE_KEY` GitHub secret, then `shred -u` the
   local file.
5. Set `RELEASE_GPG_PASSPHRASE` to the passphrase chosen in step 1
   and `RELEASE_GPG_KEY_ID` to the fingerprint from step 2.
6. Publish the **public** half of the key under
   `docs/release-key.asc` (or the project website) so verifiers
   can import it. Tag the publication commit so verifiers can
   confirm the key bytes against the git history.

### Rotation cadence

Rotate the release key on a 2-year cadence (matching the `2y`
expiry in the generation command above), or immediately on any
signal of compromise. Rotation procedure:

1. Generate the new keypair (step 1 above).
2. Sign the **new public key** with the **old private key** so
   downstream verifiers can chain trust.
3. Update the three `RELEASE_GPG_*` secrets in GitHub.
4. Publish the new public key alongside the cross-signature.
5. Continue to honor the old key for a 30-day overlap window so
   in-flight verifications don't break.
6. Revoke the old key after the overlap, publishing the
   revocation certificate.

### Verifying a signed release

End-user verification flow (also documented in the project README):

```
gpg --import docs/release-key.asc
sha256sum -c SHA256SUMS                # check digests
gpg --verify SHA256SUMS.sig SHA256SUMS # check signatures
gpg --verify pcloud-rs_X.Y.Z_amd64.deb.sig pcloud-rs_X.Y.Z_amd64.deb
```

A failure on any of those three commands means the artifact has
been tampered with; do not install it.
