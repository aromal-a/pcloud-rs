//! Block-signature builder for differential sync.
//!
//! # On-wire shape
//!
//! A [`Signature`] is `(block_size, file_len, [BlockSignature])`.
//! Each entry carries a `weak_hash: u32` (rolling-hash value) and
//! a `strong_hash: [u8; 16]` (truncated SHA-256). The block index
//! is implicit (entries arrive in offset order) so the encoded
//! form is compact: 20 bytes per block plus a small header.
//!
//! # Why truncated SHA-256
//!
//! librsync uses MD4 historically; the workspace already pulls
//! `sha2` and a 16-byte truncated SHA-256 collision probability is
//! ~2^-64 — many orders of magnitude below the per-file
//! unrecoverable error rate of consumer SSDs. Cutting from 32 →
//! 16 bytes halves the on-wire signature size at no real safety
//! cost; full 32-byte hashes can be re-introduced behind a
//! profile feature flag if a deployment ever needs them.
//!
//! # Tail block
//!
//! The last block of a file may be shorter than `block_size`. The
//! signature builder still emits an entry for it, with the
//! shorter window length implicit from `(file_len, block_size,
//! block_index)`.

// **PLATFORM:** all
// **GATING:** none.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::rolling::RollingHash;

/// Default block size in bytes. Picked as a tradeoff: large
/// enough that the strong-hash cost is amortised; small enough
/// that a 1-byte edit to a multi-MiB file only drags one block of
/// new data over the wire.
pub const DEFAULT_BLOCK_SIZE: u32 = 4 * 1024;

/// Length of the truncated strong hash (bytes). 16 bytes ≈ 128
/// bits of collision resistance, which is comfortably above the
/// per-file error budget while halving the signature footprint.
pub const STRONG_HASH_LEN: usize = 16;

/// One block's signature entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockSignature {
    /// Rolling-hash value over the block bytes.
    pub weak_hash: u32,
    /// Truncated SHA-256 over the block bytes (high 16 bytes).
    pub strong_hash: [u8; STRONG_HASH_LEN],
}

/// Full signature of a baseline file: header + per-block entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// Block size used to chunk the input. Must match between the
    /// signature and the eventual delta-encoder run.
    pub block_size: u32,
    /// Total length of the source file in bytes. Lets the delta
    /// encoder know whether the last block was short.
    pub file_len: u64,
    /// One entry per block, in offset order. Length is
    /// `ceil(file_len / block_size)` (or zero for an empty file).
    pub blocks: Vec<BlockSignature>,
}

impl Signature {
    /// Number of blocks the signature describes.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Length of the block at `index`. The last block may be
    /// shorter than `block_size`.
    ///
    /// Returns `0` when `index >= block_count`.
    #[must_use]
    pub fn block_len(&self, index: usize) -> u32 {
        if index >= self.blocks.len() {
            return 0;
        }
        let count = self.blocks.len() as u64;
        if (index as u64) + 1 < count {
            return self.block_size;
        }
        let full_blocks = count.saturating_sub(1) * u64::from(self.block_size);
        let tail = self.file_len.saturating_sub(full_blocks);
        // Cap at u32 — block_size is u32, tail bytes <= block_size.
        tail.min(u64::from(self.block_size)) as u32
    }
}

/// Errors raised while building a signature.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SignatureError {
    /// `block_size = 0` is rejected — the rolling-hash window
    /// has no defined behaviour at zero.
    #[error("block_size must be > 0")]
    ZeroBlockSize,
}

/// Build a [`Signature`] over `data` with the given block size.
///
/// # Errors
///
/// See [`SignatureError`].
///
/// # Example
///
/// ```
/// use pcloud_rsync::signature::{compute_signature, DEFAULT_BLOCK_SIZE};
/// let data = vec![0xAA; 8 * 1024];
/// let sig = compute_signature(&data, DEFAULT_BLOCK_SIZE).unwrap();
/// assert_eq!(sig.block_count(), 2); // 8 KiB / 4 KiB
/// ```
pub fn compute_signature(data: &[u8], block_size: u32) -> Result<Signature, SignatureError> {
    if block_size == 0 {
        return Err(SignatureError::ZeroBlockSize);
    }
    let bs = block_size as usize;
    let mut blocks = Vec::with_capacity(data.len() / bs + 1);
    let mut offset = 0;
    while offset < data.len() {
        let end = (offset + bs).min(data.len());
        let block = &data[offset..end];
        let weak = RollingHash::compute(block).hash();
        let strong = strong_hash_bytes(block);
        blocks.push(BlockSignature {
            weak_hash: weak,
            strong_hash: strong,
        });
        offset = end;
    }
    Ok(Signature {
        block_size,
        file_len: data.len() as u64,
        blocks,
    })
}

/// SHA-256 the input and return the high 16 bytes.
fn strong_hash_bytes(block: &[u8]) -> [u8; STRONG_HASH_LEN] {
    let digest = Sha256::digest(block);
    let mut out = [0u8; STRONG_HASH_LEN];
    out.copy_from_slice(&digest[..STRONG_HASH_LEN]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_empty_signature() {
        let sig = compute_signature(&[], DEFAULT_BLOCK_SIZE).unwrap();
        assert_eq!(sig.block_count(), 0);
        assert_eq!(sig.file_len, 0);
    }

    #[test]
    fn block_size_zero_rejected() {
        let err = compute_signature(b"abc", 0).unwrap_err();
        assert_eq!(err, SignatureError::ZeroBlockSize);
    }

    #[test]
    fn full_block_aligned_input_count() {
        let data = vec![0xCDu8; 12 * 1024];
        let sig = compute_signature(&data, DEFAULT_BLOCK_SIZE).unwrap();
        // 12 KiB / 4 KiB = 3 blocks, all full size.
        assert_eq!(sig.block_count(), 3);
        for i in 0..3 {
            assert_eq!(sig.block_len(i), DEFAULT_BLOCK_SIZE);
        }
    }

    #[test]
    fn tail_block_short_length_reported() {
        let data = vec![0u8; 4 * 1024 + 100];
        let sig = compute_signature(&data, DEFAULT_BLOCK_SIZE).unwrap();
        assert_eq!(sig.block_count(), 2);
        assert_eq!(sig.block_len(0), DEFAULT_BLOCK_SIZE);
        assert_eq!(sig.block_len(1), 100);
    }

    #[test]
    fn block_len_out_of_range_is_zero() {
        let sig = compute_signature(b"xxxx", 4).unwrap();
        assert_eq!(sig.block_len(0), 4);
        assert_eq!(sig.block_len(1), 0);
        assert_eq!(sig.block_len(99), 0);
    }

    #[test]
    fn identical_inputs_produce_identical_signatures() {
        let a = compute_signature(b"hello world this is a longer string", 8).unwrap();
        let b = compute_signature(b"hello world this is a longer string", 8).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn one_byte_change_changes_only_one_block_strong_hash() {
        let mut data = vec![0u8; 16];
        // Fill with a non-zero pattern so the second hash differs.
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i * 7) as u8;
        }
        let original = compute_signature(&data, 4).unwrap();

        // Flip one byte in block index 2 (offset 8).
        data[9] ^= 0xFF;
        let edited = compute_signature(&data, 4).unwrap();

        assert_eq!(original.block_count(), edited.block_count());
        assert_eq!(original.blocks[0], edited.blocks[0]);
        assert_eq!(original.blocks[1], edited.blocks[1]);
        assert_ne!(original.blocks[2], edited.blocks[2]);
        assert_eq!(original.blocks[3], edited.blocks[3]);
    }

    #[test]
    fn weak_hash_matches_rolling_hash_compute() {
        let data = vec![0xABu8; 16];
        let sig = compute_signature(&data, 4).unwrap();
        for (idx, block) in data.chunks(4).enumerate() {
            let expected = RollingHash::compute(block).hash();
            assert_eq!(sig.blocks[idx].weak_hash, expected);
        }
    }

    #[test]
    fn strong_hash_truncates_sha256() {
        let block = b"some block";
        let full = Sha256::digest(block);
        let truncated = strong_hash_bytes(block);
        assert_eq!(&truncated[..], &full[..STRONG_HASH_LEN]);
    }

    #[test]
    fn serde_roundtrip() {
        let sig = compute_signature(b"abcdefgh", 4).unwrap();
        let json = serde_json::to_string(&sig).expect("serialise");
        let back: Signature = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(sig, back);
    }
}
