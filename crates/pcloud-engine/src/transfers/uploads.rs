// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use pcloud_model::{
    ids::SyncId,
    sync::PlannedOperation,
    transfer::{TransferState, TransferTask},
};

/// Tracks the upload side of the transfer cycle: in-flight file
/// uploads, pending remote deletes, pending remote directory creates,
/// completed tasks, and failed tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadCoordinator {
    /// Upload chunk size in bytes for streaming writes.
    pub chunk_size_bytes: usize,
    /// File uploads that are currently in flight.
    pub active_uploads: Vec<TransferTask>,
    /// Remote delete operations waiting to execute.
    pub pending_remote_deletes: Vec<TransferTask>,
    /// Remote directory-create operations waiting to execute.
    pub pending_directory_creates: Vec<TransferTask>,
    /// Tasks that completed successfully in this cycle.
    pub completed: Vec<TransferTask>,
    /// Tasks that failed in this cycle.
    pub failed: Vec<TransferTask>,
}

impl Default for UploadCoordinator {
    fn default() -> Self {
        Self {
            chunk_size_bytes: 8 * 1024 * 1024,
            active_uploads: Vec::new(),
            pending_remote_deletes: Vec::new(),
            pending_directory_creates: Vec::new(),
            completed: Vec::new(),
            failed: Vec::new(),
        }
    }
}

impl UploadCoordinator {
    /// Accept a new scheduler batch and partition upload-side
    /// operations into their respective in-flight lists. Previous
    /// active/pending tasks are cleared.
    pub fn accept_batch(&mut self, operations: &[PlannedOperation]) {
        self.active_uploads.clear();
        self.pending_remote_deletes.clear();
        self.pending_directory_creates.clear();

        for operation in operations {
            match operation {
                PlannedOperation::UploadFile { .. } => {
                    self.active_uploads.push(started_task(operation.clone()));
                }
                PlannedOperation::DeleteRemote { .. } => {
                    self.pending_remote_deletes
                        .push(started_task(operation.clone()));
                }
                PlannedOperation::CreateRemoteDirectory { .. } => {
                    self.pending_directory_creates
                        .push(started_task(operation.clone()));
                }
                _ => {}
            }
        }
    }

    /// Remove all upload-side tasks (active, pending, completed,
    /// failed) belonging to `sync_id`.
    pub fn evict_sync_id(&mut self, sync_id: SyncId) {
        retain_other_sync_ids(&mut self.active_uploads, sync_id);
        retain_other_sync_ids(&mut self.pending_remote_deletes, sync_id);
        retain_other_sync_ids(&mut self.pending_directory_creates, sync_id);
        retain_other_sync_ids(&mut self.completed, sync_id);
        retain_other_sync_ids(&mut self.failed, sync_id);
    }

    /// Total number of upload-side tasks currently in flight or
    /// pending execution.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active_uploads.len()
            + self.pending_remote_deletes.len()
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

    /// Mark the upload-side task at `path` as completed. Returns
    /// `true` if a matching task was found and moved.
    pub fn mark_completed(&mut self, path: &str) -> bool {
        move_task(
            path,
            &mut self.active_uploads,
            &mut self.completed,
            TransferState::Completed,
            None,
        ) || move_task(
            path,
            &mut self.pending_remote_deletes,
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

    /// Remove the failed task at `path` from the failed list without
    /// re-queuing it. Used by [`crate::EngineShell::requeue_for_retry`]
    /// to clear the stale failed entry before pushing the operation back
    /// onto the scheduler. Returns `true` if a matching entry was found.
    pub fn clear_failed(&mut self, path: &str) -> bool {
        let before = self.failed.len();
        self.failed.retain(|t| t.operation.path() != path);
        self.failed.len() < before
    }

    /// Mark the upload-side task at `path` as failed with `error`.
    /// Returns `true` if a matching task was found and moved.
    pub fn mark_failed(&mut self, path: &str, error: impl Into<String>) -> bool {
        let error = error.into();
        move_task(
            path,
            &mut self.active_uploads,
            &mut self.failed,
            TransferState::Failed,
            Some(error.clone()),
        ) || move_task(
            path,
            &mut self.pending_remote_deletes,
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

/// Per-file chunked upload progress tracker used by the sync loop to
/// drive `upload_create` / `upload_write` / `upload_save` in bounded
/// chunks and to resume from the last completed offset on restart.
///
/// The `UploadCoordinator` holds a map of active trackers keyed by
/// relative path so the sync loop can ask "where did this file leave
/// off?" and resume from the last acknowledged offset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkedUploadTracker {
    /// Server-assigned `upload_id` from `upload_create`.
    pub upload_id: u64,
    /// Total file size in bytes.
    pub total_size: u64,
    /// Bytes fully acknowledged by the server so far.
    pub acked_offset: u64,
    /// Configured chunk size for this upload.
    pub chunk_size: usize,
    /// Number of chunks successfully written.
    pub chunks_done: u64,
}

impl ChunkedUploadTracker {
    /// Build a fresh tracker for a newly created upload session.
    #[must_use]
    pub fn new(upload_id: u64, total_size: u64, chunk_size: usize) -> Self {
        Self {
            upload_id,
            total_size,
            acked_offset: 0,
            chunk_size,
            chunks_done: 0,
        }
    }

    /// Record a successful chunk write. Returns the new acknowledged
    /// offset.
    pub fn advance(&mut self, bytes_written: u64) -> u64 {
        self.acked_offset += bytes_written;
        self.chunks_done += 1;
        self.acked_offset
    }

    /// Whether all bytes have been acknowledged.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.acked_offset >= self.total_size
    }

    /// Remaining bytes to upload.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.total_size.saturating_sub(self.acked_offset)
    }

    /// Size of the next chunk to write. Capped at `chunk_size` or
    /// [`Self::remaining`], whichever is smaller.
    #[must_use]
    pub fn next_chunk_size(&self) -> usize {
        self.remaining().min(self.chunk_size as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use pcloud_model::{ids::SyncId, sync::PlannedOperation, transfer::TransferState};

    use super::UploadCoordinator;

    #[test]
    fn accept_batch_partitions_upload_side_operations() {
        let mut coordinator = UploadCoordinator::default();
        coordinator.accept_batch(&[
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "report.txt".to_owned(),
            },
            PlannedOperation::DeleteRemote {
                sync_id: SyncId::new(1),
                path: "docs/old.txt".to_owned(),
            },
            PlannedOperation::CreateRemoteDirectory {
                sync_id: SyncId::new(1),
                path: "docs/archive".to_owned(),
            },
        ]);

        assert_eq!(coordinator.active_uploads.len(), 1);
        assert_eq!(coordinator.pending_remote_deletes.len(), 1);
        assert_eq!(coordinator.pending_directory_creates.len(), 1);
        assert_eq!(
            coordinator.active_uploads[0].state,
            TransferState::Streaming
        );
    }

    #[test]
    fn lifecycle_tracks_completed_and_failed_upload_tasks() {
        let mut coordinator = UploadCoordinator::default();
        coordinator.accept_batch(&[
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "report.txt".to_owned(),
            },
            PlannedOperation::DeleteRemote {
                sync_id: SyncId::new(1),
                path: "docs/old.txt".to_owned(),
            },
        ]);

        assert!(coordinator.mark_completed("docs/report.txt"));
        assert!(coordinator.mark_failed("docs/old.txt", "remote denied"));
        assert_eq!(coordinator.completed_count(), 1);
        assert_eq!(coordinator.failed_count(), 1);
        assert_eq!(coordinator.failed[0].state, TransferState::Failed);
    }

    #[test]
    fn chunked_upload_tracker_advances_and_completes() {
        let mut tracker = super::ChunkedUploadTracker::new(77, 25 * 1024 * 1024, 10 * 1024 * 1024);
        assert_eq!(tracker.chunks_done, 0);
        assert_eq!(tracker.remaining(), 25 * 1024 * 1024);
        assert_eq!(tracker.next_chunk_size(), 10 * 1024 * 1024);
        assert!(!tracker.is_complete());

        // Chunk 1: 10 MiB
        tracker.advance(10 * 1024 * 1024);
        assert_eq!(tracker.chunks_done, 1);
        assert_eq!(tracker.acked_offset, 10 * 1024 * 1024);
        assert_eq!(tracker.next_chunk_size(), 10 * 1024 * 1024);

        // Chunk 2: 10 MiB
        tracker.advance(10 * 1024 * 1024);
        assert_eq!(tracker.chunks_done, 2);
        assert_eq!(tracker.remaining(), 5 * 1024 * 1024);

        // Chunk 3: 5 MiB (tail)
        assert_eq!(tracker.next_chunk_size(), 5 * 1024 * 1024);
        tracker.advance(5 * 1024 * 1024);
        assert_eq!(tracker.chunks_done, 3);
        assert!(tracker.is_complete());
        assert_eq!(tracker.remaining(), 0);
    }

    #[test]
    fn chunked_upload_tracker_handles_zero_size() {
        let tracker = super::ChunkedUploadTracker::new(1, 0, 10 * 1024 * 1024);
        assert!(tracker.is_complete());
        assert_eq!(tracker.next_chunk_size(), 0);
    }

    #[test]
    fn evict_sync_id_removes_upload_side_tasks() {
        let mut coordinator = UploadCoordinator::default();
        coordinator.accept_batch(&[
            PlannedOperation::UploadFile {
                sync_id: SyncId::new(1),
                path: "docs/report.txt".to_owned(),
                remote_parent_folder_id: None,
                remote_name: "report.txt".to_owned(),
            },
            PlannedOperation::DeleteRemote {
                sync_id: SyncId::new(2),
                path: "docs/old.txt".to_owned(),
            },
        ]);
        assert!(coordinator.mark_completed("docs/report.txt"));

        coordinator.evict_sync_id(SyncId::new(1));

        assert!(coordinator.completed.is_empty());
        assert_eq!(coordinator.pending_remote_deletes.len(), 1);
        assert_eq!(
            coordinator.pending_remote_deletes[0].operation.sync_id(),
            SyncId::new(2)
        );
    }
}
