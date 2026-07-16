# Iter 2 Delta — Dimension 4: Sync Engine & Runtime

Auditor: Claude (read-only). Date: 2026-04-29. Iter 1: 0/6/8/5.

## Verifications against iter 1

### V1 — FK constraints in `crates/pcloud-store/src/schema.rs`

**Re-verified, iter 1 finding stands.** `grep -E "FOREIGN KEY|REFERENCES "
schema.rs` returned **0 matches**. No table declares a real FK. Iter 1
also called this out at lines 488-489 ("`PRAGMA foreign_keys = ON` …
but **no table declares a FOREIGN KEY constraint**") and M-04-3.
**No retraction.** No new finding.

### V2 — Embedded migration runner with `PRAGMA user_version`

**Iter 1 should have credited this.** `crates/pcloud-store/src/migrations.rs`
exists and is a real, structured forward-only migration runner:

- `MigrationPlan` / `MigrationError::BackwardsMigration` typed surface
  (refuses downgrade by design — documented rollback policy at lines
  26-34).
- `apply_plan` reads `PRAGMA user_version` at execution time
  (`migrations.rs:81`) and applies each `apply_schema_v{1..11}` step
  inside a single batch that ends with `PRAGMA user_version = N` so a
  crash mid-migration leaves a fully-committed prior version
  (documented at lines 75-79). This is correct crash-safe migration
  behavior.
- `tx.rs` provides a `TransactionBoundary` that callers can wrap around
  startup for atomic init.

Iter 1's M-04-2 framed this as "no SQL `migrations/` directory" and
flagged the lack of an upgrade-path test fixture. Both halves are
factually correct, but the **architectural framing was uncharitable** —
the inline runner is well-documented and crash-safe. Iter 1's actual
ask (binary fixtures under `tests/fixtures/v{N}.db.gz` proving
v6→v11 row preservation) remains valid.

**Partial retraction:** M-04-2 should be reframed from "no migration
directory" to "no upgrade-path test fixture against pre-v11 data." The
runner itself is solid. Findings count adjusted: HIGH unchanged, but
the MEDIUM tone is softened.

### V3 — Linux power awareness uses upower D-Bus?

**No. Iter 1 stands and is more correct than the question implies.**
`crates/pcloud-engine/src/power.rs:139-171` reads
`/sys/class/power_supply/*/status` directly with `std::fs::read_dir`
and string-comparing `"Discharging"` / `"Charging"` / `"Full"`. There
is **no D-Bus client, no `zbus`/`dbus` dependency, no upower wiring**.
`grep -r "upower\|dbus" crates/pcloud-engine` returned **0 matches**.

This is the documented design (line 31-36): keep `pcloud-engine`
dependency-light, leave richer reading to a daemon-injected
`PowerSource` impl. macOS / Windows / BSD return `Unknown`. Iter 1's
H-04-3 stands verbatim — operator-facing `pause_on_battery=true` is a
silent no-op on macOS / Windows.

**No retraction.** Sysfs-only is correct fact pattern.

### V4 — Conflict resolution module spot-check

`crates/pcloud-engine/src/conflict_resolver.rs` is the module. Default
policy is `RenameBoth` (line 87-91) — non-destructive, preserves both
sides with `.conflict-local.<ext>` / `.conflict-remote.<ext>` suffixes.
Documented rationale (lines 60-86) is exemplary: it explicitly
explains why `NewestWins`, `PreferLocal`, and `PreferRemote` can each
destroy data and why `RenameBoth` is the safe default. Six policies:
`PreferLocal`, `PreferRemote`, `NewestWins`, `RenameBoth`, `Error`,
`ManualReview`. The case-insensitive collision blindness flagged in
H-04-4 is confirmed: `grep "case_insensitive\|CaseInsensitive"
conflict_resolver.rs` returns **0 matches** (no detection anywhere in
the resolver). H-04-4 stands.

### V5 — Sync engine wires `pcloud-resilience` (rate-limit / CB)?

**Iter 6's "unreachable from production" claim partially confirmed for
sync engine specifically.** Of 4 files in `pcloud-engine` referencing
`pcloud_resilience`:

- `transfers/bandwidth.rs` uses `BandwidthPacer` for bandwidth pacing
  (real production use).
- `reconcile_worker.rs` uses only `Clock`/`SystemClock` for testable
  time, not retry/CB/budget.
- `lib.rs` and `benches/engine.rs` — utility / benchmark.

**Zero references to `CircuitBreaker`, `GlobalRetryBudget`, or
`TokenBucket` from inside `pcloud-engine`.** All retry / budget
plumbing lives in `pcloud-daemon` (`transport_factory.rs:44,92,105` —
`GlobalRetryBudget` constructed for `Environment::Production`,
`rate_limit.rs:33` — `TokenBucket`). The wiring exists at the daemon
boundary but **the engine itself does not consume a budget handle on
the retry path**. This is exactly iter 1's M-04-5 (no budget
enforcement at engine→resilience boundary). M-04-5 stands and is
sharpened by this verification.

### V6 — Idempotency keys persisted across daemon restart?

**Confirmed: no 128-bit idempotency keys exist anywhere in the
codebase.** `grep -r "idempotency_key\|IdempotencyKey\|idempotency-
key\|Idempotency-Key" crates/pcloud-store` returned **0 matches**.
`grep -r "idempotency_key\|IdempotencyKey\|128-bit\|u128" crates/pcloud-engine/src`
returned **1 hit unrelated to idempotency** (`local_scan.rs:291`
documents Windows `FILE_ID_INFO.FileId` 128-bit file IDs). The
`upload_resume_state` table (schema v9) persists an `upload_id` (server-
assigned i64) plus an `offset` cursor — that is a **resume token, not
an idempotency key**. There is no client-generated 128-bit unique
identifier, no `INSERT … ON CONFLICT(idempotency_key)` clause, and
nothing that survives a daemon restart for the purpose of de-duplicating
an already-completed write. Iter 1's H-04-... wait, iter 1 listed this
as point 8 in summary and did not assign it an H-/M- ID. The point
was correctly flagged, just not numbered. No retraction; the gap is
real and **deserves an explicit finding ID** in any consolidated
report.

## Convergence signal

The seven verification probes produced:

- 5 confirmations of iter 1 findings (FK absence, sysfs-only Linux power,
  case-insensitive blindness, missing engine→resilience budget plumbing,
  no idempotency keys).
- 1 partial reframe (M-04-2 — credit the embedded `PRAGMA
  user_version` runner; keep the upgrade-fixture ask).
- 1 new sharpening: the engine's only `pcloud_resilience` consumer is
  `BandwidthPacer`; retry-budget / CB / token-bucket are entirely
  daemon-side. Sync engine is therefore "wired to the wrong layer" for
  retry budget enforcement — the engine should hold a `GlobalRetryBudget`
  Arc, not the daemon transport factory.

No CRITICAL surfaced. No retracted HIGHs. No new HIGHs.

## Delta tally

- New findings: 0
- Retracted findings: 0
- Reframed findings: 1 (M-04-2 — runner is solid; keep fixture ask)
- Sharpened findings: 1 (M-04-5 — confirmed engine has zero
  CircuitBreaker / GlobalRetryBudget / TokenBucket consumption)

delta count: 0
