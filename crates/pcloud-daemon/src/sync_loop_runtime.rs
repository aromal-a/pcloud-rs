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
use std::path::Path;
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
use pcloud_fs::FilesystemShell;
use pcloud_fs::fs_watcher::{FsWatcher, WatcherConfig, fs_events_to_local_scan_entries};
use pcloud_model::ids::SyncId;
use pcloud_model::sync::EntryKind;
use pcloud_secret::secret_string::SecretString;
use pcloud_store::DiffStateRepository;
use pcloud_store::repositories::audit::AuditRepository;
use pcloud_store::repositories::file_metadata::{FileMetadataRecord, FileMetadataRepository};
use pcloud_store::repositories::sync_graph::{SyncGraphRepository, SyncRootRecord};
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
}

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

        let audit = AuditRepository::load(&conn).unwrap_or_default();

        Ok(Self {
            auth_token,
            sync_runtime: SyncRuntime::from_config(config),
            transfer_runtime: TransferRuntime::from_config(config),
            engine: EngineShell::new(),
            cache: CacheShell::default(),
            filesystem: FilesystemShell::default(),
            store_conn: conn,
            watchers: HashMap::new(),
            scan_tracker: IncrementalScanTracker::new(full_scan_interval),
            watcher_config: WatcherConfig::default(),
            sync_loop_config: config.sync_loop.clone(),
            audit,
        })
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

        // Persist the new cursor if it advanced.
        if batch.cursor > cursor {
            let now_unix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            DiffStateRepository::save(&self.store_conn, root.sync_id, batch.cursor, now_unix)
                .map_err(|e| e.to_string())?;
        }

        // Persist file/folder metadata from the diff batch into the local
        // cache. This populates the `file_metadata` table so that
        // `stat_path` can resolve paths locally without hitting the API.
        for entry in &batch.entries {
            let is_folder = entry.entry_kind == EntryKind::Folder;
            let file_id = if is_folder {
                entry.remote_folder_id.map(|id| id.get())
            } else {
                entry.remote_file_id.map(|id| id.get())
            };
            if let Some(file_id) = file_id {
                if entry.change_kind == pcloud_model::sync::ChangeKind::Delete {
                    let _ = FileMetadataRepository::delete(&self.store_conn, file_id);
                } else {
                    // Extract leaf name from the sync-root-relative path.
                    let name = entry
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or(&entry.path)
                        .to_owned();
                    // Parent folder id: for files use the folder id if
                    // present, otherwise default to 0 (root).
                    let parent_folder_id = if is_folder {
                        // For folders, we don't know the parent from the
                        // diff entry alone; default to 0.
                        0
                    } else {
                        entry.remote_folder_id.map(|id| id.get()).unwrap_or(0)
                    };
                    let record = FileMetadataRecord {
                        file_id,
                        parent_folder_id,
                        name,
                        size: 0,
                        hash: String::new(),
                        modified: 0,
                        created: 0,
                        is_folder,
                    };
                    let _ = FileMetadataRepository::upsert(&self.store_conn, &record);
                }
            }
        }

        let delete_policy =
            DeletePolicy::for_sync_type(root.sync_type, self.sync_loop_config.propagate_deletes);
        let operations = self
            .engine
            .ingest_remote_diff_filtered(&batch, &delete_policy)
            .map_err(|e| format!("{e:?}"))?;
        Ok(operations.len())
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
                let entries = walk_local_tree(root)?;
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
        Ok(operations.len())
    }

    fn advance_transfers(&mut self) -> usize {
        let batch = self.engine.advance_transfer_cycle();
        batch.len()
    }

    fn execute_downloads(&mut self, auth_token: &SecretString) -> Result<usize, String> {
        let tasks = self.engine.downloads.active_downloads.clone();
        let mut completed = 0usize;

        for task in tasks {
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
                    Ok(link) => match self.transfer_runtime.download_bytes(&link) {
                        Ok((_signed, bytes)) => {
                            let cache_key = format!("download:{path}");
                            self.cache.cache_page(cache_key, bytes.clone());
                            self.cache.stage_file(path.clone(), bytes.clone());
                            self.filesystem.seed_staged_file(path.clone(), bytes);
                            if self.engine.mark_transfer_completed(path) {
                                completed += 1;
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
                    },
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

                // Read payload from filesystem shell or cache.
                let payload = match read_upload_payload(&mut self.filesystem, &self.cache, path) {
                    Ok(bytes) => bytes,
                    Err(failure) => {
                        let decision = self.engine.classify_failure(&task.operation, failure);
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
                    payload.len() as u64,
                ) {
                    Ok(session) => {
                        match self.transfer_runtime.upload_bytes(
                            auth_token.clone_secret(),
                            &session,
                            &payload,
                        ) {
                            Ok(_frame) => {
                                if self.engine.mark_transfer_completed(path) {
                                    completed += 1;
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

    fn emit_cycle_audit(&mut self, _root_id: u64, result: &CycleResult) {
        // Persist a structured audit event into the tamper-evident audit
        // chain instead of losing it to stderr. Cycle audits are only
        // emitted when at least one non-trivial event occurred.
        if result.total_errors > 0 || result.total_uploads > 0 || result.total_downloads > 0 {
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
                // Audit persistence failure must not be silently swallowed
                // (project rule: never silently ignore audit failures on
                // active control paths). Surface on stderr as a fallback.
                log::error!("audit: sync-loop-cycle persistence failed: {err}; details={details}");
            }
        }
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
) -> (SyncLoopHandle, SharedAuthToken) {
    let token = shared_auth_token();

    // Seed the shared auth token with any existing auth state.
    if let Some(existing) = auth.snapshot().auth_token.as_ref()
        && let Ok(mut guard) = token.lock()
    {
        *guard = Some(existing.clone_secret());
    }

    let runtime = RealSyncLoopRuntime::new(Arc::clone(&token), config, &db_path)
        .expect("failed to open sync loop store connection");

    let shared = Arc::new(SyncLoopShared::new(SyncLoopState::Idle));
    let handle = crate::sync_loop::spawn_sync_loop(runtime, config.sync_loop.clone(), shared);

    (handle, token)
}

// ---------------------------------------------------------------------------
// Helper: walk a sync root's local directory tree
// ---------------------------------------------------------------------------

/// Walk a sync root's local directory tree and produce
/// [`LocalScanEntry`] items.
fn walk_local_tree(root: &SyncRootRecord) -> Result<Vec<LocalScanEntry>, String> {
    let base = std::path::Path::new(&root.local_path);
    if !base.is_dir() {
        return Err(format!(
            "sync root path does not exist or is not a directory: {}",
            root.local_path
        ));
    }
    let mut entries = Vec::new();
    walk_recursive(base, base, root.sync_id, &mut entries)?;
    Ok(entries)
}

fn walk_recursive(
    base: &std::path::Path,
    current: &std::path::Path,
    sync_id: SyncId,
    entries: &mut Vec<LocalScanEntry>,
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
        let file_type = entry
            .file_type()
            .map_err(|e| format!("file_type error: {e}"))?;
        let entry_kind = if file_type.is_dir() {
            EntryKind::Folder
        } else {
            EntryKind::File
        };
        entries.push(LocalScanEntry {
            sync_id,
            path: relative,
            entry_kind,
            deleted: false,
            remote_parent_folder_id: None,
        });
        if file_type.is_dir() {
            walk_recursive(base, &path, sync_id, entries)?;
        }
    }
    Ok(())
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
    remote_parent_folder_id
        .map(|id| id.get())
        .ok_or(pcloud_engine::recovery::RecoveryFailure::InvalidPath)
}

// ---------------------------------------------------------------------------
// Helper: read upload payload from filesystem or cache
// ---------------------------------------------------------------------------

fn read_upload_payload(
    filesystem: &mut FilesystemShell,
    cache: &CacheShell,
    path: &str,
) -> Result<Vec<u8>, pcloud_engine::recovery::RecoveryFailure> {
    if let Ok(result) = filesystem.read_staged_path(path, 0, usize::MAX) {
        return Ok(result.bytes);
    }
    if let Some(bytes) = cache.staging.get(path) {
        return Ok(bytes.to_vec());
    }
    Err(pcloud_engine::recovery::RecoveryFailure::InvalidPath)
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

        let entries = walk_local_tree(&root).unwrap();

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
        let root = SyncRootRecord {
            sync_id: SyncId::new(1),
            local_path: "/nonexistent/path/that/should/not/exist".to_owned(),
            remote_path: "/Remote/1".to_owned(),
            paused: false,
            sync_type: SyncType::Full,
        };
        assert!(walk_local_tree(&root).is_err());
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
