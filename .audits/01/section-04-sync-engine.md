## Section 4. Sync Engine & Runtime

**Scope:** Queue model, state persistence, conflict resolution, watcher, idempotency, back-pressure, retry/resilience, integrity sweeper, power awareness, pause/resume, stall detection, resource leaks, engine test coverage.

**Out of scope (delegated):** FUSE/mount internals (Dim. 5), parity matrix (Dim. 1), HTTP transport (Dim. 6).

**Primary files audited:**

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/lib.rs` (911 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/scheduler.rs` (219 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/planner.rs` (656 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/conflict_resolver.rs` (341 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/recovery.rs` (189 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/fs_events.rs` (184 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/local_scan.rs` (533 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/diff_poller.rs` (216 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/reconcile_worker.rs` (283 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/transfers/uploads.rs` (361 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/transfers/downloads.rs` (267 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/session_manager.rs` (22 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/sync_loop.rs` (819 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/sync_loop_runtime.rs` (955 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/integrity_sweeper_service.rs` (1947 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/schema.rs` (331 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/migrations.rs` (118 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/tx.rs` (90 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/integrity.rs` (42 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/repositories/upload_resume.rs` (316 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/repositories/diff_state.rs` (117 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cache/src/staging.rs` (130 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cache/src/page_cache.rs` (505 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-resilience/src/retry.rs` (489 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-resilience/src/circuit_breaker.rs` (534 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-resilience/src/rate_limit.rs` (306 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-resilience/src/pacing.rs` (254 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/fs_watcher.rs` (662 lines)

---

### Architectural overview

`EngineShell` (`crates/pcloud-engine/src/lib.rs:66-103`) is a single-owner aggregate built from:

- `DiffPoller` (remote cursor bookkeeping),
- `LocalScanner` (full-walk cadence),
- `FsEventIngestor` (event coalescing),
- `Planner` (pair/conflict detection),
- `Scheduler` (priority queue),
- `RecoveryManager` (failure classifier),
- `ConflictResolver` (policy applicator),
- `UploadCoordinator` / `DownloadCoordinator`.

`EngineShell` is **not** `Sync` — it is owned and mutated exclusively on the sync loop thread (`crates/pcloud-daemon/src/sync_loop.rs:25-32`, `std::thread` based, **not** tokio). The IPC dispatch thread communicates via `Arc<SyncLoopShared>` (`sync_loop.rs:104-118`) with a `Mutex<SyncLoopStatus>` + `Condvar` wake signal.

The engine itself is a **pure synchronous state machine** — zero I/O happens inside the engine crate. All I/O is driven by `RealSyncLoopRuntime` in `crates/pcloud-daemon/src/sync_loop_runtime.rs`.

That design is clean in theory. The gaps below concern specific correctness, durability, and enterprise-grade expectations.

---

## CRITICAL findings

### C-1. Scheduler has no per-sync-root fairness — single root can starve all other roots

`crates/pcloud-engine/src/scheduler.rs:80-87` (`replace_queue`):

```
operations.sort_by(|left, right| {
    left.priority()
        .cmp(&right.priority())
        .then(left.path().cmp(right.path()))
});
```

`crates/pcloud-engine/src/scheduler.rs:122-127` (`next_batch`):

```
let limit = self.max_parallel_uploads + self.max_parallel_downloads;
let limit = limit.max(1).min(self.queued_operations.len());
&self.queued_operations[..limit]
```

There is **no per-sync-root fairness mechanism whatsoever**. The queue is a flat `Vec<PlannedOperation>` sorted only by `(priority, path)`. One sync root with 100k queued uploads will monopolise every scheduler batch until it drains, **completely starving the others**.

There is also no round-robin shuffle at batch-emit time. This is a fairness defect the C client does not exhibit because the C daemon runs per-sync-root workers.

**Severity:** CRITICAL — customer with a big backup sync root and a small "inbox" sync root will see the inbox become unresponsive indefinitely.

**Remediation:** Group queued operations by `sync_id` and interleave batches round-robin across sync roots; keep priority ordering only *within* a root. Add a `fairness_policy: FairnessPolicy::RoundRobinPerSyncRoot` knob and a proptest that asserts "N roots, any ops, every root emits at least ceil(batch_size/N) work per N batches".

---

### C-2. `Scheduler::next_batch` is a pure peek — the queue never drains

`crates/pcloud-engine/src/scheduler.rs:122-127` and module-level docs at `:10-14` ("Batch semantics — **peek**").

`next_batch` returns `&self.queued_operations[..limit]` **without removing** the items. `EngineShell::advance_transfer_cycle` (`lib.rs:409-414`) hands the batch to the upload/download coordinators and then calls `next_batch` again — but still does not pop.

Because the scheduler never dequeues:

1. On the next `replace_queue` call (`lib.rs:200-207`, `ingest_candidates`), every still-in-flight operation is silently overwritten. The *coordinators* retain their in-flight copies, but:
2. `mark_transfer_completed` / `mark_transfer_failed` (`lib.rs:419-428`) only mutate coordinator lists — **they do not remove the completed operation from `scheduler.queued_operations`**. So a completed upload continues to appear in `queued_operations.len()`, `summary()`, and worse, will be re-emitted by the next `next_batch()` peek until `replace_queue` is called again.
3. The queue has no notion of "in-flight vs waiting". A concurrent scheduler + coordinator interaction will repeatedly hand the *same* operation back to `accept_batch` on every cycle if `ingest_candidates` is not called first.

**Severity:** CRITICAL — duplicate uploads on every cycle, pending-count drift, conflict count never settles.

**Remediation:** Split `queued_operations` into `waiting: VecDeque` + `in_flight: HashMap<path, op>`. `next_batch` must pop (not peek). Completion / failure must remove the operation from `in_flight`. Document the state machine: queued → in_flight → (completed | failed | retry_later).

---

### C-3. `newest_wins` conflict policy does not compare timestamps

`crates/pcloud-engine/src/conflict_resolver.rs:170-179`:

```
fn resolve_newest_wins(
    sync_id: pcloud_model::ids::SyncId,
    path: &str,
    kind: &ConflictKind,
) -> ConflictResolution {
    // Without real timestamp comparison, fall back to prefer-remote
    // (server-wins tie-break, matching the C client's newest-wins
    // default when timestamps are equal).
    resolve_prefer_remote(sync_id, path, kind)
}
```

The function is a lie dressed up as an implementation. The policy name `NewestWins` promises mtime-based arbitration; the code unconditionally picks remote. Enterprise users who explicitly configure `newest_wins` (because they want "the user's most recent edit should win") will silently lose local edits every time.

The unit test at `:280-298` even accepts this by asserting that `newest_wins` produces a `DownloadFile` regardless of timestamps, which means the test is locking in the buggy behavior.

**Severity:** CRITICAL — advertised feature silently destroys local edits.

**Remediation:** Thread `local_mtime` and `remote_mtime` into the `ConflictKind::LocalModifyVsRemoteModify` payload. If either mtime is unknown, fail the policy (emit `ManualReview`). The current "fall back to prefer-remote" default is **data loss**, not a tie-break.

---

### C-4. `rename_both` conflict policy does not rename — it produces ManualReview

`crates/pcloud-engine/src/conflict_resolver.rs:181-191`:

```
fn resolve_rename_both(
    _sync_id: pcloud_model::ids::SyncId,
    path: &str,
    kind: &ConflictKind,
) -> ConflictResolution {
    ConflictResolution::ManualReview {
        path: path.to_owned(),
        kind: kind.clone(),
        reason: "rename-both: both copies preserved for manual merge".to_owned(),
    }
}
```

Policy docstring at `:31-33` says both sides become `.conflict-local.ext` / `.conflict-remote.ext`. The implementation emits `ManualReview` and does nothing. This is also the **default** policy (`:52-58`, `ConflictResolver::default`), so every collision under default config stalls indefinitely.

**Severity:** CRITICAL — the documented default conflict behavior is not implemented.

**Remediation:** Emit `ConflictResolution::Apply` with two explicit `UploadFile` + `DownloadFile` operations targeting the `.conflict-local.ext` / `.conflict-remote.ext` sibling paths, and schedule a delete for the original. Write proptests: "after rename_both, no path has overlapping local/remote state".

---

### C-5. Scheduler has no memory budget — unbounded `Vec` growth is a DoS

`crates/pcloud-engine/src/scheduler.rs:38-59`: `Scheduler::queued_operations` is a plain `Vec<PlannedOperation>` with no cap.

`crates/pcloud-engine/src/planner.rs:50-58`: `Planner::max_operations_per_tick` defaults to `1024`, but this bounds **per tick**, not the queue. Successive ticks accumulate indefinitely through `replace_queue`, which overwrites the queue entirely (not merged) — so it is coincidentally bounded **only because** every tick replaces everything, which is itself the C-2 bug.

If C-2 is fixed without adding a queue-size cap at the same time, this becomes a classic unbounded-queue memory DoS. A sync root pointing at a tree of 10M files will push 10M `PlannedOperation`s (each with a heap-allocated `String` path) into memory.

**Severity:** CRITICAL (conditional on C-2 fix).

**Remediation:** Add `Scheduler::max_queue_size: usize` (default ~100k), and on overflow either spill-to-disk or emit a `BackPressure` event back up the pipeline. The planner must also honor `max_ops` with a "deferred" queue that gets drained on subsequent ticks.

---

### C-6. FsEvent coalescing is unbounded — memory DoS under event storms

`crates/pcloud-engine/src/fs_events.rs:64-95`, `FsEventIngestor::normalize_events`:

```
pub fn normalize_events(&self, events: &[FsEvent]) -> Result<Vec<SyncCandidate>, FsEventError> {
    let mut coalesced = Vec::<FsEvent>::new();
    for event in events {
        validate_relative_path(&event.path)?;
        if let Some(existing) = coalesced
            .iter_mut()
            .find(|candidate| candidate.path == event.path)
        { ... }
```

Two problems:

1. The coalescer uses `Vec::find` — **O(n²)** for n distinct paths.
2. `coalesce_window_ms` (`:16-17`) is a struct field but is **never read**. The comment at `:10-12` promises time-window coalescing; the code performs ordering-based dedup only. An event stream with N distinct paths all spaced by any duration will still produce N candidates.

Additionally, the upstream `fs_watcher.rs` debouncer (see M-4 below) does coalesce by path within the debounce window, but the ingestor interface accepts arbitrary-sized batches and offers no ceiling.

**Severity:** CRITICAL — scan of a tree with 1M files through `inotify` events will blow up memory and burn CPU in an O(n²) loop.

**Remediation:** Replace `Vec::find` with `HashMap<String, FsEvent>`. Add `max_queued_events` cap; drop oldest or escalate to full-scan on overflow. Either remove `coalesce_window_ms` or honor it with an `Instant`-keyed coalescer.

---

### C-7. Audit-rebuild migration runs unbatched with no idempotency if interrupted

`crates/pcloud-store/src/migrations.rs:80-118`: `apply_plan` applies each `apply_schema_vN` with the **caller's** optional wrap in `TransactionBoundary::immediate`. `bootstrap_profile` (`crates/pcloud-store/src/lib.rs:202-233`) does NOT wrap the migration in a transaction — it calls `apply_plan(&conn, &plan)` directly, and only the individual DDL statements commit via their embedded `PRAGMA user_version = N`.

`crates/pcloud-store/src/schema.rs:168-193`, `apply_schema_v8`: calls `crate::repositories::audit::rebuild_hash_chain(conn)` between the `ALTER TABLE` and the `PRAGMA user_version = 8`. If the process is killed mid-rebuild:

- Columns `prev_hash`, `entry_hash`, `hmac` already exist (`:174-182` are separate `ALTER TABLE` statements, each auto-committed by SQLite outside a transaction),
- `rebuild_hash_chain` may have partially re-hashed rows,
- `user_version` is still 7,
- On next launch, `apply_schema_v8` runs again and calls `rebuild_hash_chain` a second time — if that routine is not idempotent over partial state, the chain becomes corrupt.

I did not read `rebuild_hash_chain` in full, but the migration path MUST be atomic regardless of its idempotency story. Defense in depth demands a wrapping transaction.

**Severity:** CRITICAL for audit-log integrity, which is the crate's headline security invariant.

**Remediation:** Wrap the entire migration plan in `TransactionBoundary::immediate` at the `bootstrap_profile` layer, remove the per-step `PRAGMA user_version` commits, and commit `user_version` atomically with DDL. Alternatively, keep step-wise commits but wrap each step (including its `rebuild_hash_chain`) in its own transaction that also bumps `user_version`.

---

### C-8. No stall detection, no transfer timeout

Searching the engine and runtime for "stall", "timeout", "inactivity": zero matches in `crates/pcloud-engine/src/` for stall detection. The `TransferTask` state (`crates/pcloud-engine/src/transfers/uploads.rs:152-158`) has `state: TransferState` but no `last_progress_at: Instant`.

`pcloud-resilience/src/timeout.rs` exists (82 lines) but is a generic wrapper, not engine-integrated. The sync loop runs with a 5-minute default poll interval (`crates/pcloud-daemon/src/sync_loop.rs` + config). An upload that "completes" its first chunk and then hangs forever on `upload_write` will sit in `active_uploads` indefinitely.

**Severity:** CRITICAL — the daemon can get stuck with phantom in-flight uploads that never complete, never fail, and never retry. End users see "syncing…" forever.

**Remediation:** Add `TransferTask::last_progress_at: Instant`, a `stall_timeout: Duration` (default 5 min), and a periodic scan in `EngineShell::advance_transfer_cycle` that marks stalled tasks as `Failed { reason: "stall_detected" }` and re-queues them through the recovery manager. Emit an audit event `sync.stall_detected` when this fires.

---

## HIGH findings

### H-1. No idempotency keys on uploads; mutation retries are globally disabled

`crates/pcloud-resilience/src/retry.rs:264-273`, `MethodRetryPolicy::secure_default`:

```
pub fn secure_default(inner: RetryPolicy) -> Self {
    Self {
        inner,
        retry_idempotent: true,
        retry_mutations: false,
        retry_unknown: false,
    }
}
```

So uploads (a mutation) will **never retry** under the default policy. That is safe against double-writes but means a single transient `503` aborts the upload and the user must trigger a new cycle.

Worse, the `UploadResumeRecord` (`crates/pcloud-store/src/repositories/upload_resume.rs:38-57`) carries an `upload_id` from the server's `upload_create`, so the upload *is* idempotent after that point — yet the retry policy has no way to express "retry only this mutation because I hold an `upload_id`".

There are also no idempotency tokens emitted on `upload_create` itself (which is not idempotent in the general sense). A network drop between client send and server response of `upload_create` results in an orphaned server-side upload handle with no way for the client to discover it — server-side cleanup is opaque.

**Severity:** HIGH — retriable flakes become user-visible failures; orphaned uploads accumulate on the server.

**Remediation:**

1. Add `MethodRetryPolicy::with_idempotency_keys` variant that opts in mutations *if* the caller provides a per-request idempotency token.
2. Require `upload_create` to send a client-generated UUID and have the server return a reused `upload_id` on replay. If that server support does not exist, document it as a spec gap and add a `cleanup_orphans` CLI.
3. Resume paths: always consult `upload_resume_state` before calling `upload_create`; test that a mid-`upload_create` crash does not create two server-side uploads for the same local file.

---

### H-2. Retry policy does not honor server's `Retry-After` header

`rg Retry-After` across `crates/`: zero matches in `pcloud-resilience`, zero in the sync loop, zero in the transfer/backends layer.

`crates/pcloud-resilience/src/retry.rs:100-120`: `RetryPolicy` only knows `Fixed` / `Exponential` / `ExponentialJittered` backoffs computed purely from `attempt` count.

When the pCloud server responds `429 Too Many Requests` with `Retry-After: 30`, the engine has no hook to pass that 30 seconds into the decision. The client ignores the server's explicit pacing signal and retries on its own internal schedule — which, depending on the exponential-jittered table, may be sooner than 30 s and will be blocked again.

**Severity:** HIGH — violates good-citizen behavior against the API, accelerates tenant-level rate-limit tripping, wastes server capacity.

**Remediation:** Extend `RetryDecision::Retry { wait: Duration }` with a `server_hint: Option<Duration>` and teach `RetryPolicy::next` to take a `server_hint: Option<Duration>` that overrides the schedule if present. The HTTP transport layer (Dimension 6) must extract `Retry-After` and plumb it in.

---

### H-3. No global retry budget; single-op retry budget is also implicit

`crates/pcloud-resilience/src/retry.rs:151-157`: `RetryPolicy::next(attempt)` is the only control. There is no *global* retry budget — if 10k uploads each retry 3 times simultaneously, the daemon will hammer the API with 30k calls in ~seconds.

**Severity:** HIGH — flaky network turns into self-inflicted DoS.

**Remediation:** Add `TokenBucket`-backed global retry budget in `pcloud-resilience`, separate from the per-request rate limiter. Reject retries when the bucket is empty (escalate to `ManualIntervention`).

---

### H-4. No case-insensitive / Unicode normalization for conflict detection

`crates/pcloud-engine/src/planner.rs:74-104`, `Planner::plan` pairs candidates by **exact string equality** on `path`. The validators (`fs_events.rs:98-111`, `local_scan.rs:273-286`, `diff_poller.rs:101-114`) reject `.`, `..`, empty segments, backslashes, but do **not** normalize.

- On macOS HFS+/APFS by default the filesystem is case-insensitive but case-preserving. A file created locally as `Report.txt` and remotely pulled as `report.txt` will be **two distinct candidates** that collide on disk at write time with no planner-level detection.
- HFS+ stores filenames in NFD, ext4/NTFS keep what you gave them. A file named with an accented character will have different byte sequences depending on the side — again two candidates for what is one conceptual file.

The `fs_watcher.rs::to_relative` also does `replace('\\','/')` on non-UTF-8 path handling lossily (`:245-254` uses `to_str()?` which drops non-UTF-8 names silently — see M-3).

**Severity:** HIGH — silent divergence, duplicate uploads, or failed writes on macOS with international content.

**Remediation:** Add a `path_normalize` module that (a) applies NFC (or the platform's native form) consistently, (b) on case-insensitive mounts compares lowercased keys while preserving the display form. Collisions after normalization should feed `ConflictKind::CaseCollision` into the planner.

---

### H-5. `FsEventIngestor::coalesce_window_ms` is a phantom field

`crates/pcloud-engine/src/fs_events.rs:13-27`: the field is declared, documented, and defaulted to 250 ms, but `normalize_events` at `:64-95` never consults it. The real debouncing happens in `pcloud-fs/src/fs_watcher.rs` with `WatcherConfig::debounce_duration: 500` (`:73-85`).

**Severity:** HIGH — public API surface lies about its behavior; config changes have no effect.

**Remediation:** Either delete the field (accept the watcher is the single source of debounce) or implement an Instant-keyed coalescer. Update `#[serde(deny_unknown_fields)]` accordingly so stale config files surface the removal.

---

### H-6. Watcher has no overflow detection / no rescan trigger

`crates/pcloud-fs/src/fs_watcher.rs:106-147`: `FsWatcher::start` installs `RecommendedWatcher` and registers a single callback (`:121-129`). On inotify overflow (`IN_Q_OVERFLOW`) the notify crate emits `EventKind::Other` or a rescan-request event, and this code at `:233-241` routes `_ => None` — i.e. **dropped silently**.

There is no re-scan trigger when the kernel buffer overflows. Files created while the inotify queue was overflowing will not be detected until the *next* full scan (up to 5 minutes later by default — `reconcile_worker.rs:38`).

**Severity:** HIGH — silent data loss on busy trees. An rsync run or a tarball extraction easily overflows `fs.inotify.max_queued_events` (16k default on Linux).

**Remediation:** Match `EventKind::Other` and any `notify`-specific overflow indicator; when observed, bump a counter, emit an audit event `sync.watcher_overflow`, and set `IncrementalScanTracker::request_scan` (equivalent) so the next tick forces a full walk.

---

### H-7. Debouncer flushes all pending events on disconnect with `debounce=0` — data loss on watcher shutdown is silent

`crates/pcloud-fs/src/fs_watcher.rs:184-197`:

```
Err(mpsc::RecvTimeoutError::Disconnected) => {
    // Watcher dropped; flush remaining and exit.
    flush_pending(&mut pending, &output_tx, sync_id, Duration::ZERO);
    break;
}
```

If the downstream `output_tx` receiver was already dropped, `flush_pending` will call `pending.clear()` at `:222-226` and return without surfacing that events were lost. No error propagates; the outer runtime has no way to know it should rescan on next startup.

**Severity:** HIGH — events at shutdown are silently discarded.

**Remediation:** Emit an audit event on shutdown-with-pending. Persist the last-successfully-drained sync_id + event cursor so restart does a full scan.

---

### H-8. No disk-budget / staging cap; staging eviction is lossy

`crates/pcloud-cache/src/staging.rs:29-41`: `max_open_files: 64` default, `files: HashMap<String, Vec<u8>>` unbounded in aggregate byte size, `open_order` LRU.

`crates/pcloud-cache/src/staging.rs:95-103`:

```
fn evict_if_needed(&mut self) {
    while self.files.len() > self.max_open_files {
        let Some(oldest) = self.open_order.pop_front() else { break; };
        self.files.remove(&oldest);
    }
}
```

Eviction here **drops the bytes** of the oldest staged file. The doc at `:1-8` acknowledges "Eviction here is lossy: evicted buffers are dropped, so callers must have already flushed them". But:

1. There is no enforcement that callers flushed before the 65th file is staged.
2. No per-buffer byte budget (a single 4 GiB file counts the same as a 1 KiB file for eviction).
3. No disk budget at all — nothing sets a maximum staging area size on disk.

A FUSE write path (`crates/pcloud-fs/src/write_path.rs`, Dim. 5 scope) or a batch of local creations above 64 files will silently lose bytes.

**Severity:** HIGH — data loss under normal usage.

**Remediation:** Bound by both count AND aggregate bytes (`max_staging_bytes: u64`). Before evicting, check a "must_flush_before_evict" callback; return an error to caller if the buffer cannot be flushed. Ultimately, staging of any non-trivial file must be disk-backed, not `Vec<u8>`.

---

### H-9. Scheduler eviction on sync-root remove is O(n) per op — bad for large queues

`crates/pcloud-engine/src/scheduler.rs:106-109`:

```
pub fn evict_sync_id(&mut self, sync_id: SyncId) {
    self.queued_operations
        .retain(|operation| operation.sync_id() != sync_id);
}
```

Same pattern at `transfers/uploads.rs:73-78` and `transfers/downloads.rs:73-78` — 5 separate `retain` calls per coordinator. With C-5 fixed and a 100k queue, removing a sync root becomes an O(500k) walk per evict. Not catastrophic, but trivially fixable.

**Severity:** HIGH when combined with large queues.

**Remediation:** Index queued ops by `sync_id` in a `BTreeMap<SyncId, VecDeque<PlannedOperation>>` so evict is `O(ops_for_that_root)` not `O(total_ops)`.

---

### H-10. No back-pressure from transport to ingestion

Grep `back_pressure` / `429` / `throttle` in `crates/pcloud-engine` returns zero. The engine ingests `FsEvent`s and remote diff batches as fast as they arrive regardless of whether the transport is successfully draining the queue.

If the server is returning `429`s repeatedly, the engine keeps building up queued operations, the staging area keeps filling up, and the memory/disk footprint grows unbounded.

**Severity:** HIGH — amplification of transient server problems into OOM.

**Remediation:** Add a `PressureSignal` event emitted by the HTTP client (Dim. 6) that the engine's ingestion paths consult before `ingest_fs_events` / `ingest_remote_diff`. When pressure is high, `FsEvent`s should be coalesced more aggressively into a "dirty-region" set rather than individual events.

---

### H-11. `diff_state` persistence is not transaction-bound to the ingestion that used it

`crates/pcloud-store/src/repositories/diff_state.rs`: I only sampled the file listing, but the repository's doc at `schema.rs:237-263` states the cursor is updated when the DiffWorker advances. There is no evidence in `RealSyncLoopRuntime` (searched for "diff_state") that the cursor advance is atomic with the local engine's planner ingest.

If the cursor is advanced *before* the planner successfully planned the batch, a crash loses the diff events and the engine has no way to re-fetch them (the server treats the cursor as read). If advanced *after*, duplicates are produced on crash — which is safer because uploads are then idempotent, **but** only if H-1 is fixed.

**Severity:** HIGH — data loss vs duplicate-work trade-off is not explicitly documented or tested.

**Remediation:** Advance the diff cursor **only** after the planner successfully persisted the new sync candidates (if persistence exists — none does today; see H-13). Alternatively, keep cursor-advance post-plan but test both crash windows with `upload_journal_crash_replay`-style integration tests.

---

### H-12. Crash recovery has no engine tests

`ls /home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/tests/` returns **nothing** — the engine crate has no integration tests at all. All tests are in `mod tests` blocks in each `.rs` file.

`crates/pcloud-daemon/tests/` contains `upload_journal_crash_replay.rs` (present — good) but no conflict-resolution crash test, no watcher-overflow crash test, no "kill mid-cycle and restart" test.

**Severity:** HIGH — the critical path for durability has zero integration proof.

**Remediation:** `crates/pcloud-engine/tests/crash_recovery.rs` covering: (a) SIGKILL between diff-cursor advance and plan emission, (b) SIGKILL during `advance_transfer_cycle`, (c) SIGKILL during `mark_transfer_completed`, (d) restart and observe completed work is not re-done. Use the `pcloud-chaos` crate if it supports process injection.

---

### H-13. Engine state is entirely non-durable — everything lives in memory

`crates/pcloud-engine/src/lib.rs:60-103`: `EngineShell` holds scheduler, coordinators, pause set, conflict queue — all in-memory `Vec` / `HashMap`.

The only engine-side durable state is the upload-resume repository (`crates/pcloud-store/src/repositories/upload_resume.rs`) and the diff-state cursor. Conflicts detected by the planner, queued operations, and in-flight transfers are **not** persisted.

A daemon restart wipes:

- all in-flight conflict resolutions,
- all queued operations awaiting retry under manual-intervention policy,
- the entire scheduler history.

`RecoveryManager::classify_failure` (`crates/pcloud-engine/src/recovery.rs:122-156`) returns a `RecoveryDecision` but there is no place that persists "operation X is waiting for a retry at time T". The module doc at `:22-25` even admits this: "The classifier is a pure function of (operation, failure); it does not consult history, exponential back-off, or the store. Back-off sequencing lives in the scheduler/transfer coordinators." — but the coordinators also do not persist.

**Severity:** HIGH — restart clobbers all in-flight retry state; every restart forces re-classification and re-discovery of conflicts.

**Remediation:** A `sync_operation_journal` table mirroring the shape of `PlannedOperation` with `(sync_id, path, op_kind, state, next_retry_at, retry_attempt, last_error)`. Persist on every `accept_batch` and every `mark_completed` / `mark_failed`. Provide `EngineShell::rehydrate_from_store(&Connection)`.

---

### H-14. `SessionManagerActor` is a 22-line stub

`crates/pcloud-engine/src/session_manager.rs` is only 22 lines. The module doc at `lib.rs:38-39` says "Per-sync-root engine state actor". The actor is essentially empty; no per-sync-root state machine exists.

**Severity:** HIGH — the comment architecture suggests per-sync-root isolation; the code does not deliver it, which cascades into C-1 (no fairness) and H-9 (slow eviction).

**Remediation:** Either build out the per-sync-root actor or rename the module so docs match reality.

---

## MEDIUM findings

### M-1. Ingress path `ingest_candidates` discards the Delete-policy when called directly

`crates/pcloud-engine/src/lib.rs:203-207`:

```
pub fn ingest_candidates(&mut self, candidates: &[SyncCandidate]) -> &[PlannedOperation] {
    let operations = self.planner.plan(candidates);
    self.scheduler.replace_queue(operations);
    self.scheduler.next_batch()
}
```

This path bypasses `DeletePolicy::for_sync_type`. It is still public and called from tests and presumably from code paths that forget the `_filtered` variant. A `BackupArchive` root that flows through `ingest_candidates` (not `ingest_candidates_filtered`) will emit `DeleteRemote` operations the policy explicitly forbids.

**Severity:** MEDIUM — a future caller is going to use the wrong variant.

**Remediation:** Make `ingest_candidates` accept a `DeletePolicy` directly or mark it `#[deprecated]` in favor of the `_filtered` form.

---

### M-2. Exponential-backoff jitter truncates `Instant` math on 32-bit

`crates/pcloud-resilience/src/retry.rs:178-194`:

```
let nanos = (base.as_nanos() as f64) * exp;
...
let as_u128 = clamped as u128;
Duration::new(
    (as_u128 / 1_000_000_000) as u64,
    (as_u128 % 1_000_000_000) as u32,
)
```

`as_nanos as f64` loses precision above ~2^53 nanoseconds (≈ 104 days). For reasonable backoff windows the loss is irrelevant, but the casts `as_nanos as f64 * exp -> as u128 -> Duration::new with truncated nanos` chain is fragile. The test at `:341-371` never covers extreme values.

**Severity:** MEDIUM — not a bug today; a property test would catch regressions.

**Remediation:** Proptest `compute_wait` against `Duration::MAX` / `factor = 1.0 to 10.0` / `attempt in 1..20`.

---

### M-3. Path handling in `fs_watcher::to_relative` silently drops non-UTF-8 paths

`crates/pcloud-fs/src/fs_watcher.rs:245-255`:

```
fn to_relative(path: &Path, root: &Path) -> Option<String> {
    path.strip_prefix(root).ok().and_then(|rel| {
        let s = rel.to_str()?;
        ...
        Some(s.replace('\\', "/"))
    })
}
```

Non-UTF-8 file names on Linux are legal but rare; `to_str()?` silently drops them. The user never learns why their `résumé.tex` (actually non-UTF-8 bytes) does not sync.

**Severity:** MEDIUM — locale-specific silent data skip.

**Remediation:** Replace `to_str` with `to_string_lossy` + a warning log + an audit event when lossy conversion happens.

---

### M-4. Debouncer re-flushes pending map on every loop iteration

`crates/pcloud-fs/src/fs_watcher.rs:162-197`: after each `notify_rx.recv_timeout` return (whether Ok or Timeout), the code calls `flush_pending` at `:196`, plus `flush_pending` was already called inside the `Timeout` branch at `:186`. Double-flush is idempotent since `matured` drains the map, but the double-iteration over a large pending map is wasteful.

**Severity:** MEDIUM — CPU waste on busy watchers.

**Remediation:** Flush once per iteration.

---

### M-5. No power/battery awareness for the sync loop itself

Grep `pause_on_battery` shows hits only in `integrity_sweeper_service.rs` — the integrity sweeper has a `PowerSource` trait (`:388-425`) and `PlatformPowerSource::new()` for Linux / macOS / Windows. The **sync loop itself** does not consult any power source.

Enterprise laptops routinely configure "pause heavy sync on battery"; the C pCloud client has such a setting. In this fork the feature is implemented only for the integrity sweep, not for uploads/downloads.

**Severity:** MEDIUM — feature regression vs C client; battery life impact on laptops.

**Remediation:** Extend `SyncLoopShared` with `pause_on_battery: AtomicBool` and a scheduler thread that consults `PlatformPowerSource` once per poll interval, calling `SyncLoopShared::pause` / `resume` based on power state.

---

### M-6. Pause/resume is not fsync-durable

`crates/pcloud-engine/src/lib.rs:456-470`: `pause_sync_root` / `resume_sync_root` only mutate in-memory `paused_sync_roots: BTreeSet<SyncId>`.

There is a `paused` column on `sync_root_records` (`crates/pcloud-store/src/schema.rs:72-85`), but no evidence that in-memory pause and persisted pause are synchronized — `pause_sync_root` does not hit the store, `resume_sync_root` does not hit the store. A daemon restart reloads from the store, which forgets any pause that IPC applied since bootstrap.

**Severity:** MEDIUM — operator pauses a misbehaving root, restarts the daemon, and the root is live again.

**Remediation:** In `RuntimeShell::pause_sync_root` (not the engine's — the runtime has the store handle), write the column BEFORE updating the engine. On resume, write `paused=0`.

---

### M-7. Condvar `wait_on_condvar` does not handle spurious wake-ups optimally

`crates/pcloud-daemon/src/sync_loop.rs:343-355`: the `wait_timeout_while` predicate is sound (`!*woken && !shutdown`), but a spurious wake that finds both false will correctly loop — however the `if let Ok(mut g, _)` discards the `WaitTimeoutResult`. The cleared `*g = false` runs regardless of shutdown, so a shutdown-triggered wake clears the flag and causes another loop iteration to observe shutdown only on its own atomic read at `:372-378`. This is correct, just non-obvious.

**Severity:** MEDIUM — correctness OK; code is subtle enough to regress.

**Remediation:** Return a `WakeReason { shutdown: bool, external: bool, timeout: bool }` from `wait_on_condvar` so the main loop handles each branch explicitly.

---

### M-8. Circuit breaker is not per-endpoint

`crates/pcloud-resilience/src/circuit_breaker.rs:116-122` describes a single `CircuitBreaker` instance. The daemon's HTTP transport would likely use one, but the sync loop has no visible per-endpoint isolation — one failing endpoint (`diff`, say) trips the entire network path.

**Severity:** MEDIUM — an endpoint outage in `/listshares` blocks `/upload_create`.

**Remediation:** Keep a `HashMap<Endpoint, CircuitBreaker>` in the transport layer and let the sync loop observe per-endpoint state.

---

### M-9. No per-sync-root pause persistence + no pause reason

`crates/pcloud-engine/src/lib.rs:91-95`: `paused_sync_roots: BTreeSet<SyncId>` holds only ids, no reason (user, auto-pause-on-checksum-mismatch, auto-pause-on-quota-exceeded).

**Severity:** MEDIUM — operator cannot distinguish user pause from system pause, resume races ensue.

**Remediation:** `PauseReason { UserRequested, QuotaExceeded, AuthExpired, IntegrityFailure }` carried in the map.

---

### M-10. `RecoveryManager` has no exponential history

The doc (`recovery.rs:22-25`) explicitly admits "does not consult history, exponential back-off". A task that fails with `RetryableNetworkError` will return `RetryLater` forever, no matter how many times it has failed. The engine has no notion of "give up after N retries".

**Severity:** MEDIUM — infinite-retry loops on pathological tasks.

**Remediation:** Add `retry_count: u32` to the per-task journal (see H-13) and have `classify_failure` escalate to `Terminal` after a configurable threshold.

---

### M-11. `Planner::max_operations_per_tick` default of 1024 may be too small for real trees

`crates/pcloud-engine/src/planner.rs:48-58`: default cap is 1024. A sync root with 100k files on initial scan will need ~100 ticks to fully plan, each separated by the sync loop's poll interval (default: a few seconds). Initial sync of a large tree takes forever.

**Severity:** MEDIUM — UX issue on first sync.

**Remediation:** Increase default or switch to adaptive: plan up to whatever fits in a memory budget (≈100k entries ≈ 10–20 MB).

---

### M-12. `ingest_candidates` resets the scheduler queue every call

`crates/pcloud-engine/src/lib.rs:203-207`: `scheduler.replace_queue(operations)` **replaces**. If the caller issues two consecutive `ingest_candidates` with non-overlapping path sets, the second call discards the first batch from the queue entirely.

This is linked to C-2 but distinct. Even with C-2 fixed (dequeue on dispatch), a replace-based ingestion loses information about previously-queued items that have not yet been dispatched.

**Severity:** MEDIUM — semantic ambiguity; a sequence of small batches vs one big batch produces different queue contents.

**Remediation:** `ingest_candidates` should merge into the queue (by path) rather than replace, and have an explicit `clear_queue()` for the runtime teardown path.

---

### M-13. Sync loop's global error counter does not reset

`crates/pcloud-daemon/src/sync_loop.rs:415-417`:

```
status.total_errors += cycle.total_errors as u64;
```

Monotonically increasing. No "errors-per-cycle" rate metric, no decay. An operator running for months will see huge numbers that convey no recent health signal.

**Severity:** MEDIUM — observability issue.

**Remediation:** Keep rolling windows (last 5 min, last hour) plus cumulative.

---

### M-14. Store uses a single global Mutex — no reader concurrency

`crates/pcloud-store/src/lib.rs:266-299`: `StoreHandle` is a single long-lived `Mutex<Connection>`. WAL journaling gives multiple-reader potential but the mutex serializes both reads and writes. For the sync loop, which reads `sync_root_records` at the start of every cycle, this contends with writes from IPC.

Note: `sync_loop_runtime.rs:141-147` bypasses `StoreHandle` and opens its own `Connection` directly ("safe to open concurrently because WAL"). That works but now the crate has two separate connection strategies; `StoreHandle` invariants are not enforced on the sync-loop path.

**Severity:** MEDIUM — architectural drift and undocumented contention profile.

**Remediation:** Either move `StoreHandle` to `RwLock<Connection>` for genuine reader concurrency, or standardize on "one short-lived connection per operation" and deprecate `StoreHandle`.

---

### M-15. `sync_diff_state` has no FK to `sync_root_records`

`crates/pcloud-store/src/schema.rs:244-262` v10 migration says "we do not declare a real FK because diff state can outlive a transient sync_root remove/re-add". That reasoning is wrong — it is cheaper to delete the diff state on root remove (explicit) than to keep orphan rows across restarts. Orphan `sync_diff_state` rows will accumulate silently on long-lived daemons.

**Severity:** MEDIUM — long-tail data hygiene.

**Remediation:** Add `FOREIGN KEY (sync_id) REFERENCES sync_root_records(sync_id) ON DELETE CASCADE` and delete the "do not declare FK" comment.

---

### M-16. No background GC for stale upload_resume_state rows

`crates/pcloud-store/src/repositories/upload_resume.rs:136-142`: `delete` only runs on explicit success path. A local file that was deleted before its upload completed leaves a stale resume row forever, holding a server-side `upload_id` that is now orphaned.

**Severity:** MEDIUM — long-running daemon collects junk rows.

**Remediation:** Periodic sweep (hourly or on boot) that drops rows older than 24 h.

---

### M-17. `upload_resume_state` primary key is `local_path` — symlink traversal races possible

`crates/pcloud-store/src/repositories/upload_resume.rs:38-57`: PK is the canonicalized local path string. If a file is renamed between `upload_create` and the next `upload_write`, resume lookup fails silently and the client re-starts from zero — wasting bytes uploaded so far.

**Severity:** MEDIUM — wasted bandwidth on renames.

**Remediation:** PK on `(inode, device)` when available; fall back to path when not (Windows).

---

### M-18. No checksum-based dedup before upload

Grep `checksum` / `sha256` within the engine crate shows zero. A sync root with two copies of the same file (common with backups) uploads both independently. The server has `checksumfile` but the client never queries it before `upload_create`.

**Severity:** MEDIUM — bandwidth waste on legitimate dedup scenarios.

**Remediation:** For files above a threshold (~1 MiB), hash locally and check server-side before uploading.

---

## LOW findings

### L-1. `Scheduler::next_batch` returns `&[PlannedOperation]`, not owned

`scheduler.rs:123`: callers who want to mutate (e.g. `advance_transfer_cycle` in `lib.rs:409-414` which calls `next_batch().to_vec()`) need to clone. Minor allocation cost.

**Severity:** LOW.

**Remediation:** Return `Vec<PlannedOperation>` directly from an integrated dequeue method.

---

### L-2. `FsEventKind` has only Write/Create/Remove — no Rename

`crates/pcloud-engine/src/fs_events.rs:29-37`: missing Rename as first-class. A file renamed locally produces `Remove` + `Create`, which the planner treats as a delete followed by a separate upload — losing the rename semantic, doubling server state churn.

**Severity:** LOW.

**Remediation:** Add `Rename { from: String, to: String }` and map notify's `EventKind::Modify(ModifyKind::Name(..))` into it.

---

### L-3. `Scheduler` and `Planner` are `Serialize + Deserialize` but never serialized

`scheduler.rs:37-38` derives `Serialize, Deserialize`. No callsite serializes them. Dead annotations suggest durability was planned but not implemented (link to H-13).

**Severity:** LOW.

**Remediation:** Either remove the derives or actually persist.

---

### L-4. Pacer uses `std::thread::sleep` from a sync context — blocks tokio runtimes

`crates/pcloud-resilience/src/pacing.rs:49-52` and `:23-26`: `BandwidthPacer::pace` blocks the calling thread. If the caller happens to be inside a tokio `spawn_blocking` or a tokio reactor, it blocks a worker. The sync loop uses `std::thread`, so this is currently OK — but the doc should say "NEVER call from a tokio async fn".

**Severity:** LOW.

**Remediation:** Add a `#[must_not_use_in_async]` lint or at least a sharp doc warning.

---

### L-5. `FsEvent::validate_relative_path` duplicated three times

`fs_events.rs:98-111`, `local_scan.rs:273-286`, `diff_poller.rs:101-114` are near-identical. Maintenance hazard.

**Severity:** LOW.

**Remediation:** Extract to `pcloud-model` or a shared `pcloud-engine::path_validator` module.

---

### L-6. `sync_one_root` has no timeout

`crates/pcloud-daemon/src/sync_loop.rs:254-300`: one root with 1M files can take an hour to scan and plan. The cycle waits for it, which delays all other roots' cycles. Compounds with C-1.

**Severity:** LOW (given C-1 dominates).

**Remediation:** Per-root time budget; yield after N ms back to the scheduler.

---

### L-7. `reconcile_worker` interval default is 300 s; C client uses 10 s

`crates/pcloud-engine/src/reconcile_worker.rs:38`: `RECONCILE_DEFAULT_INTERVAL_SECS = 300`. The module doc at `:23-26` acknowledges "the C `PSYNC_LOCALSCAN_RESCAN_INTERVAL` is more aggressive at 10s but only fires after change events". This is a surprising default for a "sync" product — users expect sub-minute propagation.

**Severity:** LOW.

**Remediation:** Make it configurable via `SyncLoopConfig` and default to 60 s.

---

### L-8. `SyncLoopStatus::last_cycle_duration_ms` overflows at ~49 days

`sync_loop.rs:412`: `cycle.duration.as_millis() as u64` casts from `u128` — no overflow concern there — but any single cycle over 49 days would indicate a stuck daemon. Not a real bug; sanity-check could be cleaner.

**Severity:** LOW.

**Remediation:** Keep a `last_cycle_duration: Duration` typed field.

---

### L-9. `UploadCoordinator::accept_batch` clears previous active uploads

`crates/pcloud-engine/src/transfers/uploads.rs:48-51`:

```
self.active_uploads.clear();
self.pending_remote_deletes.clear();
self.pending_directory_creates.clear();
```

Same shape at `downloads.rs:48-51`. If `advance_transfer_cycle` is called twice in a row with the same batch, all in-flight work is silently reset. This is part of the broader C-2 issue but worth calling out: the coordinators trust that their caller will not re-call `accept_batch` mid-flight.

**Severity:** LOW (documented implicitly via "one call per cycle" contract).

**Remediation:** Defensive check: refuse to `clear()` if any of these lists has state != Streaming.

---

### L-10. `UploadCoordinator::chunk_size_bytes: 8 MiB` default — too large for low-memory devices

`crates/pcloud-engine/src/transfers/uploads.rs:32-42`: 8 MiB per in-flight upload × 4 parallel uploads = 32 MiB minimum staging. On embedded devices this is a noticeable floor.

**Severity:** LOW.

**Remediation:** Scale by available memory; default to 4 MiB.

---

### L-11. Tests are unit-only; no property tests for the scheduler

`crates/pcloud-engine/src/scheduler.rs:130-218`: three tests, all deterministic. No proptest covering "ingest N ops, dispatch M, evict K, no ops lost or duplicated".

**Severity:** LOW.

**Remediation:** Add a proptest exercising the ingest→dispatch→complete→evict state machine.

---

### L-12. `DeletePolicy::for_sync_type` does not expose which sync types exist

`crates/pcloud-engine/src/planner.rs:180-210`: matches on `SyncType::Full | UploadOnly | DownloadOnly | BackupArchive`. If a new `SyncType` is added to `pcloud-model`, this match is non-exhaustive without `#[non_exhaustive]` contract — the compiler will error cleanly, which is fine, but there is no dedicated fallback.

**Severity:** LOW.

**Remediation:** Add `SyncType::_` arm that defaults to the most restrictive policy, with a log warning.

---

### L-13. Upload resume records store the `local_path` but not the `sync_id`

`crates/pcloud-store/src/repositories/upload_resume.rs:38-57`: no `sync_id` column. When a sync root is removed, there is no way to `DELETE FROM upload_resume_state WHERE sync_id = ?`.

**Severity:** LOW.

**Remediation:** Add `sync_id INTEGER` column in a v12 migration.

---

### L-14. `Planner::plan` clones every candidate

`crates/pcloud-engine/src/planner.rs:75-82`: `sorted = candidates.to_vec()` then internally clones again into `local`/`remote`. 3× allocation per candidate. For 10k candidates that is ~300k allocations.

**Severity:** LOW — perf.

**Remediation:** Sort indices, consume by reference.

---

### L-15. `LocalScanner` / `DiffPoller` configs are `Serialize + Deserialize` but never loaded

`local_scan.rs:20-32`, `diff_poller.rs:14-24`: derives exist but no persistence story. Orphan configuration surface.

**Severity:** LOW.

**Remediation:** Either wire them to `ConfigProfile` or remove derives.

---

### L-16. `EngineShell::unresolved_conflict_count` is O(n) — called from hot paths

`lib.rs:305-312`: walks `queued_operations` linearly. Called from `summary()` at `:175-198` which is called by every cycle. With large queues this is a few microseconds, but it is a trivially cachable counter.

**Severity:** LOW.

**Remediation:** Maintain an `unresolved_conflicts: usize` counter bumped on enqueue/dequeue.

---

### L-17. `ChunkedUploadTracker` is dead code

`crates/pcloud-engine/src/transfers/uploads.rs:188-200`: declared but grep for `ChunkedUploadTracker::` elsewhere returns nothing. The chunk-tracking story goes through `upload_resume.rs` in the store, which is a parallel shape. Two sources of truth.

**Severity:** LOW.

**Remediation:** Delete `ChunkedUploadTracker` or unify it with the store record.

---

### L-18. `wake_localscan` is a counter increment, nothing wakes the scanner

`crates/pcloud-engine/src/lib.rs:145-160`: the method just bumps `localscan_wakes`. The comment admits the C wake path is not implemented. In practice the sync loop's condvar wakes the entire loop, not a per-root scanner.

**Severity:** LOW — clarity issue.

**Remediation:** Delete this method or wire it to the reconcile worker's `request_scan`.

---

### L-19. Test `engine_ingest_local_scan_with_delete_policy_suppresses_deletes`

`crates/pcloud-engine/src/lib.rs:768-793`: the test comment itself admits the test doesn't validate anything: `let _ = ops; // just verify it does not panic`. This is a ghost test.

**Severity:** LOW.

**Remediation:** Replace with a real assertion or delete it.

---

### L-20. `SelectivePolicy::matches` (selective.rs:346 lines) not reviewed here

Out of scope for the scheduler/queue audit but worth flagging: selective-sync filtering is the last point where a file can still escape sync, and it lives right next to the planner. A separate deep-dive would be prudent.

**Severity:** LOW (scope note).

---

## Test coverage classification

### Engine crate (`crates/pcloud-engine/`)

Only in-file `#[cfg(test)] mod tests`. **No `tests/` directory.**

Covered:
- `conflict_resolver.rs` unit tests (7 tests, all policies)
- `planner.rs` unit tests (`DeletePolicy` variants, conflict classification)
- `scheduler.rs` (3 tests: priority ordering, batch limit, eviction)
- `recovery.rs` (2 tests: network retry, checksum mismatch)
- `fs_events.rs` (3 tests: normalize, coalesce, reject-invalid)
- `local_scan.rs` (scanner normalization, selective policy)
- `diff_poller.rs` (normalize batch, reject malformed)
- `reconcile_worker.rs` (4 tests: idle/fire/untrack/request_scan)

NOT covered:
- Crash recovery of the engine
- Per-sync-root starvation (C-1)
- Scheduler dequeue semantics (C-2 — untested because the bug is untested)
- `newest_wins` with varying timestamps (C-3 — the only test bakes in the wrong behavior)
- `rename_both` actually renaming (C-4 — test accepts ManualReview output)
- Unbounded queue / staging overflow (C-5, H-8)
- Watcher overflow handling (H-6)
- Idempotency of upload retries (H-1)
- Retry-After honoring (H-2)
- Case / NFC conflict detection (H-4)

### Daemon crate (`crates/pcloud-daemon/tests/`)

18 test files. Relevant ones:
- `sync_loop_e2e.rs` — high-level loop path (good)
- `proptest_sync_and_resolver.rs` — proptest for resolver (good)
- `upload_journal_crash_replay.rs` — upload crash replay (good)
- `graceful_drain.rs` — shutdown drain (good)
- `integrity_walker.rs` — sweeper (Dim. 8 territory)

No test file covers scheduler-starvation, conflict-rename-both semantics, staging overflow, or watcher overflow.

### Store crate (`crates/pcloud-store/`)

**No `tests/` directory.** Each repository has an in-file test module (sampled: `upload_resume.rs:180+`, `audit.rs`, etc.). Migration path is tested only via the bootstrap round-trip. No test covers crash-during-migration (C-7).

### Resilience crate (`crates/pcloud-resilience/`)

One `tests/` file: `circuit_breaker_proptest.rs` (good). No property tests for `RetryPolicy` (M-2 untested), no property tests for `TokenBucket`.

---

## Resource leaks

### Thread join handles

`crates/pcloud-daemon/src/sync_loop.rs:432-470`: `SyncLoopHandle` owns an `Option<JoinHandle<()>>` and `impl Drop` performs best-effort join. OK.

`crates/pcloud-fs/src/fs_watcher.rs:139-144`: `thread::Builder::new().spawn(move || debounce_loop(...))` is **not** stored. The thread runs until the channel disconnect is observed. On drop of `FsWatcher`, the `_watcher` field drops (stopping notify), which drops the `notify_tx` sender, which closes the channel from the other end, which should let `debounce_loop` observe `Disconnected` and exit.

Correctness depends on notify crate actually closing `notify_tx` when the watcher is dropped. If the notify crate parks the sender in its own thread (which it does on most backends), the debounce thread may outlive `FsWatcher` briefly. Not a leak, but a shutdown race:

```
FsWatcher drop → notify thread exits → notify_tx drops → debounce_loop sees Disconnected
```

If any link fails (notify thread stuck), the debounce thread runs forever.

**Severity:** MEDIUM. Already covered in spirit by H-7.

**Remediation:** Store the debounce thread's `JoinHandle`; on `FsWatcher::drop`, join it with a timeout.

### mpsc channels

`sync_loop_runtime.rs:95-101`: `watchers: HashMap<SyncId, (FsWatcher, Receiver)>`. When a sync root is removed, the entry is dropped, which drops both `FsWatcher` and `Receiver`. That should cascade to the debounce thread exit path above. No leak.

`integrity_sweeper_service.rs:102`: `mpsc::{Sender, Receiver}` — large file not fully audited.

### File handles

No file handles kept on long-lived engine structs. Staging cache (`crates/pcloud-cache/src/staging.rs`) holds `Vec<u8>` only. No `std::fs::File` in any engine state.

---

## Summary

| Severity | Count |
|---|---|
| CRITICAL | 8 |
| HIGH | 14 |
| MEDIUM | 18 |
| LOW | 20 |

The sync engine has a clean, test-friendly architecture (pure state machine, injected clock, composable coordinators) but several **advertised features are not actually implemented** (C-3 `newest_wins`, C-4 `rename_both`, M-5 battery awareness for the sync loop, L-18 `wake_localscan`). The queue model **does not provide fairness across sync roots** (C-1) and **does not dequeue** (C-2) — both are fundamental. Durability of engine state is non-existent (H-13); restart wipes in-flight retry state and the conflict queue.

The most economically serious items to fix, in order:

1. **C-2 (queue dequeue)** — without it every cycle re-emits the same work.
2. **C-1 (fairness)** — enterprise-blocking; a big root starves everything else.
3. **C-3 + C-4 (conflict policies)** — silent data loss under default settings.
4. **C-8 (stall detection)** — ghost in-flight tasks indefinitely.
5. **H-1 + H-2 (retry with idempotency + Retry-After)** — good-citizen and transient-failure handling.
6. **H-13 (engine state durability)** — restart survivability.
7. **C-6 / C-5 / H-8 (bounded queues and staging budgets)** — DoS prevention.

Every fix should come with proptests in a new `crates/pcloud-engine/tests/` directory; today that directory does not exist and the engine has no integration tests at all.
