# Backup Snapshots

## 1. Purpose

The operator-facing manual for the four `pcloudc snapshot` verbs that
implement disaster-recovery (DR) for the Rust daemon:

- `pcloudc snapshot create <path> [--zstd-level N] [--gpg-recipient EMAIL]`
- `pcloudc snapshot verify <path>`
- `pcloudc snapshot restore <path> --yes`
- `pcloudc snapshot prune <dir> --retention-days N --yes`

`pcloudc snapshot` (no subcommand) is shorthand for
`snapshot create`. The legacy two-token forms
(`pcloudc backup snapshot-create|snapshot-restore|snapshot-verify|
snapshot-prune`) still parse for one release cycle and emit a
one-line stderr deprecation warning pointing at the new surface.

### 1.1 Default pipeline (no GPG)

```
tar → zstd (level: --zstd-level, default 3)
    → SHA3-256 over the compressed bytes
    → <archive>.manifest.json sidecar
```

The default pipeline produces two files next to each other on disk:

- `<archive>.tar.zst` — the compressed inner tar.
- `<archive>.tar.zst.manifest.json` — the **sidecar manifest** that
  records the SHA3-256 digest of the final on-disk archive bytes,
  the effective zstd level, the original per-payload SnapshotManifest
  (inner integrity layer, SHA-256 over the four payload entries),
  and an `encrypted: false` flag. `snapshot verify` re-reads both
  layers.

Beginner invocation: `pcloudc snapshot /tmp/today.tar.zst`.

### 1.2 Optional GPG envelope

Set `--gpg-recipient <id>` to add an outer GPG envelope. The archive
path must then end with `.tar.zst.gpg`; compression happens **before**
encryption so the ciphertext is not re-compressible, and the sidecar
SHA3 is computed over the ciphertext (so `snapshot verify` catches
transit tampering without having to decrypt first).

```
pcloudc snapshot create /var/backups/pcloud/today.tar.zst.gpg \
    --gpg-recipient dr-team@example.com
```

GPG is a runtime dependency only when `--gpg-recipient` is used. The
default pipeline needs neither `gpg(1)` nor a keyring.

### 1.3 `--zstd-level` tuning

`--zstd-level <1..=22>`, default `3` (the upstream zstd default).
Rules of thumb:

- `3` — the default; roughly balanced for operational backups.
- `6..=12` — archival with mild size wins.
- `19..=22` — archival with large size wins and materially longer
  wall-time. Useful for cold-storage retention tiers, not for
  minutely RC-soak snapshots.

Out-of-range values are rejected with a clear error (`--zstd-level
must be an integer in 1..=22`).

## 2. Prereqs

- **`gpg(1)` installed and on `$PATH`** on every host that creates,
  verifies, or restores a snapshot. The daemon shells out to the host
  `gpg` binary; there is no in-tree OpenPGP implementation. If
  `which gpg` fails, all four verbs return exit code `6`
  (Unavailable).
- **Recipient public GPG key imported** on every snapshot-creating
  host.
- **Recipient private GPG key accessible only on DR/restore hosts**
  (never on normal workstations).
- **Signer private key** (optional) on each snapshot-creating host;
  recipient signer public key on DR hosts if signature verification
  is required.
- Write access to the destination URL (local path, S3 bucket, SFTP
  server) and sufficient free space (plan ~1.2× the current store
  size — manifest + vault + audit + config + plugin registry).
- Daemon running under the same UID as the vault / store
  (cross-UID operation is rejected at open time).

## 3. Conceptual background

### What goes into the tarball

A snapshot is a **GPG-encrypted reproducible tarball** (`.tar.gpg`):
sorted entries, fixed mtime drawn from the manifest. Contents:

| Entry                          | Source                                               | Notes                                                              |
|--------------------------------|------------------------------------------------------|--------------------------------------------------------------------|
| `manifest.json`                | built at snapshot time                               | version, build id, timestamp, host id, schema versions, BLAKE3s    |
| `vault/auth_vault.bin`         | `crates/pcloud-daemon/src/auth_vault.rs`             | copied byte-for-byte; already encrypted at rest                    |
| `store/store.sqlite3`          | `crates/pcloud-store/` via SQLite online-backup API  | transaction-consistent copy while the daemon runs                  |
| `audit/audit.log` + `.idx`     | `crates/pcloud-observability/`                       | append-only chain; tail hash mirrored into the manifest            |
| `config/config.toml`           | `crates/pcloud-config/`                              | secrets redacted to `keyring:*` refs — never raw material          |
| `plugins/registry.json`        | `crates/pcloud-plugin-api/`                          | manifests + signatures only; plugin binaries are **not** shipped   |

The tarball is encrypted with `gpg --encrypt --sign --recipient
$RECIP`. This yields an asymmetric artefact: the snapshot host needs
only the recipient **public** key; the recipient **private** key is
required only for `snapshot-verify` / `snapshot-restore`.

> **Snapshots contain the auth vault.** Treat the `.tar.gpg`
> equivalent to the vault itself. Do not post it in bug reports, do
> not check it into VCS.

### Why GPG-to-a-recipient (not symmetric)

- **Principle of least privilege.** Backup-creating hosts never touch
  a decryption key. A compromised backup host can exfiltrate only
  ciphertext.
- **Key-rotation is straightforward.** Switch recipients; old
  snapshots stay decryptable as long as the old private key survives.
- **Works with air-gapped DR.** The private key lives on a separate
  administration workstation that does not need daemon access.

### GFS retention rationale

The default retention policy is a **grandfather-father-son** selector:

```toml
[backup.retention]
daily        = 14        # last 14 daily snapshots
weekly       = 8         # last 8 weekly snapshots (first of each ISO week kept)
monthly      = 12        # last 12 monthly snapshots (first of each month kept)
minimum_keep = 3         # never go below this, even if policy says 0
```

Why GFS instead of "last N days":

- **Recovery-point coverage scales with age.** Operator-error
  recovery window is ~2 weeks (dense dailies); long-tail compliance
  recovery window is ~12 months (sparse monthlies) in bounded
  storage.
- **Verification cost stays bounded.** `snapshot-verify` runs per
  snapshot; a bounded generation count keeps nightly verify within
  its budget.
- **`snapshot-prune` never drops the last known-good snapshot.**
  Prune refuses to delete a snapshot that is younger than the most
  recent verified snapshot. An unverifiable current snapshot must
  not cause loss of the last verified one.

### GPG key classes and rotation

| Key                                     | Lives on                  | Purpose                                                     |
|-----------------------------------------|---------------------------|-------------------------------------------------------------|
| `backup.gpg_recipient` public           | every backup-creating host | encrypt-to                                                  |
| `backup.gpg_recipient` private          | DR / restore hosts only    | decrypt + verify during restore                             |
| `backup.gpg_signer` private (optional)  | each signing host          | sign the tarball so recipients can verify origin            |
| `backup.gpg_signer` public              | DR / restore hosts         | verify signature during restore                             |

Rotation:

1. Generate the new key pair (air-gapped DR workstation).
2. Import the new **public** key on all backup-creating hosts; keep
   the old one imported during a transition window.
3. Switch `[backup].gpg_recipient` in `config.toml` to the new
   email.
4. When every snapshot in the retention window is encrypted to the
   new key, remove the old public key from backup hosts.
5. **Archive** the old private key — do not destroy it while any
   snapshot encrypted to it is still in retention.

## 4. Step-by-step procedure

### 4.1 Create

```bash
pcloudc backup snapshot-create /var/backups/pcloud-rs/$(date +%F).tar.gpg \
  --gpg-recipient dr-team@example.com
```

Expected output (parse with JSON selectors when scripting):

```bash
pcloudc --json backup snapshot-create \
  /var/backups/pcloud-rs/$(date +%F).tar.gpg \
  --gpg-recipient dr-team@example.com \
  | jq '{outcome:.outcome, bytes:.size_bytes, manifest_hash:.manifest.blake3}'
# { "outcome": "Created",
#   "bytes": 12345678,
#   "manifest_hash": "b3:..." }
```

### 4.2 Verify (non-mutating)

```bash
pcloudc backup snapshot-verify /var/backups/pcloud-rs/2026-04-16.tar.gpg \
  --gpg-recipient dr-team@example.com
```

Expected:

```bash
pcloudc --json backup snapshot-verify <path> \
  | jq '{outcome, checks: .checks|map(.id), failed: .failed_checks}'
# outcome == "Verified", failed == [],
# checks include: gpg.signature, manifest.hash, payload.hash,
#                 store.integrity, audit.chain, config.redaction
```

### 4.3 Restore (destructive — must `--yes`)

```bash
# Always verify first.
pcloudc backup snapshot-verify /path/to/snap.tar.gpg
pcloudc backup snapshot-restore /path/to/snap.tar.gpg --yes
```

Restore refuses to run if the verify step did not pass within the
same session. Omitting `--yes` prints the plan and exits non-zero —
the daemon never mutates state without explicit operator
confirmation.

### 4.4 Prune (GFS-selective)

```bash
pcloudc backup snapshot-prune /var/backups/pcloud-rs/ \
  --retention-days 14 \
  --gpg-recipient dr-team@example.com \
  --yes
```

- `--retention-days N` overrides only the **daily** slot count;
  weekly / monthly slots come from `[backup.retention]`.
- Omit `--yes` to see the prune plan without deleting.
- `--gpg-recipient` is accepted for recipient-scoped pruning when
  multiple recipients share a destination directory.

## 5. Verification

### Verify-before-restore discipline

`snapshot-verify` is cheap (~seconds on a 1 GiB tarball) and **MUST**
run before every restore. It is a complete dry-run:

1. `gpg --verify` of the detached signature (if signing is enabled).
2. `gpg --decrypt` into a `0700` tempdir (never into the live state
   directory).
3. Re-hash every payload entry against `manifest.json` (BLAKE3).
4. `PRAGMA integrity_check` on the SQLite store copy.
5. Replay the audit chain; confirm the tail hash matches the
   manifest.
6. Confirm `config.toml` carries only `keyring:*` references (no raw
   secret material).

Operational rules:

- Wire `snapshot-verify` into CI so every nightly snapshot is proven
  restorable, **not merely produced**.
- Run `snapshot-verify` immediately before any `snapshot-restore`,
  even if the same artefact was verified the night before — the
  filesystem or network copy in between may have corrupted it.
- **Alert (not warn)** on any verify failure. A single unverifiable
  snapshot is a data point; two in a row is a DR incident.

## 6. Rollback

"Rollback" for snapshots means **undoing a bad restore**. Because
`snapshot-restore` rewrites the live state directory, operators MUST
take a pre-restore safety snapshot:

```bash
# Before any restore, take one more snapshot of the live state.
pcloudc backup snapshot-create \
  /var/backups/pcloud-rs/pre-restore-$(date +%F-%H%M).tar.gpg \
  --gpg-recipient dr-team@example.com

pcloudc backup snapshot-verify /path/to/target-snap.tar.gpg
pcloudc backup snapshot-restore /path/to/target-snap.tar.gpg --yes
```

If the restore went wrong, verify + restore the pre-restore safety
snapshot.

## 7. Tradeoffs / tuning

| Knob                              | Default | Tradeoff                                                                          |
|-----------------------------------|---------|-----------------------------------------------------------------------------------|
| `daily` retention count           | 14      | More dailies = more operator-error recovery but more verify cost.                 |
| `weekly`                          | 8       | Longer weekly tail covers two-month regressions; adds storage.                    |
| `monthly`                         | 12      | 12 months satisfies common compliance windows; drop to 6 in cost-sensitive orgs.  |
| `minimum_keep`                    | 3       | Safety net against an over-aggressive retention config.                           |
| GPG cipher / digest preferences   | gpg-default | Override via `~/.gnupg/gpg.conf`; prefer AES-256 + SHA-512.                    |
| Destination (`local` vs `s3` / `sftp`) | `local` | `local` alone does not satisfy "offsite"; pair with rsync/restic or pick s3/sftp. |

## 8. Common failure modes

1. **`snapshot-create` fails with "recipient unknown".**
   - Cause: `gpg-agent` confinement under systemd loses the cron
     user’s `~/.gnupg`.
   - Fix: set `Environment=GNUPGHOME=/var/lib/pcloud-rs/.gnupg` in the
     unit and import the recipient there.
2. **`snapshot-verify` reports `gpg.signature` failure.**
   - Cause: transport corrupted the artefact, or signer key rotated
     without importing the new public key on the DR host.
   - Fix: re-fetch the artefact, import the current signer public
     key, re-verify. Alert if the failure reproduces.
3. **`snapshot-prune` refuses to delete.**
   - Cause: the candidate snapshot is younger than the most recent
     verified one, or `minimum_keep` has been reached.
   - Fix: run verify on the newer snapshot; if unverifiable, repair
     the backup pipeline rather than relaxing the guard.
4. **SQLite `integrity_check` fails during verify.**
   - Cause: copy taken while another process held a long write
     transaction, or storage silently corrupted the blob.
   - Fix: take a fresh snapshot; file a bead if the pattern repeats.
5. **Exit code `6` (Unavailable).**
   - Cause: `gpg` missing from `$PATH`.
   - Fix: install GnuPG; re-run. This is a blocking dependency.

## 9. Security / compliance notes

- **Snapshots are vault-sensitive.** Handle them with vault-grade
  ops: restricted storage, audited access, rotated recipient keys.
- **No private keys on backup hosts.** Ever. A compromised backup
  host must not yield plaintext.
- **SSE-KMS is not a substitute for GPG.** On S3, server-side
  encryption protects against opaque blob theft inside AWS; the
  operator-held GPG key protects against everything else. Keep both.
- **SFTP host-key pinning is mandatory.** `host_key_fingerprint` is
  required; TOFU is refused by `russh` configuration.
- **Audit log continuity**: the tail hash is recorded in the
  snapshot manifest. A restore **resumes** the chain at the
  manifest tail; snapshots taken mid-incident preserve
  tamper-evidence.
- **Data residency**: S3 destinations honour
  `[data_residency].allowed_regions` — see
  [data-residency](../../../enterprise/data-residency.md). A
  destination that points outside the allowed regions is refused at
  destination open time.

## 10. Offsite replication patterns

`local`-only is not offsite. Pair the `local` destination with one of:

### rsync

```bash
rsync -a --delete --remove-source-files \
  /var/backups/pcloud-rs/ \
  offsite:/backups/pcloud-rs/$(hostname)/
```

Combine with a destination-side **append-only** permissions scheme
(e.g. `chattr +a` on ext4 or WORM-mode on ZFS) to harden against
ransomware overwrites.

### restic

```bash
RESTIC_REPOSITORY=s3:s3.amazonaws.com/dr-bucket/restic \
RESTIC_PASSWORD_FILE=/etc/pcloud-rs/keys/restic.pass \
  restic backup /var/backups/pcloud-rs/
```

restic deduplicates across snapshots; combine with
`restic forget --keep-daily 14 --keep-weekly 8 --keep-monthly 12`
to mirror the GFS policy.

### S3 (native destination)

```toml
[backup]
destination   = "s3://dr-bucket/pcloud-rs/$(hostname)/"
gpg_recipient = "dr-team@example.com"

[backup.s3]
region     = "eu-central-1"
sse        = "aws:kms"
kms_key_id = "alias/pcloud-rs-dr"
```

### SFTP (native destination)

```toml
[backup]
destination   = "sftp://dr@offsite.example.com:/backups/pcloud-rs/"
gpg_recipient = "dr-team@example.com"

[backup.sftp]
host_key_fingerprint = "SHA256:..."
identity_file        = "/etc/pcloud-rs/keys/sftp_ed25519"
```

### Third-party destinations

GCS, Azure Blob, Backblaze B2, etc. are supported via **signed
external plugins** implementing `PluginCapability::BackupDestination`.
A malicious destination plugin only ever sees ciphertext; the
recipient GPG key is chosen by the operator on the
snapshot-creating host.

## 11. Disaster-recovery drill checklist

Run this drill at least quarterly:

- [ ] Identify a non-production host eligible for restore testing.
- [ ] Take a fresh snapshot on a production host.
- [ ] Copy to the DR host via your offsite mechanism.
- [ ] `pcloudc backup snapshot-verify <path>` — capture the JSON
      report.
- [ ] `pcloudc backup snapshot-restore <path> --yes` — capture the
      exit code and elapsed wall-time.
- [ ] Start the restored daemon; `pcloudc doctor --json` must report
      zero error-level checks.
- [ ] Compare audit tail hash against the snapshot’s manifest.
- [ ] File the drill report; update the RTO/RPO table in your
      internal runbook.
- [ ] Tear down the DR host; wipe the restored state.

## 12. Cross-references

- [CLI reference — `backup`](../reference/cli.md#backup-disaster-recovery-snapshots).
- [Runbook](./runbook.md).
- [Disaster-recovery design](../../../enterprise/disaster-recovery.md).
- [Data residency](../../../enterprise/data-residency.md).
- [DLP / audit chain invariants](../../../enterprise/dlp.md).
- [Upgrade](./upgrade.md) — pre-upgrade snapshot guidance.
