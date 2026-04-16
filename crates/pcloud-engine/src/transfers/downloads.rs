// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use pcloud_model::{
    ids::SyncId,
    sync::PlannedOperation,
    transfer::{TransferState, TransferTask},
};

/// Tracks the download side of the transfer cycle: in-flight file
/// downloads, pending local deletes, pending local directory creates,
/// completed tasks, and failed tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadCoordinator {
    /// Maximum number of concurrent range requests per file.
    pub max_range_requests: usize,
    /// File downloads that are currently in flight.
    pub active_downloads: Vec<TransferTask>,
    /// Local delete operations waiting to execute.
    pub pending_local_deletes: Vec<TransferTask>,
    /// Local directory-create operations waiting to execute.
    pub pending_directory_creates: Vec<TransferTask>,
    /// Tasks that completed successfully in this cycle.
    pub completed: Vec<TransferTask>,
    /// Tasks that failed in this cycle.
    pub failed: Vec<TransferTask>,
}

impl Default for DownloadCoordinator {
    fn default() -> Self {
        Self {
            max_range_requests: 8,
            active_downloads: Vec::new(),
            pending_local_deletes: Vec::new(),
            pending_directory_creates: Vec::new(),
            completed: Vec::new(),
            failed: Vec::new(),
        }
    }
}

impl DownloadCoordinator {
    /// Accept a new scheduler batch and partition download-side
    /// operations into their respective in-flight lists. Previous
    /// active/pending tasks are cleared.
    pub fn accept_batch(&mut self, operations: &[PlannedOperation]) {
        self.active_downloads.clear();
        self.pending_local_deletes.clear();
        self.pending_directory_creates.clear();

        for operation in operations {
            match operation {
                PlannedOperation::DownloadFile { .. } => {
                    self.active_downloads.push(started_task(operation.clone()));
                }
                PlannedOperation::DeleteLocal { .. } => {
                    self.pending_local_deletes
                        .push(started_task(operation.clone()));
                }
                PlannedOperation::CreateLocalDirectory { .. } => {
                    self.pending_directory_creates
                        .push(started_task(operation.clone()));
                }
                _ => {}
            }
        }
    }

    /// Remove all download-side tasks (active, pending, completed,
    /// failed) belonging to `sync_id`.
    pub fn evict_sync_id(&mut self, sync_id: SyncId) {
        retain_other_sync_ids(&mut self.active_downloads, sync_id);
        retain_other_sync_ids(&mut self.pending_local_deletes, sync_id);
        retain_other_sync_ids(&mut self.pending_directory_creates, sync_id);
        retain_other_sync_ids(&mut self.completed, sync_id);
        retain_other_sync_ids(&mut self.failed, sync_id);
    }

    /// Total number of download-side tasks currently in flight or
    /// pending execution.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active_downloads.len()
            + self.pending_local_deletes.len()
            + self.pending_directory_creates.len()
    }

    /// Number of tasks that have completed successfully in this cycle.
    #[must_use]
    pub fn completed_count(&self) -> usize {
        self.completed.len()
    }

    /// Number of tasks that have failed in this cycle.
    #[must_use]
    pub fn failed_count(&self) -> usize {
        self.failed.len()
    }

    /// Mark the download-side task at `path` as completed. Returns
    /// `true` if a matching task was found and moved.
    pub fn mark_completed(&mut self, path: &str) -> bool {
        move_task(
            path,
            &mut self.active_downloads,
            &mut self.completed,
            TransferState::Completed,
            None,
        ) || move_task(
            path,
            &mut self.pending_local_deletes,
            &mut self.completed,
            TransferState::Completed,
            None,
        ) || move_task(
            path,
            &mut self.pending_directory_creates,
            &mut self.completed,
            TransferState::Completed,
            None,
        )
    }

    /// Mark the download-side task at `path` as failed with `error`.
    /// Returns `true` if a matching task was found and moved.
    pub fn mark_failed(&mut self, path: &str, error: impl Into<String>) -> bool {
        let error = error.into();
        move_task(
            path,
            &mut self.active_downloads,
            &mut self.failed,
            TransferState::Failed,
            Some(error.clone()),
        ) || move_task(
            path,
            &mut self.pending_local_deletes,
            &mut self.failed,
            TransferState::Failed,
            Some(error.clone()),
        ) || move_task(
            path,
            &mut self.pending_directory_creates,
            &mut self.failed,
            TransferState::Failed,
            Some(error),
        )
    }
}

fn started_task(operation: PlannedOperation) -> TransferTask {
    TransferTask {
        operation,
        state: TransferState::Streaming,
        last_error: None,
    }
}

fn move_task(
    path: &str,
    source: &mut Vec<TransferTask>,
    destination: &mut Vec<TransferTask>,
    state: TransferState,
    error: Option<String>,
) -> bool {
    let Some(index) = source.iter().position(|task| task.operation.path() == path) else {
        return false;
    };
    let mut task = source.remove(index);
    task.state = state;
    task.last_error = error;
    destination.push(task);
    true
}

fn retain_other_sync_ids(tasks: &mut Vec<TransferTask>, sync_id: SyncId) {
    tasks.retain(|task| task.operation.sync_id() != sync_id);
}

#[cfg(test)]
mod tests {
    use pcloud_model::{
        ids::{RemoteFileId, RemoteFolderId, SyncId},
        sync::PlannedOperation,
        transfer::TransferState,
    };

    use super::DownloadCoordinator;

    #[test]
    fn accept_batch_partitions_download_side_operations() {
        let mut coordinator = DownloadCoordinator::default();
        coordinator.accept_batch(&[
            PlannedOperation::DownloadFile {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
                remote_file_id: Some(RemoteFileId::new(2)),
            },
            PlannedOperation::DeleteLocal {
                sync_id: SyncId::new(1),
                path: "docs/old.txt".to_owned(),
            },
            PlannedOperation::CreateLocalDirectory {
                sync_id: SyncId::new(1),
                path: "docs/archive".to_owned(),
                remote_folder_id: Some(RemoteFolderId::new(3)),
            },
        ]);

        assert_eq!(coordinator.active_downloads.len(), 1);
        assert_eq!(coordinator.pending_local_deletes.len(), 1);
        assert_eq!(coordinator.pending_directory_creates.len(), 1);
        assert_eq!(
            coordinator.active_downloads[0].state,
            TransferState::Streaming
        );
    }

    #[test]
    fn lifecycle_tracks_completed_and_failed_download_tasks() {
        let mut coordinator = DownloadCoordinator::default();
        coordinator.accept_batch(&[
            PlannedOperation::DownloadFile {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
                remote_file_id: Some(RemoteFileId::new(2)),
            },
            PlannedOperation::DeleteLocal {
                sync_id: SyncId::new(1),
                path: "docs/old.txt".to_owned(),
            },
        ]);

        assert!(coordinator.mark_completed("docs/report.txt"));
        assert!(coordinator.mark_failed("docs/old.txt", "permission denied"));
        assert_eq!(coordinator.completed_count(), 1);
        assert_eq!(coordinator.failed_count(), 1);
        assert_eq!(coordinator.failed[0].state, TransferState::Failed);
    }

    #[test]
    fn evict_sync_id_removes_download_side_tasks() {
        let mut coordinator = DownloadCoordinator::default();
        coordinator.accept_batch(&[
            PlannedOperation::DownloadFile {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
                remote_file_id: Some(RemoteFileId::new(2)),
            },
            PlannedOperation::DeleteLocal {
                sync_id: SyncId::new(2),
                path: "docs/old.txt".to_owned(),
            },
        ]);
        assert!(coordinator.mark_completed("docs/report.txt"));

        coordinator.evict_sync_id(SyncId::new(1));

        assert!(coordinator.completed.is_empty());
        assert_eq!(coordinator.pending_local_deletes.len(), 1);
        assert_eq!(
            coordinator.pending_local_deletes[0].operation.sync_id(),
            SyncId::new(2)
        );
    }
}
