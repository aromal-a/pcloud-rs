# Turn 3 Subagent 03 Audit: Sync/FUSE/Data Integrity

I read `pcloud_rev.md`, stayed read-only, and did not write `AUDIT_REPORT.md` or modify files. Verdict: **not enterprise-ready** for sync/FUSE/data integrity. Core sync planning, local upload execution, journal recovery, retry, and platform mount behavior have blocking correctness gaps.

## Findings

### 1. Critical: Local and remote changes are planned in separate queue-replacement passes
Evidence: `sync_one_root` polls remote diff, then runs local scan, then advances transfers (`crates/pcloud-daemon/src/sync_loop.rs:283`, `crates/pcloud-daemon/src/sync_loop.rs:291`, `crates/pcloud-daemon/src/sync_loop.rs:299`). Each ingest call replans only its current candidate batch and replaces scheduler work (`crates/pcloud-engine/src/lib.rs:501`, `crates/pcloud-engine/src/lib.rs:507`, `crates/pcloud-engine/src/lib.rs:511`). Conflict detection only happens when local and remote candidates are in the same planner batch (`crates/pcloud-engine/src/planner.rs:351`).
Impact: full-sync cycles can drop remote downloads when a later local scan replaces the queue, and local/remote same-path conflicts become one-sided upload/download operations.
Remediation: accumulate local and remote observations per root and call the planner once per root cycle; never replace remote work with a later empty/local-only scan.

### 2. Critical: Background local uploads do not read source files
Evidence: local scan records only metadata/path (`crates/pcloud-daemon/src/sync_loop_runtime.rs:1005`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:1068`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:1104`). `UploadFile` carries no local absolute path or payload handle (`crates/pcloud-engine/src/planner.rs:369`). Upload execution only looks in in-memory/staged maps (`crates/pcloud-daemon/src/sync_loop_runtime.rs:739`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:775`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:1367`).
Impact: ordinary files created under a sync root are planned for upload but fail as "missing staged upload payload"; core sync cannot reliably upload local changes.
Remediation: carry root-local path context into upload execution, open the file from disk with root-escape/symlink checks, stream it chunked with checksum/resume state, and avoid requiring prior memory staging.

### 3. Critical: FUSE write-journal replay only parses and logs records
Evidence: journal docs promise remount replay (`crates/pcloud-fs/src/write_journal.rs:1`). `WritePathService::replay_journal` only returns records (`crates/pcloud-fs/src/write_path.rs:1109`). FUSE `init` logs recovered records and continues, without applying or uploading them (`crates/pcloud-fs/src/fuser_shim.rs:222`). Daemon mount setup opens the journal and reconciles upload sidecars but does not apply journal ops (`crates/pcloud-daemon/src/mount_runtime.rs:968`).
Impact: writes acknowledged before crash/remount can remain stranded in staging while the mount advertises successful "recovery."
Remediation: implement a replay executor before serving kernel requests: validate blobs, rebuild dirty state, replay create/write/truncate/rename/unlink/barriers, upload or resume, and checkpoint only after backend ACK.

### 4. Critical: FUSE journal checkpointing is unsafe across multiple dirty files
Evidence: one `WritePathService` owns one journal for all inodes (`crates/pcloud-fs/src/write_path.rs:436`). Writes for any inode append to it (`crates/pcloud-fs/src/write_path.rs:596`). Flushing one inode resets the whole journal (`crates/pcloud-fs/src/write_path.rs:913`). Multiple dirty inodes are supported and drained independently (`crates/pcloud-fs/src/write_path.rs:1050`). Chunked flush records barriers/acks but returns without checkpointing the journal (`crates/pcloud-fs/src/write_path.rs:675`, `crates/pcloud-fs/src/write_path.rs:851`, `crates/pcloud-fs/src/write_path.rs:870`).
Impact: flushing file A can erase durable recovery records for dirty file B; chunked successful uploads can leave stale replay records forever.
Remediation: use per-inode journals or compaction that removes only records covered by a successful checkpoint; add crash tests with multiple dirty inodes and chunked uploads.

### 5. High: RetryLater work is classified but never requeued
Evidence: transient failures classify as `RetryLater` (`crates/pcloud-engine/src/recovery.rs:123`). `EngineShell::requeue_for_retry` exists and says callers should use it (`crates/pcloud-engine/src/lib.rs:849`). Runtime failure paths classify then only `mark_transfer_failed` (`crates/pcloud-daemon/src/sync_loop_runtime.rs:660`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:711`, `crates/pcloud-daemon/src/sync_loop_runtime.rs:821`). `rg requeue_for_retry` found only the definition/tests.
Impact: network blips park transfers in failed lists until restart, stalling sync during normal transient failures.
Remediation: on `RetryLater`, schedule bounded backoff, call `requeue_for_retry`, persist scheduler state, and clear stale failed entries.

### 6. High: Global transfer advancement is executed inside each root loop
Evidence: the cycle iterates roots (`crates/pcloud-daemon/src/sync_loop.rs:370`) and each root calls global `advance_transfers` plus direction-gated execution (`crates/pcloud-daemon/src/sync_loop.rs:299`). `advance_transfer_cycle` drains the global scheduler and replaces global coordinators (`crates/pcloud-engine/src/lib.rs:826`). Upload/download `accept_batch` clears previous active work (`crates/pcloud-engine/src/transfers/uploads.rs:48`, `crates/pcloud-engine/src/transfers/downloads.rs:48`).
Impact: multi-root configurations can dispatch work for root B while processing root A, skip it due to A's sync type, then clear it on the next root.
Remediation: ingest all roots first, then advance/execute global transfers once per cycle with operation-level sync-type checks, or make scheduler/coordinators root-scoped.

### 7. High: Platform mount write/orphan paths are not parity-ready
Evidence: BSD mount is explicitly validation-only and has no kernel mount path (`crates/pcloud-fs/src/platform/bsd.rs:44`), while the trait default `mount_adapter` returns unsupported (`crates/pcloud-fs/src/platform/mod.rs:90`). Windows write calls `adapter.write` (`crates/pcloud-fs/src/platform/windows.rs:1091`) but `cb_flush` is a no-op success (`crates/pcloud-fs/src/platform/windows.rs:1574`). Windows mountinfo reader always returns empty (`crates/pcloud-fs/src/platform/windows.rs:207`).
Impact: FreeBSD cannot provide mounted-drive service; Windows can report flush success without upload durability and cannot detect stale WinFSP mounts.
Remediation: implement BSD `mount_adapter`; wire Windows flush/cleanup/close to `flush_write`/`fsync_write`, implement WinFSP mount enumeration, and add live platform tests.

### 8. High: FUSE write handles break common read-after-create and large-file workflows
Evidence: `create` ignores `_flags` and returns a write handle with `read_handle: None` (`crates/pcloud-fs/src/fuser_shim.rs:444`, `crates/pcloud-fs/src/fuser_shim.rs:495`). `read` returns `EBADF` unless a read handle exists (`crates/pcloud-fs/src/fuser_shim.rs:386`). Writable open of an existing file reads the whole remote file into one `Vec` (`crates/pcloud-fs/src/fuser_shim.rs:331`, `crates/pcloud-fs/src/fuser_shim.rs:163`).
Impact: `open(O_CREAT|O_RDWR)`, write, seek, read can fail; opening large existing files for append/RW can OOM.
Remediation: honor access flags, serve staged reads for write-created handles, and replace whole-file seeding with disk-backed lazy copy-on-write/range fetch.

### 9. Medium: Filesystem watcher overflow silently loses events
Evidence: watcher channels are bounded and `try_send` errors are ignored (`crates/pcloud-fs/src/fs_watcher.rs:111`, `crates/pcloud-fs/src/fs_watcher.rs:129`). notify errors are only logged (`crates/pcloud-fs/src/fs_watcher.rs:133`). Runtime drains events without an overflow/resync marker (`crates/pcloud-daemon/src/sync_loop_runtime.rs:419`).
Impact: event storms can miss local changes until the next full scan, with no immediate forced reconciliation.
Remediation: emit an overflow event that forces a full scan next cycle and expose an overflow metric/audit event.

### 10. Medium: `BackupArchive` is advertised but rejected by persistence
Evidence: model encodes `BackupArchive` as `4` (`crates/pcloud-model/src/sync.rs:171`, `crates/pcloud-model/src/sync.rs:199`). CLI accepts `backup` aliases (`crates/pcloud-cli/src/app.rs:3176`). store schema allows only `1,2,3` (`crates/pcloud-store/src/schema.rs:125`), while sync graph saves `sync_type.as_u8()` (`crates/pcloud-store/src/repositories/sync_graph.rs:59`).
Impact: deletion-safe backup roots fail at persistence, undermining advertised data-retention behavior.
Remediation: migrate schema checks to include `4`, update v6/idempotent migration tests, and add CLI-to-store integration coverage.

### 11. Medium: Non-UTF-8 local paths are corrupted or dropped
Evidence: full scan uses `to_string_lossy()` for relative paths (`crates/pcloud-daemon/src/sync_loop_runtime.rs:1073`), while watcher conversion uses `to_str()?` and returns `None` for invalid UTF-8 (`crates/pcloud-fs/src/fs_watcher.rs:261`). scan entries store paths as `String` (`crates/pcloud-engine/src/local_scan.rs:36`).
Impact: valid Unix filenames with invalid UTF-8 can be silently skipped or uploaded under replacement-character names, causing collisions/data loss.
Remediation: preserve platform-native path bytes until an explicit remote-name encoding step, or reject/quarantine invalid names with visible audit diagnostics.

### 12. Medium: Cache staging back-pressure contract is ignored
Evidence: `StagingCache` says callers must not silently discard rejected payloads and that eviction is lossy (`crates/pcloud-cache/src/staging.rs:11`). `stage` returns `StagingResult` (`crates/pcloud-cache/src/staging.rs:102`), but `CacheShell::stage_file` ignores it (`crates/pcloud-cache/src/lib.rs:116`). Upload fallback later depends on cache staging (`crates/pcloud-daemon/src/sync_loop_runtime.rs:1382`).
Impact: staged upload bytes can be rejected or evicted with no durable fallback, later surfacing as missing payload.
Remediation: make `stage_file` return `StagingResult`, require callers to spill to disk-backed staging, and test eviction during pending uploads.

## Commands Run

Read-only commands included `sed -n '1,240p' pcloud_rev.md`, `git status --short`, scoped `find` over the requested crates/tests/benches, `rg -n` searches for retry/journal/FUSE/mount/watch/path patterns, and `nl -ba ... | sed -n ...` inspections of the referenced files. I excluded `target/`, `vendor/`, `.beads/`, `GPTREV/`, `CLAUDEREV/`, and generated tracker output from audit scope.

## Limitations

I did not run tests, formatters, live FUSE mounts, pCloud API calls, or Windows/macOS/BSD hardware checks. Several platform findings are source-level only and need live validation on the target OSes. The worktree was dirty with unrelated changes by the end of the audit; I did not modify files.
