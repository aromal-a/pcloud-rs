// **PLATFORM:** all
// **GATING:** none (portable).

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use pcloud_model::{
    ids::{RemoteFolderId, SyncId},
    sync::{ChangeKind, ChangeSource, EntryKind, SyncCandidate},
};

use crate::fs_events::FsEvent;
use crate::selective::SelectivePolicy;

/// Local filesystem scanner state. Tracks how frequently a full
/// walk of each sync root should run to catch events missed by the
/// inotify path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalScanner {
    /// Interval between full walks of each sync root, in seconds.
    pub full_scan_interval_secs: u64,
}

impl Default for LocalScanner {
    fn default() -> Self {
        Self {
            full_scan_interval_secs: 300,
        }
    }
}

/// One entry yielded by the local scanner for a sync root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalScanEntry {
    /// Sync root this entry belongs to.
    pub sync_id: SyncId,
    /// Sync-root-relative path for the entry.
    pub path: String,
    /// Whether this entry is a file or a folder.
    pub entry_kind: EntryKind,
    /// `true` if the scanner observed that the entry has been removed
    /// locally since the last scan.
    pub deleted: bool,
    /// Remote parent folder id when known, used to route the candidate
    /// to the correct remote destination.
    pub remote_parent_folder_id: Option<RemoteFolderId>,
}

/// Error returned when a [`LocalScanEntry`] cannot be normalized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalScanError {
    /// The entry's path is absolute, empty, contains `..`, or is
    /// otherwise unsafe to interpret as a sync-root-relative path.
    InvalidPath(String),
}

impl LocalScanner {
    /// Convert a batch of [`LocalScanEntry`] into [`SyncCandidate`]
    /// operations with no selective-sync filtering applied.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::local_scan::{LocalScanEntry, LocalScanner};
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::EntryKind;
    ///
    /// let scanner = LocalScanner::default();
    /// let entries = vec![LocalScanEntry {
    ///     sync_id: SyncId::new(1),
    ///     path: "docs/report.md".into(),
    ///     entry_kind: EntryKind::File,
    ///     deleted: false,
    ///     remote_parent_folder_id: None,
    /// }];
    /// let candidates = scanner.normalize_entries(&entries).unwrap();
    /// assert_eq!(candidates.len(), 1);
    /// ```
    pub fn normalize_entries(
        &self,
        entries: &[LocalScanEntry],
    ) -> Result<Vec<SyncCandidate>, LocalScanError> {
        entries.iter().map(normalize_entry).collect()
    }

    /// Normalize scan entries while honoring a selective-sync policy.
    ///
    /// Entries whose relative path is rejected by `policy.matches` are
    /// silently skipped. `Delete` entries are always kept regardless of
    /// policy so that remote cleanup for a previously-synced-then-now-
    /// excluded path is still honored.
    pub fn normalize_entries_filtered(
        &self,
        entries: &[LocalScanEntry],
        policy: &SelectivePolicy,
    ) -> Result<Vec<SyncCandidate>, LocalScanError> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let candidate = normalize_entry(entry)?;
            if !entry.deleted && !policy.matches(&entry.path) {
                continue;
            }
            out.push(candidate);
        }
        Ok(out)
    }
}

/// Outcome of a call to [`IncrementalScanTracker::decide`].
///
/// Tells the caller whether to perform a full filesystem walk or
/// only process pending watcher events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanDecision {
    /// A full directory walk is needed because either: (a) no full
    /// scan has been done yet for this root, or (b) the configured
    /// `full_scan_interval` has elapsed since the last walk. The
    /// caller should walk the entire tree and also drain any pending
    /// watcher events.
    FullScan,
    /// The full-scan interval has NOT elapsed. The caller should only
    /// process the `pending_events` watcher events that have
    /// accumulated since the last decision. If the vec is empty, no
    /// work is needed this cycle.
    IncrementalOnly {
        /// Filesystem watcher events to process.
        pending_events: Vec<FsEvent>,
    },
}

/// Tracks per-sync-root last-full-scan timestamps and queues incoming
/// filesystem watcher events. Used by the sync loop to decide whether
/// each cycle needs a full tree walk or can get by with watcher events
/// alone.
///
/// # Thread safety
///
/// This type is `Send` but not `Sync` — it lives on the sync loop
/// thread alongside the `EngineShell`.
///
/// # Example
///
/// ```
/// use pcloud_engine::local_scan::{IncrementalScanTracker, ScanDecision};
/// use pcloud_model::ids::SyncId;
/// use std::time::Duration;
///
/// let mut tracker = IncrementalScanTracker::new(Duration::from_secs(300));
///
/// // First call for a root always requests a full scan.
/// assert_eq!(tracker.decide(SyncId::new(1)), ScanDecision::FullScan);
///
/// // Mark the full scan as complete.
/// tracker.record_full_scan(SyncId::new(1));
///
/// // Immediately after, only incremental (no events queued).
/// match tracker.decide(SyncId::new(1)) {
///     ScanDecision::IncrementalOnly { pending_events } => {
///         assert!(pending_events.is_empty());
///     }
///     other => panic!("expected IncrementalOnly, got {other:?}"),
/// }
/// ```
#[derive(Debug, Clone)]
pub struct IncrementalScanTracker {
    /// Configured interval between full walks.
    full_scan_interval: std::time::Duration,
    /// Per-root last-full-scan instant.
    last_full_scan: HashMap<SyncId, Instant>,
    /// Per-root queued watcher events, drained on each `decide` call.
    pending_events: HashMap<SyncId, Vec<FsEvent>>,
}

impl IncrementalScanTracker {
    /// Create a tracker with the given full-scan interval.
    #[must_use]
    pub fn new(full_scan_interval: std::time::Duration) -> Self {
        Self {
            full_scan_interval,
            last_full_scan: HashMap::new(),
            pending_events: HashMap::new(),
        }
    }

    /// Queue a filesystem watcher event for a sync root. The event
    /// will be returned by the next [`decide`](Self::decide) call for
    /// that root (unless a full scan fires, in which case the events
    /// are discarded because the full scan covers them).
    pub fn push_event(&mut self, event: FsEvent) {
        self.pending_events
            .entry(event.sync_id)
            .or_default()
            .push(event);
    }

    /// Decide what scan work is needed for `sync_id` this cycle.
    ///
    /// If no full scan has ever been recorded, or if the full-scan
    /// interval has elapsed, returns [`ScanDecision::FullScan`] and
    /// discards any pending watcher events (they are subsumed by the
    /// walk). Otherwise returns [`ScanDecision::IncrementalOnly`]
    /// with the drained watcher events.
    pub fn decide(&mut self, sync_id: SyncId) -> ScanDecision {
        let now = Instant::now();
        let needs_full = match self.last_full_scan.get(&sync_id) {
            None => true,
            Some(last) => now.duration_since(*last) >= self.full_scan_interval,
        };

        if needs_full {
            // Discard pending events — the full scan covers everything.
            self.pending_events.remove(&sync_id);
            ScanDecision::FullScan
        } else {
            let events = self.pending_events.remove(&sync_id).unwrap_or_default();
            ScanDecision::IncrementalOnly {
                pending_events: events,
            }
        }
    }

    /// Record that a full scan completed for `sync_id`, resetting its
    /// interval timer.
    pub fn record_full_scan(&mut self, sync_id: SyncId) {
        self.last_full_scan.insert(sync_id, Instant::now());
    }

    /// Record a full scan at a specific instant (for testing with
    /// controlled timestamps).
    #[cfg(test)]
    pub fn record_full_scan_at(&mut self, sync_id: SyncId, at: Instant) {
        self.last_full_scan.insert(sync_id, at);
    }

    /// Stop tracking a sync root (called on root removal).
    pub fn untrack(&mut self, sync_id: SyncId) {
        self.last_full_scan.remove(&sync_id);
        self.pending_events.remove(&sync_id);
    }

    /// Number of pending watcher events for a sync root.
    #[must_use]
    pub fn pending_count(&self, sync_id: SyncId) -> usize {
        self.pending_events.get(&sync_id).map_or(0, |v| v.len())
    }

    /// Whether a full scan has ever been recorded for `sync_id`.
    #[must_use]
    pub fn has_scanned(&self, sync_id: SyncId) -> bool {
        self.last_full_scan.contains_key(&sync_id)
    }
}

fn normalize_entry(entry: &LocalScanEntry) -> Result<SyncCandidate, LocalScanError> {
    validate_relative_path(&entry.path)?;
    Ok(SyncCandidate {
        sync_id: entry.sync_id,
        source: ChangeSource::Local,
        path: entry.path.clone(),
        entry_kind: entry.entry_kind,
        change_kind: if entry.deleted {
            ChangeKind::Delete
        } else {
            ChangeKind::Upsert
        },
        remote_file_id: None,
        remote_folder_id: entry.remote_parent_folder_id,
    })
}

/// Thin wrapper over [`crate::is_valid_relative_path`] that maps the
/// shared boolean predicate to the local typed error.
fn validate_relative_path(path: &str) -> Result<(), LocalScanError> {
    if crate::is_valid_relative_path(path) {
        Ok(())
    } else {
        Err(LocalScanError::InvalidPath(path.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use pcloud_model::{
        ids::{RemoteFolderId, SyncId},
        sync::{ChangeKind, ChangeSource, EntryKind, SyncCandidate},
    };

    use super::{LocalScanEntry, LocalScanError, LocalScanner};

    #[test]
    fn normalizes_file_and_directory_scan_entries() {
        let scanner = LocalScanner::default();
        let candidates = scanner
            .normalize_entries(&[
                LocalScanEntry {
                    sync_id: SyncId::new(1),
                    path: "docs/report.txt".to_owned(),
                    entry_kind: EntryKind::File,
                    deleted: false,
                    remote_parent_folder_id: Some(RemoteFolderId::new(9)),
                },
                LocalScanEntry {
                    sync_id: SyncId::new(1),
                    path: "docs/archive".to_owned(),
                    entry_kind: EntryKind::Folder,
                    deleted: false,
                    remote_parent_folder_id: Some(RemoteFolderId::new(8)),
                },
            ])
            .expect("entries should normalize");

        assert_eq!(
            candidates[0],
            SyncCandidate {
                sync_id: SyncId::new(1),
                source: ChangeSource::Local,
                path: "docs/report.txt".to_owned(),
                entry_kind: EntryKind::File,
                change_kind: ChangeKind::Upsert,
                remote_file_id: None,
                remote_folder_id: Some(RemoteFolderId::new(9)),
            }
        );
        assert_eq!(candidates[1].entry_kind, EntryKind::Folder);
        assert_eq!(candidates[1].remote_folder_id, Some(RemoteFolderId::new(8)));
    }

    #[test]
    fn normalizes_deleted_entry_to_delete_candidate() {
        let scanner = LocalScanner::default();
        let candidates = scanner
            .normalize_entries(&[LocalScanEntry {
                sync_id: SyncId::new(5),
                path: "docs/old.txt".to_owned(),
                entry_kind: EntryKind::File,
                deleted: true,
                remote_parent_folder_id: Some(RemoteFolderId::new(11)),
            }])
            .expect("entry should normalize");

        assert_eq!(candidates[0].change_kind, ChangeKind::Delete);
    }

    #[test]
    fn selective_policy_filters_local_scan_entries() {
        use crate::selective::SelectivePolicy;
        let scanner = LocalScanner::default();
        let policy = SelectivePolicy::parse("docs/**\n!docs/secret/**\n").expect("parses");
        let candidates = scanner
            .normalize_entries_filtered(
                &[
                    LocalScanEntry {
                        sync_id: SyncId::new(1),
                        path: "docs/keep.txt".to_owned(),
                        entry_kind: EntryKind::File,
                        deleted: false,
                        remote_parent_folder_id: None,
                    },
                    LocalScanEntry {
                        sync_id: SyncId::new(1),
                        path: "docs/secret/password.txt".to_owned(),
                        entry_kind: EntryKind::File,
                        deleted: false,
                        remote_parent_folder_id: None,
                    },
                    LocalScanEntry {
                        sync_id: SyncId::new(1),
                        path: "bin/ignored".to_owned(),
                        entry_kind: EntryKind::File,
                        deleted: false,
                        remote_parent_folder_id: None,
                    },
                    LocalScanEntry {
                        sync_id: SyncId::new(1),
                        path: "docs/secret/gone.txt".to_owned(),
                        entry_kind: EntryKind::File,
                        deleted: true,
                        remote_parent_folder_id: None,
                    },
                ],
                &policy,
            )
            .expect("filtered normalize");

        let paths: Vec<&str> = candidates.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["docs/keep.txt", "docs/secret/gone.txt"]);
        assert_eq!(candidates[1].change_kind, ChangeKind::Delete);
    }

    #[test]
    fn rejects_invalid_local_scan_paths() {
        let scanner = LocalScanner::default();
        let error = scanner
            .normalize_entries(&[LocalScanEntry {
                sync_id: SyncId::new(3),
                path: "/etc/passwd".to_owned(),
                entry_kind: EntryKind::File,
                deleted: false,
                remote_parent_folder_id: None,
            }])
            .expect_err("invalid path should be rejected");

        assert_eq!(error, LocalScanError::InvalidPath("/etc/passwd".to_owned()));
    }

    // -- IncrementalScanTracker tests --

    use super::{IncrementalScanTracker, ScanDecision};
    use crate::fs_events::{FsEvent, FsEventKind};

    #[test]
    fn tracker_first_decide_returns_full_scan() {
        let mut tracker = IncrementalScanTracker::new(std::time::Duration::from_secs(300));
        assert_eq!(tracker.decide(SyncId::new(1)), ScanDecision::FullScan);
    }

    #[test]
    fn tracker_returns_incremental_after_full_scan() {
        let mut tracker = IncrementalScanTracker::new(std::time::Duration::from_secs(300));
        let _ = tracker.decide(SyncId::new(1));
        tracker.record_full_scan(SyncId::new(1));

        match tracker.decide(SyncId::new(1)) {
            ScanDecision::IncrementalOnly { pending_events } => {
                assert!(pending_events.is_empty());
            }
            other => panic!("expected IncrementalOnly, got {other:?}"),
        }
    }

    #[test]
    fn tracker_drains_pending_events_on_incremental() {
        let mut tracker = IncrementalScanTracker::new(std::time::Duration::from_secs(300));
        let _ = tracker.decide(SyncId::new(1));
        tracker.record_full_scan(SyncId::new(1));

        tracker.push_event(FsEvent {
            sync_id: SyncId::new(1),
            path: "docs/new.txt".to_owned(),
            entry_kind: EntryKind::File,
            kind: FsEventKind::Create,
        });
        tracker.push_event(FsEvent {
            sync_id: SyncId::new(1),
            path: "docs/old.txt".to_owned(),
            entry_kind: EntryKind::File,
            kind: FsEventKind::Remove,
        });

        assert_eq!(tracker.pending_count(SyncId::new(1)), 2);

        match tracker.decide(SyncId::new(1)) {
            ScanDecision::IncrementalOnly { pending_events } => {
                assert_eq!(pending_events.len(), 2);
            }
            other => panic!("expected IncrementalOnly, got {other:?}"),
        }

        // Events are drained after decide.
        assert_eq!(tracker.pending_count(SyncId::new(1)), 0);
    }

    #[test]
    fn tracker_discards_events_on_full_scan() {
        let mut tracker = IncrementalScanTracker::new(std::time::Duration::from_secs(0));
        tracker.record_full_scan(SyncId::new(1));

        tracker.push_event(FsEvent {
            sync_id: SyncId::new(1),
            path: "docs/new.txt".to_owned(),
            entry_kind: EntryKind::File,
            kind: FsEventKind::Create,
        });

        // With interval=0, next decide always triggers full scan.
        // Events should be discarded.
        assert_eq!(tracker.decide(SyncId::new(1)), ScanDecision::FullScan);
        assert_eq!(tracker.pending_count(SyncId::new(1)), 0);
    }

    #[test]
    fn tracker_full_scan_fires_after_interval_elapses() {
        let mut tracker = IncrementalScanTracker::new(std::time::Duration::from_millis(1));
        tracker.record_full_scan_at(
            SyncId::new(1),
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        );

        // Interval has elapsed, so full scan should fire.
        assert_eq!(tracker.decide(SyncId::new(1)), ScanDecision::FullScan);
    }

    #[test]
    fn tracker_untrack_removes_state() {
        let mut tracker = IncrementalScanTracker::new(std::time::Duration::from_secs(300));
        tracker.record_full_scan(SyncId::new(1));
        tracker.push_event(FsEvent {
            sync_id: SyncId::new(1),
            path: "docs/a.txt".to_owned(),
            entry_kind: EntryKind::File,
            kind: FsEventKind::Write,
        });

        tracker.untrack(SyncId::new(1));
        assert!(!tracker.has_scanned(SyncId::new(1)));
        assert_eq!(tracker.pending_count(SyncId::new(1)), 0);

        // After untrack, next decide is a full scan again.
        assert_eq!(tracker.decide(SyncId::new(1)), ScanDecision::FullScan);
    }

    #[test]
    fn tracker_independent_roots_do_not_interfere() {
        let mut tracker = IncrementalScanTracker::new(std::time::Duration::from_secs(300));
        tracker.record_full_scan(SyncId::new(1));

        // Root 2 has never been scanned.
        assert_eq!(tracker.decide(SyncId::new(2)), ScanDecision::FullScan);

        // Root 1 should still be incremental.
        match tracker.decide(SyncId::new(1)) {
            ScanDecision::IncrementalOnly { .. } => {}
            other => panic!("expected IncrementalOnly for root 1, got {other:?}"),
        }
    }
}
