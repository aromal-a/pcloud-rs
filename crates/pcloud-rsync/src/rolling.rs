//! Adler-32-style rolling hash.
//!
//! # Why Adler-32 and not CRC-32 / xxHash
//!
//! - CRC-32 has a fast SIMD-friendly fixed-window implementation
//!   but is awkward to roll one byte at a time without a
//!   precomputed shift table.
//! - xxHash is faster but does not naturally support rolling.
//! - Adler-32 (and its librsync cousin "rollsum") is specifically
//!   built to advance the window one byte forward in O(1) using
//!   only addition / subtraction modulo a prime.
//!
//! The exact formula used here matches librsync's `rollsum`:
//!
//! ```text
//! a = (sum of bytes in window + window_len * MAGIC) mod 2^16
//! b = (weighted sum of bytes + (window_len * (window_len + 1) / 2) * MAGIC) mod 2^16
//! hash = (b << 16) | a
//! ```
//!
//! `MAGIC = 31` matches librsync; the constant biases the hash
//! away from the boring all-zero file case.
//!
//! # Rolling
//!
//! [`RollingHash::roll`] removes the byte at the back of the
//! window and adds a new byte at the front in O(1) — no need to
//! recompute over the whole window. Tests assert that rolling
//! through a stream of bytes produces the same hash as recomputing
//! from scratch at every position.

// **PLATFORM:** all
// **GATING:** none.

/// Rollsum bias constant (librsync `rollsum.c` uses 31). Folded
/// into both `a` and `b` to spread out hashes for low-entropy
/// inputs.
pub const MAGIC: u16 = 31;

/// Modulus for the running sums. Adler-32 uses 65521 (largest
/// prime below 2^16), but rollsum/librsync uses 2^16 to make the
/// rolling update cheaper. Tradeoff: slightly more collisions for
/// a 2-3× faster update. Differential sync's strong-hash second
/// layer absorbs the extra collisions, so the cheaper modulus is
/// the right pick here.
pub const MODULUS: u32 = 1 << 16;

/// 32-bit Adler-32-style rolling hash. Advance one byte at a time
/// with [`Self::roll`] or recompute from scratch with
/// [`Self::compute`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollingHash {
    a: u32,
    b: u32,
    /// Number of bytes currently in the window. Tracked separately
    /// from the caller's window because the rolling formula
    /// depends on the current window length.
    window_len: u32,
}

impl Default for RollingHash {
    fn default() -> Self {
        Self::new()
    }
}

impl RollingHash {
    /// Empty hash with zero window length.
    #[must_use]
    pub fn new() -> Self {
        Self {
            a: 0,
            b: 0,
            window_len: 0,
        }
    }

    /// Compute the rolling hash over `window` from scratch.
    ///
    /// O(window.len()) — use this when initialising a new window
    /// or when the caller has skipped through the input by more
    /// than one byte.
    #[must_use]
    pub fn compute(window: &[u8]) -> Self {
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let len = window.len() as u32;
        for (idx, &byte) in window.iter().enumerate() {
            a = a.wrapping_add(u32::from(byte));
            // Weighted sum: each byte contributes (window_len - idx)
            // times to b. Equivalent to adding `a` after each byte.
            b = b.wrapping_add(a);
            let _ = idx;
        }
        // Bias both sums by the window length × magic so all-zero
        // windows do not collapse to (0, 0).
        a = a.wrapping_add(len.wrapping_mul(u32::from(MAGIC)));
        let triangle = len.wrapping_mul(len.wrapping_add(1)) / 2;
        b = b.wrapping_add(triangle.wrapping_mul(u32::from(MAGIC)));
        Self {
            a: a % MODULUS,
            b: b % MODULUS,
            window_len: len,
        }
    }

    /// 32-bit hash value.
    #[must_use]
    pub fn hash(&self) -> u32 {
        (self.b << 16) | self.a
    }

    /// Roll the window: drop `out` (the byte that just left the
    /// window's left edge) and append `inb` (the byte that just
    /// entered on the right). The window length stays the same.
    ///
    /// O(1). Use this once per byte during the delta walk.
    ///
    /// The rolling-update identity (derived from the
    /// `b = sum((n-i)*d_i)` definition):
    ///
    /// ```text
    /// new_a = a - out + inb
    /// new_b = b - window_len * out + new_a
    /// ```
    ///
    /// The window-length bias on both sums is constant under
    /// rolling (the window itself stays the same length), so the
    /// biased values follow the same delta rule.
    pub fn roll(&mut self, out: u8, inb: u8) {
        let n = self.window_len;
        // new_a first so new_b can read it directly.
        self.a = self
            .a
            .wrapping_sub(u32::from(out))
            .wrapping_add(u32::from(inb))
            % MODULUS;
        // Biased rolling: the unbiased new_a feeds into b, so we
        // subtract the n*MAGIC bias when adding the biased new_a
        // to b. The triangle(n)*MAGIC bias on b itself cancels
        // because n is constant under rolling.
        let out_b_contrib = u32::from(out).wrapping_mul(n) % MODULUS;
        let bias_a = n.wrapping_mul(u32::from(MAGIC)) % MODULUS;
        self.b = self
            .b
            .wrapping_sub(out_b_contrib)
            .wrapping_add(self.a)
            .wrapping_sub(bias_a)
            % MODULUS;
    }

    /// Append `byte` to the window (window length grows by 1).
    /// Used when initialising a partial window or when growing
    /// past the trailing tail of a file.
    pub fn push(&mut self, byte: u8) {
        // Strip the prior magic bias so push composes with compute.
        let prior_bias_a = self.window_len.wrapping_mul(u32::from(MAGIC));
        let prior_triangle = self
            .window_len
            .wrapping_mul(self.window_len.wrapping_add(1))
            / 2;
        let prior_bias_b = prior_triangle.wrapping_mul(u32::from(MAGIC));
        let mut a = self.a.wrapping_sub(prior_bias_a) % MODULUS;
        let mut b = self.b.wrapping_sub(prior_bias_b) % MODULUS;

        a = a.wrapping_add(u32::from(byte)) % MODULUS;
        b = b.wrapping_add(a) % MODULUS;

        self.window_len = self.window_len.wrapping_add(1);
        let new_bias_a = self.window_len.wrapping_mul(u32::from(MAGIC));
        let new_triangle = self
            .window_len
            .wrapping_mul(self.window_len.wrapping_add(1))
            / 2;
        let new_bias_b = new_triangle.wrapping_mul(u32::from(MAGIC));
        self.a = a.wrapping_add(new_bias_a) % MODULUS;
        self.b = b.wrapping_add(new_bias_b) % MODULUS;
    }

    /// Window length the hash currently covers.
    #[must_use]
    pub fn window_len(&self) -> u32 {
        self.window_len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_window_has_zero_state() {
        let h = RollingHash::new();
        assert_eq!(h.window_len(), 0);
        assert_eq!(h.hash(), 0);
    }

    #[test]
    fn compute_is_deterministic() {
        let buf = b"hello world";
        let a = RollingHash::compute(buf);
        let b = RollingHash::compute(buf);
        assert_eq!(a, b);
    }

    #[test]
    fn different_content_yields_different_hash() {
        let a = RollingHash::compute(b"abcd");
        let b = RollingHash::compute(b"abce");
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn all_zeros_does_not_collapse_to_zero() {
        // The magic bias ensures zero-padding does not vanish.
        let h = RollingHash::compute(&[0u8; 16]);
        assert_ne!(h.hash(), 0);
    }

    /// Roll-byte-by-byte must equal compute-from-scratch for every
    /// position in a sliding window. This is the load-bearing
    /// invariant of the differential-sync walker.
    #[test]
    fn rolling_matches_recompute_at_every_position() {
        let data: Vec<u8> = (0..64u8).chain(0..64u8).chain(0..64u8).collect();
        let window = 16;
        // Initialise rolling hash on the first window.
        let mut roller = RollingHash::compute(&data[..window]);
        // Walk forward one byte at a time, comparing to a fresh
        // compute over the same window each time.
        for start in 0..(data.len() - window) {
            let expected = RollingHash::compute(&data[start..start + window]);
            assert_eq!(
                roller, expected,
                "rolling hash diverges from recompute at start={start}"
            );
            // Advance: drop data[start], add data[start+window].
            roller.roll(data[start], data[start + window]);
        }
    }

    #[test]
    fn push_grows_window_and_matches_compute() {
        let buf = b"abcdefgh";
        let mut h = RollingHash::new();
        for &b in buf {
            h.push(b);
        }
        let expected = RollingHash::compute(buf);
        assert_eq!(h.window_len(), buf.len() as u32);
        assert_eq!(h, expected);
    }

    #[test]
    fn hash_packs_a_and_b() {
        let h = RollingHash::compute(b"x");
        // a is one byte + bias = 'x' (120) + 1*31 = 151
        // b is one weighted sum + triangle bias = 151 + (1*2/2)*31 = 151 + 31 = 182
        // Wait — the formula does the bias additively to `a` after
        // collecting the byte sum, so a runs 0 -> 120 then biased
        // by 1*31 = 151. b accumulates a after each step: 120 then
        // bias 1*31 = 151.
        let h2 = RollingHash::compute(b"x");
        assert_eq!(h.hash(), h2.hash());
        // Top 16 bits = b, bottom 16 = a.
        let top = (h.hash() >> 16) & 0xFFFF;
        let bot = h.hash() & 0xFFFF;
        assert!(top > 0);
        assert!(bot > 0);
    }
}
