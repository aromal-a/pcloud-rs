// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use pcloud_model::{
    ids::{RemoteFileId, RemoteFolderId, SyncId},
    sync::{ChangeKind, ChangeSource, EntryKind, SyncCandidate},
};

/// Configuration state for the remote diff poller. Holds the maximum
/// number of entries to request per poll cycle and provides batch
/// normalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffPoller {
    /// Maximum entries to request per poll cycle.
    pub batch_limit: u64,
}

impl Default for DiffPoller {
    fn default() -> Self {
        Self { batch_limit: 512 }
    }
}

/// A single batch of remote diff entries as returned by the pCloud diff
/// endpoint for a particular sync root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDiffBatch {
    /// Sync root this batch applies to.
    pub sync_id: SyncId,
    /// Next cursor to use for follow-up diff polls.
    pub cursor: u64,
    /// Whether the server has more entries beyond this batch.
    pub has_more: bool,
    /// Entries contained in this batch.
    pub entries: Vec<RemoteDiffEntry>,
}

/// One remote diff entry — a file or folder upsert/delete — in a batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDiffEntry {
    /// Sync-root-relative path for the entry.
    pub path: String,
    /// Whether the entry is a file or a folder.
    pub entry_kind: EntryKind,
    /// Upsert vs delete semantics for this entry.
    pub change_kind: ChangeKind,
    /// Remote file id, if the entry references a file.
    pub remote_file_id: Option<RemoteFileId>,
    /// Remote folder id, if the entry references a folder.
    pub remote_folder_id: Option<RemoteFolderId>,
    /// Numeric event tag from the binary protocol (`event` field in
    /// each diff entry). `None` for batches that do not carry per-entry
    /// event tags (initial-sync `diff` response without `event`). See
    /// [`crate::diff_events::DiffEventKind::from_event_id`].
    #[serde(default)]
    pub event: Option<u64>,
}

/// Error returned when a [`RemoteDiffEntry`] cannot be converted into a
/// [`SyncCandidate`] due to a malformed path or other protocol violation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffNormalizationError {
    /// The entry's path is absolute, empty, contains `..`, or is
    /// otherwise unsafe to interpret as a sync-root-relative path.
    InvalidPath(String),
}

impl DiffPoller {
    /// Convert a remote diff batch into a vector of
    /// [`SyncCandidate`]s for the planner. Validates each entry's path.
    pub fn normalize_batch(
        &self,
        batch: &RemoteDiffBatch,
    ) -> Result<Vec<SyncCandidate>, DiffNormalizationError> {
        batch
            .entries
            .iter()
            .map(|entry| normalize_entry(batch.sync_id, entry))
            .collect()
    }
}

fn normalize_entry(
    sync_id: SyncId,
    entry: &RemoteDiffEntry,
) -> Result<SyncCandidate, DiffNormalizationError> {
    validate_relative_path(&entry.path)?;
    Ok(SyncCandidate {
        sync_id,
        source: ChangeSource::Remote,
        path: entry.path.clone(),
        entry_kind: entry.entry_kind,
        change_kind: entry.change_kind,
        remote_file_id: entry.remote_file_id,
        remote_folder_id: entry.remote_folder_id,
    })
}

fn validate_relative_path(path: &str) -> Result<(), DiffNormalizationError> {
    let trimmed = path.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.starts_with("./")
        || trimmed.contains('\\')
        || trimmed
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(DiffNormalizationError::InvalidPath(path.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use pcloud_model::{
        ids::{RemoteFileId, RemoteFolderId, SyncId},
        sync::{ChangeKind, ChangeSource, EntryKind, SyncCandidate},
    };

    use super::{DiffNormalizationError, DiffPoller, RemoteDiffBatch, RemoteDiffEntry};

    #[test]
    fn normalizes_remote_file_upsert_into_sync_candidate() {
        let poller = DiffPoller::default();
        let candidates = poller
            .normalize_batch(&RemoteDiffBatch {
                sync_id: SyncId::new(1),
                cursor: 9,
                has_more: false,
                entries: vec![RemoteDiffEntry {
                    path: "docs/report.txt".to_owned(),
                    entry_kind: EntryKind::File,
                    change_kind: ChangeKind::Upsert,
                    remote_file_id: Some(RemoteFileId::new(44)),
                    remote_folder_id: None,
                    event: None,
                }],
            })
            .expect("batch should normalize");

        assert_eq!(
            candidates,
            vec![SyncCandidate {
                sync_id: SyncId::new(1),
                source: ChangeSource::Remote,
                path: "docs/report.txt".to_owned(),
                entry_kind: EntryKind::File,
                change_kind: ChangeKind::Upsert,
                remote_file_id: Some(RemoteFileId::new(44)),
                remote_folder_id: None,
            }]
        );
    }

    #[test]
    fn normalizes_remote_folder_upsert_and_delete_entries() {
        let poller = DiffPoller::default();
        let candidates = poller
            .normalize_batch(&RemoteDiffBatch {
                sync_id: SyncId::new(7),
                cursor: 11,
                has_more: true,
                entries: vec![
                    RemoteDiffEntry {
                        path: "docs".to_owned(),
                        entry_kind: EntryKind::Folder,
                        change_kind: ChangeKind::Upsert,
                        remote_file_id: None,
                        remote_folder_id: Some(RemoteFolderId::new(5)),
                        event: None,
                    },
                    RemoteDiffEntry {
                        path: "docs/old.txt".to_owned(),
                        entry_kind: EntryKind::File,
                        change_kind: ChangeKind::Delete,
                        remote_file_id: Some(RemoteFileId::new(8)),
                        remote_folder_id: None,
                        event: None,
                    },
                ],
            })
            .expect("batch should normalize");

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].entry_kind, EntryKind::Folder);
        assert_eq!(candidates[1].change_kind, ChangeKind::Delete);
    }

    #[test]
    fn rejects_malformed_remote_paths() {
        let poller = DiffPoller::default();
        let error = poller
            .normalize_batch(&RemoteDiffBatch {
                sync_id: SyncId::new(3),
                cursor: 0,
                has_more: false,
                entries: vec![RemoteDiffEntry {
                    path: "../etc/passwd".to_owned(),
                    entry_kind: EntryKind::File,
                    change_kind: ChangeKind::Upsert,
                    remote_file_id: Some(RemoteFileId::new(1)),
                    remote_folder_id: None,
                    event: None,
                }],
            })
            .expect_err("invalid path should be rejected");

        assert_eq!(
            error,
            DiffNormalizationError::InvalidPath("../etc/passwd".to_owned())
        );
    }
}
