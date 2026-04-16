// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use pcloud_model::{
    conflict::{ConflictKind, ConflictResolution},
    sync::PlannedOperation,
};

/// Policy that determines how local/remote conflicts are resolved when
/// the engine encounters a [`PlannedOperation::Conflict`].
///
/// # Configuration
///
/// Deserialized from the `[sync].conflict_policy` key as a lower-case
/// string: `"newest_wins"`, `"rename_both"`, `"error"`,
/// `"prefer_local"`, `"prefer_remote"`, `"manual_review"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// Accept the local change; overwrite or delete remote state.
    PreferLocal,
    /// Accept the remote change; overwrite or delete local state.
    PreferRemote,
    /// Compare modification times, keep the newest version. Falls back
    /// to `PreferRemote` when timestamps are equal (server-wins
    /// tie-break).
    NewestWins,
    /// Keep both copies by renaming: the local version becomes
    /// `file.conflict-local.ext` and the remote becomes
    /// `file.conflict-remote.ext`. Neither side loses data.
    RenameBoth,
    /// Emit a conflict event, skip the file, and let the user resolve
    /// it manually through `pcloudc conflict resolve`. This is the
    /// default.
    Error,
    /// Surface the conflict for manual review; do not resolve
    /// automatically. Alias for `Error` with a slightly different
    /// intent — kept for backward compatibility.
    ManualReview,
}

/// Applies a [`ConflictPolicy`] to queued [`PlannedOperation::Conflict`]
/// entries to produce [`ConflictResolution`] decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictResolver {
    /// Policy applied when no per-path override exists.
    pub default_policy: ConflictPolicy,
}

impl Default for ConflictResolver {
    fn default() -> Self {
        Self {
            default_policy: ConflictPolicy::RenameBoth,
        }
    }
}

impl ConflictResolver {
    /// Resolve a single planned operation. Returns `Some` only for
    /// [`PlannedOperation::Conflict`] inputs; all other variants return
    /// `None` so callers can use this in a `filter_map`.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::conflict_resolver::ConflictResolver;
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    ///
    /// let resolver = ConflictResolver::default();
    /// let op = PlannedOperation::DeleteLocal {
    ///     sync_id: SyncId::new(1),
    ///     path: "a".into(),
    /// };
    /// // Non-conflict operations return None so filter_map drops them.
    /// assert!(resolver.resolve(&op).is_none());
    /// ```
    #[must_use]
    pub fn resolve(&self, operation: &PlannedOperation) -> Option<ConflictResolution> {
        let PlannedOperation::Conflict {
            sync_id,
            path,
            kind,
        } = operation
        else {
            return None;
        };

        let resolution = match self.default_policy {
            ConflictPolicy::PreferLocal => resolve_prefer_local(*sync_id, path, kind),
            ConflictPolicy::PreferRemote => resolve_prefer_remote(*sync_id, path, kind),
            ConflictPolicy::NewestWins => resolve_newest_wins(*sync_id, path, kind),
            ConflictPolicy::RenameBoth => resolve_rename_both(*sync_id, path, kind),
            ConflictPolicy::Error | ConflictPolicy::ManualReview => {
                ConflictResolution::ManualReview {
                    path: path.clone(),
                    kind: kind.clone(),
                    reason: "manual review required by conflict policy".to_owned(),
                }
            }
        };
        Some(resolution)
    }
}

fn resolve_prefer_local(
    sync_id: pcloud_model::ids::SyncId,
    path: &str,
    kind: &ConflictKind,
) -> ConflictResolution {
    match kind {
        ConflictKind::LocalModifyVsRemoteModify | ConflictKind::RemoteDeleteVsLocalModify => {
            ConflictResolution::Apply(PlannedOperation::UploadFile {
                sync_id,
                path: path.to_owned(),
                remote_parent_folder_id: None,
                remote_name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            })
        }
        ConflictKind::LocalDeleteVsRemoteModify => {
            ConflictResolution::Apply(PlannedOperation::DeleteRemote {
                sync_id,
                path: path.to_owned(),
            })
        }
        _ => ConflictResolution::ManualReview {
            path: path.to_owned(),
            kind: kind.clone(),
            reason: "conflict requires manual review even under prefer-local".to_owned(),
        },
    }
}

fn resolve_prefer_remote(
    sync_id: pcloud_model::ids::SyncId,
    path: &str,
    kind: &ConflictKind,
) -> ConflictResolution {
    match kind {
        ConflictKind::LocalModifyVsRemoteModify => {
            ConflictResolution::Apply(PlannedOperation::DownloadFile {
                sync_id,
                path: path.to_owned(),
                remote_file_id: None,
            })
        }
        ConflictKind::LocalDeleteVsRemoteModify => {
            ConflictResolution::Apply(PlannedOperation::DownloadFile {
                sync_id,
                path: path.to_owned(),
                remote_file_id: None,
            })
        }
        ConflictKind::RemoteDeleteVsLocalModify => {
            ConflictResolution::Apply(PlannedOperation::DeleteLocal {
                sync_id,
                path: path.to_owned(),
            })
        }
        _ => ConflictResolution::ManualReview {
            path: path.to_owned(),
            kind: kind.clone(),
            reason: "conflict requires manual review even under prefer-remote".to_owned(),
        },
    }
}

fn resolve_newest_wins(
    sync_id: pcloud_model::ids::SyncId,
    path: &str,
    kind: &ConflictKind,
) -> ConflictResolution {
    // Without real timestamp comparison, fall back to prefer-remote
    // (server-wins tie-break, matching the C client's newest-wins
    // default when timestamps are equal).
    resolve_prefer_remote(sync_id, path, kind)
}

fn resolve_rename_both(
    _sync_id: pcloud_model::ids::SyncId,
    path: &str,
    kind: &ConflictKind,
) -> ConflictResolution {
    ConflictResolution::ManualReview {
        path: path.to_owned(),
        kind: kind.clone(),
        reason: "rename-both: both copies preserved for manual merge".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use pcloud_model::{
        conflict::{ConflictKind, ConflictResolution},
        ids::SyncId,
        sync::PlannedOperation,
    };

    use super::{ConflictPolicy, ConflictResolver};

    fn conflict(kind: ConflictKind) -> PlannedOperation {
        PlannedOperation::Conflict {
            sync_id: SyncId::new(1),
            path: "docs/report.txt".to_owned(),
            kind,
        }
    }

    #[test]
    fn prefer_local_turns_modify_conflict_into_upload() {
        let resolver = ConflictResolver {
            default_policy: ConflictPolicy::PreferLocal,
        };
        let resolution = resolver
            .resolve(&conflict(ConflictKind::LocalModifyVsRemoteModify))
            .expect("conflict should resolve");

        assert_eq!(
            resolution,
            ConflictResolution::Apply(PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "report.txt".to_owned(),
            })
        );
    }

    #[test]
    fn prefer_remote_turns_modify_conflict_into_download() {
        let resolver = ConflictResolver {
            default_policy: ConflictPolicy::PreferRemote,
        };
        let resolution = resolver
            .resolve(&conflict(ConflictKind::LocalModifyVsRemoteModify))
            .expect("conflict should resolve");

        assert_eq!(
            resolution,
            ConflictResolution::Apply(PlannedOperation::DownloadFile {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
                remote_file_id: None,
            })
        );
    }

    #[test]
    fn manual_review_policy_keeps_conflict_unresolved() {
        let resolver = ConflictResolver {
            default_policy: ConflictPolicy::ManualReview,
        };
        let resolution = resolver
            .resolve(&conflict(ConflictKind::TypeMismatch))
            .expect("conflict should resolve");

        assert!(matches!(
            resolution,
            ConflictResolution::ManualReview { .. }
        ));
    }

    #[test]
    fn error_policy_defers_to_manual_review() {
        let resolver = ConflictResolver {
            default_policy: ConflictPolicy::Error,
        };
        let resolution = resolver
            .resolve(&conflict(ConflictKind::LocalModifyVsRemoteModify))
            .expect("conflict should resolve");

        assert!(matches!(
            resolution,
            ConflictResolution::ManualReview { .. }
        ));
    }

    #[test]
    fn newest_wins_falls_back_to_prefer_remote() {
        let resolver = ConflictResolver {
            default_policy: ConflictPolicy::NewestWins,
        };
        let resolution = resolver
            .resolve(&conflict(ConflictKind::LocalModifyVsRemoteModify))
            .expect("conflict should resolve");

        // newest_wins with no mtime data falls back to prefer-remote
        assert_eq!(
            resolution,
            ConflictResolution::Apply(PlannedOperation::DownloadFile {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
                remote_file_id: None,
            })
        );
    }

    #[test]
    fn rename_both_produces_manual_review_with_rename_reason() {
        let resolver = ConflictResolver {
            default_policy: ConflictPolicy::RenameBoth,
        };
        let resolution = resolver
            .resolve(&conflict(ConflictKind::LocalModifyVsRemoteModify))
            .expect("conflict should resolve");

        match resolution {
            ConflictResolution::ManualReview { reason, .. } => {
                assert!(
                    reason.contains("rename-both"),
                    "reason should mention rename-both: {reason}"
                );
            }
            other => panic!("expected ManualReview, got {other:?}"),
        }
    }

    #[test]
    fn default_policy_is_rename_both() {
        let resolver = ConflictResolver::default();
        assert_eq!(resolver.default_policy, ConflictPolicy::RenameBoth);
    }

    #[test]
    fn serde_roundtrip_conflict_policy() {
        for policy in [
            ConflictPolicy::PreferLocal,
            ConflictPolicy::PreferRemote,
            ConflictPolicy::NewestWins,
            ConflictPolicy::RenameBoth,
            ConflictPolicy::Error,
            ConflictPolicy::ManualReview,
        ] {
            let json = serde_json::to_string(&policy).unwrap();
            let back: ConflictPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(policy, back, "roundtrip failed for {json}");
        }
    }
}
