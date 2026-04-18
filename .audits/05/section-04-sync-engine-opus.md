# Section 4 — Sync Engine Runtime Audit (Opus, Audit 05)

Scope: `crates/pcloud-engine/` + `crates/pcloud-daemon/src/{sync_loop.rs, sync_loop_runtime.rs}`.
Reviewed: planner, scheduler (fair batch), conflict resolver, stall detector,
fs-event debounce, back-pressure, pause/resume persistence, audit persistence,
resume state (dead-letter + scheduler queue), bandwidth scheduling, and
verification of audit-04 fixes.

## Audit-04 fix verification

| Audit-04 Finding | Fix Landed | Evidence |
|---|---|---|
| C1 zero-metadata poisoning | YES | `commit_diff_batch` (sync_loop_runtime.rs:1082-1115) only `delete`s metadata on remote deletes; no fabricated zero upserts |
| C2 `resolve_upload_parent` nested files | PARTIAL | `walk_local_tree` now resolves parent via `FileMetadataRepository::resolve_path` / `get_by_parent_and_name` (sync_loop_runtime.rs:912-1047). Cache-miss still returns `None` → `InvalidPath` → potentially `Terminal` (see H1 below) |
| C3 cursor-before-ingestion | YES | ingestion runs before `commit_diff_batch`; cursor persists atomically after (sync_loop_runtime.rs:449-470) |
| Planner dead-letter | YES | `plan_with_overflow` returns overflow; `persist_planner_overflow` writes `value_kv` (sync_loop_runtime.rs:258-281) |
| Fair scheduler | YES | `Scheduler::next_batch` enforces per-root cap (scheduler.rs:180-218) and is the one used by `advance_transfer_cycle` (lib.rs:657) |
| StallDetector wired | PARTIAL | Constructed (sync_loop_runtime.rs:228) and used in `advance_transfers` (sync_loop_runtime.rs:524-533), but only measures scheduler-dispatch progress; completions do not `mark_progress` |
| Pause persistence | YES | Restored from `SyncGraphRepository.paused` at startup (sync_loop_runtime.rs:173-179) |

## CRITICAL

None.

## HIGH

### H1. `resolve_newest_wins` reads mtime from a sync-root-relative path
File: `crates/pcloud-engine/src/conflict_resolver.rs:210-228`
`std::fs::metadata(path)` is called with `path` = `"docs/report.txt"` (sync-root-relative). Unless CWD happens to be the sync root, this resolves against CWD — typically the daemon working dir. Every `NewestWins` resolution therefore silently falls through to `prefer-remote`. Also, `local_mtime > now - 30s` is a bizarre "recent write" heuristic, not a real local-vs-remote comparison, and the `remote_mtime_secs` parameter is discarded entirely in the fallback branch. Pass an absolute path or the `ConflictKind` remote_modified payload; tests at conflict_resolver.rs:378-396 memorialize the broken behavior.

### H2. In-flight transfers are durable-hole between `next_batch` drain and completion
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:511-533`, `crates/pcloud-engine/src/scheduler.rs:175-218`
`Scheduler::next_batch` **removes** items from `queued_operations`; `advance_transfers` then calls `persist_scheduler_queue` which writes the now-shrunken queue. Between drain and upload/download completion the dispatched ops exist only in the `UploadCoordinator.active_uploads` / `DownloadCoordinator.active_downloads` in-memory sets — there is no persistence of active transfers. A crash here loses the work forever (the planner won't re-emit it until the next local scan / diff discovers the still-missing file, and if the file was already deleted upstream it never comes back). Pair the drain + coordinator-accept + persist into a single "dispatched + active" checkpoint, or persist `active_uploads`/`active_downloads` alongside the scheduler queue.

### H3. `commit_diff_batch` toggles `synchronous=FULL` on the shared connection without locking
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:1086-1115`
`pragma_update(None, "synchronous", "FULL")` → tx → `pragma_update(None, "synchronous", "NORMAL")` mutates connection-global state. The same `Connection` is used from `poll_remote_diff`, `run_local_scan`, `list_sync_roots`, `persist_planner_overflow`, `persist_scheduler_queue`. While the sync loop runs single-threaded, a concurrent `SyncGraphRepository::load` on the same connection would observe the transient FULL state, and any early return between the two pragma calls (e.g. `FileMetadataRepository::delete` returning `Err` — caught by `?` at line 1104) **leaves the connection permanently at FULL**, silently re-fsyncing every subsequent write for the rest of the daemon's life. Wrap the pragma toggle in a guard struct with `Drop` that always restores NORMAL.

### H4. `advance_transfers` only marks progress on dispatch, not on completion
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:519-533`
`stall_detector.mark_progress()` fires only when `next_batch()` returns non-empty. A healthy daemon uploading a multi-GiB file for 3 minutes will dispatch once, drain the queue, and then `check_stall` fires at T=120s even though real work is happening. Also `execute_downloads`/`execute_uploads` successful completions (sync_loop_runtime.rs:489, 602) do not call `mark_progress`. Call `mark_progress` from `mark_transfer_completed` success path or from download/upload success branches.

### H5. `RecoveryFailure::InvalidPath` disposition ends uploads under subdirs permanently
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:653-674, 1167-1184`
Audit-04 C2 is only half-fixed. `walk_local_tree` populates `remote_parent_folder_id` via cache, but when the metadata cache is cold (fresh sync, new daemon), every nested file gets `None` → `resolve_upload_parent` → `InvalidPath` → `classify_failure` → likely `Terminal` disposition. `mark_transfer_failed` then drops the item with no retry path. `walk_local_tree`'s cold-cache fall-through comment at sync_loop_runtime.rs:884 says "best-effort contract" but the consumer treats `None` as terminal. Either (a) stage folder-create operations first and feed the returned `RemoteFolderId` into dependent uploads, or (b) route cold-cache cases through `RecoveryFailure::RetryableNetworkError` so the planner re-attempts next cycle.

## MEDIUM

### M1. Scheduler fairness cap is too tight under low concurrency
File: `crates/pcloud-engine/src/scheduler.rs:193-198`
`per_root_cap = ceil(global_limit / num_roots).max(1)`. With `max_parallel_uploads=4, max_parallel_downloads=4` (global=8) and 8 roots, each root gets 1 slot; with 20 roots, each gets 1 and batch size stays at 8 meaning 12 roots never appear in a batch cycle. No round-robin over skipped roots across successive cycles — the BTreeSet iteration order means the same high-sync-id roots can starve repeatedly if priority ordering places them late. Maintain a next-root-to-dequeue cursor across calls.

### M2. Transfer-coordinator eviction on pause/evict never persists to the dead-letter queue
File: `crates/pcloud-engine/src/lib.rs:760-772, 823-837`
`evict_sync_root` and `pause_sync_root` call `scheduler.evict_sync_id` + `uploads/downloads.evict_sync_id`, wiping the queued and in-flight state. A paused-then-resumed root **must** rescan to rediscover work — the planner has no record of what was in-flight. Under H2 this compounds: active uploads mid-byte-stream are abandoned on pause.

### M3. `FsEventIngestor` has no cross-batch debouncing
File: `crates/pcloud-engine/src/fs_events.rs:19-26`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:481-487`
The docstring says debouncing happens in `FsWatcher::debounce_loop`. But `drain_watcher_events` non-blocking-drains everything available into `IncrementalScanTracker` each cycle. If FsWatcher emits events as they arrive (no internal debounce), a `vim`-style editor save (remove + create + write within 10ms) produces 3 candidates for the same path. `normalize_events` dedupes **within a single call**, but if those 3 events arrive across two cycles they produce 2 separate planner ticks. Add a short (e.g. 500ms) coalesce window before ingestion.

### M4. Bandwidth scheduling is entirely absent
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs` (grep for "bandwidth", "rate", "throttle" — none in engine/daemon sync path)
No token bucket, no per-root rate limit, no global ceiling. Backups of TB-scale roots on home networks will saturate upstream. `pcloud-config/src/rate_limit.rs` exists but is not wired into sync_loop_runtime. Wire it into both upload and download execution.

### M5. `emit_cycle_audit` skips all idle-cycle persistence
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:764-771`
`if total_errors == 0 && total_uploads == 0 && total_downloads == 0 { return Ok(()); }` — a cycle with only conflicts or roots_processed>0 emits nothing. The chain gap is indistinguishable from "daemon stopped running". Emit a heartbeat event at least once per N cycles or once per hour.

### M6. `resolve_upload_payload_len` / `borrow_upload_payload` race (TOCTOU)
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:1214-1240, 642-672`
Length probe returns `len`, `upload_create` sends `len` to server, second borrow may be `None` or different-length (cache can be evicted/rewritten between). The `None` branch is handled (marked failed) but a differing non-empty buffer is silently uploaded with the old advertised length → protocol inconsistency. Pin the buffer reference before `upload_create` or re-validate length == session.length before `upload_bytes`.

### M7. `walk_recursive` inode cycle detection ignores device id
File: `crates/pcloud-daemon/src/sync_loop_runtime.rs:1001-1014`
`HashSet<u64>` of inodes without `dev_t`. Two different filesystems mounted under the sync root can share inode numbers → legitimate directories skipped as "cycle". Use `(st_dev, st_ino)` tuple.

### M8. `ConflictResolver::resolve_conflicts` passes `None, None` always
File: `crates/pcloud-engine/src/lib.rs:585-592`
Bulk resolve never supplies mtimes even when the scheduler has remote-delete timestamps available. Combined with H1 this means `NewestWins` is effectively a server-wins alias. Plumb remote mtime from `ConflictKind` payload (once added per TODO at conflict_resolver.rs:154).

## LOW

- **L1.** `staged_download_path` uses only 8 bytes of SHA-256 (16 hex) — 64-bit birthday at ~2^32 paths; acceptable but document (sync_loop_runtime.rs:1131-1145).
- **L2.** `ReconcileWorker::interval` default 300s vs C's 10s on change-event; the change-event path covers fast, but cold-start first full scan takes 300s (reconcile_worker.rs:36).
- **L3.** `ValuesRepository::get_string` failures for dead-letter/scheduler on bootstrap silently skip restoration (sync_loop_runtime.rs:181, 211) — no audit event for restore-attempted. A corrupt overflow is warn-logged and deleted; no user notice.
- **L4.** `SCHEDULER_QUEUE_KEY` persistence is best-effort JSON of entire queue on every ingest. For large queues this allocates + serialises + writes on every local-scan tick — could bloat WAL. Consider delta persistence or throttling.
- **L5.** `sync_loop_main` holds no retry budget around `runtime.list_sync_roots()` — transient DB error returns empty Vec and evicts every root from `prev_root_ids` (sync_loop.rs:404-420). One bad read drops all watchers.
- **L6.** `spawn_sync_loop` panics on thread spawn failure (sync_loop.rs:559) rather than returning `Err`; daemon startup becomes an unrecoverable crash on `EAGAIN`.
- **L7.** `evict_sync_root`/`pause_sync_root` do not clear `planner_overflow` for that sync_id — deferred candidates for a removed root get replayed on next tick (lib.rs:720-772). Filter `planner_overflow` by sync_id in eviction.
- **L8.** `FsWatcher::start` failure falls back to poll-only with only a `warn!` (sync_loop_runtime.rs:383-391); no health-status bit exposes "degraded root".

## Priority fix order

H3 (silent fsync-forever) → H2 (lost in-flight) → H1 (broken NewestWins) → H5 (cold-cache upload drops) → H4 (false stall on long transfers) → M1 → M4 → M3 → rest.

## Positive observations

- Audit-04 C1, C3, planner dead-letter, scheduler fairness, and pause-persistence fixes are landed and the persistence format is deterministic (`snapshot_scheduler_queue` sorts by `(sync_id, priority, path)`).
- Download path is now write-through-to-disk with an in-mem mirror threshold (sync_loop_runtime.rs:116, 457-495) — audit-04 L-3 cleanly closed.
- Upload path is zero-copy borrow with the `read_upload_payload_zero_copy_for_large_files` regression test pinning the invariant.
- Symlink policy is conservative and correct (sync_loop_runtime.rs:978-984).
- `FsEventIngestor` phantom `coalesce_window_ms` removed — no more misleading field.
- No unsafe, no panics on hot path, no secret leakage in `Debug`.
