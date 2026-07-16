> **Pre-alpha scaffold — not live / not production-verified.** This document
> describes design and unit-tested code that has not been validated against a
> real production deployment. Do not treat it as a shippable capability.
> See `CLAUDE.md` and `docs/enterprise/README.md` for the honesty rules.

# Disaster-Recovery Snapshots

> **Status:** **LANDED** (tracker waves H12a–H12d) — snapshot /
> restore / verify / prune are wired through `pcloud-cli`,
> `pcloud-daemon`, `pcloud-plugin-api` (destination plugins), and
> `pcloud-observability` (audit chain). No new crates.
> **Honest runtime dependency:** the daemon shells out to the host
> `gpg(1)` binary for encrypt/decrypt/sign/verify; there is **no**
> in-tree OpenPGP implementation. If `gpg` is missing, snapshot
> operations fail **closed** at daemon start.

Landed waves:

- **H12a** — IPC request/response variants + CLI token parse for
  `backup snapshot-create|snapshot-verify|snapshot-restore|snapshot-prune`.
- **H12b** — snapshot builder: `BackupGuard`, SQLite online backup,
  reproducible tarball, BLAKE3 manifest.
- **H12c** — GPG encrypt + restore + grandfather-father-son prune.
- **H12d** — daemon dispatch + CLI handlers (`--yes`,
  `--gpg-recipient`, `--retention-days`) + audit events
  (`PluginAuditEvent::SnapshotCreated` /
  `PluginAuditEvent::SnapshotRestored`).
- **Post-H12** — top-level `pcloudc snapshot` surface with zstd +
  SHA3-256 sidecar as the default pipeline and GPG as an optional
  outer envelope. `--zstd-level 1..=22` (default 3) tunes the
  compression ratio. The legacy `backup snapshot-*` aliases remain
  accepted for one release cycle and emit a one-line stderr
  deprecation warning.

## 1. Purpose

Enterprise operators need a documented, automatable way to:

- capture a **consistent point-in-time snapshot** of daemon state
  before a risky upgrade, migration, or GA rollout,
- restore that snapshot on a different host or after a data-loss
  event,
- satisfy BCP / DR controls (ISO 27001 A.5.29, SOC 2 CC 7.5,
  NIS2 Annex I.2.c) requiring **periodic tested recovery
  procedures** — not just "we have backups".

Before this landed, restoring a `pcloud-rs` deployment meant
manually copying the state directory and hoping the schema
aligned. There was no integrity check, no cadence, no retention
policy, and no cross-region replication story. H12 replaces that
with a thin, opinionated snapshot/restore tool.

Snapshots contain secrets (auth vault, encrypted store rows).
Everything downstream treats the artifact as **equivalent to the
vault itself**.

## 2. Threat model

| Threat | Mitigation |
| --- | --- |
| Snapshot artifact stolen at rest | GPG asymmetric encryption to `gpg_recipient`; snapshot host holds public key only; private key never on production host |
| Snapshot artifact tampered in transit | GPG signature; restore refuses unsigned snapshots unless explicit `--allow-unsigned` |
| SQLite inconsistency from `cp` during live write | SQLite **online backup API** (`sqlite3_backup_init`), not filesystem copy; guaranteed transaction-consistent |
| Audit-chain truncation hides past events | Tail hash embedded in manifest; restore re-plays and refuses mismatched chains |
| Snapshot leaks plaintext to a temp directory | Daemon writes staging only under its own 0700 state tmp dir; staging is `shred`-then-unlinked on success |
| Cross-major-version restore corrupts schema | Explicitly refused (§11); operator must roll forward via normal migration path |
| Destination bucket outside residency boundary | Config validation refuses destination regions outside `data_residency.allowed_regions` (see `data-residency.md`) |
| `gpg(1)` binary missing on production host | Daemon refuses to start the backup subsystem at launch time; no snapshots "silently succeed" into unencrypted tarballs |
| Retention policy deletes last known-good snapshot | Prune refuses to drop any snapshot younger than the most recent **verified** snapshot, and enforces a `minimum_keep` floor |

Explicit **non-threats** (non-goals, §11): continuous
replication, per-file restore from pCloud, and cross-major-version
restore.

## 3. Scope

In scope, landed:

- tarball packager with reproducible byte-order + fixed mtime,
- SQLite online backup,
- BLAKE3 per-entry manifest + audit-tail hash,
- GPG encrypt + sign,
- `local` / `s3` / `sftp` built-in destinations,
- grandfather-father-son retention,
- `pcloudc backup {snapshot-create,snapshot-verify,snapshot-restore,
  snapshot-prune}` CLI,
- `[backup]` config schema + validation,
- nightly CI job exercising `create → verify → restore` into a
  scratch state dir.

Out of scope:

- continuous replication (use a block-level DR stack for that),
- per-file restore from pCloud (use pCloud's server-side file
  history),
- cross-major-version restore (forbidden),
- in-tree OpenPGP (runtime dependency on `gpg(1)` is intentional).

## 4. Design

### 4.1 Snapshot contents

A snapshot is a tarball containing:

- `manifest.json` — version, daemon build id, timestamp, host id,
  schema versions, BLAKE3 of each payload entry, audit chain tail
  hash.
- `vault/auth_vault.bin` — the existing owner-only vault (it is
  already encrypted at rest; copied byte-for-byte).
- `store/store.sqlite3` — produced via `sqlite3_backup_init`,
  **not** `cp`. Transaction-consistent even with the daemon
  running.
- `audit/audit.log` + `audit/audit.idx` — append-only audit
  chain. Tail hash embedded in manifest so truncation is
  detectable across generations.
- `config/config.toml` — operator config with secrets redacted to
  `keyring:*` refs (never raw material).
- `plugins/registry.json` — plugin manifests and signatures;
  plugin **binaries** are not snapshotted (operator manages them
  like any other package).

The tarball is then GPG-encrypted (`--encrypt --sign --recipient
$RECIP`) producing an asymmetric artifact: snapshot host needs
only the public key, restore needs the private key — exactly the
DR posture you want.

### 4.2 Destinations

Destinations are plugin-driven. Two new `PluginOperation`
variants:

```rust
PluginOperation::BackupPut { blob_id, bytes }     // streaming
PluginOperation::BackupGet { blob_id }            // streaming
```

Capability: `PluginCapability::BackupDestination`. Built-in
internal plugins:

- `local` — filesystem path, 0600 artifact, parent 0700.
- `s3` — `aws-sdk-s3`; honours SSE-KMS, bucket residency.
- `sftp` — `russh`.

Third-party (GCS, Azure Blob, Backblaze B2) arrive as signed
external plugins without touching daemon code.

### 4.3 Retention (grandfather-father-son)

Default: `daily 14 / weekly 8 / monthly 12`. Evaluated by
`pcloudc backup snapshot-prune`, which is idempotent and safe to
re-run. Prune never deletes a snapshot younger than the most
recent **verified** snapshot, and always keeps at least
`minimum_keep` (default 3) snapshots regardless of other rules.

## 5. Interfaces

### 5.1 IPC surface

```
backup snapshot-create  --label  --destination  --gpg-recipient  --yes
backup snapshot-verify  <artifact>
backup snapshot-restore <artifact> --state-dir  [--allow-unsigned]
backup snapshot-prune   [--retention-days N]
backup list
backup destinations     # configured destinations + health check
```

CLI aliases: older `create-snapshot` / `restore` / `verify` /
`prune` verbs are aliased to the canonical `snapshot-*` tokens.

### 5.2 Audit events

Landed variants:

```rust
PluginAuditEvent::SnapshotCreated  { label, destination, size, digest }
PluginAuditEvent::SnapshotRestored { source, host, audit_tail_before, audit_tail_after }
```

Chain links cover each record so post-hoc edits are detectable.
Quarantine events additionally record the quarantine path and
the sidecar digest.

## 6. Configuration

```toml
[backup]
enabled = true
destination = "s3://dr-bucket/pcloud-rs/$(hostname)/"
gpg_recipient = "dr-team@example.com"
gpg_signer = "host-key@example.com"   # optional
retention_days = 14                    # shorthand for [backup.retention].daily
verify_on_create = true
max_artifact_bytes = 1073741824        # 1 GiB sanity cap

[backup.retention]
daily = 14
weekly = 8
monthly = 12
minimum_keep = 3

[backup.s3]
region = "eu-central-1"
sse = "aws:kms"
kms_key_id = "alias/pcloud-rs-dr"
```

Validation (fail-closed):

- `gpg_recipient` must resolve to a non-expired public key on the
  daemon host's keyring.
- `destination` region must satisfy
  `data_residency.allowed_regions` when residency is enabled.
- `verify_on_create = true` (default) makes every successful
  `snapshot-create` run `snapshot-verify` against the produced
  artifact before reporting success — **DR you haven't tested is
  DR you don't have.**
- `gpg(1)` must be on `PATH`. If missing, the backup subsystem
  refuses to start and logs
  `backup.subsystem.disabled.gpg_missing`. Packaging manifests
  must list `gpg` as a declared runtime dependency.

## 7. Onboarding

**Minimal operator walkthrough:**

1. Install `gpg(1)` on the daemon host; confirm with
   `gpg --version`.
2. Generate or import the DR recipient public key into the daemon
   user's keyring. Never copy the private key onto the production
   host.
3. Set `[backup]` per §6, pointing `destination` at a bucket/
   directory satisfying the residency policy.
4. Run `pcloudc backup snapshot-create --label smoke` once
   manually. Confirm a `PluginAuditEvent::SnapshotCreated` lands
   in the audit log.
5. Run `pcloudc backup snapshot-verify <artifact>`. This is the
   moment you prove you can restore — don't skip it.
6. Install the systemd timer / launchd job / Windows Task
   Scheduler unit from `docs/enterprise/examples/`.

### 7.1 systemd timer

```ini
# /etc/systemd/system/pcloud-rs-backup.service
[Service]
Type=oneshot
User=pcloud-rs
ExecStart=/usr/bin/pcloudc backup snapshot-create --label "$(date -u +%%F)"

# /etc/systemd/system/pcloud-rs-backup.timer
[Timer]
OnCalendar=*-*-* 02:15:00
RandomizedDelaySec=15m
Persistent=true
```

### 7.2 launchd (macOS)

```xml
<key>Label</key><string>ai.pcloud.backup</string>
<key>ProgramArguments</key>
<array>
  <string>/usr/local/bin/pcloudc</string>
  <string>backup</string><string>snapshot-create</string>
</array>
<key>StartCalendarInterval</key>
<dict><key>Hour</key><integer>2</integer><key>Minute</key><integer>15</integer></dict>
```

### 7.3 Windows Task Scheduler

Sample XML shipped under `docs/enterprise/examples/`. The daemon
runs per-user as `pcloudd.exe`, normally started by `pcloudc start`; the task
invokes `pcloudc.exe backup snapshot-create`. The public package deliberately
does not install the experimental `pcloud-daemon-win` SCM host because the
named-pipe, DPAPI, and WinFSP security boundary is the interactive user SID.

## 8. Verification

Landed tests:

- **Integration tests** (H12b/c/d) using `pcloud-mockserver` and
  a local GPG keyring fixture cover: create, verify,
  round-trip restore, tamper detection, retention pruning.
- **Nightly CI** exercises full `create → verify → restore` into
  a scratch state-dir on every main-branch push.
- **Tamper-detection** test mutates one byte in a packed
  artifact; `snapshot-verify` returns non-zero.
- **Reproducible tarball** test runs snapshot twice against
  identical state and checks byte-equal artifacts (minus GPG
  salt).

The pre-flight invariant for every production deployment is that
`snapshot-verify` runs **nightly** against the latest artifact
and alerts on failure. An untested snapshot is not a backup.

## 9. Failure modes

| Failure | Behaviour |
| --- | --- |
| `gpg(1)` missing at startup | Backup subsystem disabled; daemon logs `backup.subsystem.disabled.gpg_missing`; other functionality unaffected |
| `gpg` returns non-zero during create | Snapshot aborted; staging `shred`-ed; no partial upload to destination |
| SQLite online backup busy-loops | Retries with backoff; after configured deadline, snapshot aborts with `BackupBusy` |
| Destination plugin returns error mid-stream | Snapshot aborted; destinations receive the full artifact or nothing (no partial artifacts land) |
| Manifest BLAKE3 mismatch during verify/restore | Verify fails non-zero; restore refuses and preserves the existing state dir |
| Audit-tail hash mismatch on restore | Restore refuses; operator must reconcile the chain manually |
| Restore target schema too old | Restore refused (`SchemaTooOld`); operator rolls forward the snapshot on a newer host |
| Restore target daemon major version differs | Restore refused (`MajorVersionMismatch`); intentional |
| Daemon fails to start after restore | Operator runs `pcloudc backup rollback` to swap in the `state-dir.pre-restore.<ts>` directory created atomically during restore |
| Prune would drop the only verified snapshot | Refused; `minimum_keep` and "no younger than last verified" floors apply |

## 10. Honest limitations

pre-alpha reality check:

- **`gpg(1)` is a hard runtime dep.** No in-tree OpenPGP. This is
  intentional — the security boundary is the OS gpg-agent — but
  packagers must ship `gpg` as a declared dependency.
- **No continuous replication.** Point-in-time DR only.
- **No per-file restore from pCloud.** Out of scope; use
  pCloud's server-side file history.
- **Cross-major-version restore forbidden.** Operators must roll
  forward through normal migration.
- **Plugin binaries not snapshotted.** Operators manage plugin
  binaries like any other package; the registry manifest is
  snapshotted so the expected set is reproducible.
- **GPG private key workflow is on the operator.** Daemon never
  reads a private key; decryption runs under the invoking user's
  gpg-agent during restore/verify.

## 11. Extension points

- **New destinations** — implement `BackupPut` / `BackupGet` in a
  signed external plugin with `PluginCapability::
  BackupDestination`. Built-ins are the reference implementation.
- **New retention policies** — the GFS selector is a pure
  function of `(snapshots, now, policy)`; swap the policy
  closure.
- **Alternate signing** — today: GPG. The tarball format is
  signing-agnostic; a Sigstore/Cosign signer could be added
  without touching the packager.
- **Alternate hash** — BLAKE3 is the manifest hash; upgrading is
  a manifest-schema change, not an artifact-format change.
- **KMS-owned encryption key** — swap the GPG recipient for a
  KMS-backed key (see `kms.md`). Not landed.

## 12. Cross-refs

Code:

- `crates/pcloud-daemon/src/backup/` — snapshot builder, GPG
  driver, prune selector.
- `crates/pcloud-daemon/src/auth_vault.rs` — vault copied into
  snapshots.
- `crates/pcloud-store/` — `store.sqlite3` source of truth.
- `crates/pcloud-observability/` — audit chain; tail hash
  embedded in manifest.
- `crates/pcloud-secret/` — secret refs redacted in snapshotted
  config.
- `crates/pcloud-config/` — `[backup]` section + validation.

Related docs:

- `docs/book/src/operations/runbook.md` — runbook with step-by-
  step restore.
- `docs/book/src/operations/backup-snapshots.md` — operator
  reference.
- `docs/book/src/reference/cli.md` — `backup` subcommands.
- `docs/enterprise/data-residency.md` — destination region
  enforcement.
- `docs/enterprise/dlp.md` — audit-chain invariants that restore
  must preserve.
- `docs/enterprise/ha.md` — Tier 2/3 fail-over assumes the store
  is restorable via this mechanism if the journal is lost.
- `docs/enterprise/kms.md` — future swap of GPG recipient for
  KMS-wrapped key.
