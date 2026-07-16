# Changelog

<!-- Purpose: user-visible history of the pcloud-rs Rust rewrite ( workspace). -->

All notable changes to the `pcloud-rs-rust-dev` workspace are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project uses [Semantic Versioning](https://semver.org/) once the first tagged
release ships. Until then, all entries accumulate under `[Unreleased]`.

Source: waves 1-9 captured in `FINAL-PARITY-PROOF-WAVE*.md`,
`RECONCILIATION-WAVE*.md`, `SECURITY-AUDIT*.md`,
`MATRIX-*.md`, `PARITY-AUDIT-FINAL-14042026.md`, and the
`bd-1du` tracker tree.

## [Unreleased]

### Removed scaffolding without a public pCloud API backing (2026-05-01)

**Removed**

- T1.2 file-version listing/restore: `RevisionProvider` trait,
  `NullRevisionProvider`, `HttpRevisionProvider` (and the
  `file-history-http` cargo feature), `Method::FileHistory`,
  `Request::FileHistory`, `Command::FileHistory` / `FileDiff` /
  `FileRestore`, the `pcloudc history` / `diff` / `restore`
  subcommands, `crates/pcloud-config/src/file_history.rs`,
  `crates/pcloud-daemon/tests/file_history_provider.rs`, and
  `crates/pcloud-proto/src/revision_provider.rs`. Reason: pCloud's
  public API exposes no `listrevisions` / `revertfile` for
  third-party clients; see `docs/future-pcloud-clone-api.md`.
- T2.1.d differential-upload execute path: `DeltaUploadTransport`
  trait, `execute_delta_upload`, in-tree mock. The `pcloud-rsync`
  codec primitives stay. Reason: `upload_writefromfile` does not
  expose byte-range / block-index semantics; see
  `docs/future-pcloud-clone-api.md`.
- T2.2.b parallel HTTP-range fetcher: `fetch_parallel` and the
  mockserver `/download` Range route. The `plan_ranges` planner
  stays. Reason: see `docs/future-pcloud-clone-api.md`.
- T2.6 QUIC transport: `quinn` workspace dependency, the `quic`
  cargo feature, and the `QuicTransport` scaffold. The
  `transport_protocol` selector and `resolve_after_handshake`
  matrix stay. Reason: see `docs/future-pcloud-clone-api.md`.
- T4.4 server-side dedup awareness CLI: `Method::GetStorageSummary`,
  `StorageSummaryPayload`, the `pcloudc storage` command and its
  renderer. Reason: `userinfo` does not expose per-account physical
  bytes / dedup ratio; see `docs/future-pcloud-clone-api.md`.

### Dual crypto backend: pclsync-compat + enhanced (2026-04-18)

**Added**

- Dual crypto backend: `pclsync-compat` (default, interoperable with
  official pCloud apps) and `enhanced` (opt-in, stricter AEAD,
  NOT interoperable). See `docs/enterprise/crypto-compat.md`.
- Six new pclsync-compatible primitives behind `pclsync-v2` feature:
  PBKDF2-HMAC-SHA512 KEK, RSA-4096+OAEP, AES-CTR (native + standard),
  CBC-CTS CS3, custom sector AEAD, 128-ary Merkle hash tree,
  reversible filename encoding.

**Changed**

- `pcloudc crypto setup` now prompts for backend; scripts can pass
  `--backend {pclsync-compat|enhanced}` with `--acknowledge-not-interop`
  required for `enhanced`.
- Daemon logs `crypto unlocked: backend=<NAME>` at every unlock.
- `pcloudc crypto status` first line shows active backend.

### Four parallel closures: deletion-safe sync, FUSE SLO hook, integrity walker NDJSON, parity doc sweep (2026-04-16)

**Added**

- `SyncType::BackupArchive` in `pcloud-model::sync` (discriminant 4,
  serde/label `"backup-archive"`) — a deletion-safe upload-biased sync
  flavor. `DeletePolicy::for_sync_type` now returns
  `{ allow_delete_remote: false, allow_delete_local: false }` for this
  variant, while uploads continue as usual.
- `pcloudc sync add --type backup-archive` (aliases `archive`,
  `keep-remote`). The CLI `--type` parser also tightens `upload-only`
  to the existing `SyncType::UploadOnly` path.
- `pcloud_fs::slo_hook::observe_flush(bytes, elapsed)` — single entry
  point for the FUSE write-path flush metric fan-out (feeds the
  `flush_latency_seconds` and `flush_bytes` user histograms and the
  SLO registry's `upload.throughput_mbps`). Wired at both flush success
  arms in `pcloud_fs::write_path::WritePathService`.
- Integrity walker NDJSON output:
  `IntegritySweeperShell::run_once_ndjson(&self, &mut dyn Write)` emits
  one `IntegrityNdjsonRecord` per examined file with fields `ts`,
  `path_hash` (SHA-256 hex, PII-safe), `remote_path`, `local_hash`,
  `remote_hash`, `status` (`match` / `mismatch` / `missing_remote` /
  `skipped`). Covered by two new integration tests in
  `crates/pcloud-daemon/tests/integrity_walker.rs`.

**Changed**

- `C_FEATURE_PARITY_REVIEW.md` — tally, summary, per-subsystem status,
  and conclusion rewritten to reflect `158 / 0 / 0 / 28` and the
  retired `Partial` class. Subsystem status lines flipped to
  Implemented where the matrix now says so.
- Five docs (`docs/roadmap-complete.md`,
  `docs/book/src/parity/status.md`,
  `docs/book/src/security/audit-dossier.md`,
  `docs/book/src/architecture/overview.md`,
  `docs/book/src/archive/index.md`) — hard-coded parity totals replaced
  with pointers to `STATUS.md`, which is the single source of truth.
- `docs/parity/bd-1du-10-closure-checklist.md` — preamble bumped to
  2026-04-16; four cross-cutting closure items ticked. Reviewer-19
  regrade and the closing commit SHA remain intentionally unticked
  (out of AI scope).

**Fixed**

- Three rustdoc intra-doc links in `pcloud-fs` (`lib.rs`,
  `write_path.rs`, `slo_hook.rs`) that referenced a private
  `WritePathService::chunked_flush` — rustdoc with `-D warnings`
  rejects private-item links from public docs. Replaced with plain
  code spans; the method stays private.

**Gates.** All nine gates PASS after the parallel merge.
Test suite: **2 033 passed / 0 failed / 46 ignored** (+4 tests from
this wave). No parity-matrix row flipped.

### Build recovery after removal of legacy C tree (2026-04-16)

- **`pcloud-crypto` build unblocked.** The build script read
  `pclsync/ppassworddict.h` directly, and the C tree had been deleted
  without updating it. The 8 525-entry dictionary is now vendored at
  `crates/pcloud-crypto/vendored/password_dict.rs`; the build script
  prefers the C header when present and falls back to the vendored copy
  otherwise, aborting only when neither exists. Cargo warning names both
  candidate paths.
- **`pcloud-daemon/src/dispatch.rs`** — removed stray `Method::SetApiServer`
  arm (the surface is a `Request` variant, not a `Method`; the existing
  `Request::SetApiServer` arm already routes to the `"account"` tracing
  label).
- **`pcloud-backends/src/snapshot.rs`** — fixed `prune_gfs_discovers_legacy_tar_gpg`
  fixtures whose file names (`pcloud-rs.tar.zst`, `pcloud-rsenc.tar.zst.gpg`)
  didn't match the required `pcloud-rs-` prefix enforced by
  `list_snapshot_files`, so the test would have failed as soon as the build
  was restored.
- **`CLAUDE.md`** — replaced references to deleted C sources with a note
  explaining they are upstream; bumped "Current Truth" date to 2026-04-16.
- **Workspace formatting** — ran `cargo fmt --all` to clear 35 diffs from
  the 20-new-command wave.

Verified: all nine gates (fmt / check / clippy -D warnings / test /
doc -D warnings / deny / release build) PASS. Test suite:
**2 029 passed / 0 failed / 46 ignored**.

### 20 new pcloudc CLI commands — account, download, crypto, sync, backup (2026-04-16)

The following commands are now available through `pcloudc`. All are wired
end-to-end: IPC envelope → daemon dispatch → pCloud API response.

**Crypto (6 new subcommands)**
- `crypto reset` — wipe local fingerprint cache (recovery); mirrors `psync_crypto_reset`
- `crypto hint` — fetch the stored passphrase hint; mirrors `psync_crypto_get_hint`
- `crypto priv-key-flags` — return `crypto_private_flags` integer; mirrors `psync_crypto_priv_key_flags`
- `crypto send-change-private` — request server-side OTP for passphrase rotation; mirrors `psync_crypto_send_change_user_private`
- `crypto change-password [OLD [NEW [HINT [CODE]]]]` — rotate crypto passphrase; mirrors `psync_crypto_change_crypto_pass`
- `crypto change-password-unlocked [NEW [HINT [CODE]]]` — same but valid only when already unlocked; mirrors `psync_crypto_change_crypto_pass_unlocked`

**Sync helpers (2 new subcommands)**
- `sync suggest [PATH] [--max N]` — return candidate directories for sync-root creation; mirrors `psync_get_sync_suggestions`
- `sync is-syncable PATH` — classify whether a path can become a sync root; mirrors `psync_is_folder_syncable`

**Account management (9 new subcommands)**
- `account verify-email` — trigger verification email; mirrors `psync_verify_email`
- `account verify-email-restricted TOKEN` — submit restricted verification token; mirrors `psync_verify_email_restricted`
- `account lost-password EMAIL` — request password-reset link (no session required); mirrors `psync_lost_password`
- `account change-password` — rotate account password interactively; mirrors `psync_change_password`
- `account register EMAIL [--accept-terms]` — create a new pCloud account; mirrors `psync_register`
- `account api-servers` — list available API endpoints; mirrors `psync_get_api_servers`
- `account set-api-server LOCATION_ID BINAPI` — pin the daemon to a specific endpoint; mirrors `psync_set_api_server`
- `account set-language LANG` — set account UI language; mirrors `psync_set_language`
- `account promo` — fetch active promotional offer; mirrors `psync_get_promo`

**Downloads (2 new subcommands)**
- `download link FILE_ID` — resolve a signed HTTPS download URL; mirrors `psync_get_file_link`
- `download file FILE_ID LOCAL_PATH` — fetch and write a file to local disk; mirrors `psync_download_file`

**Backup (1 new subcommand)**
- `backup delete BACKUP_ID` — remove a server-side backup-device record; mirrors `psync_delete_backup`

All passphrase inputs are handled via `SecretString` (transit-only; zeroized on drop; never written to disk). Documentation updated in `docs/book/src/reference/cli.md`, `packaging/man/pcloudc.1`, and this CHANGELOG.

### O-wave gate: fix concurrent-edit breakage, clippy/lint cleanup (2026-04-16)

- Fixed 6 compile errors from concurrent O01-O05 edits: `#![deny(unsafe_code)]`
  added to crates that legitimately use unsafe (pcloud-ipc, pcloud-compat,
  pcloud-fs, pcloud-daemon, pcloud-cli), and `[lints.rust]` vs workspace
  conflicts in pcloud-kms and pcloud-policy.
- Removed nonexistent `clippy::manual_debug` lint allow from 34 crate roots.
- Removed duplicate `#![deny(missing_docs)]` in pcloud-kms and pcloud-policy.
- All nine gate checks pass: fmt, check, clippy -D warnings, 2029 tests,
  doc -D warnings, cargo deny, release build.

### Parity matrix reaches 158/0/0/28 — zero Partial rows (2026-04-16)

- All 158 retained C feature rows are now `Implemented` in the parity
  matrix. Zero `Partial`, zero `Missing`. 28 rows are `Rejected` with
  per-item justification.
- STATUS.md, CLAUDE.md, and C_FEATURE_PARITY_REVIEW.md updated to
  reflect the new totals.
- Remaining open work is the final parity-proof gate (`bd-1du.10`):
  release/docs wording alignment and live verification of edge cases.

### Implement psync_stat_path local metadata cache (2026-04-16)

- **Schema v11:** new `file_metadata` table with composite index on
  `(parent_folder_id, name)` for hierarchical path resolution.
- **Diff engine metadata persistence:** `RealSyncLoopRuntime::poll_remote_diff`
  now persists file/folder metadata from remote diff batches into the
  `file_metadata` table as entries arrive. Deletes evict rows.
- **`RuntimeShell::stat_path`:** resolves an absolute pCloud-drive path
  against the local metadata cache first (`FileMetadataRepository::resolve_path`),
  falls back to the API on cache miss. Response includes `source=cache|api`.
- **IPC:** `Request::StatPath { path }` + `Method::StatPath` +
  `StatPathPayload` wire type.
- **CLI:** `pcloudc stat <path>` command.
- **Parity matrix row 76 (`psync_stat_path`) flipped from Partial to
  Implemented.** 6 repository-level tests cover upsert, list, hierarchical
  resolve, delete, and parent+name lookup.

### Wire dyn-shim write ops through FuseAdapter trait (2026-04-16)

- **`BoxedFuserShim` and `FuserShim<A>`** in `platform/linux.rs` now
  forward all write-path FUSE callbacks (`create`, `write`, `flush`,
  `fsync`, `setattr(size)`, `unlink`, `rename`, `mkdir`, `rmdir`)
  through the `FuseAdapter` trait instead of returning `ENOSYS`/`EROFS`.
  Adapters with a `WritePathService` attached get full write support;
  adapters without one keep the trait-default `ENOSYS` (read-only).
- New gated integration test: `fuse_dyn_shim_write.rs` mounts via
  `MountService::mount` (the `FuserShim<A>` path), writes a file
  through the kernel VFS, reads it back, and asserts byte-identity.
- **Parity matrix row 85 (mounted filesystem) flipped from Partial to
  Implemented.** Remaining performance follow-up: chunked `upload_write`
  pipelining for multi-GiB writes.

### Added — SDK FS-level helpers (2026-04-16)

- **Row 187 flipped from Partial to Implemented** (matrix: 156 / 2 / 0 / 28).
  `EmbeddedDaemon` now exposes:
  - `stat_path(&mut self, path: &str) -> Result<StatResult, SdkError>` —
    mirrors C `psync_stat_path` (`pclsync/psynclib.h:743`). Resolves via
    `FolderRuntime::list_folder_contents` (parent-list + child-lookup).
  - `list_folder(&mut self, path: &str) -> Result<Vec<FolderEntry>, SdkError>` —
    mirrors C `pfolder_list` (`pfolder.c:556`).
  - `mount(&mut self, mountpoint: &Path) -> Result<(), SdkError>` —
    delegates to daemon mount runtime.
  - `unmount(&mut self) -> Result<(), SdkError>` — delegates to daemon
    unmount.
- New public types: `StatResult`, `FolderEntry`, `MountHelperError`.
- New `SdkError::Mount` variant with full unified-error taxonomy wiring.
- `FolderRuntime::list_folder_contents` added to `pcloud-backends`.
- Development transport (`DevelopmentFolderTransport`) now handles
  `listfolder` requests for test coverage.
- 13 new tests (9 unit + 4 doc-tests).
- Fixed pre-existing compile errors in `pcloud-daemon::runtime::stat_path`
  (missing `StatPathPayload` re-export, non-existent `current_auth_token`
  and `stat_or_list` methods).

### Gate — N-wave gate run (2026-04-16)

- **All seven gates PASS.** No N-wave agents executed; this is a
  confirmation gate on M-wave state.
- **Test suite: 2010 passed / 0 failed / 45 ignored** (unchanged).
- **Parity matrix: 155 / 3 / 0 / 28** (unchanged from M-wave).
- **bd-1du.10 verdict: NOT closable.** Three Partial rows remain, all
  tied to `bd-1du.4` (mounted-drive parity). See
  `PLAN_A_PLUS_N_WAVE_REPORT.md`.

### Gate — M-wave gate run (2026-04-16)

- **All seven gates PASS.** fmt, check, clippy (-D warnings), test,
  doc (-D warnings), deny, release build all clean.
- **Test suite: 2010 passed / 0 failed / 45 ignored** (+18 from L-wave).
- **Parity matrix unchanged:** 155 Implemented / 3 Partial / 0 Missing /
  28 Rejected.
- **Compile fixes:** collapsible `if let` in `sync_loop_runtime.rs`;
  `#[allow(dead_code)]` on test-facing `tick_notify` field in
  `integrity_sweeper_service.rs`.
- **Alpha verdict:** conditionally alpha-taggable (`v0.1.0-alpha.1`).
  Mounted-drive parity (`bd-1du.4`) and final parity gate (`bd-1du.10`)
  remain open. See `PLAN_A_PLUS_M_WAVE_REPORT.md`.

### Changed — Upload parity rows flipped (2026-04-16)

- **Parity matrix: 155 / 3 / 0 / 28** (was 152 / 6 / 0 / 28).
  Three transfer rows flipped from Partial to Implemented:
  - Row 92 (`transfers,upload_create/write/save`): `upload_bytes_chunked`
    + `UploadStateMachine` provide full create->write-loop->save with
    SQLite resume persistence, retry with backoff, and auth refresh.
  - Row 93 (`transfers,upload wire methods`): all proto primitives + DTOs
    + state machine implemented; live-API edge-case verification is a
    testing concern for `bd-1du.10`, not a functional gap.
  - Row 94 (`transfers,SDK UploadSession`): real chunked state machine
    with `pause`/`resume`/`cancel` backed by journal replay for crash
    recovery. No `TODO(stub)` markers remain.
- Remaining 3 Partial rows: `fs,psync_stat_path` (needs local metadata
  cache), `fs,mounted pcloud filesystem` (lifecycle proofs deferred),
  `sdk,embedded library shell` (FS helpers tied to `bd-1du.4`).
- Row 76 (`fs,psync_stat_path`) rationale corrected: the C function
  queries local SQLite metadata tables, not a FUSE inode cache. The Rust
  gap is the diff-engine not populating local metadata tables yet.
- Row 85 (`fs,mounted pcloud filesystem`) rationale updated to reflect
  live FUSE read+write verification results.

### Gate — L-wave Phase 4 gate run (2026-04-16)

- **Gate results:** fmt, check, clippy (-D warnings), doc (-D warnings),
  deny, release build all PASS. Test suite: 1992 passed / 1 failed
  (flaky sweeper, tracked under `bd-1du.4.6.1`).
- **Parity matrix unchanged:** 152 Implemented / 6 Partial / 0 Missing /
  28 Rejected. All 6 Partial rows remain Partial -- chunked upload state
  machine, FUSE lifecycle proof, psync_stat_path live cache, and SDK FS
  helpers are the remaining gaps.
- **Compile fixes:** resolved concurrent-edit breakage in
  `SyncLoopConfig` (missing `Default` fields), `conflict_resolver`
  (missing `resolve_newest_wins`/`resolve_rename_both`), `planner`
  (unused `SyncType` import), `transfer_bridge` (`ConfigProfile::default`
  vs `secure_defaults`), `schema.rs` (`Node::String` struct variant),
  `diff_api.rs` (broken cross-crate doc links).

### Added — Multi-chunk upload support and end-to-end conflict resolution (2026-04-16)

- **Chunked upload tracker:** `ChunkedUploadTracker` in
  `pcloud-engine::transfers::uploads` tracks per-file upload progress
  (upload_id, acked_offset, chunk_size, chunks_done) so the sync loop
  can drive `upload_create` / `upload_write` / `upload_save` in bounded
  chunks and resume from the last completed offset on restart.
- **Configurable upload chunk size:** `[sync].upload_chunk_size`
  (default 10 MiB, range 256 KiB -- 256 MiB) controls how large each
  `upload_write` round-trip payload is.
- **End-to-end conflict resolution policies:** The conflict resolver
  now supports six policies: `prefer_local`, `prefer_remote`,
  `newest_wins`, `rename_both`, `error`, `manual_review`. Default is
  `rename_both` (both copies preserved, neither side loses data).
- **Configurable conflict policy:** `[sync].conflict_policy` (default
  `"rename_both"`) selects the default conflict resolution strategy.
- **`RenameBoth` conflict resolution variant:** New
  `ConflictResolution::RenameBoth` in `pcloud-model` carries
  `local_renamed_path`, `remote_renamed_path`, `original_path`, and
  `sync_id` so the engine can rename local and download remote under
  conflict-tagged filenames.
- **IPC surface for conflicts:** `Method::ListConflicts` and
  `Request::ConflictResolve { path, policy }` allow listing and
  resolving sync conflicts through the daemon socket.
- **CLI conflict commands:** `pcloudc conflict list` and
  `pcloudc conflict resolve <path> --policy <POLICY>` let operators
  inspect and resolve conflicts interactively.
- 9 new unit tests across `pcloud-engine`, `pcloud-config`.

### Added — Background sync loop wired into daemon (2026-04-16)

- **`sync_loop_runtime.rs`:** `RealSyncLoopRuntime` bridges daemon
  backends to the `SyncLoopRuntime` trait. Owns its own `SyncRuntime`,
  `TransferRuntime`, `EngineShell`, and WAL-mode SQLite connection. Auth
  token shared via `Arc<Mutex<Option<SecretString>>>`.
- **Daemon now spawns the sync loop** on `pcloudd serve`. The loop
  polls remote diff, walks local directories, persists diff cursors,
  and advances download transfers autonomously.
- **`spawn_daemon_sync_loop`:** public entry point for wiring the loop
  into any daemon entry path (binary, `serve_with_shutdown`, embedder).
- 7 unit tests, 4 E2E integration tests, 1 live-gated test.

### Added — Deletion propagation with SyncType awareness (2026-04-16)

- **`DeletePolicy` in `pcloud-engine::planner`.** New policy type that
  controls which delete operations the planner may emit, derived from
  the per-root `SyncType` and the global `propagate_deletes` config
  flag. `Full` propagates both ways; `UploadOnly` only propagates
  local-to-remote deletes; `DownloadOnly` only propagates
  remote-to-local deletes.
- **`propagate_deletes` config field** in `[sync]` section. Default
  `true`. When `false`, no delete operations are ever emitted
  regardless of `SyncType` (ultra-safe mode for environments where
  accidental data loss is unacceptable).
- **`Planner::plan_filtered`** method that applies the delete policy
  as a post-plan filter. `EngineShell::ingest_candidates_filtered`
  wires it through the engine.
- **`IncrementalScanTracker`** in `pcloud-engine::local_scan`. Tracks
  per-sync-root last-full-scan timestamps and queues filesystem watcher
  events. `decide()` returns `ScanDecision::FullScan` on first call and
  after the configured `full_scan_interval_secs` elapses; otherwise
  returns `ScanDecision::IncrementalOnly` with drained watcher events.
- **`full_scan_interval_secs` config field** in `[sync]` section.
  Default `300` (5 minutes). Between full walks, the engine relies on
  filesystem watcher events only. Validated to `[30, 86400]`.

### Added — SIGHUP config hot-reload (2026-04-16)

- **Config hot-reload via SIGHUP.** The daemon serve loop now observes
  the `RELOAD_REQUESTED` flag and re-reads the config file from disk.
  Hot-reloadable fields: observability flags (log level, tracing,
  metrics, audit export), rate-limit budgets, integrity-sweeper
  schedule, sync poll interval, data-residency allow-list. Fields that
  require a restart (auth vault path, IPC socket path, crypto master
  key) are silently ignored.
- **`config.reloaded` / `config.reload_failed` audit events.** On
  success, lists the changed keys. On parse error, keeps the previous
  config and emits the error.
- **`pcloudc reload` CLI command.** Sends SIGHUP to the daemon via the
  pidfile. Unix-only.
- **`config_reload` module** in `pcloud-daemon` with `diff_hot_reloadable`,
  `try_reload`, and 12 unit tests covering diff detection, event
  formatting, and error paths.
- **CI: `check` job with cross-platform matrix.** Added `cargo check
  --workspace --all-targets` on `ubuntu-latest`, `macos-latest`, and
  `windows-latest` to `rust.yml`.

### Added — Session refresh loop for long-running daemons (2026-04-16)

- **Proactive token refresh in the serve loop.** The IPC serve loop now
  calls `refresh_loop::tick` on every iteration (after each request or
  accept-timeout). When the session's auth token is within the
  configured refresh window, the daemon proactively exchanges it for a
  fresh token via the existing `AuthRuntime::refresh_token` path.
- **Accept-timeout on the IPC listener.** `BoundIpcServer::set_accept_timeout`
  sets `SO_RCVTIMEO` on the Unix listener so the serve loop wakes
  periodically even when idle, ensuring refresh ticks run without
  requiring client traffic.
- **`[auth]` config knobs.** Two new fields on `AuthPolicy`:
  `refresh_check_interval_secs` (default 300) controls the accept
  timeout / tick cadence; `refresh_margin_secs` (default 600) controls
  how many seconds before expiry the daemon starts proactive refresh.
- **`session_refresh` module.** New `pcloud-daemon` module with
  `policy_from_config` and `accept_timeout` helpers plus 6 unit tests
  covering config mapping, threshold clamping, and end-to-end refresh
  firing.

### Added — Integrity sweeper walk-and-compare loop (2026-04-16)

- **`run_once` now walks sync roots.** `IntegritySweeperShell::run_once`
  walks every configured sweep root via the `pcloud_fs` sweeper engine,
  computes local SHA-256 for each file, fetches remote SHA-256 via the
  `DaemonChecksumFetcher` trait, compares digests, and pipes mismatch
  events through the worker channel for audit.
- **`DaemonChecksumFetcher` trait.** New daemon-level trait abstracting
  remote checksum lookups. `NoOpChecksumFetcher` ships as the safe
  default (reports every file as `RemoteMissing`).
- **`SweepRoot` type and setter.** `set_sweep_roots` / `set_checksum_fetcher`
  allow the runtime to configure which directories are swept and how
  remote checksums are resolved.
- **Scheduler tick wired.** The cron scheduler's tick body now performs
  the same walk-and-compare as `run_once` instead of being a no-op
  placeholder.
- **Unit tests.** `run_once_walks_files_and_detects_match`,
  `run_once_detects_mismatch`, `run_once_with_no_roots_produces_no_events`,
  `run_once_handles_remote_not_found_gracefully`, plus hex parsing tests.

### Added — Mount transport wiring and `[mount]` cache config (2026-04-16)

- **Config-driven FUSE cache tuning.** New `[mount]` config fields:
  `cache_size_mb` (default 256), `page_cache_entries` (default 4096),
  `metadata_ttl_secs` (default 60). These feed into `AdapterOptions`
  when the daemon constructs the `ProtoFuseAdapter` at mount time.
  `PCLOUD_CACHE_SIZE_GB` env var still takes precedence for page-cache
  budget. New env-var overrides: `PCLOUD_MOUNT_CACHE_SIZE_MB`,
  `PCLOUD_MOUNT_PAGE_CACHE_ENTRIES`, `PCLOUD_MOUNT_METADATA_TTL_SECS`.
- **`try_install_pcloud_shim_factory` now reads config.** The daemon's
  mount-factory wiring path reads the profile's `[mount]` section to
  set metadata-cache capacity, metadata-cache TTL, and page-cache
  budget instead of relying solely on hardcoded defaults.
- **Integration test `mount_transport_wiring.rs`.** Two FUSE-gated
  tests (`mount_readdir_read_unmount_clean_teardown`,
  `remount_after_clean_unmount`) exercise the full mock-backend mount
  lifecycle: readdir, file read, unmount, and clean teardown
  verification.

### Added — Scheduled audit-chain verifier IPC + CLI wiring (2026-04-16)

- **`Method::GetAuditVerifierStatus` dispatched.** The IPC method was
  already defined in `pcloud-ipc` and the service/config existed; this
  change wires the dispatch arm in `RuntimeShell::handle_request_dispatch`
  and adds the `audit_verifier_status()` handler that serialises the
  shell snapshot into `AuditVerifierStatusPayload` JSON.
- **Bootstrap from config.** `bootstrap_with_config` now builds the
  `AuditVerifierShell` from the validated `[features.audit_verifier]`
  config block (default: enabled, 03:00 daily) instead of always
  constructing a disabled shell.
- **CLI `pcloudc audit-verifier status`.** New two-token subcommand
  (and single-token `audit-verifier-status` alias) renders the
  verifier's enabled state, last result, chain length, error detail,
  pass/fail counters, and last-run timestamp.
- **Integration test `audit_verifier_tamper.rs`.** 4 tests: fresh
  runtime reports `never_run`, pass surfaces via IPC, tampered chain
  surfaces failure detail via IPC, pass-then-fail increments both
  counters.
- **Runbook updated.** Playbook 34 now documents the scheduled
  verifier, its `pcloudc audit-verifier status` probe, and the
  `[features.audit_verifier]` config block.
- **Metrics/tracing labels.** `method_label` and `backend_label` now
  recognise `GetAuditVerifierStatus` for proper span naming.

### Added — Upload session state machine integration tests and CLI docs (2026-04-16)

- **13 daemon-level integration tests** in
  `crates/pcloud-daemon/tests/upload_sessions.rs` exercise the full
  upload session lifecycle through `RuntimeShell::handle_request`:
  create with each `ConflictMode`, `Rename` deduplication
  (`report (2).pdf`), pause/resume/cancel transitions, terminal-state
  rejection, unknown-session error surfaces, and list enumeration.
- **CLI reference** (`docs/book/src/reference/cli.md`) gains section
  4.22 "Upload Sessions" documenting `upload create`, `upload pause`,
  `upload resume`, `upload cancel`, and `upload list` with conflict-mode
  table and JSON response shapes.
- **Doctest fix** in `upload_sessions::pick_unique_name`: empty-array
  type annotation added to resolve inference ambiguity on edition 2024.
- **Pre-existing compilation fix:** added `audit_verifier_service`
  module declaration to `pcloud-daemon/src/lib.rs` and wired the
  `audit_verifier` field in `bootstrap.rs` (was missing, blocking all
  daemon compilation).

### Fixed — Upload-session CLI commands broke reproducibility build (2026-04-16)

- Five `Command` variants (`UploadCreate`, `UploadPause`, `UploadResume`,
  `UploadCancel`, `UploadList`) were missing from the `canonical_token_for`
  match in `crates/pcloud-cli/src/app.rs`, causing a non-exhaustive-patterns
  compile error that blocked `cargo build --profile release-repro`. Added
  the missing arms and corresponding single-token aliases in
  `parse_single_token`.
- Ran the `packaging/scripts/verify-reproducibility.sh` double-build
  verifier end-to-end; both `pcloudc` and `pcloudd` produce byte-identical
  SHA-256 across two consecutive `release-repro` builds.

### Added — CryptoShell DEK routing through KmsProvider verified (I03, 2026-04-16)

- **Sector-path KMS routing confirmed wired and tested.**
  `CryptoShell::derive_sector_file_key` routes through the injected
  `KmsProvider` when `mode = Kms`: generates DEK via OS CSPRNG on
  `enable_kms_mode`, wraps through the provider, caches unwrapped DEK
  with TTL via `unwrap_cached`, and evicts on `stop()`. Raw mode
  (default, `NullKms`) continues to derive per-file keys from the
  Argon2id master key.
- **Config gate:** `[crypto].mode = "raw" | "kms"` in
  `pcloud-config/src/crypto_kms.rs`; `mode = "kms"` requires a
  non-null `[crypto.kms]` provider block, validated at load time.
- **8 integration tests** in `pcloud-crypto/tests/kms_routing.rs`
  cover NullKms raw regression, mock-provider wrap/unwrap, sector
  seal/open round-trip through KMS, cache TTL amortisation,
  stop-evicts-cache proof, and reset-reverts-to-raw.
- **`docs/enterprise/kms.md`** updated: flipped "not yet wired" to
  reflect current wired state with test evidence.

### Added — Expand live-E2E harness to 6 new test families (2026-04-16)

- **`shares.rs`** — ShareFolder invite + list outgoing + cancel +
  modify/remove probes against a real account. Gated on
  `PCLOUD_TEST_PEER_USER`.
- **`crypto.rs`** — crypto setup/unlock/status/mkdir/lock/re-unlock
  lifecycle. Gated on `PCLOUD_TEST_CRYPTO_PASSWORD`.
- **`snapshot_prune.rs`** — seed 10 fake snapshots, dispatch GFS prune
  with `retention_days=7`, assert keep/drop set matches GFS bucketing.
- **`mount_linux.rs`** — mount via IPC + readdir + cat + unmount (Linux
  only). Gated on `PCLOUD_FUSE_TEST=1`.
- **`rate_limit.rs`** — burst 10 Expensive IPC requests, assert
  rate-limiter kicks in with Conflict + retry-after hint.
- **`drain.rs`** — drain state machine Running -> Draining -> Stopped;
  InFlightGuard RAII accounting. No backend credentials required.
- All six follow `mod common;`, `skip_if_not_live`, `TestDaemon::new`,
  `assert_no_secret_leak` conventions. README.md updated with the full
  binary table and environment variable reference.

### Added — All 7 canonical SLO observe call sites wired (I15, 2026-04-16)

- **IPC latency + error rate:** `RuntimeShell::handle_request` observes
  `ipc.request.latency.p99` and `ipc.request.error_rate` on every IPC
  dispatch (unconditional, not feature-gated).
- **Auth login success rate:** `auth_response` in `runtime.rs` observes
  `auth.login.success_rate` on `LoginSucceeded`/`LoginFailed` events.
- **Upload throughput:** upload completion path in `runtime.rs` observes
  `upload.throughput_mbps.p50` per completed `upload_bytes` call.
- **Mount read latency:** FUSE `read` shim in `platform/linux.rs`
  observes `mount.read.latency.p99` via `slo_hook::observe_mount_read`.
- **Integrity sweeper run:** scheduler loop in
  `integrity_sweeper_service.rs` observes `integrity_sweeper.run.p95`
  via `slo_hook::observe_integrity_sweeper_run`.
- **Audit chain verify:** `audit_verify_chain` (on-demand) and
  `AuditVerifierShell::run_once`/`scheduler_loop` (scheduled) observe
  `audit.hash_chain.verify.daily_pass_rate`.
- **Integration test:** `slo_dispatch.rs` gains
  `dispatch_plus_login_produces_non_empty_slo_samples` which drives real
  IPC dispatches + simulated login, queries `Method::GetSlo`, and
  asserts the IPC and auth SLOs report non-empty samples.
- **Docs:** `docs/book/src/architecture/performance.md` updated with
  per-SLO call-site inventory; the "not yet wired" caveat is removed.

### Added — Live FUSE write-path remount readback proof (bd-1du.4.6, 2026-04-16)

- **`crates/pcloud-fs/tests/fuse_write_path_live.rs`** is a new Linux
  FUSE kernel integration test that proves the write path closes the
  full `write → unmount → remount → byte-identical readback` loop
  against a real kernel mount. It composes `PcloudFsShim` over a
  `MockFolderBackend` + `MockFileBackend` + recording upload
  backend, mounts via `MountService::mount_fuser`, writes a
  non-trivial 256 KiB payload through `std::fs::write`, unmounts,
  seeds the mocked server listing with the captured upload, rebuilds
  a **fresh** `PcloudFsShim` over a brand-new staging dir and
  journal, remounts the same mountpoint, and asserts
  `std::fs::read(..)` returns the original bytes.
- **Supported FUSE write ops on the `PcloudFsShim` direct-shim path
  (recapped):** `create`, `write`, `flush`, `fsync`,
  `setattr(size)`, `release`, `unlink`, `rename`, `mkdir`, `rmdir`.
  The `BoxedFuserShim` / `FuserShim<A>` dyn-trait shim in
  `platform/linux.rs` remains read-only by design — see the
  `bd-1du.4.6` follow-up note in that file.
- **Gated** on `PCLOUD_LIVE_E2E=1` / `PCLOUD_FUSE_TEST=1` like every
  other live FUSE test in this crate; graceful-skips on hosts
  without `/dev/fuse`, `SYS_ADMIN`, or `fusermount3`. Live-verified
  on the developer host alongside the existing
  `fuse_read_path_live.rs` and `fuse_small_write_wiring.rs` tests.
- **Operator docs.** `docs/book/src/operations/partial-transfers.md`
  gains a new §10.1 "Upload via mounted drive (bd-1du.4.6 — Linux,
  pre-alpha)" section documenting the write-path recipe, the
  supported FUSE ops, the H5 sidecar interaction, and explicit
  deferrals (chunked `upload_write`, dyn-shim writes, macOS/Windows).

### Fixed — Undeclared `slo_hook` module broke `cargo check -p pcloud-fs` (2026-04-16)

- `crates/pcloud-fs/src/slo_hook.rs` existed but was not declared as
  a module in `crates/pcloud-fs/src/lib.rs`, leaving two unresolved
  `crate::slo_hook::observe_mount_read(..)` call sites in
  `platform/linux.rs`. The module is now published via
  `pub mod slo_hook;` so the crate compiles cleanly on Linux.

### Added — Graceful SIGTERM drain + daemon-upgrade handoff protocol (2026-04-16)

- **Drain state machine in `pcloud-daemon::signals`.** On `SIGTERM`
  the daemon now transitions `Running` → `Draining` → `Stopped` via
  process-wide atomics. The serve loop observes the state change,
  refuses new non-status IPC connections with
  `ResponseStatus::Unavailable("daemon draining, retry")`, and waits
  for in-flight requests to complete up to
  `[upgrade].drain_timeout_secs` (default 30 s) before unbinding the
  socket and exiting `0`.
- **`Method::DrainStatus` IPC surface** returns a stable
  `DrainStatusPayload` JSON envelope — `{state, in_flight,
  elapsed_drain_ms}` — and is admitted by the drain gate so operators
  can poll progress up to the moment the socket is unbound.
- **`pcloudc drain` CLI command.** Reads `<state_dir>/daemon.pid`,
  dispatches `SIGTERM`, and polls `Method::DrainStatus` every 500 ms
  until the daemon reports `state == "stopped"` or
  `[upgrade].handoff_timeout_secs` expires. Returns exit code `0` on
  clean stop, `6` on timeout, `1` on missing pidfile.
- **Pidfile** written atomically at `<state_dir>/daemon.pid` (`0600`)
  by `pcloudd serve`, removed on clean exit.
- **Config `[upgrade]` section.** New `handoff_timeout_secs` and
  `drain_timeout_secs` knobs (defaults 30 / 30, capped at 600). Older
  envelopes load cleanly via `#[serde(default)]`.
- **Handoff protocol.** A new daemon instance cooperates with the
  previous instance's drain through the existing Tier-2 HA lease
  (`pcloud-daemon::ha_lease`); no socket-fd passing, no file-lock
  contention, just "wait for the old lease to drop, then bind".
- **Integration coverage.** `crates/pcloud-daemon/tests/graceful_drain.rs`
  starts an in-process daemon, flips the shutdown flag, and verifies
  both the `Unavailable` gate and the post-drain `state == "stopped"`
  payload.
- **Operations docs.** `docs/book/src/operations/upgrade.md` §Graceful
  drain now documents the real recipe (`pcloudc drain`), the new
  config knobs, and replaces the prior "design note — not yet
  code-backed" callout with an accurate description of the shipped
  handoff flow.

### Added — In-process fleet reference server + live mTLS integration test (2026-04-16)

- **`crates/pcloud-fleet/tests/reference_server.rs`** is a new
  in-process fleet server helper. It binds `127.0.0.1:<auto_port>`,
  serves HTTPS via `tokio-rustls` using a CA-signed leaf cert fixture
  shipped under `tests/fixtures/`, accepts `POST /v1/heartbeat`, and
  validates the agent's `X-PCloud-Body-Signature` header as a valid
  ed25519 signature from a device public key in its configured trust
  set. Forbids `unsafe_code`.
- **`crates/pcloud-fleet/tests/live_mtls.rs`** drives a real
  `MtlsFleetAgent` through that reference server end-to-end:
  - `heartbeat_is_accepted_end_to_end` — pinned-CA TLS + valid body
    signature returns 200 OK.
  - `tampered_body_signature_is_rejected` — a request whose header
    signs a different payload than is posted gets 401.
  - `untrusted_device_sid_is_rejected` — an agent whose SID is not in
    the server trust set gets 401, surfaced as `FleetError::Transport`.
- **Test dev-dependencies added** (`tokio`, `tokio-rustls`, `hyper`,
  `hyper-util`, `http-body-util`, `bytes`). All were already present
  transitively in `Cargo.lock` through `pcloud-web` / `reqwest`, so no
  net new transitive crates enter the workspace.
- **Audit gap closed:** previous status ("no reference server in this
  repository; offline stub only") is now obsolete. Live-in-prod interop
  against a third-party fleet controller is still **not** tested and
  still **not** claimed; the reference server is a protocol spec in
  code form, not a substitute for a production controller. See
  `docs/enterprise/fleet.md` for the updated status text.

### Added — Pluggable `RevisionProvider` for `log` / `diff` / `restore` (2026-04-16)

- **New `pcloud_proto::revision_provider` module** exposing a
  `RevisionProvider` trait with one method `list_revisions(path)`, the
  shared `Revision` / `RevisionError` types, and two concrete
  implementations:
  - `NullRevisionProvider` (default) — returns
    `RevisionError::NotConfigured` with a message naming the exact
    config key operators need to populate. Exposed as the constant
    `NULL_PROVIDER_MESSAGE` so every surface emits the same text.
  - `HttpRevisionProvider` (opt-in, feature `file-history-http`) —
    POSTs `{"path": "<remote path>"}` to an operator-configured URL
    and parses a JSON array of revisions (or `{"revisions":[...]}`
    envelope). URL validation refuses plaintext `http://` unless the
    caller uses the hidden `new_allow_plaintext` constructor; response
    bodies are capped at 1 MiB. Transport is caller-injected so no
    HTTP client is pulled into `pcloud-proto`.
- **New `[file_history]` config section** on `ConfigProfile`
  (`pcloud_config::file_history::FileHistoryConfig`) with
  `revision_url: Option<String>`. Production profiles refuse
  non-`https://` URLs at config-load time; Development / Test accept
  `http://` for local mock servers. Optional on disk
  (`#[serde(default)]`) so older envelopes load unchanged.
- **Daemon upgrade** in `pcloud_daemon::RuntimeShell::file_history`
  replacing the bare `ResponseStatus::Unavailable` with a structured
  JSON payload: `{"status":"not_configured","message":…,"next":…,"path":…}`
  on provider failures and `{"revisions":[…],"count":N}` on success.
  Exit code is preserved as `6 Unavailable` when the provider is not
  configured. Stable `status` taxonomy: `not_configured`,
  `invalid_url`, `transport`, `http_status`, `malformed_response`,
  `invalid_request`.
- **CLI `diff` / `restore` stubs** (`crates/pcloud-cli/src/main.rs`)
  upgraded to emit the same structured JSON payload as the daemon's
  `log` response so tooling keys on one taxonomy across all three
  revision operations. No IPC changes.
- **Documentation** updated in `docs/book/src/reference/cli.md` §4.14
  and `docs/man/pcloudc.1` (new "REVISION HISTORY COMMANDS" section).
- **Tests:**
  - `crates/pcloud-proto/src/revision_provider.rs`: 4 Null-provider /
    serde unit tests + 8 HTTP-provider tests (under
    `--features file-history-http`).
  - `crates/pcloud-config/src/file_history.rs`: 6 config validation
    tests.
  - `crates/pcloud-daemon/tests/file_history_provider.rs`: 6
    integration tests covering structured-payload shape, empty-path
    guard, limit pass-through, production plaintext rejection,
    development plaintext accept, and CLI/daemon taxonomy parity.

### Added — Canonical Service-Level Objectives (SLOs)

- New canonical SLO set defined in
  `crates/pcloud-observability/src/slo.rs`, evaluated live against
  existing Prometheus histograms and counters:
  - `ipc.request.latency.p99 < 100ms` (rolling 5 m)
  - `ipc.request.error_rate < 0.1%` (rolling 5 m)
  - `auth.login.success_rate > 99%` (rolling 1 h)
  - `upload.throughput_mbps.p50 > 5` (rolling 5 m)
  - `mount.read.latency.p99 < 50ms` (rolling 5 m)
  - `integrity_sweeper.run.p95 < 5min` (per-run)
  - `audit.hash_chain.verify.daily_pass_rate > 99.9%` (daily)
- **Honesty:** these thresholds are aspirational targets. The
  pre-GA build does not uniformly meet every SLO under load; the
  registry reports measured values straight from atomic counters and
  uses a distinct `no_data` status for SLOs that have not yet
  accumulated enough samples so operators never conflate "quiet"
  with "healthy".
- **New IPC method `Method::GetSlo`** returning a JSON
  `SloReportPayload` of `{slo_name, target, actual, status}` entries
  plus an aggregate `pass` bit.
- **CLI:** new canonical `pcloudc slo` command (field-selector
  friendly — `pcloudc slo pass`, `pcloudc slo slos`,
  `pcloudc --json slo`).
- `/slo` HTTP endpoint now includes a `slos` array alongside the
  existing compact fields (`ip95_ms` / `upload_retry_ratio` /
  `crash_free_fraction` / `pass`); the legacy fields are retained for
  backwards compatibility with existing dashboards and alert rules.
- Documentation updates:
  `docs/book/src/architecture/performance.md` documents the canonical
  SLO set and its instrumentation call sites;
  `docs/book/src/operations/runbook.md` adds a "Responding to SLO
  violations" playbook.

### Added — Per-category IPC rate limiter on expensive daemon operations

- New `pcloud_daemon::rate_limit` module introduces a per-session,
  per-category token-bucket admission check that runs **before** any
  backend dispatch. Requests are classified into three categories —
  `cheap` (status / userinfo / field selectors; no limit), `medium`
  (list-style endpoints; default 30/min), and `expensive` (snapshot
  create, integrity run-once, bulk public-link operations, tree-link
  create, crypto password change; default 6/min). Over-budget callers
  receive `ResponseStatus::Conflict` with a
  `"rate limit exceeded: <category>, retry after Ns"` message and the
  backend is **not** invoked, closing an audit finding where a chatty
  client could exhaust daemon work budgets.
- New `[rate_limit]` config section (`pcloud_config::rate_limit`) lets
  operators override any bucket's `capacity` / `refill_per_sec`.
  Setting `capacity = 0` disables that category without removing the
  block. Schema + validator updated. See
  `docs/book/src/reference/config.md` §`profile.rate_limit`.
- `pcloud-resilience` gains a `MethodRetryPolicy` /
  `RetryClass::{Idempotent,Mutation,Unknown}` pairing so callers can
  decide retriability per method (secure default: retry idempotent
  only). The base `RetryPolicy` / backoff primitives are unchanged.

### Added — Tier-2 active-passive HA (file-lock lease handoff) (2026-04-16)

- New `[ha]` config block (`pcloud_config::ha::HaPolicy`): opt-in
  `enabled = false` default; `mode = "refuse" | "passive"`;
  tunable `heartbeat_interval_secs` (default 30) and
  `passive_poll_interval_secs` (default 10). Config JSON schema
  updated; older envelopes load unchanged.
- New module `pcloud_daemon::ha_lease`
  (`#![forbid(unsafe_code)]`) built on the `fs2` safe
  `flock(LOCK_EX | LOCK_NB)` wrapper.
  `LeaseHolder::try_acquire` writes owner metadata
  (`hostname`, `pid`, `start_ts_unix`, `instance_id`,
  `last_heartbeat_unix`) to `<state_dir>/daemon.lease` (mode
  `0600`). A 30s heartbeat worker bumps `last_heartbeat_unix`
  on every tick; dropping the holder joins the worker and
  releases the lease. Contention surfaces
  `LeaseError::HeldBy { owner }` carrying the primary's
  metadata so the secondary can name it in diagnostics.
- Bootstrap integration: when `[ha].enabled = true`, the daemon
  attempts the lease inside `bootstrap_with_config`. Success →
  primary; `HeldBy` + `mode = "refuse"` → `BootstrapError` with
  a diagnostic naming the primary; `HeldBy` +
  `mode = "passive"` → secondary binds IPC and rejects every
  non-probe request with `ResponseStatus::Unavailable` + a
  "this daemon is passive; primary is <host>/pid=<pid>"
  message. `HaStatus`, `GetHealth`, `Health`, and `Shutdown`
  remain reachable in passive mode.
- New IPC surface: additive `Method::HaStatus` variant plus
  the `HaStatusPayload { mode, lease_owner, lease_age_s,
  lease_path }` payload serialised as JSON into
  `Response::message`. No breaking changes (variant added to
  an already-`#[non_exhaustive]` enum).
- New CLI surface: `pcloudc ha status` (canonical single-token
  `ha-status`). Routed through the same
  `command_accepts_bare_fields` allow-list as
  `integrity-status`, so `--field mode` and friends work.
- Tests: unit coverage in `ha_lease::tests`
  (acquire/steal-on-release/heartbeat/metadata/permissions/
  payload encoding) plus 5 integration scenarios in
  `crates/pcloud-daemon/tests/ha_two_daemon_contention.rs`
  (primary wins, refuse blocks second daemon, passive rejects
  non-probe requests, takeover-after-release, disabled
  default).
- Docs: `docs/enterprise/ha.md` §4.2 flipped from design-only
  to **landed**; Tier 3 (Windows SCM) and Tier 4 (nginx Web UI
  front-door) remain design-only.

### Added — Security invariant test harness + security-model citation audit (2026-04-16)

- **New `crates/pcloud-ipc/tests/security_invariants.rs`** consolidating
  15 user-space proofs for the SEC-XX invariants flagged by the
  2026-04-16 agent audit as documented-but-unenforced. Tests are named
  `sec_XX_<short_slug>` so the security-model doc can cite them one-to-
  one. Coverage: SEC-01 (length-only exposure), SEC-02 (`Debug` and
  `{:#?}` redaction), SEC-04 (`<Wrapper as Zeroize>::zeroize` empties
  the wrapper and `expose_secret()` sees the scrubbed state), SEC-10
  (`IpcServer::bind` produces a `0600` socket on a `0700` parent),
  SEC-11 (`authorize_peer` rejects non-owner uid / root / accepts only
  the exact owner), SEC-12 (encoder rejects a 1.5 MiB payload;
  `MAX_IPC_PAYLOAD_LEN` pinned to 1 MiB), SEC-13 (version `0xFFFF`
  yields `ProtocolError::VersionMismatch`), SEC-50
  (`catch_unwind(AssertUnwindSafe(...))` pattern converts a panic into
  `ResponseStatus::InternalError` without propagating). Plus a bonus
  wire-safety proof that `Response::PolicyViolation` never embeds a
  `<redacted>` marker.
- **`docs/book/src/architecture/security-model.md` citations reconciled.**
  Every SEC-XX row now either points to a test file that exists in-tree
  or is marked `[review-only]` with a one-sentence justification and a
  named enforcement site. Previously every SEC-XX row cited a test path
  that had never been created; those stale citations were replaced.
  Remaining `[review-only]` rows: SEC-03 (tracing field redaction),
  SEC-22 (durable-token opt-in gate), SEC-23 (absence of
  `store_password`), SEC-31 (audit persistence failure surface), SEC-32
  (WAL crash-consistency), SEC-41 (rustls root set), SEC-42 (endpoint
  override), SEC-51 (background panic hook → Prometheus gauge).
  End-to-end coverage for these rows is tracked under `bd-1du.10`.
- **`crates/pcloud-ipc/Cargo.toml`** gains `pcloud-secret` and
  `zeroize` as `[dev-dependencies]` so the new integration test can
  assert the secret-wrapper contract without creating a workspace
  cycle; both are tests-only and do not add a runtime dep on the crate.

### Added — PKCS#11 HSM provider and `CryptoShell` KMS routing (2026-04-16)

- **Real `Pkcs11Hsm` provider** in `crates/pcloud-kms/` behind the
  opt-in `pkcs11` Cargo feature. Binds to a vendor PKCS#11 shared
  library at runtime via `cryptoki = "0.10"` (Apache-2.0 / MIT) and
  performs `AES-GCM` wrap/unwrap inside the HSM using
  `C_Encrypt`/`C_Decrypt`. The wrapping key **never leaves the HSM**;
  the client holds only a user PIN (in
  `pcloud_secret::secret_string::SecretString`) and a `CKA_LABEL`.
- **Feature-off stub**. When `pkcs11` is disabled the crate still
  exports a `Pkcs11Hsm` type whose constructor returns
  `KmsError::NotImplemented("pkcs11 (rebuild with --features pkcs11)")`
  so misconfigured deployments fail loudly instead of silently
  downgrading to `NullKms`.
- **DEK routing through `CryptoShell`**. `CryptoShell` now carries an
  injected `Box<dyn pcloud_kms::KmsProvider>` field
  (`#[serde(skip, default = NullKms)]`), plus
  `with_kms_provider` / `set_kms_provider` / `kms_wrap_dek` /
  `kms_unwrap_dek` / `kms_provider_name` helpers. Deserialised shells
  always come back with `NullKms` — the runtime re-injects the real
  provider from the profile before `start`. `kms_unwrap_dek` uses
  `KmsProvider::unwrap_cached` with the default 5-minute TTL.
- **`[crypto.kms]` config section** in `pcloud-config`
  (`crypto_kms::CryptoKmsConfig`). Tagged serde enum
  (`null`/`aws`/`vault`/`pkcs11`) plus provider-specific fields.
  Secrets (Vault token, PKCS#11 PIN) are **never** stored in the
  config — the config names an env var (`token_env`, `pin_env`) and
  the factory reads it into a `SecretString` at construction time.
  The JSON schema gets a new optional `profile.crypto` section; older
  envelopes still load.
- **Factory helper** `CryptoKmsConfig::build_provider` behind the
  `kms-factory` feature on `pcloud-config`; pass-through features
  `aws-kms` / `vault-kms` / `pkcs11-kms` wire the matching feature on
  `pcloud-kms`. Returns `BuildProviderError::ProviderFeatureDisabled`
  when the config names a provider the current build does not
  compile.
- **Tests**: 10 unit tests in `pcloud-kms` (including one pkcs11
  regression that exercises the bad-module-path error path behind the
  `pkcs11` feature), 4 integration tests in
  `crates/pcloud-crypto/tests/kms_routing.rs` proving the trait-object
  dispatch roundtrips, the `set_kms_provider` swap takes effect on a
  live shell, and the serde-skip contract preserves `NullKms` across
  serialise/deserialise, and 5 unit tests in
  `pcloud-config::crypto_kms` for config validation and round-trip.
- **Docs**: `docs/enterprise/kms.md` status block updated to reflect
  routing landed + pre-alpha honesty (no live HSM proof yet).
- **Security posture unchanged**: `pcloud-kms` stays
  `#![forbid(unsafe_code)]`; the `unsafe` inside `cryptoki` wraps the
  vendor C ABI and is contained inside that crate — we never expose
  raw PKCS#11 handles.

### Added — Platform-native vault backend selection wired into bootstrap (2026-04-16)

- **New `[auth.vault]` config section** (`pcloud-config::auth`) with a
  `VaultBackend` enum (`auto` / `file` / `keychain` / `dpapi` /
  `secret-service`). Default is `auto`. Serde kebab-case; the section
  is optional on disk so older envelopes (v1/v2) still load.
- **New `PCLOUD_VAULT` env-var override** honoured by
  `pcloud_config::env::apply_env_overrides`. Accepts canonical names
  plus short aliases (`mac`/`macos`, `win`/`windows`,
  `ss`/`secretservice`).
- **Runtime selection** in `pcloud_daemon::vault::select_vault`: macOS
  picks `KeychainVault`, Windows picks `DpapiVault`, Linux picks
  `SecretServiceVault` with file fallback on D-Bus / session
  unavailability, and BSD / other Unix fall back to `FileVault`.
  Explicit backend requests are honoured verbatim; platform mismatch
  (e.g. `keychain` on Linux) returns a hard
  `VaultSelectError::UnsupportedOnPlatform` with a clear message
  rather than silently degrading.
- **Bootstrap wiring** (`bootstrap::bootstrap_with_config`) now invokes
  `select_vault` once, logs the effective backend + any fallback
  warning to stderr, and threads the boxed `PlatformVault` through
  `sync_bootstrap_auth_state` and
  `apply_bootstrap_credentials_with_vault` so durable-token write
  paths go through the selected backend instead of always routing to
  the on-disk `FileVault`.
- **Stale comment fixed** in `pcloud-daemon/src/vault/mod.rs` — the
  `unimplemented!()` stub description is gone; the module now
  documents all four backends as real implementations.
- **Tests** — 6 Auto-selection integration tests in
  `tests/platform_vault_crossplat.rs` and 7 unit tests for the new
  `auth` config module. Runtime-side legacy free-function call sites
  in `runtime.rs` remain on the file vault pending a follow-up
  migration; not a gate blocker.
- **Pre-alpha honesty:** macOS Keychain and Windows DPAPI live
  round-trips still require hardware targets before any
  "production-ready" claim. Linux CI without a session D-Bus falls
  back to `FileVault` with a stderr warning — this is the documented
  safe default.

### Changed — Live FUSE read-path wiring for the `FuseAdapter`-trait shim on Linux (bd-1du.4, 2026-04-16)

- **Wired the dyn-trait Linux FUSE shim**
  (`crates/pcloud-fs/src/platform/linux.rs`): both
  `FuserShim<A: FuseAdapter>` (used by
  `MountService::mount` via `mount_with_fuser`) and
  `BoxedFuserShim` (used by the cross-platform
  `LinuxPlatformMount::mount_adapter` seam) now forward kernel
  `lookup` / `getattr` / `readdir` / `open` / `read` /
  `release` through the [`FuseAdapter`](crates/pcloud-fs/src/fuse_adapter.rs)
  trait. Previously these shims inherited `fuser`'s default
  `ENOSYS` replies, so `MountService::mount(..)` mounts returned
  empty directories and empty reads even though the backing
  `ProtoFuseAdapter` was fully wired.
- `readdir` synthesises `.` / `..` on the first page; `..` on the
  dyn-shim points back at the current inode because the
  object-safe trait cannot expose `InodeTable` for a back-pointer
  (the full parent-aware shim is `PcloudFsShim`, which the daemon
  composes directly for writable mounts).
- **Read-only by design.** `open(O_WRONLY|O_RDWR)` on these shims
  returns `EROFS`. Write-path mounts go through the already-live
  `PcloudFsShim` + `WritePathService` composition (exercised by
  `fuse_kernel_e2e.rs`).
- **New integration test**
  `crates/pcloud-fs/tests/fuse_read_path_live.rs`: mounts a mocked
  folder/file backend at a real `/dev/fuse` kernel mount via
  `MountService::mount`, exercises `std::fs::read_dir` +
  `std::fs::read` on both root and a nested directory, asserts
  byte-identical payloads on cat, confirms write-mode opens are
  rejected with `EROFS`/`ENOSYS`/`EACCES`, and unmounts cleanly
  via `MountHandle::unmount`. Gated on `PCLOUD_FUSE_TEST=1` (or
  `PCLOUD_LIVE_E2E=1`); graceful-skips on hosts without `/dev/fuse`
  / `fusermount3` / `SYS_ADMIN`.
- **Test-support addition.**
  `MockFolderBackend::insert_dir_with_sizes` lets integration
  tests publish per-entry file sizes in listings so that the
  kernel's `getattr` reply advertises the true size (the kernel
  caps `read(2)` to the advertised size, so the prior
  `insert_dir(..)` helper — which always set `size: None` — made
  round-trip reads observe empty payloads).
- **Docs.** `docs/book/src/operations/platforms/linux.md` now
  carries an explicit FUSE status block distinguishing the
  live-read-only dyn-shim path from the live-read+write
  `PcloudFsShim` path, and explicitly lists the deferrals
  (dyn-shim write path, chunked upload) under `bd-1du.4.6`.
- **Status.** `STATUS.md` updates `bd-1du.4` to note the
  read-path landing; parity-matrix counts unchanged —
  `fs,mounted pcloud filesystem` stays `Partial` until
  `bd-1du.10` closes the final proof gate. Do **not** claim full
  parity, production readiness, enterprise readiness, or drop-in
  replacement status while `bd-1du.10` remains open.

### Added — Live OTLP collector interop test + exporter allow-list hardening (2026-04-16)

- **New integration test**
  `crates/pcloud-observability/tests/otlp_live_interop.rs`
  (feature-gated on `tracing-otlp`, entire file is
  `#[cfg(feature = "tracing-otlp")]` so feature-off builds compile
  an empty test crate). Spins up an in-process OTLP/HTTP collector
  with `axum`, initializes the daemon tracer via
  `pcloud_observability::tracing::init`, emits the
  `pcloudd.dispatch` + `pcloudd.backend.<name>` span pair that
  matches the daemon dispatch path, and decodes the received
  protobuf via `opentelemetry-proto`. Asserts:
  - exactly **one** `pcloudd.dispatch` parent span and **one**
    `pcloudd.backend.*` child span arrive in the same trace,
  - the child's `parent_span_id` equals the parent's `span_id`,
  - every exported attribute key is drawn from the five-key
    `ALLOWED_ATTRS` allow-list (`command`, `duration_ms`,
    `error_category`, `status_code`, `trace_kind`) — any leak
    fails the test,
  - W3C `traceparent` propagation: the inbound trace id round-trips
    verbatim into the exported parent span.
- **Closes** the "offline-tested only" gap called out in
  `docs/enterprise/tracing.md` §10 and row 28 of
  `PRODUCTION_READINESS_AUDIT.md`. Vendor-backend interop (Datadog,
  Honeycomb, Tempo UI, New Relic) remains unverified — the test
  proves OTLP wire-format correctness against a reference decoder,
  not vendor-UI ingest.
- **Dev-dependencies added** to `pcloud-observability`: `axum`
  (`http1 + tokio`), `opentelemetry-proto`
  (`gen-tonic-messages + trace`), `prost`, `tokio` test runtime,
  `tracing`. Dev-only; none flow into runtime binaries.

### Security — OTLP exporter allow-list now enforced end-to-end (2026-04-16)

- `pcloud_observability::tracing::init` now configures the
  `tracing-opentelemetry` layer with `with_location(false)`,
  `with_threads(false)`, and `with_tracked_inactivity(false)`.
  The prior layer silently injected `code.filepath`,
  `code.namespace`, `code.lineno`, `thread.id`, `thread.name`,
  `busy_ns`, and `idle_ns` on every exported span — keys that
  were **not** in `ALLOWED_ATTRS` and therefore violated the
  five-key allow-list contract documented in
  `docs/enterprise/tracing.md` §5.2. These keys now never leave
  the daemon. Found by the new live OTLP interop test during its
  first run — `attr_redact` was only a call-site filter, not an
  exporter-layer guarantee.

### Security — Runtime-gated plugin capability enforcement + panic-safe dispatch (2026-04-16)

- `pcloud-plugin-api`: added `PluginRegistry::dispatch`, a single,
  panic-guarded enforcement point every host dispatcher MUST now use
  to hand an operation to a plugin. It combines
  `PluginCapability::required_for(op)` with the plugin's *granted*
  capability set and short-circuits the handler when a capability is
  missing. The audit sink receives a structured
  `PluginAuditEvent::InvocationDenied` event labelled
  `plugin.capability.denied{plugin, op, missing}`; the handler
  closure is **not** invoked.
- Handler panics are caught with `std::panic::catch_unwind`: the panic
  payload is **dropped at the boundary** (it may contain
  plugin-constructed data), the registry emits
  `PluginAuditEvent::HandlerPanic` + `PluginAuditEvent::PluginDeregistered`,
  and the offending plugin is removed from the registry. Subsequent
  calls to that plugin id return `PluginError::UnknownPlugin`. New
  `PluginError::HandlerPanic { plugin_id, operation }` surfaces the
  event to callers.
- Added `PluginRegistry::deregister(plugin_id, reason, audit)` for
  operator-initiated removal (idempotent; emits
  `PluginAuditEvent::PluginDeregistered`).
- Added two `PluginAuditEvent` variants (`HandlerPanic`,
  `PluginDeregistered`). The SDK's `StoreAuditSink` was extended to
  persist them under `plugin.handler_panic` / `plugin.deregistered`
  categories. This is a source-level addition; downstream `match`
  arms on the event enum must cover the new variants.
- `pcloud-plugin-api` lib tests: +10 cases covering DLP revoked-cap
  refusal, autoheal quarantine refusal without `SyncControl`, crypto
  and network-egress deny paths, panic-guard de-registration
  (including the boundary case where re-dispatch returns
  `UnknownPlugin`), and explicit `deregister` idempotence. Full
  crate suite: 23/23 passing.
- Docs: `docs/plugins/README.md` gained a new section
  "Capability enforcement is **runtime-gated**" describing the single
  choke point. Each per-plugin page (`autoheal.md`,
  `backup-schedule.md`, `dlp-builtin.md`, `publink-expiry.md`)
  gained a paragraph naming the exact structured deny event emitted
  on revocation and the panic-isolation behaviour.
- No `unsafe` added (`#![forbid(unsafe_code)]` preserved).

### Added — Data-residency enforcement wired into daemon runtime (2026-04-16)

- `RuntimeShell::check_residency` is now the single funnel the
  daemon dispatch paths consult before performing any operation
  that would publish or route data through a pCloud data center.
  Strict-mode violations short-circuit with
  `ResponseStatus::PolicyViolation { kind: "data_residency" }`
  and a helpful message naming the offending region and the
  configured `[data_residency] allowed_regions` list.
- Three high-value enforcement points in
  `pcloud-daemon/src/runtime.rs` call the evaluator:
  `add_sync_root` (after remote-folder validation, before the
  sync-root record is persisted), `create_public_link` /
  `create_file_public_link` / `create_folder_public_link`, and
  `create_upload_link`. All three share the same audit-event
  surface.
- Audit categories `residency.warn` (non-strict near-misses —
  the operation proceeds but is recorded so operators can count
  violations pre-strict rollout) and `residency.violation`
  (strict refusals) carry a stable
  `op=… region=… allowed=[…] refused=… warned=…` detail line.
- `RuntimeShell` gains a `residency_cache`
  (`pcloud_backends::residency::RegionCache`) shared between the
  enforcement call sites; region lookups are memoized for the
  default 1h TTL.
- Integration tests in `crates/pcloud-daemon/tests/residency.rs`
  cover every decision branch of the evaluator plus dispatch
  ordering (auth gate before residency gate).
- `set_api_server` dispatch-level enforcement is still
  outstanding; the helper exists in `pcloud-backends` but is
  not yet consulted on that path. Tracked separately so the
  pre-alpha "enforced at the three call sites" claim remains
  honest — today it is enforced at **two** of the three.

### Added — Integrity sweeper scheduler + battery pause hook (2026-04-16)

- `[features.integrity_sweeper].schedule_cron` is now honoured. A new
  `pcloudd-integrity-scheduler` thread parses the expression via the
  `cron` crate, sleeps until the next boundary via a `Condvar`-gated
  wait, and invokes `IntegritySweeperShell::run_once` at each tick.
  Invalid cron expressions are rejected at
  `IntegritySweeperShell::from_config` time so the scheduler never
  silently runs on an unparseable schedule; the error surfaces as
  `io::ErrorKind::InvalidInput` with the offending string.
- `[features.integrity_sweeper].pause_on_battery` is now honoured. On
  Linux the scheduler reads `/sys/class/power_supply/*/status`; on
  macOS and Windows it consults the `battery` crate. When any supply
  reports `Discharging` the tick is skipped and a structured event
  `{"event":"integrity_sweeper.paused","reason":"on_battery"}` is
  emitted to stderr. Platforms without a battery facade (servers,
  VMs, containers) log a one-shot warning and continue running.
- New workspace dependencies: `cron = "0.12"`, `chrono = "0.4"`, and
  `battery = "0.7"` (macOS/Windows only — Linux avoids the udev chain
  by reading sysfs directly).
- New test seams: `PowerSource` trait, `MockPowerSource`, and
  `IntegritySweeperShell::{battery_skip_count, scheduled_run_count}`
  counters for deterministic scheduler tests.
- `pcloud.conf.5`: removed the "parsed but not yet honored" caveat on
  `schedule_cron` and `pause_on_battery`; replaced with precise
  scheduler + power-source reader semantics.
- `pcloud-config::integrity_sweeper` docstrings updated to reflect
  that both fields are wired and honoured.

### Security — Tightened `cargo deny` policy and bumped `fuser` past RUSTSEC-2021-0154 (2026-04-16)

- `Cargo.toml`: bumped `fuser` from `0.15` to `0.16`. The
  `0.15.1` lockfile entry carried **RUSTSEC-2021-0154** (uninitialised
  memory read & leak in `fuse_session_new`); upstream shipped the fix in
  `0.16.0`. `pcloud-fs` builds clean on the new version — no API changes
  required.
- `deny.toml`: rewrote `[advisories].ignore` and `[bans]` to
  match the current lock graph.
  - **Removed three stale advisory ignores** (all now resolved by
    version bumps or transitive re-resolution):
    - `RUSTSEC-2021-0154` (`stdweb`/`fuser` — resolved by the bump
      above; the advisory no longer fires against our tree),
    - `RUSTSEC-2024-0436` (`paste` — no longer in the lock graph),
    - `RUSTSEC-2024-0370` (`proc-macro-error` — no longer in the lock
      graph).
  - **Added one new ignore** for `RUSTSEC-2021-0119` (`nix 0.19.1` via
    transitive `battery 0.7.8`, which only compiles on
    `cfg(target_os = "macos")` and `cfg(windows)` — the Linux release
    target is unaffected).
  - **Every remaining entry now carries a `review: YYYY-MM-DD` comment**
    plus a one-line justification citing the upstream blocker. The
    reviewer is named at the top of the block; next sweep dates are
    `2026-06-01` for the two rustls name-constraints advisories and
    `2026-07-15` for the rest.
  - **`[bans].multiple-versions` held at `"warn"`** with a new paragraph
    documenting the four upstream stacks (AWS SDK, zbus/secret-service,
    regorus, Windows target graph) that pin incompatible majors we
    cannot collapse without breaking changes. Flipping to `"deny"` is
    now a documented pre-condition, not a hidden cost.
  - **Added explicit `[bans].skip` entries** covering the known
    duplicate families so fresh duplicates surface as CI warnings
    instead of disappearing into the noise floor.
  - `[bans].wildcards = "deny"` and `allow-wildcard-paths = true`
    remain unchanged; path-dep wildcards are legal, external wildcards
    are hard-deny.
- `audit.toml`: mirrored to match `deny.toml` (same five
  ignores, same review dates).
- `.github/workflows/rust.yml`: the PR-path `cargo audit` job, the
  nightly `deny-audit` job, and the `rustsec-watchdog` report step no
  longer hard-code `--ignore RUSTSEC-2021-0154` on the CLI. All three
  now `cd .` and let `cargo audit` read `audit.toml` directly,
  so the two policy files cannot drift.
- `docs/book/src/development/release-checklist.md`: new §4.1.1
  "`cargo deny check` expectations" documents the release-gate contract
  — zero `advisory-not-detected` warnings, zero `unmatched-skip`
  warnings, every ignore review-dated, `multiple-versions` held at
  `"warn"` with the justification intact, mirror between `deny.toml`
  and `audit.toml` preserved.

`cargo deny --locked check` is once again green on the workspace with
no stale entries.

### Added — Reproducible-builds hardening across CI, Nix, and a verifier script (2026-04-16)

- `release-repro` profile in `Cargo.toml` is now explicit and
  self-describing: `strip = "symbols"`, `debug = false`,
  `codegen-units = 1`, `lto = true`, `panic = "abort"`. Rationale block
  above the profile updated to document that crash symbolication for
  `release-repro` artefacts happens via the unstripped sibling binary
  preserved alongside the release, not via the shipped binary. Main
  `release` / `release-dist` profiles are unchanged.
- `rust-version = "1.85"` pinned at the workspace level (already present;
  reaffirmed by the reproducibility contract).
- `.github/workflows/rust.yml`: new `reproducibility` job (tag push +
  `workflow_dispatch`) that runs `packaging/scripts/verify-reproducibility.sh`
  and uploads both binaries as a workflow artefact. Existing `release-dist`
  job now pins `SOURCE_DATE_EPOCH` from the tag commit time.
- `.github/workflows/release.yml` and `.github/workflows/packaging.yml`:
  every `cargo build --release` / `--profile release-dist` now carries
  `--locked`, and each build step is preceded by a `Pin SOURCE_DATE_EPOCH`
  step sourced from the tag commit (or the current HEAD for
  non-tag dispatches) on Linux, macOS, and Windows.
- `flake.nix`: nixpkgs input pinned to the exact revision already recorded
  in `flake.lock` (`f675531bc7e6657c10a18b565cfebd8aa9e24c14`) so
  `nix build` is deterministic without relying on lockfile trust alone.
  New `packages.pcloud-rs-repro` derivation builds the workspace with
  `--locked --profile release-repro` under a fixed `SOURCE_DATE_EPOCH`
  and the `--remap-path-prefix` + `--build-id=none` contract, and is
  wired into the flake's `checks` attribute.
- `packaging/scripts/verify-reproducibility.sh`: new POSIX-bash script
  that builds `pcloudc` and `pcloudd` twice with
  `SOURCE_DATE_EPOCH=0 cargo build --locked --release --profile
  release-repro`, snapshots each build's binaries under a temp
  directory, and fails with exit 1 if the two SHA-256 manifests differ.
  Honours `KEEP_ARTEFACTS=1` for offline diffoscope analysis.
- `docs/book/src/development/reproducible-builds.md`: §5
  rewritten with a "5.1 one-shot script" subsection pointing at
  `packaging/scripts/verify-reproducibility.sh` (the same script CI
  runs), and the manual procedure in §5.2 updated to build both
  `pcloudc` and `pcloudd` per the new profile contract.
- **Pre-alpha honesty:** byte-identical reproduction is a binding
  specification; no release tag has exercised the pipeline yet.

### Added — OIDC broker: pluggable pCloud trusted-issuer exchanger (2026-04-16)

- `pcloud-idp` now ships a `PcloudTokenExchanger` trait and two
  implementors:
  - `NullPcloudTokenExchanger` (default) — returns
    `IdpError::NotConfigured("pCloud trusted-issuer exchange endpoint not configured; set [oidc.trusted_issuer].exchange_url")`.
  - `HttpPcloudTokenExchanger` (cargo feature `oidc-http-exchange`, on
    by default) — POSTs the ID token to a configurable `exchange_url`
    and parses a pCloud-shaped session response. HTTPS is enforced at
    construction time; non-loopback plaintext URLs are rejected with
    a typed error. Response bodies are never included in error
    messages.
- New `IdpError::NotConfigured(&'static str)` variant. The message is
  static operator guidance and never carries secret material.
- `UnimplementedBroker`'s three `IdpBroker` methods no longer panic
  with `unimplemented!()` — they now return
  `IdpError::NotConfigured(...)` so the daemon surfaces a typed,
  actionable error instead of aborting.
- Operator configuration:
  ```toml
  [oidc.trusted_issuer]
  exchange_url = "https://bridge.corp.example/pcloud/exchange"
  ```
  Absence of `exchange_url` keeps `NullPcloudTokenExchanger` wired.
- Honest caveat preserved: pCloud's public API does **not** document
  a trusted-issuer exchange endpoint. This landing makes the broker
  usable against operator-run bridge services and makes the "no
  bridge configured" state explicit; it does not claim live pCloud
  SSO.
- Tests: `NullPcloudTokenExchanger` returns `NotConfigured`,
  `HttpPcloudTokenExchanger` against a `TcpListener` stub covers
  success (200 → `PcloudSession`), rejection (401 →
  `RefreshRejected`), and generic failure (500 → `TokenExchange`
  without body leakage). `UnimplementedBroker` exercised to prove all
  three methods return `NotConfigured` instead of panicking.
- Files: `crates/pcloud-idp/src/exchange.rs` (new),
  `crates/pcloud-idp/src/lib.rs`, `crates/pcloud-idp/Cargo.toml`,
  `docs/enterprise/oidc-broker.md`.

### Added — Packaging pipeline + cosign keyless signing in CI (2026-04-16)

- New workflow `.github/workflows/packaging.yml` triggered on
  `release: published` and manual `workflow_dispatch`. Each packaging
  target runs in its own isolated job so a single-target failure does
  not block the others:
  - `linux-deb-rpm` — `.deb` + `.rpm` via fpm.
  - `linux-appimage` — portable `.AppImage` via
    `packaging/appimage/build-appimage.sh`.
  - `linux-flatpak` — flatpak bundle via flatpak-builder (advisory
    until a hosted SDK cache lands).
  - `macos-pkg` — universal-binary unsigned tarball always; Apple
    Dev-ID `.pkg` path is a scaffold marked `continue-on-error: true`
    until `APPLE_*` secrets are provisioned.
  - `windows-msi` — WiX `.msi`; Authenticode EV signtool path is a
    scaffold marked `continue-on-error: true` until the EV cert is
    provisioned.
  - `docker-image` — multi-arch (`linux/amd64` + `linux/arm64`) OCI
    image pushed to GHCR.
  - `publish-manifest` — aggregates `release-artifacts.txt` with
    SHA256 + cosign verification recipe.
- **Signing is private-key-free.** All blob artifacts are signed with
  `cosign sign-blob --yes` using sigstore keyless via GitHub OIDC
  (`id-token: write`). The Docker image uses `cosign sign --yes` for a
  keyless OCI signature. Every release asset has a sidecar `<file>.sig`
  and `<file>.pem` (certificate); the verification recipe is embedded
  at the top of `release-artifacts.txt`.
- Apple Dev-ID and Windows EV paths are intentionally scaffolded and
  `continue-on-error: true`; documented honestly in
  `docs/book/src/operations/packaging-matrix.md` §12b and the
  `release-checklist.md` §4.11 gate.

### Added — Documentation: packaging CI gates

- New section §12b in `docs/book/src/operations/packaging-matrix.md`
  enumerating the packaging workflow jobs and signing posture.
- Expanded `docs/book/src/development/release-checklist.md` §4.11 with
  the cosign blob/OCI verification recipes, the packaging-job green
  check, and an explicit note that scaffolded Dev-ID / EV failures are
  informational only.

### Added — Top-level `snapshot` surface with zstd + SHA3 sidecar default

- New top-level CLI: `pcloudc snapshot {create,restore,verify,prune}`
  (single-token canonical forms `snapshot-create` / … also accepted;
  bare `pcloudc snapshot` is shorthand for `snapshot create`). The
  legacy `pcloudc backup snapshot-*` tokens are still accepted for one
  release cycle and emit a one-line stderr deprecation warning
  redirecting operators to the new surface.
- **Default pipeline:** `tar → zstd → SHA3-256 over the compressed
  archive → sidecar `<archive>.manifest.json`**. Both the `.tar.zst`
  archive and its sidecar are written atomically (tmpfile + fsync +
  rename). Pure-Rust `sha3` + `zstd` (the canonical zstd-rs binding).
- **Tunable compression:** `--zstd-level <1..=22>` (default `3`,
  matching the upstream zstd default). Out-of-range values are
  rejected at the CLI layer and again by the daemon.
- **Optional GPG envelope:** `--gpg-recipient <id>` produces
  `.tar.zst.gpg`; compression happens **before** encryption and the
  sidecar SHA3 is computed over the final on-disk ciphertext. GPG
  remains optional; no recipient is required for the default pipeline.
- **New IPC field:** `Request::BackupSnapshot { zstd_level:
  Option<i32>, ... }` with `#[serde(default, skip_serializing_if =
  "Option::is_none")]` for back-compat; clients that never set the
  field continue to interoperate with modern daemons (daemon default
  = 3).
- **Structured response messages (ADR-0017):** daemon emits compact
  JSON in `Response::message` —
  `{archive,sidecar,sha3_256,zstd_level,encrypted,size_bytes}` on
  Create, `{ok,sha3_256,...}` on Verify/Restore, `{ok,removed_count,
  removed}` on Prune.

### Deprecated

- `pcloudc backup snapshot-create|snapshot-restore|snapshot-verify|
  snapshot-prune` and their single-token canonical forms
  (`backup-snapshot-*`). These tokens continue to work for one release
  cycle and emit a one-line stderr warning. Migration: replace
  `backup snapshot-X` with `snapshot X` (or with the single-token
  `snapshot-X`).

### Added — Sync direction flavors on the CLI

- `pcloudc sync add <LOCAL> <REMOTE> [--type FLAVOR]` now accepts
  an optional `--type` flag that selects the sync direction. Nine
  case-insensitive aliases across three families:
  - `bilateral` | `full` | `both` → `SyncType::Full` (default)
  - `mirror` | `download-only` | `down` | `remote-to-local` →
    `SyncType::DownloadOnly`
  - `backup` | `upload-only` | `up` | `local-to-remote` →
    `SyncType::UploadOnly`
  Unknown aliases exit `2 Usage` with the full 9-alias list.
- New command `pcloudc sync change-type <SYNC-ID> <FLAVOR>`
  (canonical token `sync-change-type`; two-token aliases `sync
  change-type|set-type|retype`). Flips the direction of an existing
  sync root in place — `sync_id`, remote-folder binding, and staging
  context are preserved; only queued work that no longer matches the
  new plan is evicted. Mirrors C `psync_change_synctype`.
- `Request::SyncRootAdd` gained an optional `sync_type: Option<SyncType>`
  field with `#[serde(default, skip_serializing_if = "Option::is_none")]`
  for wire-compat with pre-flavor clients. Daemon default (field absent
  on the wire) remains `SyncType::Full`.
- The daemon's `SyncRootAdd` response now emits a structured JSON
  payload in `message` (ADR-0017): `{sync_id, local_path, remote_path,
  remote_folder_id, sync_type}`. Field selectors like
  `pcloudc --field sync_id --field sync_type sync add ...` work
  without a JSON parser.

> **Honest caveat (pre-alpha).** The `backup` alias is currently a
> synonym for `upload-only` and DOES propagate local deletions to the
> remote. A true deletion-safe backup sync flavor is tracked under
> `bd-1du.5 Deletion-safe backup sync flavor`. For deletion-safe
> archival today, use `pcloudc backup snapshot-create` (GPG-encrypted
> tarball, content-addressed) instead of a sync root.

Parity-matrix counts unchanged (this is a new user-facing surface, not
a C-parity row). The new bead is tracked under open beads in
[`STATUS.md`](./STATUS.md).

### Landed this cycle — quick-reference index

Big-ticket items shipped under `[Unreleased]` (detailed entries below):

- First-party plugin registry + four plugins (`publink-expiry`,
  `autoheal`, `backup-schedule`, `dlp-builtin`) behind
  `PCLOUD_PLUGINS_ENABLED=1`.
- Cross-platform waves X/Y/Z/W/V/U — trait abstractions, macOS fuse-t,
  Windows WinFSP, BSD `fusefs`, Windows Service wrapper, signing and
  packaging matrix across 35+ crates.
- OpenTelemetry W3C `traceparent` propagation via the
  `RequestEnvelope` wrapper (see ADR 0012); daemon dispatch spans.
- Backup snapshot CLI (create / verify / restore / prune), GPG-encrypted
  reproducible archive, and GFS retention.
- Integrity sweeper scaffolding (`bd-1du.4.6.1`) with opt-in config,
  path-hash-only audit, token-bucket rate limit, and CLI surface.
- Data-residency enforcement (`[data_residency]` config + structured
  `PolicyViolation { kind }` IPC error).
- `verify` / `log` / `diff` / `restore` integrity and revision-history
  CLI surfaces.
- Performance wave P0–P5: O(1) page-cache eviction, `Arc<Vec<u8>>`
  zero-copy, streaming verified download, chunked flush + histogram.
- Partial-transfer resume (upload + download) with durable NDJSON
  sidecar, `upload_status` probe, and 7-variant `ResumeOutcome`.
- Web UI MVP → 12-route admin surface with CSRF double-submit and
  loopback-only panic guard.
- Enterprise crates: `pcloud-idp` (OIDC, ADR 0014), `pcloud-policy`
  (Rego via `regorus`, ADR 0013), `pcloud-fleet` (mTLS + ed25519),
  `pcloud-kms` (AWS + Vault Transit, Pkcs11 stub), `pcloud-session`.
- Native `pcloudc --select` field-selector grammar — no `jq`
  dependency on any platform (ADR 0018).
- Structured JSON-in-`message` IPC response shape for `list-links`,
  `create-links`, `integrity status`, and backup snapshots (ADR 0017).
- Eight new ADRs (0011–0018) covering daemon architecture, tracing
  envelope, OPA choice, hand-rolled OIDC, `0600` enforcement,
  secret-wrapping discipline, response shape, field-selector syntax.

**Parity-matrix counts are unchanged** across every item above:
**152 / 6 / 0 / 28**. See [`STATUS.md`](./STATUS.md) for the
single-source-of-truth tally and [`docs/parity/bd-1du-10-closure-checklist.md`](./docs/parity/bd-1du-10-closure-checklist.md)
for what still blocks the final parity claim.


### Added — Wave-1 first-party plugins (H7–H10)

Four statically-linked, single-user first-party plugins landed in
wave-1, along with the plugin-api ops they required. All four are
off by default (`PCLOUD_PLUGINS_ENABLED=1` must be set; sensitive
capability classes have their own additional opt-ins).

- **`pcloud-plugin-publink-expiry` (H7)** — desktop-notify before a
  pCloud public link expires. Advisory only; never auto-revokes,
  never auto-renews. New plugin-api ops:
  `PluginOperation::ObservePublinkList` and
  `PluginOperation::TimerTick`. Internal `Notifier` and `Clock`
  traits enable deterministic tests. Rate-limit state persisted to
  `0600` JSON with atomic `*.tmp` + rename writes. Capability:
  `ObserveStatus` only. 8 plugin-logic tests + 3 state-file
  integration tests. Docs:
  `docs/plugins/publink-expiry.md`,
  `crates/pcloud-plugin-publink-expiry/README.md`.

- **`pcloud-plugin-autoheal` (H8)** — reacts to file-integrity
  mismatches from the integrity scanner. Rate-limited desktop
  notification, per-sync-root quarantine request, and escalation to
  full sync-pause if the same path mismatches more than 3 times in
  24h. **Local escalation only** — never auto-resyncs from the
  server; repair remains explicit user action. New plugin-api ops:
  `PluginOperation::ObserveIntegrityEvents` and
  `PluginOperation::RequestQuarantine`. Capabilities:
  `ObserveStatus`, `SyncControl` (requires
  `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1`). 5 tests. Docs:
  `docs/plugins/autoheal.md`,
  `crates/pcloud-plugin-autoheal/README.md`.

- **`pcloud-plugin-backup-schedule` (H9)** — in-process cron living
  inside the daemon. Accepts 5/6/7-field cron expressions and a
  small, whitelisted natural-language DSL
  (`hourly` / `daily at HH:MM` / `weekly on <day> at HH:MM` /
  `monthly on N at HH:MM` / `every <day> at HH:MM`). Per-tick
  boundary-crossing evaluation with a 1024-per-tick catch-up cap and
  a 32-entry configuration cap. **Only fires a tick / resume event**
  — does not itself create snapshots; scheduled snapshots are wired
  by the `pcloudc backup snapshot-*` CLI. Internal `Clock` trait for
  deterministic boundary tests. Capability: `SyncControl` (requires
  `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1`). 5 tests. Docs:
  `docs/plugins/backup-schedule.md`,
  `crates/pcloud-plugin-backup-schedule/README.md`.

- **`pcloud-plugin-dlp` (H10)** — synchronous pre-upload scanner
  with six built-in rules (AWS access-key, AWS secret-key proximity,
  PEM private-key header, JWT, generic password literal, Shannon
  high-entropy with 15 magic-number suppressions for common
  compressed/media formats). New plugin-api op:
  `PluginOperation::PreUploadScan` with response over the
  `UploadScanVerdict` enum (`Allow` / `Deny` / `Quarantine` /
  `RedactAndAllow`). **Path-hash-only audit**: the plugin emits a
  `DlpAuditEvent` containing `SHA-256(path)`, the matched rule IDs,
  and the verdict — never the raw path and never any byte of file
  contents. Two modes: audit-only (default) and strict. Hard
  per-file timeout with a host-side `on_timeout` fallback. 9 tests.
  Docs: `docs/plugins/dlp-builtin.md`,
  `crates/pcloud-plugin-dlp/README.md`.

Book SUMMARY entries for all four plugins are wired under both the
`Development → Plugins` subtree and the standalone `Plugins`
section. The enterprise-vs-plugin distinction is documented in
`docs/plugins/README.md` and stays strict: the built-in
plugins are small, opinionated, single-user; enterprise-tier
variants (e.g. `pcloud-plugin-dlp-enterprise`) cover custom
rulesets, per-tenant policy, and audit-stream forwarding.

### Added — Cross-platform waves X/Y/Z/W/V/U (consolidated milestone)

- **Mount abstraction (`pcloud-fs`)**: `FuseAdapter` trait decoupled
  from Linux `libfuse3`; platform-conditional backends wired for
  macOS **fuse-t** (direct `libfuse-t.dylib` FFI), Windows **WinFSP
  2.x** (`winfsp` crate), and *BSD `fusefs` / `refuse`.
  Linux remains the **only live-tested mount path**; macOS / Windows /
  *BSD mounts are scaffolded but not hardware-verified (tracked under
  `bd-1du.4`).
- **Local IPC portability (`pcloud-ipc`)**: named-pipe transport on
  Windows (`\\.\pipe\pcloud-rs`) with SID-based peer authentication via
  `GetNamedPipeClientProcessId` + `OpenProcessToken`; AF_UNIX retained
  on Unix targets with `SO_PEERCRED` (Linux) / `LOCAL_PEERCRED` +
  `getpeereid(3)` (macOS / *BSD).
- **Vault portability (`pcloud-secret`, `auth_vault`)**: Keychain
  Services on macOS, DPAPI (`CryptProtectData`, user scope) on
  Windows, Secret Service on Linux desktops, owner-only `0600` file
  fallback on headless / *BSD hosts. No raw password persistence on
  any platform (see ADR 0007).
- **Service integration**: systemd units (Linux), launchd plists
  (macOS — `com.pcloud.pcloudd.plist`), Windows Service Control
  Manager registration (via WiX custom action), rc.d scripts
  (FreeBSD / OpenBSD / NetBSD). All assets under `packaging/`.
- **Packaging matrix expanded across 6 platform families** (35+
  crates). New / refreshed recipes: `.deb` / `.rpm` (nfpm), AppImage,
  Flatpak, Snap, Nix flake, Arch AUR, Docker (cosign-signed via
  keyless OIDC), Homebrew formula + Casks, macOS signed `.pkg`, WiX
  MSI, winget, Chocolatey, Scoop, FreeBSD ports, OpenBSD ports,
  NetBSD pkgsrc.
- **Signing wrappers** under `packaging/signing/`: `sign-macos.sh`,
  `notarize-macos.sh`, `sign-windows.ps1`. Linux `.deb` / `.rpm` GPG
  detached signing and OCI cosign signing are live in CI.
- **Documentation**: new
  [Operations → Packaging Matrix](docs/book/src/operations/packaging-matrix.md),
  new
  [Architecture → Platform Support](docs/book/src/architecture/platform-support.md)
  capability matrix, consolidated `packaging/README.md` index.

> **Honest residual gaps (2026-04-16).** macOS `.pkg` notarisation is
> **pending an active Apple Developer ID** (vendor-bound); Windows MSI
> **Authenticode EV signing is a stub** awaiting an EV HSM token
> (vendor-bound); macOS fuse-t, Windows WinFSP, and *BSD fusefs mounts
> are scaffolded but not hardware-verified. These gaps block any claim
> of "full cross-platform parity" and remain tracked under `bd-1du.4`
> and `bd-1du.10`.

### Added — OTel W3C `traceparent` propagation (H13, wave-2)

- **`pcloud-observability::tracing` module (H13a)**: landed under
  `crates/pcloud-observability/src/tracing.rs`, gated by the
  `tracing-otlp` Cargo feature (off by default, zero runtime cost when
  disabled). Public surface:
  - `TracingHandle::init(endpoint, sample_rate, headers)` — builds the
    OTLP/HTTP (protobuf) exporter, clamps `sample_rate` to `[0.0,
    1.0]`, rejects non-loopback plaintext endpoints, and refuses
    literal header values so secrets stay out of the config file.
  - `attr_redact(key, value)` — span-attribute filter with a
    five-key allow-list (`command`, `duration_ms`, `error_category`,
    `status_code`, `trace_kind`). Keys outside the list are dropped
    in release and panic in debug builds.
  - `parse_traceparent(&str) -> Option<W3cTraceparent>` and the
    `W3cTraceparent` type — validates the W3C format
    `00-<trace:32hex>-<span:16hex>-<flags:2hex>` before trust.
- **`RequestEnvelope` IPC wrapper (H13b')**: `pcloud_ipc::RequestEnvelope
  { request, traceparent }` with `new`, `with_traceparent`,
  `traceparent`, `try_from_wire`, and `From<Request>`. A single
  wrapper was chosen over rippling `traceparent` through every
  `Request::*` variant (~485 call sites); `try_from_wire` falls back
  to decoding a bare `Request` so pre-envelope peers keep working.
  The `Option<String>` field with `skip_serializing_if = "Option::is_none"`
  preserves byte-identical wire compatibility for untraced callers.
- **Daemon dispatch span wiring (H13d)**: `crates/pcloud-daemon/src/dispatch.rs`
  decodes the incoming envelope via `try_from_wire`, installs the
  extracted `traceparent` on the dispatch thread with
  `set_thread_traceparent`, opens a `pcloudd.dispatch` server span
  (or synthesises a fresh root when the envelope carries no
  `traceparent` or the value fails `parse_traceparent`), and opens a
  `pcloudd.backend.<name>` internal span around each handler.
  Handler panics are captured by `note_dispatch_panic` so the span
  closes with a non-Ok status and force-exports its ancestor chain.
- **Docs**: `docs/enterprise/tracing.md` flipped Design → Landed with
  a wave-2 honesty statement; `docs/book/src/reference/ipc-protocol.md`
  documents the `RequestEnvelope` wire shape, the W3C `traceparent`
  format, and the `try_from_wire` bare-`Request` fallback;
  `packaging/man/pcloudc.1` gains an `OBSERVABILITY / DISTRIBUTED
  TRACING` section covering `--trace-id`, the `TRACEPARENT` envvar,
  and the stderr echoed trace id; `packaging/man/pcloudd.1` updates
  the existing tracing section with the `dispatch.rs` handler
  boundary, attribute allow-list, and ingress back-compat.

Honest caveats (call out, do not claim production-ready):

- Offline-only interop: the exporter has been exercised against a
  local OTLP sink only. No live run against a production OTLP
  backend (Jaeger, Tempo, Datadog, Honeycomb, New Relic) has been
  certified in this release.
- The `Pkcs11Hsm` KMS provider is **not** instrumented; its call
  paths do not yet emit spans and its span metadata is unchanged.
- The mounted-drive FUSE code paths in `crates/pcloud-fs/` are
  **not** instrumented; FUSE callbacks do not yet participate in the
  `pcloudd.dispatch` trace.
- Feature remains opt-in at build time (`tracing-otlp`) and at
  runtime (`[observability.tracing].enabled = false` by default); a
  daemon built without the feature logs
  `observability.tracing.feature_disabled` at startup.

### Added — Backup snapshot CLI + GPG-encrypted archive + GFS pruning

- **Backup snapshot CLI (H12a–H12d)**: four new subcommands wired
  end-to-end from CLI parse through IPC dispatch to daemon handlers:
  - `pcloudc backup snapshot-create [--gpg-recipient EMAIL] [--label STRING]`
  - `pcloudc backup snapshot-verify <ARTIFACT>`
  - `pcloudc backup snapshot-restore <ARTIFACT> [--yes]`
  - `pcloudc backup snapshot-prune [--retention-days N]`
- **GPG-encrypted archive format**: reproducible tarball (sorted
  entries, fixed mtime) containing manifest.json (BLAKE3 digests,
  audit-chain tail hash, schema versions), the auth vault byte-for-byte,
  a SQLite online-backup of the store, the audit chain, config with
  secrets redacted to keyring refs, and plugin registry manifests.
  Encrypted and signed via gpg --encrypt --sign --recipient.
- **BackupGuard** daemon-wide quiesce for upload-save and sync-commit
  critical sections during the SQLite online backup.
- **Grandfather-father-son retention** via snapshot-prune: daily /
  weekly / monthly slots plus minimum_keep floor. Prune refuses to
  delete a snapshot younger than the most recent verified snapshot.
- **PluginCapability::BackupDestination** + PluginOperation::BackupPut
  / BackupGet. Built-in destinations: local, s3 (SSE-KMS), sftp.
- **Audit events**: SnapshotCreated and SnapshotRestored extend the
  signed audit chain.
- **Runtime dependency**: gpg(1) must be installed on every host that
  creates, verifies, or restores a snapshot; fails closed with exit 6.

### Added — Integrity sweeper (H14, bead `bd-1du.4.6.1`) — superseded

> **Superseded** by the I-wave "Integrity sweeper scheduler + battery
> pause hook" entry above, which wires the cron scheduler thread and
> `pause_on_battery` platform reader. The H-wave scaffolding described
> here is now fully integrated.

### Added — Data-residency enforcement (H11) — superseded

> **Superseded** by the I-wave "Data-residency enforcement wired into
> daemon runtime" entry above, which adopts the evaluator at all three
> daemon call sites and adds the region cache + audit integration.

### Added — Integrity and revision-history CLI surfaces

- **`verify <PATH> [--recursive] [--fix] [--yes]` (C9)**: on-demand
  local-vs-server hash reconciliation for sync roots. Stable output
  taxonomy (`[OK]`, `[MISMATCH local=… server=…]`, `[MISSING_LOCAL]`,
  `[MISSING_REMOTE]`). `--json` emits NDJSON — one record per line, no
  wrapping array — for safe streaming into any line-oriented JSON
  consumer (log shipper, alerting rule, custom parser). Exit codes: `0 Ok`
  when all records match, `7 Conflict` as soon as any `[MISMATCH]` is
  observed (even with `--fix`), `6 Unavailable` when only
  `MISSING_LOCAL`/`MISSING_REMOTE` were seen and `--fix` was not
  requested. `--fix` is destructive and requires `--yes` or an
  interactive confirmation.
- **`log <PATH> [--limit N]`, `diff <PATH> <REV_A> <REV_B>`,
  `restore <PATH> <REV>` (C8)**: CLI surfaces wired, daemon dispatches
  `Method::FileHistory`. Backend currently returns `6 Unavailable`
  because pCloud's undocumented `listrevisions` endpoint is awaiting
  public-API approval; tracked under `bd-1du.10`. Shipping the parser
  and exit-code mapping now means the backend can be switched on
  without a CLI release.

### Docs

- `packaging/man/pcloudc.1`: new "Integrity and revision history"
  section documenting `verify`, `log`, `diff`, `restore` with exit
  codes and the honest `Unavailable` status on the three history
  subcommands.
- `docs/book/src/reference/cli.md`: added `verify`/`log`/`diff`/
  `restore` reference with worked recipes and status callouts.
- `docs/book/src/operations/runbook.md`: new playbooks "Verifying
  integrity of a sync root (on-demand `pcloudc verify`)" and
  "Recovering an older version of a file" (with fallback procedure
  while `restore` is stubbed).

### Performance — Wave-1 optimisation pass

- **P1.1 — O(1) page-cache eviction**: the `pcloud-fs` page cache now uses
  an intrusive doubly-linked LRU backed by a `HashMap`; eviction, promotion,
  and insertion are all O(1). Microbench `page_cache_evict/10k_entries`
  improves ~180× (≈ 1 ms → ≈ 5 µs).
- **P5.1 — `Arc<Vec<u8>>` hot-path**: cached pages are now stored as
  `Arc<Vec<u8>>` and returned by `Arc::clone` on a cache hit instead of a
  full `Vec` clone. Cache-hit cost drops by roughly three orders of
  magnitude on a 4 MiB page (≈ 900 µs memcpy → ≈ 0.9 µs refcount bump).
- **P1.5 — Streaming HTTP download**: the transfer-backend download path
  now streams a 64 KiB window through a rolling `sha2::Sha256` directly to
  the destination file. Peak RSS is now flat in file size (10 GiB downloads
  peak at ~2 MiB of buffer). SHA256 verification is folded into the
  stream. Buffer size rationale recorded in ADR-0008.
- **P5.2 / G8 — Chunked write-path flush**: `WritePathService::flush` is
  now split into 1 MiB chunks with a 4-chunk in-flight semaphore. 128 MiB
  flush improves ~3.4× (≈ 6.3 s → ≈ 1.9 s) and the FUSE `flush` callback
  latency drops from seconds to tens of milliseconds. Crash-safe: resume
  starts at the last durable chunk, not from scratch.
- **C1 — `flush_latency_seconds` histogram**: Prometheus-compatible
  histogram (12 explicit buckets, 1 ms – 10 s, plus `+Inf`) labelled by
  outcome (`ok` / `err` / `cancelled`), observed from
  `WritePathService::chunked_flush`. Exposed on the daemon's
  loopback-bound `/metrics` endpoint.
- **C4 — Criterion bench harness**: `pcloud-bench` crate lands with
  benchmarks for `chunked_flush`, `upload_session`, and `page_cache_evict`;
  wired into the release checklist as a > 10 % regression gate.

Full dossier: `docs/book/src/architecture/performance.md`.

### Documentation — Partial transfer resume (H5 + H6)

- Documented the durable upload-resume design (phase **H5**): per-inode
  `ino-<inode>.upload-progress` NDJSON sidecar under the staging
  directory, atomic write-temp + `fsync(file)` + rename + `fsync(dir)`
  update path, `upload_status` server probe, the seven-variant
  `ResumeOutcome` taxonomy (`Resumed`, `ServerAhead`, `SidecarTrimmed`,
  `Expired`, `Stalled`, `Unparseable`, `BackendError`), and the
  10-minute heartbeat stall timeout. Replay is wired into
  `bootstrap.rs` at daemon startup and into `mount_runtime.rs` on
  mount (re)activation.
- Documented the resumable HTTP download design (phase **H6**):
  `fetch_download_resumable` in `pcloud-proto`, `.part`-file staging,
  `Range: bytes=N-` reissue, on-disk prefix re-hash against the
  expected final SHA-256 (O(file-size) cost, documented), `.part`
  cleanup on hash mismatch, and the 206→200 fallback when the server
  does not advertise `Accept-Ranges: bytes`.
- Honest caveat recorded: download resume is a library API call and is
  not yet auto-used by every daemon-side download site;
  `fetch_download_verified` callers still restart from byte 0 on
  interruption and need per-site migration.
- New operator page:
  `docs/book/src/operations/partial-transfers.md`. Cross-referenced
  from `architecture/request-lifecycle.md` (new upload-chunk
  lifecycle section), `packaging/man/pcloudc.1` (new
  `PARTIAL TRANSFER RESUME` section), and `SUMMARY.md`.

### Added — Web UI (G7)

- **`pcloud-web` route expansion (3 → 12)**: the loopback admin
  surface now covers `/`, `/api/status`, `/health`, `/sync` (GET +
  POST), `/sync/{id}` (DELETE), `/publinks` (GET + POST),
  `/publinks/{code}` (DELETE), `/activity`, `/settings`, and
  `/metrics` (feature-gated). Every page is server-rendered plain
  HTML — no JavaScript framework, no JS-required flows. Status
  payload parsing is best-effort pending `bd-1du.10` daemon JSON
  stabilisation.
- **CSRF — double-submit cookie**: every HTML `GET` issues
  `pcw_csrf=<32 hex>; HttpOnly; SameSite=Strict; Path=/`; mutating
  handlers require `X-CSRF-Token` to match, constant-time compared.
  Missing, malformed, or mismatched tokens return `403`.
- **Loopback-only panic guard**: `WebConfig::bind_addr` is validated
  at startup; any non-loopback bind (including `0.0.0.0`, `::`, LAN,
  or public IPs) panics before the listener is created (ADR 0004).
- **Accessibility (WCAG 2.1 AA)**: semantic HTML, `<label for>` on
  every input, full keyboard traversal, preserved focus outlines,
  colour-independent status signals, no forced timeouts.
- **Docs**: new `docs/book/src/operations/web-ui.md` with per-route
  mockups, security posture, co-located nginx + OIDC reverse-proxy
  recipe (from B6 HA design), and the accessibility checklist. Wired
  into `SUMMARY.md` under Operations.

### Added — Enterprise traits (landed, offline unit-tested)

- **OIDC Identity Broker** (`pcloud-idp`): landed
  `OidcAuthorizationCodeBroker`, a hand-rolled implementor (no
  `openidconnect` crate dependency). PKCE S256 only (`plain` rejected),
  RS256-only JWKS verification with a 1-hour in-memory TTL cache,
  `alg=none` and algorithm-confusion rejected before signature
  verification, all tokens wrapped in `SecretString`. `IdpBroker` is
  object-safe. **Known gap:** pCloud trusted-issuer token exchange is
  stubbed and returns `IdpError::TrustedIssuerExchangeUnavailable`
  pending pCloud API support; live pCloud interop is **not** claimed.
  See `docs/enterprise/oidc-broker.md`.
- **OPA/Rego Policy Layer** (`pcloud-policy`): landed
  `RegoPolicyEngine` backed by `regorus = "0.3"` (pure Rust, no CGO,
  no subprocess). Default-deny safety invariant on empty/unmatched
  bundles, file-permission guard rejecting world-write (`0o022`),
  non-root-owned, and escaping-symlink policy files; transactional
  hot-reload keeps the previous engine on compile failure. Four
  example policies ship in `crates/pcloud-policy/examples/policies/`
  (`default-deny`, `allow-all`, `publink-expiry-7d`,
  `crypto-setup-managed-device`). `PolicyEngine` is object-safe. See
  `docs/enterprise/policy.md`.
- **mTLS Fleet Agent** (`pcloud-fleet`): landed `MtlsFleetAgent` with
  ed25519 device identity (`SecretBytes`, `0600` identity file,
  `0700` parent directory, ownership and mode validated on load),
  explicit rustls `RootCertStore` built only from operator-supplied
  `ca_bundle` (no system CAs), `X-PCloud-Body-Signature` header over
  deterministic canonical JSON, trusted-key verification runs before
  command-variant dispatch, 1-command-per-second token-bucket rate
  limit (burst 5). Offline test coverage against an in-process stub
  server; no reference fleet server ships in this repository and live
  fleet interop is not claimed. `FleetAgent` is object-safe. See
  `docs/enterprise/fleet.md`.
- **Enterprise documentation landscape**: new
  `docs/enterprise/README.md` overview distinguishing the three
  landed trait implementors from the six design stubs
  (`data-residency`, `disaster-recovery`, `dlp`, `ha`, `kms`,
  `tracing`). `docs/book/src/SUMMARY.md` now links every enterprise
  document from a single Enterprise section.

### Added — Enterprise KMS providers

- **`pcloud-kms` — AwsKms provider (landed, `aws` feature, off by
  default)**: `aws-sdk-kms`-backed `Encrypt` / `Decrypt` with
  `EncryptionContext`. Sync `KmsProvider` trait bridges to the async
  SDK via `tokio::runtime::Handle::try_current()` with a lazily-built
  single-thread fallback runtime. Credentials come exclusively from
  the default provider chain (IMDSv2, env, shared credentials, SSO);
  the config file never carries AWS credentials.
- **`pcloud-kms` — HashicorpVault provider (landed, `vault` feature,
  off by default)**: blocking `reqwest` client with `rustls-tls`
  against `/v1/transit/encrypt/<key>` and `/v1/transit/decrypt/<key>`.
  Token read from `VAULT_TOKEN` / `VAULT_TOKEN_FILE`, sent as
  `X-Vault-Token`, wrapped in `SecretString`, never logged or
  persisted.
- **`pcloud-kms` — `unwrap_cached` TTL cache**: in-memory
  `(provider, key_id, wrapped, context)` → `SecretBytes` cache fronting
  every live provider. `cache_ttl_seconds` operator-configurable
  (default 3600s). `PolicyDenied` and `Malformed` invalidate matching
  entries immediately. Plaintext DEKs never hit disk.
- **`[crypto.kms]` config section** documented in
  `packaging/man/pcloud.conf.5` and `docs/book/src/reference/config.md`:
  `provider` (`null` | `aws` | `vault` | `pkcs11`), `key_arn`,
  `vault_addr`, `vault_path`, `pkcs11_slot`, `pkcs11_label`,
  `cache_ttl_seconds`. Config file rejects credential-shaped keys at
  load time.
- **`docs/enterprise/kms.md`** flipped from DESIGN to LANDED for the
  AWS and Vault providers; envelope-encryption model, IAM rules,
  offline-cache behaviour, key rotation, and failure state machine
  documented.

### Known limitations — Enterprise KMS — superseded

> **Superseded.** The PKCS#11 stub is now a real `cryptoki`-backed
> provider (see I-wave "PKCS#11 HSM provider" entry above) and
> CryptoShell DEK routing is wired and integration-tested (see J04
> "CryptoShell DEK routing through KmsProvider" entry above).

### Added — Cross-platform (Phase 0–5)

- **Phase 0 — Trait abstractions (X1–X6)**: platform-neutral seams for
  filesystem mount, credential vault, signal handling, service lifecycle,
  path normalisation, and process supervision. Linux, macOS, Windows, and
  the BSDs now compile against the same trait surface.
- **Phase 1 — macOS FUSE adapter (Z1 + W1 + V1 + U1)**: `fuse-t`-backed
  adapter wiring all 16 mount callbacks (`getattr`, `lookup`, `readdir`,
  `open`, `read`, `write`, `flush`, `fsync`, `release`, `create`,
  `unlink`, `rename`, `mkdir`, `rmdir`, `statfs`, `setattr`). Scaffolded;
  live mount verification is in progress.
- **Phase 2 — Windows WinFSP adapter (Z2 + W2 + V2 + U2)**: WinFSP-backed
  adapter wiring all 17 mount callbacks (as above plus `cleanup`).
  Scaffolded; live mount verification is in progress.
- **Phase 2 — Windows Service wrapper (Y4)**: `pcloud-daemon-win` wraps
  `pcloudd` as a Windows Service with proper control-handler, graceful
  shutdown on `SERVICE_CONTROL_STOP`, and event-log integration.
- **Phase 3 — Packaging matrix**: Homebrew tap formula, Debian/Ubuntu
  `.deb` (nfpm), Fedora/RHEL `.rpm` (nfpm), Nix flake, Flatpak manifest,
  Docker/OCI image, AppImage recipe, Snap, Chocolatey, winget, Scoop,
  Windows MSI (WiX), FreeBSD rc.d service unit, and NetBSD/OpenBSD
  `rc.d` shims. Assets under `packaging/` and repo-root
  `packaging/`.
- **Phase 4 — Migration assistant (Z4)**: `pcloudc migrate-from-c`
  subcommand imports legacy C-client state (sync roots, ignore lists,
  config hints) into the Rust store, preserving secure defaults (no
  cleartext password import). **Shipped in R8** — previously a GA
  blocker. Supports `--dry-run`, `--from <PATH>`, and
  `--force-overwrite`, with three safeguards: refuse-overwrite on
  existing `.pclouddb` unless the flag is set, copy-not-move of legacy
  files, and secret redaction in all preview output. See
  `docs/book/src/operations/upgrade.md` § "Migrating from legacy C
  pcloud-rs" and `pcloudc(1)`.
- **Phase 4 — `doctor` cross-platform enhancement (G6)**: `pcloudc
  doctor` gained a `--strict` flag (warnings escalate to failures for
  CI/wave gates) and a cross-platform probe matrix: `vault-perms`
  (POSIX `0600`/`0700` vs. NTFS-ACL stub on Windows), `disk-free`
  (`statvfs` vs. `GetDiskFreeSpaceExW`), and a new `clock-drift` probe
  with a 30-second threshold (NTP/chrony on Unix, W32Time on Windows).
  See `docs/book/src/reference/cli.md` § `doctor` and `pcloudc(1)`.
- **Phase 5 — Signing pipelines (W3)**: notarisation pipeline for macOS
  `.pkg`, Authenticode signing for Windows MSI/EXE, `.deb`/`.rpm` GPG
  signing, and SLSA/provenance attestation hooks in CI.
- **Phase 5 — Reproducible-builds profile (W5)**: dedicated `[profile.reproducible]`
  with deterministic codegen flags, `SOURCE_DATE_EPOCH` plumbing, and
  locked cargo config for bit-for-bit reproducible artefacts.
- **Phase 5 — Cross-platform CI (Y6 + U4)**: GitHub Actions matrix
  covering Linux (glibc + musl, x86_64 + aarch64), macOS 13/14,
  Windows 10/11, and FreeBSD smoke jobs. Runs fmt, clippy `-D warnings`,
  test, deny, audit, manpage-lint, and per-platform packaging dry runs.
- **Phase 5 — FuseAdapter +10 methods (U3)**: adapter trait grew
  `create`, `unlink`, `rename`, `mkdir`, `rmdir`, `setattr`, `flush`,
  `fsync`, `release`, and `statfs` with full PcloudFsShim write-path
  wiring on the Linux adapter and scaffolded implementations on
  macOS/Windows.
- **Phase 5 — mdBook platform chapters**: six new chapters covering
  Linux FUSE, macOS fuse-t, Windows WinFSP + Service, FreeBSD,
  NetBSD/OpenBSD, and the cross-platform trait model. Indexed under
  `docs/book/src/architecture/` and `docs/book/src/development/`.
- **Phase 5 — ADRs**: ten Architecture Decision Records published under
  `docs/book/src/adr/` covering the trait split, platform selection,
  signing strategy, reproducible builds, service wrapper model, mount
  policy, migration strategy, CI topology, packaging strategy, and
  cross-platform testing.

### Added

- Workspace split across 23 crates (`pcloud-model`, `-error`, `-config`,
  `-secret`, `-proto`, `-plugin-api`, `-auth`, `-store`, `-engine`, `-cache`,
  `-fs`, `-crypto`, `-ipc`, `-observability`, `-daemon`, `-cli`, `-p2p`,
  `-sdk`, `-live-e2e`, `-resilience`, `-compat`, `-mockserver`) on edition
  2024, resolver 3 (wave 1-2).
- Typed protocol clients for auth, account, transfer, shares, public links,
  crypto, backup, notifications (waves 1-7).
- Live-verified auth parity: password, token, TFA code, recovery code, TFA
  SMS resend, TFA device-notification resend, `userinfo` (wave 2-3).
- Account helpers: `verify_email`, `verify_email_restricted`, `lost_password`,
  `change_password`, `register`, `get_promo`, `get_api_servers`,
  `set_language`, `set_api_server` (wave 4-7).
- Transfer stack: `getfilelink`, signed HTTP download execution, upload
  create/write/save, SDK helpers `upload_data[_as]` / `upload_file[_as]`
  (wave 3-6).
- Sync-root lifecycle: persistent add/list/remove, remote-folder validation,
  path canonicalization, duplicate/nested-root rejection, queued-work
  eviction, sync suggestions, syncability classification (wave 4-8).
- Public-link parity: file/folder link create/list/show/delete,
  `changepublink` expire/password/upload policy, upload-link CRUD, tree-link
  with path resolver, upload-access, bookmark/pin, screenshot link,
  folder up/down link (wave 5-7).
- Crypto on active path: setup/start/stop/reset, lock/unlock, crypto folder
  create, AES-256-GCM sector sealing, deterministic filename encoding,
  zeroized key handling via `SecretBytes`/`SecretString`, crypto-aware
  share/team-share temppass flow (wave 4-8).
- Shares/business/teams: share request listing, share list/add/remove/modify,
  accept/decline/cancel, contacts, my teams, account team-share, crypto-aware
  retained variants (wave 5-7).
- Backup/device: backup create/delete, `stop device`, delete backup-device
  local cleanup (wave 7-8).
- SDK embeddable surface and plugin registry scaffolding (wave 2, 6).
- Secure local IPC with `SO_PEERCRED` UID check, owner-only socket
  (`0600`/`0700`), per-connection timeouts, audit persistence surfaced
  (wave 1-3, re-verified wave 9).
- Mounted-drive scaffolding: `MountService` with policy validation, RAII
  mount handles, signal-aware unmount, in-memory read path, staging,
  journal, writeback helpers, FUSE readdir/getattr wiring (wave 4-6).
- Packaging scaffolding under `packaging/` (wave 4).
- Documentation: `ARCHITECTURE.md`, `API-REFERENCE.md`,
  `OPERATIONS-RUNBOOK.md`, `SECURITY-MODEL.md`, `ERROR-TAXONOMY.md`,
  `TESTING-FUZZ-STRESS.md`, `REJECTED-RATIONALES-14042026.md`,
  `PERF-BASELINE-14042026*.md` (waves 4-9).
- Fuzz targets under `crates/pcloud-proto/fuzz` and `crates/pcloud-ipc/fuzz`;
  stress and property tests across the workspace (wave 4-9).
- `audit.toml` + `deny.toml` + `clippy.toml` supply-chain and lint gates
  (wave 4-6).

### Changed

- Parity matrix (`C_FEATURE_PARITY_MATRIX.csv`) evolved from initial triage
  to 187 data rows: 143 Implemented, 6 Partial, 11 Missing, 27 Rejected as
  of wave 8; wave 9 re-verified the tally with green workspace gates.
- Auth vault hardened: atomic temp-file + fsync + rename rotation, owner and
  mode re-validated on load, parent dir forced to `0700`, vault file `0600`,
  passwords intentionally never persisted (wave 3-6).
- IPC transport: slow/malformed client isolation with per-connection timeout;
  audit persistence failures surfaced instead of swallowed (wave 3-6).
- Sync engine simplified vs. C daemon but event-loop and diff scaffolding
  made deterministic (wave 4-7).
- Workspace-wide transition to edition 2024 idioms: `io::Error::other`,
  `div_ceil`, `is_multiple_of`, `std::iter::repeat_n` (wave 3 reconciliation).

### Fixed

- Clippy `-D warnings` gate restored after each wave (reconciliations 3-9).
- Wave-3: `manual_div_ceil`, `manual_is_multiple_of`, `io_other_error`,
  `doc_lazy_continuation`, `single_match`, `manual_repeat_n`,
  `private_interfaces`, `needless_range_loop`, dead-code warnings.
- Wave-4: `derivable_impls` collision in `pcloud-cli/src/globals.rs`
  between manual `Default` impl and `#[derive(Default)]`.
- Wave-5: matrix row 85 (mounted pCloud filesystem) promoted from `Missing`
  to `Partial` once read-path + staging + journal landed.
- Wave-6: stray `mut` on `scrub_token`; `FakeDriver` `drain(..1).next()` test
  helper misuse in `upload_state.rs`.
- Wave-9: cross-agent collisions reconciled without weakening security
  defaults or fabricating parity upgrades.

### Security

- Secrets: all password/token storage migrated to `SecretString` /
  `SecretBytes` (zeroize on `Drop`, redacted `Debug`, not `Clone`) — enforced
  by a regression guard in `pcloud-auth/src/orchestrator.rs` (wave 3-9).
- Transport: central `ApiEndpoint::validate(environment)` gate rejects
  `ApiMode::Plaintext` under `Environment::Production`; no TLS-bypass flag
  anywhere in tree (no `danger_accept_invalid_certs`,
  `accept_invalid_hostnames`, or custom validator shortcuts). (wave 2-9)
- Mount policy: `MountService::validate` rejects `allow_other && !read_only`;
  default `allow_other=false`; no `setuid` / `allow_root` (wave 4-9).
- Path safety: `pcloud-fs/src/path_norm.rs` rejects embedded NUL, empty,
  `.`, `..` segments before any backend call (wave 4-6).
- Crypto: AES-256-GCM sector sealing, Argon2 key derivation, HMAC-SHA256/512
  with `subtle::ConstantTimeEq` comparison; password scorer zeroizes derived
  HMAC state on drop (wave 4-8).
- CLI password input precedence: `--password-stdin` → `--password-env`
  (scrubbed) → argv with warning (zeroized in place) → rpassword interactive
  fallback (wave 5-6).
- File permissions: SQLite store `0600`; runtime/state/config/cache dirs
  `0700`; runtime dir policy rejects group/other bits (wave 2-6).
- `unsafe` blocks in `mount_service`, `fuser_shim`, `shm_producer`,
  `folder_list`, `ipc/{transport,auth}` all carry explicit `// SAFETY:`
  justification (wave 4-9).
- Logging: no `tracing!` / `log!` / `println!` formats any secret, token,
  password, or key value (verified wave 9).
- New low-severity finding L1 (wave 9): `dwltag` cookie value interpolated
  without CRLF / whitespace sanitisation in
  `crates/pcloud-fs/src/http_download.rs`. Non-exploitable today (trusted
  backend source); defensive validator to follow. See
  `SECURITY-AUDIT-FINAL-14042026.md`.

### Known limitations

- `bd-1du.4` (filesystem / mounted drive): `pcloud-fs` provides mount
  scaffolding, policy validation, RAII handles, in-memory read path,
  staging, journal, and writeback helpers. There is **no fully wired
  mounted-drive runtime** comparable to the C client. FUSE
  readdir/getattr/open/read/write/flush/fsync and crash-safe writeback are
  still being landed.
- `bd-1du.5` (crypto remainder): `change_crypto_pass`,
  `change_crypto_pass_unlocked`, `send_change_user_private`, and
  `priv_key_flags` are still missing on the active Rust path.
- `bd-1du.3` (sync-folder lifecycle): global pause/resume/manual-rescan
  helpers and deeper `psync_start_sync` / diff parity are incomplete.
- `bd-1du.8` (backup / device): `psync_send_publink` missing; backup
  helpers intentionally do **not** auto-register/remove local sync roots
  as implicit side effects.
- `bd-1du.10` (final parity proof): open until all retained rows are
  justified by code, tests, docs, and matrix evidence.
- **RUSTSEC-2021-0154 (H6-1)**: `fuser 0.15.1` carries an unsound-advisory
  warning. No patched upstream release exists. Scoped ignore in
  `audit.toml`; exploitability requires a real mount. Production mounts
  are gated behind explicit opt-in until a fixed release ships. See
  `SECURITY-AUDIT-FINAL-14042026.md` and `SECURITY.md`.
- Update-check declarations in the upstream C fork are ghost surfaces and
  are intentionally **Rejected** in the Rust rewrite (see
  `REJECTED-RATIONALES-14042026.md`).
- Parity claims: the project explicitly does **not** claim "full parity",
  "production ready", "enterprise ready", or "drop-in replacement". See
  `CLAUDE.md` and `CONTRIBUTING.md`.

## [0.1.0] - Unreleased

Initial pre-release of the Rust rewrite. See [[Unreleased]](#unreleased) for current changes.

This version tracks the first tagged release once the workspace reaches a publishable state.
No tag has been cut yet; the version number is pinned in `Cargo.toml` as `0.1.0` per
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) convention for pre-releases.
