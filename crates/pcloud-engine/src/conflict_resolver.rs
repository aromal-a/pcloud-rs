// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::Path;

use serde::{Deserialize, Serialize};

use pcloud_model::{
    conflict::{ConflictKind, ConflictResolution},
    ids::SyncId,
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
    /// Compare local modification times against the remote `modified`
    /// timestamp carried in [`ConflictKind`]. Keeps the newest version.
    /// Falls back to `PreferRemote` when timestamps are equal or when
    /// the local `mtime` cannot be read (server-wins tie-break).
    NewestWins,
    /// Preserve both copies by renaming: local file → `<stem>.conflict-local.<ext>`;
    /// remote file → `<stem>.conflict-remote.<ext>`. Both renames are
    /// represented as a [`ConflictResolution::RenameBoth`] so the sync
    /// loop can execute the two-step operation atomically.
    RenameBoth,
    /// Emit a conflict event, skip the file, and let the user resolve
    /// it manually through `pcloudc conflict resolve`.
    ///
    /// **Not** the default policy. The default is [`ConflictPolicy::RenameBoth`]
    /// (see [`ConflictResolver::default`]). Select `Error` explicitly via
    /// `--on-conflict=error` or the daemon config key `sync.conflict_policy`.
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
    /// Default policy is [`ConflictPolicy::RenameBoth`] (audit-06
    /// §4-sonnet M-04-S03 / ncx.44).
    ///
    /// # Why `RenameBoth` is the safe default
    ///
    /// A conflicting modify-vs-modify requires picking one side's
    /// bytes to keep as authoritative. `PreferLocal` loses remote
    /// collaborator edits; `PreferRemote` loses local work in
    /// progress; `NewestWins` silently resolves via wall-clock time,
    /// which is notoriously unreliable across machines with skewed
    /// clocks or offline edits. All three have scenarios where they
    /// destroy user data with no audit trail.
    ///
    /// `RenameBoth` preserves *both* copies on disk with stable,
    /// human-readable suffixes (`.conflict-local.<ext>` /
    /// `.conflict-remote.<ext>`) and lets the operator — or a
    /// downstream automation — inspect both versions before deciding
    /// which to keep. No bytes are lost; the user is forced to
    /// acknowledge the conflict; automated jobs can trigger
    /// `pcloudc conflict resolve` on the presence of the suffix.
    ///
    /// This matches the spirit of the pCloud C client's default
    /// conflict-handling policy which preserves an explicit conflict
    /// copy rather than silently overwriting. An operator who
    /// explicitly prefers a destructive policy can opt in via the
    /// `[sync].conflict_policy` config key or `--on-conflict=...`.
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
    /// assert!(resolver.resolve(&op, None, None).is_none());
    /// ```
    ///
    /// # `local_mtime_secs` and `remote_mtime_secs`
    ///
    /// Unix timestamps (seconds since epoch) used by [`ConflictPolicy::NewestWins`]
    /// to compare the local and remote versions. Pass `None` when the
    /// timestamp is unknown; the resolver falls back to prefer-remote
    /// (server-wins tie-break) in that case.
    ///
    /// For all other policies these arguments are ignored.
    #[must_use]
    pub fn resolve(
        &self,
        operation: &PlannedOperation,
        local_mtime_secs: Option<u64>,
        remote_mtime_secs: Option<u64>,
    ) -> Option<ConflictResolution> {
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
            ConflictPolicy::NewestWins => {
                resolve_newest_wins(*sync_id, path, kind, local_mtime_secs, remote_mtime_secs)
            }
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

    /// P2-a (H1): resolve a conflict operation while reading the local
    /// mtime from an **absolute** path rooted at `sync_root`. The previous
    /// `resolve_newest_wins` best-effort fallback called
    /// `std::fs::metadata(path)` with a sync-root-relative string, which
    /// fails in the daemon's cwd and silently fell through to prefer-remote.
    ///
    /// This variant builds `sync_root.join(operation.path())` and passes
    /// the mtime it reads into [`Self::resolve`]. When the read fails the
    /// supplied `local_mtime_secs_override` (if any) is used; otherwise
    /// the resolver falls back to prefer-remote as documented.
    #[must_use]
    pub fn resolve_with_sync_root(
        &self,
        operation: &PlannedOperation,
        sync_root: &Path,
        remote_mtime_secs: Option<u64>,
        local_mtime_secs_override: Option<u64>,
    ) -> Option<ConflictResolution> {
        let PlannedOperation::Conflict { path, .. } = operation else {
            return None;
        };
        let absolute = sync_root.join(path);
        let local_mtime = local_mtime_secs_override.or_else(|| {
            std::fs::metadata(&absolute)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|sys| {
                    sys.duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs())
                })
        });
        self.resolve(operation, local_mtime, remote_mtime_secs)
    }
}

fn resolve_prefer_local(sync_id: SyncId, path: &str, kind: &ConflictKind) -> ConflictResolution {
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

fn resolve_prefer_remote(sync_id: SyncId, path: &str, kind: &ConflictKind) -> ConflictResolution {
    // TODO(bd-1du): `ConflictKind` does not yet carry a `remote_file_id`
    // payload. When it does, thread the id through to `DownloadFile` to
    // avoid a redundant server lookup at resolve time.  Tracked separately
    // from the planner / scheduler work because it requires a model change.
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

/// Resolve a modify/delete conflict by picking the newer side's
/// bytes.
///
/// # Tie-break rule
///
/// When the local and remote Unix-timestamp modification times are
/// **equal** (or only one side is readable) the resolver falls back
/// to **prefer-remote** ("server wins"). This matches the C client's
/// server-wins default and is documented explicitly because a silent
/// tie-break can surprise operators whose local edits appear to
/// evaporate.
///
/// Every tie-break fires an `info!` log line carrying the sync id
/// and the path so operators can correlate lost-work reports with
/// concrete conflict events (audit-06 §4-opus M-4.3 / ncx.41). The
/// log message contains neither file contents nor any secret.
///
/// If deterministic local-wins-on-tie is required, select
/// [`ConflictPolicy::PreferLocal`] explicitly instead.
fn resolve_newest_wins(
    sync_id: SyncId,
    path: &str,
    kind: &ConflictKind,
    local_mtime_secs: Option<u64>,
    remote_mtime_secs: Option<u64>,
) -> ConflictResolution {
    // When both timestamps are provided, do a direct comparison.
    // Local strictly greater → prefer local; otherwise → prefer remote
    // (tie-break and remote-newer both go to prefer-remote, matching the
    // C client's server-wins default when timestamps are equal).
    if let (Some(local), Some(remote)) = (local_mtime_secs, remote_mtime_secs) {
        if local > remote {
            return resolve_prefer_local(sync_id, path, kind);
        } else {
            if local == remote {
                // Tie-break: equal mtimes route to prefer-remote. Emit
                // a structured log line so operators can audit
                // silently-resolved conflicts. ncx.41.
                log::info!(
                    "conflict_resolver: newest_wins tie-break sync_id={} path={} \
                     mtime={} — prefer-remote (server-wins default)",
                    sync_id.get(),
                    path,
                    local,
                );
            }
            return resolve_prefer_remote(sync_id, path, kind);
        }
    }

    // Exactly one side's timestamp is unknown. Log the fall-through so
    // operators can distinguish "tie on equal mtimes" from "no mtime
    // available" — both route to prefer-remote but the operational
    // cause is different.
    if local_mtime_secs.is_some() ^ remote_mtime_secs.is_some() {
        log::info!(
            "conflict_resolver: newest_wins missing one mtime sync_id={} path={} \
             local={:?} remote={:?} — prefer-remote (server-wins default)",
            sync_id.get(),
            path,
            local_mtime_secs,
            remote_mtime_secs,
        );
    }

    // P2-a (H1): The previous implementation called
    // `std::fs::metadata(path)` with a sync-root-relative string. In the
    // daemon that path resolves against the daemon's cwd (not the sync
    // root), so the call almost always failed and silently fell through
    // to prefer-remote. Callers that want a real local-mtime read must
    // use [`ConflictResolver::resolve_with_sync_root`], which builds an
    // absolute path from the sync-root base first.
    //
    // With no timestamp information available we fall back to
    // prefer-remote (server-wins tie-break), matching the documented
    // contract at the top of [`ConflictResolver::resolve`].
    let _ = (path, kind); // suppress unused-variable lint in the fallback branch
    resolve_prefer_remote(sync_id, path, kind)
}

/// Build a conflict-safe rename path for a local or remote copy.
///
/// Given `"docs/report.txt"` and label `"local"`, produces
/// `"docs/report.conflict-local.txt"`.  For paths with no extension,
/// produces `"docs/report.conflict-local"`.
fn conflict_rename_path(path: &str, label: &str) -> String {
    if let Some(dot_pos) = path.rfind('.') {
        // Ensure the dot belongs to the final path segment, not a parent dir.
        let last_sep = path.rfind('/').map(|p| p + 1).unwrap_or(0);
        if dot_pos > last_sep {
            let stem = &path[..dot_pos];
            let ext = &path[dot_pos + 1..];
            return format!("{stem}.conflict-{label}.{ext}");
        }
    }
    format!("{path}.conflict-{label}")
}

fn resolve_rename_both(sync_id: SyncId, path: &str, kind: &ConflictKind) -> ConflictResolution {
    match kind {
        // For symmetric modify-vs-modify conflicts we can produce distinct
        // rename paths that preserve both copies.  The sync loop is
        // responsible for executing both rename operations atomically.
        ConflictKind::LocalModifyVsRemoteModify => ConflictResolution::RenameBoth {
            local_renamed_path: conflict_rename_path(path, "local"),
            remote_renamed_path: conflict_rename_path(path, "remote"),
            original_path: path.to_owned(),
            sync_id,
        },
        // For asymmetric conflicts (delete on one side) we cannot
        // meaningfully rename-both — the deleted copy no longer exists.
        // Fall through to ManualReview so the operator can decide.
        _ => ConflictResolution::ManualReview {
            path: path.to_owned(),
            kind: kind.clone(),
            reason: "rename-both: asymmetric conflict requires manual review".to_owned(),
        },
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
            .resolve(
                &conflict(ConflictKind::LocalModifyVsRemoteModify),
                None,
                None,
            )
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
            .resolve(
                &conflict(ConflictKind::LocalModifyVsRemoteModify),
                None,
                None,
            )
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
            .resolve(&conflict(ConflictKind::TypeMismatch), None, None)
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
            .resolve(
                &conflict(ConflictKind::LocalModifyVsRemoteModify),
                None,
                None,
            )
            .expect("conflict should resolve");

        assert!(matches!(
            resolution,
            ConflictResolution::ManualReview { .. }
        ));
    }

    #[test]
    fn newest_wins_falls_back_to_prefer_remote_when_local_unreadable() {
        let resolver = ConflictResolver {
            default_policy: ConflictPolicy::NewestWins,
        };
        let resolution = resolver
            .resolve(
                &conflict(ConflictKind::LocalModifyVsRemoteModify),
                None,
                None,
            )
            .expect("conflict should resolve");

        // The conflict path "docs/report.txt" does not exist on disk in the
        // test environment, so mtime lookup fails and newest_wins falls back
        // to prefer-remote (server-wins tie-break).
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
    fn rename_both_produces_rename_both_resolution_for_modify_conflict() {
        let resolver = ConflictResolver {
            default_policy: ConflictPolicy::RenameBoth,
        };
        let resolution = resolver
            .resolve(
                &conflict(ConflictKind::LocalModifyVsRemoteModify),
                None,
                None,
            )
            .expect("conflict should resolve");

        match resolution {
            ConflictResolution::RenameBoth {
                local_renamed_path,
                remote_renamed_path,
                original_path,
                ..
            } => {
                assert!(
                    local_renamed_path.contains("conflict-local"),
                    "local rename path must contain 'conflict-local': {local_renamed_path}"
                );
                assert!(
                    remote_renamed_path.contains("conflict-remote"),
                    "remote rename path must contain 'conflict-remote': {remote_renamed_path}"
                );
                assert_eq!(original_path, "docs/report.txt");
            }
            other => panic!("expected RenameBoth, got {other:?}"),
        }
    }

    #[test]
    fn rename_both_falls_back_to_manual_review_for_asymmetric_conflicts() {
        let resolver = ConflictResolver {
            default_policy: ConflictPolicy::RenameBoth,
        };
        // A delete-vs-modify conflict cannot be rename-both'd (nothing to rename).
        let resolution = resolver
            .resolve(
                &conflict(ConflictKind::LocalDeleteVsRemoteModify),
                None,
                None,
            )
            .expect("conflict should resolve");

        assert!(
            matches!(resolution, ConflictResolution::ManualReview { .. }),
            "asymmetric conflict should fall through to ManualReview: {resolution:?}"
        );
    }

    #[test]
    fn conflict_rename_path_produces_correct_stem_and_extension() {
        use super::conflict_rename_path;
        assert_eq!(
            conflict_rename_path("docs/report.txt", "local"),
            "docs/report.conflict-local.txt"
        );
        assert_eq!(
            conflict_rename_path("docs/report", "remote"),
            "docs/report.conflict-remote"
        );
        // Dot in a parent dir component must not be treated as an extension.
        assert_eq!(
            conflict_rename_path("v1.0/notes", "local"),
            "v1.0/notes.conflict-local"
        );
    }

    #[test]
    fn default_policy_is_rename_both() {
        let resolver = ConflictResolver::default();
        assert_eq!(resolver.default_policy, ConflictPolicy::RenameBoth);
    }

    #[test]
    fn newest_wins_with_absolute_path_compares_mtimes_correctly() {
        // P2-a (H1) regression test: the resolver must be able to read
        // local mtime via `resolve_with_sync_root` when given the
        // absolute sync-root base, NOT the daemon cwd.
        use std::io::Write as _;

        let tmp = std::env::temp_dir().join(format!(
            "pcloud-rs-conflict-h1-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("subdir")).unwrap();
        let file_path = tmp.join("subdir").join("file.txt");
        {
            let mut f = std::fs::File::create(&file_path).unwrap();
            f.write_all(b"hello").unwrap();
        }

        let resolver = ConflictResolver {
            default_policy: ConflictPolicy::NewestWins,
        };

        // Sync-root-relative path that would fail under the old
        // `fs::metadata("subdir/file.txt")` (cwd-relative) call.
        let op = PlannedOperation::Conflict {
            sync_id: SyncId::new(1),
            path: "subdir/file.txt".to_owned(),
            kind: ConflictKind::LocalModifyVsRemoteModify,
        };

        // Local file's mtime is "now"; remote mtime is in the distant past.
        // Expect prefer-local → UploadFile.
        let remote_mtime = Some(1_000u64);
        let resolution = resolver
            .resolve_with_sync_root(&op, &tmp, remote_mtime, None)
            .expect("conflict should resolve");
        assert!(
            matches!(
                resolution,
                ConflictResolution::Apply(PlannedOperation::UploadFile { .. })
            ),
            "local file is newer than remote; must prefer local: {resolution:?}"
        );

        // Now flip the comparison: remote mtime in the far future.
        let remote_mtime = Some(u64::MAX / 2);
        let resolution = resolver
            .resolve_with_sync_root(&op, &tmp, remote_mtime, None)
            .expect("conflict should resolve");
        assert!(
            matches!(
                resolution,
                ConflictResolution::Apply(PlannedOperation::DownloadFile { .. })
            ),
            "remote is newer; must prefer remote: {resolution:?}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
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
