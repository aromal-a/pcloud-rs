//! Candidate-ingestion planner.
//!
//! The planner is the join point between the local scanner / fs-event
//! ingestor and the remote diff poller. It consumes
//! [`SyncCandidate`]s — one per observed change on either side — groups
//! them by path, and emits a deterministic stream of
//! [`PlannedOperation`]s.
//!
//! # Pairing rules
//!
//! 1. Candidates are sorted by `(path, source)` so that each path's
//!    local + remote entries sit adjacent; `ChangeSource::Local <
//!    ChangeSource::Remote` by the derived `Ord` instance.
//! 2. For each path group the planner extracts at most one local and
//!    one remote candidate.
//! 3. If only one side changed, the planner emits the matching
//!    single-sided operation (upload/download/mkdir/delete).
//! 4. If both sides changed, the planner decides:
//!    * type mismatch → [`PlannedOperation::Conflict`] with
//!      `ConflictKind::TypeMismatch`,
//!    * both upsert → `LocalModifyVsRemoteModify` conflict,
//!    * local delete + remote modify → `LocalDeleteVsRemoteModify`,
//!    * remote delete + local modify → `RemoteDeleteVsLocalModify`,
//!    * both delete → [`PlannedOperation::DeleteLocal`] (idempotent
//!      convergence).
//!
//! The output is capped by `Planner::max_operations_per_tick` so one
//! scheduler batch can never be starved by an unboundedly large
//! burst.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use pcloud_model::{
    conflict::ConflictKind,
    sync::{ChangeKind, ChangeSource, EntryKind, PlannedOperation, SyncCandidate, SyncType},
};

/// Turns a set of [`SyncCandidate`] changes into executable
/// [`PlannedOperation`]s by pairing local and remote changes per path
/// and surfacing conflicts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Planner {
    /// Maximum number of operations produced per planning tick, to
    /// keep each scheduler batch bounded. Excess candidates are
    /// deferred — a later tick will process them once the scheduler
    /// drains. Default is `1024`.
    pub max_operations_per_tick: usize,
}

impl Default for Planner {
    fn default() -> Self {
        Self {
            max_operations_per_tick: 1024,
        }
    }
}

impl Planner {
    /// Plan executable operations for the given candidates. Candidates
    /// are grouped by path so that simultaneous local and remote
    /// changes surface as a [`PlannedOperation::Conflict`].
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::planner::Planner;
    /// let planner = Planner::default();
    /// // An empty batch of candidates yields an empty plan.
    /// assert!(planner.plan(&[]).is_empty());
    /// ```
    #[must_use]
    pub fn plan(&self, candidates: &[SyncCandidate]) -> Vec<PlannedOperation> {
        self.plan_with_overflow(candidates).0
    }

    /// Plan executable operations and additionally return the candidates
    /// that were **skipped because the per-tick operation cap was
    /// reached**.
    ///
    /// The returned overflow list contains the original `SyncCandidate`s
    /// (not planned operations) so callers can persist them verbatim in
    /// a dead-letter store and replay them on the next planning tick.
    /// Audit-04 P2-6: previously over-cap candidates were dropped with
    /// only a `warn!` log and had to be re-discovered by the next full
    /// scan; persisting them closes that silent-drop window.
    #[must_use]
    pub fn plan_with_overflow(
        &self,
        candidates: &[SyncCandidate],
    ) -> (Vec<PlannedOperation>, Vec<SyncCandidate>) {
        let mut sorted = candidates.to_vec();
        // F-06: sort and group by (sync_id, path, source) so that the same
        // relative path under different sync roots is never collapsed into a
        // single pairing, preventing cross-root conflict misrouting and path
        // collapse bugs on multi-root configurations.
        sorted.sort_by(|left, right| {
            left.sync_id
                .get()
                .cmp(&right.sync_id.get())
                .then(left.path.cmp(&right.path))
                .then(left.source.cmp(&right.source))
        });

        let mut operations = Vec::new();
        let mut idx = 0usize;
        while idx < sorted.len() && operations.len() < self.max_operations_per_tick {
            let sync_id = sorted[idx].sync_id;
            let path = sorted[idx].path.clone();
            let mut local = None;
            let mut remote = None;

            // Consume all candidates for the same (sync_id, path) group.
            while idx < sorted.len()
                && sorted[idx].sync_id == sync_id
                && sorted[idx].path == path
            {
                match sorted[idx].source {
                    ChangeSource::Local => local = Some(sorted[idx].clone()),
                    ChangeSource::Remote => remote = Some(sorted[idx].clone()),
                }
                idx += 1;
            }

            if let Some(operation) = plan_pair(local.as_ref(), remote.as_ref()) {
                operations.push(operation);
            }
        }

        // Collect skipped candidates for dead-letter persistence. The
        // sync-loop adapter is responsible for writing this list to the
        // `value_kv` store so a crash between planner overflow and the
        // next full scan does not drop them silently.
        let overflow: Vec<SyncCandidate> = sorted[idx..].to_vec();
        if !overflow.is_empty() {
            let skipped_ops = {
                let mut count = 0usize;
                let mut scan = idx;
                while scan < sorted.len() {
                    let cur_id = sorted[scan].sync_id;
                    let cur_path = &sorted[scan].path;
                    while scan < sorted.len()
                        && sorted[scan].sync_id == cur_id
                        && &sorted[scan].path == cur_path
                    {
                        scan += 1;
                    }
                    count += 1;
                }
                count
            };
            let mut affected_ids = std::collections::BTreeSet::new();
            for skipped in &overflow {
                affected_ids.insert(skipped.sync_id.get());
            }
            let ids_str = affected_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            log::warn!(
                "planner cap exceeded: deferring {} operations for sync_id(s)=[{}] \
                 to the dead-letter overflow buffer (will replay next tick)",
                skipped_ops,
                ids_str,
            );
        }

        (operations, overflow)
    }
}

/// Policy controlling which delete operations the planner may emit.
///
/// Applied as a post-plan filter by [`Planner::plan_filtered`]. This
/// separates the mechanical pairing logic (which is `SyncType`-agnostic)
/// from the directional policy that the sync loop enforces per root.
///
/// # Example
///
/// ```
/// use pcloud_engine::planner::DeletePolicy;
/// use pcloud_model::sync::SyncType;
///
/// let policy = DeletePolicy::for_sync_type(SyncType::UploadOnly, true);
/// assert!(policy.allow_delete_remote);
/// assert!(!policy.allow_delete_local);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletePolicy {
    /// Whether `DeleteRemote` operations are allowed.
    pub allow_delete_remote: bool,
    /// Whether `DeleteLocal` operations are allowed.
    pub allow_delete_local: bool,
}

impl Default for DeletePolicy {
    /// Default policy: all deletes allowed (matching `SyncType::Full`
    /// with `propagate_deletes = true`).
    fn default() -> Self {
        Self {
            allow_delete_remote: true,
            allow_delete_local: true,
        }
    }
}

impl DeletePolicy {
    /// Build a policy from the sync root's [`SyncType`] and the global
    /// `propagate_deletes` config flag.
    ///
    /// When `propagate_deletes` is `false`, no deletes are ever emitted
    /// regardless of `SyncType` (ultra-safe mode).
    ///
    /// When `propagate_deletes` is `true`:
    /// - `Full`: both directions allowed.
    /// - `UploadOnly`: local-to-remote deletes (`DeleteRemote`) allowed;
    ///   remote-to-local deletes (`DeleteLocal`) suppressed.
    /// - `DownloadOnly`: remote-to-local deletes (`DeleteLocal`) allowed;
    ///   local-to-remote deletes (`DeleteRemote`) suppressed.
    /// - `BackupArchive`: deletion-safe archival — no deletes in either
    ///   direction, regardless of `propagate_deletes`. A local file that
    ///   the user removes stays on the remote as an archived copy.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::planner::DeletePolicy;
    /// use pcloud_model::sync::SyncType;
    ///
    /// // Ultra-safe: no deletes at all.
    /// let ultra_safe = DeletePolicy::for_sync_type(SyncType::Full, false);
    /// assert!(!ultra_safe.allow_delete_remote);
    /// assert!(!ultra_safe.allow_delete_local);
    ///
    /// // DownloadOnly: only remote->local deletes.
    /// let dl = DeletePolicy::for_sync_type(SyncType::DownloadOnly, true);
    /// assert!(!dl.allow_delete_remote);
    /// assert!(dl.allow_delete_local);
    ///
    /// // BackupArchive: archival — no deletes even with propagate_deletes=true.
    /// let archive = DeletePolicy::for_sync_type(SyncType::BackupArchive, true);
    /// assert!(!archive.allow_delete_remote);
    /// assert!(!archive.allow_delete_local);
    /// ```
    #[must_use]
    pub fn for_sync_type(sync_type: SyncType, propagate_deletes: bool) -> Self {
        if !propagate_deletes {
            return Self {
                allow_delete_remote: false,
                allow_delete_local: false,
            };
        }
        match sync_type {
            SyncType::Full => Self {
                allow_delete_remote: true,
                allow_delete_local: true,
            },
            SyncType::UploadOnly => Self {
                allow_delete_remote: true,
                allow_delete_local: false,
            },
            SyncType::DownloadOnly => Self {
                allow_delete_remote: false,
                allow_delete_local: true,
            },
            // BackupArchive is deletion-safe: uploads new/changed local
            // files, but a local deletion must NOT be mirrored to the
            // remote copy. Remote-to-local deletes never apply because
            // remote changes are not pulled. See bd-1du.5.
            SyncType::BackupArchive => Self {
                allow_delete_remote: false,
                allow_delete_local: false,
            },
        }
    }

    /// Returns `true` if the given operation is a delete that this policy
    /// suppresses.
    #[must_use]
    pub fn suppresses(&self, op: &PlannedOperation) -> bool {
        match op {
            PlannedOperation::DeleteRemote { .. } => !self.allow_delete_remote,
            PlannedOperation::DeleteLocal { .. } => !self.allow_delete_local,
            _ => false,
        }
    }
}

impl Planner {
    /// Plan operations and then filter out deletions according to
    /// `delete_policy`. This is the primary entry point for the sync
    /// loop, which knows the per-root `SyncType` and the global
    /// `propagate_deletes` flag.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::planner::{DeletePolicy, Planner};
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::{
    ///     ChangeKind, ChangeSource, EntryKind, SyncCandidate, SyncType,
    /// };
    ///
    /// let planner = Planner::default();
    /// let policy = DeletePolicy::for_sync_type(SyncType::DownloadOnly, true);
    /// let ops = planner.plan_filtered(
    ///     &[SyncCandidate {
    ///         sync_id: SyncId::new(1),
    ///         source: ChangeSource::Local,
    ///         path: "old.txt".into(),
    ///         entry_kind: EntryKind::File,
    ///         change_kind: ChangeKind::Delete,
    ///         remote_file_id: None,
    ///         remote_folder_id: None,
    ///     }],
    ///     &policy,
    /// );
    /// // DownloadOnly suppresses DeleteRemote (local->remote delete).
    /// assert!(ops.is_empty());
    /// ```
    #[must_use]
    pub fn plan_filtered(
        &self,
        candidates: &[SyncCandidate],
        delete_policy: &DeletePolicy,
    ) -> Vec<PlannedOperation> {
        self.plan_filtered_with_overflow(candidates, delete_policy)
            .0
    }

    /// [`Self::plan_filtered`] variant that additionally returns the
    /// per-tick overflow candidates. See [`Self::plan_with_overflow`].
    #[must_use]
    pub fn plan_filtered_with_overflow(
        &self,
        candidates: &[SyncCandidate],
        delete_policy: &DeletePolicy,
    ) -> (Vec<PlannedOperation>, Vec<SyncCandidate>) {
        let (ops, overflow) = self.plan_with_overflow(candidates);
        let filtered = ops
            .into_iter()
            .filter(|op| !delete_policy.suppresses(op))
            .collect();
        (filtered, overflow)
    }
}

fn plan_pair(
    local: Option<&SyncCandidate>,
    remote: Option<&SyncCandidate>,
) -> Option<PlannedOperation> {
    match (local, remote) {
        (Some(local), Some(remote)) => Some(plan_conflict_or_resolution(local, remote)),
        (Some(local), None) => Some(plan_single(local)),
        (None, Some(remote)) => Some(plan_single(remote)),
        (None, None) => None,
    }
}

fn plan_single(candidate: &SyncCandidate) -> PlannedOperation {
    match (
        candidate.source,
        candidate.entry_kind,
        candidate.change_kind,
    ) {
        (ChangeSource::Local, EntryKind::File, ChangeKind::Upsert) => {
            PlannedOperation::UploadFile {
                sync_id: candidate.sync_id,
                path: candidate.path.clone(),
                remote_parent_folder_id: candidate.remote_folder_id,
                remote_name: basename(&candidate.path).to_owned(),
            }
        }
        (ChangeSource::Remote, EntryKind::File, ChangeKind::Upsert) => {
            PlannedOperation::DownloadFile {
                sync_id: candidate.sync_id,
                path: candidate.path.clone(),
                remote_file_id: candidate.remote_file_id,
            }
        }
        (ChangeSource::Local, EntryKind::Folder, ChangeKind::Upsert) => {
            PlannedOperation::CreateRemoteDirectory {
                sync_id: candidate.sync_id,
                path: candidate.path.clone(),
            }
        }
        (ChangeSource::Remote, EntryKind::Folder, ChangeKind::Upsert) => {
            PlannedOperation::CreateLocalDirectory {
                sync_id: candidate.sync_id,
                path: candidate.path.clone(),
                remote_folder_id: candidate.remote_folder_id,
            }
        }
        (ChangeSource::Local, _, ChangeKind::Delete) => PlannedOperation::DeleteRemote {
            sync_id: candidate.sync_id,
            path: candidate.path.clone(),
        },
        (ChangeSource::Remote, _, ChangeKind::Delete) => PlannedOperation::DeleteLocal {
            sync_id: candidate.sync_id,
            path: candidate.path.clone(),
        },
    }
}

fn plan_conflict_or_resolution(local: &SyncCandidate, remote: &SyncCandidate) -> PlannedOperation {
    if local.entry_kind != remote.entry_kind {
        return conflict(local, ConflictKind::TypeMismatch);
    }

    match (local.change_kind, remote.change_kind) {
        (ChangeKind::Upsert, ChangeKind::Upsert) => {
            conflict(local, ConflictKind::LocalModifyVsRemoteModify)
        }
        (ChangeKind::Delete, ChangeKind::Upsert) => {
            conflict(local, ConflictKind::LocalDeleteVsRemoteModify)
        }
        (ChangeKind::Upsert, ChangeKind::Delete) => {
            conflict(local, ConflictKind::RemoteDeleteVsLocalModify)
        }
        (ChangeKind::Delete, ChangeKind::Delete) => PlannedOperation::DeleteLocal {
            sync_id: remote.sync_id,
            path: remote.path.clone(),
        },
    }
}

fn conflict(candidate: &SyncCandidate, kind: ConflictKind) -> PlannedOperation {
    PlannedOperation::Conflict {
        sync_id: candidate.sync_id,
        path: candidate.path.clone(),
        kind,
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use pcloud_model::{
        conflict::ConflictKind,
        ids::{RemoteFileId, SyncId},
        sync::{ChangeKind, ChangeSource, EntryKind, PlannedOperation, SyncCandidate},
    };

    use pcloud_model::sync::SyncType;

    use super::{DeletePolicy, Planner};

    fn candidate(
        source: ChangeSource,
        path: &str,
        entry_kind: EntryKind,
        change_kind: ChangeKind,
    ) -> SyncCandidate {
        SyncCandidate {
            sync_id: SyncId::new(1),
            source,
            path: path.to_owned(),
            entry_kind,
            change_kind,
            remote_file_id: Some(RemoteFileId::new(7)),
            remote_folder_id: None,
        }
    }

    #[test]
    fn planner_maps_single_local_file_change_to_upload() {
        let planner = Planner::default();
        let operations = planner.plan(&[candidate(
            ChangeSource::Local,
            "docs/report.txt",
            EntryKind::File,
            ChangeKind::Upsert,
        )]);

        assert_eq!(
            operations,
            vec![PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "report.txt".to_owned(),
            }]
        );
    }

    #[test]
    fn planner_maps_single_remote_delete_to_local_delete() {
        let planner = Planner::default();
        let operations = planner.plan(&[candidate(
            ChangeSource::Remote,
            "docs/report.txt",
            EntryKind::File,
            ChangeKind::Delete,
        )]);

        assert_eq!(
            operations,
            vec![PlannedOperation::DeleteLocal {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
            }]
        );
    }

    #[test]
    fn planner_surfaces_conflict_when_local_and_remote_modify_same_path() {
        let planner = Planner::default();
        let operations = planner.plan(&[
            candidate(
                ChangeSource::Local,
                "docs/report.txt",
                EntryKind::File,
                ChangeKind::Upsert,
            ),
            candidate(
                ChangeSource::Remote,
                "docs/report.txt",
                EntryKind::File,
                ChangeKind::Upsert,
            ),
        ]);

        assert_eq!(
            operations,
            vec![PlannedOperation::Conflict {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
                kind: ConflictKind::LocalModifyVsRemoteModify,
            }]
        );
    }

    // -- DeletePolicy tests --

    #[test]
    fn delete_policy_full_propagates_both_directions() {
        let policy = DeletePolicy::for_sync_type(SyncType::Full, true);
        assert!(policy.allow_delete_remote);
        assert!(policy.allow_delete_local);

        let planner = Planner::default();
        let ops = planner.plan_filtered(
            &[
                candidate(
                    ChangeSource::Local,
                    "a.txt",
                    EntryKind::File,
                    ChangeKind::Delete,
                ),
                candidate(
                    ChangeSource::Remote,
                    "b.txt",
                    EntryKind::File,
                    ChangeKind::Delete,
                ),
            ],
            &policy,
        );
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], PlannedOperation::DeleteRemote { .. }));
        assert!(matches!(ops[1], PlannedOperation::DeleteLocal { .. }));
    }

    #[test]
    fn delete_policy_upload_only_suppresses_delete_local() {
        let policy = DeletePolicy::for_sync_type(SyncType::UploadOnly, true);
        assert!(policy.allow_delete_remote);
        assert!(!policy.allow_delete_local);

        let planner = Planner::default();
        let ops = planner.plan_filtered(
            &[
                candidate(
                    ChangeSource::Local,
                    "a.txt",
                    EntryKind::File,
                    ChangeKind::Delete,
                ),
                candidate(
                    ChangeSource::Remote,
                    "b.txt",
                    EntryKind::File,
                    ChangeKind::Delete,
                ),
            ],
            &policy,
        );
        // Local delete -> DeleteRemote: allowed
        // Remote delete -> DeleteLocal: suppressed
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], PlannedOperation::DeleteRemote { .. }));
    }

    #[test]
    fn delete_policy_download_only_suppresses_delete_remote() {
        let policy = DeletePolicy::for_sync_type(SyncType::DownloadOnly, true);
        assert!(!policy.allow_delete_remote);
        assert!(policy.allow_delete_local);

        let planner = Planner::default();
        let ops = planner.plan_filtered(
            &[
                candidate(
                    ChangeSource::Local,
                    "a.txt",
                    EntryKind::File,
                    ChangeKind::Delete,
                ),
                candidate(
                    ChangeSource::Remote,
                    "b.txt",
                    EntryKind::File,
                    ChangeKind::Delete,
                ),
            ],
            &policy,
        );
        // Local delete -> DeleteRemote: suppressed
        // Remote delete -> DeleteLocal: allowed
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], PlannedOperation::DeleteLocal { .. }));
    }

    #[test]
    fn delete_policy_propagate_false_suppresses_all_deletes() {
        let policy = DeletePolicy::for_sync_type(SyncType::Full, false);
        assert!(!policy.allow_delete_remote);
        assert!(!policy.allow_delete_local);

        let planner = Planner::default();
        let ops = planner.plan_filtered(
            &[
                candidate(
                    ChangeSource::Local,
                    "a.txt",
                    EntryKind::File,
                    ChangeKind::Delete,
                ),
                candidate(
                    ChangeSource::Remote,
                    "b.txt",
                    EntryKind::File,
                    ChangeKind::Delete,
                ),
            ],
            &policy,
        );
        assert!(ops.is_empty());
    }

    #[test]
    fn delete_policy_backup_archive_suppresses_all_deletes_but_keeps_uploads() {
        // bd-1du.5: BackupArchive is deletion-safe. A local file is
        // uploaded the first time it appears; if the user later deletes
        // the local file, the planner MUST NOT emit a DeleteRemote so
        // the archived remote copy survives.
        let archive_policy = DeletePolicy::for_sync_type(SyncType::BackupArchive, true);
        assert!(!archive_policy.allow_delete_remote);
        assert!(!archive_policy.allow_delete_local);

        let upload_only_policy = DeletePolicy::for_sync_type(SyncType::UploadOnly, true);

        let planner = Planner::default();

        // Step 1: local file exists → upload planned under both flavors.
        let existing = vec![candidate(
            ChangeSource::Local,
            "docs/keepme.txt",
            EntryKind::File,
            ChangeKind::Upsert,
        )];
        let archive_upload_ops = planner.plan_filtered(&existing, &archive_policy);
        let upload_only_upload_ops = planner.plan_filtered(&existing, &upload_only_policy);
        assert_eq!(archive_upload_ops.len(), 1);
        assert!(matches!(
            archive_upload_ops[0],
            PlannedOperation::UploadFile { .. }
        ));
        assert_eq!(upload_only_upload_ops.len(), 1);
        assert!(matches!(
            upload_only_upload_ops[0],
            PlannedOperation::UploadFile { .. }
        ));

        // Step 2: local file is deleted. BackupArchive must emit zero
        // DeleteRemote ops; UploadOnly must emit at least one.
        let deleted = vec![candidate(
            ChangeSource::Local,
            "docs/keepme.txt",
            EntryKind::File,
            ChangeKind::Delete,
        )];
        let archive_delete_ops = planner.plan_filtered(&deleted, &archive_policy);
        let upload_only_delete_ops = planner.plan_filtered(&deleted, &upload_only_policy);

        let archive_remote_deletes = archive_delete_ops
            .iter()
            .filter(|op| matches!(op, PlannedOperation::DeleteRemote { .. }))
            .count();
        let upload_only_remote_deletes = upload_only_delete_ops
            .iter()
            .filter(|op| matches!(op, PlannedOperation::DeleteRemote { .. }))
            .count();

        assert_eq!(
            archive_remote_deletes, 0,
            "BackupArchive must never emit DeleteRemote for local deletions, got {archive_delete_ops:?}"
        );
        assert!(
            upload_only_remote_deletes >= 1,
            "UploadOnly must propagate a local delete to DeleteRemote, got {upload_only_delete_ops:?}"
        );
    }

    #[test]
    fn delete_policy_does_not_suppress_non_delete_ops() {
        let policy = DeletePolicy::for_sync_type(SyncType::Full, false);
        let planner = Planner::default();
        let ops = planner.plan_filtered(
            &[candidate(
                ChangeSource::Local,
                "new.txt",
                EntryKind::File,
                ChangeKind::Upsert,
            )],
            &policy,
        );
        // Upload is not a delete, should pass through even when propagate_deletes=false
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], PlannedOperation::UploadFile { .. }));
    }
}
