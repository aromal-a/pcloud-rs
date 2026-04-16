//! Writeback service: stages writes into the journal, tracks completed
//! flushes, and enforces a flush threshold before handing entries to
//! the upload pipeline. Consumed by `FilesystemShell::flush_writeback`
//! and the mount runtime's fsync/flush callbacks.
//!
//! Portable; no platform gating.

use serde::{Deserialize, Serialize};

use crate::journal::{JournalEntry, WritebackJournal};
use pcloud_cache::staging::StagingCache;

/// Writeback service: stages writes, tracks successful flushes, and caches
/// the threshold at which a flush should be triggered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritebackService {
    /// Number of staged bytes above which a caller should invoke
    /// [`flush`](Self::flush). The service does not auto-flush; this is
    /// advisory state consumed by the runtime.
    pub flush_threshold_bytes: usize,
    /// In-memory staging area for write payloads.
    pub staging: StagingCache,
    /// Monotonically increasing counter of successfully drained writes.
    pub completed_writes: usize,
}

impl Default for WritebackService {
    fn default() -> Self {
        Self {
            flush_threshold_bytes: 4 * 1024 * 1024,
            staging: StagingCache::default(),
            completed_writes: 0,
        }
    }
}

impl WritebackService {
    /// Stage a write into the staging area and append a matching entry to
    /// `journal`. The write is considered durable only after the journal
    /// has been persisted by the runtime.
    pub fn stage_write(
        &mut self,
        journal: &mut WritebackJournal,
        path: impl Into<String>,
        bytes: Vec<u8>,
    ) {
        let path = path.into();
        let byte_len = bytes.len();
        self.staging.stage(path.clone(), bytes);
        journal.append(JournalEntry {
            path,
            operation: "write".to_owned(),
            bytes: byte_len,
        });
    }

    /// Drain up to `max_entries` journal entries, evicting their staged
    /// payloads from the staging area and returning the drained entries
    /// so the caller can hand them to the upload pipeline.
    pub fn flush(
        &mut self,
        journal: &mut WritebackJournal,
        max_entries: usize,
    ) -> Vec<JournalEntry> {
        let drained = journal.drain(max_entries);
        for entry in &drained {
            self.staging.files.remove(&entry.path);
            self.staging
                .open_order
                .retain(|candidate| candidate != &entry.path);
        }
        self.completed_writes += drained.len();
        drained
    }

    /// Number of distinct files currently held in the staging area.
    #[must_use]
    pub fn staged_file_count(&self) -> usize {
        self.staging.staged_count()
    }
}

#[cfg(test)]
mod tests {
    use crate::journal::WritebackJournal;

    use super::WritebackService;

    #[test]
    fn stage_write_populates_staging_and_journal() {
        let mut service = WritebackService::default();
        let mut journal = WritebackJournal::default();

        service.stage_write(&mut journal, "docs/report.txt", b"hello".to_vec());

        assert_eq!(service.staged_file_count(), 1);
        assert_eq!(journal.pending_count(), 1);
    }

    #[test]
    fn flush_drains_journal_and_clears_staged_buffers() {
        let mut service = WritebackService::default();
        let mut journal = WritebackJournal::default();
        service.stage_write(&mut journal, "docs/report.txt", b"hello".to_vec());

        let drained = service.flush(&mut journal, 10);

        assert_eq!(drained.len(), 1);
        assert_eq!(service.staged_file_count(), 0);
        assert_eq!(journal.pending_count(), 0);
        assert_eq!(service.completed_writes, 1);
    }
}
