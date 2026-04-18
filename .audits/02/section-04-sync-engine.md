# Section 4: Sync Engine & Runtime
## Date: 2026-04-17
## Scope
Audit of: crates/pcloud-engine/src/, crates/pcloud-engine/tests/, crates/pcloud-store/src/, crates/pcloud-resilience/src/, crates/pcloud-daemon/src/runtime.rs, (requested sync_backend.rs does NOT exist under crates/pcloud-daemon — see Note 1).

## Note 1 — missing file in audit request
The audit scope names `crates/pcloud-daemon/src/sync_backend.rs`; that file does NOT exist. The sync backend in this workspace is `crates/pcloud-backends/src/sync_backend.rs`. The runtime file `crates/pcloud-daemon/src/runtime.rs` is 6202 lines; its sync-related sections were sampled via grep only. Several daemon files matter for this section: `sync_loop.rs` (819 LOC), `sync_loop_runtime.rs` (955 LOC), `transfer_bridge.rs` (693 LOC).

## Findings Summary

### CRITICAL [3]
### HIGH [7]
### MEDIUM [9]
### LOW [6]

---

## Detailed Findings

### CRITICAL

#### C1 — `stall_detector` module is orphaned / NOT wired into the engine
- File: `crates/pcloud-engine/src/stall_detector.rs` (122 LOC)
- File: `crates/pcloud-engine/src/lib.rs:15-41` (pub mod list)
- Severity: **CRITICAL**
- Evidence: `lib.rs` declares `conflict_resolver, diff_events, diff_poller, fs_events, local_scan, planner, reconcile_worker, recovery, scheduler, selective, session_manager, transfers` but NOT `stall_detector`. `grep -n stall_detector crates/pcloud-daemon/src/runtime.rs` returns no matches. `EngineShell` (lib.rs:66-103) has no `stall_detector` field and the sync loop (`sync_loop.rs:364-429 sync_loop_main`) never calls `mark_progress` or `check_stall`.
- Impact: A silent hang — network wedged, lost watcher events, stuck transfer coordinator, dead upload coordinator — will never surface any warn log. Operators get no feedback that the daemon is making no progress. Contradicts the docstring "wired into the engine loop" and the audit question #9.
- Fix: (1) Add `pub mod stall_detector;` to `pcloud-engine/src/lib.rs`. (2) Add `pub stall_detector: stall_detector::StallDetector` to `EngineShell` (init via `StallDetector::new(DEFAULT_STALL_TIMEOUT)`). (3) In `sync_loop.rs:sync_one_root` call `engine.stall_detector.mark_progress()` after any successful transfer/scan; at the top of `sync_loop_main`'s wait loop call `engine.stall_detector.check_stall()`. (4) Expose a counter and bubble it up via `SyncLoopStatus`.

#### C2 — `pcloud-resilience/src/transport.rs` is orphaned (not declared as a module)
- File: `crates/pcloud-resilience/src/transport.rs` (583 LOC)
- File: `crates/pcloud-resilience/src/lib.rs:47-56` (module list)
- Severity: **CRITICAL**
- Evidence: `transport.rs` exists but `lib.rs` only declares `circuit_breaker, clock, metered, pacing, rate_limit, retry, timeout` — no `transport`. Because the file is never compiled, the body contains `RetryDecision::Exhausted` (lines 289, 299, 308) referencing a variant that does NOT exist in the real enum (`retry.rs:51-59` has only `Retry` and `GiveUp`). Cargo still succeeds because nobody compiles the file.
- Impact: The centralized `ResilientTransport` (retry-aware HTTP executor with MethodRetryPolicy + Retry-After + global budget) is non-functional. All retry wiring in `pcloud-proto/src/resilient_transport.rs:291` and `pcloud-backends/src/sync_backend.rs:880` re-implements the pieces ad hoc, which is what the "GlobalRetryBudget wired in?" audit question asks about — answer: **no**, there is no single wiring point.
- Fix: (a) If the module is intended to be live: add `pub mod transport;` to `lib.rs`, delete all `RetryDecision::Exhausted` references (or add the variant to `retry.rs`), then re-compile. (b) If the module is dead: delete the file. The current state silently hides a broken implementation behind a passing `cargo check`.

#### C3 — Migrations do NOT use `SAVEPOINT`; individual migration steps run outside a transaction
- File: `crates/pcloud-store/src/migrations.rs:80-118` (`apply_plan`)
- File: `crates/pcloud-store/src/schema.rs:36-301` (every `apply_schema_vN`)
- Severity: **CRITICAL**
- Evidence: `grep -rn SAVEPOINT crates/pcloud-store` returns zero matches. `apply_plan` calls each `apply_schema_vN` sequentially without an outer `TransactionBoundary::immediate`. Each per-version function uses `execute_batch(...)` containing DDL + `PRAGMA user_version = N` as separate statements. `execute_batch` in rusqlite runs each statement auto-commit; `ALTER TABLE` + `PRAGMA user_version` are therefore two independent commits.
- Impact: A crash between the DDL and the PRAGMA inside `apply_schema_v8` (which also runs `rebuild_hash_chain`) leaves the audit table widened with empty hash columns AND `user_version = 7`. On restart, v8 runs again and tries to `ALTER TABLE ADD COLUMN` a column that already exists — the `column_exists` guards protect this particular case, but the general pattern is unsafe (v6's `ALTER ... DEFAULT 3` has no idempotence guard). The audit question "Does every migration step use SAVEPOINT?" — **no**, none do.
- Fix: Wrap each `apply_schema_vN(conn)` call in `apply_plan` inside a `TransactionBoundary.immediate(conn, |c| apply_schema_vN(c))`. Inside each schema step, SQLite auto-commits DDL even inside a transaction (SQLite serializes DDL into transactions for WAL); the resulting semantics become all-or-nothing per step. For larger multi-step migrations, add explicit `SAVEPOINT migration_vN` / `RELEASE SAVEPOINT migration_vN` so that partial failure doesn't leave half-applied ALTERs.

---

### HIGH

#### H1 — Scheduler enqueue truncates front of queue, losing high-priority items
- File: `crates/pcloud-engine/src/scheduler.rs:112-121`
- Severity: **HIGH**
- Evidence: `enqueue` inserts via `partition_point` maintaining priority order (lower priority first), then `if self.queued_operations.len() > 100_000 { ... self.queued_operations.truncate(100_000); }`. `Vec::truncate(100_000)` drops the TAIL (entries beyond index 100k) — i.e. the LOWEST-priority items. The warning text says "oldest operations truncated" but actually the lowest-priority items are dropped (correctly). However, the log message is misleading, AND the warning fires every subsequent enqueue while the queue stays above the cap (no rate limit).
- Impact: Log spam under pressure; operator misled about what was dropped.
- Fix: Fire the warning only on the transition from ≤100k to >100k (track a `over_cap: bool` flag). Correct the message: "scheduler: queue capped at 100,000 operations; lowest-priority entries truncated (total_over_cap=N)". Emit a metric rather than just a log line.

#### H2 — Scheduler `next_batch` round-robin fairness is O(N²) and builds groups on every call
- File: `crates/pcloud-engine/src/scheduler.rs:144-190`
- Severity: **HIGH**
- Evidence: Each call iterates all `queued_operations` and uses `groups.iter_mut().find(|(gid, _)| *gid == id)` — O(N·M) per call where M = number of distinct sync ids. Then it calls `self.queued_operations.remove(i)` inside a loop, each `remove` is O(N).
- Impact: At 100k queued ops across 100 sync roots, the per-batch cost is roughly 100k·100 + batch_size·100k = tens of millions of ops per batch. This is the hot dispatcher path called every poll interval.
- Fix: Replace the linear-scan grouping with `BTreeMap<SyncId, VecDeque<usize>>` built once; do round-robin pops; replace `Vec::remove(i)` with a single pass retaining non-selected indices.

#### H3 — `FsEventIngestor` coalescer is O(N²) in-memory and has no cap
- File: `crates/pcloud-engine/src/fs_events.rs:60-96`
- Severity: **HIGH**
- Evidence: `coalesced.iter_mut().find(|candidate| candidate.path == event.path)` is O(N) per incoming event → O(N²) for a batch. Despite the docstring claiming "HashMap-based O(1) dedup" (audit question #4), the code uses a `Vec<FsEvent>` linear scan. There is no `max_events` cap; an inotify burst of 1M events will build a 1M-entry Vec.
- Impact: Either the claim of O(1) dedup is wrong, or the code does not match its spec. CPU-bound freeze under file-storm conditions (e.g. `npm install`, `rsync`, bulk download). The audit question "Max event cap with warning?" — **no**.
- Fix: Replace with `let mut by_path: std::collections::HashMap<String, usize> = ...`; cap to e.g. 10k events per normalize call and emit `warn!` with overflow count.

#### H4 — `fs_events.rs` coalescer loses intermediate CREATE then DELETE semantics
- File: `crates/pcloud-engine/src/fs_events.rs:68-73`
- Severity: **HIGH**
- Evidence: When the same path has [Create, Write, Remove] within a window, the code simply overwrites `existing.kind = event.kind` so the final candidate is `Remove`. But a sequence like [Remove, Create] (rename-in-place via editor atomic save) collapses to `Create` — correct. [Create, Remove] collapses to `Remove` — which omits the implicit "delete something that never existed remotely" scenario. This is not obviously wrong but is not documented.
- Impact: Atomic-save patterns on Linux (editor writes `.swp`, renames over target) can be mis-classified. Without timestamps the ordering is input-order dependent; the caller (`FsWatcher`) debounces with `HashMap<String, (kind, entry_kind, Instant)>` which loses ordering too.
- Fix: Track a small per-path state machine (None→Create→Write→Remove stays as Remove; Remove→Create becomes Write; etc.). Add a property test.

#### H5 — `newest_wins` conflict policy does not fall back correctly when one timestamp is missing
- File: `crates/pcloud-engine/src/conflict_resolver.rs:177-191`
- Severity: **HIGH**
- Evidence: `resolve_newest_wins` returns `prefer_remote` whenever BOTH timestamps aren't present. If only `local_mtime = Some(T_local)` and `remote_mtime = None` (common: remote just deleted so no mtime), code returns prefer-remote (→ DeleteLocal), discarding the local modification silently even though the local is demonstrably newer.
- Impact: Data loss under the "remote-delete vs local-modify" kind when using NewestWins, despite the user having chosen that policy expecting their newer local copy to win.
- Fix: In `resolve_newest_wins`, if exactly one mtime is available, prefer that side. Document the behavior. Add a test for `(Some, None)` and `(None, Some)`.

#### H6 — `UploadCoordinator::accept_batch` clears previous in-flight tasks — causes mid-flight drops
- File: `crates/pcloud-engine/src/transfers/uploads.rs:48-68`
- File: `crates/pcloud-engine/src/transfers/downloads.rs:48-68`
- Severity: **HIGH**
- Evidence: `accept_batch` unconditionally `self.active_uploads.clear()` (and `pending_remote_deletes.clear()`, `pending_directory_creates.clear()`) before accepting the new batch. If a previous batch had an upload still streaming (not yet `mark_completed`), the TransferTask is dropped on the floor — `ChunkedUploadTracker` state is lost, the `upload_resume_state` table row persists but the in-memory coordinator forgets the path.
- Impact: On every scheduler tick the coordinator "forgets" any in-flight transfer whose mark_completed hasn't landed yet. Either the bridge layer (`transfer_bridge.rs`) synchronously drives each batch to completion before the next `accept_batch` — in which case this is fine — or it's asynchronous and data-loss bugs are possible. The current sync_loop (`sync_loop.rs:254-300`) calls `advance_transfers → execute_downloads → execute_uploads` synchronously so the pattern holds by accident, but the type does not enforce it.
- Fix: Document the invariant. Better: make `accept_batch` EXTEND rather than REPLACE, and require explicit `drain_batch` between cycles; panic if active set is non-empty when a new batch arrives.

#### H7 — Planner `max_operations_per_tick` is a silent drop (no metric, no warn log)
- File: `crates/pcloud-engine/src/planner.rs:85-104`
- Severity: **HIGH**
- Evidence: `while idx < sorted.len() && operations.len() < self.max_operations_per_tick` stops early once the cap is hit. The remaining candidates are silently dropped — the planner is not stateful so they do not reappear on the next tick unless the caller re-ingests the same list.
- Impact: Large scans (say 10k changes) with the default `max_operations_per_tick = 1024` produce a plan that covers only the first 1024 paths (lexicographically). The other 9k are DROPPED unless the caller re-runs. This contradicts the docstring "Excess candidates are deferred — a later tick will process them once the scheduler drains."
- Fix: Either (a) actually persist the deferred tail (return `(Vec<PlannedOperation>, Vec<SyncCandidate>)` so the caller can re-submit), or (b) remove the cap and rely on downstream back-pressure. Today's behaviour is neither.

---

### MEDIUM

#### M1 — `IncrementalScanTracker::decide` discards watcher events on every full-scan cycle
- File: `crates/pcloud-engine/src/local_scan.rs:212-215`
- Severity: **MEDIUM**
- Evidence: When `needs_full` is true, `self.pending_events.remove(&sync_id)` discards all queued watcher events.
- Impact: If the full scan itself misses some change (e.g. ACL preventing a read; filesystem in the middle of a rename), the watcher event that could have caught it is dropped. Minor race.
- Fix: Merge watcher events into the scan output rather than dropping them.

#### M2 — Scheduler `replace_queue` is destructive — unresolved conflicts from previous cycle are dropped
- File: `crates/pcloud-engine/src/scheduler.rs:80-87`
- File: `crates/pcloud-engine/src/lib.rs:208-212 ingest_candidates`
- Severity: **MEDIUM**
- Evidence: `replace_queue(operations)` overwrites `queued_operations` wholesale. `ingest_candidates` calls `replace_queue`. A `PlannedOperation::Conflict` that was queued in the previous cycle and has not yet been operator-resolved is dropped when the next ingest arrives.
- Impact: Conflicts appear, get listed by `list_unresolved_conflicts`, operator runs `pcloudc conflict list`, then the next sync cycle wipes them — the operator is racing the engine.
- Fix: Use `merge_queue` semantics: retain every `PlannedOperation::Conflict` across cycles until explicitly resolved or evicted.

#### M3 — `DiffPoller` has NO cursor persistence; the poller itself only configures `batch_limit`
- File: `crates/pcloud-engine/src/diff_poller.rs:14-24`
- Severity: **MEDIUM**
- Evidence: The struct has only `batch_limit: u64`. No `cursor` field. Cursor persistence is handled OUTSIDE the engine in `pcloud-store/src/repositories/diff_state.rs`, and the engine code never consults it — `normalize_batch` is stateless.
- Impact: The audit question "cursor persistence?" has a split answer: the STORE supports it, the ENGINE does not participate. Whether the daemon wires them up is the real question — that ownership is entirely in the backends/daemon layer (outside the engine crate). Flag for documentation.
- Fix: Either pull the cursor into `DiffPoller` (load on boot, advance on each batch) or document the handoff clearly in `diff_poller.rs`.

#### M4 — `DiffPoller::normalize_batch` fails the entire batch on the first malformed entry
- File: `crates/pcloud-engine/src/diff_poller.rs:73-82`
- Severity: **MEDIUM**
- Evidence: `.map(...).collect::<Result<Vec<_>, _>>()` — one bad path (e.g. with `\` on a server glitch) kills the whole batch. The audit question "Error recovery on malformed entries?" — **no**, all-or-nothing.
- Fix: Log and skip individual malformed entries (bump a metric) so a single poisoned path cannot wedge the diff cursor.

#### M5 — `RecoveryManager` taxonomy missing several failure classes
- File: `crates/pcloud-engine/src/recovery.rs:68-99`
- Severity: **MEDIUM**
- Evidence: `RecoveryFailure` only enumerates `RetryableNetworkError, ChecksumMismatch, InvalidPath, PermissionDenied`. Missing: `QuotaExceeded`, `RateLimited` (429), `AuthenticationExpired`, `ServerInternalError (5xx)`, `LocalDiskFull`, `ConflictOnUpload` (server rejected due to `ifhash`). The audit question "all failure classes classified?" — **no**.
- Fix: Extend the enum; map each one to an appropriate `FailureDisposition` (e.g. `QuotaExceeded → Terminal until operator frees space`, `RateLimited → RetryLater with server-provided Retry-After`).

#### M6 — `ConflictResolver::rename_both` uses string-literal `.conflict-local` suffixes — collides with real files
- File: `crates/pcloud-engine/src/conflict_resolver.rs:193-212`
- Severity: **MEDIUM**
- Evidence: If a user already has `report.conflict-local.txt` (from a prior resolution), a new conflict on `report.txt` will generate the SAME rename target, overwriting the older conflict artifact.
- Fix: Append a timestamp or monotonically-increasing suffix — `report.conflict-local.20260417-153012.txt` — to guarantee uniqueness.

#### M7 — `SelectivePolicy` path validation only strips leading `/`; does NOT reject `..` or absolute paths
- File: `crates/pcloud-engine/src/selective.rs:230-239`
- Severity: **MEDIUM**
- Evidence: `matches` calls `relative_path.trim_start_matches('/')` then runs globset. A pattern `!../etc/passwd` compiled from `.pcloudsync` becomes a glob that tests against `../etc/passwd` — the glob may or may not match anything, but the parser does NOT reject the dangerous pattern. The audit question "does it reject .. and absolute paths?" — **no**, the parser accepts them.
- Fix: In `parse`, validate each pattern: reject patterns containing `..` segments or starting with `/` (other than Unix absolute-root which globset treats specially). Return `SelectiveError::ParseError` with a helpful message.

#### M8 — Back-pressure: no memory/disk budget enforcement anywhere in the engine
- File: `crates/pcloud-engine/src/scheduler.rs`, `transfers/{uploads,downloads}.rs`
- Severity: **MEDIUM**
- Evidence: Scheduler cap = 100k items (by count, not bytes). UploadCoordinator default `chunk_size_bytes = 8 * 1024 * 1024` (8 MB) with up to `max_parallel_uploads = 4` uploads → 32 MB burst. DownloadCoordinator `max_range_requests = 8`. No global byte cap, no disk-free probe before a download, no memory budget shared across coordinators. The audit question "memory/disk budget enforcement, flow control to API?" — **no**, only rate_limit / BandwidthPacer in the resilience crate but no wiring.
- Fix: Introduce a `TransferBudget { in_flight_upload_bytes, in_flight_download_bytes, total_memory_bytes }` shared via Arc; gate `accept_batch` on available budget; fail-fast with `Deferred` rather than silently queueing.

#### M9 — GlobalRetryBudget defined but NOT wired into the engine or transport
- File: `crates/pcloud-resilience/src/global_budget.rs` (198 LOC)
- File: `crates/pcloud-resilience/src/lib.rs:47-66` — NOT exported
- Severity: **MEDIUM**
- Evidence: `grep -c GlobalRetryBudget crates/pcloud-resilience/src/lib.rs` = 0 (no re-export). `grep -n GlobalRetryBudget crates/pcloud-daemon/src/{runtime,sync_loop,transfer_bridge}.rs` = 0. The module exists, is tested in isolation, and is unused. The audit question "GlobalRetryBudget wired in?" — **no**.
- Fix: Re-export from `lib.rs` (`pub mod global_budget; pub use global_budget::GlobalRetryBudget;`). Wire it into `pcloud-proto/resilient_transport.rs` alongside the CircuitBreaker and into the backend's upload/download retry loops.

---

### LOW

#### L1 — `SessionManagerActor` is a 22-line stub with only a `refresh_margin_secs` field
- File: `crates/pcloud-engine/src/session_manager.rs:1-22`
- Severity: **LOW**
- Evidence: No per-sync-root state tracked. The audit question "per-root state tracked correctly?" — **no**, the type is a placeholder.
- Fix: Document as not-yet-implemented; add TODO tracking.

#### L2 — `ChunkedUploadTracker::advance` can overflow `acked_offset` silently
- File: `crates/pcloud-engine/src/transfers/uploads.rs:217-221`
- Severity: **LOW**
- Evidence: `self.acked_offset += bytes_written;` — no saturating add. For a malicious / buggy `bytes_written` near `u64::MAX` this wraps.
- Fix: `self.acked_offset = self.acked_offset.saturating_add(bytes_written);`. Also cap at `total_size`.

#### L3 — `TransactionBoundary` docs claim panic safety but Drop is not used
- File: `crates/pcloud-store/src/tx.rs:63-89`
- Severity: **LOW**
- Evidence: The docstring warns "If `work` panics, the transaction is not rolled back by this method". That is intentional but fragile — a panic while holding a `StoreHandle` mutex guard (poisoning is recovered per `lib.rs:348-351`) leaves the SQLite connection with an open IMMEDIATE transaction; the NEXT writer must trip SQLITE_BUSY or a stale reserved lock.
- Fix: Add an RAII `TransactionGuard` wrapper (not a replacement) that rolls back on drop if not committed; keep the explicit `immediate` for defensive call sites.

#### L4 — Scheduler `next_batch` combined upload+download limit can starve one side
- File: `crates/pcloud-engine/src/scheduler.rs:145`
- Severity: **LOW**
- Evidence: `let limit = (self.max_parallel_uploads + self.max_parallel_downloads).max(1);` — a single batch can be 100% uploads (or downloads), because the limit is their SUM. The upload and download coordinator slot counts aren't actually enforced here.
- Fix: Split into two passes: first pass selects up to `max_parallel_uploads` upload ops by priority, second pass picks `max_parallel_downloads` download ops.

#### L5 — Power/battery awareness is present for the integrity sweeper only, NOT for the sync engine
- File: `crates/pcloud-daemon/src/integrity_sweeper_service.rs:393-580`
- Severity: **LOW**
- Evidence: `pause_on_battery` is wired ONLY to the integrity sweeper scheduler. The sync loop (`sync_loop.rs`) never checks power state. The audit question "any platform signal for AC/battery? Flag if missing." — partially present; not used to gate transfers.
- Fix: Extend `SyncLoopConfig` with `pause_on_battery: bool` and reuse the existing `BatteryReader` trait from `integrity_sweeper_service.rs:408-580` to gate `run_cycle`.

#### L6 — No audit log / structured event when the scheduler drops operations at the 100k cap or planner drops at `max_operations_per_tick`
- Files: `crates/pcloud-engine/src/scheduler.rs:117-120`, `planner.rs:85`
- Severity: **LOW**
- Fix: Emit a `log::warn!` + audit event (`scheduler.truncated`, `planner.tick_capped`) — operators need to know without tailing engine traces.

---

## Area-by-Area Summary

### 1. Queue model (`scheduler.rs`)
- Priority ordering by `PlannedOperation::priority` then path — correct.
- Round-robin fairness — implemented (next_batch builds per-sync-id groups, round-robins), but O(N²) — **H2**.
- `next_batch` DOES drain (`self.queued_operations.remove(i)`) — correct despite the stale docstring on line 12 that says "peek".
- Starvation possible: **yes** between upload vs download classes within a single batch — **L4**.
- 100k cap with warning: **yes** but misleading and spammy — **H1**.

### 2. Conflict resolution (`conflict_resolver.rs`)
- `rename_both` produces concrete `RenameBoth` paths (not `ManualReview`) — correct at `conflict_resolver.rs:193-212`. **M6** about collision risk.
- `newest_wins` mtime comparison at line 184-190 — **H5** (one-sided missing mtime drops data).
- All conflict kinds handled: `LocalModifyVsRemoteModify`, `LocalDeleteVsRemoteModify`, `RemoteDeleteVsLocalModify`, `TypeMismatch` — present; some policy/kind combos fall through to `ManualReview` correctly (prefer_local at line 135-139, prefer_remote at line 169-173).

### 3. State persistence (`pcloud-store`)
- SQLite schema v1..v11 present (`schema.rs`). WAL journal_mode + synchronous=NORMAL + FK on + temp_store=MEMORY.
- Migrations forward-only — refuse backward (`migrations.rs:58-66`). Good.
- SAVEPOINT atomicity: **no** — **C3** critical.
- Transaction safety: `BEGIN IMMEDIATE` pattern for `persist_profile`; migrations skip this — **C3**.
- Crash-consistency: `PRAGMA user_version` bump inside same `execute_batch` gives partial protection (claim in `migrations.rs:74-79`) but DDL + PRAGMA are two SQLite auto-commits under default settings.

### 4. FsEvent coalescer (`fs_events.rs`)
- HashMap-based O(1) dedup — **H3** WRONG: uses `Vec::find`, O(N²).
- Max event cap with warning: **no** — **H3**.
- Overflow behaviour: unbounded Vec growth — **H3**.

### 5. Watcher integration (`pcloud-fs/src/fs_watcher.rs`)
- `notify::RecommendedWatcher` — correct cross-platform choice.
- Debouncing: custom `debounce_loop` at fs_watcher.rs:152-198 with `HashMap<String, (kind, entry_kind, Instant)>` — O(1) per event, good.
- Dropped-event handling: if `notify_tx.send(event)` fails the event is silently dropped (fs_watcher.rs:124 `let _ = notify_tx.send(event);`). No overflow log.
- Cross-platform semantics: classify_event_kind at fs_watcher.rs:233-240 treats `EventKind::Access` and others as no-op — correct. Windows' `EventKind::Modify(MetadataKind)` would surface as `Write` — may generate false positives for chmod-only changes.

### 6. Idempotency
- `ChunkedUploadTracker` persists `upload_id + acked_offset` via `upload_resume_state` — good for upload resume.
- Mid-upload crash: the row persists; the next daemon boot re-reads `upload_resume` and SHOULD resume; this path lives in `pcloud-backends/src/upload_state.rs` (not in scope per the audit prompt but referenced).
- Downloads have NO comparable resume table — partial download restart reads from scratch. Moderate gap.

### 7. Back-pressure — **M8** (no enforcement).

### 8. Rate limiting & retry (`pcloud-resilience`)
- Exponential backoff with jitter: `retry.rs:26-47 BackoffSchedule::ExponentialJittered` — deterministic seed, "equal jitter" — good.
- Retry budget: `GlobalRetryBudget` present — but **M9** not wired.
- Circuit breaker: three-state, `parking_lot::Mutex` (panic-safe), `ProbeGuard` RAII — excellent (`circuit_breaker.rs:1-329`).
- `RetryDecision::Exhausted` referenced in `transport.rs` but variant does not exist — **C2**.

### 9. Stall detection — **C1** orphaned module.

### 10. Diff poller (`diff_poller.rs`)
- Cursor persistence: not in the engine; store-level — **M3**.
- Error recovery on malformed entries: **no** — **M4**.

### 11. Planner (`planner.rs`)
- Delete policy: `DeletePolicy::for_sync_type` at `planner.rs:180-210` handles `Full, UploadOnly, DownloadOnly, BackupArchive` — all four variants — correct with good unit coverage (`delete_policy_backup_archive_suppresses_all_deletes_but_keeps_uploads` test). BackupArchive correctly suppresses all deletes.
- `max_operations_per_tick` silent drop — **H7**.

### 12. Session manager — **L1** stub.

### 13. Selective sync (`selective.rs`)
- `.pcloudsync` parsing supports `#` comments, blank lines, `!` excludes, leading-`!` strip — good.
- Security: path validation **does NOT** reject `..` or absolute paths — **M7**.
- Exclude-wins precedence — correct per `selective.rs:230-239`.

### 14. RecoveryManager (`recovery.rs`)
- Classes covered: RetryableNetworkError, ChecksumMismatch, InvalidPath, PermissionDenied — **M5** several missing.

### 15. Transfer coordinators (`transfers/`)
- `accept_batch`, `mark_completed`, `mark_failed`, `evict_sync_id` all present in both uploads.rs and downloads.rs — confirmed at uploads.rs:48, 104, 128, 73 and downloads.rs:48, 104, 128, 73.
- **H6** accept_batch clears previous in-flight.

### 16. Power/battery awareness — **L5** integrity sweeper only.

---

## Severity Rollup

| Severity | Count | Items |
| --- | --- | --- |
| CRITICAL | 3 | C1 stall_detector orphaned; C2 transport.rs orphaned + uses non-existent RetryDecision::Exhausted; C3 no SAVEPOINT in migrations |
| HIGH | 7 | H1 scheduler cap log; H2 scheduler O(N²); H3 fs_events O(N²) + no cap; H4 coalescer semantics; H5 newest_wins one-sided mtime; H6 accept_batch clears; H7 planner silent drop |
| MEDIUM | 9 | M1–M9 |
| LOW | 6 | L1–L6 |

## Top 5 fixes by impact
1. Fix **C1 StallDetector** — wire it into the engine and sync loop; a silent daemon hang is the worst operator experience.
2. Delete or enable **C2 transport.rs** — orphaned broken code is a compilation timebomb and contradicts the claimed retry architecture.
3. Wrap migrations in transactions (**C3**) — crash-consistency is the store's whole job.
4. Fix **H3 FsEventIngestor** — O(N²) and unbounded; easy to cause a daemon DoS with a large file burst.
5. Wire **GlobalRetryBudget** (**M9**) and retry-after into a single transport layer rather than ad hoc per-caller retries.
