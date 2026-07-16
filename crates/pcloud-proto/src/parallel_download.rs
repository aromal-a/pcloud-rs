//! T2.2 — multi-range HTTP download planner.
//!
//! # What this module does (T2.2.a)
//!
//! Splits a large download into N roughly-equal byte ranges so a
//! caller can fan them out across N parallel HTTP `GET ... Range:`
//! requests. The actual HTTP fetcher is the next sub-step
//! (T2.2.b); the planner is pure compute and tested without
//! sockets.
//!
//! # Why a separate planner
//!
//! Range arithmetic is fiddly: ranges must be contiguous, start
//! at zero, sum to the total length, respect a minimum chunk size
//! (so a 4 KiB file does not fan out to 4 workers fetching 1 KiB
//! each — the per-request overhead would exceed the parallelism
//! gain). Pulling the math into its own function with exhaustive
//! tests means the eventual fetcher can focus on HTTP plumbing
//! rather than off-by-one bugs.
//!
//! # Algorithm
//!
//! 1. If `total <= min_chunk`, the whole file is one range.
//! 2. Otherwise pick `chunk_count = min(workers, total / min_chunk)`,
//!    floored to at least 1.
//! 3. `chunk_size = total / chunk_count`. The last chunk absorbs
//!    the remainder so every request is at least `chunk_size`
//!    bytes long and the sum is exactly `total`.
//!
//! Step 2 is the load-bearing one: it stops the planner from
//! emitting tiny ranges below `min_chunk` even when `workers >
//! total / min_chunk`. The default `min_chunk` is 256 KiB, large
//! enough to amortise typical per-request setup over the byte
//! count fetched.

// **PLATFORM:** all
// **GATING:** none (portable; no I/O).

use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};

/// Default minimum chunk size: 256 KiB. Below this the per-request
/// HTTP overhead (TLS handshake reuse, header roundtrip) makes
/// parallelism not worth it.
pub const DEFAULT_MIN_CHUNK_BYTES: u64 = 256 * 1024;

/// One byte range to fetch with `Range: bytes=offset-(offset+length-1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeRequest {
    /// Inclusive start offset in bytes.
    pub offset: u64,
    /// Length in bytes. Always > 0; the planner never emits empty
    /// ranges.
    pub length: u64,
}

impl RangeRequest {
    /// Inclusive end offset (`offset + length - 1`). Convenience
    /// for clients that need to format the `Range:` header
    /// directly.
    #[must_use]
    pub fn end_inclusive(&self) -> u64 {
        self.offset + self.length - 1
    }

    /// Format the value for the `Range:` HTTP header.
    #[must_use]
    pub fn header_value(&self) -> String {
        format!("bytes={}-{}", self.offset, self.end_inclusive())
    }
}

/// Plan a contiguous, ordered list of byte ranges that together
/// cover `[0, total)`. Returns at least one range when `total > 0`.
///
/// # Arguments
///
/// - `total` — total file size in bytes.
/// - `workers` — desired parallelism. The planner clamps so no
///   range is shorter than `min_chunk`.
/// - `min_chunk` — minimum bytes per range. Pass
///   [`DEFAULT_MIN_CHUNK_BYTES`] for the workspace default.
///
/// # Properties (asserted by tests)
///
/// - `result` is empty iff `total == 0`.
/// - Every range's length is `> 0`.
/// - Ranges are contiguous: `result[i].offset + result[i].length
///   == result[i+1].offset`.
/// - The first range starts at offset `0`.
/// - The last range ends at `total - 1`.
/// - The total length sums to `total`.
/// - At most `workers` ranges are emitted.
/// - Every range except the last is exactly `total / chunk_count`
///   bytes; the last absorbs the remainder.
#[must_use]
pub fn plan_ranges(total: u64, workers: NonZeroUsize, min_chunk: u64) -> Vec<RangeRequest> {
    if total == 0 {
        return Vec::new();
    }
    // Single-range fast path.
    let workers = workers.get() as u64;
    let min_chunk = min_chunk.max(1);
    if total <= min_chunk || workers == 1 {
        return vec![RangeRequest {
            offset: 0,
            length: total,
        }];
    }

    // Honour min_chunk: cap the chunk count at `total / min_chunk`
    // so no range is shorter than `min_chunk` (except possibly the
    // final remainder, which is *added* to the last chunk and
    // therefore at least `min_chunk` long).
    let max_chunks_by_min = total / min_chunk;
    let chunk_count = workers.min(max_chunks_by_min).max(1);
    if chunk_count == 1 {
        return vec![RangeRequest {
            offset: 0,
            length: total,
        }];
    }
    let chunk_size = total / chunk_count;
    let mut out = Vec::with_capacity(chunk_count as usize);
    let mut offset = 0u64;
    for i in 0..chunk_count {
        let length = if i + 1 == chunk_count {
            // Last chunk absorbs the modulo remainder so the sum
            // is exactly `total`.
            total - offset
        } else {
            chunk_size
        };
        out.push(RangeRequest { offset, length });
        offset += length;
    }
    debug_assert_eq!(offset, total);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nz(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap()
    }

    fn assert_covers(total: u64, ranges: &[RangeRequest]) {
        if total == 0 {
            assert!(ranges.is_empty());
            return;
        }
        assert!(!ranges.is_empty());
        assert_eq!(ranges[0].offset, 0, "must start at 0");
        let mut sum = 0u64;
        for window in ranges.windows(2) {
            assert_eq!(
                window[0].offset + window[0].length,
                window[1].offset,
                "ranges must be contiguous"
            );
            assert!(window[0].length > 0, "no zero-length ranges");
        }
        for r in ranges {
            sum += r.length;
        }
        assert_eq!(sum, total, "ranges must sum to total");
        let last = ranges.last().unwrap();
        assert_eq!(last.offset + last.length, total, "last range ends at total");
    }

    #[test]
    fn empty_total_yields_no_ranges() {
        let r = plan_ranges(0, nz(8), DEFAULT_MIN_CHUNK_BYTES);
        assert!(r.is_empty());
    }

    #[test]
    fn tiny_file_collapses_to_one_range() {
        // 100 bytes < 256 KiB min_chunk → single range regardless
        // of worker count.
        let r = plan_ranges(100, nz(8), DEFAULT_MIN_CHUNK_BYTES);
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0],
            RangeRequest {
                offset: 0,
                length: 100
            }
        );
    }

    #[test]
    fn one_worker_emits_one_range() {
        let r = plan_ranges(10 * 1024 * 1024, nz(1), 64 * 1024);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].length, 10 * 1024 * 1024);
    }

    #[test]
    fn min_chunk_caps_worker_count() {
        // 1 MiB total, min_chunk 1 MiB, 8 workers → only 1 chunk
        // is allowed (any more would violate the min-chunk cap).
        let r = plan_ranges(1024 * 1024, nz(8), 1024 * 1024);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].length, 1024 * 1024);
    }

    #[test]
    fn even_split_4_workers() {
        let total = 1024u64 * 1024 * 1024; // 1 GiB
        let r = plan_ranges(total, nz(4), DEFAULT_MIN_CHUNK_BYTES);
        assert_eq!(r.len(), 4);
        assert_covers(total, &r);
        // 1 GiB / 4 = 256 MiB exactly → all chunks identical.
        for chunk in &r {
            assert_eq!(chunk.length, 256 * 1024 * 1024);
        }
    }

    #[test]
    fn last_chunk_absorbs_remainder() {
        let total = 1000u64; // not divisible by 3
        let r = plan_ranges(total, nz(3), 100);
        assert_eq!(r.len(), 3);
        assert_covers(total, &r);
        // 1000 / 3 = 333; last chunk gets 334.
        assert_eq!(r[0].length, 333);
        assert_eq!(r[1].length, 333);
        assert_eq!(r[2].length, 334);
    }

    #[test]
    fn range_request_header_value_format() {
        let r = RangeRequest {
            offset: 1024,
            length: 4096,
        };
        assert_eq!(r.end_inclusive(), 5119);
        assert_eq!(r.header_value(), "bytes=1024-5119");
    }

    #[test]
    fn ranges_are_serializable() {
        let plan = plan_ranges(8000, nz(2), 1024);
        let json = serde_json::to_string(&plan).unwrap();
        let back: Vec<RangeRequest> = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
    }

    #[test]
    fn many_workers_min_chunk_clamps() {
        // 1 MiB total, min_chunk 256 KiB, 32 workers → 4 chunks
        // (1 MiB / 256 KiB).
        let total = 1024 * 1024u64;
        let r = plan_ranges(total, nz(32), 256 * 1024);
        assert!(r.len() <= 4);
        assert_covers(total, &r);
    }

    #[test]
    fn min_chunk_zero_clamped_to_one() {
        // Defensive: passing min_chunk = 0 must not panic with a
        // divide-by-zero. The planner clamps to 1.
        let r = plan_ranges(8, nz(4), 0);
        assert_eq!(r.len(), 4);
        assert_covers(8, &r);
    }

    /// Sweep: every (total, workers) combo within reasonable
    /// bounds satisfies the contract. Catches off-by-ones the
    /// targeted tests above might miss.
    #[test]
    fn sweep_property_check() {
        for total in [0u64, 1, 2, 7, 100, 1024, 256 * 1024, 1024 * 1024, 1_000_001] {
            for workers in [1usize, 2, 3, 4, 8, 16] {
                let plan = plan_ranges(total, nz(workers), 4096);
                assert_covers(total, &plan);
                assert!(plan.len() <= workers);
            }
        }
    }
}
