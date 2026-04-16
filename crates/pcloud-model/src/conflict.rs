// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use crate::sync::PlannedOperation;

/// Taxonomy of sync conflicts surfaced by the engine planner.
///
/// Each variant captures a specific combination of local and remote
/// state that cannot be reconciled by a single unambiguous
/// [`PlannedOperation`]. Callers (CLI, SDK, conflict resolver) branch
/// on this to pick a policy or to defer to a human operator.
///
/// # Example
///
/// ```
/// use pcloud_model::conflict::ConflictKind;
/// let k = ConflictKind::TypeMismatch;
/// let j = serde_json::to_string(&k).unwrap();
/// let back: ConflictKind = serde_json::from_str(&j).unwrap();
/// assert_eq!(k, back);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictKind {
    /// Same path modified on both sides since the last reconcile.
    LocalModifyVsRemoteModify,
    /// Deleted locally while the server has a newer modification.
    LocalDeleteVsRemoteModify,
    /// Deleted remotely while the local file was modified.
    RemoteDeleteVsLocalModify,
    /// Same path is a file on one side and a folder on the other.
    TypeMismatch,
    /// Two different source paths collided on the same destination
    /// after a rename on one side.
    RenameCollision,
    /// A parent folder on the path prefix is itself in conflict,
    /// blocking progress until the parent is resolved.
    ParentPathConflict,
    /// A resumed transfer's checksum does not match the server state;
    /// partial upload/download cannot be trusted.
    ResumeChecksumMismatch,
    /// The engine cannot tell whether the remote copy is encrypted at
    /// the moment (crypto locked/expired) and refuses to write blindly.
    CryptoAvailabilityConflict,
}

/// Outcome of passing a [`PlannedOperation::Conflict`] through a
/// resolver policy.
///
/// [`PlannedOperation::Conflict`]: crate::sync::PlannedOperation::Conflict
///
/// # Example
///
/// ```
/// use pcloud_model::conflict::{ConflictKind, ConflictResolution};
/// let r = ConflictResolution::ManualReview {
///     path: "doc.txt".into(),
///     kind: ConflictKind::TypeMismatch,
///     reason: "file-vs-folder".into(),
/// };
/// match r {
///     ConflictResolution::ManualReview { path, .. } => assert_eq!(path, "doc.txt"),
///     _ => unreachable!(),
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictResolution {
    /// Policy produced a concrete operation that should be executed to
    /// resolve the conflict (e.g. upload the local copy).
    Apply(PlannedOperation),
    /// Policy declined to auto-resolve; the conflict requires human
    /// intervention. Includes the path, kind, and a human-readable
    /// reason suitable for surfacing in logs and UIs.
    ManualReview {
        /// Conflicting path (relative to the sync root).
        path: String,
        /// The specific kind of conflict encountered.
        kind: ConflictKind,
        /// Human-readable explanation (not a stable API — do not parse).
        reason: String,
    },
    /// Both local and remote copies are preserved under conflict-renamed
    /// paths. The sync engine must rename the local file to
    /// `local_renamed_path` and download the remote version to
    /// `remote_renamed_path`. The original path is freed for the next
    /// sync cycle to decide ownership.
    RenameBoth {
        /// Path the local file will be renamed to (e.g.
        /// `docs/report.conflict-local.txt`).
        local_renamed_path: String,
        /// Path the remote file will be downloaded to (e.g.
        /// `docs/report.conflict-remote.txt`).
        remote_renamed_path: String,
        /// Original conflicting path (relative to the sync root).
        original_path: String,
        /// Sync root that owns the conflicting file.
        sync_id: crate::ids::SyncId,
    },
}
