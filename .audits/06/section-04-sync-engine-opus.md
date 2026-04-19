# Audit 06 — Section 4: Sync Engine & Runtime

**Date:** 2026-04-18
**Auditor:** Opus 4.7
**Scope:** `crates/pcloud-engine/` (post audit-05 H1–H5 + additional guards)

## Executive Summary

Audit-05 remediations are largely held. H1 (sync-root-absolute conflict
resolution), H2 (peek/ack durability via `dispatched_operations`), H5
(cold-cache → `RetryableNetworkError`), plus the additional guards
(`PLANNER_OVERFLOW_MAX=100_000`, `(ino,dev)` cycle detection,
`drain_batch` deprecation) are present and correct in source. **H3
(`SynchronousGuard` RAII) and H4 (byte-progress `StallDetector`) are
NOT held in `crates/pcloud-engine/`** — no such API exists. The H4
regression test is mislabelled: it proves a *time-based* progress loop,
not byte-level progress.

## Findings

### CRITICAL
None.

### HIGH

**H-4.1 — H3 not held: `SynchronousGuard` RAII type is absent**
- Grep for `SynchronousGuard|synchronous_guard` across the workspace
  returns zero hits in `crates/pcloud-engine/`. The only
  "synchronous"-named constructs are unrelated doc comments in
  `pcloud-session`, `pcloud-resilience`, `pcloud-store`.
- Expected per audit-05 remediation: an RAII guard that couples
  `scheduler.next_batch()` dispatch with a Drop-triggered re-enqueue /
  ack path so a panic between dispatch and ack cannot permanently
  leak `dispatched_operations`.
- Current state: `Scheduler::next_batch` (`scheduler.rs:204-211`)
  pushes into `dispatched_operations` and relies on callers to invoke
  `ack_batch`. There is no Drop-coupled safety net.
- Impact: a panic in the transfer worker between `next_batch` and
  `ack_batch` will leave the op in `dispatched_operations` forever
  (H2 restart path re-queues it, which is partial mitigation, but
  in-process panic recovery without restart is not covered).
- Remediation: introduce `pub struct DispatchedGuard<'a>{…}` that
  holds `&mut Scheduler` + path list and in `Drop` either acks (on
  `commit()`) or re-enqueues (on unwind). File target:
  `crates/pcloud-engine/src/scheduler.rs`.

**H-4.2 — H4 not held: `StallDetector` has no byte-progress API**
- `stall_detector.rs:37-95` exposes only `mark_progress()` (wall-clock
  reset). No `observe_bytes(n)` / `update_bytes_transferred` /
  `progress_bytes` method exists.
- The test `long_running_transfer_does_not_stall_if_bytes_progress`
  (`stall_detector.rs:194-219`) is misnamed: it calls `mark_progress()`
  in a loop, which is the time-based API, not byte-based. A transfer
  that opens a socket and hangs mid-stream (bytes=0 but periodic
  mark_progress heartbeats from a liveness ticker) would never be
  flagged as stalled.
- Impact: the advertised H4 fix ("byte-progress StallDetector") is not
  actually implemented. Upload/download loops that tick a heartbeat
  on schedule but deliver no bytes will fool the detector.
- Remediation: add `observe_bytes(delta: u64)` and track
  `last_bytes_seen + last_bytes_change_instant` separately from
  wall-clock heartbeats; `check_stall` should fire on *either* axis.

### MEDIUM

**M-4.1 — `drain_batch` deprecated but still live**
- `scheduler.rs:369-378`: `#[deprecated(since = "0.1.0", note = "…")]`
  is applied, but the method body still exists and is used by two
  callers in `pcloud-engine/tests/engine_basics.rs:372,427` (confirmed
  via Grep). No `#[allow(deprecated)]` guard — a `-D warnings` build
  would fail. Consider gating behind `#[cfg(test)]` or removing.

**M-4.2 — `ack_batch` is O(N·M) on path match**
- `scheduler.rs:262-276`: `dispatched_operations.retain(|op|
  !paths.contains(&op.path()))` is quadratic on batch width. At the
  documented `max_parallel_uploads+downloads` budgets (small) this is
  fine; if the budget is ever raised to hundreds it becomes a hot
  spot. Consider a `HashSet<&str>` fast path when `paths.len() > 8`.

**M-4.3 — `resolve_newest_wins` silent tie-break is correct but
undocumented in matrix**
- `conflict_resolver.rs:222-254`: tie (`local == remote`) routes to
  prefer-remote without surfacing a conflict event. Matches C
  "server-wins" default. Recommend emitting a `log::debug!` with sync
  id + path so operators can audit tie decisions.

### LOW

**L-4.1 — `cap_overflow` uses `log::warn!` without rate limiting**
- `lib.rs:515-527`: a repeated overflow scenario can flood logs each
  cycle. Add a single-shot / throttled warn so the dropped-count is
  surfaced once per window.

**L-4.2 — `walk_local_tree` only has `(ino,dev)` on `#[cfg(unix)]`**
- `local_scan.rs:305-339`: Windows branch (not shown in excerpt) falls
  back to depth-limit alone per the module doc. Cross-platform cycle
  detection is a `bd-1du.4` cross-platform item, not a regression.

**L-4.3 — `peek_batch` doc warns of infinite loop but no assertion**
- `scheduler.rs:213-232`: doc-only guidance; consider a debug-only
  `assert!` when `peek_batch` is called twice in a row without
  `ack_batch` / `next_batch` interleave to catch mis-use in tests.

## Verification Matrix (audit-05 fixes)

| Fix | Status | Evidence |
|-----|--------|----------|
| H1 sync-root-absolute resolver | HELD | `conflict_resolver.rs:128-161` |
| H2 peek/ack `dispatched_operations` | HELD | `scheduler.rs:51-68, 204-276, 520-579` |
| H3 `SynchronousGuard` RAII | **NOT HELD** | No such symbol |
| H4 byte-progress `StallDetector` | **NOT HELD** | `stall_detector.rs:37-95` time-only |
| H5 cold-cache→`RetryableNetworkError` | HELD | `recovery.rs:74,129-142`, `lib.rs:1266` |
| `PLANNER_OVERFLOW_MAX=100_000` cap | HELD | `lib.rs:73, 515-527` |
| `(ino,dev)` cycle detection | HELD (unix) | `local_scan.rs:286-325` |
| `drain_batch` deprecated | HELD | `scheduler.rs:369-378` |

## Recommendation

Re-open audit-05 H3 and H4 under a new bead (suggested:
`bd-1du.engine-raii`) before closing `bd-1du.10`. These are genuine
durability gaps, not cosmetic.
