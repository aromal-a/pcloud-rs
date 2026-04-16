# Integrity Sweeper (Background Scrub) — Design Note

Tracking bead: `bd-1du.4.6.1` (additive over the C surface — no parity
matrix row flip).
Status: H14d wired end-to-end at IPC + CLI; background worker thread is
a placeholder that currently spawns on the dispatch thread. See
*H14 status summary* at the bottom of this file.

## What the sweeper will do

The integrity sweeper is a **background** worker that periodically walks
the set of files the daemon claims authoritative knowledge of (synced
folders, mounted-drive cache, crypto-folder metadata) and verifies that
each file's on-disk content still matches the daemon's recorded
expectation: size, mtime, and — when a content hash is available from
the protocol layer or the encryption sector tables — a recomputed
content hash.

Its job is to detect silent corruption that the active read/write paths
will not notice on their own:

- bit-rot on long-lived staged files in the writeback queue,
- content drift on locally-cached crypto-folder metadata,
- truncation / partial writes after a crash that escaped journal
  replay,
- unexpected modification by something outside the daemon's control
  (an external editor touching a synced file behind our back, malware
  scanners quarantining and restoring),
- inode-reuse confusion after a `rsync` / restore-from-backup operation
  that re-creates a file with the same path but a fresh content hash.

When the sweeper finds a divergence it does **not** silently "fix"
anything. It records an audit event (path-hash-only, see Privacy below),
optionally flags the file for re-verification on next access, and
surfaces a structured report through the IPC control surface (PR3).

## Why this is not `pcloudc verify`

`pcloudc verify` is an operator-driven, foreground, on-demand command:
the user runs it, it walks a path, it prints results, it exits. It is
synchronous, observable, and expected to interfere with normal daemon
work for as long as the operator is willing to wait.

The integrity sweeper is the opposite axis:

| axis             | `pcloudc verify`                | integrity sweeper                  |
|------------------|---------------------------------|------------------------------------|
| trigger          | operator command                | scheduled / on-demand via IPC      |
| lifetime         | foreground, exits               | long-lived background worker       |
| observability    | stdout to operator              | audit log + IPC status surface     |
| performance bias | "finish fast"                   | "stay invisible" (rate-limited)    |
| failure mode     | non-zero exit                   | structured event, no exit          |
| scope            | what the operator typed         | configured global scope            |

Conflating the two would either turn `verify` into a daemon (bad — it
should remain a leaf command) or turn the sweeper into a synchronous
RPC (bad — it must not block the IPC client for an hour).

They share a verification primitive in the engine (the per-file
"recompute and compare" routine) but their lifecycles, configuration,
and reporting paths are distinct.

## Privacy posture

The sweeper crosses a privacy line that `verify` does not: it runs
without a human in the loop, and any data it persists (audit events,
restart-resume cursors) lives on disk between reboots.

Rules:

- **Path-hash only in audit events.** The audit event records a
  HMAC-SHA256 of the path under a per-installation key, never the
  cleartext path. Operators with vault access can reverse the hash
  for a known path; a leaked audit log alone reveals nothing about
  which files diverged.
- **Skip-list is on disk, not in audit events.** When a file is
  excluded by `skip_list_path`, no audit event is emitted for it.
  Skipped files do not appear in the divergence report either.
- **No content excerpts.** The sweeper never logs file contents,
  byte ranges, or partial hashes that could leak content. Only the
  binary "matched" / "diverged" bit and the recomputed content hash
  (which is already known to the protocol layer) are recorded.
- **Battery and on-AC posture.** `pause_on_battery = true` is the
  default so a laptop on battery does not silently grind through the
  sync set. The flag is wired today and becomes load-bearing once the
  battery-detection facade lands.

## Rate limiting

The sweeper consumes one token per file via [`RatedTokenBucket`].
`rate_files_per_minute` (default `100`) caps the steady-state file
inspection rate. A value of `0` permanently disables work. Tokens
accrue at `rate / 60` per second up to the configured per-minute
capacity, so short bursts (e.g. after the worker resumes from a long
sleep) cannot exceed the per-minute budget.

## Rollout plan across the 4 H14 sub-PRs

- **PR1 (this PR) — scaffolding.** Adds [`features.integrity_sweeper`]
  TOML block with safe-off defaults, the [`load_skip_list`] glob
  parser, and the [`RatedTokenBucket`] / [`Clock`] primitives. No
  daemon worker, no IPC, no audit plumbing. The feature is opt-in but
  inert: setting `enabled = true` today changes nothing because no
  consumer reads the flag yet.
- **PR2 — daemon worker.** Wires a `pcloud-daemon` background task
  that, when `enabled`, drives the configured scope through the
  verification primitive, throttled by the bucket. Adds the
  on-AC / on-battery facade. Still no IPC surface, still no audit
  plumbing — divergences land in the structured log only.
- **PR3 — IPC control surface.** Adds owner-only IPC commands to
  start/stop/status the sweeper and to fetch the most recent
  divergence report. Honours the existing IPC peer-check posture.
- **PR4 — audit plumbing + parity matrix flip.** Routes divergences
  into the persistent audit store with the path-hash-only privacy
  rule, adds the resume cursor on disk, ships a `man` page snippet,
  and only then updates `C_FEATURE_PARITY_MATRIX.csv` and
  `STATUS.md` to reflect the row's new state. `bd-1du.10` consumes
  the matrix flip.

Until PR4 lands, this feature must be referred to as "scaffolding /
opt-in" everywhere in the repo. Do not claim parity coverage based on
the presence of the config block alone.

## PR4 status update — wired but pending live verification

As of PR4 the plumbing is **fully wired end-to-end** at the IPC and CLI
layers. Specifically:

- `pcloud-daemon::integrity_sweeper_service` ships the `IntegritySweeperShell`
  worker-thread harness, MPSC channel, progress accumulator, skip-list
  append+reload helper, and audit-detail formatter (path-hash-only).
- IPC adds `Method::IntegrityStatus`, `Request::IntegrityRunOnce`, and
  `Request::IntegritySkip { path }` plus the `IntegrityStatusPayload`
  JSON envelope.
- CLI adds `pcloudc integrity status` (default), `pcloudc integrity
  run-once`, and `pcloudc integrity skip <PATH>`.
- Audit emission goes through `pcloud_store::append_audit_event` with
  category `integrity.mismatch`. `Ok` events drop silently. `Throttled`
  events bump the `throttled` counter only. `audit_drops` is exposed in
  the IPC status payload so silent persistence drops stay observable
  (audit invariant M1).

**Honest scope for PR4 — what is NOT done yet:**

- The actual file walker that produces `IntegrityEvent`s is still a
  placeholder. PR2/PR3 of the H14 series will populate it; until then a
  `run-once` call against an enabled sweeper returns zero deltas.
- The bootstrap helper (`RuntimeShell::bootstrap_integrity_sweeper`) is
  a no-op stub. Spawning the worker with a borrowed audit sink requires
  refactoring the audit closure to flow through an
  `Arc<Mutex<StoreProfile>>` shim; that refactor is queued behind the
  PR2/PR3 walker work.
- **No live verification has been performed against a real account.**
  The unit suite covers the channel, the audit details formatter, the
  skip-list append+reload cycle, and the disabled-shell guard.
- **No parity-matrix row is flipped.** This feature is additive over
  the C surface and `bd-1du.4.6.1` remains the single tracker.

## H14 status summary (as of 2026-04-15)

| Sub-task | Status | Notes |
|----------|--------|-------|
| **H14a** — config block + bead | Done | `[features.integrity_sweeper]` is parsed and validated with safe-off defaults. Tracker row `bd-1du.4.6.1`. No `C_FEATURE_PARITY_MATRIX.csv` flip (feature is additive over the C surface). |
| **H14b** — in-process sweeper primitive | Done | `RatedTokenBucket` (token-bucket, `rate/60` per second, per-minute cap), `Clock` facade, skip-list glob parser, and the per-file "recompute and compare" verification primitive live in `pcloud-engine`. |
| **H14c** — server cross-check | Done | `ChecksumFetcher` trait lets the sweeper cross-check the recomputed local content hash against a server-side checksum, yielding `Mismatch` / `RemoteMissing` / `FetchFailed` variants. |
| **H14d** — daemon service + IPC + CLI | Wired, placeholder walker | `pcloud-daemon::integrity_sweeper_service` ships `IntegritySweeperShell`, MPSC channel, progress accumulator, skip-list append+reload, and path-HMAC audit formatter. IPC methods `IntegrityStatus`, `IntegrityRunOnce`, and `IntegritySkip { path }` plus `IntegrityStatusPayload` envelope. CLI commands `pcloudc integrity status` / `run-once` / `skip <PATH>`. `Ok` events drop silently; `Throttled` bumps a counter only; `audit_drops` is exposed in the status payload so silent persistence drops stay observable (audit invariant M1). |

**Honest caveat — what is still pending real-run integration:**

- The file-walker inside the worker is a placeholder. `pcloudc
  integrity run-once` is end-to-end at the IPC/CLI/audit layer and
  returns a well-formed status + delta report, but until the walker is
  populated by the PR2/PR3 follow-up the delta is zero against a
  correctly-synced root.
- `RuntimeShell::bootstrap_integrity_sweeper` currently runs the sweep
  **synchronously on the dispatch thread** rather than on a dedicated
  background worker. Spawning the worker with a borrowed audit sink
  requires refactoring the audit closure to flow through an
  `Arc<Mutex<StoreProfile>>` shim; that refactor is queued behind the
  walker work.
- `schedule_cron` is parsed and validated but has no in-process
  scheduler yet. Automatic runs must be driven externally (cron,
  systemd timer) — see the runbook playbook *Verifying local-vs-server
  integrity on a schedule*.
- No live verification against a real pCloud account has been
  performed. Unit coverage: channel, audit details formatter, skip-list
  append+reload cycle, disabled-shell guard.
