// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use crate::sync::PlannedOperation;

/// Lifecycle state of an individual transfer task as it moves through
/// the upload/download pipeline.
///
/// # Normal progression
///
/// `Planned -> Preparing -> Streaming -> Verifying -> Committing ->
/// Completed`. On a transient error the task transitions to `Retrying`
/// and then re-enters `Preparing` once re-armed. On a terminal or
/// operator-gated failure it transitions to `Failed` and stays there
/// until evicted.
///
/// # Serde invariant
///
/// Roundtrips losslessly through `serde_json` using the variant name
/// as tag.
///
/// # Example
///
/// ```
/// use pcloud_model::transfer::TransferState;
/// let s = TransferState::Streaming;
/// let j = serde_json::to_string(&s).unwrap();
/// let back: TransferState = serde_json::from_str(&j).unwrap();
/// assert_eq!(s, back);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferState {
    /// Task has been created but no work has started yet.
    Planned,
    /// Pre-transfer work (hashing, size checks, session setup) is
    /// running.
    Preparing,
    /// Bytes are actively flowing over the network.
    Streaming,
    /// Transferred content is being verified (hash/size) against
    /// expectations.
    Verifying,
    /// Server-side finalization (save/commit) is in progress.
    Committing,
    /// A transient failure occurred and the task is scheduled for
    /// another attempt.
    Retrying,
    /// A terminal or non-retried failure stopped the task.
    Failed,
    /// The task finished successfully.
    Completed,
}

/// A single planned transfer together with its current state and the
/// most recent error (if any).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferTask {
    /// The sync engine operation this task carries out.
    pub operation: PlannedOperation,
    /// Current lifecycle state of the task.
    pub state: TransferState,
    /// Human-readable message for the last observed error, if any.
    pub last_error: Option<String>,
}

impl TransferTask {
    /// Construct a new `TransferTask` in the [`TransferState::Planned`] state
    /// with no recorded error.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    /// use pcloud_model::transfer::{TransferState, TransferTask};
    ///
    /// let op = PlannedOperation::DeleteLocal {
    ///     sync_id: SyncId::new(1),
    ///     path: "x".into(),
    /// };
    /// let task = TransferTask::planned(op);
    /// assert_eq!(task.state, TransferState::Planned);
    /// assert!(task.last_error.is_none());
    /// ```
    #[must_use]
    pub fn planned(operation: PlannedOperation) -> Self {
        Self {
            operation,
            state: TransferState::Planned,
            last_error: None,
        }
    }
}

/// Decision returned by the failure classifier indicating how a failed
/// transfer should be handled.
///
/// Produced by the engine's recovery classifier from a
/// [`crate::sync::PlannedOperation`] and an observed failure; the
/// scheduler and transfer coordinators consult this disposition to
/// decide the next state transition for the task (see
/// [`TransferState`]).
///
/// # Serde invariant
///
/// Roundtrips losslessly through `serde_json` using the variant name
/// as tag.
///
/// # Example
///
/// ```
/// use pcloud_model::transfer::FailureDisposition;
///
/// fn is_retryable(d: &FailureDisposition) -> bool {
///     matches!(d, FailureDisposition::RetryNow | FailureDisposition::RetryLater)
/// }
/// assert!(is_retryable(&FailureDisposition::RetryLater));
/// assert!(!is_retryable(&FailureDisposition::Terminal));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureDisposition {
    /// Retry the transfer immediately (typically after a transient
    /// network hiccup that has already resolved).
    RetryNow,
    /// Retry the transfer after a back-off delay applied by the
    /// scheduler's retry policy.
    RetryLater,
    /// The failure requires human action (credentials, quota,
    /// conflict, checksum mismatch) before retry.
    ManualIntervention,
    /// The failure is terminal and the task must not be retried
    /// automatically. The scheduler drops the task; restoring progress
    /// requires external repair (e.g. fixing the offending path or
    /// re-adding the sync root).
    Terminal,
}

/// Output of recovery classification for a failed transfer: the offending
/// operation, the chosen disposition, and a human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryDecision {
    /// The operation whose failure is being classified.
    pub operation: PlannedOperation,
    /// The chosen disposition for how to proceed.
    pub disposition: FailureDisposition,
    /// Human-readable explanation for the chosen disposition.
    pub reason: String,
}
