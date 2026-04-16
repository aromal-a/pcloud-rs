//! Priority-aware scheduler queue for planned operations.
//!
//! The scheduler owns the ordered queue of [`PlannedOperation`]s
//! produced by [`crate::planner::Planner`] and hands out the next
//! batch to the transfer coordinators. Ordering is
//! **priority-then-path**: operation priorities are defined by
//! [`PlannedOperation::priority`] (lower = more urgent), with lexical
//! path ordering as a deterministic tiebreaker.
//!
//! # Batch semantics
//!
//! `Scheduler::next_batch` is a **peek** — it does not mutate the
//! queue. The embedding engine call
//! [`crate::EngineShell::advance_transfer_cycle`] hands each batch to
//! the upload and download coordinators; those coordinators manage
//! their own in-flight slots. The batch width is bounded by
//! `max_parallel_uploads + max_parallel_downloads` (minimum 1) so the
//! coordinators can always make forward progress.
//!
//! # Eviction
//!
//! `Scheduler::evict_sync_id` removes every queued operation that
//! belongs to a sync root being paused or removed, matching the C
//! `psync_delete_sync` teardown behavior.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use pcloud_model::ids::SyncId;
use pcloud_model::sync::PlannedOperation;

/// Priority-ordered queue of [`PlannedOperation`]s plus parallelism
/// limits, used to hand out the next ready batch to the transfer
/// coordinators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scheduler {
    /// Maximum number of upload operations that may be in flight at
    /// once across all sync roots. Default is 4.
    pub max_parallel_uploads: usize,
    /// Maximum number of download operations that may be in flight at
    /// once across all sync roots. Default is 4.
    pub max_parallel_downloads: usize,
    /// Operations waiting to be dispatched, ordered by
    /// [`PlannedOperation::priority`] (ascending) then path. Conflicts
    /// therefore appear first, followed by deletes, directory
    /// operations, then file transfers.
    pub queued_operations: Vec<PlannedOperation>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            max_parallel_uploads: 4,
            max_parallel_downloads: 4,
            queued_operations: Vec::new(),
        }
    }
}

impl Scheduler {
    /// Replace the queued operations with `operations`, sorting them by
    /// operation priority and then path for deterministic ordering.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::scheduler::Scheduler;
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    ///
    /// let mut s = Scheduler::default();
    /// s.replace_queue(vec![PlannedOperation::DeleteLocal {
    ///     sync_id: SyncId::new(1),
    ///     path: "a".into(),
    /// }]);
    /// assert_eq!(s.next_batch().len(), 1);
    /// ```
    pub fn replace_queue(&mut self, mut operations: Vec<PlannedOperation>) {
        operations.sort_by(|left, right| {
            left.priority()
                .cmp(&right.priority())
                .then(left.path().cmp(right.path()))
        });
        self.queued_operations = operations;
    }

    /// Remove all queued operations belonging to `sync_id`.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::scheduler::Scheduler;
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    ///
    /// let mut s = Scheduler::default();
    /// s.replace_queue(vec![
    ///     PlannedOperation::DeleteLocal { sync_id: SyncId::new(1), path: "a".into() },
    ///     PlannedOperation::DeleteLocal { sync_id: SyncId::new(2), path: "b".into() },
    /// ]);
    /// s.evict_sync_id(SyncId::new(1));
    /// assert_eq!(s.queued_operations.len(), 1);
    /// ```
    pub fn evict_sync_id(&mut self, sync_id: SyncId) {
        self.queued_operations
            .retain(|operation| operation.sync_id() != sync_id);
    }

    /// Peek at the next batch of operations that should be dispatched,
    /// bounded by the combined upload/download parallelism limit.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::scheduler::Scheduler;
    /// let s = Scheduler::default();
    /// // An empty scheduler returns an empty batch (never a panic).
    /// assert!(s.next_batch().is_empty());
    /// ```
    #[must_use]
    pub fn next_batch(&self) -> &[PlannedOperation] {
        let limit = self.max_parallel_uploads + self.max_parallel_downloads;
        let limit = limit.max(1).min(self.queued_operations.len());
        &self.queued_operations[..limit]
    }
}

#[cfg(test)]
mod tests {
    use pcloud_model::{
        conflict::ConflictKind,
        ids::{RemoteFileId, SyncId},
        sync::PlannedOperation,
    };

    use super::Scheduler;

    #[test]
    fn scheduler_orders_conflicts_before_transfers() {
        let mut scheduler = Scheduler::default();
        scheduler.replace_queue(vec![
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "b.txt".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "b.txt".to_owned(),
            },
            PlannedOperation::Conflict {
                sync_id: SyncId::new(1),
                path: "a.txt".to_owned(),
                kind: ConflictKind::LocalModifyVsRemoteModify,
            },
            PlannedOperation::DownloadFile {
                sync_id: SyncId::new(1),
                path: "a.bin".to_owned(),
                remote_file_id: Some(RemoteFileId::new(9)),
            },
        ]);

        assert!(matches!(
            scheduler.queued_operations.first(),
            Some(PlannedOperation::Conflict { .. })
        ));
    }

    #[test]
    fn scheduler_limits_batch_by_parallel_capacity() {
        let mut scheduler = Scheduler {
            max_parallel_uploads: 1,
            max_parallel_downloads: 1,
            queued_operations: Vec::new(),
        };
        scheduler.replace_queue(vec![
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "a".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "a".to_owned(),
            },
            PlannedOperation::DownloadFile {
                sync_id: SyncId::new(1),
                path: "b".to_owned(),
                remote_file_id: Some(RemoteFileId::new(2)),
            },
            PlannedOperation::DeleteLocal {
                sync_id: SyncId::new(1),
                path: "c".to_owned(),
            },
        ]);

        assert_eq!(scheduler.next_batch().len(), 2);
    }

    #[test]
    fn scheduler_evicts_operations_for_removed_sync_root() {
        let mut scheduler = Scheduler::default();
        scheduler.replace_queue(vec![
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "a".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "a".to_owned(),
            },
            PlannedOperation::DownloadFile {
                sync_id: SyncId::new(2),
                path: "b".to_owned(),
                remote_file_id: Some(RemoteFileId::new(2)),
            },
        ]);

        scheduler.evict_sync_id(SyncId::new(1));

        assert_eq!(scheduler.queued_operations.len(), 1);
        assert_eq!(scheduler.queued_operations[0].sync_id(), SyncId::new(2));
    }
}
