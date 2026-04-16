//! Recovery classifier that maps observed transfer failures to a
//! [`pcloud_model::transfer::RecoveryDecision`].
//!
//! # Failure taxonomy: retryable vs terminal
//!
//! The engine distinguishes three tiers, enforced by this module and
//! exposed through [`pcloud_model::transfer::FailureDisposition`]:
//!
//! * **Retryable (auto):** `RecoveryFailure::RetryableNetworkError`
//!   with `RecoveryManager::automatic_repair_enabled` set to `true` —
//!   becomes `FailureDisposition::RetryLater` and is re-armed by the
//!   scheduler after a back-off.
//! * **Retryable (operator):** `RecoveryFailure::ChecksumMismatch`,
//!   `RecoveryFailure::PermissionDenied`, and network errors with
//!   automatic repair disabled — become
//!   `FailureDisposition::ManualIntervention`. The task is parked
//!   until a human confirms a retry.
//! * **Terminal:** `RecoveryFailure::InvalidPath` — becomes
//!   `FailureDisposition::Terminal`. The task is dropped and the only
//!   way to make progress is repairing the underlying path (rename,
//!   re-add the sync root, etc.).
//!
//! The classifier is a pure function of (operation, failure); it does
//! not consult history, exponential back-off, or the store. Back-off
//! sequencing lives in the scheduler/transfer coordinators.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use pcloud_model::{
    sync::PlannedOperation,
    transfer::{FailureDisposition, RecoveryDecision},
};

/// Classifies transfer failures and produces [`RecoveryDecision`]s
/// (retry, manual intervention, or terminal) for the transfer loop.
///
/// Stateless aside from the [`Self::automatic_repair_enabled`] flag;
/// safe to clone and share across sync roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryManager {
    /// When `true`, transient network failures are retried automatically
    /// (disposition [`FailureDisposition::RetryLater`]). When `false`,
    /// they are escalated to [`FailureDisposition::ManualIntervention`]
    /// so the operator can review before the daemon re-tries. All other
    /// failure kinds are unaffected by this flag — checksum mismatches
    /// always require manual intervention and invalid-path failures are
    /// always terminal.
    pub automatic_repair_enabled: bool,
}

impl Default for RecoveryManager {
    fn default() -> Self {
        Self {
            automatic_repair_enabled: true,
        }
    }
}

/// Observed failure mode for a planned operation, used as input to
/// [`RecoveryManager::classify_failure`].
///
/// See the module-level docs for the retryable-vs-terminal taxonomy
/// mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryFailure {
    /// Transient network failure (timeout, reset, DNS, 5xx).
    ///
    /// **Taxonomy:** retryable. Maps to
    /// [`FailureDisposition::RetryLater`] when automatic repair is on,
    /// otherwise [`FailureDisposition::ManualIntervention`].
    RetryableNetworkError,
    /// Uploaded/downloaded bytes do not match the expected checksum.
    ///
    /// **Taxonomy:** retryable-but-suspicious. Always maps to
    /// [`FailureDisposition::ManualIntervention`] because a checksum
    /// mismatch signals either a server-side corruption or a local
    /// race; the daemon refuses to silently overwrite either side
    /// without operator review.
    ChecksumMismatch,
    /// Path is invalid (empty, absolute, contains `.`/`..`, or fails
    /// platform validation) or otherwise unsafe to act on.
    ///
    /// **Taxonomy:** terminal. Maps to
    /// [`FailureDisposition::Terminal`]. Retrying will deterministically
    /// fail in the same way; the only fix is to repair the path or
    /// evict the offending sync candidate.
    InvalidPath,
    /// Filesystem or API-level permission denied (ACL, read-only mount,
    /// share permission mask too restrictive).
    ///
    /// **Taxonomy:** retryable-only-after-repair. Maps to
    /// [`FailureDisposition::ManualIntervention`]; the daemon will not
    /// spin retrying a call that is being refused by the OS or the
    /// server authz layer.
    PermissionDenied,
}

impl RecoveryManager {
    /// Classify `failure` for `operation` into a [`RecoveryDecision`]
    /// with a disposition and a human-readable reason string.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::recovery::{RecoveryFailure, RecoveryManager};
    /// use pcloud_model::ids::SyncId;
    /// use pcloud_model::sync::PlannedOperation;
    /// use pcloud_model::transfer::FailureDisposition;
    ///
    /// let mgr = RecoveryManager::default();
    /// let op = PlannedOperation::DownloadFile {
    ///     sync_id: SyncId::new(1),
    ///     path: "a".into(),
    ///     remote_file_id: None,
    /// };
    /// let decision = mgr.classify_failure(&op, RecoveryFailure::InvalidPath);
    /// assert_eq!(decision.disposition, FailureDisposition::Terminal);
    /// ```
    #[must_use]
    pub fn classify_failure(
        &self,
        operation: &PlannedOperation,
        failure: RecoveryFailure,
    ) -> RecoveryDecision {
        let disposition = match failure {
            RecoveryFailure::RetryableNetworkError if self.automatic_repair_enabled => {
                FailureDisposition::RetryLater
            }
            RecoveryFailure::RetryableNetworkError => FailureDisposition::ManualIntervention,
            RecoveryFailure::ChecksumMismatch => FailureDisposition::ManualIntervention,
            RecoveryFailure::InvalidPath => FailureDisposition::Terminal,
            RecoveryFailure::PermissionDenied => FailureDisposition::ManualIntervention,
        };

        RecoveryDecision {
            operation: operation.clone(),
            disposition,
            reason: match failure {
                RecoveryFailure::RetryableNetworkError => {
                    "transient network failure should be retried".to_owned()
                }
                RecoveryFailure::ChecksumMismatch => {
                    "checksum mismatch requires integrity review".to_owned()
                }
                RecoveryFailure::InvalidPath => {
                    "invalid path is terminal until state is repaired".to_owned()
                }
                RecoveryFailure::PermissionDenied => {
                    "permission denied requires operator intervention".to_owned()
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use pcloud_model::{ids::SyncId, sync::PlannedOperation, transfer::FailureDisposition};

    use super::{RecoveryFailure, RecoveryManager};

    fn upload() -> PlannedOperation {
        PlannedOperation::UploadFile {
            sync_id: SyncId::new(1),
            path: "docs/report.txt".to_owned(),
            remote_parent_folder_id: None,
            remote_name: "report.txt".to_owned(),
        }
    }

    #[test]
    fn retryable_failures_become_retry_later_when_auto_repair_enabled() {
        let manager = RecoveryManager::default();
        let decision = manager.classify_failure(&upload(), RecoveryFailure::RetryableNetworkError);

        assert_eq!(decision.disposition, FailureDisposition::RetryLater);
    }

    #[test]
    fn checksum_mismatch_requires_manual_intervention() {
        let manager = RecoveryManager::default();
        let decision = manager.classify_failure(&upload(), RecoveryFailure::ChecksumMismatch);

        assert_eq!(decision.disposition, FailureDisposition::ManualIntervention);
    }
}
