//! Token-bucket rate limiter.
//!
//! A [`TokenBucket`] enforces a per-endpoint rate limit with burst capacity.
//! The bucket refills continuously at `refill_rate` tokens per second up to
//! `capacity`. [`TokenBucket::try_acquire`] is a non-blocking check;
//! [`TokenBucket::acquire`] returns the [`Duration`] a caller must wait
//! before the request is allowed, so the caller can integrate with any async
//! runtime (including the optional `crate::timeout` helper).

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::clock::{Clock, SystemClock};

/// Configuration for a [`TokenBucket`].
#[derive(Debug, Clone, Copy)]
pub struct TokenBucketConfig {
    /// Maximum burst size (tokens).
    pub capacity: u32,
    /// Steady-state refill rate in tokens/second. Must be > 0.
    pub refill_rate_per_sec: f64,
}

impl TokenBucketConfig {
    /// Builds a new config, validating fields.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_resilience::TokenBucketConfig;
    /// let cfg = TokenBucketConfig::new(100, 50.0).unwrap();
    /// assert_eq!(cfg.capacity, 100);
    /// // Zero capacity is rejected.
    /// assert!(TokenBucketConfig::new(0, 1.0).is_err());
    /// ```
    pub fn new(capacity: u32, refill_rate_per_sec: f64) -> Result<Self, RateLimitError> {
        if capacity == 0 {
            return Err(RateLimitError::InvalidConfig("capacity must be > 0"));
        }
        if !(refill_rate_per_sec.is_finite() && refill_rate_per_sec > 0.0) {
            return Err(RateLimitError::InvalidConfig(
                "refill_rate_per_sec must be a positive finite number",
            ));
        }
        Ok(Self {
            capacity,
            refill_rate_per_sec,
        })
    }
}

/// Errors returned by rate-limiter operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RateLimitError {
    /// Configuration was invalid at construction time.
    #[error("invalid token-bucket config: {0}")]
    InvalidConfig(&'static str),
    /// Requested more tokens than the bucket can ever hold.
    #[error("requested {requested} tokens, but bucket capacity is {capacity}")]
    RequestExceedsCapacity {
        /// Tokens the caller asked for.
        requested: u32,
        /// Bucket capacity.
        capacity: u32,
    },
}

#[derive(Debug)]
struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

/// A thread-safe, cheaply-cloneable token-bucket rate limiter.
///
/// Clones share the same internal state.
#[derive(Clone)]
pub struct TokenBucket {
    cfg: TokenBucketConfig,
    state: Arc<Mutex<BucketState>>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for TokenBucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenBucket")
            .field("cfg", &self.cfg)
            .finish()
    }
}

impl TokenBucket {
    /// Creates a new token bucket using [`SystemClock`]. The bucket starts
    /// full.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_resilience::{TokenBucket, TokenBucketConfig};
    /// let cfg = TokenBucketConfig::new(5, 1.0).unwrap();
    /// let bucket = TokenBucket::new(cfg);
    /// assert_eq!(bucket.capacity(), 5);
    /// // Starts full, so 5 immediate acquires succeed.
    /// for _ in 0..5 {
    ///     assert!(bucket.try_acquire(1).unwrap());
    /// }
    /// ```
    pub fn new(cfg: TokenBucketConfig) -> Self {
        Self::with_clock(cfg, Arc::new(SystemClock))
    }

    /// Creates a new token bucket using an injected [`Clock`]. The bucket
    /// starts full.
    pub fn with_clock(cfg: TokenBucketConfig, clock: Arc<dyn Clock>) -> Self {
        let now = clock.now();
        Self {
            cfg,
            state: Arc::new(Mutex::new(BucketState {
                tokens: f64::from(cfg.capacity),
                last_refill: now,
            })),
            clock,
        }
    }

    /// Returns the configured capacity.
    pub fn capacity(&self) -> u32 {
        self.cfg.capacity
    }

    /// Non-blocking attempt to take `n` tokens. Returns `true` on success.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_resilience::{TokenBucket, TokenBucketConfig};
    /// let bucket = TokenBucket::new(TokenBucketConfig::new(2, 1.0).unwrap());
    /// assert!(bucket.try_acquire(1).unwrap());
    /// assert!(bucket.try_acquire(1).unwrap());
    /// // Bucket exhausted — returns Ok(false), not an error.
    /// assert!(!bucket.try_acquire(1).unwrap());
    /// ```
    pub fn try_acquire(&self, n: u32) -> Result<bool, RateLimitError> {
        if n == 0 {
            return Ok(true);
        }
        if n > self.cfg.capacity {
            return Err(RateLimitError::RequestExceedsCapacity {
                requested: n,
                capacity: self.cfg.capacity,
            });
        }
        let mut state = self.state.lock().expect("token-bucket mutex poisoned");
        self.refill_locked(&mut state);
        let need = f64::from(n);
        if state.tokens >= need {
            state.tokens -= need;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Attempts to reserve `n` tokens. On success returns `Duration::ZERO`.
    /// Otherwise returns the duration the caller must wait before the
    /// tokens will be available; on return from the wait, the tokens are
    /// already deducted (this is a reserving acquire, not a polling one).
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use pcloud_resilience::{TokenBucket, TokenBucketConfig};
    /// let bucket = TokenBucket::new(TokenBucketConfig::new(2, 2.0).unwrap());
    /// // First acquire is immediate.
    /// assert_eq!(bucket.acquire(2).unwrap(), Duration::ZERO);
    /// // Second acquire must wait.
    /// let wait = bucket.acquire(2).unwrap();
    /// assert!(wait > Duration::ZERO);
    /// ```
    pub fn acquire(&self, n: u32) -> Result<Duration, RateLimitError> {
        if n == 0 {
            return Ok(Duration::ZERO);
        }
        if n > self.cfg.capacity {
            return Err(RateLimitError::RequestExceedsCapacity {
                requested: n,
                capacity: self.cfg.capacity,
            });
        }
        let mut state = self.state.lock().expect("token-bucket mutex poisoned");
        self.refill_locked(&mut state);
        let need = f64::from(n);
        // Always deduct; tokens go negative by at most `need`. The wait
        // duration equals the time for the refill rate to zero out the
        // remaining debt.
        state.tokens -= need;
        if state.tokens >= 0.0 {
            Ok(Duration::ZERO)
        } else {
            let deficit = -state.tokens;
            let seconds = deficit / self.cfg.refill_rate_per_sec;
            Ok(Duration::from_secs_f64(seconds))
        }
    }

    fn refill_locked(&self, state: &mut BucketState) {
        let now = self.clock.now();
        if now <= state.last_refill {
            return;
        }
        let elapsed = now - state.last_refill;
        let added = elapsed.as_secs_f64() * self.cfg.refill_rate_per_sec;
        state.tokens = (state.tokens + added).min(f64::from(self.cfg.capacity));
        state.last_refill = now;
    }

    /// Returns the approximate current token count (for diagnostics/tests).
    pub fn available_tokens(&self) -> f64 {
        let mut state = self.state.lock().expect("token-bucket mutex poisoned");
        self.refill_locked(&mut state);
        state.tokens.max(0.0)
    }

    /// Returns the duration the caller would have to wait before `n` tokens
    /// are available, **without** deducting any tokens. Intended for
    /// "Retry-After" style advisories where the caller has already been
    /// rejected and must not burn a further reservation.
    ///
    /// Returns `Duration::ZERO` if `n` tokens are available immediately.
    /// Returns `RateLimitError::RequestExceedsCapacity` if `n` exceeds
    /// bucket capacity.
    pub fn peek_wait_for(&self, n: u32) -> Result<Duration, RateLimitError> {
        if n == 0 {
            return Ok(Duration::ZERO);
        }
        if n > self.cfg.capacity {
            return Err(RateLimitError::RequestExceedsCapacity {
                requested: n,
                capacity: self.cfg.capacity,
            });
        }
        let mut state = self.state.lock().expect("token-bucket mutex poisoned");
        self.refill_locked(&mut state);
        let need = f64::from(n);
        if state.tokens >= need {
            Ok(Duration::ZERO)
        } else {
            let deficit = need - state.tokens;
            let seconds = deficit / self.cfg.refill_rate_per_sec;
            Ok(Duration::from_secs_f64(seconds))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;

    fn bucket(capacity: u32, rate: f64, clock: Arc<ManualClock>) -> TokenBucket {
        let cfg = TokenBucketConfig::new(capacity, rate).unwrap();
        TokenBucket::with_clock(cfg, clock)
    }

    #[test]
    fn starts_full_and_allows_burst() {
        let clock = Arc::new(ManualClock::new());
        let b = bucket(5, 1.0, clock);
        for _ in 0..5 {
            assert!(b.try_acquire(1).unwrap());
        }
        assert!(!b.try_acquire(1).unwrap());
    }

    #[test]
    fn refills_linearly_over_time() {
        let clock = Arc::new(ManualClock::new());
        let b = bucket(10, 10.0, clock.clone());
        for _ in 0..10 {
            assert!(b.try_acquire(1).unwrap());
        }
        assert!(!b.try_acquire(1).unwrap());
        clock.advance(Duration::from_secs(1)); // +10 tokens
        for _ in 0..10 {
            assert!(b.try_acquire(1).unwrap());
        }
        assert!(!b.try_acquire(1).unwrap());
    }

    #[test]
    fn capacity_caps_refill() {
        let clock = Arc::new(ManualClock::new());
        let b = bucket(3, 1000.0, clock.clone());
        clock.advance(Duration::from_secs(60));
        assert!((b.available_tokens() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn reserving_acquire_returns_wait_duration() {
        let clock = Arc::new(ManualClock::new());
        let b = bucket(2, 2.0, clock);
        assert_eq!(b.acquire(2).unwrap(), Duration::ZERO);
        // Next acquire for 2 tokens at 2 tok/s must wait 1s.
        let wait = b.acquire(2).unwrap();
        assert!(wait >= Duration::from_millis(900) && wait <= Duration::from_millis(1100));
    }

    #[test]
    fn request_exceeds_capacity_errors() {
        let b = bucket(3, 1.0, Arc::new(ManualClock::new()));
        let err = b.try_acquire(4).unwrap_err();
        assert!(matches!(err, RateLimitError::RequestExceedsCapacity { .. }));
    }

    #[test]
    fn peek_wait_for_does_not_consume_tokens() {
        let clock = Arc::new(ManualClock::new());
        let b = bucket(2, 2.0, clock);
        // Drain the bucket.
        assert!(b.try_acquire(2).unwrap());
        // Peek reports a non-zero wait without burning a token.
        let peek = b.peek_wait_for(1).unwrap();
        assert!(peek > Duration::ZERO);
        // A second peek is unchanged because no token was consumed.
        let peek2 = b.peek_wait_for(1).unwrap();
        assert_eq!(peek, peek2);
    }

    #[test]
    fn peek_wait_for_zero_when_available() {
        let b = bucket(3, 1.0, Arc::new(ManualClock::new()));
        assert_eq!(b.peek_wait_for(1).unwrap(), Duration::ZERO);
        assert_eq!(b.peek_wait_for(3).unwrap(), Duration::ZERO);
        // Still full.
        assert!((b.available_tokens() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn zero_request_is_ok() {
        let b = bucket(1, 1.0, Arc::new(ManualClock::new()));
        assert!(b.try_acquire(0).unwrap());
        assert_eq!(b.acquire(0).unwrap(), Duration::ZERO);
    }

    #[test]
    fn invalid_config_rejected() {
        assert!(TokenBucketConfig::new(0, 1.0).is_err());
        assert!(TokenBucketConfig::new(1, 0.0).is_err());
        assert!(TokenBucketConfig::new(1, -1.0).is_err());
        assert!(TokenBucketConfig::new(1, f64::NAN).is_err());
        assert!(TokenBucketConfig::new(1, f64::INFINITY).is_err());
    }
}
