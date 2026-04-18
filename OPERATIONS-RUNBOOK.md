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

## Backup and restore of local state

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
