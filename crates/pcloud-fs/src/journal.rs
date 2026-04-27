//! Writeback journal: ordered, crash-recoverable record of pending
//! filesystem mutations waiting for upload. Consumed by `writeback` to
//! drain entries into remote upload calls and by recovery logic after
//! daemon restart.
//!
//! Portable; no platform gating.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;

/// Current on-disk schema version for [`WritebackJournal`].
///
/// Bumped whenever the serialized layout of `WritebackJournal` or any
/// of its embedded types changes in a non-additive way. Serialized
/// payloads carry this value in the `version` field; deserialization
/// rejects any value strictly greater than [`CURRENT_VERSION`] with
/// [`JournalError::VersionMismatch`] so a forward-incompatible journal
/// is never silently misinterpreted.
///
/// Backwards compatibility: payloads without a `version` field
/// deserialize as `version = 1` via `#[serde(default = "default_version")]`,
/// matching the original v1 layout (no `version` key).
pub const CURRENT_VERSION: u32 = 1;

/// Default `version` value applied when an on-disk record predates the
/// version field (legacy v1 records). Public for documentation only;
/// callers should rely on [`CURRENT_VERSION`].
fn default_version() -> u32 {
    1
}

/// Error returned by [`WritebackJournal::append`] when the journal is at
/// capacity, or by [`WritebackJournal::ensure_compatible_version`] when a
/// loaded payload is from a forward-incompatible schema. Callers MUST
/// apply back-pressure for `Full` (block the writer, flush pending
/// entries, or fail the mutation) — the journal never silently evicts
/// in-flight work because that is a data-loss path.
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
    /// Loaded journal payload declares a `version` greater than
    /// [`CURRENT_VERSION`]. The current binary cannot interpret it
    /// safely — refuse rather than guess. This is the forward-
    /// incompatibility guard added in audit-06 stream E (§11 MEDIUM):
    /// a downgrade-then-upgrade cycle must not silently lose state.
    VersionMismatch {
        /// Schema version found on disk.
        found: u32,
        /// Highest schema version this binary knows how to read.
        supported: u32,
    },
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JournalError::Full { pending, capacity } => write!(
                f,
                "writeback journal is full ({pending}/{capacity}); flush before appending"
            ),
            JournalError::VersionMismatch { found, supported } => write!(
                f,
                "writeback journal version {found} is newer than supported version {supported}; \
                 refusing to interpret"
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
///
/// The `version` field declares the on-disk schema; payloads that omit it
/// deserialize as v1 for backwards compatibility (see [`CURRENT_VERSION`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritebackJournal {
    /// On-disk schema version. Defaults to v1 on legacy payloads that
    /// lack the field. Migrate via
    /// [`WritebackJournal::ensure_compatible_version`] before consumption.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Hard upper bound on the number of entries retained before the
    /// oldest is evicted.
    pub max_pending_operations: usize,
    /// FIFO queue of pending entries awaiting upload.
    pub pending: VecDeque<JournalEntry>,
}

impl Default for WritebackJournal {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
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

    /// Verify that the on-disk version is readable by this binary. Returns
    /// [`JournalError::VersionMismatch`] when `self.version > CURRENT_VERSION`
    /// so a downgraded daemon cannot silently misinterpret a journal
    /// produced by a newer build. Pre-v1 payloads (where the `version`
    /// field was absent) deserialize as `version = 1` and pass this check
    /// unchanged.
    ///
    /// Migration discipline (audit-06 §11 MEDIUM):
    /// * **Add a field** → keep `version` unchanged; existing journals
    ///   round-trip via `#[serde(default)]` on the new field.
    /// * **Remove or repurpose a field** → bump [`CURRENT_VERSION`] and
    ///   add a one-shot migration in this function (rewriting the
    ///   in-memory representation before returning `Ok`).
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::VersionMismatch`] when the loaded
    /// `version` exceeds the version this binary supports.
    pub fn ensure_compatible_version(&mut self) -> Result<(), JournalError> {
        if self.version > CURRENT_VERSION {
            return Err(JournalError::VersionMismatch {
                found: self.version,
                supported: CURRENT_VERSION,
            });
        }
        // v1 → v1 is a no-op. Future migrations slot in here.
        if self.version == 0 {
            // Defensive: a literal `version: 0` cannot have come from any
            // released build (the field defaults to 1 and no schema has
            // ever written 0). Coerce to v1 rather than reject so a
            // hand-edited config does not brick the daemon.
            self.version = 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CURRENT_VERSION, JournalEntry, JournalError, WritebackJournal};

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

    /// audit-06 §11 MEDIUM: a legacy v1 payload (no `version` key) must
    /// deserialize cleanly and migrate to the current version without
    /// data loss. Going-forward payloads must round-trip the field.
    #[test]
    fn legacy_payload_without_version_migrates_to_v1() {
        // The pre-versioning shape: only `max_pending_operations` and
        // `pending`. This is what a daemon binary from before audit-06
        // would have written.
        let legacy = r#"{
            "max_pending_operations": 16,
            "pending": [
                { "path": "/a.txt", "operation": "write", "bytes": 4 }
            ]
        }"#;

        let mut journal: WritebackJournal =
            serde_json::from_str(legacy).expect("legacy payload deserializes");
        assert_eq!(
            journal.version, 1,
            "missing version field must default to v1"
        );
        assert_eq!(journal.pending_count(), 1);

        journal
            .ensure_compatible_version()
            .expect("v1 is compatible with current binary");
        assert_eq!(journal.version, 1);

        // Round-trip through the current schema preserves the version.
        let encoded = serde_json::to_string(&journal).expect("serialize");
        let reloaded: WritebackJournal =
            serde_json::from_str(&encoded).expect("reloaded deserializes");
        assert_eq!(reloaded.version, CURRENT_VERSION);
        assert_eq!(reloaded.pending_count(), 1);
    }

    /// A forward-incompatible journal (newer schema than the running
    /// binary supports) MUST be refused via `JournalError::VersionMismatch`
    /// — silent misinterpretation would risk data loss in a
    /// downgrade-then-upgrade cycle.
    #[test]
    fn forward_incompatible_version_is_rejected() {
        let future = serde_json::json!({
            "version": CURRENT_VERSION + 7,
            "max_pending_operations": 8,
            "pending": []
        });
        let mut journal: WritebackJournal =
            serde_json::from_value(future).expect("structurally valid future payload parses");

        let err = journal
            .ensure_compatible_version()
            .expect_err("future version must be rejected");
        assert!(matches!(
            err,
            JournalError::VersionMismatch {
                supported,
                ..
            } if supported == CURRENT_VERSION
        ));
    }

    /// A literal `version: 0` cannot have come from any released build
    /// (the default and the originally-shipped layout both round-trip as
    /// v1). Coerce to v1 rather than reject, so a hand-edited config
    /// does not brick the daemon.
    #[test]
    fn version_zero_is_coerced_to_v1() {
        let zero = serde_json::json!({
            "version": 0,
            "max_pending_operations": 4,
            "pending": []
        });
        let mut journal: WritebackJournal = serde_json::from_value(zero).expect("payload parses");
        assert_eq!(journal.version, 0);
        journal
            .ensure_compatible_version()
            .expect("v0 coerces to v1");
        assert_eq!(journal.version, 1);
    }
}
