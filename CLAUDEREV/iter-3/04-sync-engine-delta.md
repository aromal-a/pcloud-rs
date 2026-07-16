# Iter 3 Delta — Dimension 4: Sync Engine & Runtime

Auditor: Claude (read-only). Date: 2026-04-29. Iter 1: 0/6/8/5. Iter 2:
converged (0 new). Iter 3 task: re-verify convergence, audit any
sync-engine touches that landed in `iter-2-fixes` window.

## Files modified after iter-2 audit

Commit `858ce5e fix(workspace): cross-stream code fixes from GPTREV +
live A↔B share findings` (2026-04-30, the day **after** iter-2
sync-engine audit) touched six sync-engine surface files:

- `crates/pcloud-engine/src/lib.rs` (+95 / -3)
- `crates/pcloud-engine/src/planner.rs` (+19 / -6)
- `crates/pcloud-engine/src/transfers/downloads.rs` (+10 / 0)
- `crates/pcloud-engine/src/transfers/uploads.rs` (+10 / 0)
- `crates/pcloud-store/src/lib.rs` (+53 / 0, tests only)
- `crates/pcloud-store/src/schema.rs` (+22 / -16)

These are GPTREV-driven fixes (separate from CLAUDEREV iter-1/iter-2)
addressing F-05/F-06/F-08/F-09/F-11. CLAUDEREV `iter-2-fixes.md`
explicitly lists `SYNC-H-04-1..H-04-4` as **deferred**, not landed.

## Audit of the new code

### planner.rs sort key change (F-06)

Sort upgraded from `(path, source)` to `(sync_id, path, source)`, and
the inner pairing loop now consumes only same-`(sync_id, path)`
candidates. **This is a correctness improvement** — it prevents
cross-root collapse of identical relative paths into a single conflict
group. Logic is sound; no new finding.

### EngineShell::resolve_conflict_by_sync_id_and_path (F-11)

New `Option<SyncId>` variant; `None` falls back to path-only matching
(backward compat). Position lookup uses `is_none_or` correctly. No new
finding.

### EngineShell::requeue_for_retry + clear_failed (F-05)

Closes the previously-broken `RetryLater` path: clears stale failed-list
entries, then pushes the operation to the **front** of
`scheduler.queued_operations`. Documented in rustdoc with a working
doctest. No new finding.

### schema.rs idempotent v5/v6 (F-08/F-09)

`apply_schema_v{5,6}` now guard each `ALTER TABLE` with a
`column_exists` probe and write `PRAGMA user_version = N` separately.
Two new regression tests in `lib.rs` cover the partial-migration replay
case. Crash-safety improvement. No new finding.

## Re-verification of iter-1 HIGHs (all 6)

| ID | Finding | Status iter-3 |
|----|---------|---------------|
| H-04-1 | Silent userspace event drops in `fs_events.rs` | **Open.** No `overflow`/`drop`/`warn` telemetry added. `iter-2-fixes.md` lists this as deferred. |
| H-04-2 | Hand-rolled debouncer in fs_events instead of `notify-debouncer-mini` | **Open.** Module unchanged in this window. Deferred. |
| H-04-3 | `power.rs` battery facade silent no-op on macOS/Windows/BSD (Linux-only) | **Open.** `power.rs` unchanged; rustdoc still says "containers without a battery facade are treated as Unknown". Deferred. |
| H-04-4 | Case-insensitive collision blindness in planner sort/pairing | **Open.** Even after F-06's `(sync_id, path)` sort upgrade, comparison is still byte-wise (`path.cmp(&path)`) — no case-fold. `probe_case_insensitive_fs` exists in `lib.rs:143` but is a *warning probe*, not a planner-time collision detector. Deferred. |
| H-04-5 | `SQLITE_BUSY` un-retried (no `busy_timeout`) | **Open.** `pcloud-store/src/tx.rs:37-40` rustdoc still concedes "`SQLITE_BUSY` is only surfaced". No retry wrapper added. Deferred. |
| H-04-6 | ~22 `.unwrap()` in `integrity_sweeper_service.rs` | **Open.** Grep count: **22 unwraps**, unchanged from iter-1. Deferred. |

All 6 iter-1 HIGHs stand. None of the 6 file touches in commit `858ce5e`
addressed them; they fix orthogonal GPTREV findings.

## Re-skim: conflict_resolver.rs

Re-read top 60 lines (definition surface). `ConflictPolicy` enum,
`serde(rename_all = "snake_case")`, default = `RenameBoth`,
documentation cross-references match the iter-1 finding set. **Nothing
new** that iter-1 missed. The body of `apply_policy` was already
covered by iter-1 H-04-4 (case-fold) and the iter-2 verification of the
public-link/conflict CLI surface.

## Convergence outcome

- New findings: **0**
- Retractions: **0**
- Regressions introduced by iter-2 fix window: **0** (the GPTREV fixes
  in commit `858ce5e` are net positive for sync-engine correctness)
- All 6 iter-1 HIGHs remain open and tracked

Dimension 4 holds at convergence (3 consecutive iters: iter-1 baseline,
iter-2 0/0/0, iter-3 0/0/0).

delta count: 0 new, 0 retractions, 0 regressions
