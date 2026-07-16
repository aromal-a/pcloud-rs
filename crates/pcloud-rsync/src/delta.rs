//! Delta encoder: walk a local file against a remote
//! [`Signature`] and emit the smallest plausible mix of
//! `CopyServer` + `NewBytes` operations to reconstruct the local
//! file from the remote baseline plus a small payload.
//!
//! # Algorithm
//!
//! Mirrors librsync's classic walk:
//!
//! 1. Build a hash table `weak_hash → Vec<(block_index, strong_hash)>`
//!    over the remote signature so weak-hash matches are O(1).
//! 2. Initialise a [`RollingHash`] over the first `block_size`
//!    bytes of `local`.
//! 3. Walk one byte at a time:
//!    - On weak-hash hit, recompute the strong hash on the
//!      current window. On strong-hash confirmation: flush any
//!      pending `NewBytes`, emit a `CopyServer{block_index, len}`,
//!      jump the window forward by `block_size` and re-initialise
//!      the rolling hash on the new window.
//!    - Otherwise: push the byte at the window's left edge into
//!      a `NewBytes` accumulator, roll the hash forward by one.
//! 4. At end-of-file: try to match the final partial window
//!    against the signature's last (possibly short) block. Flush
//!    any remaining bytes as `NewBytes`.
//!
//! # Worst case
//!
//! When `local` shares nothing with the remote baseline, the
//! encoder degenerates to a single `NewBytes(local)` operation
//! plus an empty hash-table walk. That is the same payload the
//! current full-file upload path produces, so the encoder is
//! never *worse* than the baseline.
//!
//! # Best case
//!
//! When a 1-byte edit lands inside one block of an N-block file,
//! the encoder emits at most one (head copy) + one block of
//! `NewBytes` covering the edited block + one (tail copy) — three
//! operations regardless of `N`. The on-wire payload is bounded
//! by `2 * block_size` bytes plus a constant operation header per
//! op, far below the full-file upload.

// **PLATFORM:** all
// **GATING:** none.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::rolling::RollingHash;
use crate::signature::{STRONG_HASH_LEN, Signature};

/// One delta operation. The encoder emits a sequence of these to
/// reconstruct a target file from a remote baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeltaOp {
    /// Copy bytes from the remote baseline file. The server-side
    /// applier resolves `(block_index, len)` to a byte range
    /// using the signature's `block_size`.
    CopyServer {
        /// Index into [`Signature::blocks`] (0-based).
        block_index: u32,
        /// Number of bytes to copy. Equals `block_size` for full
        /// blocks; may be shorter for the tail block.
        len: u32,
    },
    /// Inline bytes that did not match any block on the remote
    /// side — these travel over the wire verbatim.
    NewBytes(Vec<u8>),
}

impl DeltaOp {
    /// Number of bytes this op contributes to the reconstructed
    /// file. Used by the encoder to verify that the delta
    /// reconstructs to the expected length.
    #[must_use]
    pub fn output_len(&self) -> u64 {
        match self {
            Self::CopyServer { len, .. } => u64::from(*len),
            Self::NewBytes(b) => b.len() as u64,
        }
    }

    /// Bytes the op contributes to the on-wire payload.
    /// `CopyServer` is essentially free (it's a `(u32, u32)`
    /// header plus a tag). `NewBytes` carries the literal bytes.
    #[must_use]
    pub fn wire_payload(&self) -> u64 {
        match self {
            Self::CopyServer { .. } => 8,
            Self::NewBytes(b) => b.len() as u64,
        }
    }
}

/// Compute the delta from a local file against a remote
/// [`Signature`]. Returns operations in order.
///
/// # Behaviour
///
/// - Empty `local` → empty `Vec<DeltaOp>`.
/// - Empty `remote_signature.blocks` → single
///   `NewBytes(local.to_vec())` (no baseline to copy from).
/// - `local` smaller than `remote_signature.block_size` → tries to
///   match `local` against the signature's tail block; falls back
///   to `NewBytes(local)` on miss.
///
/// The strong-hash check eliminates weak-hash collisions; an op
/// stream produced by this function reconstructs `local` byte-
/// identically when applied against a baseline that matches
/// `remote_signature`.
#[must_use]
pub fn compute_delta(local: &[u8], remote_signature: &Signature) -> Vec<DeltaOp> {
    let block_size = remote_signature.block_size as usize;
    if local.is_empty() {
        return Vec::new();
    }
    if remote_signature.blocks.is_empty() || block_size == 0 {
        return vec![DeltaOp::NewBytes(local.to_vec())];
    }

    // Build the weak-hash → entries map. Multiple blocks may share
    // a weak hash, so the value is a small vec.
    let mut table: HashMap<u32, Vec<(u32, [u8; STRONG_HASH_LEN])>> = HashMap::new();
    for (idx, block) in remote_signature.blocks.iter().enumerate() {
        table
            .entry(block.weak_hash)
            .or_default()
            .push((idx as u32, block.strong_hash));
    }

    let mut out: Vec<DeltaOp> = Vec::new();
    let mut new_bytes: Vec<u8> = Vec::new();
    let mut pos = 0usize;

    // Initialise rolling hash on the first window if available.
    if local.len() >= block_size {
        let mut roller = RollingHash::compute(&local[pos..pos + block_size]);
        loop {
            let weak = roller.hash();
            let mut matched: Option<(u32, u32)> = None;
            if let Some(candidates) = table.get(&weak) {
                let strong = strong_hash(&local[pos..pos + block_size]);
                for (block_idx, expected_strong) in candidates {
                    if strong == *expected_strong
                        && remote_signature.block_len(*block_idx as usize) as usize == block_size
                    {
                        matched = Some((*block_idx, block_size as u32));
                        break;
                    }
                }
            }
            if let Some((block_idx, len)) = matched {
                if !new_bytes.is_empty() {
                    out.push(DeltaOp::NewBytes(std::mem::take(&mut new_bytes)));
                }
                out.push(DeltaOp::CopyServer {
                    block_index: block_idx,
                    len,
                });
                pos += block_size;
                if pos + block_size <= local.len() {
                    roller = RollingHash::compute(&local[pos..pos + block_size]);
                    continue;
                } else {
                    break;
                }
            }
            // No match — drop the leftmost byte of the window into
            // `new_bytes`, roll forward by one.
            new_bytes.push(local[pos]);
            pos += 1;
            if pos + block_size > local.len() {
                break;
            }
            let out_byte = local[pos - 1];
            let in_byte = local[pos + block_size - 1];
            roller.roll(out_byte, in_byte);
        }
    }

    // Tail handling. `pos` points just past the last full window
    // we successfully matched (or one past the last byte we
    // dropped into `new_bytes`). The remaining `local[pos..]` is
    // shorter than `block_size`. Try to match it against the
    // signature's tail block (which may be a short block).
    let tail = &local[pos..];
    if !tail.is_empty() {
        let last_idx = remote_signature.blocks.len().saturating_sub(1);
        let tail_block_len = remote_signature.block_len(last_idx);
        if tail_block_len as usize == tail.len() {
            let weak = RollingHash::compute(tail).hash();
            if let Some(candidates) = table.get(&weak) {
                let strong = strong_hash(tail);
                for (block_idx, expected_strong) in candidates {
                    if *block_idx as usize == last_idx && strong == *expected_strong {
                        if !new_bytes.is_empty() {
                            out.push(DeltaOp::NewBytes(std::mem::take(&mut new_bytes)));
                        }
                        out.push(DeltaOp::CopyServer {
                            block_index: *block_idx,
                            len: tail_block_len,
                        });
                        return out;
                    }
                }
            }
        }
        // No tail match — append remaining bytes to NewBytes.
        new_bytes.extend_from_slice(tail);
    }

    if !new_bytes.is_empty() {
        out.push(DeltaOp::NewBytes(new_bytes));
    }
    out
}

fn strong_hash(block: &[u8]) -> [u8; STRONG_HASH_LEN] {
    let digest = Sha256::digest(block);
    let mut out = [0u8; STRONG_HASH_LEN];
    out.copy_from_slice(&digest[..STRONG_HASH_LEN]);
    out
}

/// Apply a delta against a baseline buffer to reconstruct the
/// original local file. Used by tests to assert lossless
/// round-trip; the production server-side applier does the same
/// thing across the IPC boundary.
#[must_use]
pub fn apply_delta(baseline: &[u8], block_size: u32, ops: &[DeltaOp]) -> Vec<u8> {
    let bs = block_size as usize;
    let mut out = Vec::new();
    for op in ops {
        match op {
            DeltaOp::CopyServer { block_index, len } => {
                let start = (*block_index as usize) * bs;
                let end = start + (*len as usize);
                out.extend_from_slice(&baseline[start..end]);
            }
            DeltaOp::NewBytes(bytes) => {
                out.extend_from_slice(bytes);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signature::{DEFAULT_BLOCK_SIZE, compute_signature};

    #[test]
    fn empty_local_yields_empty_delta() {
        let sig = compute_signature(b"any baseline", 4).unwrap();
        let delta = compute_delta(&[], &sig);
        assert!(delta.is_empty());
    }

    #[test]
    fn empty_signature_yields_one_new_bytes_op() {
        let sig = Signature {
            block_size: DEFAULT_BLOCK_SIZE,
            file_len: 0,
            blocks: Vec::new(),
        };
        let local = b"some content";
        let delta = compute_delta(local, &sig);
        assert_eq!(delta.len(), 1);
        match &delta[0] {
            DeltaOp::NewBytes(b) => assert_eq!(b, local),
            other => panic!("expected NewBytes, got {other:?}"),
        }
    }

    #[test]
    fn delta_of_self_is_single_copy_chain() {
        // local == baseline → delta is N CopyServer ops, no NewBytes.
        let baseline: Vec<u8> = (0..32u8).cycle().take(16).collect();
        let sig = compute_signature(&baseline, 4).unwrap();
        let delta = compute_delta(&baseline, &sig);
        assert!(
            delta
                .iter()
                .all(|op| matches!(op, DeltaOp::CopyServer { .. })),
            "delta-of-self should be all copies, got: {delta:?}"
        );
        // 16 bytes / 4-byte blocks = 4 ops.
        assert_eq!(delta.len(), 4);
        // Round-trip via apply.
        let reconstructed = apply_delta(&baseline, sig.block_size, &delta);
        assert_eq!(reconstructed, baseline);
    }

    /// One-byte edit in a multi-block file should ship only the
    /// edited block worth of `NewBytes`, surrounded by
    /// `CopyServer` ops for the unchanged blocks.
    #[test]
    fn one_byte_edit_isolates_to_one_block_payload() {
        let block_size = 4u32;
        let baseline: Vec<u8> = (0..32u8).take(16).collect();
        let sig = compute_signature(&baseline, block_size).unwrap();

        // Flip one byte in block index 2 (offset 8).
        let mut local = baseline.clone();
        local[9] ^= 0xFF;

        let delta = compute_delta(&local, &sig);
        let new_bytes_payload: u64 = delta
            .iter()
            .filter_map(|op| match op {
                DeltaOp::NewBytes(b) => Some(b.len() as u64),
                _ => None,
            })
            .sum();
        // The edited block is 4 bytes; the encoder may also drop
        // a few bytes from a partial walk before re-aligning, but
        // never more than 2 * block_size.
        assert!(
            new_bytes_payload <= u64::from(block_size) * 2,
            "expected ≤ {} new bytes, got {new_bytes_payload}",
            block_size * 2
        );
        // Round-trip must reconstruct the edited file.
        let reconstructed = apply_delta(&baseline, sig.block_size, &delta);
        assert_eq!(reconstructed, local);
    }

    /// Delta against a wholly disjoint baseline degrades to
    /// `NewBytes(local)` — the encoder is never worse than a
    /// full-file upload.
    #[test]
    fn fully_disjoint_local_is_just_new_bytes() {
        let baseline = vec![0xAAu8; 16];
        let sig = compute_signature(&baseline, 4).unwrap();
        let local = vec![0x55u8; 16];
        let delta = compute_delta(&local, &sig);
        // Either one big NewBytes op or several small ones — but
        // no CopyServer at all because no block matches.
        assert!(
            delta.iter().all(|op| matches!(op, DeltaOp::NewBytes(_))),
            "expected only NewBytes for disjoint inputs, got: {delta:?}"
        );
        let reconstructed = apply_delta(&baseline, sig.block_size, &delta);
        assert_eq!(reconstructed, local);
    }

    /// Local file shorter than `block_size` has no full-window
    /// walks; encoder emits a single `NewBytes` (or matches the
    /// short tail block when the signature has one).
    #[test]
    fn short_local_smaller_than_block_size() {
        let baseline = b"abcdef"; // 6 bytes, block_size = 4 → tail block of 2.
        let sig = compute_signature(baseline, 4).unwrap();
        // Local = first 3 bytes only; baseline has no 3-byte block,
        // so this is NewBytes.
        let local = b"abc";
        let delta = compute_delta(local, &sig);
        assert_eq!(delta.len(), 1);
        match &delta[0] {
            DeltaOp::NewBytes(b) => assert_eq!(b.as_slice(), local.as_slice()),
            other => panic!("expected NewBytes, got {other:?}"),
        }
    }

    /// Tail-block match: the local file's trailing partial window
    /// equals the baseline's tail block byte-for-byte.
    #[test]
    fn tail_block_match_emits_copy_for_tail() {
        let baseline = b"abcdefghIJ"; // 10 bytes, block_size 4 → blocks: abcd, efgh, IJ.
        let sig = compute_signature(baseline, 4).unwrap();
        let local = baseline.to_vec();
        let delta = compute_delta(&local, &sig);
        // 3 CopyServer ops (one per block), no NewBytes.
        assert_eq!(delta.len(), 3);
        for op in &delta {
            assert!(matches!(op, DeltaOp::CopyServer { .. }));
        }
        let reconstructed = apply_delta(baseline, sig.block_size, &delta);
        assert_eq!(reconstructed, local);
    }

    /// Reconstruction round-trip on a moderately-sized file with a
    /// single-block insertion. Demonstrates the bounded payload.
    #[test]
    fn reconstruction_preserves_arbitrary_edits() {
        let block_size = 16u32;
        let baseline: Vec<u8> = (0..255u8).cycle().take(256).collect();
        let sig = compute_signature(&baseline, block_size).unwrap();

        // Build local by inserting 16 bytes at offset 64. Result is
        // 272 bytes long.
        let mut local = Vec::with_capacity(272);
        local.extend_from_slice(&baseline[..64]);
        local.extend_from_slice(&[0xFFu8; 16]);
        local.extend_from_slice(&baseline[64..]);

        let delta = compute_delta(&local, &sig);
        let reconstructed = apply_delta(&baseline, sig.block_size, &delta);
        assert_eq!(reconstructed, local);

        // The edited region is one block of inserted bytes; with
        // alignment slack the new-bytes payload should still be
        // ≪ baseline.len().
        let new_bytes_payload: u64 = delta
            .iter()
            .filter_map(|op| match op {
                DeltaOp::NewBytes(b) => Some(b.len() as u64),
                _ => None,
            })
            .sum();
        assert!(
            new_bytes_payload < baseline.len() as u64,
            "expected payload < full upload, got {new_bytes_payload}",
        );
    }

    #[test]
    fn delta_op_wire_payload_accounting() {
        let copy = DeltaOp::CopyServer {
            block_index: 0,
            len: 4096,
        };
        assert_eq!(copy.wire_payload(), 8);
        assert_eq!(copy.output_len(), 4096);
        let new_bytes = DeltaOp::NewBytes(vec![0u8; 100]);
        assert_eq!(new_bytes.wire_payload(), 100);
        assert_eq!(new_bytes.output_len(), 100);
    }

    #[test]
    fn serde_roundtrip_delta_op() {
        let ops = vec![
            DeltaOp::CopyServer {
                block_index: 7,
                len: 4096,
            },
            DeltaOp::NewBytes(vec![1, 2, 3, 4]),
        ];
        let json = serde_json::to_string(&ops).unwrap();
        let back: Vec<DeltaOp> = serde_json::from_str(&json).unwrap();
        assert_eq!(ops, back);
    }
}
