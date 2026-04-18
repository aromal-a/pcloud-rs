//! Writeback journal: ordered, crash-recoverable record of pending
//! filesystem mutations waiting for upload. Consumed by `writeback` to
//! drain entries into remote upload calls and by recovery logic after
//! daemon restart.
//!
//! Portable; no platform gating.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;

/// Error returned by [`WritebackJournal::append`] when the journal is at
/// capacity. Callers MUST apply back-pressure (block the writer, flush
/// pending entries, or fail the mutation) — the journal never silently
/// evicts in-flight work because that is a data-loss path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalError {
    /// Journal is full: `pending_count >= max_pending_operations`. The
    /// caller must drain or fail before appending again.
    Full {
        /// Current number of pending operations.
        pending: usize,
        /// Configured capacity (inclusive upper bound).
        capacity: usize,
    },
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JournalError::Full { pending, capacity } => write!(
                f,
                "writeback journal is full ({pending}/{capacity}); flush before appending"
            ),
        }
    }
}

impl std::error::Error for JournalError {}

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
    /// Append `entry` to the back of the queue.
    ///
    /// Returns [`JournalError::Full`] when the journal is already at
    /// capacity. The journal **never** silently evicts in-flight work —
    /// that would be a direct data-loss path for writeback operations
    /// waiting on an upload. Callers must apply back-pressure (block the
    /// writer, trigger a flush, or surface an error to the caller) when
    /// this variant is returned.
    ///
    /// # Errors
    ///
    /// Returns `JournalError::Full` if `pending_count() >= max_pending_operations`.
    pub fn append(&mut self, entry: JournalEntry) -> Result<(), JournalError> {
        if self.pending.len() >= self.max_pending_operations {
            return Err(JournalError::Full {
                pending: self.pending.len(),
                capacity: self.max_pending_operations,
            });
        }
        self.pending.push_back(entry);
        Ok(())
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
    use super::{JournalEntry, JournalError, WritebackJournal};

    #[test]
    fn appends_and_drains_journal_entries() {
        let mut journal = WritebackJournal::default();
        journal
            .append(JournalEntry {
                path: "docs/report.txt".to_owned(),
                operation: "write".to_owned(),
                bytes: 5,
            })
            .expect("append within capacity");

        assert_eq!(journal.pending_count(), 1);
        let drained = journal.drain(1);
        assert_eq!(drained.len(), 1);
        assert_eq!(journal.pending_count(), 0);
    }

    #[test]
    fn enqueue_rejects_when_full_not_evict_oldest() {
        // Regression: at capacity, `append` must return `JournalError::Full`
        // instead of silently evicting the oldest entry (which would be a
        // direct data-loss path for writeback work awaiting upload).
        let mut journal = WritebackJournal {
            max_pending_operations: 2,
            ..WritebackJournal::default()
        };
        journal
            .append(JournalEntry {
                path: "a.txt".to_owned(),
                operation: "write".to_owned(),
                bytes: 1,
            })
            .expect("first fits");
        journal
            .append(JournalEntry {
                path: "b.txt".to_owned(),
                operation: "write".to_owned(),
                bytes: 1,
            })
            .expect("second fits");

        let err = journal
            .append(JournalEntry {
                path: "c.txt".to_owned(),
                operation: "write".to_owned(),
                bytes: 1,
            })
            .expect_err("third must error, not evict");
        assert!(matches!(
            err,
            JournalError::Full {
                pending: 2,
                capacity: 2
            }
        ));

        // Oldest entry MUST still be present — no silent eviction.
        let drained = journal.drain(10);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].path, "a.txt");
        assert_eq!(drained[1].path, "b.txt");
    }
}
