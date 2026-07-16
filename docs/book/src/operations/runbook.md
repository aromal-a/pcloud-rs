# Operations Runbook

This chapter is the book-embedded operations runbook. It mirrors the
workspace-root [`OPERATIONS-RUNBOOK.md`](../../../../OPERATIONS-RUNBOOK.md)
and expands the eight core playbooks from plan item P3.7 with exact
commands.

Related chapters:

- [Deployment](./deployment.md) for the 1000-seat rollout checklist.
- [Upgrade](./upgrade.md) for the semver policy, 2-wave rolling upgrade,
  and the `migrate-from-c` flow.
- Per-platform chapters for init-system and mount specifics:
  [Linux](./platforms/linux.md), [macOS](./platforms/macos.md),
  [Windows](./platforms/windows.md), [FreeBSD](./platforms/freebsd.md),
  [OpenBSD](./platforms/openbsd.md), [NetBSD](./platforms/netbsd.md).

> Scope note: the Rust path still has open parity gaps
> (`bd-1du`, `bd-1du.4`, `bd-1du.10`). Do not claim production
> readiness until `STATUS.md` reflects a closed matrix for your
> use case.

## Startup

```bash
target/release/pcloud-daemon \
  --config /etc/pcloud-rs/config.json \
  --log-format json \
  --log-level info
```

On startup the daemon loads config, opens the store, opens the vault
(if enabled), binds the IPC socket at
`$XDG_RUNTIME_DIR/pcloud-rs/daemon.sock` with mode `0600`, starts
backend runtimes, and emits `daemon.started`.

## Shutdown

Graceful:

```bash
pcloudc shutdown
# or:
kill -TERM <pid>
```

The daemon stops new IPC, drains in-flight work, flushes schedulers,
commits store transactions, zeroizes resident secrets, removes the IPC
socket, and emits `daemon.stopped`. Force stop (`kill -KILL`) only when
graceful fails — the next startup will roll forward the journal and
re-validate staged cache consistency.

## Config hot-reload

Edit the on-disk config file, then:

```bash
pcloudc reload
# or:
kill -HUP <pid>
```

Hot-reloadable fields: observability flags (log level, tracing, metrics,
audit export), rate-limit budgets, integrity-sweeper schedule, sync poll
interval, data-residency allow-list. The daemon emits a
`config.reloaded { changed_keys: [...] }` audit event on success, or
`config.reload_failed { error }` if the file cannot be parsed. On parse
error the previous config is kept.

**Not hot-reloadable** (require restart): auth vault path, IPC socket
path, crypto master key / KMS config, managed directory paths,
environment, API endpoint binding.

## Health

```bash
pcloudc status            # plaintext summary: auth, sync, crypto, engine
pcloudc --json status     # same as a stable JSON envelope
pcloudc status auth sync crypto   # extract just those fields (selector order)
pcloudc doctor --json
```

`ready = true` requires: store open and migrations current, IPC socket
bound, and at minimum one successful auth heartbeat if credentials are
persisted.

## Playbook 1: First install

Fresh single-host install. Do not reuse a vault copied from another UID.

1. Install the binaries (package manager per
   [platform chapters](./platforms/linux.md)).
2. Create state dirs with correct perms:
   ```bash
   install -d -m 0700 ~/.config/pcloud-rs ~/.local/share/pcloud-rs ~/.cache/pcloud-rs
   ```
3. Drop a minimal `production` `config.json` at
   `~/.config/pcloud-rs/config.json` (TLS enforced, vault persistence
   opted in if desired).
4. Enable the service (Linux example; see per-platform chapters for
   launchd, per-user Windows startup, and rc.d):
   ```bash
   systemctl --user daemon-reload
   systemctl --user enable --now pcloudd
   ```
5. Verify:
   ```bash
   pcloudc status
   pcloudc doctor --json
   pcloudc --json status         # stable JSON envelope for scripts
   ```
6. Log in: `pcloudc login <user>`; complete TFA if prompted.
7. Capture install fingerprint:
   ```bash
   pcloudc --version > ~/pcloud-rs-install.txt
   sha256sum "$(command -v pcloud-daemon)" >> ~/pcloud-rs-install.txt
   ```

Do not proceed past step 5 until `pcloudc status` reports
`auth=Authenticated` and a healthy engine summary.

## Playbook 2: Upgrade (pinned -> latest)

See [Upgrade](./upgrade.md) for the full semver policy and 2-wave
procedure. Quick path on a single host:

```bash
# 1. Record current state
pcloudc --version
pcloudc --json status > /tmp/pre-upgrade.json
sha256sum "$(command -v pcloud-daemon)" > /tmp/pre-upgrade.sha

# 2. Snapshot the vault (see Playbook 4)

# 3. Stop cleanly
systemctl --user stop pcloudd

# 4. Install new binary (same package source)

# 5. Restart and verify
systemctl --user start pcloudd
pcloudc doctor --json
pcloudc status                 # check auth=Authenticated, engine summary
pcloudc --version              # confirm daemon version banner matches target
curl --unix-socket $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock http://localhost/health
curl --unix-socket $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock http://localhost/slo
```

If `/health` is not `ok` after restart, roll back immediately
(Playbook 3) rather than investigating with users online.

## Playbook 3: Rollback

Use only when Playbook 2 verification fails. Scope: binary + vault
rollback. There are **no** DB schema rollbacks — the store format is
forward-compatible within a minor series.

```bash
# 1. Stop the failing daemon
systemctl --user stop pcloudd

# 2. Reinstall the previously pinned version, verify sha256
sha256sum "$(command -v pcloud-daemon)"
diff - /tmp/pre-upgrade.sha

# 3. Restore vault snapshot from before the upgrade
install -m 0600 /secure/backup/auth_token.dat ~/.config/pcloud-rs/auth_token.dat
install -m 0600 /secure/backup/auth_token.meta ~/.config/pcloud-rs/auth_token.meta

# 4. Start and verify
systemctl --user start pcloudd
pcloudc status
```

If `pcloudc status` does not report `auth=Authenticated` after
rollback, delete the vault and re-authenticate. Never copy the store
file across versions — that is a release bug; stop and escalate.

## Playbook 4: Vault backup / restore

The vault holds durable auth tokens (opt-in). It is UID-bound, mode
`0600`, parent dir `0700`. Handle it like a private key.

Backup:

```bash
systemctl --user stop pcloudd
install -m 0600 ~/.config/pcloud-rs/auth_token* /secure/backup/
systemctl --user start pcloudd
```

Restore:

```bash
systemctl --user stop pcloudd
install -d -m 0700 ~/.config/pcloud-rs
install -m 0600 /secure/backup/auth_token* ~/.config/pcloud-rs/
systemctl --user start pcloudd
pcloudc status
```

Cautions:

- Never commit vault files to version control or ticket attachments.
- Never restore a vault taken from a different UID, hostname, or
  machine image — the daemon rejects cross-UID restores by design.
- Treat a leaked vault as a full credential compromise: revoke the
  token server-side, rotate, and replace the snapshot.

## Playbook 5: Certificate rotation

The Rust client trusts the system CA bundle via `webpki-roots` /
`rustls-native-certs`. **There is no application-level certificate
pinning** — rotation is a system-CA concern.

```bash
# Debian/Ubuntu
sudo apt update && sudo apt install --reinstall ca-certificates
sudo update-ca-certificates

# Fedora/RHEL
sudo update-ca-trust extract

# Arch
sudo trust extract-compat

# Then restart the daemon
systemctl --user restart pcloudd
pcloudc status              # confirm engine summary re-establishes API calls
```

Do **not** patch the binary to disable verification. Do **not**
introduce a local pin. File a bead if tempted.

## Playbook 6: Incident triage checklist

Run these in order when `pcloudc status` fails or users report the
daemon is down.

1. **Run doctor first:**
   ```bash
   pcloudc doctor --json
   ```
   This captures auth state, IPC reachability, vault perms, mount
   state, store health, and journal state in one pass.

2. **Check the service per platform:**
   ```bash
   # Linux (systemd)
   systemctl --user status pcloud-rs-daemon
   journalctl --user -u pcloud-rs-daemon -n 200

   # macOS (launchd)
   launchctl print gui/$(id -u)/com.pcloud.pcloudd

   # Windows (per-user daemon)
   pcloudc status
   Get-Process pcloudd -ErrorAction SilentlyContinue

   # FreeBSD / OpenBSD / NetBSD (rc.d)
   service pcloudd status
   ```

3. **IPC socket orphaned?** If the journal shows
   `bind: Address already in use`:
   ```bash
   ls -l $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock
   rm $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock
   systemctl --user start pcloud-rs-daemon
   ```

4. **Mount orphaned?** See Playbook 7 (kernel-mount recovery).

5. **Journal corrupted?** Startup logs `journal.replay.failed`:
   ```bash
   systemctl --user stop pcloud-rs-daemon
   mv ~/.local/share/pcloud-rs/journal \
      ~/.local/share/pcloud-rs/journal.bad-$(date +%s)
   systemctl --user start pcloud-rs-daemon
   ```
   Uncommitted writeback will be lost; file a bead with the moved
   journal attached (redact paths).

6. **Secret leak?** Run the grep in
   [Log analysis guide](#log-analysis-guide). Any match is P0.

7. **Still broken?** Capture `pcloudc doctor --json`,
   `pcloudc --json status`, the journal, and
   `bd list --status=open`, then escalate.

## Playbook 7: Kernel-mount recovery

When a FUSE mount is wedged and a graceful unmount fails:

```bash
# 1. Identify the mountpoint
mount | grep pcloud

# 2. Platform-specific forced unmount:

# Linux (FUSE3)
fusermount3 -u -z /path/to/mount
# fallback:
sudo umount -l /path/to/mount

# macOS (fuse-t / macFUSE)
diskutil unmount force /path/to/mount
# fallback:
sudo umount -f /path/to/mount

# Windows (WinFSP) — services must be stopped, then manual cleanup:
sc stop pcloudd
net use <drive>: /delete
# If a stale FUSE device remains in Device Manager, remove it:
pnputil /remove-device "WinFspNet"  # admin shell

# FreeBSD (fusefs)
umount -f /path/to/mount

# OpenBSD / NetBSD
umount -f /path/to/mount
```

Then restart the daemon with the force-unmount hint:

```bash
PCLOUD_FORCE_UMOUNT=1 systemctl --user restart pcloud-rs-daemon
# or:
pcloudc mount --force-umount
```

On startup, journal replay will roll forward any staged writes that
were committed locally but not yet uploaded. Entries that fail
integrity checks are quarantined under
`~/.local/share/pcloud-rs/journal/quarantine/` and logged as
`journal.entry.quarantined`; they are not retried automatically.

Mounted-drive parity is tracked under `bd-1du.4` — expect rough edges
and file beads for anything surprising.

## Playbook 8: Crypto password rotation

`change_crypto_pass` and `send_change_user_private` parity is still
pending. Until those rows land, the supported procedure is:

```bash
# 1. Unlock with the current password
pcloudc crypto start

# 2. Export or copy out any locally-held crypto-folder data you
#    need to preserve, while unlocked.

# 3. Use the official pCloud web UI to change the crypto password.

# 4. Stop and restart the daemon
systemctl --user stop pcloud-rs-daemon
systemctl --user start pcloud-rs-daemon

# 5. Re-unlock with the new password
pcloudc crypto start

# 6. Verify fingerprint
pcloudc status crypto      # prints just the crypto= substring of the summary
pcloudc crypto status
```

A follow-up bead tracks first-class CLI support for
`change_crypto_pass`; until it closes, treat the web UI as the source
of truth for crypto password changes.

## Playbook: Vault + store snapshot backup / restore

Applies to: all production deployments. Source of truth for the design:
[`docs/enterprise/disaster-recovery.md`](../../../enterprise/disaster-recovery.md).
Feature overview and key-management guidance:
[Backup Snapshots](./backup-snapshots.md).

The four CLI verbs `backup snapshot-create`, `snapshot-verify`,
`snapshot-restore`, and `snapshot-prune` implement a
GPG-encrypted, point-in-time snapshot of the auth vault, SQLite store,
audit chain, config (secrets redacted to `keyring:*` refs), and plugin
registry manifests. All four verbs shell out to host `gpg(1)` — the
binary must be installed on every host that creates, verifies, or
restores a snapshot.

### 1. Pre-flight (run once per host)

```bash
gpg --version                                    # confirm gpg(1) is installed
gpg --import /secure/keys/dr-team.pub            # public key on backup host
gpg --list-keys dr-team@example.com              # confirm recipient present
test -w /var/backups/pcloud-rs && echo OK         # confirm destination writable
```

Configure `[backup]` in `config.json`:

```toml
[backup]
enabled            = true
destination        = "local:/var/backups/pcloud-rs"
gpg_recipient      = "dr-team@example.com"
retention_days     = 14
verify_on_create   = true
max_artifact_bytes = 1073741824
```

### 2. Create a snapshot (manual, pre-upgrade)

```bash
# Capture the artifact path with a single invocation; --field pulls the
# path out of the structured reply so scripts do not need a JSON parser.
# A full JSON envelope is still available via `pcloudc --json ...` if a
# downstream consumer wants the whole record.
ART=$(pcloudc --field artifact_path backup snapshot-create \
    --gpg-recipient dr-team@example.com \
    --label "pre-upgrade-$(date -u +%F)")
pcloudc backup snapshot-verify "$ART"            # prove it is restorable
```

`snapshot-create` runs inside the daemon under `BackupGuard`, which
quiesces upload-save and sync-commit critical sections only; other
traffic continues.

### 3. Nightly cron

Recommended cron for an operator-owned account (never root):

```cron
# /etc/cron.d/pcloud-rs-backup
15 2 * * *  pcloud-rs  ( \
  pcloudc backup snapshot-create \
      --gpg-recipient dr-team@example.com \
      --label "nightly-$(date -u +\%F)" \
 && LATEST=$(ls -t /var/backups/pcloud-rs/*.tar.gpg | head -1) \
 && pcloudc backup snapshot-verify "$LATEST" \
 && pcloudc backup snapshot-prune --retention-days 14 \
 ) >> /var/log/pcloud-rs/backup.log 2>&1
```

`snapshot-prune` refuses to delete below `[backup.retention].minimum_keep`
and refuses to drop the most recent verified snapshot.

### 4. Restore to the same host

Restore is **destructive**: it stops the daemon and atomically renames
the state-dir to `state-dir.pre-restore.<ts>` before moving the verified
payload into place.

```bash
systemctl stop pcloud-rs                          # (restore also stops it)
pcloudc backup snapshot-verify /var/backups/pcloud-rs/2026-04-15.tar.gpg
pcloudc backup snapshot-restore \
    /var/backups/pcloud-rs/2026-04-15.tar.gpg --yes
systemctl start pcloud-rs
pcloudc status
pcloudc audit verify                             # chain replays from
                                                 # pre-restore tail
```

If the daemon fails to start within 60s, roll back by swapping directories:

```bash
systemctl stop pcloud-rs
mv /var/lib/pcloud-rs   /var/lib/pcloud-rs.failed-restore
mv /var/lib/pcloud-rs.pre-restore.<ts> /var/lib/pcloud-rs
systemctl start pcloud-rs
```

### 5. Restore onto a fresh host

1. Install `pcloud-rs` of the same major version as the snapshot.
2. Import the DR **private** key on the new host only
   (`gpg --import dr-team.priv`).
3. Copy the `.tar.gpg` from your offsite destination.
4. `pcloudc backup snapshot-verify <artifact>`.
5. `pcloudc backup snapshot-restore <artifact> --yes`.
6. Start the daemon; confirm `pcloudc status` and `pcloudc audit verify`.

### 6. Honesty notes

- `gpg(1)` is a runtime dependency: no in-tree GPG implementation is
  used. Audits of the packaging tree should confirm the `gpg` binary is
  listed as a declared runtime dependency of the `.deb` / `.rpm` / brew
  formula on the operator host class.
- Offsite destinations (`s3://`, `sftp://`) are plugin-driven
  (`PluginCapability::BackupDestination`,
  `PluginOperation::BackupPut` / `BackupGet`); see
  [Backup Snapshots](./backup-snapshots.md) for configuration.
- `verify_on_create = true` is the supported default. DR you have not
  verified is DR you do not have.

## Playbook: Verifying local-vs-server integrity on a schedule

**When to use.** You want periodic, throttled cross-checking of local
files against the daemon's recorded size / mtime / content-hash plus a
server-side checksum cross-check via the `ChecksumFetcher` trait.
Typical targets: long-lived staged content on servers, backup
destinations, and crypto-folder metadata.

**Status.** Opt-in. Bead `bd-1du.4.6.1`. `pcloudc integrity run-once`
is end-to-end wired today; the in-process background worker thread
that would drive `schedule_cron` is stubbed, so automatic runs must
currently be driven externally (cron, systemd timer, Task Scheduler).

**Enable in `pcloud.conf`:**

```toml
[profile.features.integrity_sweeper]
enabled               = true
rate_files_per_minute = 60
pause_on_battery      = true
skip_list_path        = "/home/alice/.config/pcloud/integrity.skip"
# schedule_cron left empty — drive it from cron until the in-process
# scheduler lands. Keep this empty to avoid double-running.
schedule_cron         = ""
```

**Seed the skip list** with paths that legitimately diverge (build
outputs, temp caches):

```bash
install -m 0600 /dev/null ~/.config/pcloud/integrity.skip
pcloudc integrity skip ~/pCloud/build-artifacts
pcloudc integrity skip ~/pCloud/.DS_Store
```

**Drive scheduled runs from cron** (nightly at 03:00, JSON captured
for log-shipping):

```cron
0 3 * * *  /usr/bin/pcloudc --json integrity run-once \
           >> /var/log/pcloud/integrity.jsonl 2>&1
```

Or as a systemd timer:

```ini
# /etc/systemd/system/pcloud-integrity.service
[Service]
Type=oneshot
User=alice
ExecStart=/usr/bin/pcloudc --json integrity run-once

# /etc/systemd/system/pcloud-integrity.timer
[Timer]
OnCalendar=*-*-* 03:00:00
Persistent=true
[Install]
WantedBy=timers.target
```

**Inspect results** between runs:

```bash
pcloudc integrity status          # plaintext rollup
pcloudc --json integrity status   # full structured envelope
# Or pluck individual counters with the built-in selector:
pcloudc --field files_hashed --field mismatches_found integrity-status
```

Counter keys: `ok`, `mismatch`, `local_missing`, `remote_missing`,
`throttled`, `fetch_failed`. Mismatches, missing files, and fetch
failures are routed into the audit log under category
`integrity.mismatch` with a path-HMAC (never the cleartext path).
Reconcile divergences by hand using `pcloudc audit verify` and
`pcloudc sync localscan` for the affected sync root.

If the `audit_drops` counter in status is non-zero, treat it as a P1:
persistent audit writes are failing and audit invariant M1 requires
investigation — check disk space, permissions on the audit sink, and
the daemon log for `audit.sink.*` events.

To stop verifying a path permanently: `pcloudc integrity skip <PATH>`.
The daemon appends to `skip_list_path` and hot-reloads it in-process.

## Verifying integrity of a sync root (on-demand `pcloudc verify`)

Use this playbook after a crash, a disk swap, a suspected ransomware
event, or whenever `pcloudc doctor` flags `integrity.mismatch` audit
rows. This is the on-demand, operator-driven counterpart to the
periodic integrity sweeper above.

1. **Read-only pass first.** Never start with `--fix`.

   ```bash
   pcloudc verify "$SYNC_ROOT" --recursive
   ```

   Exit codes:

   - `0 Ok` — all objects matched.
   - `6 Unavailable` — walk completed with `MISSING_LOCAL` /
     `MISSING_REMOTE` records and no `--fix` was requested. Safe.
   - `7 Conflict` — at least one `MISMATCH local=… server=…` was
     observed. Treat as a data-integrity incident.

2. **Collect evidence for any mismatch.** Always capture before
   touching anything:

   ```bash
   # NDJSON — one record per line, safe to stream. Pipe the captured
   # file into whatever JSON consumer your incident pipeline already
   # uses (log shipper, SIEM rule, custom parser).
   pcloudc --json verify "$SYNC_ROOT" --recursive \
     | tee "/var/log/pcloud-rs/verify-$(date -u +%FT%TZ).ndjson" \
     > /dev/null
   grep -v '"kind":"ok"' \
     "/var/log/pcloud-rs/verify-$(date -u +%FT%TZ).ndjson"
   ```

   The NDJSON stream is append-safe: one JSON object per line, never a
   wrapping array.

3. **Decide remediation direction per record.** `MISSING_LOCAL` and
   `MISMATCH` usually resolve by re-download; `MISSING_REMOTE`
   resolves by re-upload. If the local disk is suspect, prefer
   re-download. If the server copy is suspect (rare), restore from a
   `backup snapshot-restore` first and re-run verify.

4. **Apply fixes (destructive).** Only after step 3:

   ```bash
   pcloudc verify "$SYNC_ROOT" --recursive --fix --yes
   ```

   Re-run the read-only pass afterwards; it must return `0 Ok`.

5. **Wire into cron.** `7 Conflict` is the signal to page; `6
   Unavailable` is the signal to file a ticket.

   ```bash
   # /etc/cron.d/pcloud-rs-verify
   30 3 * * *  pcloud-rs  pcloudc verify /srv/pcloud --recursive \
                 >> /var/log/pcloud-rs/verify.log 2>&1
   ```

## Recovering an older version of a file

pCloud's third-party API does not expose revision-history endpoints
(`listrevisions` / `revertfile`), so `pcloud-rs` does not ship a
`pcloudc log` / `diff` / `restore` surface. See
[`docs/future-pcloud-clone-api.md`](../../../future-pcloud-clone-api.md)
("Removed scaffolds") for context.

To recover an older version of a file today:

1. Use the pCloud web UI revision history to identify the revision id
   and timestamp you want to recover.
2. If the file lives inside a sync root covered by a recent
   `backup snapshot-create`, verify the snapshot and perform a targeted
   out-of-band extract rather than a full `snapshot-restore`:

   ```bash
   pcloudc backup snapshot-verify /var/backups/pcloud-rs/<date>.tar.gpg
   ```

3. Failing that, download the desired revision from the web UI and
   place it back under the sync root; `pcloudc verify --recursive`
   will confirm the reconciliation and `pcloudc sync localscan` will
   propagate the change.

## Log analysis guide

Structured JSON via `pcloud-observability::logging`. Key events:

- `daemon.started` / `daemon.stopped`
- `auth.login.ok` / `auth.login.failed` / `auth.tfa.required`
- `auth.vault.persisted` / `auth.vault.removed`
- `sync.root.added` / `sync.root.removed` / `sync.root.paused`
- `transfer.upload.ok` / `transfer.download.ok` / `transfer.*.failed`
- `publink.created` / `publink.changed` / `publink.deleted`
- `crypto.setup` / `crypto.start` / `crypto.stop` / `crypto.mkdir`
- `ipc.peer.rejected` (UID mismatch)
- `store.migration.applied`
- `journal.entry.quarantined`

Filter failed operations (ripgrep on the NDJSON log file; any
JSON-aware log shipper works equally well):

```bash
grep '"event":"[^"]*\.failed"' daemon.log
```

Find secret leaks (should return **nothing**):

```bash
grep -E '"(password|token|master_key|temppass)":"[^"]' daemon.log
```

If that grep ever prints, treat it as a P0 security incident.

## Traceparent correlation

When the daemon is built with the `tracing-otlp` feature and
`[observability.tracing]` is enabled, every `pcloudc` invocation emits
a W3C traceparent line on **stderr** before the command result:

```
[trace: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01]
```

Use it as the primary correlation key during triage:

1. **Ask the user for the `[trace: ...]` line** (or have them rerun
   with `--trace-id <hex>` using an id you provide). This is the
   single piece of data support needs.
2. **Search the trace id in the OTLP backend** (Jaeger, Tempo,
   Datadog, Honeycomb, New Relic). You should see:
   - `pcloudc.command` — CLI root
   - `pcloudd.dispatch` — IPC server span
   - `pcloudd.backend.<name>` — handler span
     (`transfer` / `sync` / `crypto` / `public_link` / `shares` /
     `backup` / `account`)
   - `pcloud.proto.<method>` — upstream HTTPS call
   Missing levels localise the failure hop.
3. **Error-biased 100% sampling** means any non-Ok span was exported
   with its full ancestor chain, even at the default
   `sample_rate = 0.01`. If an error trace is not present, the
   failure happened before the CLI opened the span — check the
   stderr line was emitted at all, then fall through to the standard
   playbooks.
4. **Force-sample a repro** by passing `--trace-id` on the retry —
   the supplied id force-samples regardless of `sample_rate` and
   keeps the same trace id across multiple invocations for
   correlation.
5. **PII safety**: attributes on exported spans are restricted by the
   `attr_redact` five-key allow-list (`command`, `duration_ms`,
   `error_category`, `status_code`, `trace_kind`). Forbidden keys
   panic in debug builds and are dropped in release. Traces therefore
   do not leak filenames, paths, emails, tokens, or crypto material
   and are safe to attach to tickets without redaction.

See [enterprise/tracing](../../../enterprise/tracing.md) for the
full design, sampling policy, and collector configuration schema.

## Escalation

1. Capture: daemon log (json), `pcloudc doctor --json`,
   `pcloudc --json status`, `bd list --status=open`.
2. Check open beads under `bd-1du.*` — your issue may already be known.
3. File a new bead with reproduction steps; never attach secret
   material.

---

# Enterprise Incident Playbooks

The playbooks below follow a uniform shape so on-call operators can skim
quickly:

- **Problem.** One-sentence statement of the failure mode.
- **Symptoms.** What the operator or user actually sees.
- **Diagnose.** Exact commands (no guess work) that produce expected
  output shapes. Avoid `jq` — use the built-in `--field` selector and
  `--json` envelopes.
- **Remediate.** The minimal corrective action.
- **Cleanup.** Post-incident state reset so the system is left in a
  known-good posture.
- **Prevent.** What to change so the incident does not recur.
- **Escalate.** When to page, what artefacts to attach.
- **Related.** Other playbooks and beads.

> **Pre-alpha honesty:** some playbooks reference CLI paths whose
> daemon side is still partially wired (see `bd-1du`, `bd-1du.4`,
> `bd-1du.10`). Each such section is tagged with a **Status** note that
> tells you what works today vs. what is forward-looking.

## Playbook 9: `pcloudc login` hangs at password prompt

**Problem.** The interactive login prompt never completes the TFA /
password handshake and the CLI appears to block indefinitely.

**Symptoms.**

- `pcloudc login alice@example.com` prints `Password:` and hangs.
- No `auth.login.ok` or `auth.login.failed` in the daemon log.
- `pcloudc status` from a second terminal returns `auth=Unauthenticated`.

**Diagnose.**

```bash
# 1. Is the daemon actually alive and reachable on the IPC socket?
pcloudc --field ready doctor            # expect: true
pcloudc --field ipc_socket doctor       # expect: path + mode 0600

# 2. Is the login pending a second factor the CLI is not surfacing?
pcloudc session-status
pcloudc --json session-status           # structured: phase, waiting_for
pcloudc --field phase session-status    # expect: "awaiting_tfa" or similar

# 3. Is there a stale password prompt blocking stdin (e.g. pipe closed)?
ps -fC pcloudc | head
```

**Remediate.**

```bash
# Kill the hung client (daemon session survives).
pkill -TERM -f 'pcloudc login'

# Resume the session explicitly by token. Preferred: stdin, not argv.
printf '%s' "$PCLOUD_PW" | pcloudc submit-password --password-stdin alice@example.com
pcloudc submit-tfa 123456               # if a TFA code is still pending
pcloudc userinfo                        # confirms the session
```

**Cleanup.**

- Clear the shell history line containing any bare argv password:
  `history -d $(history 1 | awk '{print $1}')`.
- Confirm `auth.login.ok` appears in the journal.

**Prevent.**

- Always prefer `--password-stdin` or `--password-env PCLOUD_PW`; never
  pass a password as a positional argv token (it leaks via
  `/proc/<pid>/cmdline`).
- Scripts should set a hard timeout around the login command.

**Escalate.** Capture `pcloudc doctor --json` and the daemon log for
the last 2 minutes. Escalate if `session-status` returns a phase the
CLI does not render.

**Related.** Playbook 10 (2FA), Playbook 11 (mid-pipeline session
expiry), `canonical_token_for` entries `login`, `submit-password`,
`submit-tfa`, `session-status`.

## Playbook 10: 2FA code rejected repeatedly

**Problem.** `pcloudc submit-tfa <code>` fails with `auth.tfa.rejected`
or the session keeps demanding a code.

**Symptoms.**

- `auth.tfa.rejected` events in the daemon log.
- Exit code `4 Unauthorized` or `7 Conflict` from the submit commands.
- `pcloudc session-status` stays in `awaiting_tfa`.

**Diagnose.**

```bash
pcloudc --field phase session-status           # expected: awaiting_tfa
pcloudc --field tfa_channel session-status     # sms / app / notification
pcloudc --json session-status                  # full envelope
# Recent TFA-related events:
grep -E '"event":"auth\.tfa\.' /var/log/pcloud-rs/daemon.log | tail -20
```

**Remediate.**

```bash
# If the code is expired or the channel went silent, re-request delivery:
pcloudc send-tfa-sms                    # SMS channel
pcloudc send-tfa-notification           # push/app channel

# If the device is unreachable, fall through to a recovery code:
pcloudc submit-recovery                 # prompts on stdin

# Then retry:
pcloudc submit-tfa                      # prompts on stdin (no argv)
```

**Cleanup.**

- If a recovery code was consumed, rotate the recovery list via the
  pCloud web UI.
- Shred any paper / screenshot of the used recovery code.

**Prevent.**

- Keep host clocks within ±30 s of NTP — TOTP windows are narrow.
- Configure `--trust-device` after the first successful TFA on a
  trusted host to avoid per-command prompts.

**Escalate.** If three consecutive SMS resends fail, the upstream SMS
gateway is likely the cause — escalate to the pCloud account team with
`auth.tfa.sms.*` events.

**Related.** Playbook 9, `send-tfa-sms`, `send-tfa-notification`,
`submit-tfa`, `submit-recovery`.

## Playbook 11: Session expired mid-pipeline (exit 3)

**Problem.** A long-running batch pipeline sees a scripted `pcloudc`
call return exit code `3 SessionExpired` partway through.

**Symptoms.**

- Exit code `3` from any authenticated subcommand.
- `auth.session.expired` event in the daemon log.
- `pcloudc --field auth status` returns `Unauthenticated`.

**Diagnose.**

```bash
pcloudc --field auth status                    # expect: Authenticated
pcloudc --field last_auth_heartbeat doctor
pcloudc --json session-status
```

**Remediate.**

```bash
# If durable vault persistence is enabled, the daemon refreshes the
# token on next authenticated call. Force it:
pcloudc userinfo

# If the vault is not persistent (opt-in), re-authenticate:
pcloudc submit-password --password-stdin "$PCLOUD_USER" < /run/pcloud.pw
# (for opt-in persistence only)
pcloudc authsave                        # persists the freshly-minted token
```

**Cleanup.**

- Re-run the failed pipeline stage. Prefer idempotent stages that
  tolerate retry.
- Verify the vault is still `0600` and parent dir is `0700`
  (Playbook 12).

**Prevent.**

- Wrap authenticated pipelines in a guard that detects exit `3` and
  re-auths once before giving up.
- For unattended batches, opt in to vault persistence and pin the
  session with `authsave`.

**Escalate.** Repeated mid-pipeline expiry on a single host points at
clock skew, TLS trust anchor issues, or a forced server-side logout.
Capture `doctor --json` + the `auth.*` event stream for the last
hour.

**Related.** Playbook 12, Playbook 4 (vault backup/restore), `authsave`.

## Playbook 12: Token vault perms wrong (0644 instead of 0600)

**Problem.** The auth vault file or its parent directory has loose
permissions. The daemon refuses to open it on startup as a
safety-preserving default.

**Symptoms.**

- Daemon start fails with `auth.vault.perm.rejected`.
- `pcloudc doctor --json` reports vault health as `unhealthy`.
- `ls -l ~/.config/pcloud-rs/auth_token*` shows mode `0644` / `0664`.

**Diagnose.**

```bash
pcloudc --field vault_ok doctor
pcloudc --json doctor | grep -E 'vault|perm'
ls -ld ~/.config/pcloud-rs
ls -l  ~/.config/pcloud-rs/auth_token*
stat -c '%a %U %G %n' ~/.config/pcloud-rs ~/.config/pcloud-rs/auth_token* 2>/dev/null
```

**Remediate.**

```bash
systemctl --user stop pcloudd
chmod 0700 ~/.config/pcloud-rs
chmod 0600 ~/.config/pcloud-rs/auth_token*
chown "$(id -u):$(id -g)" ~/.config/pcloud-rs/auth_token*
systemctl --user start pcloudd
pcloudc --field vault_ok doctor          # expect: true
```

**Cleanup.**

- Audit the umask used by whatever process last wrote the file.
- If the file was readable by another UID, treat it as a credential
  leak and follow Playbook 13.

**Prevent.**

- Never copy `auth_token*` across UIDs or hosts.
- Run a daily cron that greps `stat` output and alerts on drift.

**Escalate.** If the daemon still rejects after chmod, the vault
metadata is likely signed against a different UID — rebuild the
vault by re-authenticating.

**Related.** Playbook 4, Playbook 13,
`crates/pcloud-daemon/src/auth_vault.rs`.

## Playbook 13: Persisted token stolen — full revocation

**Problem.** A copy of `auth_token.dat` escaped the host (leaked log,
shared backup, compromised workstation).

**Symptoms.** Any of: vault file on a non-origin host, vault file in
git history, `auth_token.dat` found in a world-readable location.

**Diagnose.**

```bash
pcloudc --json session-status
grep -E '"event":"auth\.vault\.' /var/log/pcloud-rs/daemon.log
grep -E '"password"|"token"' /var/log/pcloud-rs/daemon.log   # must be empty
```

**Remediate.**

```bash
# 1. Invalidate the daemon-side token immediately.
pcloudc logout
# 2. Remove the local vault (daemon must be stopped to delete safely).
systemctl --user stop pcloudd
rm -f ~/.config/pcloud-rs/auth_token*
# 3. Revoke upstream sessions via the pCloud web UI
#    (Settings -> Active sessions -> Terminate all).
# 4. Rotate the account password via the web UI.
# 5. Re-authenticate on trusted hosts only; opt back into persistence
#    only on hosts that still need it.
systemctl --user start pcloudd
pcloudc login "$PCLOUD_USER"
pcloudc authsave                        # opt-in
```

**Cleanup.**

- Purge backup artefacts containing the old vault.
- If the leak touched git history, force-delete the ref and rotate any
  shared deploy keys used to push the repo.

**Prevent.**

- Encrypt backup destinations at rest (GPG-only for
  `backup snapshot-create`, per Playbook 5 / backup chapter).
- Add a pre-commit hook that blocks staging `auth_token*`.

**Escalate.** P0 security incident. File an internal ticket with the
time of first suspected access and the revocation trail.

**Related.** Playbook 4, Playbook 12, `logout`, `authsave`.

## Playbook 14: `sync-add` rejects path as nested root

**Problem.** `pcloudc sync-add /path /remote` fails because the local
path is a parent of, or contained in, an already-registered sync root.

**Symptoms.**

- Exit code `7 Conflict` with message `nested local root`.
- `sync.root.add.rejected` event in the daemon log.

**Diagnose.**

```bash
pcloudc sync-list
pcloudc --json sync-list
pcloudc --field roots sync-list                # array of active roots
readlink -f /path/to/would-be-root             # canonical form matters
```

**Remediate.**

```bash
# Option A: use a disjoint path.
pcloudc sync-add /srv/pcloud/project-b /ProjectB

# Option B: remove the enclosing root first (destructive to its state):
pcloudc sync-list
pcloudc sync-remove <ID>
pcloudc sync-add /srv/pcloud/project-b /ProjectB
```

**Cleanup.**

- After `sync-remove`, confirm `sync-list` no longer shows the root.
- Run `pcloudc sync-localscan` on the replacement root to repopulate
  the catalogue.

**Prevent.**

- Adopt a flat directory convention: one top-level folder per sync root.
- Encode the naming rule in provisioning scripts so operators do not
  accidentally nest.

**Escalate.** Rare. If the rejection happens for a path that is
genuinely disjoint (per `readlink -f`), capture both canonical forms
and file a bead — this is a classifier bug.

**Related.** Playbook 15, `sync-add`, `sync-list`, `sync-remove`,
`sync-localscan`.

## Playbook 15: Mountpoint busy on unmount (force-unmount recipe)

**Problem.** `pcloudc unmount` returns `EBUSY` because a process still
holds an open file descriptor inside the mount.

**Symptoms.**

- `unmount` exits non-zero with `Device or resource busy`.
- `lsof` shows open files under the mountpoint.
- `mount | grep pcloud` still lists the mount.

**Status.** Mounted-drive parity is still in progress under `bd-1du.4`.
The CLI surface is wired; FUSE runtime edge cases may still surprise
you.

**Diagnose.**

```bash
mount | grep pcloud
lsof +D /path/to/mount | head -20
fuser -vm /path/to/mount
pcloudc --field mounts fs-status
pcloudc --json fs-status
```

**Remediate.**

```bash
# 1. Try the polite path first.
pcloudc unmount /path/to/mount

# 2. Escalate to a lazy / forced unmount (Linux).
fusermount3 -u -z /path/to/mount
# or:
sudo umount -l /path/to/mount
sudo umount -f /path/to/mount

# 3. If those fail, kill offending processes and retry.
fuser -km /path/to/mount
sudo umount /path/to/mount
```

**Cleanup.**

- Confirm `mount | grep pcloud` is empty.
- Restart the daemon to rebuild the mount supervisor cleanly:
  `systemctl --user restart pcloudd`.

**Prevent.**

- Teach users not to `cd` into mounts from long-lived shells.
- Run a pre-unmount `fuser -vm` sweep in automation.

**Escalate.** If `fuser -km` cannot clear the busy state, capture
`dmesg | tail` and the `fs-status` envelope, and page. Possible kernel
FUSE driver wedge.

**Related.** Playbook 7, Playbook 16, `mount`, `unmount`, `fs-status`.

## Playbook 16: Mount silently drops writes

**Problem.** Files written under the mount appear locally but never
appear on the server.

**Symptoms.**

- No `transfer.upload.ok` events for the affected paths.
- `pcloudc fs-status` shows `pending_writes > 0` and not decreasing.
- `sync-localscan` flags `remote_missing`.

**Status.** Write path parity is part of `bd-1du.4`. Expect rough edges.

**Diagnose.**

```bash
pcloudc fs-status
pcloudc --json fs-status
pcloudc --field pending_writes --field oldest_pending_age_s fs-status
journalctl --user -u pcloudd -n 500 | grep -E 'upload|sidecar|writeback'
```

**Remediate.**

```bash
# 1. Force a localscan of the affected sync root — it reconciles the
#    catalogue and re-queues missing uploads.
pcloudc sync-localscan

# 2. Cycle the daemon to flush the writeback scheduler.
systemctl --user restart pcloudd

# 3. If the upload queue still does not drain, replay sidecars explicitly.
pcloudc pending
pcloudc --json pending
```

**Cleanup.**

- Re-run `pcloudc verify <sync-root> --recursive` to confirm server
  and local agree.
- Inspect quarantined journal entries (Playbook 6 step 5).

**Prevent.**

- Enable the integrity sweeper (runbook §Verifying local-vs-server
  integrity on a schedule).
- Monitor `pending_writes` and `writeback_errors` in Prometheus.

**Escalate.** If `pending_writes` stalls at the same number across
two daemon restarts, the sidecar journal is likely stuck on a specific
object — attach the relevant sidecar and open a `bd-1du.4` bead.

**Related.** Playbook 7, Playbook 17, `fs-status`, `sync-localscan`,
`pending`.

## Playbook 17: Sync loop oscillates

**Problem.** The same set of files is continuously uploaded and
re-downloaded; throughput is burned on churn.

**Symptoms.**

- Log interleaves `transfer.upload.ok` and `transfer.download.ok` for
  identical paths on a tight loop.
- High CPU and bandwidth with no net progress.

**Diagnose.**

```bash
pcloudc --json sync-list
pcloudc sync-localscan
grep -E '"event":"transfer\.(upload|download)\.ok"' \
     /var/log/pcloud-rs/daemon.log | tail -200 | sort | uniq -c | sort -rn | head
pcloudc --field engine_summary status
```

**Remediate.**

```bash
# 1. Pause both directions for the root.
pcloudc pause

# 2. Full re-enumeration / catalogue rebuild.
pcloudc sync-localscan

# 3. Resume.
pcloudc resume
```

**Cleanup.**

- `pcloudc verify <root> --recursive` to confirm steady state.
- Inspect the culprit files — common root cause is filesystem mtime
  rounding (FAT/exFAT) or an editor that rewrites atomically on save.

**Prevent.**

- Avoid sync roots on filesystems with 2-second mtime resolution.
- Add editor-specific ignore rules (`.DS_Store`, `~*.tmp`) to the
  integrity skip list.

**Escalate.** Attach the top oscillating paths (HMAC-redacted) and the
last 500 `transfer.*` events to a bead.

**Related.** Playbook 16, Playbook 20, `pause`, `resume`,
`sync-localscan`.

## Playbook 18: Orphaned upload sidecar

**Problem.** A crashed upload left behind a sidecar file that did not
get replayed on the next daemon start.

**Symptoms.**

- `pcloudc pending` lists an upload that never progresses.
- Sidecar files under the state dir older than the most recent daemon
  uptime.

**Status.** Sidecar replay helper
(`pcloud_fs::write_path::replay_upload_sidecars`) is wired in-process;
there is no top-level CLI verb for it yet, so recovery runs on daemon
restart.

**Diagnose.**

```bash
pcloudc --json pending
pcloudc --field items pending
ls -lt ~/.local/share/pcloud-rs/journal/upload-sidecars/ | head
```

**Remediate.**

```bash
# 1. Graceful restart — the write-path enumerator runs on boot.
systemctl --user restart pcloudd
pcloudc pending

# 2. If a sidecar cannot be replayed, quarantine it by hand:
systemctl --user stop pcloudd
mv ~/.local/share/pcloud-rs/journal/upload-sidecars/<bad> \
   ~/.local/share/pcloud-rs/journal/quarantine/
systemctl --user start pcloudd
pcloudc pending
```

**Cleanup.**

- Re-queue the originating action (drop the file back into the mount).
- Audit any `journal.entry.quarantined` events.

**Prevent.**

- Avoid killing the daemon with `SIGKILL` during an active upload.
- Run the integrity sweeper nightly so slow-leaked sidecars are caught.

**Escalate.** If the sidecar reappears after quarantine, the upstream
write path is re-creating it — file a `bd-1du.4` bead with the
sidecar and the originating path HMAC.

**Related.** Playbook 16, Playbook 17, `pending`.

## Playbook 19: Large upload interrupted — resume semantics

**Problem.** A multi-gigabyte upload was interrupted. The operator
wants to resume rather than restart.

**Symptoms.**

- `transfer.upload.ok` never arrives; `transfer.upload.failed` is
  followed by silence.
- `pcloudc pending` shows the item with a partial byte count.

**Diagnose.**

```bash
pcloudc --json pending
pcloudc --field items pending
# Inspect sidecar on disk (layout: one dir per in-flight upload):
ls -l ~/.local/share/pcloud-rs/journal/upload-sidecars/
```

**Remediate.**

```bash
# 1. Ensure the daemon is running.
pcloudc status

# 2. Trigger a re-queue by touching the source (if it still exists).
#    The sidecar carries the uploadid so bytes already accepted by the
#    server are not re-sent.
touch /path/under/sync/root/bigfile.iso
pcloudc sync-localscan

# 3. Watch the sidecar drain.
watch -n 2 'pcloudc --field items pending'
```

**Cleanup.**

- `pcloudc verify /path/under/sync/root --recursive` to confirm size
  and SHA256 match the server.

**Prevent.**

- Run uploads under `systemd-run --property=OOMScoreAdjust=-500` so the
  OOM killer is less likely to terminate them.
- Monitor `network.throughput` metrics for sustained drops.

**Escalate.** If `pending` never decreases after 2 localscan cycles,
the sidecar `uploadid` is likely expired server-side. Remove the
sidecar and restart the upload from byte 0 (Playbook 18 cleanup).

**Related.** Playbook 18, Playbook 20, `pending`, `sync-localscan`.

## Playbook 20: Large download interrupted — resumable fetch

**Problem.** A long download died partway. The operator wants the
client to resume with `Range:` semantics rather than restart.

**Symptoms.**

- A `.part` file sits next to the target path.
- `transfer.download.failed` in the log with a `Range` error
  classification.

**Status.** The resumable HTTP helper
(`pcloud_proto::http_download::fetch_download_resumable`) is
in-tree and exercised by the transfer backend. There is no user-visible
`download --resume` flag yet; recovery runs via re-queue.

**Diagnose.**

```bash
pcloudc --json pending
pcloudc --field items pending
ls -l /path/to/target /path/to/target.part 2>/dev/null
```

**Remediate.**

```bash
# 1. Re-queue the download.
pcloudc sync-localscan

# 2. Monitor progress; the client sends a Range request matching the
#    current .part length.
watch -n 2 'ls -l /path/to/target.part 2>/dev/null; pcloudc --field items pending'
```

**Cleanup.**

- Once complete, confirm SHA256 with `pcloudc verify <path>` (see
  Playbook 21 if it mismatches).

**Prevent.**

- Keep a generous `[transfer.download].max_retries` in config.
- Provision enough free disk for `.part` (peak 2x file size during
  atomic rename).

**Escalate.** If every resume attempt re-starts from byte 0, the
server is refusing `Range:` for that object; attach the request id.

**Related.** Playbook 19, Playbook 21, `pending`, `sync-localscan`.

## Playbook 21: SHA256 mismatch on download

**Problem.** A completed download fails its content-hash check.

**Symptoms.**

- `pcloudc verify <path>` exits `7 Conflict` with
  `MISMATCH local=… server=…`.
- `transfer.download.hash_mismatch` in the log.

**Diagnose.**

```bash
pcloudc verify /path/to/file
pcloudc --json verify /path/to/file
sha256sum /path/to/file
# Cross-check against the daemon-recorded server digest:
pcloudc --field server_sha256 --field local_sha256 verify /path/to/file
```

**Remediate.**

```bash
# 1. Delete the corrupt artefact and any lingering .part.
rm -f /path/to/file /path/to/file.part

# 2. Re-queue the download.
pcloudc sync-localscan

# 3. Re-verify.
pcloudc verify /path/to/file           # expect: 0 Ok
```

**Cleanup.**

- If the mismatch recurs across three fresh downloads, suspect
  on-path corruption (NIC, disk, TLS MITM). Capture
  `doctor --json` + TLS endpoint info and escalate.
- Rotate the TLS trust store (Playbook 5) if corruption is TLS-shaped.

**Prevent.**

- Keep the integrity sweeper enabled on critical roots.
- Monitor `transfer.*.hash_mismatch` events; page on non-zero.

**Escalate.** Persistent mismatches are a P1 integrity incident.

**Related.** Playbook 20, Runbook §Verifying local-vs-server
integrity.

## Playbook 22: 429 Too Many Requests — backoff tuning

**Problem.** The server returns sustained `429` responses and the
client is throttled.

**Symptoms.**

- `transfer.*.throttled` or `proto.http.status=429` in the log.
- `pcloudc --field engine_summary status` shows reduced throughput.

**Diagnose.**

```bash
grep -c '"status_code":429' /var/log/pcloud-rs/daemon.log
pcloudc --json status
pcloudc --field rate_limited status
```

**Remediate.**

- Let the client back off; its built-in exponential policy respects
  `Retry-After`. No manual action usually needed.
- If urgent, reduce concurrency in config and restart:

```toml
# config.json
[transfer]
max_concurrent_uploads   = 2
max_concurrent_downloads = 2
```

```bash
systemctl --user restart pcloudd
```

**Cleanup.**

- After the storm, re-enable your previous concurrency.
- Audit any external tooling hitting the same account.

**Prevent.**

- Do not run multiple `pcloudd` instances against the same account.
- Stagger batch jobs across hosts.

**Escalate.** Persistent 429 across a 24 h window despite low
concurrency is an account-side rate-limit — page the pCloud account
team with a 1-hour sample of 429 events.

**Related.** Playbook 19, Playbook 20.

## Playbook 23: Bulk public-link cleanup (native selectors)

**Problem.** Hundreds of stale public links must be removed.

**Symptoms.** Audit of `list-links` shows many links the account no
longer wants exposed.

**Diagnose.**

```bash
pcloudc list-links
pcloudc --json list-links
# Stream just the ids using the native field selector — no jq needed:
pcloudc --field id list-links
pcloudc --field id --field path --field expires list-links
```

**Remediate.**

```bash
# Iterate the id stream into delete-link. Safe because --field prints
# one value per line, whitespace-free.
pcloudc --field id list-links \
  | while IFS= read -r ID; do
        pcloudc delete-link "$ID"
    done
```

**Cleanup.**

- Re-run `pcloudc list-links` to confirm the target set is gone.
- Capture the pre-delete envelope
  (`pcloudc --json list-links > /tmp/pre-cleanup.json`) for audit.

**Prevent.**

- Enforce link expiry at creation time
  (`pcloudc change-link-expire <ID> <date>`).
- Run the publink-expiry plugin (Playbook 24).

**Escalate.** If deletion fails with `7 Conflict` on a specific id,
the link is currently in-flight — retry after 30s.

**Related.** Playbook 24, Playbook 25, `list-links`, `delete-link`,
`change-link-expire`.

## Playbook 24: Expired-link audit

**Problem.** Operators want a daily report of links that expired in
the last N days so owners can refresh or remove them.

**Status.** The `publink-expiry` helper is plugin-driven. Until the
plugin ships, use the native field-selector pipeline below.

**Diagnose.**

```bash
pcloudc list-links
pcloudc --json list-links
pcloudc --field id --field path --field expires list-links
```

**Remediate.**

```bash
# Extract expired links with a native, jq-free pipeline.
NOW_EPOCH=$(date -u +%s)
pcloudc --json list-links \
  | grep -oE '"id":[0-9]+,"path":"[^"]*","expires":[0-9]+' \
  | awk -F'[,:]' -v now="$NOW_EPOCH" '$NF != "" && $NF < now'
```

**Cleanup.**

- Feed the expired id list into `pcloudc delete-link`
  (see Playbook 23).

**Prevent.**

- Set an organisation-wide default expiry at link creation.
- Schedule this audit as a cron and feed results into the ticketing
  system.

**Escalate.** If the list includes links owned by other accounts,
shares permissions are misconfigured — see the shares chapter.

**Related.** Playbook 23, `list-links`, `change-link-expire`.

## Playbook 25: Orphaned upload-link cleanup

**Problem.** Upload links created for one-off receive workflows are
never cleaned up.

**Diagnose.**

```bash
pcloudc list-upload-links
pcloudc --json list-upload-links
pcloudc --field id list-upload-links
```

**Remediate.**

```bash
pcloudc --field id list-upload-links \
  | while IFS= read -r ID; do
        pcloudc delete-upload-link "$ID"
    done
```

**Cleanup.**

- Confirm the target path no longer accepts anonymous uploads.
- Record the deletion batch id in the change log.

**Prevent.**

- Always set an expiry when creating upload links.
- Scope upload links to a dedicated subfolder that can be revoked
  wholesale if abused.

**Escalate.** If an upload link keeps re-appearing despite deletion,
it is being re-created by automation — trace the creator from the
audit log.

**Related.** Playbook 23, `list-upload-links`, `delete-upload-link`,
`create-upload-link`.

## Playbook 26: Nightly GPG snapshot failure

**Problem.** The nightly `backup snapshot-create` cron fails.

**Symptoms.**

- Non-zero exit from the cron entry (Playbook §Nightly cron).
- One of: `gpg: command not found`, `no public key`, `No space left
  on device`.

**Diagnose.**

```bash
which gpg || echo 'MISSING'
gpg --list-keys "$GPG_RECIPIENT"
df -h /var/backups/pcloud-rs
pcloudc --json backup-snapshot-verify "$(ls -t /var/backups/pcloud-rs/*.tar.gpg | head -1)"
tail -200 /var/log/pcloud-rs/backup.log
```

**Remediate.**

```bash
# Case A: gpg missing.
sudo apt install gnupg           # or: dnf install gnupg2 / pacman -S gnupg

# Case B: recipient key absent.
gpg --import /secure/keys/dr-team.pub
gpg --edit-key "$GPG_RECIPIENT" trust quit   # bump to "ultimate" for automation

# Case C: disk full.
pcloudc backup-snapshot-prune --retention-days 7
# or free space manually, then:
pcloudc backup-snapshot-create --gpg-recipient "$GPG_RECIPIENT" --label "manual-$(date -u +%F)"
```

**Cleanup.**

- Re-run `backup-snapshot-verify` on the last good snapshot.
- Update the monitoring alert threshold if disk exhaustion recurs.

**Prevent.**

- Alert at 80% disk use on the backup destination.
- Package `gpg` as a hard dependency of the operator bundle.

**Escalate.** If `snapshot-verify` fails on a newly-created snapshot,
the encryption key material may be compromised — treat as P0.

**Related.** Playbook 27, runbook §Playbook: Vault + store snapshot
backup / restore.

## Playbook 27: Snapshot restore to a fresh host (DR)

**Problem.** The original host is gone (hardware failure, ransomware).
Restore service on a new host from the offsite snapshot.

**Diagnose.**

```bash
# On the new host
which gpg
gpg --import /secure/keys/dr-team.priv
gpg --list-secret-keys
ls -l /secure/recovered/*.tar.gpg
```

**Remediate.**

```bash
# 1. Install pcloud-rs at the same major version as the snapshot.
# 2. Verify the artefact before touching state.
pcloudc backup-snapshot-verify /secure/recovered/2026-04-14.tar.gpg

# 3. Stop any running daemon, then restore.
systemctl --user stop pcloudd 2>/dev/null || true
pcloudc backup-snapshot-restore /secure/recovered/2026-04-14.tar.gpg --yes

# 4. Start and validate.
systemctl --user start pcloudd
pcloudc status
pcloudc audit-verify
```

**Cleanup.**

- Rotate the account password (the restored vault assumes prior
  credential hygiene).
- Trigger a fresh `backup-snapshot-create` on the new host — DR you
  have not verified is DR you do not have.

**Prevent.**

- Run a DR drill quarterly.
- Keep private keys in an HSM or dedicated keyring; never on the
  origin host.

**Escalate.** If `audit-verify` reports a chain break, the restore is
partial — do not accept it as steady state.

**Related.** Playbook 26, Playbook 33 (audit chain break).

## Playbook 28: Integrity sweeper fire — investigate a mismatch

**Problem.** The sweeper reported an `integrity.mismatch` audit row
overnight.

**Diagnose.**

```bash
pcloudc integrity-status
pcloudc --json integrity-status
pcloudc --field mismatches_found --field files_hashed integrity-status
pcloudc audit-verify
grep '"category":"integrity.mismatch"' /var/log/pcloud-rs/audit.jsonl | tail
```

**Remediate.**

```bash
# Identify the affected sync root by the path-HMAC in the audit row
# (consult your path-HMAC -> path mapping table; the cleartext path is
# never in logs by design).
pcloudc verify <SYNC_ROOT> --recursive
pcloudc --json verify <SYNC_ROOT> --recursive > /tmp/verify.ndjson
grep -v '"kind":"ok"' /tmp/verify.ndjson

# If the local is bad: re-download.
rm -f <bad-local-file>
pcloudc sync-localscan

# If the server is bad (rare): restore from snapshot, then:
pcloudc verify <SYNC_ROOT> --recursive
```

**Cleanup.**

- File an incident ticket citing the audit row id.
- Re-run `integrity-run-once` to prove steady state.

**Prevent.**

- Keep the sweeper enabled with `rate_files_per_minute` sized to your
  I/O budget (Playbook 29).
- Monitor `audit_drops` — non-zero means audit writes are failing and
  the invariant is broken (see Playbook 33).

**Escalate.** If mismatches recur on the same path, the upstream
producer is non-deterministic — escalate to the data-owning team.

**Related.** Playbook 29, Playbook 33, runbook §Verifying
local-vs-server integrity.

## Playbook 29: Integrity sweeper pinning CPU

**Problem.** The integrity sweeper is saturating a core and causing
I/O contention.

**Diagnose.**

```bash
pcloudc integrity-status
pcloudc --field throttled --field files_hashed integrity-status
top -p "$(pgrep -f pcloud-daemon)"
iostat -xm 5 3
```

**Remediate.**

```toml
# config.json — reduce the sweeper rate and pause on battery.
[profile.features.integrity_sweeper]
rate_files_per_minute = 15
pause_on_battery      = true
```

```bash
systemctl --user restart pcloudd
pcloudc integrity-status
```

**Cleanup.**

- Consider adding large static trees to the skip list:
  `pcloudc integrity-skip <path>`.

**Prevent.**

- Size `rate_files_per_minute` relative to disk IOPS: ~60 is fine on
  NVMe, ~15 is safer on spinning disks.
- Use `pause_on_battery=true` for laptop deployments.

**Escalate.** If CPU stays pinned at `rate_files_per_minute=1`, the
hashing loop is stuck — attach a `doctor --json` and a perf capture.

**Related.** Playbook 28.

## Playbook 30: Crypto folder locked — cannot unlock

**Problem.** `pcloudc unlock-crypto` fails repeatedly after a password
rotation.

**Symptoms.**

- `crypto.start.failed` events with `invalid password`.
- `pcloudc crypto-status` shows `locked`.

**Status.** `change_crypto_pass` and `send_change_user_private` parity
is still pending (see CLAUDE.md). Rotate via the web UI and unlock with
the new password locally.

**Diagnose.**

```bash
pcloudc crypto-status
pcloudc --json crypto-status
pcloudc --field phase crypto-status
```

**Remediate.**

```bash
# 1. Confirm the current active password in the web UI.
# 2. Re-attempt unlock via stdin prompt (no argv):
pcloudc unlock-crypto
# 3. If the correct password still fails, lock and retry from scratch:
pcloudc lock-crypto
pcloudc unlock-crypto
```

**Cleanup.**

- Record which password generation is now active in your secret store.
- Rotate sharing-related temppass flows if key material changed.

**Prevent.**

- Track crypto password rotations in a change log; keep one password
  generation back, securely stored, for 72 h.

**Escalate.** Persistent failure with the confirmed-correct password
points at a stale master-key cache. Follow `runbook.md` Playbook 8 and
file a `bd-1du` bead if the state survives a clean restart.

**Related.** Playbook 8, `unlock-crypto`, `lock-crypto`, `crypto-status`.

## Playbook 31: Corrupted crypto sector — recovery

**Problem.** A crypto folder file reports a decryption failure on read.

**Status.** Forward-looking. A first-class `crypto-repair` CLI is not
in tree yet. Today: treat as a restore scenario.

**Diagnose.**

```bash
pcloudc crypto-status
pcloudc --json crypto-status
pcloudc verify /path/inside/crypto/folder
```

**Remediate.**

```bash
# 1. Unlock the folder.
pcloudc unlock-crypto

# 2. If a recent GPG snapshot exists, restore that file out-of-band
#    via snapshot-verify + manual extract (do NOT run snapshot-restore
#    for a single file — that is destructive of the whole state dir).
pcloudc backup-snapshot-verify /var/backups/pcloud-rs/<date>.tar.gpg

# 3. If no good copy exists locally, pull from the web UI revision
#    history, drop the plaintext back into the crypto folder, then:
pcloudc sync-localscan
pcloudc verify /path/inside/crypto/folder    # expect: 0 Ok
```

**Cleanup.**

- Open an integrity ticket — a single corrupted sector is usually a
  signal of wider media trouble.
- `smartctl -a` on the underlying disk.

**Prevent.**

- Keep the integrity sweeper on.
- Use ECC memory / filesystem checksums (ZFS/Btrfs) for crypto-folder
  hosts.

**Escalate.** Multiple sectors corrupted on the same host: P0.
Preserve the state dir as a forensic snapshot before any remediation.

**Related.** Playbook 28, Playbook 30.

## Playbook 32: Correlating a client trace to a daemon span

**Problem.** A user reports a failure. You need to find the matching
daemon span quickly.

**Diagnose.**

```bash
# The user or your repro emitted a traceparent line like:
# [trace: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01]
TRACE_ID=4bf92f3577b34da6a3ce929d0e0e4736

# 1. Search in your OTLP backend for that trace id — you should see:
#    pcloudc.command -> pcloudd.dispatch -> pcloudd.backend.<name>
#    -> pcloud.proto.<method>

# 2. Force a fresh, sampled repro with the same id:
pcloudc --trace-id "$TRACE_ID" status
pcloudc --trace-id "$TRACE_ID" --json status
```

**Remediate.**

- Work the hop where the span chain stops. The backend span carries
  `error_category` — read it first.

**Cleanup.**

- If the trace shows a client-side-only failure, update the CLI
  troubleshooting doc.

**Prevent.**

- Ensure `[observability.tracing]` is enabled on all operator hosts.
- Instruct users to always include the `[trace: ...]` line in tickets.

**Escalate.** If the OTLP backend cannot find a supplied trace id that
the CLI definitely printed, the collector is dropping spans — check
`audit_drops` and collector egress.

**Related.** Runbook §Traceparent correlation.

## Playbook 33: Prometheus exporter not scraping

**Problem.** Your Prometheus scrape job reports `DOWN` for the daemon.

**Diagnose.**

```bash
ss -ltnp | grep pcloud
curl -s http://127.0.0.1:9301/metrics | head
pcloudc --json status | grep -i prom
```

**Remediate.**

```toml
# config.json — enable and bind the exporter.
[observability.prometheus]
enabled = true
listen  = "127.0.0.1:9301"
mode    = "restricted"
```

```bash
systemctl --user restart pcloudd
curl -s http://127.0.0.1:9301/metrics | head
```

**Cleanup.**

- Re-check the Prometheus scrape job shows the target as `UP`.
- Confirm firewall allows the Prom collector host.

**Prevent.**

- Pin the exporter port in the deployment template.
- Alert on exporter scrape-age > 2 minutes.

**Escalate.** If the exporter binds but returns empty metrics, the
collector registry was not populated — attach startup log and open a
bead.

**Related.** Playbook 35 (audit hash-chain break).

## Playbook 34: Audit log hash-chain break

**Problem.** `pcloudc audit-verify` reports a chain break.

**Symptoms.**

- `audit.chain.broken` event.
- `audit_drops` counter non-zero.

**Diagnose.**

```bash
pcloudc audit-verify
pcloudc --json audit-verify
pcloudc --field ok --field break_row_id audit-verify
df -h "$(dirname /var/log/pcloud-rs/audit.jsonl)"
```

**Remediate.**

```bash
# 1. Stop the daemon.
systemctl --user stop pcloudd

# 2. Preserve the current audit log as evidence.
mv /var/log/pcloud-rs/audit.jsonl \
   /var/log/pcloud-rs/audit.jsonl.broken-$(date +%s)

# 3. Let the daemon start a fresh chain anchored on an identity row.
systemctl --user start pcloudd
pcloudc audit-verify                    # expect: ok=true on fresh chain
```

**Cleanup.**

- File a P1 incident with the preserved broken file.
- Confirm the underlying disk is healthy (`smartctl`, `df`).

**Prevent.**

- Alert on `audit_drops > 0`.
- Move the audit sink to a durable volume with plenty of free space.
- Keep the scheduled audit-chain verifier enabled (the default). It
  runs at 03:00 daily and emits `audit.chain.verified` on success or
  `audit.chain.broken` on failure. Check its status at any time:

  ```bash
  pcloudc audit-verifier status
  pcloudc --json audit-verifier status
  ```

  The status payload reports `enabled`, `last_result` (`never_run`,
  `pass`, or `fail`), `chain_length`, `total_passes`,
  `total_failures`, `last_error`, and `last_run_ts`. Configure the
  schedule and optional checkpoint path in `config.json`:

  ```toml
  [features.audit_verifier]
  enabled = true                       # default
  schedule_cron = "0 0 3 * * *"        # 03:00 daily (6-field cron)
  checkpoint_path = "/var/lib/pcloud-rs/audit_verifier_checkpoint.json"
  ```

  The checkpoint lets subsequent runs skip already-verified rows.
  Disabling the verifier removes the only automatic tamper-detection
  path; operators who opt out must arrange out-of-band verification.

**Escalate.** A chain break with zero `audit_drops` and healthy disk
is a code-level invariant failure: P0, attach the broken file and open
`bd-1du.10`-scope bead.

**Related.** Playbook 27, runbook §Verifying local-vs-server
integrity.

## Playbook 35: Graceful drain during `pcloud-daemon` restart

**Problem.** A restart must preserve in-flight transfers and
mount state without visible disruption.

**Diagnose.**

```bash
pcloudc --field in_flight_uploads --field in_flight_downloads status
pcloudc --json status
pcloudc pending
```

**Remediate.**

```bash
# 1. Announce drain — stop accepting new work.
pcloudc pause

# 2. Wait for the drain to reach zero.
until [ "$(pcloudc --field in_flight_uploads status)" = "0" ] \
   && [ "$(pcloudc --field in_flight_downloads status)" = "0" ]; do
     sleep 2
done

# 3. Graceful restart — journal replay resumes any pending items.
systemctl --user restart pcloudd

# 4. Un-pause after the new daemon reports ready.
pcloudc --field ready doctor
pcloudc resume
```

**Cleanup.**

- Confirm `pcloudc pending` drains after resume.
- Verify SLO metrics recovered in Prometheus.

**Prevent.**

- Wire the pause/drain/resume dance into your deployment tooling so
  every restart uses it.

**Escalate.** If the drain loop does not reach zero within 5 minutes,
a stuck transfer is likely — inspect `pending` and move to
Playbook 18 / 19 / 20.

**Related.** Playbook 18, Playbook 19, Playbook 20, `pause`, `resume`.

## Playbook 36: Migrating from legacy C `pcloud-rs`

**Problem.** A host still runs the legacy C `pcloud-rs` and must move
to the Rust daemon without losing sync state.

**Status.** `pcloudc migrate-from-c` is CLI-wired. Scope: copies
config / vault / store from a reasonable legacy layout. Always run
with `--dry-run` first; parity gaps are tracked under `bd-1du.10`.

**Diagnose.**

```bash
# 1. Inspect what migrate-from-c will do.
pcloudc migrate-from-c --dry-run
pcloudc --json migrate-from-c --dry-run
pcloudc migrate-from-c --dry-run --from /legacy/pcloud
```

**Remediate.**

```bash
# 2. Stop the legacy client.
pkill -TERM -f pcloud-rs-legacy || true

# 3. Run the real migration. --force-overwrite if you are confident.
pcloudc migrate-from-c --from /legacy/pcloud
# or:
pcloudc migrate-from-c --from /legacy/pcloud --force-overwrite

# 4. Start the new daemon and validate.
systemctl --user start pcloudd
pcloudc status
pcloudc sync-list
pcloudc audit-verify
```

**Cleanup.**

- Archive `/legacy/pcloud` to an encrypted offsite location for 30
  days before deletion.
- Decommission the legacy package.

**Prevent.**

- Pin a migration runbook owner so every host migrates through the
  same checklist.

**Escalate.** If `sync-list` on the new daemon is empty after
migration, the legacy layout did not match the migrator's assumptions
— attach the `--dry-run` output to a bead.

**Related.** Playbook 2 (upgrade), Playbook 37 (schema bump),
`migrate-from-c`.

## Playbook 37: Upgrading through a schema bump

**Problem.** A release contains a store schema bump. The daemon
applies an online migration at first start.

**Status.** Store migrations are forward-only; there is no rollback
path. Always snapshot before upgrading through a bump.

**Diagnose.**

```bash
pcloudc --version                                  # pre-upgrade
pcloudc --json status > /tmp/pre-schema.json
pcloudc backup-snapshot-create --label "pre-schema-$(date -u +%F)"
pcloudc backup-snapshot-verify "$(ls -t /var/backups/pcloud-rs/*.tar.gpg | head -1)"
```

**Remediate.**

```bash
# 1. Graceful drain and restart with the new binary.
pcloudc pause
systemctl --user stop pcloudd
# ...install new binary...
systemctl --user start pcloudd

# 2. Watch the migration land.
journalctl --user -u pcloudd -f | grep -E 'store\.migration\.(applied|failed)'

# 3. Validate.
pcloudc --field ready doctor
pcloudc status
pcloudc audit-verify
pcloudc resume
```

**Cleanup.**

- Confirm `store.migration.applied` was logged for every expected step.
- Keep the pre-schema snapshot for 14 days minimum.

**Prevent.**

- Stage upgrades on a canary host first (see [Deployment](./deployment.md)).
- Never skip the pre-upgrade snapshot.

**Escalate.** If `store.migration.failed` fires, stop the world: do
**not** roll the binary back (forward-only). Restore the snapshot
onto a fresh state dir and retry with the previous binary. Open a
P0 bead.

**Related.** Playbook 2, Playbook 3, Playbook 27, runbook §Playbook:
Vault + store snapshot backup / restore.

## Playbook: Choosing the right sync flavor

**When.** Before registering a sync root, or when you realise the
direction you picked no longer fits the workflow.

**Scope.** `sync add --type <FLAVOR>`, `sync change-type`,
`backup snapshot-create` — pick one per pairing.

**Decision matrix.**

| Workflow | Flavor | CLI |
|---|---|---|
| Interactive workstation (bi-directional work) | `bilateral` / `full` / `both` | `pcloudc sync add ~/work /Work` (default) |
| Read-only replica of a shared folder | `mirror` / `download-only` / `down` / `remote-to-local` | `pcloudc sync add ~/shared /Shared --type mirror` |
| One-way local-to-remote push (non-deletion-safe) | `backup` / `upload-only` / `up` / `local-to-remote` | `pcloudc sync add ~/push /Push --type backup` |
| Deletion-safe archival (GPG-encrypted, content-addressed) | — (not a sync root) | `pcloudc backup snapshot-create ~/archive --gpg-recipient you@domain` |

**Honest caveat.** In the current pre-alpha implementation, the
`backup` alias is a synonym for `upload-only` and DOES propagate local
deletions to the remote. A true deletion-safe backup flavor is tracked
under `bd-1du.5 Deletion-safe backup sync flavor`. Until that lands,
reach for `backup snapshot-create` (nightly GPG tarballs) when you
actually need "local delete never deletes the remote copy".

**Validation.** After `sync add`:

```bash
pcloudc --field sync_id --field sync_type sync add /l /r --type mirror
# sync_id=7
# sync_type=DownloadOnly
```

**Related.** Playbook: Converting an existing bilateral sync to
mirror-only (below); `backup snapshot-create` in the Backup Snapshots
chapter.

## Playbook: Converting an existing bilateral sync to mirror-only

**When.** You started with a bilateral sync but the local side has
diverged and you only want the remote as the source of truth going
forward (local will be overwritten with the remote tree, new local
edits will no longer be uploaded).

**Scope.** `sync change-type`. No re-add required; `sync_id`,
remote-folder binding, and staging context are preserved.

**Preflight (data safety).**

```bash
# 1. Identify the sync id you want to flip.
pcloudc --json sync list | jq '.result.sync_roots[] | {sync_id, local_path, sync_type}'

# 2. Snapshot the local tree before flipping — download-only will
#    happily remove local files that the remote side has dropped.
tar --zstd -cf "/tmp/pre-flip-$(date +%F).tar.zst" ~/work

# 3. (optional) Pause sync so nothing runs while you flip.
pcloudc pause
```

**Apply.**

```bash
# 4. Flip to mirror.
pcloudc sync change-type 7 mirror
# sync root 7 sync type changed: full -> download-only

# 5. Resume.
pcloudc resume
```

**Verify.**

```bash
# 6. Re-inspect.
pcloudc --json sync list | jq '.result.sync_roots[] | select(.sync_id==7)'
# { "sync_id": 7, "sync_type": "DownloadOnly", ... }

# 7. Watch the next reconcile cycle.
pcloudc status --watch
```

**Rollback.** If you change your mind, flip back:

```bash
pcloudc sync change-type 7 bilateral
```

Flipping does not re-upload any previously pruned local files — the
daemon only has the current state of both sides to work with. Restore
from the tarball you captured in step 2 if you lost data.

**Related.** Playbook: Choosing the right sync flavor (above); CLI
reference `sync-change-type`.

## Playbook: Responding to SLO violations

The daemon publishes a canonical SLO report under `GET /slo` (HTTP
exporter) and `Method::GetSlo` (IPC). This playbook walks through the
triage path when an alert fires on a `violation` status.

### 0. Pull the live report

```bash
# HTTP exporter (feature = metrics, loopback by default)
curl --unix-socket $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock http://localhost/slo | jq

# IPC — same data, same JSON shape (field-selector-friendly)
pcloudc slo
pcloudc --json slo
pcloudc slo pass             # extract aggregate bit
pcloudc slo slos             # extract full list
```

Each entry is `{slo_name, target, actual, status}` with `status` one
of `ok` / `violation` / `no_data`. `no_data` is **not** a breach —
the registry honestly reports when a counter has no samples yet.

### 1. Identify which SLO tripped

| SLO name                                   | Target     | Common causes |
|--------------------------------------------|------------|---------------|
| `ipc.request.latency.p99`                  | `<100ms`   | CPU starvation, blocking lock inside dispatch, GC stall on store mutex |
| `ipc.request.error_rate`                   | `<0.1%`    | Backend connectivity flap, expired auth, malformed requests from a buggy client |
| `auth.login.success_rate`                  | `>99%`     | Password-change event, bad TFA device, upstream rate-limit |
| `upload.throughput_mbps.p50`               | `>5MB/s`   | Uplink saturation, TLS renegotiation, journal contention |
| `mount.read.latency.p99`                   | `<50ms`    | Page-cache miss storm, remote-list hit, FUSE channel stall |
| `integrity_sweeper.run.p95`                | `<5min`    | Sweeper walking an unexpectedly large root, DB vacuum overlap |
| `audit.hash_chain.verify.daily_pass_rate`  | `>99.9%`   | Clock skew, disk corruption, tampering (P0: escalate) |

### 2. Cross-check against raw metrics

Every SLO is backed by a Prometheus family already exposed on
`/metrics`. Confirm the SLO is not firing on a thin sample:

```bash
curl --unix-socket $XDG_RUNTIME_DIR/pcloud-rs/daemon.sock http://localhost/metrics \
  | grep -E 'pcloud_request_latency_seconds|pcloud_auth_attempts_total'
```

A breach that comes with `<100` observations in the 5-minute window
is usually noise from a client test fixture; capture the sample
count before opening a page.

### 3. Per-SLO first action

- **`ipc.request.latency.p99` / `error_rate`** — `pcloudc drain`
  status + `pcloudc doctor` + the last five daemon log lines tagged
  `method=...` identify a stuck handler in under a minute.
- **`auth.login.success_rate`** — check the authsave vault mode
  (`stat -c '%a' ~/.local/share/pcloud-rs/vault*`), confirm the
  session lifecycle clock
  (`pcloudc session status`), and inspect
  `pcloud_auth_attempts_total{result="rate_limited"}`.
- **`upload.throughput_mbps.p50`** — `pcloudc pending`, plus
  `pcloud_transfer_bytes_total{direction="upload"}` as a rate over
  the alert window.
- **`mount.read.latency.p99`** — `pcloudc fs status <path>` to
  confirm the mount is live; then page-cache hit-rate from
  `/metrics`.
- **`integrity_sweeper.run.p95`** — `pcloudc integrity status`
  (`files_hashed` / `bytes_hashed` progress); if `throttled` is
  non-zero, tune the rate-limit in
  `[features.integrity_sweeper]`.
- **`audit.hash_chain.verify.daily_pass_rate`** — treat as P0.
  Run `pcloudc audit verify` by hand; if it reports a broken link,
  stop the daemon and follow *Playbook 13: Persisted token stolen*
  for blast-radius containment while forensics are captured.

### 4. When the breach persists

1. Capture `pcloudc doctor --json`, `pcloudc --json slo`,
   `pcloudc --json status`, the last 500 journal lines, and a
   Prometheus scrape.
2. Confirm the SLO is **not** definitional pre-GA drift (see
   `docs/book/src/architecture/performance.md` — several SLOs are
   aspirational and the current build does not meet all of them
   uniformly under load).
3. Open a bead under `bd-1du.10` with the captured evidence and the
   exact `slo_name` + `actual` string. Do not silence the alert
   until either the bead closes or the SLO threshold is adjusted in
   `crates/pcloud-observability/src/slo.rs` by release review.

### 5. Do not do

- Do not edit `SLO_*_THRESHOLD` constants to silence an alert.
  Threshold changes require release review and a CHANGELOG entry.
- Do not mute `violation` in dashboards without a matching bead.
- Do not confuse `no_data` with `ok` — the registry distinguishes
  them deliberately.
