//! Bandwidth pacing via a simple token bucket.
//!
//! [`BandwidthPacer`] enforces an upper bound on outbound (or inbound) byte
//! throughput by blocking the calling thread with [`std::thread::sleep`] when
//! the configured budget for the current refill window would be exceeded.
//!
//! # Token-bucket algorithm
//!
//! The bucket holds up to `bytes_per_sec` tokens and refills continuously
//! at the same rate (1 byte per 1/rate seconds). On each [`BandwidthPacer::pace`] call:
//!
//! 1. Refill: `tokens += (now − last_refill) × limit`, clamped to `limit`
//!    (the maximum burst size equals one second's worth of bandwidth).
//! 2. If `tokens ≥ bytes_to_send`: deduct and return immediately.
//! 3. Otherwise: compute `deficit = bytes_to_send − tokens`, zero the
//!    bucket, and sleep for `deficit / limit` seconds to let the bucket
//!    accumulate the shortfall. After the sleep, `last_refill` is
//!    advanced to `Instant::now()` and `tokens` stays at zero, because
//!    the deficit was consumed exactly.
//!
//! # Sleep precision
//!
//! The sleep uses [`std::thread::sleep`], whose granularity is bounded
//! by the OS scheduler (~1 ms on modern Linux). Small bursts below that
//! granularity round up; over many calls, throughput converges on the
//! configured limit to within scheduler jitter.
//!
//! The critical section is kept short: the sleep happens **outside** the
//! mutex so concurrent callers are not serialised on wall time.
//!
//! # Runtime adjustment
//!
//! [`BandwidthPacer::set_limit`] swaps the limit atomically and resets
//! the bucket and `last_refill` so the new rate takes effect cleanly
//! from the next [`pace`](BandwidthPacer::pace) call — no carry-over
//! burst from the previous rate.
//!
//! # Usage
//!
//! The pacer is intentionally synchronous: it is intended to be called
//! from transfer worker threads that own the I/O loop. A limit of
//! [`None`] disables pacing entirely and [`BandwidthPacer::pace`] becomes
//! a cheap no-op. A limit of `Some(0)` is also treated as unlimited to
//! avoid divide-by-zero.

// **PLATFORM:** all
// **GATING:** none (portable).

use parking_lot::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// Internal mutable state guarded by a [`Mutex`].
struct PacerState {
    /// Current limit in bytes per second; `None` means unlimited.
    limit: Option<u64>,
    /// Tokens (bytes) available in the current window.
    tokens: u64,
    /// Last refill instant.
    last_refill: Instant,
}

/// Token-bucket bandwidth pacer.
///
/// The bucket holds up to `bytes_per_sec` tokens and refills continuously at
/// the same rate. A call to [`BandwidthPacer::pace`] consumes tokens; if the
/// request exceeds what is available, the thread sleeps just long enough for
/// the bucket to refill the required amount.
pub struct BandwidthPacer {
    state: Mutex<PacerState>,
}

impl std::fmt::Debug for BandwidthPacer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't touch the mutex in `Debug`: it may be held by a hot-path
        // pacer call and blocking for formatting would be a surprise.
        f.debug_struct("BandwidthPacer")
            .field("state", &"<locked>")
            .finish()
    }
}

impl BandwidthPacer {
    /// Create a new pacer with the given limit in bytes per second.
    ///
    /// Pass [`None`] for unlimited (pacing disabled).
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_resilience::BandwidthPacer;
    /// // Cap uploads at 1 MB/s.
    /// let pacer = BandwidthPacer::new(Some(1024 * 1024));
    /// assert_eq!(pacer.limit(), Some(1024 * 1024));
    ///
    /// // Unlimited.
    /// let unlim = BandwidthPacer::new(None);
    /// assert_eq!(unlim.limit(), None);
    /// ```
    pub fn new(bytes_per_sec: Option<u64>) -> Self {
        Self {
            state: Mutex::new(PacerState {
                limit: bytes_per_sec,
                tokens: bytes_per_sec.unwrap_or(0),
                last_refill: Instant::now(),
            }),
        }
    }

    /// Update the bandwidth limit at runtime.
    ///
    /// Passing [`None`] disables pacing. The current bucket is reset so that
    /// the new limit takes effect cleanly from the next [`pace`](Self::pace)
    /// call.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_resilience::BandwidthPacer;
    /// let pacer = BandwidthPacer::new(Some(10_000));
    /// pacer.set_limit(None);
    /// assert_eq!(pacer.limit(), None);
    /// ```
    pub fn set_limit(&self, new: Option<u64>) {
        let mut st = self.state.lock();
        st.limit = new;
        st.tokens = new.unwrap_or(0);
        st.last_refill = Instant::now();
    }

    /// Return the currently configured limit.
    pub fn limit(&self) -> Option<u64> {
        self.state.lock().limit
    }

    /// Pace a transfer of `bytes_to_send` bytes.
    ///
    /// If a limit is configured and the bucket has insufficient tokens, the
    /// calling thread sleeps until enough tokens have accumulated, then
    /// deducts `bytes_to_send` from the bucket. Sleep precision is bounded by
    /// the OS scheduler (typically ~1 ms on modern Linux).
    pub fn pace(&self, bytes_to_send: u64) {
        if bytes_to_send == 0 {
            return;
        }

        // Compute a sleep duration while holding the lock briefly, release,
        // then sleep outside the critical section.
        let sleep_for = {
            let mut st = self.state.lock();
            let Some(limit) = st.limit else {
                return;
            };
            if limit == 0 {
                // Treat zero as unlimited to avoid divide-by-zero.
                return;
            }

            // Refill based on elapsed time.
            let now = Instant::now();
            let elapsed = now.saturating_duration_since(st.last_refill);
            let refill = (elapsed.as_secs_f64() * limit as f64).floor() as u64;
            if refill > 0 {
                st.tokens = st.tokens.saturating_add(refill).min(limit);
                st.last_refill = now;
            }

            if st.tokens >= bytes_to_send {
                st.tokens -= bytes_to_send;
                Duration::ZERO
            } else {
                let deficit = bytes_to_send - st.tokens;
                // Seconds required to accumulate the deficit.
                let secs = deficit as f64 / limit as f64;
                // Consume what we have; remainder will be consumed after sleep.
                st.tokens = 0;
                // Advance last_refill by the whole-second portion we are about
                // to wait so post-sleep accounting stays accurate.
                Duration::from_secs_f64(secs)
            }
        };

        if !sleep_for.is_zero() {
            thread::sleep(sleep_for);
            // After sleeping, account for the remainder: last_refill advances
            // and tokens stay at zero (we consumed the deficit exactly).
            let mut st = self.state.lock();
            st.last_refill = Instant::now();
        }
    }

    /// Compute how long the caller should wait before consuming `bytes`
    /// from the bucket, and atomically reserve the budget.
    ///
    /// Unlike [`Self::pace`], this does not actually sleep — it returns the
    /// [`Duration`] the caller should sleep (typically by handing it to an
    /// async runtime via `tokio::time::sleep`). A return value of
    /// [`Duration::ZERO`] means the request fits entirely within the current
    /// bucket and the caller can proceed immediately.
    ///
    /// Returns [`Duration::ZERO`] when the pacer is configured with
    /// [`None`] (unlimited) or a zero limit.
    ///
    /// Bead: pcloud-rs-6mx — `BandwidthLimiter::acquire(bytes) -> Duration`.
    pub fn acquire(&self, bytes: u64) -> Duration {
        if bytes == 0 {
            return Duration::ZERO;
        }
        let mut st = self.state.lock();
        let Some(limit) = st.limit else {
            return Duration::ZERO;
        };
        if limit == 0 {
            return Duration::ZERO;
        }

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(st.last_refill);
        let refill = (elapsed.as_secs_f64() * limit as f64).floor() as u64;
        if refill > 0 {
            st.tokens = st.tokens.saturating_add(refill).min(limit);
            st.last_refill = now;
        }

        if st.tokens >= bytes {
            st.tokens -= bytes;
            Duration::ZERO
        } else {
            let deficit = bytes - st.tokens;
            let secs = deficit as f64 / limit as f64;
            st.tokens = 0;
            // Advance last_refill optimistically so back-to-back callers
            // do not double-count the same sleep window.
            st.last_refill = now + Duration::from_secs_f64(secs);
            Duration::from_secs_f64(secs)
        }
    }

    /// Reserve budget for `bytes` and, if a wait is required, block the
    /// current thread until the budget is available.
    ///
    /// Equivalent to `thread::sleep(self.acquire(bytes))`. Use this in
    /// synchronous byte loops (HTTP download `read()` loops, upload
    /// `write()` loops). Use [`Self::acquire`] when the caller is async and
    /// wants to hand the returned [`Duration`] to an async sleep.
    ///
    /// Bead: pcloud-rs-6mx — `BandwidthLimiter::acquire_blocking(bytes)`.
    pub fn acquire_blocking(&self, bytes: u64) {
        let wait = self.acquire(bytes);
        if !wait.is_zero() {
            thread::sleep(wait);
        }
    }
}

impl Default for BandwidthPacer {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacer_unlimited_has_no_delay() {
        let pacer = BandwidthPacer::new(None);
        let start = Instant::now();
        for _ in 0..10_000 {
            pacer.pace(1_000_000);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(100),
            "unlimited pacer should not block; took {elapsed:?}"
        );
    }

    #[test]
    fn pacer_limit_enforces_throughput_approximately() {
        // Limit: 100 KB/s. Drain initial bucket first so subsequent sends
        // actually engage pacing, then measure throughput over a 500-iter
        // burst of 100 bytes = 50 KB.
        let limit: u64 = 100 * 1024;
        let pacer = BandwidthPacer::new(Some(limit));
        // Drain the starting tokens.
        pacer.pace(limit);

        let chunk: u64 = 100;
        let iters: u64 = 500;

        let start = Instant::now();
        for _ in 0..iters {
            pacer.pace(chunk);
        }
        let elapsed = start.elapsed().as_secs_f64();
        let total_bytes = (chunk * iters) as f64;
        let observed_bps = total_bytes / elapsed.max(1e-6);

        // ±30% tolerance around the configured limit.
        let upper = limit as f64 * 1.30;
        let lower = limit as f64 * 0.70;
        assert!(
            observed_bps <= upper,
            "observed {observed_bps:.0} B/s exceeds upper bound {upper:.0}"
        );
        assert!(
            observed_bps >= lower,
            "observed {observed_bps:.0} B/s below lower bound {lower:.0}"
        );
        assert!(
            elapsed < 1.0,
            "test should run in under 1s of wall time; took {elapsed}s"
        );
    }

    #[test]
    fn bandwidth_limiter_none_is_unlimited() {
        // Bead pcloud-rs-6mx: `acquire` on an unlimited pacer must return
        // Duration::ZERO for any request, no matter how large.
        let pacer = BandwidthPacer::new(None);
        assert_eq!(pacer.acquire(0), Duration::ZERO);
        assert_eq!(pacer.acquire(1), Duration::ZERO);
        assert_eq!(pacer.acquire(u64::MAX / 2), Duration::ZERO);

        let start = Instant::now();
        pacer.acquire_blocking(10_000_000_000);
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn bandwidth_limiter_throttles_to_configured_rate() {
        // Bead pcloud-rs-6mx: with a 100 KB/s limit, requesting 1 MB from
        // an empty bucket must return a sleep duration that matches the
        // deficit / rate ratio within tight tolerance, WITHOUT any real
        // sleeping (mock-time style: inspect `acquire`, not `pace`).
        let limit: u64 = 100 * 1024; // 100 KB/s
        let pacer = BandwidthPacer::new(Some(limit));

        // Drain the initial bucket (one full second worth of tokens).
        let first = pacer.acquire(limit);
        assert_eq!(
            first,
            Duration::ZERO,
            "first request should fit in the initial burst"
        );

        // Immediately request another 1 MB — must require ~10 s of wait.
        let request: u64 = 1_024 * 1_024; // 1 MB
        let wait = pacer.acquire(request);
        let expected_secs = request as f64 / limit as f64; // ~10.24 s
        let observed = wait.as_secs_f64();

        // ±5 % tolerance around the analytic expected wait.
        let upper = expected_secs * 1.05;
        let lower = expected_secs * 0.95;
        assert!(
            observed <= upper && observed >= lower,
            "observed wait {observed:.3}s not within ±5% of expected {expected_secs:.3}s"
        );

        // A request of zero bytes must never block and must not drain tokens.
        assert_eq!(pacer.acquire(0), Duration::ZERO);
    }

    #[test]
    fn acquire_blocking_respects_unlimited() {
        let pacer = BandwidthPacer::new(None);
        let start = Instant::now();
        pacer.acquire_blocking(1_000_000);
        assert!(start.elapsed() < Duration::from_millis(20));
    }

    #[test]
    fn set_limit_changes_behavior() {
        let pacer = BandwidthPacer::new(Some(1));
        pacer.set_limit(None);
        let start = Instant::now();
        pacer.pace(1_000_000_000);
        assert!(start.elapsed() < Duration::from_millis(50));
        assert_eq!(pacer.limit(), None);
    }
}
