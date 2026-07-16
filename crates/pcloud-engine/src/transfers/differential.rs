//! T2.1.c — plan-side differential-upload strategy.
//!
//! The engine consults this module when a sync candidate is queued for
//! upload to decide whether to pre-compute an rsync-style delta against
//! a known baseline `Signature` or fall through to a full-file
//! upload.
//!
//! # Scope (plan-only)
//!
//! As of T2.1.c the helper *prepares* a delta and returns it as part of
//! `UploadStrategy::Delta`; the actual byte-range upload via
//! `upload_writefromfile` is the next sub-step and is gated on upstream
//! API parity plus a live test box. No I/O happens here.
//!
//! # Threshold
//!
//! Files smaller than `threshold` bytes — or files for which we have no
//! baseline signature on hand — always return `UploadStrategy::Full`.
//! Below the threshold the rolling-hash + per-block strong-hash overhead
//! is heavier than the bandwidth saved by partial transfer.
//!
//! # Worst-case behaviour
//!
//! When `local` shares no blocks with the baseline, `compute_delta`
//! degrades to a single `NewBytes(local.to_vec())` op. The wrapper
//! therefore *still* returns `UploadStrategy::Delta`, but the planner
//! is free to inspect the ops and downgrade to a full upload if it
//! prefers; tests assert this disjoint-baseline behaviour explicitly.

// **PLATFORM:** all
// **GATING:** none (pure compute, no I/O).

use pcloud_rsync::{DeltaOp, Signature, compute_delta};

/// Decision returned by [`plan_upload`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadStrategy {
    /// Upload the whole file. The current behaviour: no rsync-style
    /// negotiation, the upload path streams `local_bytes` end-to-end.
    Full,
    /// Pre-computed differential payload. The planner stores this
    /// alongside the planned operation; the upload-execute path will
    /// later send `delta_ops` instead of the full file once
    /// `upload_writefromfile` byte-range semantics are wired.
    Delta {
        /// Ordered delta operations: a mix of `CopyServer` references
        /// to baseline blocks and `NewBytes` literals.
        delta_ops: Vec<DeltaOp>,
        /// Total reconstructed length of the baseline that produced
        /// `remote_signature`. Stored so the executor can sanity-check
        /// the server-side baseline still matches before applying.
        baseline_signature_size: u64,
    },
}

/// Decide which upload strategy applies to `local_bytes`.
///
/// - Returns [`UploadStrategy::Full`] if `local_bytes.len() < threshold`
///   (the differential path is not worth the hash overhead).
/// - Returns [`UploadStrategy::Full`] if `remote_signature` is `None`
///   (no baseline → nothing to delta against).
/// - Otherwise computes the delta with [`compute_delta`] and returns
///   [`UploadStrategy::Delta`].
///
/// This function performs no I/O and never blocks. It is suitable for
/// use inside the planner hot path.
#[must_use]
pub fn plan_upload(
    local_bytes: &[u8],
    remote_signature: Option<&Signature>,
    threshold: u64,
) -> UploadStrategy {
    let len_u64 = local_bytes.len() as u64;
    if len_u64 < threshold {
        return UploadStrategy::Full;
    }
    let Some(signature) = remote_signature else {
        return UploadStrategy::Full;
    };
    let delta_ops = compute_delta(local_bytes, signature);
    UploadStrategy::Delta {
        delta_ops,
        baseline_signature_size: signature.file_len,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_rsync::{DEFAULT_BLOCK_SIZE, compute_signature};

    /// Files strictly smaller than the threshold short-circuit to
    /// `Full`, even when a baseline signature is available.
    #[test]
    fn plan_upload_returns_full_for_small_files() {
        let baseline = vec![0u8; 1024];
        let sig = compute_signature(&baseline, DEFAULT_BLOCK_SIZE).unwrap();
        let local = vec![1u8; 1024];
        // Threshold = 4 KiB; local is 1 KiB.
        let strategy = plan_upload(&local, Some(&sig), 4 * 1024);
        assert_eq!(strategy, UploadStrategy::Full);
    }

    /// Without a baseline signature the planner has no choice but to
    /// upload the full file, even for large payloads.
    #[test]
    fn plan_upload_returns_full_when_no_baseline_signature() {
        // 8 KiB local, threshold 4 KiB — large enough to pass the
        // size gate, but no signature available.
        let local = vec![0xABu8; 8 * 1024];
        let strategy = plan_upload(&local, None, 4 * 1024);
        assert_eq!(strategy, UploadStrategy::Full);
    }

    /// With a baseline signature and a 1-byte edit, the planner emits
    /// a `Delta` whose total `NewBytes` payload is bounded by
    /// `2 * block_size` — mirroring the T2.1.b contract.
    #[test]
    fn plan_upload_returns_delta_when_signature_matches_mostly() {
        let block_size = 4u32;
        // Build an 8 KiB baseline so we comfortably clear the
        // 4 KiB threshold even after signature compression.
        let baseline: Vec<u8> = (0..=255u8).cycle().take(8 * 1024).collect();
        let sig = compute_signature(&baseline, block_size).unwrap();

        // Flip a single byte; the rest matches.
        let mut local = baseline.clone();
        local[1234] ^= 0xFF;

        let strategy = plan_upload(&local, Some(&sig), 4 * 1024);
        match strategy {
            UploadStrategy::Delta {
                delta_ops,
                baseline_signature_size,
            } => {
                assert_eq!(baseline_signature_size, baseline.len() as u64);
                let new_bytes_payload: u64 = delta_ops
                    .iter()
                    .filter_map(|op| match op {
                        DeltaOp::NewBytes(b) => Some(b.len() as u64),
                        _ => None,
                    })
                    .sum();
                assert!(
                    new_bytes_payload <= u64::from(block_size) * 2,
                    "expected NewBytes payload ≤ 2*block_size = {} bytes, \
                     got {new_bytes_payload}",
                    block_size * 2
                );
            }
            other => panic!("expected UploadStrategy::Delta, got {other:?}"),
        }
    }

    /// When the local file is wholly disjoint from the baseline, the
    /// planner still returns `Delta`, but the delta is effectively a
    /// single `NewBytes(local)` — semantically equivalent to a full
    /// upload. The executor is free to short-circuit this case.
    #[test]
    fn plan_upload_returns_full_when_local_disjoint_from_baseline() {
        let block_size = 16u32;
        // Threshold-clearing payload; baseline & local share no bytes.
        let baseline = vec![0xAAu8; 8 * 1024];
        let sig = compute_signature(&baseline, block_size).unwrap();
        let local = vec![0x55u8; 8 * 1024];

        let strategy = plan_upload(&local, Some(&sig), 4 * 1024);
        match strategy {
            UploadStrategy::Delta {
                delta_ops,
                baseline_signature_size,
            } => {
                assert_eq!(baseline_signature_size, baseline.len() as u64);
                // No CopyServer ops at all — every block of `local`
                // missed the table.
                assert!(
                    delta_ops
                        .iter()
                        .all(|op| matches!(op, DeltaOp::NewBytes(_))),
                    "expected only NewBytes ops for disjoint inputs, got: {delta_ops:?}"
                );
                let total_new_bytes: u64 = delta_ops
                    .iter()
                    .filter_map(|op| match op {
                        DeltaOp::NewBytes(b) => Some(b.len() as u64),
                        _ => None,
                    })
                    .sum();
                // A disjoint delta carries the entire local payload —
                // i.e. it is effectively a full upload, just wearing
                // a Delta wrapper. The executor may downgrade.
                assert_eq!(total_new_bytes, local.len() as u64);
            }
            other => panic!("expected UploadStrategy::Delta, got {other:?}"),
        }
    }
}
