# Section 4: Sync Engine & Runtime
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 4)

## Scope

Audited `crates/pcloud-engine/` (scheduler, planner, conflict resolver,
diff poller, fs_events, local_scan, recovery, transfers, reconcile_worker,
selective, session_manager, stall_detector), `crates/pcloud-store/`
(schema, migrations, tx, lib), `crates/pcloud-resilience/` (retry,
circuit_breaker, rate_limit, pacing, global_budget, timeout, metered),
`crates/pcloud-daemon/src/sync_loop.rs`,
`crates/pcloud-daemon/src/sync_loop_runtime.rs`, and
`crates/pcloud-daemon/src/integrity_sweeper_service.rs`, plus
`crates/pcloud-fs/src/fs_watcher.rs`.

## Findings

### CRITICAL [6]

#### C1 — Scheduler has no per-root fairness, no starvation guard, and no in-flight slot accounting
**Severity:** CRITICAL
**File:** `crates/pcloud-engine/src/scheduler.rs:62-128`, `crates/pcloud-engine/src/lib.rs:409-414`

The scheduler is a single flat `Vec<PlannedOperation>` sorted by `(priority, path)` only (`scheduler.rs:80-87`). `next_batch()` returns the first `max_parallel_uploads + max_parallel_downloads` entries; there is no per-sync-root fairness, no aging / anti-starvation. A single very active sync root whose paths sort lexicographically early will starve every other root indefinitely.

Worse, `EngineShell::advance_transfer_cycle` (`lib.rs:409-414`) calls `self.scheduler.next_batch().to_vec()` then hands the full batch to **both** `uploads.accept_batch` and `downloads.accept_batch`, and `UploadCoordinator::accept_batch` / `DownloadCoordinator::accept_batch` **clear active lists on every call** (`transfers/uploads.rs:48-51`, `transfers/downloads.rs:48-51`). This means:

1. There is no real in-flight-slot accounting — the "coordinator" is a one-shot partition of the current top-of-queue, not a bounded concurrent worker pool.
2. Between two `advance_transfer_cycle` calls, `active_uploads` gets cleared, so a transfer that is genuinely in-flight inside `execute_uploads`/`execute_downloads` in `sync_loop_runtime.rs:412-508` becomes unreachable the moment another cycle happens.
3. The `max_parallel_uploads` / `max_parallel_downloads` fields on `Scheduler` are never honored as true concurrency limits; they are merely a cap on the peek batch width.

**Remediation:** Replace `Scheduler` with a true priority queue that supports (a) per-root deficit-round-robin or weighted fair queueing, (b) in-flight counts tracked separately from queued work, (c) a non-destructive `admit_next(n)` API that only hands out work up to available capacity. Coordinators must not clear their active lists on batch ingestion; they should append new work up to `max_parallel_*`, track real in-flight tasks with explicit complete/fail callbacks, and block further admission while saturated.

#### C2 — Planner silently drops excess candidates with no dead-letter / re-queue path
**Severity:** CRITICAL
**File:** `crates/pcloud-engine/src/planner.rs:85-104`

`Planner::plan` caps output at `max_operations_per_tick` (default 1024, `planner.rs:55-58`). The loop terminates the moment the cap is hit (`planner.rs:85`), and the over-cap candidates are **silently dropped** — they are not persisted to disk, not re-queued, not logged, not audited. After one planning tick, the engine has no memory that they existed.

Combined with `scheduler.replace_queue()` in `ingest_candidates` (`lib.rs:203-207`), every planning tick wipes the queue and rebuilds it from the current candidate slice. If a local-scan pass produces 10 000 entries, only the first 1024 (by sorted path) are ever executed; the other ~9000 are invisible until the next full scan. For a 300s scan interval this means up to 5 minutes of silent data loss for large trees.

The doc comment claims "Excess candidates are deferred — a later tick will process them once the scheduler drains" (`planner.rs:46-48`); the code does not implement that deferral.

**Remediation:** Either (a) remove the artificial cap and let SQLite-backed queue-depth limits do the work, (b) persist over-cap candidates into a durable `pending_plan` table and dequeue them on the next tick, or (c) explicitly surface `PlanIncomplete { dropped: n }` in the return type so callers are forced to decide. The current "silently drop and re-derive from next scan" behaviour is a data-integrity bug, not a safety throttle.

#### C3 — `FsEventIngestor::coalesce_window_ms` is declared but not used; coalescing uses raw order only
**Severity:** CRITICAL
**File:** `crates/pcloud-engine/src/fs_events.rs:14-26, 64-95`

`FsEventIngestor` has a `coalesce_window_ms` field (default 250), but `normalize_events` (`fs_events.rs:64-95`) never consults time. It coalesces by collapsing duplicate-path events inside a single input slice — not within a time window — and the "last seen" wins unconditionally. If the caller hands in two batches separated by 10 minutes, no coalescing happens across batches.

The upstream `FsWatcher::debounce_loop` (`fs_watcher.rs:152-198`) already does path-keyed debouncing, but it lives in a separate crate and uses a separate `HashMap`. So the engine-level ingestor is effectively dead code masquerading as a time-window coalescer. The `coalesce_window_ms` claim in the docs is misleading; tests rely on the field but never assert timing semantics.

**Remediation:** Either (a) actually implement a time-window coalescer that absorbs events older than `coalesce_window_ms` from its own state store, or (b) remove the `coalesce_window_ms` field, rename the method to `dedupe_events`, and document that this is pure batch-local de-dup, not temporal debouncing. The current public name is a lie.

#### C4 — Watcher loses events silently when consumer is slow or channel fills
**Severity:** CRITICAL
**File:** `crates/pcloud-fs/src/fs_watcher.rs:111-147, 152-229`

The `FsWatcher` uses `std::sync::mpsc::channel` (unbounded) for both `notify_tx` (`fs_watcher.rs:111, 119`) and the outbound `tx` (`fs_watcher.rs:111`). Under load this is a memory pressure risk; under memory pressure, Linux's OOM killer reaps the daemon.

More importantly, the inner notify callback (`fs_watcher.rs:121-127`) does `if let Ok(event) = result { let _ = notify_tx.send(event); }` — errors from `notify` (including `notify::Error::MaxFilesWatch`, path-not-found, rescan needed) are **silently swallowed**. On Linux the inotify kernel queue can overflow (`IN_Q_OVERFLOW`); `notify` surfaces this as an `Error::MaxFilesWatch` or a rescan hint. The current code drops both, meaning a burst that blows out `/proc/sys/fs/inotify/max_queued_events` will produce a permanently desynced sync root and **no operator-visible signal**.

`debounce_loop`'s `flush_pending` call ordering (`fs_watcher.rs:184-197`) also has a latent bug: after a `Timeout`, `flush_pending` is called; then the outer loop falls through and calls it again on the next line (`fs_watcher.rs:196`). Harmless but wasteful.

The `RecvTimeoutError::Disconnected` branch (`fs_watcher.rs:188-192`) flushes pending and exits, but by that point the `FsWatcher` handle has been dropped and the sync loop will keep polling `rx` forever with no way to tell the watcher died. There is no health check on the watcher thread.

**Remediation:**
- Replace unbounded mpsc with a bounded channel plus explicit overflow handling; on overflow, log an `overflow` event, increment a counter, and force a full re-scan of the affected root (`IncrementalScanTracker::record_full_scan` should be invalidated).
- Never silently drop `notify::Error`; route it to a health counter and raise an audit event on `MaxFilesWatch` / rescan conditions.
- Expose watcher liveness through `FsWatcher::is_healthy()` so the sync loop can detect a dead watcher and restart it.

#### C5 — Audit events lose payload on persistence failure in active control path
**Severity:** CRITICAL
**File:** `crates/pcloud-daemon/src/sync_loop_runtime.rs:522-546`

`emit_cycle_audit` (`sync_loop_runtime.rs:522-546`) calls `self.audit.append_event(&self.store_conn, "sync.loop.cycle", Some(&details))` and on error falls back to `log::error!`. CLAUDE.md's security rules explicitly forbid silently swallowing audit failures ("do not silently swallow persistence or audit failures on active control paths"). Emitting to `log::error!` is not audit-equivalent: stderr is not the tamper-evident chain, and a log-drop or rotate loses the event without detection.

Moreover, the whole branch is gated on `result.total_errors > 0 || result.total_uploads > 0 || result.total_downloads > 0` — an idle cycle never emits. Security-relevant cycles (auth missing, quota exhausted, root permission denied) produce zero uploads/downloads and zero errors (because errors are only counted when transfer execution is reached) so are invisible to audit.

**Remediation:**
- On `append_event` failure, set a `pending_audit` flag and stop processing further sync work until the failure resolves (fail-closed semantics). At a minimum increment an atomic `audit_drop_count` and expose it through metrics so operators can alert on non-zero.
- Emit a structured cycle-started / cycle-ended audit pair regardless of counters, so absence of records proves daemon inactivity rather than event loss.

#### C6 — Upload-resume state has no periodic reconciliation; orphan upload sessions accumulate forever
**Severity:** CRITICAL
**File:** `crates/pcloud-store/src/repositories/upload_resume.rs:59-80`, `crates/pcloud-store/src/schema.rs:195-235`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:412-508`

Schema v9 adds `upload_resume_state` to persist chunked-upload resume metadata. The table is written by the upload state machine but `sync_loop_runtime::execute_uploads` (`sync_loop_runtime.rs:412-508`) does not consult it: every upload attempt calls `upload_create` fresh, never `put`-ing or looking up resume state. Files partially uploaded then the daemon crashes leak a remote `upload_id` session on the server with no local reference, and there is no GC sweeper that (a) reads `upload_resume_state` rows older than `N` days, (b) checks the server state, and (c) either resumes or aborts them.

Long-running daemons will therefore gradually exhaust the pCloud account's upload-session quota (API documents a hard ceiling on concurrent sessions per user) while using zero local disk. There is no upload GC in the integrity sweeper either (`integrity_sweeper_service.rs` is checksum-only).

**Remediation:** Add an upload-sweeper service that, on startup and every `N` hours, walks `upload_resume_state`, matches each row against in-flight local uploads, and either resumes (if the local file still exists at the recorded size) or calls the server `upload_save` / cancel path and deletes the row. Wire this into bootstrap before accepting IPC requests.

### HIGH [8]

#### H1 — `Planner::plan` is O(n²) on path collisions and O(n log n) on sort with no hash-set dedup
**Severity:** HIGH
**File:** `crates/pcloud-engine/src/planner.rs:75-104`

The planner sorts the full candidate slice then linear-scans for adjacent `(path, source)` pairs. For very large scan batches (typical enterprise: 500k-1M files on initial onboard) the sort-then-group pattern materializes a `Vec<SyncCandidate>` copy on each tick. With the 1024 cap (see C2) this is masked today, but a fix to C2 that permits larger batches will hit a quadratic hotspot inside `plan_pair`'s `while idx < sorted.len() && sorted[idx].path == path` loop (`planner.rs:90-96`) for large path-collision sets.

**Remediation:** Build a `HashMap<String, (Option<SyncCandidate>, Option<SyncCandidate>)>` in one pass; emit ops from the map. O(n) with clear semantics.

#### H2 — Conflict resolver `resolve_prefer_remote` drops `remote_file_id` to `None`
**Severity:** HIGH
**File:** `crates/pcloud-engine/src/conflict_resolver.rs:136-168`

When the policy is `PreferRemote` on `LocalModifyVsRemoteModify`, the resolver emits:
```
PlannedOperation::DownloadFile { sync_id, path, remote_file_id: None }
```
(`conflict_resolver.rs:143-147`). The caller then has no remote file id to resolve against the API; `sync_loop_runtime::execute_downloads` requires `remote_file_id: Some(file_id)` (`sync_loop_runtime.rs:358-363`) and silently skips tasks that have `None`. The conflict is marked "resolved" in the scheduler but nothing actually downloads. Worst case: user picks prefer-remote, conflict clears, local copy is unchanged, no error.

**Remediation:** `ConflictPolicy::PreferRemote` must carry the observed `remote_file_id` from the planner's original `SyncCandidate`. Plumb the conflict context through so resolution has the ids needed to act.

#### H3 — `newest_wins` policy has no timestamp comparison; it is a silent `prefer_remote`
**Severity:** HIGH
**File:** `crates/pcloud-engine/src/conflict_resolver.rs:170-179`

`resolve_newest_wins` is explicitly documented to compare mtimes, but the implementation is a 3-line comment + `resolve_prefer_remote(sync_id, path, kind)` (`conflict_resolver.rs:174-179`). There is no `mtime` on `SyncCandidate`/`PlannedOperation::Conflict`, so the input data needed for "newest wins" is not even collected. Users that select this policy will silently get server-wins without any warning; enterprise auditors would flag this as a lie in the config docs.

**Remediation:** Either implement real mtime comparison (requires adding `local_mtime` / `remote_mtime` to the conflict planned-op and sourcing them from scanner + diff poller), or remove `NewestWins` from the enum and document it as unsupported.

#### H4 — `rename_both` policy never actually renames
**Severity:** HIGH
**File:** `crates/pcloud-engine/src/conflict_resolver.rs:181-191`, `crates/pcloud-engine/src/lib.rs:373-393`

`ConflictPolicy::RenameBoth` claims to keep both copies by renaming to `file.conflict-local.ext` / `file.conflict-remote.ext`. `resolve_rename_both` (`conflict_resolver.rs:181-191`) returns `ConflictResolution::ManualReview { reason: "rename-both: both copies preserved for manual merge" }`. There is no rename, no duplicate, no side effect. `resolve_conflict_by_path` (`lib.rs:358-393`) uses the same resolver and also does nothing to the filesystem. The doc comment is aspirational.

This is the **default** policy (`conflict_resolver.rs:52-58`: `default_policy: ConflictPolicy::RenameBoth`), so every conflict the engine sees is effectively left as `ManualReview` with a misleading string. Users will see `reason: "rename-both: ..."` in IPC responses but no renamed files on disk.

**Remediation:** Implement the rename on both sides — emit a `PlannedOperation::RenameLocal { from, to }` and `PlannedOperation::RenameRemote { from, to }`, then download/upload the other copy. Or change the default to `ManualReview` and keep the docs honest.

#### H5 — Case-insensitive filesystem collisions are not detected anywhere
**Severity:** HIGH
**Files:** `crates/pcloud-engine/src/conflict_resolver.rs`, `crates/pcloud-engine/src/planner.rs`, `crates/pcloud-engine/src/local_scan.rs`

pCloud's remote namespace is case-sensitive; macOS HFS+/APFS default and Windows NTFS default are case-insensitive. A search for `case_insensitive|case_fold|NFC|NFD` across the engine and planner returns zero hits. A remote that contains both `Report.txt` and `report.txt` downloaded onto a default APFS or NTFS mount will produce (a) one file silently overwriting the other with no conflict surfaced, (b) infinite modify-loop as the filesystem reports a different canonical path than the server. There is also no Unicode normalization (NFC/NFD); macOS decomposes filenames while pCloud stores composed, producing perpetual false-positive conflicts on accented names.

**Remediation:** Add a platform-aware `PathNormalizer` consulted by both local-scan and diff-normalize. On case-insensitive filesystems, detect collisions at scan time and surface `ConflictKind::CaseInsensitiveCollision`. Always normalize to NFC before comparison; store both the original and the normalized form.

#### H6 — SQLite `BEGIN IMMEDIATE` rollback failures are discarded with no audit trail
**Severity:** HIGH
**File:** `crates/pcloud-store/src/tx.rs:78-89`

`TransactionBoundary::immediate` on error does:
```rust
let _ = conn.execute_batch("ROLLBACK");
Err(err)
```
(`tx.rs:84-86`). A rollback failure silently discards both the original error context and the rollback error. If rollback fails because the connection is in a bad state (disk full, SQLite busy, WAL checkpoint stuck), the next `BEGIN IMMEDIATE` will fail with "cannot start a transaction within a transaction" and the daemon enters a loop of failed writes with no root-cause diagnostic.

The doc comment claims rollback failures are "deliberately discarded so the caller always sees the root cause" — but a rollback failure *is* a root cause signal that something fundamental is broken, and silently dropping it makes post-mortem debugging harder.

**Remediation:** Log rollback failures at `error!` with enough context to trace the original op; if rollback fails, force the connection to be closed and recreated on the next mutation.

#### H7 — No memory/disk back-pressure anywhere; cache is unbounded
**Severity:** HIGH
**File:** `crates/pcloud-daemon/src/sync_loop_runtime.rs:354-410`

`execute_downloads` calls `self.transfer_runtime.download_bytes(&link)` which returns a full `Vec<u8>` (all bytes in RAM), then `self.cache.cache_page(cache_key, bytes.clone())` + `self.cache.stage_file(path.clone(), bytes.clone())` + `self.filesystem.seed_staged_file(path.clone(), bytes)` — **three copies** of every downloaded file in memory. There is no streaming, no chunked write to disk. A 10GB download OOMs the daemon; a 1GB download triples to 3GB RAM.

There is no global memory budget (`pcloud-resilience::global_budget::GlobalRetryBudget` is retry-only, not memory) and no disk budget enforcement. The `BandwidthPacer` (`pcloud-resilience::pacing`) exists but is not wired into `sync_loop_runtime::execute_downloads` / `execute_uploads`.

**Remediation:** Stream downloads to disk chunk-by-chunk (writeback staging), bound concurrent downloads by total-bytes-in-flight (not just task count), wire `BandwidthPacer::pace()` into the download inner loop, and add a `DiskBudget` guard that refuses new downloads when the staging directory exceeds `N` GB.

#### H8 — Retry/circuit-breaker primitives are not wired into the live upload/download path
**Severity:** HIGH
**Files:** `crates/pcloud-daemon/src/sync_loop_runtime.rs:354-508`, `crates/pcloud-resilience/src/retry.rs`, `crates/pcloud-resilience/src/circuit_breaker.rs`

`pcloud-resilience` provides `RetryPolicy`, `MethodRetryPolicy`, `CircuitBreaker`, `TokenBucket`, `BandwidthPacer`, `GlobalRetryBudget`. Grep for these types inside `sync_loop_runtime::execute_uploads` / `execute_downloads` returns **zero hits**. On failure the code calls `self.engine.classify_failure(&task.operation, RecoveryFailure::RetryableNetworkError)`, prints the disposition in the failure message, and immediately marks the transfer failed. There is no backoff, no retry, no circuit-breaker gating subsequent attempts, no token-bucket rate limit. The `RecoveryDecision::RetryLater` disposition has no consumer on this path; the transfer simply dies.

**Remediation:** Replace the direct `transfer_runtime.*` calls with a wrapper that consults `CircuitBreaker::try_acquire`, applies `TokenBucket::acquire` for API rate limiting, applies `BandwidthPacer::pace` on the byte stream, and on retryable failures runs `RetryPolicy::next(attempt)` with `GlobalRetryBudget::try_consume` throttle across all concurrent ops.

### MEDIUM [9]

#### M1 — `IncrementalScanTracker` stores `Instant` in-memory only; restart loses the timer
**Severity:** MEDIUM
**File:** `crates/pcloud-engine/src/local_scan.rs:166-254`

`last_full_scan: HashMap<SyncId, Instant>` (`local_scan.rs:170`) is in-memory only. On daemon restart every sync root gets a fresh `FullScan` on first tick — which is reasonable — but there is no throttle against a crash-loop that walks a million-file tree on every restart. A pathological crash loop traverses the tree every N seconds.

**Remediation:** Persist `last_full_scan` as a unix timestamp in the store and consult it on bootstrap; enforce a minimum interval from the persisted timestamp.

#### M2 — `DiffPoller::batch_limit` default of 512 is low; no dynamic tuning
**Severity:** MEDIUM
**File:** `crates/pcloud-engine/src/diff_poller.rs:20-24`

Fixed at 512 entries/call with no adaptive sizing. On a large bulk import (10 000 remote changes) the daemon makes ~20 sequential API round-trips before local state is current. No batching signal exists to request bigger pulls on high-latency links.

**Remediation:** Increase default to 2048 or make it adaptive (grow on `has_more = true`, shrink on elapsed > SLO).

#### M3 — Audit chain rebuild on schema v8 migration is not transactional
**Severity:** MEDIUM
**File:** `crates/pcloud-store/src/schema.rs:168-193`

`apply_schema_v8` runs three `ALTER TABLE` statements and then `rebuild_hash_chain(conn)` followed by `PRAGMA user_version = 8`. These are **not** wrapped in a single transaction; if a crash occurs between `rebuild_hash_chain` completing and the `PRAGMA` bump, the next launch will re-run the ALTERs (idempotent) and re-run `rebuild_hash_chain` (also idempotent), but in-between the database has a hash-chain written for v7 format at v7 `user_version`. This is self-healing on the next run, but it leaves a window where another process opening the DB sees inconsistent state.

**Remediation:** Wrap each apply_schema_vN in an explicit `BEGIN IMMEDIATE` / `COMMIT` via `TransactionBoundary::immediate`. Currently `apply_plan` (`migrations.rs:80-118`) calls each step bare.

#### M4 — `ChunkedUploadTracker::advance` does not saturate against `total_size`
**Severity:** MEDIUM
**File:** `crates/pcloud-engine/src/transfers/uploads.rs:217-241`

`ChunkedUploadTracker::advance` does `self.acked_offset += bytes_written;` — if the caller passes a wrong `bytes_written` (e.g. retransmit double-count), `acked_offset` exceeds `total_size` silently. `is_complete` still returns `true` but `remaining()` uses `saturating_sub` so callers downstream see zero, hiding the bug.

**Remediation:** `advance` should return `Result<u64, OverrunError>` and clamp or reject values that would exceed `total_size`.

#### M5 — `ReconcileWorker::tick` uses `Instant` without deadline-skew resistance
**Severity:** MEDIUM
**File:** `crates/pcloud-engine/src/reconcile_worker.rs:178-202`

`clock.now()` returns a platform `Instant`; `duration_since(last)` panics if `last > now` (monotonicity violation on platforms that freeze time, e.g. VM suspend). The surrounding daemon code does not catch the panic.

**Remediation:** Use `saturating_duration_since` throughout the engine (the circuit breaker already does; `circuit_breaker.rs:283`).

#### M6 — Sync loop single point of failure: no supervisor, no restart on panic
**Severity:** MEDIUM
**File:** `crates/pcloud-daemon/src/sync_loop.rs:479-506`

`spawn_sync_loop` uses `std::thread::Builder::new().spawn(...)`. On panic the thread dies and nothing restarts it. `SyncLoopHandle::is_alive()` returns `false` but the IPC dispatch thread has no watcher that converts this into a restart or an audit event. A single bug in any per-root processing silently halts all sync for all roots.

**Remediation:** Wrap the sync loop body in `catch_unwind`; on panic, emit a `sync.loop.panic` audit event, cool down, and respawn. Consider a supervisor thread that `join`s the worker and restarts it with exponential backoff.

#### M7 — Diff cursor advance is persisted, but metadata upsert errors are swallowed
**Severity:** MEDIUM
**File:** `crates/pcloud-daemon/src/sync_loop_runtime.rs:262-302`

The `let _ = FileMetadataRepository::upsert(...)` and `let _ = FileMetadataRepository::delete(...)` calls (`sync_loop_runtime.rs:271, 299`) discard errors. If metadata writes fail (disk full, schema drift), the diff cursor advances anyway and the local metadata cache silently diverges from the server. Subsequent `stat_path` calls return stale or wrong data with no indication.

**Remediation:** On metadata persistence failure, either (a) do not advance the cursor, or (b) emit an audit event and bump a divergence counter so the integrity sweeper notices.

#### M8 — Watcher debounce thread name is generic; multiple roots share the same thread name
**Severity:** MEDIUM
**File:** `crates/pcloud-fs/src/fs_watcher.rs:139-143`

Every `FsWatcher::start` call spawns a thread named `"fs-watcher-debounce"` — multiple sync roots produce multiple threads all named the same, making `ps`/`top` forensics impossible.

**Remediation:** Name the thread `fs-watcher-debounce-<sync_id>`.

#### M9 — Integrity sweeper has no quarantine policy; mismatches are audited but nothing moves the file
**Severity:** MEDIUM
**File:** `crates/pcloud-daemon/src/integrity_sweeper_service.rs:827-845, 1169-1281`

On a detected `Mismatch`, the sweeper emits an audit event with hashed path but **does not quarantine** the divergent local file, does not stop the sync loop from re-uploading a corrupt copy, does not mark the file as "do-not-sync", does nothing to prevent the bad data from overwriting the server. Divergence detection without response is a half-measure — the operator learns, but automated protection is absent.

**Remediation:** On `Mismatch`, move the local file to a `.pcloud-quarantine/` sidecar directory (preserving filename), evict the file from the sync scheduler for that root until operator confirms, emit a structured `mismatch.quarantined` event.

### LOW [6]

#### L1 — `Scheduler::next_batch` panics risk on zero parallelism
**File:** `crates/pcloud-engine/src/scheduler.rs:122-127`

`limit = limit.max(1).min(self.queued_operations.len())` is safe but the `max_parallel_uploads + max_parallel_downloads` sum can overflow if both are set to `usize::MAX`. Realistic inputs won't hit this, but validation at construction would be cleaner.

#### L2 — `validate_relative_path` duplication
**Files:** `crates/pcloud-engine/src/fs_events.rs:98-111`, `crates/pcloud-engine/src/diff_poller.rs:101-114`, `crates/pcloud-engine/src/local_scan.rs:273-286`

Three identical copies of `validate_relative_path`. Extract to a shared helper to prevent drift.

#### L3 — `planner::basename` duplicates logic found in 4 other places
**File:** `crates/pcloud-engine/src/planner.rs:356-358`

Shared `fn basename(path: &str) -> &str { path.rsplit('/').next().unwrap_or(path) }` also appears in `conflict_resolver.rs:119` (inline), watcher code, and elsewhere. Extract once.

#### L4 — `sync_loop_runtime::shared_auth_token` returns `Mutex<Option<SecretString>>`
**File:** `crates/pcloud-daemon/src/sync_loop_runtime.rs:61-68`

Locking a `std::sync::Mutex` on every auth read (once per cycle per root) is fine at low rates but `parking_lot::RwLock` would let the sync loop avoid the writer-starvation risk on active token refresh. Low frequency today, worth noting.

#### L5 — `CycleResult` fields are `pub` with no invariants
**File:** `crates/pcloud-daemon/src/sync_loop.rs:189-204`

Direct field mutation from outside the module (e.g. test harness) can produce inconsistent aggregates. Prefer accessors + private fields.

#### L6 — `diff_events.rs` documents 26 C event kinds but the `DiffEventKind` enum includes all of them as stubs, and many handlers are wire-only with no side-effect tests
**File:** `crates/pcloud-engine/src/diff_events.rs`

The file is 488 lines of well-documented classifiers. Coverage is thin on the share/crypto branch — unit tests exist for dispatch plumbing but not for idempotency of a repeated event (e.g. receiving the same `deletefile` twice). Minor documentation-vs-tests gap.

## Summary

| Severity | Count |
|----------|-------|
| CRITICAL | 6     |
| HIGH     | 8     |
| MEDIUM   | 9     |
| LOW      | 6     |
| **Total**| **29**|

The engine and store scaffolding is sound in isolation (good doc comments, explicit transaction boundaries, hash-chained audit log, deterministic clock injection, panic-safe circuit breaker). The critical gaps are at the **integration layer**: the scheduler/planner/transfer coordinators together do not implement a production-grade concurrent worker pool, the conflict resolver's rename-both default does not rename, retry/circuit-breaker/pacing crates are not wired into the live transfer path, there is no back-pressure, audit failures drop to stderr, and case-insensitive / Unicode-normalized filesystems are not handled at all. These are the findings that must block "production ready" claims per `bd-1du.10`.
