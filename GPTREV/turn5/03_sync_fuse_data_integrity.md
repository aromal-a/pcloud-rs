# Turn 5 Sync / FUSE / Data Integrity Review

Review-only; no files edited. Scope was taken from `pcloud_rev.md` and focused on the current dirty tree after Turn 4.

## Findings

### CRITICAL - FUSE journal replay still only logs records

Evidence: `crates/pcloud-fs/src/write_journal.rs:1` promises replay of unflushed records, but `crates/pcloud-fs/src/write_path.rs:1102` only returns parsed records. `crates/pcloud-fs/src/fuser_shim.rs:222` calls `replay_journal()`, logs record count or failure, then returns `Ok(())`; it never replays create/write/truncate/unlink/rename/upload work. `crates/pcloud-daemon/src/mount_runtime.rs:970` reconciles upload sidecars before constructing the writer, but there is no equivalent executor for `journal.log`.

Impact: a remount can accept new writes while previously journaled mutations remain stranded in staging, so acknowledged FUSE writes can be lost or later overwritten.

Remediation: add a pre-mount replay executor that replays records in sequence, validates staging blobs, re-drives backend create/write/truncate/unlink/rename/upload/save operations idempotently, checkpoints only after backend acknowledgement, and fails closed or mounts read-only if replay cannot complete.

### HIGH - `O_TRUNC` mutates staging before any journal record exists

Evidence: Linux FUSE open skips existing content when truncation is requested at `crates/pcloud-fs/src/fuser_shim.rs:334`, seeds an empty blob at `crates/pcloud-fs/src/fuser_shim.rs:356`, then calls `open_for_write(..., trunc)` at `crates/pcloud-fs/src/fuser_shim.rs:360`. `open_for_write` directly truncates staging at `crates/pcloud-fs/src/write_path.rs:485`, while the journaled truncate path only exists in `WritePathService::truncate` at `crates/pcloud-fs/src/write_path.rs:951`.

Impact: crash after `open(O_TRUNC)` succeeds but before a later write/flush leaves no replayable truncate record. Even a future replay executor could not reconstruct the acknowledged zero-length state.

Remediation: journal `JournalOp::Truncate { new_size: 0 }` before any `O_TRUNC` staging mutation, or route open-truncate through the existing journaled truncate path before returning success. Add crash tests for open-truncate-close without subsequent writes.

### HIGH - WinFSP flush/close does not flush dirty writes

Evidence: Windows writes stage data through `adapter.write` at `crates/pcloud-fs/src/platform/windows.rs:1199`, but `cb_flush` ignores the file context and returns success as a no-op at `crates/pcloud-fs/src/platform/windows.rs:1630`. `cb_close` only releases a cached read handle at `crates/pcloud-fs/src/platform/windows.rs:1542`. Small writes are otherwise flushed only if size or time triggers fire during `WritePathService::write` at `crates/pcloud-fs/src/write_path.rs:620`.

Impact: normal Windows write-close or write-flush can report success while content remains only in staging until threshold flush or unmount drain, breaking mounted-drive parity and durability expectations.

Remediation: make `cb_flush` and dirty-file close/cleanup call `adapter.flush_write(ctx.ino)` or a real handle-based flush, propagate failures to WinFSP, and add parity tests for write-close-upload and flush failure propagation.

### HIGH - Planned folder create/delete operations are never executed

Evidence: the planner emits `CreateRemoteDirectory`, `CreateLocalDirectory`, `DeleteRemote`, and `DeleteLocal` at `crates/pcloud-engine/src/planner.rs:381` and `crates/pcloud-engine/src/planner.rs:394`. Coordinators store them in pending lists at `crates/pcloud-engine/src/transfers/uploads.rs:58` and `crates/pcloud-engine/src/transfers/downloads.rs:58`. The daemon execution loops only clone `active_downloads` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:655` and `active_uploads` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:772`; no loop processes those pending delete/directory lists. The pending lists are still included in active counts at `crates/pcloud-engine/src/transfers/uploads.rs:84` and `crates/pcloud-engine/src/transfers/downloads.rs:84`.

Impact: sync roots do not converge for folder creation or deletes; status can show pending work that has no executor.

Remediation: add execution phases for remote/local mkdir and delete operations, with idempotent already-exists/already-missing handling, path containment checks, delete policy enforcement, scoped acknowledgements, and tests for all four planned operation kinds.

### HIGH - Sync uploads still use the non-idempotent single-shot upload path

Evidence: `execute_uploads` creates a session at `crates/pcloud-daemon/src/sync_loop_runtime.rs:850` and calls `transfer_runtime.upload_bytes` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:865` and `:871`. The network single-shot implementation sends `UploadWriteRequest` with `idempotency_key: None` at `crates/pcloud-backends/src/transfer_backend.rs:670` and `UploadSaveRequest` with `idempotency_key: None` at `crates/pcloud-backends/src/transfer_backend.rs:696`. A resumable state-machine path exists at `crates/pcloud-backends/src/transfer_backend.rs:847`, but this sync path does not use it.

Impact: retry after ambiguous network failure can duplicate or corrupt server-side upload state, and large uploads cannot resume from durable offsets.

Remediation: route sync uploads through the resumable upload state machine with stable idempotency keys, persisted resume rows, checksum verification before save, and retry-safe save semantics.

### HIGH - Local upload fallback is unbounded and validates only length/mtime

Evidence: local upload payloads are stored as `Vec<u8>` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:1437`, read with `read_to_end` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:1519`, then uploaded as one contiguous slice at `crates/pcloud-daemon/src/sync_loop_runtime.rs:871`. The snapshot records only path, length, and modified time at `crates/pcloud-daemon/src/sync_loop_runtime.rs:1443`, and post-upload validation compares only length and mtime at `crates/pcloud-daemon/src/sync_loop_runtime.rs:1561`. Validation happens after `upload_bytes` has already performed write and save at `crates/pcloud-daemon/src/sync_loop_runtime.rs:887`.

Impact: large ordinary sync-root files can spike daemon RSS, and a same-size/same-mtime replacement can be marked complete with stale bytes already saved remotely.

Remediation: stream from a safely opened file descriptor into chunked upload, bound memory by chunk size, track stable file identity where available, hash uploaded bytes, validate before final save, and requeue changed files without committing stale remote state.

### HIGH - Integrity sweeper still has no production checksum fetcher

Evidence: runtime bootstrap configures sweep roots at `crates/pcloud-daemon/src/runtime.rs:6829` but does not install a real fetcher before spawning the worker. The default fetcher is `NoOpChecksumFetcher` at `crates/pcloud-daemon/src/integrity_sweeper_service.rs:204`, and readiness fails when the fetcher is still noop at `crates/pcloud-daemon/src/integrity_sweeper_service.rs:870`. The setter exists at `crates/pcloud-daemon/src/integrity_sweeper_service.rs:838`, but the production bootstrap shown does not call it.

Impact: Turn 4 changed this from false success to fail-closed, but enabled production integrity verification still cannot run.

Remediation: implement and inject an authenticated remote checksum/stat fetcher, refresh it with session changes, expose unhealthy status while absent, and add integration tests for match, mismatch, missing remote, and fetch failure paths.

### MEDIUM - Retry requeue has no backoff, budget, or fairness control

Evidence: recovery docs say retryable failures are re-armed after backoff at `crates/pcloud-engine/src/recovery.rs:9`, and that backoff sequencing belongs in scheduler/coordinators at `crates/pcloud-engine/src/recovery.rs:23`. Current runtime immediately calls `requeue_for_retry` for `RetryNow`/`RetryLater` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:470`. `requeue_for_retry` inserts the operation at the front of the scheduler queue at `crates/pcloud-engine/src/lib.rs:856` and `crates/pcloud-engine/src/lib.rs:889`.

Impact: transient failures, 429s, or network outages can hammer the API and starve fresh work.

Remediation: persist per-operation retry metadata with attempt count, `next_attempt_at`, jittered exponential backoff, retry budget, and fair scheduling. Do not dispatch retryable work until due.

### MEDIUM - Non-UTF local paths are still lossy or silently dropped

Evidence: full local scan converts relative paths with `to_string_lossy()` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:1156` and leaf names at `crates/pcloud-daemon/src/sync_loop_runtime.rs:1185`. Watcher relative conversion uses `rel.to_str()?` at `crates/pcloud-fs/src/fs_watcher.rs:290`, dropping invalid UTF-8 paths. Integrity sweep remote mapping also uses `to_string_lossy()` at `crates/pcloud-daemon/src/integrity_sweeper_service.rs:1374`.

Impact: byte-distinct Unix filenames can collapse or disappear from sync and integrity checks.

Remediation: carry `OsString` or raw platform path bytes through scan/watch planning, reject or quarantine names only at the remote API boundary, and add Unix tests for invalid UTF-8 and U+FFFD collision cases.

## Commands / Results

- `sed -n '1,240p' pcloud_rev.md`: read master prompt.
- `git status --short`: confirmed dirty tree; no source files edited by this review.
- `git diff --stat`: dirty tree currently spans 130 files, 4265 insertions, 1824 deletions.
- `cargo test -p pcloud-fs --lib write_path --locked`: passed, 38 tests.
- `cargo test -p pcloud-daemon --lib sync_loop_runtime --locked`: passed, 24 tests; emitted existing vendored password dictionary warning from `pcloud-crypto`.
- `cargo test -p pcloud-daemon --lib integrity_sweeper --locked`: passed, 23 tests; same warning.
- `cargo test -p pcloud-engine --lib local_scan --locked`: passed, 14 tests.

Full workspace, live FUSE, WinFSP, and macOS mount tests were not run in this review pass.
