//! Writeback journal: ordered, crash-recoverable record of pending
//! filesystem mutations waiting for upload. Consumed by `writeback` to
//! drain entries into remote upload calls and by recovery logic after
//! daemon restart.
//!
//! Portable; no platform gating.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// A single ordered record of a pending filesystem mutation awaiting
/// upload. Entries are persisted so they survive daemon restarts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Remote path that this operation targets, rooted at `/`.
    pub path: String,
    /// Human-readable operation tag (e.g. `"write"`, `"delete"`). Used for
    /// diagnostics; recovery logic treats the entry as opaque.
    pub operation: String,
    /// Number of bytes associated with the operation, or `0` for metadata-
    /// only mutations.
    pub bytes: usize,
}

/// Ordered writeback journal backed by a `VecDeque`. Bounded by
/// `max_pending_operations`; if the bound is exceeded, the oldest entry is
/// discarded to make room for the newer one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritebackJournal {
    /// Hard upper bound on the number of entries retained before the
    /// oldest is evicted.
    pub max_pending_operations: usize,
    /// FIFO queue of pending entries awaiting upload.
    pub pending: VecDeque<JournalEntry>,
}

impl Default for WritebackJournal {
    fn default() -> Self {
        Self {
            max_pending_operations: 4096,
            pending: VecDeque::new(),
        }
    }
}

impl WritebackJournal {
    /// Append `entry` to the back of the queue. If the journal is already
    /// at capacity the oldest entry is dropped first — callers that need
    /// durability must flush before appending near the bound.
    pub fn append(&mut self, entry: JournalEntry) {
        if self.pending.len() >= self.max_pending_operations {
            let _ = self.pending.pop_front();
        }
        self.pending.push_back(entry);
    }

    /// Number of entries currently queued for upload.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Remove up to `limit` entries from the front of the queue and return
    /// them in FIFO order. If fewer entries are available, returns only
    /// those that exist.
    pub fn drain(&mut self, limit: usize) -> Vec<JournalEntry> {
        let mut drained = Vec::new();
        for _ in 0..limit {
            let Some(entry) = self.pending.pop_front() else {
                break;
            };
            drained.push(entry);
        }
        drained
    }
}

#[cfg(test)]
mod tests {
    use super::{JournalEntry, WritebackJournal};

    #[test]
    fn appends_and_drains_journal_entries() {
        let mut journal = WritebackJournal::default();
        journal.append(JournalEntry {
            path: "docs/report.txt".to_owned(),
            operation: "write".to_owned(),
            bytes: 5,
        });

        assert_eq!(journal.pending_count(), 1);
        let drained = journal.drain(1);
        assert_eq!(drained.len(), 1);
        assert_eq!(journal.pending_count(), 0);
    }

    #[test]
    fn bounded_journal_evicts_oldest_entries() {
        let mut journal = WritebackJournal {
            max_pending_operations: 1,
            ..WritebackJournal::default()
        };
        journal.append(JournalEntry {
            path: "a.txt".to_owned(),
            operation: "write".to_owned(),
            bytes: 1,
        });
        journal.append(JournalEntry {
            path: "b.txt".to_owned(),
            operation: "write".to_owned(),
            bytes: 1,
        });

        let drained = journal.drain(10);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].path, "b.txt");
    }
}
