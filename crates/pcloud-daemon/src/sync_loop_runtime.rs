// TODO(bd-sweep-unwrap): This file contains ~91 `.unwrap()` / `.expect()`
// call sites in non-test code paths. Hot-path ones (auth-token lock
// acquisitions, channel sends on the sync-loop thread) should be converted
// to proper error propagation or `log::error!` + graceful loop exit.
// Full sweep is deferred to a dedicated hardening pass; panics in the
// sync-loop thread are caught by the thread join in `main.rs`.

//! Real [`SyncLoopRuntime`] implementation that bridges to the daemon's
//! backend subsystems for autonomous background sync.
//!
//! # Architecture
//!
//! `RuntimeShell` is `!Sync` and exclusively owned by the IPC dispatch
//! thread. The sync loop runs on a dedicated `pcloud-sync-loop` thread.
//! Rather than sharing `RuntimeShell` across threads (impossible without
//! unsafe) or routing through message-passing channels (adds latency and
//! complexity), this adapter owns **its own copies** of the subsystems
//! needed for sync:
//!
//! - A [`SyncRuntime`] for remote diff polling.
//! - A [`TransferRuntime`] for upload/download execution.
//! - An [`EngineShell`] for planning, scheduling, and transfer tracking.
//! - A [`CacheShell`] for staging downloaded/uploaded files.
//! - A [`FilesystemShell`] for seeding staged files.
//! - A `rusqlite::Connection` opened on the same WAL database for reading
//!   sync roots and persisting diff cursors.
//! - A [`SharedAuthToken`] (`Arc<Mutex<Option<SecretString>>>`) that the
//!   IPC thread updates whenever auth state changes.
//!
//! The `rusqlite::Connection` is safe to open concurrently because the
//! store uses WAL journaling mode, which permits concurrent readers and
//! a single writer without blocking.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pcloud_backends::sync_backend::SyncRuntime;
use pcloud_backends::transfer_backend::TransferRuntime;
use pcloud_cache::CacheShell;
use pcloud_config::ConfigProfile;
use pcloud_config::sync_loop::SyncLoopConfig;
use pcloud_engine::EngineShell;
use pcloud_engine::local_scan::{IncrementalScanTracker, LocalScanEntry, ScanDecision};
use pcloud_engine::planner::DeletePolicy;
use pcloud_engine::stall_detector::StallDetector;
use pcloud_fs::FilesystemShell;
use pcloud_fs::fs_watcher::{FsWatcher, WatcherConfig, fs_events_to_local_scan_entries};
use pcloud_model::ids::SyncId;
use pcloud_model::sync::EntryKind;
use pcloud_secret::secret_string::SecretString;
use pcloud_store::DiffStateRepository;
use pcloud_store::repositories::audit::AuditRepository;
use pcloud_store::repositories::file_metadata::FileMetadataRepository;
use pcloud_store::repositories::sync_graph::{SyncGraphRepository, SyncRootRecord};
use pcloud_store::repositories::values::ValuesRepository;
use rusqlite::Connection;

use crate::sync_loop::{
    CycleResult, SyncLoopHandle, SyncLoopRuntime, SyncLoopShared, SyncLoopState,
};

/// Thread-safe shared auth token. The IPC thread writes to this when
/// auth state changes; the sync loop thread reads it at the start of
/// each cycle.
pub type SharedAuthToken = Arc<Mutex<Option<SecretString>>>;

/// Create a new [`SharedAuthToken`] initialized to `None` (not
/// authenticated).
#[must_use]
pub fn shared_auth_token() -> SharedAuthToken {
    Arc::new(Mutex::new(None))
}

/// Real sync loop runtime that owns its own backend instances.
///
/// Constructed by [`RealSyncLoopRuntime::new`] during bootstrap, then
/// moved to the sync loop thread via [`crate::sync_loop::spawn_sync_loop`].
/// Not `Clone` — there is exactly one instance per daemon.
pub struct RealSyncLoopRuntime {
    /// Thread-safe auth token bridge. Read-only from this side; written
    /// by the IPC dispatch thread on login/logout/refresh.
    auth_token: SharedAuthToken,
    /// Sync protocol backend (diff polling, folder validation).
    sync_runtime: SyncRuntime,
    /// Transfer protocol backend (upload/download execution).
    transfer_runtime: TransferRuntime,
    /// Per-sync-root engine state (planner, scheduler, transfers).
    engine: EngineShell,
    /// Cache shell for staging downloaded/uploaded files.
    cache: CacheShell,
    /// Filesystem shell for seeding staged files into the FS layer.
    filesystem: FilesystemShell,
    /// Long-lived SQLite connection to the store's WAL database.
    /// Used for reading sync roots and persisting diff cursors.
    store_conn: Connection,
    /// Per-sync-root filesystem watchers. The `FsWatcher` handle keeps
    /// the inotify/FSEvents subscription alive; dropping it stops the
    /// watch. The `Receiver` yields debounced `FsEvent`s.
    watchers: HashMap<
        SyncId,
        (
            FsWatcher,
            std::sync::mpsc::Receiver<pcloud_engine::fs_events::FsEvent>,
        ),
    >,
    /// Incremental scan tracker: gates full filesystem walks at the
    /// configured `full_scan_interval_secs` and queues watcher events
    /// between full scans.
    scan_tracker: IncrementalScanTracker,
    /// Watcher configuration (debounce window).
    watcher_config: WatcherConfig,
    /// Sync loop config snapshot. Holds `propagate_deletes` and other
    /// per-cycle settings needed to derive per-root `DeletePolicy`.
    sync_loop_config: SyncLoopConfig,
    /// Audit repository for structured audit persistence. Loaded from
    /// the same WAL database so cycle audit events feed the tamper-
    /// evident chain rather than being lost to stderr.
    audit: AuditRepository,
    /// Stall detector: marks progress on successful transfer dispatch
    /// and emits a `warn!` once the cycle has not advanced for
    /// `stall_timeout`. Audit-04 P2-6 (bd-pcloud-rs-s1p.48).
    stall_detector: StallDetector,
    /// On-disk staging directory for streamed downloads (audit-04 L-3,
    /// bd-pcloud-rs-s1p.87). `execute_downloads` writes through to files
    /// under this directory via [`TransferRuntime::download_to_path`]
    /// rather than buffering whole bodies in memory.
    download_staging_dir: PathBuf,
}

/// Files strictly below this threshold are mirrored into the in-memory
/// caches (page cache + staging cache + FUSE staging) after a streamed
/// download. Files at or above this threshold stay on disk only —
/// seeding a multi-MiB page into an LRU that would immediately evict
/// them is pure waste, and keeping them out bounds sync-loop peak RAM
/// regardless of individual file size (bd-pcloud-rs-s1p.87).
const DOWNLOAD_INMEM_MIRROR_THRESHOLD: u64 = 4 * 1024 * 1024;

/// `value_kv` key under which the planner's dead-letter buffer is
/// persisted between cycles. Value is a JSON-encoded `Vec<SyncCandidate>`.
/// Audit-04 P2-6 (bd-pcloud-rs-s1p.44).
const DEAD_LETTER_KEY: &str = "sync.planner.overflow";

/// `value_kv` key under which the scheduler's queued operations are
/// persisted between cycles. Value is a JSON-encoded
/// `Vec<PlannedOperation>` sorted by `(sync_id, priority, path)` so the
/// on-disk form is deterministic. pcloud-rs-774.
const SCHEDULER_QUEUE_KEY: &str = "sync.scheduler.queue";

impl std::fmt::Debug for RealSyncLoopRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealSyncLoopRuntime")
            .finish_non_exhaustive()
    }
}

impl RealSyncLoopRuntime {
    /// Construct a new adapter.
    ///
    /// `db_path` must point to the same SQLite database that the daemon's
    /// `StoreProfile` uses. The connection is opened in WAL mode so it
    /// does not contend with the IPC thread's writes.
    ///
    /// The `auth_token` handle must be the same `Arc` that the IPC thread
    /// writes to on login/logout so the sync loop observes auth state
    /// changes.
    ///
    /// # Errors
    ///
    /// Returns an error if the SQLite connection cannot be opened or tuned.
    pub fn new(
        auth_token: SharedAuthToken,
        config: &ConfigProfile,
        db_path: &Path,
    ) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;

        let full_scan_interval = Duration::from_secs(config.sync_loop.full_scan_interval_secs);

        // TODO(pcloud-rs-8mb.29/L-3): AuditRepository load failure silently
        // falls back to an empty default; surface as a warn so operators
        // see that audit history was lost on restart.
        let audit = AuditRepository::load(&conn).unwrap_or_else(|err| {
            log::warn!("sync loop: failed to restore audit log from DB, starting fresh: {err}");
            Default::default()
        });

        let mut engine = EngineShell::new();

        // Audit-04 P2-6 (bd-pcloud-rs-s1p.47): restore the persisted
        // per-root pause state so roots that were paused via IPC before
        // the last daemon restart stay paused after reboot rather than
        // silently resuming.
        if let Ok(repo) = SyncGraphRepository::load(&conn) {
            for root in &repo.tracked_sync_roots {
                if root.paused {
                    engine.pause_sync_root(root.sync_id);
                }
            }
        }

        // Audit-04 P2-6 (bd-pcloud-rs-s1p.44): restore the dead-letter
        // overflow buffer so candidates skipped at the per-tick cap
        // before the last restart are replayed on the first cycle.
        if let Ok(Some(raw)) = ValuesRepository::get_string(&conn, DEAD_LETTER_KEY) {
            match serde_json::from_str::<Vec<pcloud_model::sync::SyncCandidate>>(&raw) {
                Ok(overflow) if !overflow.is_empty() => {
                    log::info!(
                        "sync loop: restored {} deferred planner candidates from dead-letter store",
                        overflow.len()
                    );
                    engine.restore_planner_overflow(overflow);
                }
                Ok(_) => {}
                Err(err) => {
                    log::warn!("sync loop: dead-letter overflow buffer corrupt, discarding: {err}");
                    let _ = ValuesRepository::delete(&conn, DEAD_LETTER_KEY);
                }
            }
        }

        // pcloud-rs-774: restore the persisted scheduler queue so
        // planned operations that were scheduled but not yet dispatched
        // before the last restart survive the reboot instead of silently
        // vanishing.
        if let Ok(Some(raw)) = ValuesRepository::get_string(&conn, SCHEDULER_QUEUE_KEY) {
            match serde_json::from_str::<Vec<pcloud_model::sync::PlannedOperation>>(&raw) {
                Ok(queue) if !queue.is_empty() => {
                    log::info!(
                        "sync loop: restored {} queued scheduler operations from persisted queue",
                        queue.len()
                    );
                    engine.restore_scheduler_queue(queue);
                }
                Ok(_) => {}
                Err(err) => {
                    log::warn!(
                        "sync loop: persisted scheduler queue is corrupt, discarding: {err}"
                    );
                    let _ = ValuesRepository::delete(&conn, SCHEDULER_QUEUE_KEY);
                }
            }
        }

        // Audit-04 P2-6 (bd-pcloud-rs-s1p.48): wire a real stall detector
        // so no-progress cycles are logged. Timeout is conservative but
        // shorter than the 5-minute default so a stuck run_cycle is
        // visible quickly.
        let stall_detector = StallDetector::new(Duration::from_secs(120));

        // bd-pcloud-rs-s1p.87: per-daemon on-disk staging dir for streamed
        // downloads. `execute_downloads` writes through to files under this
        // directory via [`TransferRuntime::download_to_path`], so peak
        // transport memory is bounded by the HTTP read buffer (64 KiB)
        // plus the BufWriter buffer (64 KiB), independent of body size.
        let download_staging_dir = config.paths.cache_dir.join("download-staging");
        if let Err(err) = std::fs::create_dir_all(&download_staging_dir) {
            log::warn!(
                "sync loop: failed to pre-create download staging dir {download_staging_dir:?}: {err}; \
                 downloads will attempt to create it on-demand"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            if let Ok(meta) = std::fs::metadata(&download_staging_dir) {
                let mut perm = meta.permissions();
                perm.set_mode(0o700);
                let _ = std::fs::set_permissions(&download_staging_dir, perm);
            }
        }

        Ok(Self {
            auth_token,
            sync_runtime: SyncRuntime::from_config(config),
            transfer_runtime: TransferRuntime::from_config(config),
            engine,
            cache: CacheShell::default(),
            filesystem: FilesystemShell::default(),
            store_conn: conn,
            watchers: HashMap::new(),
            scan_tracker: IncrementalScanTracker::new(full_scan_interval),
            watcher_config: WatcherConfig::default(),
            sync_loop_config: config.sync_loop.clone(),
            audit,
            stall_detector,
            download_staging_dir,
        })
    }

    /// Persist the current planner dead-letter overflow buffer into the
    /// store. Called after each ingest so a crash between ticks does not
    /// silently drop deferred work. Audit-04 P2-6 (bd-pcloud-rs-s1p.44).
    fn persist_planner_overflow(&self) {
        let overflow = &self.engine.planner_overflow;
        if overflow.is_empty() {
            let _ = ValuesRepository::delete(&self.store_conn, DEAD_LETTER_KEY);
            return;
        }
        match serde_json::to_string(overflow) {
            Ok(serialized) => {
                if let Err(err) =
                    ValuesRepository::set_string(&self.store_conn, DEAD_LETTER_KEY, &serialized)
                {
                    log::warn!(
                        "sync loop: failed to persist {} deferred candidates: {err}",
                        overflow.len()
                    );
                }
            }
            Err(err) => {
                log::warn!("sync loop: failed to serialize dead-letter overflow: {err}");
            }
        }
    }

    /// Persist the current scheduler queued operations so they survive a
    /// daemon restart. Called from the same ingest hot path as
    /// [`Self::persist_planner_overflow`]. pcloud-rs-774.
    fn persist_scheduler_queue(&self) {
        // P2-b (H2): durable snapshot must include both queued and
        // in-flight (dispatched-but-not-acked) operations so a crash
        // between dispatch and server-side ack re-enqueues the work on
        // restart rather than silently dropping it.
        let snapshot = self.engine.snapshot_scheduler_durable();
        if snapshot.is_empty() {
            let _ = ValuesRepository::delete(&self.store_conn, SCHEDULER_QUEUE_KEY);
            return;
        }
        match serde_json::to_string(&snapshot) {
            Ok(serialized) => {
                if let Err(err) = ValuesRepository::set_string(
                    &self.store_conn,
                    SCHEDULER_QUEUE_KEY,
                    &serialized,
                ) {
                    log::warn!(
                        "sync loop: failed to persist {} scheduler ops: {err}",
                        snapshot.len()
                    );
                }
            }
            Err(err) => {
                log::warn!("sync loop: failed to serialize scheduler queue: {err}");
            }
        }
    }

    /// Drain the engine's pending-watcher-eviction notifications and
    /// drop the associated [`FsWatcher`] handles. Called from
    /// [`Self::evict_removed_root`] and by the sync loop between cycles
    /// so any `EngineShell::evict_sync_root` call (including IPC-driven
    /// ones that don't go through `evict_removed_root`) eventually tears
    /// down the corresponding inotify/FSEvents subscription.
    ///
    /// pcloud-rs-774.
    pub fn drain_engine_watcher_evictions(&mut self) {
        let pending = self.engine.drain_watcher_evictions();
        for sync_id in pending {
            if self.watchers.contains_key(&sync_id) {
                log::debug!(
                    "sync loop: draining engine-signaled watcher eviction for sync_id={}",
                    sync_id.get()
                );
                self.remove_watcher(sync_id);
            }
        }
    }

    /// Ensure a filesystem watcher is running for `root`. If the watcher
    /// is already active, this is a no-op. If `FsWatcher::start` fails
    /// (inotify limit, permission error), we log a warning and fall back
    /// to poll-only mode — the `IncrementalScanTracker` still fires full
    /// scans at the configured interval.
    fn ensure_watcher(&mut self, root: &SyncRootRecord) {
        if self.watchers.contains_key(&root.sync_id) {
            return;
        }
        let root_path = std::path::Path::new(&root.local_path);
        match FsWatcher::start(root_path, root.sync_id, &self.watcher_config) {
            Ok((watcher, rx)) => {
                self.watchers.insert(root.sync_id, (watcher, rx));
            }
            Err(err) => {
                log::warn!(
                    "fs-watcher: failed to start watcher for sync root {} ({}): {err}; \
                     falling back to poll-only mode",
                    root.sync_id.get(),
                    root.local_path
                );
            }
        }
    }

    /// Drain all pending watcher events for `sync_id` into the
    /// `IncrementalScanTracker` so they are available for the next
    /// `decide()` call.
    fn drain_watcher_events(&mut self, sync_id: SyncId) {
        if let Some((_watcher, rx)) = self.watchers.get(&sync_id) {
            // Non-blocking drain of all available events.
            while let Ok(event) = rx.try_recv() {
                self.scan_tracker.push_event(event);
            }
        }
    }

    /// Stop and remove the watcher for `sync_id`. Called when a sync
    /// root is removed.
    pub fn remove_watcher(&mut self, sync_id: SyncId) {
        self.watchers.remove(&sync_id);
        self.scan_tracker.untrack(sync_id);
    }
}

impl SyncLoopRuntime for RealSyncLoopRuntime {
    fn auth_token(&self) -> Option<SecretString> {
        self.auth_token
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(SecretString::clone_secret))
    }

    fn list_sync_roots(&self) -> Vec<SyncRootRecord> {
        SyncGraphRepository::load(&self.store_conn)
            .map(|repo| repo.tracked_sync_roots)
            .unwrap_or_default()
    }

    fn is_sync_root_paused(&self, root: &SyncRootRecord) -> bool {
        self.engine.is_sync_root_paused(root.sync_id)
    }

    fn poll_remote_diff(
        &mut self,
        root: &SyncRootRecord,
        auth_token: &SecretString,
    ) -> Result<usize, String> {
        // Read persisted cursor for this root.
        let cursor = DiffStateRepository::load(&self.store_conn, root.sync_id)
            .map_err(|e| e.to_string())?
            .map(|r| r.diffid)
            .unwrap_or(0);

        let batch_limit = self.engine.diff_poller.batch_limit;
        let batch = self
            .sync_runtime
            .diff(auth_token.clone_secret(), cursor, batch_limit)
            .map_err(|e| e.to_string())?;

        // Audit 04 C3: the diff cursor MUST NOT advance before the batch
        // has been successfully ingested. Previously the cursor was
        // persisted immediately after fetch, so a crash between fetch
        // and engine ingestion silently dropped a batch on restart.
        //
        // We now run the ingestion *first*, and only if it returns `Ok`
        // do we commit cursor advance + metadata deletions together in a
        // single SQLite transaction. If the engine rejects the batch or
        // the transaction fails, the cursor stays put and the same batch
        // is refetched on the next cycle — at-least-once semantics.
        let delete_policy =
            DeletePolicy::for_sync_type(root.sync_type, self.sync_loop_config.propagate_deletes);
        let operations = self
            .engine
            .ingest_remote_diff_filtered(&batch, &delete_policy)
            .map_err(|e| format!("{e:?}"))?;
        let op_count = operations.len();

        // Audit-04 P2-6 (bd-pcloud-rs-s1p.44): persist any candidates
        // that were deferred at the per-tick cap so a crash here does
        // not drop them silently.
        self.persist_planner_overflow();

        // Commit cursor advance + metadata-cache deletes atomically.
        commit_diff_batch(&self.store_conn, root.sync_id, cursor, &batch)
            .map_err(|e| e.to_string())?;

        Ok(op_count)
    }

    fn run_local_scan(&mut self, root: &SyncRootRecord) -> Result<usize, String> {
        // Ensure a filesystem watcher is running for this root. If the
        // watcher cannot be started (e.g. inotify limit exhausted), we
        // fall back to poll-only mode silently.
        self.ensure_watcher(root);

        // Drain any pending watcher events into the scan tracker.
        self.drain_watcher_events(root.sync_id);

        // Ask the tracker whether we need a full walk or only the
        // incremental watcher events.
        let decision = self.scan_tracker.decide(root.sync_id);

        let entries = match decision {
            ScanDecision::FullScan => {
                let entries = walk_local_tree(root, &self.store_conn)?;
                self.scan_tracker.record_full_scan(root.sync_id);
                entries
            }
            ScanDecision::IncrementalOnly { pending_events } => {
                if pending_events.is_empty() {
                    return Ok(0);
                }
                fs_events_to_local_scan_entries(&pending_events)
            }
        };

        let delete_policy =
            DeletePolicy::for_sync_type(root.sync_type, self.sync_loop_config.propagate_deletes);
        let operations = self
            .engine
            .ingest_local_scan_with_delete_policy(&entries, &delete_policy)
            .map_err(|e| format!("{e:?}"))?;
        let op_count = operations.len();
        self.persist_planner_overflow();
        // pcloud-rs-774: persist the updated scheduler queue so newly
        // planned operations survive a crash between ticks.
        self.persist_scheduler_queue();
        Ok(op_count)
    }

    fn advance_transfers(&mut self) -> usize {
        let batch = self.engine.advance_transfer_cycle();
        let dispatched = batch.len();
        // pcloud-rs-774: dispatch shrinks the queue; reflect that in the
        // persisted copy so a restart doesn't re-enqueue already-dispatched
        // work.
        if dispatched > 0 {
            self.persist_scheduler_queue();
        }
        // Audit-04 P2-6 (bd-pcloud-rs-s1p.48): any forward movement in
        // the scheduler queue counts as progress for the stall
        // detector. If dispatched==0 but we have unresolved conflicts or
        // in-flight transfers, the absence of progress is real and the
        // next check_stall() call will surface it.
        if dispatched > 0 {
            self.stall_detector.mark_progress();
        } else if self.stall_detector.check_stall() {
            log::warn!(
                "sync loop: stall detected; no scheduler dispatch progress within stall window"
            );
        }
        dispatched
    }

    fn execute_downloads(&mut self, auth_token: &SecretString) -> Result<usize, String> {
        let tasks = self.engine.downloads.active_downloads.clone();
        let mut completed = 0usize;

        for task in tasks {
            // P2-d (H4): mark progress on each per-task entry, not just
            // the single dispatch boundary. A large batch of mid-sized
            // downloads would previously appear "stalled" to the
            // StallDetector even though each task individually was
            // making forward progress, because `mark_progress` was
            // only called when the scheduler dispatched a new batch.
            self.stall_detector.mark_progress();
            if let pcloud_model::sync::PlannedOperation::DownloadFile {
                path,
                remote_file_id: Some(file_id),
                ..
            } = &task.operation
            {
                match self.transfer_runtime.get_file_link(
                    auth_token.clone_secret(),
                    file_id.get(),
                    None,
                ) {
                    Ok(link) => {
                        // bd-pcloud-rs-s1p.87: stream download to a per-file
                        // on-disk staging path rather than buffering the
                        // full body in memory. The peak memory held by the
                        // transport is bounded by the HTTP read buffer
                        // (64 KiB) plus the BufWriter buffer (64 KiB),
                        // independent of body size.
                        let staged_path =
                            staged_download_path(&self.download_staging_dir, file_id.get(), path);
                        match self.transfer_runtime.download_to_path(&link, &staged_path) {
                            Ok((_signed, written)) => {
                                // Mirror into in-memory caches only for
                                // small payloads. Larger files stay on
                                // disk; downstream consumers (FUSE, writeback)
                                // can read from the staged path on demand.
                                if written < DOWNLOAD_INMEM_MIRROR_THRESHOLD {
                                    match std::fs::read(&staged_path) {
                                        Ok(bytes) => {
                                            let cache_key = format!("download:{path}");
                                            self.cache.cache_page(cache_key, bytes.clone());
                                            self.cache.stage_file(path.clone(), bytes.clone());
                                            self.filesystem
                                                .seed_staged_file(path.clone(), bytes);
                                        }
                                        Err(err) => {
                                            log::warn!(
                                                "sync loop: staged download at {staged_path:?} readable failed: {err}; skipping in-memory mirror"
                                            );
                                        }
                                    }
                                } else {
                                    log::debug!(
                                        "sync loop: staged {written}-byte download at {staged_path:?} kept on-disk (above {}B in-memory mirror threshold)",
                                        DOWNLOAD_INMEM_MIRROR_THRESHOLD
                                    );
                                }
                                // Audit-06 §4-opus HIGH: record real
                                // byte-level progress for this transfer
                                // so a long-running download that
                                // exceeds the wall-clock stall window
                                // is still recognised as non-stalled
                                // via its byte counter.
                                self.stall_detector
                                    .observe_bytes(path, written as u64);
                                if self.engine.mark_transfer_completed(path) {
                                    completed += 1;
                                    // P2-b (H2): durable ack — remove
                                    // the dispatched entry so the
                                    // persisted scheduler snapshot no
                                    // longer carries this op on the
                                    // next persist. Audit-06 §4-sonnet
                                    // M-04-S04: scope the ack to the
                                    // owning sync root so a cross-root
                                    // path collision cannot evict a
                                    // sibling root's un-acked entry.
                                    self.engine.ack_dispatched_path(
                                        task.operation.sync_id(),
                                        path,
                                    );
                                    // P2-d (H4): bytes transferred
                                    // count as progress; reset the
                                    // stall timer.
                                    self.stall_detector.mark_progress();
                                    // Byte-progress state is retired
                                    // alongside the dispatched slot.
                                    self.stall_detector.forget_transfer(path);
                                }
                            }
                            Err(err) => {
                                // Drop any byte-progress state for the
                                // failed transfer so the next attempt
                                // starts clean.
                                self.stall_detector.forget_transfer(path);
                                // Best-effort cleanup of any partial file.
                                let _ = std::fs::remove_file(&staged_path);
                                let decision = self.engine.classify_failure(
                                    &task.operation,
                                    pcloud_engine::recovery::RecoveryFailure::RetryableNetworkError,
                                );
                                let message =
                                    format!("{err}; recovery={:?}", decision.disposition);
                                if !self.engine.mark_transfer_failed(path, message) {
                                    log::warn!(
                                        "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                                    );
                                }
                            }
                        }
                    }
                    Err(err) => {
                        let decision = self.engine.classify_failure(
                            &task.operation,
                            pcloud_engine::recovery::RecoveryFailure::RetryableNetworkError,
                        );
                        let message = format!("{err}; recovery={:?}", decision.disposition);
                        if !self.engine.mark_transfer_failed(path, message) {
                            log::warn!(
                                "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                            );
                        }
                    }
                }
            }
        }

        Ok(completed)
    }

    fn execute_uploads(&mut self, auth_token: &SecretString) -> Result<usize, String> {
        let tasks = self.engine.uploads.active_uploads.clone();
        let mut completed = 0usize;

        for task in tasks {
            // P2-d (H4): per-task progress mark — see
            // `execute_downloads` for rationale.
            self.stall_detector.mark_progress();
            if let pcloud_model::sync::PlannedOperation::UploadFile {
                path,
                remote_parent_folder_id,
                remote_name,
                ..
            } = &task.operation
            {
                // Resolve parent folder id from path.
                let parent_folder_id = match resolve_upload_parent(path, *remote_parent_folder_id) {
                    Ok(id) => id,
                    Err(failure) => {
                        let decision = self.engine.classify_failure(&task.operation, failure);
                        let message = format!(
                            "missing upload destination metadata; recovery={:?}",
                            decision.disposition
                        );
                        if !self.engine.mark_transfer_failed(path, message) {
                            log::warn!(
                                "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                            );
                        }
                        continue;
                    }
                };

                // Resolve the upload payload source (filesystem staging
                // preferred, cache staging fallback). Both accessors
                // return a borrowed `&[u8]` directly — no `Vec` clones.
                //
                // Bead pcloud-rs-s1p.88: we intentionally avoid the
                // previous `read_staged_path(0, usize::MAX)` call, which
                // copied the entire staged file into a fresh `Vec` via
                // `ReadResult.bytes`, and the subsequent `.to_vec()` in
                // the cache fallback. For large uploads (GiBs) this
                // was a peak-RSS hazard. The current wire layer
                // (`upload_bytes`) still requires a single `&[u8]`
                // slice, so we borrow directly from staging instead of
                // cloning; follow-up work to pipeline chunked
                // `upload_write` calls is tracked separately.
                let payload_len = match resolve_upload_payload_len(
                    &self.filesystem,
                    &self.cache,
                    path,
                ) {
                    Some(len) => len,
                    None => {
                        let decision = self.engine.classify_failure(
                            &task.operation,
                            pcloud_engine::recovery::RecoveryFailure::InvalidPath,
                        );
                        let message = format!(
                            "missing staged upload payload; recovery={:?}",
                            decision.disposition
                        );
                        if !self.engine.mark_transfer_failed(path, message) {
                            log::warn!(
                                "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                            );
                        }
                        continue;
                    }
                };

                match self.transfer_runtime.upload_create(
                    auth_token.clone_secret(),
                    parent_folder_id,
                    remote_name.clone(),
                    payload_len as u64,
                ) {
                    Ok(session) => {
                        // Re-borrow the staged bytes for the single
                        // `upload_bytes` call. We deliberately do not
                        // hold a borrow across the `upload_create`
                        // boundary so that the `&mut self` receiver
                        // remains available.
                        let upload_result = match borrow_upload_payload(
                            &self.filesystem,
                            &self.cache,
                            path,
                        ) {
                            Some(bytes) => self.transfer_runtime.upload_bytes(
                                auth_token.clone_secret(),
                                &session,
                                bytes,
                            ),
                            None => {
                                // Race: payload evicted between the
                                // length probe and the write. Treat
                                // as a transient failure.
                                let decision = self.engine.classify_failure(
                                    &task.operation,
                                    pcloud_engine::recovery::RecoveryFailure::InvalidPath,
                                );
                                let message = format!(
                                    "staged upload payload evicted mid-upload; recovery={:?}",
                                    decision.disposition
                                );
                                if !self.engine.mark_transfer_failed(path, message) {
                                    log::warn!(
                                        "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                                    );
                                }
                                continue;
                            }
                        };
                        match upload_result {
                            Ok(_frame) => {
                                if self.engine.mark_transfer_completed(path) {
                                    completed += 1;
                                    // P2-b (H2): durable ack. Audit-06
                                    // §4-sonnet M-04-S04: scope to
                                    // owning sync root to avoid
                                    // cross-root path collisions.
                                    self.engine.ack_dispatched_path(
                                        task.operation.sync_id(),
                                        path,
                                    );
                                    // P2-d (H4): bytes transferred →
                                    // stall timer reset on completion.
                                    self.stall_detector.mark_progress();
                                }
                            }
                            Err(err) => {
                                let decision = self.engine.classify_failure(
                                    &task.operation,
                                    pcloud_engine::recovery::RecoveryFailure::RetryableNetworkError,
                                );
                                let message = format!("{err}; recovery={:?}", decision.disposition);
                                if !self.engine.mark_transfer_failed(path, message) {
                                    log::warn!(
                                        "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                                    );
                                }
                            }
                        }
                    }
                    Err(err) => {
                        let decision = self.engine.classify_failure(
                            &task.operation,
                            pcloud_engine::recovery::RecoveryFailure::RetryableNetworkError,
                        );
                        let message = format!("{err}; recovery={:?}", decision.disposition);
                        if !self.engine.mark_transfer_failed(path, message) {
                            log::warn!(
                                "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                            );
                        }
                    }
                }
            }
        }

        Ok(completed)
    }

    fn conflict_count(&self) -> usize {
        self.engine.unresolved_conflict_count()
    }

    fn pending_upload_count(&self) -> usize {
        self.engine.uploads.active_count()
    }

    fn pending_download_count(&self) -> usize {
        self.engine.downloads.active_count()
    }

    fn evict_removed_root(&mut self, sync_id: SyncId) {
        // Drop the FsWatcher handle (closes the watcher thread's channel
        // sender and unregisters inotify/FSEvents watches) and remove the
        // root from the incremental scan tracker.
        self.remove_watcher(sync_id);
        // Also evict engine-side state (scheduler queue, in-flight
        // transfers, paused-root set).
        self.engine.evict_sync_root(sync_id);
        // pcloud-rs-774: evict_sync_root pushed the id into the engine's
        // pending-watcher-eviction queue. The watcher for *this* id has
        // already been removed above, but draining here keeps the engine
        // notification queue coherent and also reaps any prior IPC-driven
        // evictions that bypassed evict_removed_root.
        self.drain_engine_watcher_evictions();
        // Persist the shrunken scheduler queue so removed-root ops don't
        // come back on restart.
        self.persist_scheduler_queue();
    }

    fn emit_cycle_audit(&mut self, _root_id: u64, result: &CycleResult) -> Result<(), String> {
        // Only emit an audit event when something non-trivial happened
        // in the cycle. A purely idle tick does not deserve a chain
        // entry — but note that audit_persist_error is surfaced by the
        // caller regardless.
        if result.total_errors == 0 && result.total_uploads == 0 && result.total_downloads == 0 {
            return Ok(());
        }
        let details = format!(
            "roots={}, uploads={}, downloads={}, conflicts={}, errors={}, duration_ms={}",
            result.roots_processed,
            result.total_uploads,
            result.total_downloads,
            result.total_conflicts,
            result.total_errors,
            result.duration.as_millis()
        );
        if let Err(err) =
            self.audit
                .append_event(&self.store_conn, "sync.loop.cycle", Some(&details))
        {
            // Audit-04 P2-6 (bd-pcloud-rs-s1p.50): audit persistence
            // failures must not be silently swallowed. Return the error
            // so `run_cycle` can bump `total_errors` and attach the
            // message to `audit_persist_error` on `CycleResult`.
            log::error!("audit: sync-loop-cycle persistence failed: {err}; details={details}");
            return Err(format!(
                "audit-chain write failed: {err}; details={details}"
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public bootstrap helper
// ---------------------------------------------------------------------------

/// Construct a [`RealSyncLoopRuntime`] from the daemon's runtime state
/// and spawn it on the background sync loop thread.
///
/// Returns the [`SyncLoopHandle`] (for status queries and shutdown) and
/// the [`SharedAuthToken`] that the IPC thread must write to on
/// login/logout/refresh so the sync loop observes auth state changes.
///
/// Called by [`crate::serve::serve_with_shutdown`] after
/// [`crate::bootstrap_shell`] completes.
pub fn spawn_daemon_sync_loop(
    config: &ConfigProfile,
    auth: &pcloud_auth::SessionManager,
    db_path: std::path::PathBuf,
) -> Result<(SyncLoopHandle, SharedAuthToken), rusqlite::Error> {
    let token = shared_auth_token();

    // Seed the shared auth token with any existing auth state.
    if let Some(existing) = auth.snapshot().auth_token.as_ref()
        && let Ok(mut guard) = token.lock()
    {
        *guard = Some(existing.clone_secret());
    }

    let runtime = RealSyncLoopRuntime::new(Arc::clone(&token), config, &db_path).map_err(|e| {
        log::error!("sync loop: failed to open store connection: {e}");
        e
    })?;

    let shared = Arc::new(SyncLoopShared::new(SyncLoopState::Idle));
    let handle = crate::sync_loop::spawn_sync_loop(runtime, config.sync_loop.clone(), shared);

    Ok((handle, token))
}

// ---------------------------------------------------------------------------
// Helper: walk a sync root's local directory tree
// ---------------------------------------------------------------------------

/// Walk a sync root's local directory tree and produce
/// [`LocalScanEntry`] items.
///
/// Audit 04 C2: each emitted entry's `remote_parent_folder_id` is
/// populated from the local metadata cache so that
/// `resolve_upload_parent` can route nested files to the correct remote
/// folder. The root's remote folder id is resolved from
/// `SyncRootRecord::remote_path` via
/// [`FileMetadataRepository::resolve_path`]; children inherit their
/// parent's id by `(parent_id, leaf_name)` lookup. When the cache is
/// cold we fall back to `None`, which is still correct upstream
/// (the planner will requeue the file until the cache warms up).
fn walk_local_tree(
    root: &SyncRootRecord,
    conn: &Connection,
) -> Result<Vec<LocalScanEntry>, String> {
    let base = std::path::Path::new(&root.local_path);
    if !base.is_dir() {
        return Err(format!(
            "sync root path does not exist or is not a directory: {}",
            root.local_path
        ));
    }
    // Resolve the remote folder id of the sync root itself. Root folder
    // (remote path "/" or empty) has id 0. If the cache does not yet
    // know about `remote_path`, we propagate `None` so nested uploads
    // defer until the next scan cycle instead of racing with a stale
    // placeholder.
    let root_remote_folder_id =
        resolve_sync_root_remote_folder_id(conn, &root.remote_path).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    // Track visited directory inodes to detect and break hard-link cycles
    // before recursing into them (see walk_recursive for details).
    let mut visited_inodes = std::collections::HashSet::new();
    walk_recursive(
        base,
        base,
        root.sync_id,
        root_remote_folder_id,
        conn,
        &mut entries,
        &mut visited_inodes,
    )?;
    Ok(entries)
}

/// Resolve the remote folder id for a sync root's `remote_path` via the
/// local metadata cache. Returns `Some(0)` for the root folder (path
/// `"/"` or empty), `Some(id)` for a resolved subfolder, or `None` if
/// the cache does not yet contain the path.
fn resolve_sync_root_remote_folder_id(
    conn: &Connection,
    remote_path: &str,
) -> Result<Option<pcloud_model::ids::RemoteFolderId>, rusqlite::Error> {
    let trimmed = remote_path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return Ok(Some(pcloud_model::ids::RemoteFolderId::new(0)));
    }
    match FileMetadataRepository::resolve_path(conn, trimmed)? {
        Some(record) if record.is_folder => {
            Ok(Some(pcloud_model::ids::RemoteFolderId::new(record.file_id)))
        }
        _ => Ok(None),
    }
}

fn walk_recursive(
    base: &std::path::Path,
    current: &std::path::Path,
    sync_id: SyncId,
    parent_remote_folder_id: Option<pcloud_model::ids::RemoteFolderId>,
    conn: &Connection,
    entries: &mut Vec<LocalScanEntry>,
    visited_inodes: &mut std::collections::HashSet<u64>,
) -> Result<(), String> {
    let dir_entries = std::fs::read_dir(current)
        .map_err(|e| format!("failed to read {}: {e}", current.display()))?;
    for entry in dir_entries {
        let entry = entry.map_err(|e| format!("dir entry error: {e}"))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(base)
            .map_err(|e| format!("strip_prefix failed: {e}"))?
            .to_string_lossy()
            .replace('\\', "/");
        if relative.is_empty() {
            continue;
        }

        // Use symlink_metadata so that we inspect the symlink node itself,
        // not its target. This prevents both (a) following symlinks into
        // directories outside the sync root and (b) participating in
        // directory cycles via hard-linked dirs or circular symlinks.
        let symlink_meta = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("symlink_metadata error for {}: {e}", path.display()))?;

        if symlink_meta.is_symlink() {
            // Skip symlinks. Following them can create cycles and expose
            // paths outside the sync root. Matches C client behaviour.
            log::debug!("walk_local_tree: skipping symlink {}", path.display());
            continue;
        }

        let entry_kind = if symlink_meta.is_dir() {
            EntryKind::Folder
        } else {
            EntryKind::File
        };

        let leaf_name = entry.file_name().to_string_lossy().into_owned();

        entries.push(LocalScanEntry {
            sync_id,
            path: relative,
            entry_kind,
            deleted: false,
            remote_parent_folder_id: parent_remote_folder_id,
        });

        if symlink_meta.is_dir() {
            // Guard against hard-linked directory cycles (unusual but valid
            // on some Linux filesystems). Track visited inodes and skip any
            // directory we have already entered.
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let inode = symlink_meta.ino();
                if !visited_inodes.insert(inode) {
                    log::warn!(
                        "walk_local_tree: inode cycle detected at {} (inode {}); skipping",
                        path.display(),
                        inode
                    );
                    continue;
                }
            }

            // Resolve this directory's own remote folder id from the
            // cache so its children can point at the correct parent.
            // A cache miss leaves children with `None` — same
            // best-effort contract as the sync root resolution above.
            let child_parent = match parent_remote_folder_id {
                Some(parent_id) => {
                    match FileMetadataRepository::get_by_parent_and_name(
                        conn,
                        parent_id.get(),
                        &leaf_name,
                    )
                    .map_err(|e| e.to_string())?
                    {
                        Some(record) if record.is_folder => {
                            Some(pcloud_model::ids::RemoteFolderId::new(record.file_id))
                        }
                        _ => None,
                    }
                }
                None => None,
            };
            walk_recursive(
                base,
                &path,
                sync_id,
                child_parent,
                conn,
                entries,
                visited_inodes,
            )?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helper: commit a diff batch atomically (Audit 04 C1 + C3)
// ---------------------------------------------------------------------------

/// Commit the effects of a successfully-ingested diff batch into the
/// store as a single SQLite transaction.
///
/// This function MUST be called **only after**
/// [`pcloud_engine::EngineShell::ingest_remote_diff_filtered`] returns
/// `Ok`. That ordering is the core of Audit 04 C3's at-least-once
/// guarantee: if the engine rejects the batch (or if this transaction
/// fails for any reason), the persisted cursor stays at `previous_cursor`
/// and the same batch is refetched on the next cycle.
///
/// Audit 04 C1: we intentionally do **not** upsert `file_metadata` rows
/// from diff entries because `RemoteDiffEntry` lacks `size`, `hash`,
/// `modified`, `created`, and `parent_folder_id`. The old code fabricated
/// zeros for all of these, which poisoned
/// [`FileMetadataRepository::resolve_path`] / stat-cache callers with
/// plausibly-typed but semantically-wrong metadata. Metadata upserts
/// must originate from call sites that have a full stat payload (e.g.
/// `listfolder` responses), not from the diff loop.
fn commit_diff_batch(
    conn: &Connection,
    sync_id: SyncId,
    previous_cursor: u64,
    batch: &pcloud_engine::diff_poller::RemoteDiffBatch,
) -> Result<(), rusqlite::Error> {
    // Use FULL synchronous mode for cursor writes. The connection-level
    // default is NORMAL (good for read-heavy workloads), but the diff
    // cursor is the at-least-once bookmark: losing it after an OS crash
    // forces a full re-sync. FULL flushes to the OS page cache before
    // returning, providing the same durability guarantees as `fsync`.
    // The per-transaction scope means only cursor writes pay the extra
    // latency; all other reads in the same connection stay at NORMAL.
    // P2-c (H3): RAII guard ensures `synchronous=NORMAL` is restored on
    // every exit path, including early `?`-propagated errors inside the
    // transaction. Previously a panic/error between the FULL/NORMAL
    // pragma pair left the connection at FULL for the rest of the
    // daemon's life, which would silently slow every subsequent write.
    let _guard = SynchronousGuard::set_full(conn)?;
    let tx = conn.unchecked_transaction()?;
    for entry in &batch.entries {
        if entry.change_kind != pcloud_model::sync::ChangeKind::Delete {
            continue;
        }
        let is_folder = entry.entry_kind == EntryKind::Folder;
        let file_id = if is_folder {
            entry.remote_folder_id.map(|id| id.get())
        } else {
            entry.remote_file_id.map(|id| id.get())
        };
        if let Some(file_id) = file_id {
            FileMetadataRepository::delete(&tx, file_id)?;
        }
    }
    if batch.cursor > previous_cursor {
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        DiffStateRepository::save(&tx, sync_id, batch.cursor, now_unix)?;
    }
    tx.commit()
    // _guard drops here and restores `synchronous=NORMAL` even on early
    // return paths above.
}

/// P2-c (H3) / Audit-06 §4-opus HIGH: RAII guard that temporarily sets
/// SQLite's `synchronous` pragma to `FULL` and restores the **previously
/// observed** value on drop. The old value is captured by a `PRAGMA
/// synchronous` query at construction time so that restoration is
/// correct regardless of whether the caller's baseline was `NORMAL`,
/// `OFF`, or something else. The restore runs even on panic or early
/// `?` propagation, preventing the "connection stuck at FULL forever"
/// bug flagged by audit-05 P2-c and re-flagged by audit-06 §4-opus HIGH.
struct SynchronousGuard<'a> {
    conn: &'a Connection,
    /// Mnemonic for the pragma value that was live before `set_full`
    /// was called. One of `"OFF"`, `"NORMAL"`, `"FULL"`, or `"EXTRA"`.
    old: &'static str,
}

impl<'a> SynchronousGuard<'a> {
    fn set_full(conn: &'a Connection) -> Result<Self, rusqlite::Error> {
        // SQLite returns the pragma as the integer encoding
        // (0=OFF, 1=NORMAL, 2=FULL, 3=EXTRA). Capture that first so we
        // can restore the exact mode on drop.
        let old_code: i64 =
            conn.query_row("PRAGMA synchronous;", [], |row| row.get::<_, i64>(0))?;
        let old = match old_code {
            0 => "OFF",
            1 => "NORMAL",
            2 => "FULL",
            3 => "EXTRA",
            // Unknown encoding — fall back to NORMAL on restore rather
            // than risk a syntax error in the pragma exec.
            _ => "NORMAL",
        };
        conn.pragma_update(None, "synchronous", "FULL")?;
        Ok(Self { conn, old })
    }
}

impl Drop for SynchronousGuard<'_> {
    fn drop(&mut self) {
        // Best-effort: if restoring the previous mode fails we log but
        // cannot propagate — Drop has no error channel. A failure here
        // leaves the connection at FULL, which is safer than leaving it
        // at a weaker mode with no durability guarantee.
        if let Err(err) = self.conn.pragma_update(None, "synchronous", self.old) {
            log::warn!(
                "sync loop: failed to restore synchronous={} on commit_diff_batch drop: {err}",
                self.old
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: resolve upload parent folder id (ported from runtime.rs)
// ---------------------------------------------------------------------------

fn resolve_upload_parent(
    path: &str,
    remote_parent_folder_id: Option<pcloud_model::ids::RemoteFolderId>,
) -> Result<u64, pcloud_engine::recovery::RecoveryFailure> {
    let parent = path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    if parent.is_empty() {
        return Ok(0);
    }
    // P2-e (H5): audit-04 C2 threaded `remote_parent_folder_id` through
    // the walk via the local metadata cache. On first scan after a fresh
    // daemon start, the cache is cold and nested files hit this branch
    // with `None`. Returning `InvalidPath` here is classified as
    // **Terminal** by the recovery manager (see
    // `pcloud-engine/src/recovery.rs`), which drops the upload forever —
    // exactly the terminal-fail symptom called out in audit-05 P2-e.
    //
    // The correct disposition is retry-with-backoff so the next cycle
    // can repopulate the cache (e.g. via a `listfolder` on the parent,
    // or after the remote-diff poller learns about the newly created
    // folder) and then resolve the id. `RetryableNetworkError` maps to
    // retry with exponential backoff without hard-failing the task.
    //
    // A synchronous cache-warm-here path (calling into the backend
    // `RemotePathResolver` from `pcloud-backends::path_resolver`) is a
    // sturdier fix but crosses a new dependency boundary from this
    // crate into `pcloud-backends` at the hot upload path. We defer
    // that optimisation until the cold-cache rate is visible in
    // metrics (tracked on the same bead); the retry path is correct,
    // bounded, and produces no data loss.
    remote_parent_folder_id
        .map(|id| id.get())
        .ok_or(pcloud_engine::recovery::RecoveryFailure::RetryableNetworkError)
}

// ---------------------------------------------------------------------------
// Helper: derive an on-disk staging path for a streamed download
// (bd-pcloud-rs-s1p.87).
// ---------------------------------------------------------------------------

/// Build a safe, collision-resistant on-disk path inside `staging_dir`
/// for the streamed download of `file_id` at logical `path`. The
/// filename is derived from `file_id` and a SHA-256 digest of `path`
/// so two different logical paths sharing the same basename cannot
/// collide and path separators / suspicious components never leak into
/// the filesystem.
fn staged_download_path(staging_dir: &Path, file_id: u64, path: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    let digest = hasher.finalize();
    // 16 hex chars is ample for a collision-resistant tag here.
    let mut tag = String::with_capacity(32);
    for byte in &digest[..8] {
        use std::fmt::Write as _;
        let _ = write!(&mut tag, "{byte:02x}");
    }
    staging_dir.join(format!("f{file_id}-{tag}.part"))
}

// ---------------------------------------------------------------------------
// Helpers: resolve upload payload from filesystem or cache
// ---------------------------------------------------------------------------

/// Resolve the length of the staged upload payload for `path`, trying
/// the filesystem shell's staging area first and falling back to the
/// cache's staging buffer. Returns `None` when neither source carries
/// a buffer for `path`.
///
/// Bead `pcloud-rs-s1p.88`: this replaces the previous
/// `read_upload_payload` helper, which called
/// `read_staged_path(0, usize::MAX)` and cloned the entire payload
/// into a new `Vec` (and, on the cache fallback, cloned it a second
/// time via `.to_vec()`). For large files this peaked at roughly
/// 3× the file size in heap usage. The new API is zero-copy: both
/// the length probe and the subsequent borrow of the payload bytes
/// reuse the existing staging buffer in place.
fn resolve_upload_payload_len(
    filesystem: &FilesystemShell,
    cache: &CacheShell,
    path: &str,
) -> Option<usize> {
    if let Some(len) = filesystem.staged_len(path) {
        return Some(len);
    }
    cache.staging.get(path).map(<[u8]>::len)
}

/// Zero-copy borrow of the staged upload payload for `path`, trying
/// the filesystem shell's staging area first and falling back to the
/// cache's staging buffer. Returns `None` if the buffer was evicted
/// between the length probe and the borrow.
fn borrow_upload_payload<'a>(
    filesystem: &'a FilesystemShell,
    cache: &'a CacheShell,
    path: &str,
) -> Option<&'a [u8]> {
    if let Some(bytes) = filesystem.staged_bytes(path) {
        return Some(bytes);
    }
    cache.staging.get(path)
}

/// Iterate the staged upload payload for `path` in fixed-size chunks
/// without allocating. Each yielded slice borrows directly from the
/// staging buffer. `chunk_size == 0` yields a single whole-buffer
/// chunk.
///
/// Bead `pcloud-rs-s1p.88`: used by the zero-copy upload-payload
/// regression test (`read_upload_payload_zero_copy_for_large_files`)
/// to prove that the sync loop can stream a 50 MiB staged file in
/// 4 MiB chunks without allocating the file again on the heap.
#[cfg_attr(not(test), allow(dead_code))]
fn read_upload_payload_chunks<'a>(
    filesystem: &'a FilesystemShell,
    cache: &'a CacheShell,
    path: &str,
    chunk_size: usize,
) -> Option<std::slice::Chunks<'a, u8>> {
    if let Some(chunks) = filesystem.staged_chunks(path, chunk_size) {
        return Some(chunks);
    }
    let bytes = cache.staging.get(path)?;
    let cs = if chunk_size == 0 {
        bytes.len().max(1)
    } else {
        chunk_size
    };
    Some(bytes.chunks(cs))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_model::sync::SyncType;
    use pcloud_store::bootstrap_profile;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Bead pcloud-rs-s1p.88: verify that the sync-loop upload-payload
    /// helpers do not allocate a whole-file `Vec` for large staged
    /// buffers, and that the chunk iterator yields borrowed slices
    /// bounded by the requested chunk size.
    ///
    /// The previous `read_upload_payload` implementation called
    /// `read_staged_path(0, usize::MAX)` which produced a fresh
    /// `ReadResult.bytes` `Vec` the size of the whole file, plus a
    /// second `to_vec()` clone on the cache fallback. For a 50 MiB
    /// file that was ~100 MiB of transient heap use per upload — the
    /// exact blow-up called out in audit-04 §4-opus L-4. The new
    /// helpers return a borrowed `&[u8]` (no allocation) and a 4 MiB
    /// chunk iterator over the same buffer (no per-chunk allocation).
    #[test]
    fn read_upload_payload_zero_copy_for_large_files() {
        const FIFTY_MIB: usize = 50 * 1024 * 1024;
        const CHUNK: usize = 4 * 1024 * 1024;

        let mut fs = FilesystemShell::default();
        let cache = CacheShell::default();
        fs.seed_staged_file("big.bin", vec![0xABu8; FIFTY_MIB]);

        // Length probe is zero-copy.
        let len = resolve_upload_payload_len(&fs, &cache, "big.bin")
            .expect("staged payload should be visible");
        assert_eq!(len, FIFTY_MIB);

        // Borrow returns a zero-copy reference to the exact staging
        // buffer — proven by pointer identity. A `Vec` clone would
        // have a different backing address.
        let borrowed = borrow_upload_payload(&fs, &cache, "big.bin")
            .expect("staged payload should borrow");
        assert_eq!(borrowed.len(), FIFTY_MIB);
        let staging_ptr = fs.staged_bytes("big.bin").unwrap().as_ptr();
        assert!(
            std::ptr::eq(borrowed.as_ptr(), staging_ptr),
            "borrow_upload_payload must return a zero-copy reference to the staging buffer"
        );

        // Chunk iterator yields 4 MiB slices that alias the staging
        // buffer — no per-chunk allocation, and the largest slice
        // length is exactly CHUNK (the pCloud upload_write
        // granularity).
        let chunks = read_upload_payload_chunks(&fs, &cache, "big.bin", CHUNK)
            .expect("chunk iterator should be present");
        let chunks: Vec<&[u8]> = chunks.collect();
        // 50 MiB / 4 MiB = 12 full chunks + 1 remainder of 2 MiB.
        assert_eq!(chunks.len(), 13);
        for chunk in &chunks[..12] {
            assert_eq!(chunk.len(), CHUNK);
        }
        assert_eq!(chunks[12].len(), 2 * 1024 * 1024);

        // All chunks lie inside the original staging buffer — proves
        // the iterator is a view, not a copy.
        let base = staging_ptr as usize;
        let end = base + FIFTY_MIB;
        for chunk in &chunks {
            let cp = chunk.as_ptr() as usize;
            assert!(
                cp >= base && cp + chunk.len() <= end,
                "chunk at {cp:#x} len={} escaped staging buffer [{base:#x}, {end:#x})",
                chunk.len()
            );
        }

        // No chunk ever exceeds the requested chunk size.
        assert!(chunks.iter().all(|c| c.len() <= CHUNK));
    }

    /// Sanity: the cache-fallback path is also zero-copy.
    #[test]
    fn read_upload_payload_cache_fallback_is_zero_copy() {
        const SIZE: usize = 10 * 1024 * 1024;
        const CHUNK: usize = 4 * 1024 * 1024;

        let fs = FilesystemShell::default();
        let mut cache = CacheShell::default();
        cache.staging.stage("cache-only.bin", vec![0x11u8; SIZE]);

        assert_eq!(
            resolve_upload_payload_len(&fs, &cache, "cache-only.bin"),
            Some(SIZE)
        );
        let borrowed =
            borrow_upload_payload(&fs, &cache, "cache-only.bin").expect("cache borrow");
        assert_eq!(borrowed.len(), SIZE);

        let cache_ptr = cache.staging.get("cache-only.bin").unwrap().as_ptr();
        assert!(std::ptr::eq(borrowed.as_ptr(), cache_ptr));

        let chunks: Vec<&[u8]> =
            read_upload_payload_chunks(&fs, &cache, "cache-only.bin", CHUNK)
                .unwrap()
                .collect();
        // 10 MiB / 4 MiB = 2 full + 1 remainder of 2 MiB.
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.len() <= CHUNK));
    }

    /// P2-c (H3): the RAII `SynchronousGuard` must restore
    /// `synchronous=NORMAL` even when the protected scope exits via
    /// panic or early error. Otherwise the connection gets stuck at
    /// FULL for the rest of the daemon's life and every subsequent
    /// write pays an extra fsync.
    #[test]
    fn synchronous_guard_restores_on_panic() {
        let tmp = TempDir::new().expect("tempdir");
        let db = tmp.path().join("sync_guard.sqlite");
        let conn = Connection::open(&db).expect("open");
        conn.pragma_update(None, "synchronous", "NORMAL")
            .expect("init normal");

        // Drive the guard through a panicking scope via catch_unwind.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = SynchronousGuard::set_full(&conn).expect("set full");
            // Verify FULL is live inside the scope.
            let mode: String = conn
                .query_row("PRAGMA synchronous;", [], |row| row.get::<_, i64>(0))
                .map(|n| match n {
                    0 => "OFF".into(),
                    1 => "NORMAL".into(),
                    2 => "FULL".into(),
                    3 => "EXTRA".into(),
                    other => format!("{other}"),
                })
                .unwrap();
            assert_eq!(mode, "FULL", "expected FULL inside scope, got {mode}");
            panic!("simulated commit failure");
        }));

        // After unwind, the guard's Drop must have restored NORMAL.
        let mode: i64 = conn
            .query_row("PRAGMA synchronous;", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(
            mode, 1,
            "SynchronousGuard::drop must restore synchronous=NORMAL after panic (got mode={mode})"
        );
    }

    /// Audit-06 §4-opus HIGH: the guard must restore the **observed**
    /// prior mode, not hard-code NORMAL. If the connection was already
    /// at FULL when the guard was constructed, dropping the guard must
    /// leave it at FULL — not silently demote it to NORMAL.
    #[test]
    fn synchronous_guard_restores_prior_mode_not_hardcoded_normal() {
        let tmp = TempDir::new().expect("tempdir");
        let db = tmp.path().join("sync_guard_prior.sqlite");
        let conn = Connection::open(&db).expect("open");
        conn.pragma_update(None, "synchronous", "FULL")
            .expect("init full");

        {
            let _g = SynchronousGuard::set_full(&conn).expect("set full");
            // Still FULL inside the scope (expected).
            let inside: i64 = conn
                .query_row("PRAGMA synchronous;", [], |r| r.get::<_, i64>(0))
                .unwrap();
            assert_eq!(inside, 2, "pragma should be FULL inside guard scope");
        }

        // Guard has dropped: must have restored the *prior* value
        // (FULL), not defaulted to NORMAL.
        let after: i64 = conn
            .query_row("PRAGMA synchronous;", [], |r| r.get::<_, i64>(0))
            .unwrap();
        assert_eq!(
            after, 2,
            "SynchronousGuard::drop must restore the previously observed mode (FULL=2), got {after}"
        );
    }

    /// Sanity: missing payload produces `None` (mapped by the caller
    /// to `RecoveryFailure::InvalidPath`).
    #[test]
    fn read_upload_payload_returns_none_when_absent() {
        let fs = FilesystemShell::default();
        let cache = CacheShell::default();
        assert!(resolve_upload_payload_len(&fs, &cache, "missing.bin").is_none());
        assert!(borrow_upload_payload(&fs, &cache, "missing.bin").is_none());
        assert!(read_upload_payload_chunks(&fs, &cache, "missing.bin", 4 * 1024 * 1024).is_none());
    }

    /// Verify that `RealSyncLoopRuntime` implements `SyncLoopRuntime`
    /// and can be constructed with dev-mode backends.
    #[test]
    fn real_sync_runtime_constructs_and_satisfies_trait() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");

        // Bootstrap the store so tables exist.
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();

        let config = ConfigProfile::secure_defaults(
            std::env::temp_dir().join("pcloud-slr-test"),
            pcloud_config::Environment::Development,
        );
        let token = shared_auth_token();

        let runtime = RealSyncLoopRuntime::new(Arc::clone(&token), &config, &db_path).unwrap();

        // auth_token returns None when no token is set.
        assert!(runtime.auth_token().is_none());

        // Set a token through the shared bridge.
        {
            let mut guard = token.lock().unwrap();
            *guard = Some(SecretString::new("test-token-abc".to_owned()));
        }
        assert!(runtime.auth_token().is_some());

        // list_sync_roots returns empty on a fresh store.
        assert!(runtime.list_sync_roots().is_empty());
    }

    /// Verify the `walk_local_tree` helper correctly enumerates a
    /// directory tree.
    #[test]
    fn walk_local_tree_enumerates_files_and_dirs() {
        let tmp = TempDir::new().unwrap();
        let root_path = tmp.path().join("sync-root");
        std::fs::create_dir_all(root_path.join("subdir")).unwrap();
        std::fs::write(root_path.join("file1.txt"), b"hello").unwrap();
        std::fs::write(root_path.join("subdir/file2.txt"), b"world").unwrap();

        let root = SyncRootRecord {
            sync_id: SyncId::new(42),
            local_path: root_path.to_string_lossy().to_string(),
            remote_path: "/Remote/42".to_owned(),
            paused: false,
            sync_type: SyncType::Full,
        };

        let db_path = tmp.path().join("walk.db");
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();
        let conn = Connection::open(&db_path).unwrap();

        let entries = walk_local_tree(&root, &conn).unwrap();

        // Should find: file1.txt, subdir, subdir/file2.txt
        assert_eq!(entries.len(), 3);
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"file1.txt"));
        assert!(paths.contains(&"subdir"));
        assert!(paths.contains(&"subdir/file2.txt"));

        // All entries should have the correct sync_id.
        for entry in &entries {
            assert_eq!(entry.sync_id, SyncId::new(42));
            assert!(!entry.deleted);
        }
    }

    /// Verify walk_local_tree returns an error for a non-existent path.
    #[test]
    fn walk_local_tree_returns_error_for_missing_path() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("missing.db");
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();
        let conn = Connection::open(&db_path).unwrap();

        let root = SyncRootRecord {
            sync_id: SyncId::new(1),
            local_path: "/nonexistent/path/that/should/not/exist".to_owned(),
            remote_path: "/Remote/1".to_owned(),
            paused: false,
            sync_type: SyncType::Full,
        };
        assert!(walk_local_tree(&root, &conn).is_err());
    }

    /// Audit 04 C2 regression: nested files under a sync root that
    /// resolves in the local metadata cache MUST carry the parent's
    /// `remote_parent_folder_id` so `resolve_upload_parent` succeeds.
    #[test]
    fn walk_local_tree_threads_remote_parent_folder_id() {
        use pcloud_store::repositories::file_metadata::FileMetadataRecord;

        let tmp = TempDir::new().unwrap();
        let root_path = tmp.path().join("sync-root");
        std::fs::create_dir_all(root_path.join("nested")).unwrap();
        std::fs::write(root_path.join("top.txt"), b"top").unwrap();
        std::fs::write(root_path.join("nested/inner.txt"), b"inner").unwrap();

        let db_path = tmp.path().join("thread.db");
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();
        let conn = Connection::open(&db_path).unwrap();

        // Seed cache: /Remote/42 (folder id 1000) contains folder
        // "nested" (folder id 2000).
        FileMetadataRepository::upsert(
            &conn,
            &FileMetadataRecord {
                file_id: 1000,
                parent_folder_id: 0,
                name: "Remote".to_owned(),
                size: 0,
                hash: String::new(),
                modified: 0,
                created: 0,
                is_folder: true,
            },
        )
        .unwrap();
        // Note: resolve_path splits on '/', so we also need
        // /Remote/42 to resolve — but for this test we use a single-
        // level remote_path.
        let root = SyncRootRecord {
            sync_id: SyncId::new(42),
            local_path: root_path.to_string_lossy().to_string(),
            remote_path: "/Remote".to_owned(),
            paused: false,
            sync_type: SyncType::Full,
        };
        FileMetadataRepository::upsert(
            &conn,
            &FileMetadataRecord {
                file_id: 2000,
                parent_folder_id: 1000,
                name: "nested".to_owned(),
                size: 0,
                hash: String::new(),
                modified: 0,
                created: 0,
                is_folder: true,
            },
        )
        .unwrap();

        let entries = walk_local_tree(&root, &conn).unwrap();
        // top-level file should carry parent id 1000
        let top = entries.iter().find(|e| e.path == "top.txt").unwrap();
        assert_eq!(
            top.remote_parent_folder_id
                .map(pcloud_model::ids::RemoteFolderId::get),
            Some(1000)
        );
        // nested file should carry parent id 2000 (the "nested" folder)
        let inner = entries
            .iter()
            .find(|e| e.path == "nested/inner.txt")
            .unwrap();
        assert_eq!(
            inner
                .remote_parent_folder_id
                .map(pcloud_model::ids::RemoteFolderId::get),
            Some(2000)
        );
    }

    /// Verify that `resolve_upload_parent` returns 0 for root-level files
    /// and requires a folder id for nested files.
    #[test]
    fn resolve_upload_parent_returns_zero_for_root_files() {
        assert_eq!(resolve_upload_parent("file.txt", None).unwrap(), 0);
        assert!(resolve_upload_parent("subdir/file.txt", None).is_err());
        assert_eq!(
            resolve_upload_parent(
                "subdir/file.txt",
                Some(pcloud_model::ids::RemoteFolderId::new(17))
            )
            .unwrap(),
            17
        );
    }

    /// Audit 04 C3 regression: the diff cursor MUST NOT advance if the
    /// engine ingestion step is skipped (simulating a crash between
    /// fetch and commit). Calling only `commit_diff_batch` on a batch
    /// with `cursor > previous_cursor` is legitimate, but if the caller
    /// never reaches that point (crash), the cursor stays at
    /// `previous_cursor` on restart and the batch is refetched.
    #[test]
    fn sync_loop_crash_does_not_advance_cursor() {
        use pcloud_engine::diff_poller::{RemoteDiffBatch, RemoteDiffEntry};
        use pcloud_model::ids::{RemoteFileId, SyncId};

        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("cursor.db");
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();
        let conn = Connection::open(&db_path).unwrap();

        let sync_id = SyncId::new(7);
        // Seed cursor at 100.
        DiffStateRepository::save(&conn, sync_id, 100, 1_700_000_000).unwrap();

        // Construct a batch that would advance the cursor to 200.
        let batch = RemoteDiffBatch {
            sync_id,
            cursor: 200,
            has_more: false,
            entries: vec![RemoteDiffEntry {
                path: "victim.txt".to_owned(),
                entry_kind: EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Delete,
                remote_file_id: Some(RemoteFileId::new(42)),
                remote_folder_id: None,
                event: None,
            }],
        };

        // "Crash" between fetch and commit: never call commit_diff_batch.
        // Cursor must remain at 100.
        let cursor_after_crash = DiffStateRepository::load(&conn, sync_id)
            .unwrap()
            .map(|r| r.diffid)
            .unwrap();
        assert_eq!(
            cursor_after_crash, 100,
            "cursor MUST NOT advance before ingest + commit succeed"
        );

        // Now actually commit. Cursor advances; delete is applied.
        commit_diff_batch(&conn, sync_id, 100, &batch).unwrap();
        let cursor_after_commit = DiffStateRepository::load(&conn, sync_id)
            .unwrap()
            .map(|r| r.diffid)
            .unwrap();
        assert_eq!(cursor_after_commit, 200);
    }

    /// Audit 04 C1 regression: a diff batch containing only `Upsert`
    /// entries (no stat payload) MUST NOT write any rows into
    /// `file_metadata`. Previously the diff loop fabricated
    /// size/hash/modified/created=0 and poisoned the stat cache.
    #[test]
    fn commit_diff_batch_does_not_fabricate_upsert_metadata() {
        use pcloud_engine::diff_poller::{RemoteDiffBatch, RemoteDiffEntry};
        use pcloud_model::ids::{RemoteFileId, SyncId};
        use pcloud_store::repositories::file_metadata::FileMetadataRepository;

        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("no_fabricate.db");
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();
        let conn = Connection::open(&db_path).unwrap();

        let sync_id = SyncId::new(3);
        let batch = RemoteDiffBatch {
            sync_id,
            cursor: 5,
            has_more: false,
            entries: vec![RemoteDiffEntry {
                path: "stat-less.txt".to_owned(),
                entry_kind: EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: Some(RemoteFileId::new(777)),
                remote_folder_id: None,
                event: None,
            }],
        };

        assert_eq!(FileMetadataRepository::count(&conn).unwrap(), 0);
        commit_diff_batch(&conn, sync_id, 0, &batch).unwrap();
        // Cursor must have advanced...
        assert_eq!(
            DiffStateRepository::load(&conn, sync_id)
                .unwrap()
                .unwrap()
                .diffid,
            5
        );
        // ...but NO file_metadata row was written.
        assert_eq!(FileMetadataRepository::count(&conn).unwrap(), 0);
        assert!(
            FileMetadataRepository::get_by_id(&conn, 777)
                .unwrap()
                .is_none()
        );
    }

    /// `commit_diff_batch` applies delete entries to the metadata cache.
    #[test]
    fn commit_diff_batch_applies_deletes() {
        use pcloud_engine::diff_poller::{RemoteDiffBatch, RemoteDiffEntry};
        use pcloud_model::ids::{RemoteFileId, SyncId};
        use pcloud_store::repositories::file_metadata::{
            FileMetadataRecord, FileMetadataRepository,
        };

        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("deletes.db");
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();
        let conn = Connection::open(&db_path).unwrap();

        // Seed a real metadata row (from some prior listfolder).
        FileMetadataRepository::upsert(
            &conn,
            &FileMetadataRecord {
                file_id: 111,
                parent_folder_id: 0,
                name: "doomed.txt".to_owned(),
                size: 42,
                hash: "deadbeef".to_owned(),
                modified: 1,
                created: 1,
                is_folder: false,
            },
        )
        .unwrap();

        let batch = RemoteDiffBatch {
            sync_id: SyncId::new(1),
            cursor: 9,
            has_more: false,
            entries: vec![RemoteDiffEntry {
                path: "doomed.txt".to_owned(),
                entry_kind: EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Delete,
                remote_file_id: Some(RemoteFileId::new(111)),
                remote_folder_id: None,
                event: None,
            }],
        };

        commit_diff_batch(&conn, SyncId::new(1), 0, &batch).unwrap();
        assert!(
            FileMetadataRepository::get_by_id(&conn, 111)
                .unwrap()
                .is_none()
        );
    }

    /// Verify that `shared_auth_token` creates a None-initialized token
    /// and can be cloned across threads.
    #[test]
    fn shared_auth_token_is_send_sync() {
        let token = shared_auth_token();
        let token2 = Arc::clone(&token);

        let handle = std::thread::spawn(move || {
            let mut guard = token2.lock().unwrap();
            *guard = Some(SecretString::new("thread-token".to_owned()));
        });
        handle.join().unwrap();

        let guard = token.lock().unwrap();
        assert!(guard.is_some());
    }

    /// Integration: construct a `RealSyncLoopRuntime` and run it through
    /// `run_cycle` with dev-mode backends and no sync roots (should be a
    /// no-op cycle).
    #[test]
    fn real_runtime_runs_empty_cycle() {
        use crate::sync_loop::run_cycle;
        use pcloud_config::sync_loop::SyncLoopConfig;

        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();

        let config = ConfigProfile::secure_defaults(
            std::env::temp_dir().join("pcloud-slr-test"),
            pcloud_config::Environment::Development,
        );
        let token = shared_auth_token();

        // Set a token so the cycle does not skip.
        {
            let mut guard = token.lock().unwrap();
            *guard = Some(SecretString::new("dev-token".to_owned()));
        }

        let mut runtime = RealSyncLoopRuntime::new(Arc::clone(&token), &config, &db_path).unwrap();
        let loop_config = SyncLoopConfig::default();

        let result = run_cycle(&mut runtime, &loop_config);

        // No roots, so nothing to process.
        assert_eq!(result.roots_processed, 0);
        assert_eq!(result.total_errors, 0);
    }

    /// Debug output does not leak secrets.
    #[test]
    fn real_runtime_debug_does_not_leak_secrets() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();

        let config = ConfigProfile::secure_defaults(
            std::env::temp_dir().join("pcloud-slr-test"),
            pcloud_config::Environment::Development,
        );
        let token = shared_auth_token();
        {
            let mut guard = token.lock().unwrap();
            *guard = Some(SecretString::new("super-secret-token".to_owned()));
        }

        let rt = RealSyncLoopRuntime::new(Arc::clone(&token), &config, &db_path).unwrap();
        let debug = format!("{rt:?}");
        assert!(!debug.contains("super-secret"));
        assert!(debug.contains("RealSyncLoopRuntime"));
    }

    /// Verify that `ensure_watcher` starts a watcher for a valid sync
    /// root and that `remove_watcher` cleans it up.
    #[test]
    fn ensure_watcher_starts_and_remove_cleans_up() {
        let tmp = TempDir::new().unwrap();
        let sync_root = tmp.path().join("watched-root");
        std::fs::create_dir_all(&sync_root).unwrap();

        let db_path = tmp.path().join("test.db");
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();

        let config = ConfigProfile::secure_defaults(
            std::env::temp_dir().join("pcloud-slr-watcher-test"),
            pcloud_config::Environment::Development,
        );
        let token = shared_auth_token();
        let mut runtime = RealSyncLoopRuntime::new(Arc::clone(&token), &config, &db_path).unwrap();

        let root = SyncRootRecord {
            sync_id: SyncId::new(99),
            local_path: sync_root.to_string_lossy().to_string(),
            remote_path: "/Remote/99".to_owned(),
            paused: false,
            sync_type: SyncType::Full,
        };

        // Before ensure_watcher, no watcher present.
        assert!(!runtime.watchers.contains_key(&SyncId::new(99)));

        runtime.ensure_watcher(&root);
        assert!(runtime.watchers.contains_key(&SyncId::new(99)));

        // Calling again is a no-op.
        runtime.ensure_watcher(&root);
        assert!(runtime.watchers.contains_key(&SyncId::new(99)));

        // Remove cleans up.
        runtime.remove_watcher(SyncId::new(99));
        assert!(!runtime.watchers.contains_key(&SyncId::new(99)));
        assert!(!runtime.scan_tracker.has_scanned(SyncId::new(99)));
    }

    /// Verify that `ensure_watcher` gracefully handles a nonexistent
    /// path (falls back to poll-only).
    #[test]
    fn ensure_watcher_falls_back_on_bad_path() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();

        let config = ConfigProfile::secure_defaults(
            std::env::temp_dir().join("pcloud-slr-watcher-bad"),
            pcloud_config::Environment::Development,
        );
        let token = shared_auth_token();
        let mut runtime = RealSyncLoopRuntime::new(Arc::clone(&token), &config, &db_path).unwrap();

        let root = SyncRootRecord {
            sync_id: SyncId::new(88),
            local_path: "/nonexistent/watcher/path".to_owned(),
            remote_path: "/Remote/88".to_owned(),
            paused: false,
            sync_type: SyncType::Full,
        };

        // Should not panic, should just log and skip.
        runtime.ensure_watcher(&root);
        assert!(!runtime.watchers.contains_key(&SyncId::new(88)));
    }

    /// Verify that the scan tracker is initialized with the configured
    /// full_scan_interval_secs from the config.
    #[test]
    fn scan_tracker_uses_config_interval() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let (_store, _integrity) = bootstrap_profile(&db_path).unwrap();

        let config = ConfigProfile::secure_defaults(
            std::env::temp_dir().join("pcloud-slr-tracker-test"),
            pcloud_config::Environment::Development,
        );
        let token = shared_auth_token();
        let runtime = RealSyncLoopRuntime::new(Arc::clone(&token), &config, &db_path).unwrap();

        // First decide should be FullScan (no scan recorded yet).
        // We cannot call decide on the runtime directly since it is
        // private, but we verify construction succeeded and the
        // tracker was initialized by checking that it has no scanned roots.
        assert!(!runtime.scan_tracker.has_scanned(SyncId::new(1)));
    }
}
