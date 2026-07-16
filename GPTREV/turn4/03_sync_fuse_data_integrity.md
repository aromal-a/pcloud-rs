# Turn 4 Sync/FUSE/Data Integrity Review

Read-only audit using `pcloud_rev.md` as the master prompt. No files were edited.

## Findings

### CRITICAL: FUSE journal replay does not replay writes

Evidence: `crates/pcloud-fs/src/write_journal.rs:1`, `crates/pcloud-fs/src/write_path.rs:1109`, `crates/pcloud-fs/src/fuser_shim.rs:222`, `crates/pcloud-daemon/src/mount_runtime.rs:968`.

The journal docs promise remount replay, but `replay_journal` only returns records, `fuser_shim` logs recovered records, and mount bootstrap only reconciles upload sidecars. A crash after acknowledged FUSE writes can leave dirty staged data stranded without backend replay.

Remediation: implement a real replay executor before serving mount traffic. Reapply create/write/truncate/rename/unlink/barrier records against staging and remote transport, checkpoint only after backend ACK, and fail or mount read-only if replay cannot complete.

### CRITICAL: Shared write journal checkpointing can erase unrelated dirty-file recovery records

Evidence: `crates/pcloud-fs/src/write_path.rs:436`, `crates/pcloud-fs/src/write_path.rs:596`, `crates/pcloud-fs/src/write_path.rs:909`, `crates/pcloud-fs/src/write_path.rs:851`, `crates/pcloud-fs/src/write_path.rs:870`, `crates/pcloud-fs/src/write_path.rs:958`.

One `WritePathService` owns one `Mutex<WriteJournal>` for all dirty inodes. A whole-file flush resets the entire journal after one file upload, which can discard records for another still-dirty inode. Chunked flush records ACKs and saves remotely but does not compact/checkpoint the main journal. Truncate mutates staging before journaling, violating write-ahead ordering.

Remediation: use per-inode journals or sequence-based compaction that removes only records covered by a completed flush. Journal truncate before staging mutation. Add crash tests with two dirty inodes where inode A flushes, the process crashes, and inode B must still replay.

### HIGH: Local sync uploads do not read ordinary source files

Evidence: `crates/pcloud-engine/src/local_scan.rs:36`, `crates/pcloud-model/src/sync.rs:316`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:1073`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:1104`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:739`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:775`.

`LocalScanEntry` and `UploadFile` carry metadata and relative path, but no absolute source path or payload handle. Upload execution only looks for staged/cache payload bytes and marks missing payloads as failed. A normal file created under a sync root can be discovered and planned but never uploaded unless it also exists in FUSE staging/cache.

Remediation: carry the sync root base path or durable payload reference into upload execution. Open the local file safely with root-containment and symlink checks, snapshot size/mtime/hash, stream from disk, and retry/quarantine if the file changes mid-upload.

### HIGH: Retryable sync failures are classified but not requeued

Evidence: `crates/pcloud-engine/src/recovery.rs:123`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:660`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:789`, `crates/pcloud-daemon/src/runtime.rs:2699`, `crates/pcloud-engine/src/lib.rs:846`.

The engine exposes `requeue_for_retry`, but daemon execution paths classify transient failures and then only mark operations failed. `rg requeue_for_retry` found no daemon caller. Retryable network failures can park work until a later scan/restart happens to rediscover it.

Remediation: on `RetryLater`, persist retry metadata, call `requeue_for_retry`, clear the failed marker when requeued, and add tests proving transient upload/download/create failures return to the pending queue with bounded backoff.

### HIGH: Sync planning still separates remote and local observations for the same cycle

Evidence: `crates/pcloud-daemon/src/sync_loop.rs:283`, `crates/pcloud-engine/src/lib.rs:461`, `crates/pcloud-engine/src/lib.rs:496`, `crates/pcloud-engine/src/planner.rs:115`, `crates/pcloud-engine/src/planner.rs:348`.

Remote diff candidates are ingested before local scan candidates, and each ingest replaces scheduler contents for the scoped sync. The planner can only pair local/remote conflicts that appear in the same batch. Same-path local and remote changes in one cycle can be planned as separate operations or lose pending remote work when local scan replacement runs.

Remediation: build one per-root observation set per cycle and call the planner once, or change ingest to merge observations without replacing pending remote work. Add conflict tests covering same-path local edit plus remote diff in the same sync cycle.

### HIGH: Daemon sync uploads bypass the idempotent/resumable upload path

Evidence: `crates/pcloud-proto/src/transfer_api.rs:249`, `crates/pcloud-backends/src/transfer_backend.rs:670`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:763`, `crates/pcloud-daemon/src/runtime.rs:2798`, `crates/pcloud-backends/src/transfer_backend.rs:1195`, `crates/pcloud-daemon/src/transfer_bridge.rs:217`.

The protocol supports idempotency keys and the chunked driver threads them through create/write/save, but daemon sync upload paths use `upload_create` plus `upload_bytes` with no key. Upload resume state exists, but bootstrap only logs resumable rows and `transfer_bridge` still has TODOs for actual resumption.

Remediation: route daemon sync uploads through the chunked/idempotent state machine with stable keys persisted in `upload_resume_state`. Resume pending sessions at startup before issuing new uploads, and stream from disk rather than holding whole files in memory.

### HIGH: Integrity sweeper is not wired to production roots or remote checksums

Evidence: `crates/pcloud-daemon/src/integrity_sweeper_service.rs:198`, `crates/pcloud-daemon/src/integrity_sweeper_service.rs:662`, `crates/pcloud-daemon/src/runtime.rs:6785`, `crates/pcloud-daemon/src/bootstrap.rs:836`.

The default checksum fetcher is `NoOpChecksumFetcher`, `bootstrap_integrity_sweeper` is empty, and production code does not call `set_checksum_fetcher`. Tests passing for `noop_fetcher_returns_not_found` and `run_once_with_no_roots_produces_no_events` confirm the current no-op behavior.

Remediation: bootstrap sweep roots from persisted sync roots, inject a real authenticated remote checksum fetcher, start the worker when enabled, and fail health checks if enabled with no roots or the no-op fetcher.

### MEDIUM: Watcher backpressure silently drops filesystem events

Evidence: `crates/pcloud-fs/src/fs_watcher.rs:111`, `crates/pcloud-fs/src/fs_watcher.rs:121`, `crates/pcloud-fs/src/fs_watcher.rs:216`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:416`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:501`.

The notify callback uses a bounded channel and ignores `try_send` failure. No overflow event or forced full-rescan marker is emitted, so event storms can drop changes until the periodic full scan happens to repair state.

Remediation: handle `TrySendError::Full` by setting a durable root-level full-rescan-required flag and emitting metrics. Treat notify overflow/errors as immediate full-scan triggers.

### MEDIUM: Non-UTF-8 Unix paths are dropped or lossy-converted

Evidence: `crates/pcloud-fs/src/fs_watcher.rs:261`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:1073`.

Watcher paths use `to_str()?`, silently dropping invalid UTF-8. Full scans use `to_string_lossy()`, which can corrupt names or collide distinct byte paths via replacement characters.

Remediation: carry `OsString` or raw platform bytes through local scan and watcher models. If remote APIs require UTF-8, reject/quarantine invalid names explicitly with user-visible diagnostics instead of lossy conversion.

### MEDIUM: FUSE/platform parity is overstated by tests and stubs

Evidence: `crates/pcloud-fs/src/platform/mod.rs:20`, `crates/pcloud-fs/src/platform/bsd.rs:17`, `crates/pcloud-fs/src/mount_service.rs:258`, `crates/pcloud-fs/src/platform/windows.rs:207`, `crates/pcloud-fs/src/platform/windows.rs:1574`.

The platform table advertises broad parity, but BSD mount is validation-only/unsupported through `MountService`, Windows mountinfo is a placeholder, and Windows flush is a read-only MVP no-op. The test run skipped live FUSE coverage unless environment flags are set.

Remediation: narrow documented support tiers or implement the missing adapters. Add CI/live smoke coverage for each claimed platform, including write, flush, remount replay, and mount enumeration.

## Commands And Results

- `cargo test -p pcloud-engine -p pcloud-store -p pcloud-resilience --all-targets`: passed.
- `cargo test -p pcloud-fs --lib --tests`: passed; many live FUSE/E2E tests were ignored because required env flags were not set.
- `cargo test -p pcloud-daemon sync_loop_runtime`: passed.
- `cargo test -p pcloud-daemon integrity_sweeper --lib`: passed, including tests that demonstrate no-op/no-root sweeper behavior.
