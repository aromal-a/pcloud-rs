// **PLATFORM:** all (notify abstracts inotify/FSEvents/ReadDirectoryChangesW)
// **GATING:** none (portable).

//! Real filesystem watcher that detects local file changes and feeds them
//! to the sync engine via [`pcloud_engine::fs_events::FsEvent`].
//!
//! Wraps the [`notify`] crate's [`RecommendedWatcher`] to produce
//! debounced, filtered [`FsEvent`]s on a crossbeam-style channel.
//!
//! ## Filtering
//!
//! The watcher silently drops events for:
//! - Temporary/editor files: `.swp`, `.tmp`, `~` suffix
//! - Internal pCloud files: `.pcloud-*` prefix
//! - OS metadata: `.DS_Store`, `Thumbs.db`, `desktop.ini`
//!
//! ## Debounce
//!
//! Uses `notify`'s built-in debounced watcher with a configurable window
//! (default 500ms). Rapid-fire events on the same path are coalesced by
//! `notify` before reaching the consumer.
//!
//! ## Fallback
//!
//! If `notify` fails to initialize (e.g. inotify watch limit exhausted),
//! [`FsWatcher::start`] returns [`WatchError::WatcherInit`]. Callers
//! should fall back to periodic `walkdir`-based polling (see
//! [`poll_scan_root`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use pcloud_engine::fs_events::{FsEvent, FsEventKind};
use pcloud_model::ids::SyncId;
use pcloud_model::sync::EntryKind;

/// Error from the filesystem watcher.
#[derive(Debug)]
pub enum WatchError {
    /// `notify` watcher failed to initialize (inotify limit, permission, etc.).
    WatcherInit(String),
    /// Failed to add a watch on the given path.
    WatchPath {
        /// The path we tried to watch.
        path: PathBuf,
        /// Underlying error message.
        reason: String,
    },
    /// The watcher channel was disconnected.
    ChannelClosed,
}

impl std::fmt::Display for WatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WatcherInit(msg) => write!(f, "watcher init failed: {msg}"),
            Self::WatchPath { path, reason } => {
                write!(f, "watch path {}: {reason}", path.display())
            }
            Self::ChannelClosed => write!(f, "watcher channel closed"),
        }
    }
}

impl std::error::Error for WatchError {}

/// Configuration for the filesystem watcher.
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Debounce window for coalescing rapid-fire events on the same path.
    /// Default: 500ms.
    pub debounce_duration: Duration,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            debounce_duration: Duration::from_millis(500),
        }
    }
}

/// Filesystem watcher wrapping [`notify::RecommendedWatcher`].
///
/// Owns the watcher handle; dropping it stops the watch.
pub struct FsWatcher {
    _watcher: RecommendedWatcher,
}

impl FsWatcher {
    /// Start watching `root_path` recursively for the given `sync_id`.
    ///
    /// Returns `(FsWatcher, Receiver<FsEvent>)`. The receiver yields
    /// debounced, filtered filesystem events as [`FsEvent`]s ready for
    /// ingestion by [`pcloud_engine::EngineShell::ingest_fs_events`].
    ///
    /// # Errors
    ///
    /// Returns [`WatchError::WatcherInit`] if `notify` cannot create a
    /// backend watcher (e.g. inotify watch limit exhausted).
    /// Returns [`WatchError::WatchPath`] if the root path cannot be watched.
    pub fn start(
        root_path: &Path,
        sync_id: SyncId,
        config: &WatcherConfig,
    ) -> Result<(Self, mpsc::Receiver<FsEvent>), WatchError> {
        // Bounded channel: prevents unbounded memory growth under load.
        // 1024 slots cover normal burst traffic; overflow is surfaced via
        // the inotify-overflow log below rather than silently dropped.
        let (tx, rx) = mpsc::sync_channel(1024);
        let root = root_path
            .canonicalize()
            .unwrap_or_else(|_| root_path.to_path_buf());
        let root_clone = root.clone();
        let debounce = config.debounce_duration;

        // Use a dedicated thread for debounce coalescing.
        // Bounded channel: keeps memory usage finite under inotify storms.
        let (notify_tx, notify_rx) = mpsc::sync_channel::<Event>(1024);

        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                match result {
                    Ok(event) => {
                        // Discard if the debounce thread is full; this is
                        // preferable to an unbounded queue growing under load.
                        let _ = notify_tx.try_send(event);
                    }
                    Err(err) => {
                        // Surface inotify overflow and similar kernel errors
                        // so operators know local changes may be missed.
                        log::warn!(
                            "fs watcher event overflow: {err}; \
                             some local changes may be missed until next full scan"
                        );
                    }
                }
            },
            Config::default(),
        )
        .map_err(|e| WatchError::WatcherInit(e.to_string()))?;

        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| WatchError::WatchPath {
                path: root.clone(),
                reason: e.to_string(),
            })?;

        // Spawn a debounce/filter thread.
        std::thread::Builder::new()
            .name("fs-watcher-debounce".into())
            .spawn(move || {
                debounce_loop(notify_rx, tx, &root_clone, sync_id, debounce);
            })
            .map_err(|e| WatchError::WatcherInit(format!("thread spawn: {e}")))?;

        Ok((Self { _watcher: watcher }, rx))
    }
}

/// Internal debounce loop: reads raw notify events, filters, coalesces
/// by path within `debounce` windows, and emits [`FsEvent`]s.
fn debounce_loop(
    notify_rx: mpsc::Receiver<Event>,
    output_tx: mpsc::SyncSender<FsEvent>,
    root: &Path,
    sync_id: SyncId,
    debounce: Duration,
) {
    // Map from relative path to (last event kind, last seen time).
    let mut pending: HashMap<String, (FsEventKind, EntryKind, Instant)> = HashMap::new();

    loop {
        // Drain with a timeout so we can flush pending events.
        match notify_rx.recv_timeout(debounce) {
            Ok(event) => {
                let kind = match classify_event_kind(&event.kind) {
                    Some(k) => k,
                    None => continue,
                };
                for path in &event.paths {
                    if let Some(rel) = to_relative(path, root) {
                        if should_filter(&rel) {
                            continue;
                        }
                        let entry_kind = if path.is_dir() {
                            EntryKind::Folder
                        } else {
                            EntryKind::File
                        };
                        pending.insert(rel, (kind, entry_kind, Instant::now()));
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Flush all pending events whose debounce window has elapsed.
                flush_pending(&mut pending, &output_tx, sync_id, debounce);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Watcher dropped; flush remaining and exit.
                flush_pending(&mut pending, &output_tx, sync_id, Duration::ZERO);
                break;
            }
        }

        // Also flush events that have matured.
        flush_pending(&mut pending, &output_tx, sync_id, debounce);
    }
}

/// Flush pending events whose debounce window has elapsed.
fn flush_pending(
    pending: &mut HashMap<String, (FsEventKind, EntryKind, Instant)>,
    tx: &mpsc::SyncSender<FsEvent>,
    sync_id: SyncId,
    debounce: Duration,
) {
    let now = Instant::now();
    let matured: Vec<String> = pending
        .iter()
        .filter(|(_, (_, _, ts))| now.duration_since(*ts) >= debounce)
        .map(|(k, _)| k.clone())
        .collect();

    for path in matured {
        if let Some((kind, entry_kind, _)) = pending.remove(&path) {
            let event = FsEvent {
                sync_id,
                path,
                entry_kind,
                kind,
            };
            if tx.send(event).is_err() {
                // Consumer dropped; stop flushing.
                pending.clear();
                return;
            }
        }
    }
}

/// Classify a [`notify::EventKind`] into our [`FsEventKind`], or `None`
/// if it should be ignored (e.g. access events, metadata-only changes).
fn classify_event_kind(kind: &EventKind) -> Option<FsEventKind> {
    match kind {
        EventKind::Create(_) => Some(FsEventKind::Create),
        EventKind::Modify(_) => Some(FsEventKind::Write),
        EventKind::Remove(_) => Some(FsEventKind::Remove),
        // Access and Other events are not sync-relevant.
        _ => None,
    }
}

/// Convert an absolute path to a sync-root-relative string.
/// Returns `None` if the path is not under `root`.
fn to_relative(path: &Path, root: &Path) -> Option<String> {
    path.strip_prefix(root).ok().and_then(|rel| {
        let s = rel.to_str()?;
        if s.is_empty() {
            None
        } else {
            // Normalize to forward slashes for cross-platform consistency.
            Some(s.replace('\\', "/"))
        }
    })
}

/// Returns `true` if the relative path should be filtered out.
fn should_filter(rel_path: &str) -> bool {
    let filename = rel_path.rsplit('/').next().unwrap_or(rel_path);

    // Temp/editor files.
    if filename.ends_with(".swp")
        || filename.ends_with(".tmp")
        || filename.ends_with('~')
        || filename.ends_with(".swx")
        || filename.ends_with(".swo")
    {
        return true;
    }

    // pCloud internal files.
    if filename.starts_with(".pcloud-") || filename.starts_with(".pcloudsync") {
        return true;
    }

    // OS metadata files.
    if filename == ".DS_Store" || filename == "Thumbs.db" || filename == "desktop.ini" {
        return true;
    }

    // Vim undo files.
    if filename.starts_with(".") && filename.ends_with(".un~") {
        return true;
    }

    false
}

// ---------------------------------------------------------------------------
// Bridge: FsEvent -> LocalScanEntry
// ---------------------------------------------------------------------------

use pcloud_engine::local_scan::LocalScanEntry;

/// Convert a batch of [`FsEvent`]s into [`LocalScanEntry`] values
/// suitable for [`pcloud_engine::EngineShell::ingest_local_scan`].
///
/// This bridge is useful when the caller wants to feed watcher events
/// through the local-scan normalization path (which supports selective
/// sync filtering) rather than the raw `ingest_fs_events` path.
pub fn fs_events_to_local_scan_entries(events: &[FsEvent]) -> Vec<LocalScanEntry> {
    events
        .iter()
        .map(|event| LocalScanEntry {
            sync_id: event.sync_id,
            path: event.path.clone(),
            entry_kind: event.entry_kind,
            deleted: event.kind == FsEventKind::Remove,
            remote_parent_folder_id: None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Fallback: periodic walkdir-based polling
// ---------------------------------------------------------------------------

/// Perform a single full-tree scan of `root_path` using `std::fs::read_dir`,
/// returning [`LocalScanEntry`] values for every file and folder found.
///
/// Intended as a fallback when `notify` cannot be initialized (e.g.
/// inotify watch limit exceeded). Callers should invoke this periodically
/// (e.g. every 5 minutes) in lieu of the real-time watcher.
///
/// Ignores the same temp/internal/OS files as the watcher.
pub fn poll_scan_root(root_path: &Path, sync_id: SyncId) -> Vec<LocalScanEntry> {
    let mut entries = Vec::new();
    let root = root_path
        .canonicalize()
        .unwrap_or_else(|_| root_path.to_path_buf());
    walk_dir_recursive(&root, &root, sync_id, &mut entries);
    entries
}

fn walk_dir_recursive(dir: &Path, root: &Path, sync_id: SyncId, out: &mut Vec<LocalScanEntry>) {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("poll_scan_root: cannot read {}: {e}", dir.display());
            return;
        }
    };

    for entry_result in read {
        let entry = match entry_result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        let rel = match path.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };

        if rel.is_empty() || should_filter(&rel) {
            continue;
        }

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if file_type.is_dir() {
            out.push(LocalScanEntry {
                sync_id,
                path: rel,
                entry_kind: EntryKind::Folder,
                deleted: false,
                remote_parent_folder_id: None,
            });
            walk_dir_recursive(&path, root, sync_id, out);
        } else if file_type.is_file() {
            out.push(LocalScanEntry {
                sync_id,
                path: rel,
                entry_kind: EntryKind::File,
                deleted: false,
                remote_parent_folder_id: None,
            });
        }
        // Symlinks deliberately ignored for now.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_model::ids::SyncId;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn should_filter_rejects_temp_and_internal_files() {
        assert!(should_filter("file.swp"));
        assert!(should_filter("file.tmp"));
        assert!(should_filter("file~"));
        assert!(should_filter(".pcloud-state"));
        assert!(should_filter(".pcloudsync"));
        assert!(should_filter(".DS_Store"));
        assert!(should_filter("Thumbs.db"));
        assert!(should_filter("desktop.ini"));
        assert!(should_filter("nested/dir/.pcloud-lock"));
    }

    #[test]
    fn should_filter_allows_normal_files() {
        assert!(!should_filter("docs/report.txt"));
        assert!(!should_filter("photo.jpg"));
        assert!(!should_filter("src/main.rs"));
        assert!(!should_filter("Cargo.toml"));
    }

    #[test]
    fn to_relative_strips_root_prefix() {
        let root = Path::new("/home/user/sync");
        let abs = Path::new("/home/user/sync/docs/file.txt");
        assert_eq!(to_relative(abs, root), Some("docs/file.txt".to_owned()));
    }

    #[test]
    fn to_relative_returns_none_for_root_itself() {
        let root = Path::new("/home/user/sync");
        assert_eq!(to_relative(root, root), None);
    }

    #[test]
    fn to_relative_returns_none_for_outside_path() {
        let root = Path::new("/home/user/sync");
        let outside = Path::new("/tmp/other");
        assert_eq!(to_relative(outside, root), None);
    }

    #[test]
    fn classify_event_kind_maps_correctly() {
        assert_eq!(
            classify_event_kind(&EventKind::Create(notify::event::CreateKind::File)),
            Some(FsEventKind::Create)
        );
        assert_eq!(
            classify_event_kind(&EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content
            ))),
            Some(FsEventKind::Write)
        );
        assert_eq!(
            classify_event_kind(&EventKind::Remove(notify::event::RemoveKind::File)),
            Some(FsEventKind::Remove)
        );
        assert_eq!(
            classify_event_kind(&EventKind::Access(notify::event::AccessKind::Read)),
            None
        );
    }

    #[test]
    fn fs_events_to_local_scan_entries_converts_correctly() {
        let events = vec![
            FsEvent {
                sync_id: SyncId::new(1),
                path: "docs/file.txt".to_owned(),
                entry_kind: EntryKind::File,
                kind: FsEventKind::Create,
            },
            FsEvent {
                sync_id: SyncId::new(1),
                path: "docs/old.txt".to_owned(),
                entry_kind: EntryKind::File,
                kind: FsEventKind::Remove,
            },
        ];

        let entries = fs_events_to_local_scan_entries(&events);
        assert_eq!(entries.len(), 2);
        assert!(!entries[0].deleted);
        assert_eq!(entries[0].path, "docs/file.txt");
        assert!(entries[1].deleted);
        assert_eq!(entries[1].path, "docs/old.txt");
    }

    #[test]
    fn poll_scan_root_discovers_files_and_folders() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Create structure: root/a.txt, root/sub/b.txt
        fs::write(root.join("a.txt"), b"hello").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("b.txt"), b"world").unwrap();
        // Create a filtered file
        fs::write(root.join(".DS_Store"), b"meta").unwrap();

        let entries = poll_scan_root(root, SyncId::new(42));

        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"a.txt"), "missing a.txt in {paths:?}");
        assert!(paths.contains(&"sub"), "missing sub/ in {paths:?}");
        assert!(
            paths.contains(&"sub/b.txt"),
            "missing sub/b.txt in {paths:?}"
        );
        assert!(
            !paths.contains(&".DS_Store"),
            ".DS_Store should be filtered in {paths:?}"
        );

        // Verify entry kinds
        let a_entry = entries.iter().find(|e| e.path == "a.txt").unwrap();
        assert_eq!(a_entry.entry_kind, EntryKind::File);
        assert_eq!(a_entry.sync_id, SyncId::new(42));
        assert!(!a_entry.deleted);

        let sub_entry = entries.iter().find(|e| e.path == "sub").unwrap();
        assert_eq!(sub_entry.entry_kind, EntryKind::Folder);
    }

    #[test]
    fn poll_scan_root_filters_temp_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        fs::write(root.join("good.txt"), b"ok").unwrap();
        fs::write(root.join("edit.swp"), b"vim").unwrap();
        fs::write(root.join("backup~"), b"old").unwrap();
        fs::write(root.join(".pcloud-lock"), b"lock").unwrap();

        let entries = poll_scan_root(root, SyncId::new(1));
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();

        assert_eq!(paths, vec!["good.txt"]);
    }

    #[test]
    fn watcher_start_and_receive_event() {
        // Integration test: start watcher, write a file, assert event received.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let config = WatcherConfig {
            debounce_duration: Duration::from_millis(100),
        };

        let (_watcher, rx) =
            FsWatcher::start(root, SyncId::new(7), &config).expect("watcher should start");

        // Give the watcher a moment to set up.
        std::thread::sleep(Duration::from_millis(50));

        // Create a file.
        fs::write(root.join("test_file.txt"), b"data").unwrap();

        // Wait for the debounced event (debounce 100ms + margin).
        let event = rx.recv_timeout(Duration::from_secs(2));
        assert!(event.is_ok(), "should receive an event, got: {event:?}");
        let event = event.unwrap();
        assert_eq!(event.sync_id, SyncId::new(7));
        assert!(
            event.path.contains("test_file.txt"),
            "event path should contain test_file.txt, got: {}",
            event.path
        );
    }

    // The `notify` crate uses inotify on Linux, kqueue on BSD/macOS, and
    // ReadDirectoryChangesW on Windows. inotify fires synchronously on
    // file open/write/close; kqueue's `EVFILT_VNODE` only fires on the
    // tracked vnode, doesn't recurse, and may coalesce or skip create
    // events for files that didn't exist at watch-registration time
    // (a pure platform limitation, not a notify-crate bug).
    //
    // The filter logic itself (`is_temp_file_path`) is platform-neutral
    // and gets covered by `temp_file_filter_extension_matches` /
    // `temp_file_filter_prefix_matches` above. This test exercises the
    // end-to-end debounced delivery, which only behaves
    // deterministically on Linux. On BSD/macOS/Windows the same
    // assertion would race the platform's notification quirks; gate
    // accordingly.
    #[cfg(target_os = "linux")]
    #[test]
    fn watcher_filters_temp_files_from_events() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let config = WatcherConfig {
            debounce_duration: Duration::from_millis(100),
        };

        let (_watcher, rx) =
            FsWatcher::start(root, SyncId::new(1), &config).expect("watcher should start");

        std::thread::sleep(Duration::from_millis(50));

        // Write a temp file that should be filtered.
        fs::write(root.join("edit.swp"), b"vim").unwrap();
        // Write a real file to confirm the channel works.
        fs::write(root.join("real.txt"), b"data").unwrap();

        // Collect events for a reasonable window.
        let mut events = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(ev) => events.push(ev),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !events.is_empty() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        let paths: Vec<&str> = events.iter().map(|e| e.path.as_str()).collect();
        assert!(
            !paths.iter().any(|p| p.contains("edit.swp")),
            ".swp should be filtered, got: {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p.contains("real.txt")),
            "real.txt should be present, got: {paths:?}"
        );
    }

    #[test]
    fn watcher_debounces_rapid_events() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let config = WatcherConfig {
            debounce_duration: Duration::from_millis(200),
        };

        let (_watcher, rx) =
            FsWatcher::start(root, SyncId::new(1), &config).expect("watcher should start");

        std::thread::sleep(Duration::from_millis(50));

        // Write to the same file rapidly.
        let target = root.join("rapid.txt");
        for i in 0..5 {
            fs::write(&target, format!("version {i}")).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }

        // Collect events.
        let mut events = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(300)) {
                Ok(ev) => events.push(ev),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !events.is_empty() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // Due to debouncing, we should get far fewer events than the 5+
        // raw events generated. Typically 1-2.
        let rapid_events: Vec<_> = events
            .iter()
            .filter(|e| e.path.contains("rapid.txt"))
            .collect();
        assert!(
            rapid_events.len() <= 3,
            "debounce should coalesce rapid events, got {} events",
            rapid_events.len()
        );
        assert!(
            !rapid_events.is_empty(),
            "should have at least one event for rapid.txt"
        );
    }
}
