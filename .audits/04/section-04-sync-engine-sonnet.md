# Section 4 — Sync Engine & Runtime
## Auditor: Sonnet (independent)
## Date: 2026-04-18

---

## Scope

`crates/pcloud-engine/src/` (lib, planner, scheduler, stall_detector,
conflict_resolver, fs_events, reconcile_worker, recovery, transfers/)  
`crates/pcloud-store/src/repositories/audit.rs`  
`crates/pcloud-resilience/src/`

---

## Findings

### CRITICAL

**None identified.**

---

### HIGH

**H-1 — `coalesce_window_ms` declared but never enforced**  
`crates/pcloud-engine/src/fs_events.rs:19`

`FsEventIngestor::coalesce_window_ms` is a config field with a comment
explicitly admitting it is not applied:

```
// TODO(bd-1du): coalesce_window_ms is declared but not applied; all debouncing is
// batch-local only (last-writer-wins within a single normalize_events call).
```

In production, `notify`/inotify delivers events in discrete batches; two
rapid writes to the same file that arrive in separate batches will each
produce a separate `SyncCandidate` and therefore a separate upload. This
is not data-loss but it causes spurious duplicate uploads and races
against remote state. The declared 250 ms window gives users a false
sense that debounce is active.

**Remediation:** Wire a real time-keyed debounce map (e.g., `BTreeMap<String, (Instant, FsEvent)>`) inside `FsEventIngestor`, keyed by path and flushed when `coalesce_window_ms` has elapsed. Alternatively, if debounce is intentionally deferred, remove the field and document the gap under a bead.

---

**H-2 — No per-root fairness enforcement in scheduler (`next_batch`)**  
`crates/pcloud-engine/src/scheduler.rs:119-139`

`Scheduler::next_batch` is explicitly documented as having no
per-root fairness:

```
// TODO(bd-1du): per-root fairness is not enforced; a high-throughput root
// can starve others.
```

`next_batch_fair` (line 173) exists but is never called from
`EngineShell::advance_transfer_cycle` (lib.rs:487-492). The fair
variant is wired only in tests. A user with two sync roots — one large
burst (e.g., initial backup) and one interactive folder — will see the
interactive root completely starved until the burst drains.

**Remediation:** Replace the `next_batch()` call in `advance_transfer_cycle` with `next_batch_fair(max_ops_per_root)` and expose `max_ops_per_root` as a configurable field on `Scheduler`. Close or link to the existing `TODO(bd-1du)` bead.

---

**H-3 — Planner silently drops over-cap candidates without dead-letter persistence**  
`crates/pcloud-engine/src/planner.rs:122-132`

When `max_operations_per_tick` (default 1024) is exceeded, the planner logs
a `warn!` and drops the excess path groups:

```
// TODO(bd-1du): dropped-over-cap operations should be tracked in a
// dead-letter store rather than re-discovered via full scan.
```

Re-discovery depends on the next full scan (300 s cadence by default). On
a large initial sync or after a bulk file move, thousands of operations
can be silently deferred. If the daemon restarts between ticks there is no
guarantee the dropped groups are ever replayed because the engine holds no
in-flight record of them — they are simply absent from the scheduler queue.

**Remediation:** Persist excess candidates to the `pcloud-store` dead-letter queue before discarding them from the in-memory batch. Alternatively, maintain a durable cursor into the candidate list so the next tick resumes from where the cap was reached.

---

### MEDIUM

**M-1 — `NewestWins` conflict policy performs no timestamp comparison**  
`crates/pcloud-engine/src/conflict_resolver.rs:175-183`

`resolve_newest_wins` unconditionally delegates to `resolve_prefer_remote`
with an inline comment acknowledging the missing mtime comparison. The
`ConflictKind` variants carry no timestamp. Any file where the local copy
is newer will silently lose its local changes when `NewestWins` is
configured.

**Remediation:** Extend `ConflictKind::LocalModifyVsRemoteModify` (or a new variant) to carry optional `local_mtime` / `remote_mtime`. Thread the local mtime through the `LocalScanEntry` → `SyncCandidate` → `ConflictKind` pipeline and perform the comparison in `resolve_newest_wins`.

---

**M-2 — `RenameBoth` conflict policy is a stub — behaves identically to `ManualReview`**  
`crates/pcloud-engine/src/conflict_resolver.rs:185-194`

The default conflict policy (`ConflictResolver::default`) is `RenameBoth`,
but `resolve_rename_both` returns `ConflictResolution::ManualReview`
unconditionally. Users who rely on the documented "both copies preserved"
semantic get no automatic rename; files simply remain blocked in the queue.

**Remediation:** Implement the rename-both path: emit two `PlannedOperation::UploadFile`/`DownloadFile` entries with distinct names (e.g., `<name>.local.<ts>` and `<name>.remote.<ts>`). Until implemented, change the default policy to `ManualReview` and document `RenameBoth` as not-yet-implemented.

---

**M-3 — `resolve_prefer_remote` threads `remote_file_id: None` through `DownloadFile`**  
`crates/pcloud-engine/src/conflict_resolver.rs:147, 158`

Two TODO comments note that `remote_file_id` is dropped (`None`) when
producing `DownloadFile` from a conflict resolution. The transfer runtime
must then perform a redundant server metadata lookup to re-resolve the
file id. This is a correctness fragility: if the server state changes
between conflict detection and resolution the wrong file version may be
downloaded.

**Remediation:** Thread `remote_file_id` through `ConflictKind::LocalModifyVsRemoteModify` and `LocalDeleteVsRemoteModify` from the diff-poller candidate and populate it on the resolved `DownloadFile` operation.

---

**M-4 — Stall detector is not wired into the engine loop**  
`crates/pcloud-engine/src/stall_detector.rs`

`StallDetector` is a well-implemented module (timeout, `mark_progress`,
`check_stall`) but `EngineShell` (lib.rs) does not hold or call it. There
is no call site in `runtime.rs` that invokes `mark_progress()` after a
successful transfer or `check_stall()` on a timer tick. The module exists
as a standalone utility but contributes no runtime behavior.

**Remediation:** Add a `StallDetector` field to `EngineShell`, call `mark_progress()` from `mark_transfer_completed`, and call `check_stall()` from the daemon's periodic health-check tick.

---

**M-5 — `ReconcileWorker` cadence is 300 s vs C's event-driven 10 s**  
`crates/pcloud-engine/src/reconcile_worker.rs:27-38`

The module comment acknowledges the cadence mismatch with the C client:
C fires `plocalscan` at ~10 s after file-system events; the Rust path
uses a fixed 300 s periodic timer. This means a local file change may
sit unsynced for up to 5 minutes if the `notify`/inotify event path is
disrupted or the event was dropped on watcher overflow.

**Remediation:** Wire the watcher event path to call `ReconcileWorker::request_scan()` on the first detected change event, reducing perceived latency to near-zero for interactive edits while keeping the 300 s full-tree safety net.

---

**M-6 — `hmac_key` stored as raw `Vec<u8>` rather than `SecretBytes`**  
`crates/pcloud-store/src/repositories/audit.rs:108-109`

The module comment acknowledges this:

```rust
/// Stored as `Option<Vec<u8>>` (rather than `SecretBytes`) because
/// `pcloud-store` is a low-level crate and cannot depend on `pcloud-secret`.
```

The HMAC key is held in plain `Vec<u8>` on the heap and will not be
zeroed on drop. If the daemon process is forked, crashed with a core
dump, or swapped out, the HMAC key leaks.

**Remediation:** Either add a `pcloud-secret` dependency to `pcloud-store` (preferred — it has no other workspace deps) or implement a local `Zeroizing<Vec<u8>>` wrapper using the `zeroize` crate directly, which `pcloud-store` likely already transitively depends on.

---

**M-7 — `localscan_wakes` counter is purely cosmetic — no actual scan loop**  
`crates/pcloud-engine/src/lib.rs:176-181`

The counter mirrors C's `psync_wake_localscan` wake signal but the
module comment (line 178) is explicit:

```
// In Rust the actual scan loop is still pending parity work
// (bd-1du.3); the counter exists so callers and tests can confirm
// the wake signal is observed by the engine.
```

This means local-file-change-triggered scans do not actually happen
through the `wake_localscan` path — callers calling `wake_localscan()`
believe they are triggering a scan that never fires.

**Remediation:** Tracked under `bd-1du.3`. The remediation is to connect `wake_localscan` to the `ReconcileWorker::request_scan` call and fan the scan into the `LocalScanner`. Mark the intermediate state clearly in API docs to avoid callers relying on the current no-op behavior.

---

### LOW

**L-1 — `validate_relative_path` does not reject NUL bytes or excessively long paths**  
`crates/pcloud-engine/src/fs_events.rs:100-113`

The path validator rejects `..`, absolute paths, `./`, and empty
segments, but does not check for NUL bytes (`\0`) or OS-level path
length limits. A malicious or corrupted event could produce a path
with an embedded NUL that passes validation but causes a C-FFI
boundary fault if ever passed to a native API.

**Remediation:** Add `trimmed.contains('\0')` and `trimmed.len() > 4096` guards to `validate_relative_path`.

---

**L-2 — `plan` batch cap warning logs `sync_id` of first skipped candidate only**  
`crates/pcloud-engine/src/planner.rs:123-128`

When multiple sync roots are batched together and the cap fires, the
warning always names only the first skipped candidate's `sync_id`, which
may be misleading when the overflow is caused by a different root.

**Remediation:** Collect all distinct `sync_id`s in the skipped window and include them in the log message.

---

**L-3 — `StallDetector` cannot be serialized; transient restart loses progress timestamp**  
`crates/pcloud-engine/src/stall_detector.rs:93-95`

The comment correctly notes this. On a daemon restart the stall detector
resets to `Instant::now()`, so a pre-crash stall (engine stuck for >5 min)
would not be detected after restart until another 5 minutes pass. This is
low-severity but means post-crash stall detection is blind for one timeout
window.

**Remediation:** Persist the last-progress epoch to the SQLite `preferences` table via `pcloud-store` so the stall detector can reconstruct its baseline across restarts.

---

## Summary Table

| ID  | Severity | Component             | File:Line                                       | One-line description                                    |
|-----|----------|-----------------------|-------------------------------------------------|---------------------------------------------------------|
| H-1 | HIGH     | FsEventIngestor       | fs_events.rs:19                                 | coalesce_window_ms declared but never applied           |
| H-2 | HIGH     | Scheduler             | scheduler.rs:119 / lib.rs:487                   | next_batch_fair unused; per-root starvation possible    |
| H-3 | HIGH     | Planner               | planner.rs:122-132                              | over-cap candidates silently dropped, no dead-letter    |
| M-1 | MEDIUM   | ConflictResolver      | conflict_resolver.rs:175-183                    | NewestWins does no mtime comparison, always prefer-remote |
| M-2 | MEDIUM   | ConflictResolver      | conflict_resolver.rs:185-194                    | RenameBoth is a stub, behaves like ManualReview         |
| M-3 | MEDIUM   | ConflictResolver      | conflict_resolver.rs:147, 158                   | remote_file_id lost on conflict resolution → extra lookup |
| M-4 | MEDIUM   | StallDetector         | stall_detector.rs (no call sites in lib.rs)     | stall detector not wired into engine runtime            |
| M-5 | MEDIUM   | ReconcileWorker       | reconcile_worker.rs:27-38                       | 300 s cadence vs C 10 s; no event-triggered early scan  |
| M-6 | MEDIUM   | AuditRepository       | audit.rs:108-109                                | hmac_key held in plain Vec<u8>, not zeroed on drop      |
| M-7 | MEDIUM   | EngineShell           | lib.rs:176-181                                  | localscan_wakes is a no-op counter; scan loop not wired |
| L-1 | LOW      | FsEventIngestor       | fs_events.rs:100-113                            | NUL bytes and path length not checked in path validator |
| L-2 | LOW      | Planner               | planner.rs:123-128                              | cap warning names only first skipped sync_id            |
| L-3 | LOW      | StallDetector         | stall_detector.rs:93-95                         | progress baseline lost on daemon restart                |

---

## Notable Strengths

- Planner, scheduler, conflict resolver, recovery manager, and reconcile
  worker are all well-documented, unit-tested, and structurally clean.
- The audit repository implements a genuine tamper-evident SHA-256 hash
  chain with optional HMAC-SHA256 non-repudiation and atomic
  insert+back-fill via unchecked_transaction — this exceeds the C client.
- `DeletePolicy` covering Full / UploadOnly / DownloadOnly / BackupArchive
  is correct and has thorough test coverage.
- `RecoveryManager` failure taxonomy (retryable / manual / terminal) is
  clear and matches enterprise expectations.
- `#![forbid(unsafe_code)]` on the engine crate and `pcloud-resilience`.
