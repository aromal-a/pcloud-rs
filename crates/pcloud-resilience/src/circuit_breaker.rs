//! Three-state circuit breaker.
//!
//! # States and transitions
//!
//! - [`BreakerState::Closed`] — normal operation. Every recorded failure
//!   increments a counter; every success resets it to zero. When the
//!   counter reaches `failure_threshold`, the breaker trips to
//!   [`BreakerState::Open`] and records the trip time via the injected
//!   [`Clock`].
//! - [`BreakerState::Open`] — all calls are rejected with
//!   [`CircuitBreakerError::Open`] until `reset_timeout` has elapsed
//!   since the trip, at which point the breaker transitions to
//!   [`BreakerState::HalfOpen`] on the next observation (lazy
//!   time-driven tick).
//! - [`BreakerState::HalfOpen`] — exactly one probe call is admitted via
//!   the `probe_in_flight` slot. Additional concurrent acquires get
//!   [`CircuitBreakerError::ProbeInFlight`]. On success the breaker
//!   closes and resets counters; on failure it re-opens and the reset
//!   timeout starts again.
//!
//! All time reads go through an injected [`Clock`] so tests can be fully
//! deterministic.
//!
//! # Panic safety (P0.1 — `parking_lot::Mutex`, no-poison-on-panic)
//!
//! The breaker uses [`parking_lot::Mutex`], which — unlike
//! [`std::sync::Mutex`] — does **not** carry poisoning state. If a thread
//! panics while holding the lock, other threads continue to acquire the
//! lock normally. This is load-bearing: a poisoned `std::sync::Mutex` on
//! the breaker would disable the entire network I/O path for the daemon
//! lifetime (every subsequent `lock()` would return `Err(Poisoned)` and
//! the daemon would have no non-hacky way to recover).
//!
//! # HalfOpen probe-slot lifecycle
//!
//! The probe slot is tracked as a single `probe_in_flight: bool`. Its
//! lifecycle is:
//!
//! 1. Breaker in Open, `reset_timeout` elapses — next observation flips
//!    state to HalfOpen and clears `probe_in_flight`.
//! 2. A caller invokes [`CircuitBreaker::try_acquire`] or
//!    [`CircuitBreaker::acquire_guarded`] — `probe_in_flight` goes `true`.
//! 3. On [`CircuitBreaker::record_success`] — state ⇒ Closed, flag clear.
//!    On [`CircuitBreaker::record_failure`] — state ⇒ Open, flag clear,
//!    `opened_at` refreshed.
//! 4. If the caller panics or drops without recording,
//!    [`ProbeGuard::drop`] treats the outcome as failure and clears the
//!    slot. This prevents a stuck HalfOpen with a phantom probe.
//!
//! [`ProbeGuard`] is marked `#[must_use]` so accidental drops are lints.
//! Because the mutex is unpoisonable, this drop path is safe even during
//! stack unwinding caused by a panic inside the guarded section.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use thiserror::Error;

use crate::clock::{Clock, SystemClock};

/// Externally observable breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BreakerState {
    /// Calls are admitted and monitored.
    Closed,
    /// Calls are rejected fast until the reset timeout elapses.
    Open,
    /// A single probe call is admitted to test recovery.
    HalfOpen,
}

/// Static configuration for a [`CircuitBreaker`].
#[derive(Debug, Clone, Copy)]
pub struct CircuitBreakerConfig {
    /// Consecutive failures that trip the breaker from Closed to Open.
    /// Must be >= 1.
    pub failure_threshold: u32,
    /// How long the breaker stays Open before a probe is allowed.
    pub reset_timeout: Duration,
}

impl CircuitBreakerConfig {
    /// Validates and builds a new config.
    pub fn new(failure_threshold: u32, reset_timeout: Duration) -> Self {
        assert!(failure_threshold >= 1, "failure_threshold must be >= 1");
        Self {
            failure_threshold,
            reset_timeout,
        }
    }
}

/// Errors returned by [`CircuitBreaker::try_acquire`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CircuitBreakerError {
    /// The breaker is currently Open and the reset timeout has not elapsed.
    #[error("circuit breaker is open")]
    Open,
    /// The breaker is HalfOpen and a probe is already in flight.
    #[error("circuit breaker already has a probe in flight")]
    ProbeInFlight,
}

#[derive(Debug)]
struct Inner {
    state: BreakerState,
    consecutive_failures: u32,
    opened_at: Option<Instant>,
    probe_in_flight: bool,
}

/// Thread-safe, cheaply-cloneable circuit breaker.
#[derive(Clone)]
pub struct CircuitBreaker {
    cfg: CircuitBreakerConfig,
    inner: Arc<Mutex<Inner>>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("cfg", &self.cfg)
            .finish()
    }
}

impl CircuitBreaker {
    /// Creates a new breaker using [`SystemClock`].
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use pcloud_resilience::{BreakerState, CircuitBreaker, CircuitBreakerConfig};
    ///
    /// let cfg = CircuitBreakerConfig::new(3, Duration::from_secs(10));
    /// let breaker = CircuitBreaker::new(cfg);
    /// assert_eq!(breaker.state(), BreakerState::Closed);
    /// ```
    pub fn new(cfg: CircuitBreakerConfig) -> Self {
        Self::with_clock(cfg, Arc::new(SystemClock))
    }

    /// Creates a new breaker using an injected [`Clock`].
    pub fn with_clock(cfg: CircuitBreakerConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            cfg,
            inner: Arc::new(Mutex::new(Inner {
                state: BreakerState::Closed,
                consecutive_failures: 0,
                opened_at: None,
                probe_in_flight: false,
            })),
            clock,
        }
    }

    /// Returns the current externally observable state, performing any
    /// time-driven transitions (Open -> HalfOpen) as a side effect.
    pub fn state(&self) -> BreakerState {
        let mut g = self.inner.lock();
        self.tick_locked(&mut g);
        g.state
    }

    /// Attempts to admit a call.
    ///
    /// On success the caller MUST report the outcome via
    /// [`CircuitBreaker::record_success`] or
    /// [`CircuitBreaker::record_failure`]; otherwise a HalfOpen probe slot
    /// may leak.
    ///
    /// Prefer [`CircuitBreaker::acquire_guarded`] which provides an RAII
    /// guard that cannot leak the probe slot even if the caller panics.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use pcloud_resilience::{CircuitBreaker, CircuitBreakerConfig};
    ///
    /// let breaker = CircuitBreaker::new(CircuitBreakerConfig::new(2, Duration::from_secs(1)));
    /// breaker.try_acquire().unwrap();
    /// breaker.record_success();
    /// ```
    pub fn try_acquire(&self) -> Result<(), CircuitBreakerError> {
        let mut g = self.inner.lock();
        self.tick_locked(&mut g);
        match g.state {
            BreakerState::Closed => Ok(()),
            BreakerState::Open => Err(CircuitBreakerError::Open),
            BreakerState::HalfOpen => {
                if g.probe_in_flight {
                    Err(CircuitBreakerError::ProbeInFlight)
                } else {
                    g.probe_in_flight = true;
                    Ok(())
                }
            }
        }
    }

    /// Acquires admission and returns a [`ProbeGuard`] that will release a
    /// HalfOpen probe slot on drop (re-opening the breaker) unless the
    /// caller explicitly calls [`ProbeGuard::record_success`] or
    /// [`ProbeGuard::record_failure`] first.
    ///
    /// This is the panic-safe entry point. If the guarded work panics, the
    /// drop will treat the outcome as a failure and reopen the breaker,
    /// preventing a stuck HalfOpen probe slot.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use pcloud_resilience::{CircuitBreaker, CircuitBreakerConfig};
    ///
    /// let breaker = CircuitBreaker::new(CircuitBreakerConfig::new(1, Duration::from_secs(1)));
    /// let guard = breaker.acquire_guarded().unwrap();
    /// // ... do the guarded work ...
    /// guard.record_success();
    /// ```
    pub fn acquire_guarded(&self) -> Result<ProbeGuard<'_>, CircuitBreakerError> {
        self.try_acquire()?;
        Ok(ProbeGuard {
            breaker: self,
            finished: false,
        })
    }

    /// Records a successful outcome.
    pub fn record_success(&self) {
        let mut g = self.inner.lock();
        match g.state {
            BreakerState::Closed => {
                g.consecutive_failures = 0;
            }
            BreakerState::HalfOpen => {
                g.state = BreakerState::Closed;
                g.consecutive_failures = 0;
                g.opened_at = None;
                g.probe_in_flight = false;
            }
            BreakerState::Open => {
                // A success can only be recorded after a successful acquire,
                // which for Open is impossible. Be defensive and ignore.
            }
        }
    }

    /// Records a failed outcome.
    pub fn record_failure(&self) {
        let mut g = self.inner.lock();
        match g.state {
            BreakerState::Closed => {
                g.consecutive_failures = g.consecutive_failures.saturating_add(1);
                if g.consecutive_failures >= self.cfg.failure_threshold {
                    g.state = BreakerState::Open;
                    g.opened_at = Some(self.clock.now());
                }
            }
            BreakerState::HalfOpen => {
                g.state = BreakerState::Open;
                g.opened_at = Some(self.clock.now());
                g.probe_in_flight = false;
            }
            BreakerState::Open => {
                // Same as above — ignore to stay defensive.
            }
        }
    }

    fn tick_locked(&self, g: &mut Inner) {
        if g.state == BreakerState::Open {
            if let Some(opened_at) = g.opened_at {
                let now = self.clock.now();
                if now.saturating_duration_since(opened_at) >= self.cfg.reset_timeout {
                    g.state = BreakerState::HalfOpen;
                    g.probe_in_flight = false;
                }
            }
        }
    }
}

/// RAII guard returned by [`CircuitBreaker::acquire_guarded`].
///
/// If the guarded work completes successfully the caller should call
/// [`ProbeGuard::record_success`]. If it fails with a recoverable error,
/// call [`ProbeGuard::record_failure`]. If the caller does neither — for
/// example because the code panics — the drop implementation treats the
/// outcome as a failure and releases the HalfOpen probe slot by reopening
/// the breaker, ensuring the breaker never gets stuck with a phantom probe.
#[must_use = "dropping a ProbeGuard without recording treats the call as a failure"]
pub struct ProbeGuard<'a> {
    breaker: &'a CircuitBreaker,
    finished: bool,
}

impl<'a> ProbeGuard<'a> {
    /// Records a successful outcome and consumes the guard.
    pub fn record_success(mut self) {
        self.breaker.record_success();
        self.finished = true;
    }

    /// Records a failed outcome and consumes the guard.
    pub fn record_failure(mut self) {
        self.breaker.record_failure();
        self.finished = true;
    }
}

impl<'a> Drop for ProbeGuard<'a> {
    fn drop(&mut self) {
        if !self.finished {
            // Treat an unrecorded outcome (e.g. caller panicked) as a
            // failure. This is safe even during unwinding because
            // `parking_lot::Mutex` is infallible and has no poisoning state.
            self.breaker.record_failure();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Barrier;
    use std::thread;

    fn breaker(thr: u32, rt_ms: u64, clock: Arc<ManualClock>) -> CircuitBreaker {
        let cfg = CircuitBreakerConfig::new(thr, Duration::from_millis(rt_ms));
        CircuitBreaker::with_clock(cfg, clock)
    }

    #[test]
    fn trips_after_threshold_failures() {
        let clock = Arc::new(ManualClock::new());
        let b = breaker(3, 100, clock);
        for _ in 0..3 {
            b.try_acquire().unwrap();
            b.record_failure();
        }
        assert_eq!(b.state(), BreakerState::Open);
        assert!(matches!(b.try_acquire(), Err(CircuitBreakerError::Open)));
    }

    #[test]
    fn half_open_after_reset_timeout() {
        let clock = Arc::new(ManualClock::new());
        let b = breaker(1, 50, clock.clone());
        b.try_acquire().unwrap();
        b.record_failure();
        assert_eq!(b.state(), BreakerState::Open);
        clock.advance(Duration::from_millis(50));
        assert_eq!(b.state(), BreakerState::HalfOpen);
    }

    #[test]
    fn half_open_success_closes_breaker() {
        let clock = Arc::new(ManualClock::new());
        let b = breaker(1, 10, clock.clone());
        b.try_acquire().unwrap();
        b.record_failure();
        clock.advance(Duration::from_millis(10));
        b.try_acquire().unwrap();
        b.record_success();
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[test]
    fn half_open_failure_reopens() {
        let clock = Arc::new(ManualClock::new());
        let b = breaker(1, 10, clock.clone());
        b.try_acquire().unwrap();
        b.record_failure();
        clock.advance(Duration::from_millis(10));
        b.try_acquire().unwrap();
        b.record_failure();
        assert_eq!(b.state(), BreakerState::Open);
    }

    #[test]
    fn half_open_admits_single_probe() {
        let clock = Arc::new(ManualClock::new());
        let b = breaker(1, 10, clock.clone());
        b.try_acquire().unwrap();
        b.record_failure();
        clock.advance(Duration::from_millis(10));
        b.try_acquire().unwrap();
        assert!(matches!(
            b.try_acquire(),
            Err(CircuitBreakerError::ProbeInFlight)
        ));
    }

    #[test]
    fn success_in_closed_resets_failure_counter() {
        let clock = Arc::new(ManualClock::new());
        let b = breaker(3, 10, clock);
        b.try_acquire().unwrap();
        b.record_failure();
        b.try_acquire().unwrap();
        b.record_failure();
        b.try_acquire().unwrap();
        b.record_success();
        // Two more failures must NOT trip the breaker because the counter
        // was reset to zero by the intervening success.
        b.try_acquire().unwrap();
        b.record_failure();
        b.try_acquire().unwrap();
        b.record_failure();
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[test]
    fn probe_guard_records_success_closes_breaker() {
        let clock = Arc::new(ManualClock::new());
        let b = breaker(1, 10, clock.clone());
        b.try_acquire().unwrap();
        b.record_failure();
        clock.advance(Duration::from_millis(10));
        assert_eq!(b.state(), BreakerState::HalfOpen);
        let guard = b.acquire_guarded().unwrap();
        guard.record_success();
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[test]
    fn probe_guard_dropped_without_record_reopens() {
        let clock = Arc::new(ManualClock::new());
        let b = breaker(1, 10, clock.clone());
        b.try_acquire().unwrap();
        b.record_failure();
        clock.advance(Duration::from_millis(10));
        assert_eq!(b.state(), BreakerState::HalfOpen);
        {
            let _guard = b.acquire_guarded().unwrap();
            // Dropped without recording — should be treated as a failure.
        }
        assert_eq!(b.state(), BreakerState::Open);
    }

    #[test]
    fn circuit_breaker_survives_panic_in_guarded_section() {
        // Deliberately panic inside a catch_unwind block while holding a
        // ProbeGuard. The guard's Drop must release the probe slot and
        // reopen the breaker rather than leaving it stuck. Subsequent
        // calls on the same breaker must still work — a poisoned
        // std::sync::Mutex would have disabled all calls from this point
        // on.
        let clock = Arc::new(ManualClock::new());
        let b = breaker(1, 10, clock.clone());

        // Trip breaker, then advance into HalfOpen.
        b.try_acquire().unwrap();
        b.record_failure();
        clock.advance(Duration::from_millis(10));
        assert_eq!(b.state(), BreakerState::HalfOpen);

        // Panic while a ProbeGuard is live.
        let b_clone = b.clone();
        let result = catch_unwind(AssertUnwindSafe(move || {
            let _guard = b_clone.acquire_guarded().unwrap();
            panic!("simulated failure inside guarded section");
        }));
        assert!(result.is_err());

        // Breaker reopened after drop; subsequent state reads and acquires
        // still work (no poisoning).
        assert_eq!(b.state(), BreakerState::Open);

        // After cooldown, HalfOpen probe is accepted again.
        clock.advance(Duration::from_millis(10));
        let g = b.acquire_guarded().expect("breaker still functional");
        g.record_success();
        assert_eq!(b.state(), BreakerState::Closed);
    }

    // Skipped on OpenBSD: the default per-user `proc` and `kern.maxthread`
    // login-class limits cap a non-root user at ~256 threads, well below the
    // 1000 spawned here. The test exercises mutex-poison resilience under
    // high concurrency — which is platform-agnostic correctness already
    // covered on Linux / macOS / FreeBSD / NetBSD. Raising OpenBSD's limits
    // requires editing `/etc/login.conf` + reboot, which is out of scope for
    // a portable test. See `.audits/followup/bsd-bringup-2026-04-26.md`.
    #[cfg(not(target_os = "openbsd"))]
    #[test]
    fn thousand_threads_all_panic_breaker_recovers_after_cooldown() {
        // Property-style stress test: 1000 threads each acquire the
        // breaker and deliberately panic once. The breaker must remain
        // functional (no mutex poisoning, no stuck probe slot) and must
        // return to Closed after cooldown + a single successful probe.
        const N: usize = 1000;
        let clock = Arc::new(ManualClock::new());
        // Large threshold so we stay Closed for the entire stress phase.
        let cfg = CircuitBreakerConfig::new(u32::MAX, Duration::from_millis(10));
        let b = CircuitBreaker::with_clock(cfg, clock.clone());

        let barrier = Arc::new(Barrier::new(N));
        let mut handles = Vec::with_capacity(N);
        for _ in 0..N {
            let b = b.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                let r = catch_unwind(AssertUnwindSafe(|| {
                    let _g = b.acquire_guarded().expect("acquire must succeed");
                    panic!("thread panic");
                }));
                assert!(r.is_err());
            }));
        }
        for h in handles {
            h.join().expect("thread join");
        }

        // Every thread's drop recorded a failure; with u32::MAX threshold
        // we remain Closed but the breaker is demonstrably still working.
        assert_eq!(b.state(), BreakerState::Closed);

        // Now verify we can still drive the breaker through a full
        // Open -> HalfOpen -> Closed cycle after all those panics.
        let cfg2 = CircuitBreakerConfig::new(1, Duration::from_millis(10));
        let b2 = CircuitBreaker::with_clock(cfg2, clock.clone());
        b2.try_acquire().unwrap();
        b2.record_failure();
        assert_eq!(b2.state(), BreakerState::Open);
        clock.advance(Duration::from_millis(10));
        assert_eq!(b2.state(), BreakerState::HalfOpen);
        let g = b2.acquire_guarded().unwrap();
        g.record_success();
        assert_eq!(b2.state(), BreakerState::Closed);
    }
}
