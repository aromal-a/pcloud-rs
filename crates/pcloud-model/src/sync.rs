// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use crate::{
    conflict::ConflictKind,
    ids::{RemoteFileId, RemoteFolderId, SyncId},
};

/// Lifecycle state of a single sync root.
///
/// The engine-level state machine transitions through these as the
/// daemon reconciles local and remote state. Callers render this in
/// dashboards; they should not assume any particular fairness or
/// ordering of transitions.
///
/// # Serde invariant
///
/// Externally tagged by variant name; `serde_json::to_string` followed
/// by `serde_json::from_str` roundtrips every variant losslessly.
///
/// # Example
///
/// ```
/// use pcloud_model::sync::SyncState;
/// let s = SyncState::Steady;
/// let j = serde_json::to_string(&s).unwrap();
/// let back: SyncState = serde_json::from_str(&j).unwrap();
/// assert_eq!(s, back);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncState {
    /// Engine is setting up internal state for the sync root.
    Initializing,
    /// Initial full reconcile is in progress after a fresh start or a
    /// long offline period.
    CatchingUp,
    /// Incremental reconcile is converged; the engine only reacts to
    /// new events.
    Steady,
    /// Sync root is paused by the operator; no operations are issued.
    Paused,
    /// A soft failure occurred (network, rate limit, store pressure)
    /// but the engine is still making forward progress.
    Degraded,
    /// The engine is performing crash/integrity recovery before it can
    /// return to normal operation.
    Recovering,
}

/// Origin of a [`SyncCandidate`] — which side observed the change.
///
/// The planner uses this together with [`ChangeKind`] to pick a
/// [`PlannedOperation`]; when both local and remote events are seen
/// for the same path, the planner emits a
/// [`PlannedOperation::Conflict`].
///
/// # Ordering invariant
///
/// The `PartialOrd`/`Ord` instance is stable (`Local < Remote`) and is
/// relied upon by the planner's sort step (see
/// [`crate::sync`] module docs in `pcloud-engine`).
///
/// # Serde invariant
///
/// Roundtrips losslessly through `serde_json`; the on-wire tag is the
/// literal variant name (`"Local"` / `"Remote"`).
///
/// # Example
///
/// ```
/// use pcloud_model::sync::ChangeSource;
/// // The Ord instance is stable: Local sorts before Remote.
/// assert!(ChangeSource::Local < ChangeSource::Remote);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ChangeSource {
    /// Change was observed locally (filesystem scan or fs-event).
    Local,
    /// Change was observed remotely (diff-poll batch).
    Remote,
}

/// File-vs-folder discriminator for an entry under reconciliation.
///
/// Symlinks, sockets, FIFOs, and device nodes are filtered out
/// upstream by the local scanner; only these two kinds ever reach the
/// planner.
///
/// # Serde invariant
///
/// Roundtrips losslessly through `serde_json` using the variant name
/// as tag.
///
/// # Example
///
/// ```
/// use pcloud_model::sync::EntryKind;
/// let k = EntryKind::File;
/// let j = serde_json::to_string(&k).unwrap();
/// assert!(j.contains("File"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Folder,
}

/// Coarse classification of a change event.
///
/// "Upsert" conflates create and modify intentionally: the server-side
/// and kernel-side event streams are not reliable enough to distinguish
/// them at this layer, and the planner does not need to.
///
/// # Serde invariant
///
/// Roundtrips losslessly through `serde_json` using the variant name
/// as tag.
///
/// # Example
///
/// ```
/// use pcloud_model::sync::ChangeKind;
/// assert!(ChangeKind::Upsert < ChangeKind::Delete || ChangeKind::Delete < ChangeKind::Upsert);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ChangeKind {
    /// Entry was created or modified.
    Upsert,
    /// Entry was deleted.
    Delete,
}

/// Direction of data flow configured for a sync root.
///
/// Mirrors the three C `psync_synctype_t` values declared in `psynclib.h`:
/// `PSYNC_DOWNLOAD_ONLY` (1), `PSYNC_UPLOAD_ONLY` (2), and `PSYNC_FULL`
/// (3), and extends them with `BackupArchive` (4) — a Rust-only
/// deletion-safe archival flavor that uploads new/changed local files
/// but never deletes the remote copy when a local file is removed. The
/// numeric encoding is part of the on-disk schema so
/// [`Self::as_u8`] / [`Self::from_u8`] MUST remain stable.
///
/// # Serde invariant
///
/// Roundtrips losslessly through `serde_json` using the variant name
/// as tag; the `u8` encoding is a separate schema concern exposed
/// through [`Self::as_u8`]/[`Self::from_u8`] and is **not** the serde
/// shape.
///
/// # Example
///
/// ```
/// use pcloud_model::sync::SyncType;
/// // The default is bidirectional sync.
/// assert_eq!(SyncType::default(), SyncType::Full);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum SyncType {
    /// Remote-to-local only; local changes are never uploaded.
    DownloadOnly,
    /// Local-to-remote only; remote changes are never downloaded. A
    /// local deletion propagates to the remote (destructive mirror).
    UploadOnly,
    /// Bidirectional sync. Default for new sync roots.
    #[default]
    Full,
    /// Deletion-safe local-to-remote archival flavor. Uploads new and
    /// changed local files like [`Self::UploadOnly`], but **never**
    /// deletes the remote copy when a local file is removed. Remote
    /// changes are still not downloaded. Rust-only — no corresponding
    /// `psync_synctype_t` value in the legacy C client. Tracked under
    /// `bd-1du.5`.
    BackupArchive,
}

impl SyncType {
    /// Encode as a stable numeric value. Values 1–3 mirror the legacy C
    /// `psync_synctype_t` encoding (`PSYNC_DOWNLOAD_ONLY`,
    /// `PSYNC_UPLOAD_ONLY`, `PSYNC_FULL`); value 4 is the Rust-only
    /// [`Self::BackupArchive`] flavor.
    ///
    /// This encoding is part of the persisted schema and MUST NOT
    /// change without a store migration.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::sync::SyncType;
    /// assert_eq!(SyncType::DownloadOnly.as_u8(), 1);
    /// assert_eq!(SyncType::UploadOnly.as_u8(), 2);
    /// assert_eq!(SyncType::Full.as_u8(), 3);
    /// assert_eq!(SyncType::BackupArchive.as_u8(), 4);
    /// ```
    #[must_use]
    pub fn as_u8(self) -> u8 {
        match self {
            Self::DownloadOnly => 1,
            Self::UploadOnly => 2,
            Self::Full => 3,
            Self::BackupArchive => 4,
        }
    }

    /// Decode from the stable numeric value.
    /// Returns `None` for any value outside `1..=4`.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::sync::SyncType;
    /// assert_eq!(SyncType::from_u8(3), Some(SyncType::Full));
    /// assert_eq!(SyncType::from_u8(4), Some(SyncType::BackupArchive));
    /// assert_eq!(SyncType::from_u8(0), None);
    /// assert_eq!(SyncType::from_u8(99), None);
    /// ```
    #[must_use]
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::DownloadOnly),
            2 => Some(Self::UploadOnly),
            3 => Some(Self::Full),
            4 => Some(Self::BackupArchive),
            _ => None,
        }
    }

    /// Short kebab-case label suitable for log lines and CLI output.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::sync::SyncType;
    /// assert_eq!(SyncType::default().label(), "full");
    /// assert_eq!(SyncType::DownloadOnly.label(), "download-only");
    /// assert_eq!(SyncType::BackupArchive.label(), "backup-archive");
    /// ```
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::DownloadOnly => "download-only",
            Self::UploadOnly => "upload-only",
            Self::Full => "full",
            Self::BackupArchive => "backup-archive",
        }
    }
}

/// A candidate change observed on one side of a sync pair.
///
/// The local scanner, diff poller, and fs-event ingestor all produce
/// `SyncCandidate` values; the planner consumes them and pairs
/// candidates at the same `path` across sources to produce a stream of
/// [`PlannedOperation`]s.
///
/// # Serde invariant
///
/// All fields are directly serializable; a `SyncCandidate` roundtrips
/// losslessly through `serde_json` (see the crate test
/// `sync_candidate_serde_roundtrip`).
///
/// # Example
///
/// ```
/// use pcloud_model::ids::SyncId;
/// use pcloud_model::sync::{ChangeKind, ChangeSource, EntryKind, SyncCandidate};
///
/// let c = SyncCandidate {
///     sync_id: SyncId::new(1),
///     source: ChangeSource::Local,
///     path: "a/b.txt".into(),
///     entry_kind: EntryKind::File,
///     change_kind: ChangeKind::Upsert,
///     remote_file_id: None,
///     remote_folder_id: None,
/// };
/// assert_eq!(c.path, "a/b.txt");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCandidate {
    /// Owning sync root.
    pub sync_id: SyncId,
    /// Which side observed the change.
    pub source: ChangeSource,
    /// Forward-slash relative path under the sync root. Must not be
    /// empty, absolute, or contain `.` / `..` segments — upstream
    /// ingestors validate this before constructing the candidate.
    pub path: String,
    /// File or folder.
    pub entry_kind: EntryKind,
    /// Upsert or delete.
    pub change_kind: ChangeKind,
    /// Remote file id, if already known (remote candidates or local
    /// candidates that correspond to a previously-synced path).
    pub remote_file_id: Option<RemoteFileId>,
    /// Remote folder id of the parent (or of this entry, when it is a
    /// folder), if known.
    pub remote_folder_id: Option<RemoteFolderId>,
}

/// A single actionable operation produced by the planner.
///
/// Executed by the scheduler/transfer coordinators on the daemon side.
/// Variants carry just enough state for the executor to contact the
/// API or touch the local filesystem without consulting the planner
/// again. [`Self::priority`] defines the execution ordering.
///
/// # Serde invariant
///
/// Externally tagged (variant name as a single-key map for struct
/// variants). Roundtrips losslessly through `serde_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlannedOperation {
    /// Upload a local file to the remote side.
    UploadFile {
        /// Owning sync root.
        sync_id: SyncId,
        /// Relative path under the sync root.
        path: String,
        /// Target parent folder id on the server, if known.
        remote_parent_folder_id: Option<RemoteFolderId>,
        /// File name (basename of `path`) to use on the server.
        remote_name: String,
    },
    /// Download a remote file into the local tree.
    DownloadFile {
        /// Owning sync root.
        sync_id: SyncId,
        /// Relative path under the sync root.
        path: String,
        /// Remote file id to download, if known.
        remote_file_id: Option<RemoteFileId>,
    },
    /// Create a local directory mirroring a remote folder.
    CreateLocalDirectory {
        /// Owning sync root.
        sync_id: SyncId,
        /// Relative path of the directory to create.
        path: String,
        /// Remote folder id this directory corresponds to.
        remote_folder_id: Option<RemoteFolderId>,
    },
    /// Create a remote folder mirroring a local directory.
    CreateRemoteDirectory {
        /// Owning sync root.
        sync_id: SyncId,
        /// Relative path of the directory to create on the server.
        path: String,
    },
    /// Delete a local file or directory.
    DeleteLocal {
        /// Owning sync root.
        sync_id: SyncId,
        /// Relative path to delete locally.
        path: String,
    },
    /// Delete a remote file or folder.
    DeleteRemote {
        /// Owning sync root.
        sync_id: SyncId,
        /// Relative path to delete on the server.
        path: String,
    },
    /// Unresolved conflict — requires the conflict resolver or a human
    /// operator to pick a concrete operation.
    Conflict {
        /// Owning sync root.
        sync_id: SyncId,
        /// Conflicting path.
        path: String,
        /// Classification of the conflict.
        kind: ConflictKind,
    },
}

impl PlannedOperation {
    /// Return the sync-root id this operation belongs to.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    /// let op = PlannedOperation::DeleteLocal {
    ///     sync_id: SyncId::new(7),
    ///     path: "old/file.txt".to_owned(),
    /// };
    /// assert_eq!(op.sync_id(), SyncId::new(7));
    /// ```
    #[must_use]
    pub fn sync_id(&self) -> SyncId {
        match self {
            Self::UploadFile { sync_id, .. }
            | Self::DownloadFile { sync_id, .. }
            | Self::CreateLocalDirectory { sync_id, .. }
            | Self::CreateRemoteDirectory { sync_id, .. }
            | Self::DeleteLocal { sync_id, .. }
            | Self::DeleteRemote { sync_id, .. }
            | Self::Conflict { sync_id, .. } => *sync_id,
        }
    }

    /// Execution priority. **Lower is more urgent.** The scheduler
    /// orders operations by this value so conflicts are surfaced
    /// before any destructive work runs:
    ///
    /// | Variant                                             | Priority |
    /// |-----------------------------------------------------|----------|
    /// | [`Self::Conflict`]                                  | 0        |
    /// | [`Self::DeleteLocal`] / [`Self::DeleteRemote`]      | 1        |
    /// | [`Self::CreateLocalDirectory`] / [`Self::CreateRemoteDirectory`] | 2 |
    /// | [`Self::DownloadFile`] / [`Self::UploadFile`]       | 3        |
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    /// let upload = PlannedOperation::UploadFile {
    ///     sync_id: SyncId::new(1),
    ///     path: "a.txt".into(),
    ///     remote_parent_folder_id: None,
    ///     remote_name: "a.txt".into(),
    /// };
    /// let delete = PlannedOperation::DeleteLocal {
    ///     sync_id: SyncId::new(1),
    ///     path: "b.txt".into(),
    /// };
    /// assert!(delete.priority() < upload.priority());
    /// ```
    #[must_use]
    pub fn priority(&self) -> u8 {
        match self {
            Self::Conflict { .. } => 0,
            Self::DeleteLocal { .. } | Self::DeleteRemote { .. } => 1,
            Self::CreateLocalDirectory { .. } | Self::CreateRemoteDirectory { .. } => 2,
            Self::DownloadFile { .. } | Self::UploadFile { .. } => 3,
        }
    }

    /// Return the path this operation acts on (relative to the sync
    /// root).
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    /// let op = PlannedOperation::DownloadFile {
    ///     sync_id: SyncId::new(1),
    ///     path: "dir/notes.md".into(),
    ///     remote_file_id: None,
    /// };
    /// assert_eq!(op.path(), "dir/notes.md");
    /// ```
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::UploadFile { path, .. }
            | Self::DownloadFile { path, .. }
            | Self::CreateLocalDirectory { path, .. }
            | Self::CreateRemoteDirectory { path, .. }
            | Self::DeleteLocal { path, .. }
            | Self::DeleteRemote { path, .. }
            | Self::Conflict { path, .. } => path.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conflict::ConflictKind;

    #[test]
    fn sync_type_default_is_full() {
        assert_eq!(SyncType::default(), SyncType::Full);
    }

    #[test]
    fn sync_type_u8_roundtrip_all_variants() {
        for v in [
            SyncType::DownloadOnly,
            SyncType::UploadOnly,
            SyncType::Full,
            SyncType::BackupArchive,
        ] {
            assert_eq!(SyncType::from_u8(v.as_u8()), Some(v));
        }
    }

    #[test]
    fn sync_type_from_u8_rejects_invalid() {
        assert_eq!(SyncType::from_u8(0), None);
        assert_eq!(SyncType::from_u8(5), None);
        assert_eq!(SyncType::from_u8(u8::MAX), None);
    }

    #[test]
    fn sync_type_labels() {
        assert_eq!(SyncType::DownloadOnly.label(), "download-only");
        assert_eq!(SyncType::UploadOnly.label(), "upload-only");
        assert_eq!(SyncType::Full.label(), "full");
        assert_eq!(SyncType::BackupArchive.label(), "backup-archive");
    }

    #[test]
    fn planned_operation_priority_ordering() {
        let conflict = PlannedOperation::Conflict {
            sync_id: SyncId::new(1),
            path: "/a".into(),
            kind: ConflictKind::TypeMismatch,
        };
        let del = PlannedOperation::DeleteLocal {
            sync_id: SyncId::new(1),
            path: "/b".into(),
        };
        let mkdir = PlannedOperation::CreateRemoteDirectory {
            sync_id: SyncId::new(1),
            path: "/c".into(),
        };
        let upload = PlannedOperation::UploadFile {
            sync_id: SyncId::new(1),
            path: "/d".into(),
            remote_parent_folder_id: None,
            remote_name: "d".into(),
        };
        assert!(conflict.priority() < del.priority());
        assert!(del.priority() < mkdir.priority());
        assert!(mkdir.priority() < upload.priority());
    }

    #[test]
    fn planned_operation_accessors_match_construction() {
        let op = PlannedOperation::DownloadFile {
            sync_id: SyncId::new(99),
            path: "/foo/bar".into(),
            remote_file_id: None,
        };
        assert_eq!(op.sync_id(), SyncId::new(99));
        assert_eq!(op.path(), "/foo/bar");
    }

    #[test]
    fn planned_operation_empty_path_boundary() {
        let op = PlannedOperation::DeleteRemote {
            sync_id: SyncId::new(0),
            path: String::new(),
        };
        assert_eq!(op.path(), "");
    }

    #[test]
    fn sync_candidate_serde_roundtrip() {
        let c = SyncCandidate {
            sync_id: SyncId::new(5),
            source: ChangeSource::Remote,
            path: "/x".into(),
            entry_kind: EntryKind::File,
            change_kind: ChangeKind::Upsert,
            remote_file_id: None,
            remote_folder_id: None,
        };
        let j = serde_json::to_string(&c).unwrap();
        let back: SyncCandidate = serde_json::from_str(&j).unwrap();
        assert_eq!(c, back);
    }
}
