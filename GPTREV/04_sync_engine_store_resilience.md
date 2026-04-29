# Subagent 04 Audit: Sync Engine, Store, Cache, Resilience

Scope was read-only. I did not modify files and did not write `AUDIT_REPORT.md`.

## Findings

### F-01 Critical: ordinary local-root uploads have no byte source
Severity: Critical.  
Evidence: `crates/pcloud-daemon/src/sync_loop_runtime.rs:1005` walks metadata only, `crates/pcloud-daemon/src/sync_loop_runtime.rs:1104` emits `deleted: false` entries with no content, and upload execution only reads staged buffers from filesystem/cache at `crates/pcloud-daemon/src/sync_loop_runtime.rs:739` and `crates/pcloud-daemon/src/sync_loop_runtime.rs:1367`. Missing payloads are explicitly treated as absent at `crates/pcloud-daemon/src/sync_loop_runtime.rs:1604`.  
Impact: A file created or modified directly under a sync root is planned as an upload but cannot be uploaded unless it was previously staged by FUSE/cache code. Bidirectional sync is functionally broken for normal filesystem edits.  
Remediation: Upload executor must stream bytes from `root.local_path + relative_path`, with canonical root containment checks, symlink/race protection, chunked upload, resume state, and an end-to-end test that creates a real local file and verifies uploaded bytes.

### F-02 Critical: remote diff is not root-scoped and corrupts multi-root sync
Severity: Critical.  
Evidence: `SyncRuntime::diff` accepts only auth/cursor/limit at `crates/pcloud-backends/src/sync_backend.rs:447`, but conversion hardcodes `SyncId::new(1)` at `crates/pcloud-backends/src/sync_backend.rs:509` and reduces paths to basename only at `crates/pcloud-backends/src/sync_backend.rs:543`. The daemon calls this per root at `crates/pcloud-daemon/src/sync_loop_runtime.rs:454`, while `DiffEntryMetadata` has parent IDs but no full path at `crates/pcloud-proto/src/sync_api.rs:109`.  
Impact: Remote changes for roots other than `sync_id=1` are misrouted; nested remote files collapse to basenames; per-root cursors can advance over an account-global stream and lose or duplicate events.  
Remediation: Maintain one account-level diff cursor and route entries to roots by folder ancestry, or implement true folder-scoped diff. Reconstruct sync-root-relative paths from `parent_folder_id` and metadata cache before planning.

### F-03 Critical: directory creates and deletes are planned but never executed
Severity: Critical.  
Evidence: Planner emits `CreateRemoteDirectory`, `CreateLocalDirectory`, `DeleteRemote`, and `DeleteLocal` at `crates/pcloud-engine/src/planner.rs:369`. Coordinators store them in pending lists at `crates/pcloud-engine/src/transfers/uploads.rs:48` and `crates/pcloud-engine/src/transfers/downloads.rs:48`. Runtime executors only match `DownloadFile` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:578` and `UploadFile` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:700`.  
Impact: Folder sync and deletion propagation do not converge. Deletes can be queued, cleared on the next batch, and never applied.  
Remediation: Add executor paths for local mkdir/delete and remote mkdir/delete, with idempotent "already exists/gone" handling, recursive delete policy, durable ack only after side effects, and tests for each planned operation type.

### F-04 High: local deletes are watcher-only and watcher overflow drops events silently
Severity: High.  
Evidence: Watcher channels are bounded at `crates/pcloud-fs/src/fs_watcher.rs:114` and `crates/pcloud-fs/src/fs_watcher.rs:123`, but callback overflow is ignored at `crates/pcloud-fs/src/fs_watcher.rs:129`. Full scans emit only existing entries with `deleted: false` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:1104` and `crates/pcloud-fs/src/fs_watcher.rs:381`. Delete candidates exist only when watcher remove events are converted at `crates/pcloud-fs/src/fs_watcher.rs:317`.  
Impact: Deletes that happen while the daemon is down, while watchers are not running, or during event storms are not detected later. This can leave remote data undeleted indefinitely.  
Remediation: Persist a local inventory per root and compare full scans against it. On channel overflow, set a per-root overflow flag and force reconciliation from persisted state.

### F-05 High: retry policy classifies transient failures but does not retry them
Severity: High.  
Evidence: Recovery docs say retryable network errors are re-armed by the scheduler at `crates/pcloud-engine/src/recovery.rs:9`, and the classifier returns `RetryLater` at `crates/pcloud-engine/src/recovery.rs:128`. Runtime failure paths classify then call `mark_transfer_failed` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:660` and `crates/pcloud-daemon/src/sync_loop_runtime.rs:820`. `mark_transfer_failed` only moves tasks into failed lists at `crates/pcloud-engine/src/lib.rs:796`.  
Impact: A transient network error can park work permanently until rediscovered by a later scan/diff, if ever.  
Remediation: Persist retry state with attempts and `next_ready_at`, requeue `RetryLater` operations with jittered backoff, and integrate global retry budget/circuit breaker state into transfer execution.

### F-06 High: planner grouping is not root-safe under multi-root overflow
Severity: High.  
Evidence: Planner sorts and groups candidates by `path` only at `crates/pcloud-engine/src/planner.rs:94` and keeps at most one local and one remote candidate per path at `crates/pcloud-engine/src/planner.rs:104`. Engine merges old overflow with fresh candidates at `crates/pcloud-engine/src/lib.rs:562`; if combined candidates span roots it falls back to full queue replacement at `crates/pcloud-engine/src/lib.rs:507`.  
Impact: Same relative paths across different roots can be collapsed, cross-root conflicted, or dropped, especially after planner overflow replay.  
Remediation: Key planning groups by `(sync_id, path)`, keep overflow partitioned per root, and never use full queue replacement for mixed-root replay.

### F-07 High: durable sync queue uses schemaless JSON and discards corruption
Severity: High.  
Evidence: Durable planner/scheduler state is JSON stored under `value_kv` keys at `crates/pcloud-daemon/src/sync_loop_runtime.rs:143`. Restore deletes corrupt values at `crates/pcloud-daemon/src/sync_loop_runtime.rs:238` and `crates/pcloud-daemon/src/sync_loop_runtime.rs:259`. Persistence serializes whole snapshots back to string values at `crates/pcloud-daemon/src/sync_loop_runtime.rs:323` and `crates/pcloud-daemon/src/sync_loop_runtime.rs:353`.  
Impact: A single corrupt JSON blob can discard all queued sync work. There is no schema version, per-root indexing, checksum, or quarantine path.  
Remediation: Replace with typed SQLite tables for queue and overflow rows, including schema version, operation kind, sync ID, path, attempt metadata, checksum, and repair/quarantine behavior.

### F-08 High: store migrations v5/v6 are not idempotent despite crash-safety claims
Severity: High.  
Evidence: Migration docs claim idempotent steps at `crates/pcloud-store/src/migrations.rs:68`. v5 blindly adds preference columns at `crates/pcloud-store/src/schema.rs:103`, and v6 blindly adds `sync_type` at `crates/pcloud-store/src/schema.rs:118`.  
Impact: A partial migration that adds a column but does not advance `user_version` will brick subsequent startup with duplicate-column errors.  
Remediation: Guard every `ALTER TABLE` with `column_exists` and wrap version updates transactionally. Add tests for "column exists, old user_version" for v5/v6.

### F-09 Medium: store directory and WAL/SHM sidecar permissions are not enforced
Severity: Medium.  
Evidence: Store docs promise a `0700` parent at `crates/pcloud-store/src/lib.rs:22`. Bootstrap only creates the parent, opens DB, chmods the main DB to `0600`, and enables WAL at `crates/pcloud-store/src/lib.rs:203`.  
Impact: With a permissive umask, the state directory and SQLite sidecars can expose metadata and queue/audit content even though the main DB file is tightened.  
Remediation: Enforce parent `0700` before opening SQLite, validate ownership, and chmod `store.sqlite3-wal`/`store.sqlite3-shm` after WAL creation.

### F-10 Medium: staging cache is byte-unbounded and eviction is lossy
Severity: Medium.  
Evidence: Staging stores `HashMap<String, Vec<u8>>` and is bounded only by file count at `crates/pcloud-cache/src/staging.rs:16`. Default max is 64 files at `crates/pcloud-cache/src/staging.rs:28`; eviction drops buffers at `crates/pcloud-cache/src/staging.rs:95`. A 50 MiB staged-file test is accepted at `crates/pcloud-daemon/src/sync_loop_runtime.rs:1447`.  
Impact: Large staged writes can exhaust memory, and eviction can drop the only upload payload.  
Remediation: Add a byte budget, return back-pressure errors, move dirty upload staging to disk, and prevent eviction of non-durable payloads.

### F-11 Medium: conflict resolution is not executable or root-safe
Severity: Medium.  
Evidence: `resolve_conflicts` only returns decisions and does not mutate scheduler state at `crates/pcloud-engine/src/lib.rs:703`. `resolve_conflict_by_path` matches only by path at `crates/pcloud-engine/src/lib.rs:724`. Prefer-remote emits `DownloadFile` with `remote_file_id: None` at `crates/pcloud-engine/src/conflict_resolver.rs:217`, but downloader executes only `Some(file_id)` at `crates/pcloud-daemon/src/sync_loop_runtime.rs:578`.  
Impact: Operators can resolve the wrong root's conflict, and some resolutions cannot execute.  
Remediation: Persist conflicts with `(sync_id, path, remote IDs, local metadata)`, resolve by conflict ID, and enqueue concrete operations with required file IDs.

### F-12 Medium: integrity sweeper is not production-wired
Severity: Medium.  
Evidence: Bootstrap constructs the shell at `crates/pcloud-daemon/src/bootstrap.rs:836`, but `RuntimeShell::bootstrap_integrity_sweeper` is a no-op at `crates/pcloud-daemon/src/runtime.rs:6783`. Enabled shells default to empty roots and `NoOpChecksumFetcher` at `crates/pcloud-daemon/src/integrity_sweeper_service.rs:738`; `NoOpChecksumFetcher` reports every remote file missing at `crates/pcloud-daemon/src/integrity_sweeper_service.rs:198`. Runtime `integrity_run_once` runs synchronously at `crates/pcloud-daemon/src/runtime.rs:6432`.  
Impact: An enabled sweeper can report enabled while doing no useful production verification, or run only on manually injected test roots/fetchers. Audit events can be dropped if the worker is not spawned.  
Remediation: Wire worker/scheduler startup, populate roots from sync graph, install a real checksum fetcher from authenticated backend state, and fail visibly when enabled but unwired.

### F-13 Medium: pause/remove sync-root runtime semantics leave stale or delayed work
Severity: Medium.  
Evidence: Global pause sets flags but does not wake the loop at `crates/pcloud-daemon/src/runtime.rs:3680` and `crates/pcloud-daemon/src/sync_loop.rs:152`; per-root pause also lacks wake at `crates/pcloud-daemon/src/runtime.rs:5834`, while resume does wake at `crates/pcloud-daemon/src/runtime.rs:5921`. Sync-root remove clears only the IPC runtime cache by absolute prefix at `crates/pcloud-daemon/src/runtime.rs:5797`, but the sync loop owns separate relative-keyed cache/filesystem state at `crates/pcloud-daemon/src/sync_loop_runtime.rs:296`, and `evict_removed_root` does not clear it at `crates/pcloud-daemon/src/sync_loop_runtime.rs:865`.  
Impact: Pause may take effect only after the poll interval, and stale staged bytes can survive root removal in the sync-loop runtime.  
Remediation: Wake and cooperatively cancel/drain on pause, namespace caches by `sync_id`, and clear sync-loop cache/filesystem staging during root eviction.

### F-14 Low: case-insensitive filesystem handling is advisory and apparently unused
Severity: Low.  
Evidence: The helper warns that case-conflicting remote files are not handled correctly at `crates/pcloud-engine/src/lib.rs:161`, and `rg` found no call outside its definition.  
Impact: macOS/Windows/default case-folding roots can collide paths such as `README` and `Readme`, causing overwrites or stuck conflicts.  
Remediation: Probe during sync-root add and either reject case-insensitive roots or enable a casefold-aware conflict model.

### F-15 Low: runtime/sweeper still carry explicit panic-hardening debt
Severity: Low.  
Evidence: `sync_loop_runtime.rs` declares about 91 non-test unwrap/expect sites at `crates/pcloud-daemon/src/sync_loop_runtime.rs:1`; `integrity_sweeper_service.rs` declares about 50 at `crates/pcloud-daemon/src/integrity_sweeper_service.rs:1`, and thread spawn still uses `expect` at `crates/pcloud-daemon/src/integrity_sweeper_service.rs:815`.  
Impact: Background runtime or sweeper failures can become panics instead of supervised errors.  
Remediation: Replace production unwrap/expect sites with typed errors, supervisor-visible status, and restart/fail-closed behavior.

## Commands Run

Read-only command groups used:

```sh
sed -n '1,220p' pcloud_rev.md
sed -n '221,520p' pcloud_rev.md
rg --files crates/pcloud-engine crates/pcloud-store crates/pcloud-cache crates/pcloud-resilience crates/pcloud-backends crates/pcloud-daemon
nl -ba <scoped files> | sed -n '<ranges>'
rg -n "set_sweep_roots|set_checksum_fetcher|bootstrap_integrity_sweeper|NoOpChecksumFetcher|run_once" crates/pcloud-daemon/src crates/pcloud-fs/src
rg -n "GlobalRetryBudget|CircuitBreaker|RetryPolicy|retry_budget|circuit" crates/pcloud-daemon/src crates/pcloud-backends/src crates/pcloud-engine/src crates/pcloud-resilience/src
rg -n "TODO|FIXME|STUB|placeholder|unimplemented!|todo!|panic!|unwrap\\(|expect\\(" <scoped paths>
rg -n "sync loop|sync state machine|bidirectional|sweeper|parity" STATUS.md C_FEATURE_PARITY_MATRIX.csv C_FEATURE_PARITY_REVIEW.md README.md docs/book
git status --short
```

## Limitations

No tests or `cargo check` were run because the lead override prohibited file modifications and Cargo would write build artifacts under `target/`. The worktree was already dirty before this audit (`git status --short` showed existing modified/untracked files, including `AUDIT_REPORT.md`), and I did not change it. Static review excluded `target/`, `vendor/`, `.beads/`, and generated tracker output.
