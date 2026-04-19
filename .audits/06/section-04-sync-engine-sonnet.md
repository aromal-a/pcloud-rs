# Audit 06 §4 — Sync Engine & Runtime (Sonnet, Independent)
Date: 2026-04-18
Auditor: claude-sonnet-4-6 (independent cross-validation of Opus audit-05)

## Scope

Files reviewed (post audit-05 state):

- `crates/pcloud-engine/src/lib.rs`
- `crates/pcloud-engine/src/planner.rs`
- `crates/pcloud-engine/src/scheduler.rs`
- `crates/pcloud-engine/src/local_scan.rs`
- `crates/pcloud-engine/src/stall_detector.rs`
- `crates/pcloud-engine/src/conflict_resolver.rs`
- `crates/pcloud-engine/src/recovery.rs`
- `crates/pcloud-engine/src/fs_events.rs`
- `crates/pcloud-engine/src/transfers/bandwidth.rs`
- `crates/pcloud-daemon/src/sync_loop_runtime.rs` (partial grep)

Audit-05 fixes claimed held: H1-H5, planner cap (M-4.2), (ino,dev) cycle detection (M-4.5), drain_batch deprecated (M-4.6).

---

## Verification: Claimed Fixes from Audit-05

**H1 — Planner overflow silent-drop**: HELD.
`Planner::plan_with_overflow` returns overflow slice at `lib.rs:461-506`.
`EngineShell::cap_overflow` enforces `PLANNER_OVERFLOW_MAX = 100_000` with `warn!`.
`persist_planner_overflow` in `sync_loop_runtime.rs:295` serializes to `value_kv`.
`restore_planner_overflow` is called on bootstrap at `sync_loop_runtime.rs:213`. Verified.

**H2 — Crash-between-dispatch-and-ack**: HELD.
`Scheduler::dispatched_operations` field present at `scheduler.rs:58`.
`next_batch` pushes to `dispatched_operations` at `scheduler.rs:207`.
`ack_batch` path-matches and removes at `scheduler.rs:270-276`.
`snapshot_scheduler_durable` combines queue + dispatched at `lib.rs:879-892`.
`restore_scheduler_queue` clears `dispatched_operations` on boot at `lib.rs:867-870`.
Regression test `crash_between_dispatch_and_ack_recovers_work_on_restart` at `scheduler.rs:520`. Verified.

**H3 — Per-root fairness**: HELD.
`take_fair_batch` distributes per-root using ceiling division at `scheduler.rs:278-316`.
`next_batch_fair(max_per_root)` variant also present at `scheduler.rs:433-449`. Verified.

**H4 — Stall detector reset on partial progress**: HELD.
`StallDetector::mark_progress` resets to `Instant::now()` at `stall_detector.rs:63`.
Long-transfer regression test at `stall_detector.rs:195`.
`StallDetector::new_with_elapsed` cross-restart persistence helper at `stall_detector.rs:130`. Verified.

**H5 — Zero-timeout stall clamp**: HELD.
`MIN_STALL_TIMEOUT = 1s` applied in `StallDetector::new` at `stall_detector.rs:54`. Verified.

**M-4.2 — Planner cap**: HELD.
`PLANNER_OVERFLOW_MAX = 100_000` at `lib.rs:73`.
`cap_overflow` truncates and logs warn at `lib.rs:515-527`. Verified.

**M-4.5 — (ino,dev) cycle detection**: HELD.
`walk_recursive` uses `HashSet<(u64, u64)>` keyed on `(meta.dev(), meta.ino())` at `local_scan.rs:316`.
Comment "M-4.5: use (ino, dev) pair, not ino alone" at `local_scan.rs:315`. Verified.

**M-4.6 — drain_batch deprecated**: HELD.
`#[deprecated(since = "0.1.0", note = "Unfair: ...")]` at `scheduler.rs:369`.
Production sync-loop references reviewed via grep — only two call sites in `scheduler.rs` test code and one in `next_batch` internals; no production caller found in `pcloud-daemon`. Verified.

---

## New Findings

### MEDIUM — M-04-S01: `walk_local_tree` is dead-code annotated but called from sync loop

`local_scan.rs:281`:
```
#[allow(dead_code)] // called from the sync loop; unused in unit tests
pub fn walk_local_tree<F>(...) -> std::io::Result<()>
```

The function is `pub` and carries a `#[allow(dead_code)]` annotation that admits it is unreferenced in the local crate. If it is genuinely called from the daemon's sync loop at runtime, the annotation is misleading to future readers and masks any future breakage where the call site is removed. If it is NOT wired yet, the annotation falsely downplays a gap.

**Remediation**: confirm the call site in `pcloud-daemon/src/sync_loop_runtime.rs`. If wired, remove the `#[allow(dead_code)]` attribute. If not yet wired, promote to HIGH (missing integration) and open a `bd-1du.3` sub-task.

### MEDIUM — M-04-S02: `IncrementalScanTracker` not integrated into `EngineShell`

`local_scan.rs:167-254`: `IncrementalScanTracker` is a well-tested, complete struct that manages per-root full/incremental scan decisions and debounces watcher events. It is **not a field on `EngineShell`** and is not imported in `lib.rs`. This means the sync-loop runtime must construct and own it separately, outside the engine's serialization boundary.

The consequence: a daemon restart cannot reliably restore per-root `last_full_scan` timestamps (since `Instant` is not serializable), causing every root to force a full scan on startup even when an incremental cycle would suffice. For large sync roots this adds unnecessary API load on restart.

**Remediation**: either accept this behavior (document it explicitly) or store a wall-clock `SystemTime` for `last_full_scan` alongside the `value_kv` snapshot and restore via `record_full_scan_at`. LOW impact operationally, MEDIUM documentation gap.

### MEDIUM — M-04-S03: `ConflictResolver` default policy is `RenameBoth`, not documented as default at IPC/config level

`conflict_resolver.rs:57-63`: `Default::default()` returns `ConflictPolicy::RenameBoth`. The prior audit-05 state claimed `ManualReview` was the default in some code paths. The actual code is `RenameBoth` which performs side-effects (renames files on disk). Users who rely on "no destructive action without confirmation" semantics are not warned that the out-of-the-box policy renames both copies.

**Remediation**: verify the operator-facing config documentation names `RenameBoth` as the default and describes what it does to local files. If the intent was `ManualReview`, update the `Default` impl. File:line `conflict_resolver.rs:60`.

### MEDIUM — M-04-S04: `ack_batch` matches by path only — ignores `sync_id`

`scheduler.rs:270-276`:
```rust
pub fn ack_batch(&mut self, paths: &[&str]) {
    ...
    self.dispatched_operations
        .retain(|op| !paths.iter().any(|p| op.path() == *p));
}
```

Ack matches `op.path() == *p` but does not check `op.sync_id()`. If two different sync roots each have an in-flight operation for a file at the same relative path (e.g. `documents/report.pdf`), acking the completion of root A's operation would also drop root B's dispatched entry, causing root B's operation to be silently discarded on restart rather than retried.

This is a real correctness gap since paths are sync-root-relative (not absolute).

**Remediation**: change `ack_batch` and `ack_dispatched_path` to match on `(sync_id, path)` pairs. The caller already has the `PlannedOperation` and can supply both. File:line `scheduler.rs:270`, `lib.rs:898`.

### LOW — L-04-S01: Bandwidth limiter `acquire` usize cast silently truncates on 32-bit targets

`bandwidth.rs:137`:
```rust
pub fn acquire(&self, bytes: usize) -> Duration {
    self.pacer.acquire(bytes as u64)
}
```

On 32-bit platforms `bytes` is a `u32`-width `usize`; casting to `u64` is always safe (widening). However `acquire_blocking` at line 148 does the same cast. The real concern is the opposite direction: if `BandwidthPacer::acquire` internally casts back to `usize`, large requests on 64-bit may silently truncate to 32 bits on a future internal refactor. The API surface should document that `bytes` represents a chunk size and callers should not pass values larger than the transfer chunk (which is bounded by config). No immediate bug; document.

### LOW — L-04-S02: `next_batch_fair(max_per_root)` does not drain — no crash-recovery coverage

`scheduler.rs:433-449`: `next_batch_fair` is a **non-mutating peek** that returns `&PlannedOperation` references. Unlike `next_batch` it does not populate `dispatched_operations`. If a caller used `next_batch_fair` as its dispatch path (rather than `next_batch`), the H2 crash-recovery guarantees would be silently absent.

The function has a doc comment explaining it is a peek, but the name `next_batch_fair` could mislead callers into using it as a primary dispatch path. Consider renaming to `peek_batch_fair` and adding a `# Warning` in the doc comment.

File:line `scheduler.rs:433`.

---

## Summary Table

| Severity | ID | Finding | File:line |
|---|---|---|---|
| MEDIUM | M-04-S01 | `walk_local_tree` `#[allow(dead_code)]` — integration status unclear | `local_scan.rs:281` |
| MEDIUM | M-04-S02 | `IncrementalScanTracker` not in `EngineShell` — full scan forced on every restart | `local_scan.rs:167` |
| MEDIUM | M-04-S03 | Default conflict policy `RenameBoth` not documented at user level | `conflict_resolver.rs:60` |
| MEDIUM | M-04-S04 | `ack_batch` matches path only, ignores `sync_id` — cross-root ack collision | `scheduler.rs:270`, `lib.rs:898` |
| LOW | L-04-S01 | `bandwidth.rs` `usize as u64` cast: document chunk-size contract | `bandwidth.rs:137,148` |
| LOW | L-04-S02 | `next_batch_fair` naming misleads — not a draining/dispatch path | `scheduler.rs:433` |

---

## Audit-05 Fix Verification: All 5 HIGH + 2 MEDIUM fixes confirmed held.

No regressions detected in the H1-H5 / M-4.2 / M-4.5 / M-4.6 surface since audit-05.

The most actionable new finding is **M-04-S04** (ack_batch path-only match): a cross-root path collision on a multi-root setup would silently drop an un-acked operation from `dispatched_operations`, defeating the H2 crash-recovery guarantee for that root. This should be addressed before any multi-root production deployment.
