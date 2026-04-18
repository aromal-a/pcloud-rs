# Section 4 — Sync Engine Runtime Audit (Opus)

Scope: `crates/pcloud-engine/` + `crates/pcloud-daemon/src/sync_loop_runtime.rs`.
Reviewed: planner, scheduler, conflict resolver, stall detector, debounce/watcher,
back-pressure, pause/resume, audit persistence, resume state, bandwidth scheduling.

Summary: the engine is a well-factored pure state machine (EngineShell aggregates
planner/scheduler/recovery/conflict/transfers) with deterministic unit tests. Live
runtime bridging (sync_loop_runtime) is functional but has meaningful correctness,
fairness, and durability gaps. No unsafe. No secret leakage detected.

---

## CRITICAL

### C1. Remote diff entries persist fabricated/zero metadata into file_metadata
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:282-302`
`poll_remote_diff` unconditionally upserts `FileMetadataRecord` with
`parent_folder_id = 0` for folders, `size=0`, empty `hash`, `modified=0`,
`created=0`. This poisons the local metadata cache that `stat_path` relies on.
Any later local resolution using this record will see zero sizes/hashes,
producing false-positive "changed" decisions and spurious uploads, and wipes
correct metadata from previous full fetches (upsert overwrites). Fix: only
upsert when real metadata is available (fetch via `listfolder`/`stat`) or mark
the row incomplete with a sentinel and skip it in stat lookups.

### C2. Upload cycle cannot discover parent folder id — files under subdirs always fail
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:424-440, 652-666`
`resolve_upload_parent` returns `RecoveryFailure::InvalidPath` whenever the
path contains `/` and `remote_parent_folder_id` is `None`. Local scan entries
produced by `walk_local_tree` (`sync_loop_runtime.rs:634-640`) **always** set
`remote_parent_folder_id: None`. Consequence: any file not at the sync root
will be classified `InvalidPath` → `FailureDisposition::Terminal`, dropped,
and never retried. This silently breaks sync for every non-flat tree. Fix:
resolve parent folder via `createfolderifnotexists`/path-to-id walk before
upload, or stage folder creation operations first and feed their resulting
`RemoteFolderId` into queued uploads.

### C3. Diff cursor persisted before candidates are planned/applied
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:250-257, 304-310`
Cursor is saved as soon as a batch is fetched, before planning/ingestion and
well before operations execute. If the process crashes or `ingest_remote_diff_filtered`
returns error (line 309), the server-side changes are silently lost forever
— next poll starts from the new cursor. Classic at-least-once → at-most-once
regression. Fix: persist cursor only after ingestion succeeds and operations
are safely queued/durably recorded. Combined with #C1, this can leave local
state permanently inconsistent with remote.

---

## HIGH

### H1. Planner cap drops operations silently to a log line, no dead-letter
File: `crates/pcloud-engine/src/planner.rs:104-132`
When `max_operations_per_tick` (default 1024) is exceeded, excess path groups
are dropped and a `TODO(bd-1du)` comment notes the dead-letter gap. The
comment at `planner.rs:125-129` promises re-discovery via "next full scan" —
but the sync loop only full-scans every `full_scan_interval_secs` (default
300s, `local_scan.rs:27-30`). For 300s any bursty change beyond 1024 ops is
invisible. Fix: persist dropped paths to a retry queue table; do not rely on
the full scan.

### H2. Scheduler is not back-pressure aware — `replace_queue` clobbers in-flight work
File: `crates/pcloud-engine/src/scheduler.rs:80-87`, called from
`lib.rs:281-347`. Each `ingest_*` call invokes `replace_queue`, wiping the
previous queue including any not-yet-advanced ops that were not in the last
batch. Transfers already accepted by coordinators survive (they are cloned
into `active_*`), but queued items for the same or other roots are lost when
another root's ingest fires. For multi-root daemons this causes starvation
of non-actively-ingesting roots. Fix: merge rather than replace; or maintain
per-root sub-queues and interleave.

### H3. `next_batch` starvation — no per-root fairness used by default
File: `crates/pcloud-engine/src/scheduler.rs:136-140, 173-189`
`next_batch_fair` exists but `EngineShell::advance_transfer_cycle`
(`lib.rs:487-492`) calls `next_batch()` which has a `TODO` acknowledging
starvation. A single busy sync root monopolises parallelism slots. Fix: wire
`next_batch_fair` with a sensible `max_per_root` as the production path.

### H4. Pause is not persisted; resume loses state on daemon restart
File: `crates/pcloud-engine/src/lib.rs:534-555`, `sync_loop_runtime.rs:228-230`
`paused_sync_roots: BTreeSet<SyncId>` is in-memory only. Comment at
`lib.rs:170-173` claims persistence lives in the store's `paused` column, but
`RealSyncLoopRuntime` never reads/writes that column on pause/resume, nor
hydrates `EngineShell.paused_sync_roots` at startup. A restart resumes every
root regardless of user intent. Fix: load paused roots on `new()`; mutate the
store column on pause/resume IPC handlers.

### H5. StallDetector is instantiated nowhere in the loop runtime
File: `crates/pcloud-engine/src/stall_detector.rs` + `sync_loop_runtime.rs` (grep)
The stall detector is never wired into `RealSyncLoopRuntime`. The "stall"
capability is dead code. Audit matrix claims implemented parity for
liveness/stall detection. Fix: own a `StallDetector` per sync root (or global)
and call `mark_progress()` on transfer complete, `check_stall()` each cycle.

### H6. Coalesce window ignored — watcher debouncing is batch-local only
File: `crates/pcloud-engine/src/fs_events.rs:17-20, 62-97`
`coalesce_window_ms: 250` is declared but never used. The normalize_events
coalesces within a single call only. For real-world editors that write a
burst across 100 ms spread over several sync cycles, each burst is not
deduplicated. The TODO is acknowledged. Fix: carry a persistent window or
timestamp each event.

### H7. Audit persistence failure logs only — breaks tamper-evident chain silently
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:522-547`
Comment explicitly says "audit persistence failures must not be silently
swallowed" but the handler only calls `log::error!` and returns. The cycle
proceeds as if auditing succeeded. CLAUDE.md rule: "audit persistence
failures surfaced instead of being silently ignored". Fix: propagate via
`CycleResult` so callers can degrade, or halt the loop into a surfaced
error state.

---

## MEDIUM

### M1. Checksum mismatch disposition never surfaced from transfer execution
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:379-406, 477-503`
All execution errors are classified as `RetryableNetworkError` regardless of
actual cause. `ChecksumMismatch` / `PermissionDenied` / `InvalidPath` paths in
`recovery.rs` are unreachable from the live runtime. Fix: inspect error type
and classify; permission/4xx should not be retried as network.

### M2. newest_wins conflict has no timestamp comparison
File: `crates/pcloud-engine/src/conflict_resolver.rs:174-183`
Silently falls back to PreferRemote — not "newest wins". Users selecting
`newest_wins` get server-wins. Either rename the policy or plumb mtimes.

### M3. `resolve_conflict_by_path` swallows `remote_file_id` context
File: `crates/pcloud-engine/src/conflict_resolver.rs:146-158`, `lib.rs:436-471`
PreferRemote downloads are emitted with `remote_file_id: None`, forcing a
secondary server lookup each time a conflict is resolved (TODO acknowledged).

### M4. Bandwidth scheduling is entirely absent
File: `crates/pcloud-engine/src/scheduler.rs` (whole module)
No bandwidth limiter / token bucket / rate cap. C client had bandwidth
throttling hooks. Nothing paces uploads/downloads; the only limits are
`max_parallel_*` counts. Fix: add a byte-bucket limiter on the transfer
coordinators.

### M5. Resume state — planner has no durable queue
File: `crates/pcloud-engine/src/scheduler.rs:49` + `sync_loop_runtime.rs`
`queued_operations: Vec<PlannedOperation>` is in-memory. On crash after
planning but before execution, work is lost (requires re-scan/re-diff).
Works because scans and diff polling are idempotent — but #C3 pairs badly
with this: cursor advanced + queue lost ⇒ permanent loss. Fix: spill plan
to a pending_ops table (already noted as TODO in #H1).

### M6. `EngineShell::evict_sync_root` does not untrack the FsWatcher/IncrementalScanTracker
File: `crates/pcloud-engine/src/lib.rs:512-517` vs
`crates/pcloud-daemon/src/sync_loop_runtime.rs:208-211`
Only `RealSyncLoopRuntime::remove_watcher` stops watchers; nothing calls it
on pause (`EngineShell::pause_sync_root`) or from the IPC handler for sync
root deletion (not visible in this file, but evict_sync_root does not reach
the watcher map). Orphaned inotify handles leak.

### M7. `walk_local_tree` has no cycle detection / symlink policy
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:607-646`
`read_dir` + recursion with no symlink follow guard and no depth bound. A
symlink loop inside a sync root causes infinite recursion and stack blow-up.
Fix: skip symlinks (document), or detect with a visited set keyed by
canonical path.

### M8. `EngineShell` derives `Clone, PartialEq, Eq` on a struct containing
non-serialisable transient state once StallDetector is added; already seen:
`scheduler.queued_operations` is cloned for every IPC snapshot. Performance
only — not correctness.

---

## LOW

### L1. `planner.rs:123` picks `sync_id` from first dropped candidate only
Multi-root overflow logs a misleading `sync_id`. Cosmetic.

### L2. `ConflictResolver::RenameBoth` is documented as manual review
File: `conflict_resolver.rs:185-195`. Not a bug; the variant exists but
behaves identical to ManualReview. Confusing UX.

### L3. `execute_downloads` reads entire file into memory
File: `sync_loop_runtime.rs:370-378`. `download_bytes` is unchunked; large
files will OOM. Not in scope of this section but visible here.

### L4. `read_upload_payload` reads whole file via `read_staged_path(0, usize::MAX)`
File: `sync_loop_runtime.rs:672-684`. Same OOM concern.

### L5. `validate_relative_path` triplicated across `fs_events.rs`,
`diff_poller.rs`, `local_scan.rs`. Drift risk. Extract to shared helper.

### L6. `stall_detector.rs:117-121` zero-timeout test documents infinite
stalling; nothing prevents config misuse. Clamp to reasonable minimum.

### L7. `ingest_candidates_filtered` always calls `replace_queue` (see #H2).
Public helpers are not named "replace" so callers may not realise the
destructive semantics. Rename or document.

### L8. `RealSyncLoopRuntime::new` opens SQLite with `synchronous=NORMAL`
(line 146). Acceptable for WAL, but combined with #C3 raises durability
concern. Consider `FULL` for the cursor write specifically.

---

## Positive observations

- EngineShell is `Clone + Eq`, deterministic, and cleanly separated from I/O.
- Planner conflict taxonomy is exhaustive and symmetric.
- `DeletePolicy::for_sync_type` correctly treats `BackupArchive` as deletion-safe
  and ultra-safe-mode suppresses all deletes (`planner.rs:212-241`).
- Path validation uniformly rejects absolute/`..`/empty segments.
- Test coverage on pure engine modules is high (unit + serde roundtrips).
- Debug impl on `RealSyncLoopRuntime` uses `finish_non_exhaustive` and does
  not leak auth tokens (`sync_loop_runtime.rs:117-122`, test line 846-865).

Priority fix order: C2 → C3 → C1 → H4 → H5 → H2/H3 → M1 → H1/M5 → M7.
