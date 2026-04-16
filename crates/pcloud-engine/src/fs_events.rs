// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use pcloud_model::{
    ids::SyncId,
    sync::{ChangeKind, ChangeSource, EntryKind, SyncCandidate},
};

/// Ingests raw local filesystem events (from notify/inotify) and
/// coalesces rapid same-path events before handing them to the planner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsEventIngestor {
    /// Time window (in milliseconds) within which repeated events for
    /// the same path are coalesced into a single candidate.
    pub coalesce_window_ms: u64,
}

impl Default for FsEventIngestor {
    fn default() -> Self {
        Self {
            coalesce_window_ms: 250,
        }
    }
}

/// Kind of local filesystem event observed at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FsEventKind {
    /// Write/modify content at the path.
    Write,
    /// Create a new file or folder at the path.
    Create,
    /// Remove the path (file or folder).
    Remove,
}

/// One local filesystem event observed inside a sync root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsEvent {
    /// Sync root the event belongs to.
    pub sync_id: SyncId,
    /// Sync-root-relative path for the event.
    pub path: String,
    /// Whether the path refers to a file or a folder.
    pub entry_kind: EntryKind,
    /// Kind of change observed at the path.
    pub kind: FsEventKind,
}

/// Error returned when an incoming [`FsEvent`] cannot be normalized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FsEventError {
    /// The event's path is absolute, empty, contains `..`, or is
    /// otherwise unsafe to interpret as a sync-root-relative path.
    InvalidPath(String),
}

impl FsEventIngestor {
    /// Coalesce and normalize a batch of raw [`FsEvent`]s into
    /// [`SyncCandidate`]s. Rapid duplicate events for the same path are
    /// collapsed to the most recent kind.
    pub fn normalize_events(&self, events: &[FsEvent]) -> Result<Vec<SyncCandidate>, FsEventError> {
        let mut coalesced = Vec::<FsEvent>::new();
        for event in events {
            validate_relative_path(&event.path)?;
            if let Some(existing) = coalesced
                .iter_mut()
                .find(|candidate| candidate.path == event.path)
            {
                existing.kind = event.kind;
                existing.entry_kind = event.entry_kind;
                existing.sync_id = event.sync_id;
            } else {
                coalesced.push(event.clone());
            }
        }

        Ok(coalesced
            .into_iter()
            .map(|event| SyncCandidate {
                sync_id: event.sync_id,
                source: ChangeSource::Local,
                path: event.path,
                entry_kind: event.entry_kind,
                change_kind: match event.kind {
                    FsEventKind::Remove => ChangeKind::Delete,
                    FsEventKind::Write | FsEventKind::Create => ChangeKind::Upsert,
                },
                remote_file_id: None,
                remote_folder_id: None,
            })
            .collect())
    }
}

fn validate_relative_path(path: &str) -> Result<(), FsEventError> {
    let trimmed = path.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.starts_with("./")
        || trimmed.contains('\\')
        || trimmed
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(FsEventError::InvalidPath(path.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use pcloud_model::{
        ids::SyncId,
        sync::{ChangeKind, EntryKind},
    };

    use super::{FsEvent, FsEventError, FsEventIngestor, FsEventKind};

    #[test]
    fn normalizes_create_and_remove_events() {
        let ingestor = FsEventIngestor::default();
        let candidates = ingestor
            .normalize_events(&[
                FsEvent {
                    sync_id: SyncId::new(1),
                    path: "docs/new.txt".to_owned(),
                    entry_kind: EntryKind::File,
                    kind: FsEventKind::Create,
                },
                FsEvent {
                    sync_id: SyncId::new(1),
                    path: "docs/old.txt".to_owned(),
                    entry_kind: EntryKind::File,
                    kind: FsEventKind::Remove,
                },
            ])
            .expect("events should normalize");

        assert_eq!(candidates[0].change_kind, ChangeKind::Upsert);
        assert_eq!(candidates[1].change_kind, ChangeKind::Delete);
    }

    #[test]
    fn coalesces_multiple_events_for_same_path() {
        let ingestor = FsEventIngestor::default();
        let candidates = ingestor
            .normalize_events(&[
                FsEvent {
                    sync_id: SyncId::new(1),
                    path: "docs/new.txt".to_owned(),
                    entry_kind: EntryKind::File,
                    kind: FsEventKind::Create,
                },
                FsEvent {
                    sync_id: SyncId::new(1),
                    path: "docs/new.txt".to_owned(),
                    entry_kind: EntryKind::File,
                    kind: FsEventKind::Write,
                },
            ])
            .expect("events should normalize");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].change_kind, ChangeKind::Upsert);
    }

    #[test]
    fn rejects_invalid_event_paths() {
        let ingestor = FsEventIngestor::default();
        let error = ingestor
            .normalize_events(&[FsEvent {
                sync_id: SyncId::new(1),
                path: "../escape".to_owned(),
                entry_kind: EntryKind::File,
                kind: FsEventKind::Write,
            }])
            .expect_err("invalid path should be rejected");

        assert_eq!(error, FsEventError::InvalidPath("../escape".to_owned()));
    }
}
