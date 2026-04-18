//! Global retry budget — shared token pool that limits retries across all
//! concurrent operations on a single client instance.
//!
//! A [`GlobalRetryBudget`] prevents retry storms: when many requests are
//! failing simultaneously their combined retries amplify load on an already
//! struggling backend. By sharing a token pool across all in-flight
//! operations the total number of additional attempts is bounded regardless
//! of how many goroutines / tasks are retrying concurrently.
//!
//! ## Usage
//!
//! ```
//! use pcloud_resilience::global_budget::GlobalRetryBudget;
//!
//! let budget = GlobalRetryBudget::new(100);
//!
//! if budget.try_consume() {
//!     // safe to retry
//! } else {
//!     // budget exhausted — give up immediately
//! }
//!
//! // After a successful attempt or during idle time the caller can
//! // replenish tokens so future retries are allowed again.
//! budget.replenish(1);
//! ```

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

/// Shared token pool that bounds the total number of retries across all
/// concurrent operations.
///
/// Internally the available token count is stored as an [`AtomicI64`] so
/// that both decrement and increment are lock-free. The invariant is that
/// the counter never exceeds `capacity` and never drops below `0` from the
/// caller's perspective: [`try_consume`] is a no-op when the counter is
/// already `0`.
///
/// [`try_consume`]: GlobalRetryBudget::try_consume
#[derive(Clone, Debug)]
pub struct GlobalRetryBudget {
    tokens: Arc<AtomicI64>,
    capacity: i64,
}

impl GlobalRetryBudget {
    /// Create a new budget with `capacity` tokens available.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is `0`.
    pub fn new(capacity: u32) -> Self {
        assert!(capacity > 0, "GlobalRetryBudget capacity must be > 0");
        Self {
            tokens: Arc::new(AtomicI64::new(capacity as i64)),
            capacity: capacity as i64,
        }
    }

    /// Attempt to consume one token.
    ///
    /// Returns `true` when a token was successfully consumed and the caller
    /// may proceed with a retry.  Returns `false` when the budget is
    /// exhausted; in that case the caller should give up immediately.
    ///
    /// The operation is lock-free and uses [`Ordering::Relaxed`] because
    /// the token count is a soft bound — exact synchronisation is not
    /// required for correctness; slightly over- or under-counting under
    /// contention is acceptable and expected.
    pub fn try_consume(&self) -> bool {
        // Optimistically decrement; if the previous value was already <= 0
        // the budget is exhausted and we put the token back.
        let prev = self.tokens.fetch_sub(1, Ordering::Relaxed);
        if prev <= 0 {
            // Restore the over-decremented token.
            self.tokens.fetch_add(1, Ordering::Relaxed);
            false
        } else {
            true
        }
    }

    /// Replenish up to `n` tokens, capped at `capacity`.
    ///
    /// Callers should call this after a successful request or during
    /// periodic refill cycles so that transient exhaustion does not
    /// permanently disable retries.
    pub fn replenish(&self, n: u32) {
        if n == 0 {
            return;
        }
        // Clamp to capacity using a compare-and-swap loop so we never
        // exceed the configured ceiling.
        let add = n as i64;
        loop {
            let current = self.tokens.load(Ordering::Relaxed);
            let desired = (current + add).min(self.capacity);
            if self
                .tokens
                .compare_exchange_weak(current, desired, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
    }

    /// Returns the configured capacity of this budget.
    pub fn capacity(&self) -> u32 {
        self.capacity as u32
    }

    /// Returns the current number of available tokens.  This is an
    /// instantaneous snapshot and may be stale by the time the caller acts
    /// on it; use [`try_consume`] for the authoritative decision.
    ///
    /// [`try_consume`]: GlobalRetryBudget::try_consume
    pub fn available(&self) -> u32 {
        self.tokens.load(Ordering::Relaxed).max(0) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_budget_has_full_tokens() {
        let b = GlobalRetryBudget::new(5);
        assert_eq!(b.available(), 5);
    }

    #[test]
    fn consume_decrements_tokens() {
        let b = GlobalRetryBudget::new(3);
        assert!(b.try_consume());
        assert_eq!(b.available(), 2);
        assert!(b.try_consume());
        assert_eq!(b.available(), 1);
        assert!(b.try_consume());
        assert_eq!(b.available(), 0);
    }

    #[test]
    fn exhausted_budget_returns_false() {
        let b = GlobalRetryBudget::new(1);
        assert!(b.try_consume());
        // Budget is now empty.
        assert!(!b.try_consume());
        assert!(!b.try_consume());
        // Available should never go negative.
        assert_eq!(b.available(), 0);
    }

    #[test]
    fn replenish_restores_tokens() {
        let b = GlobalRetryBudget::new(3);
        b.try_consume();
        b.try_consume();
        assert_eq!(b.available(), 1);
        b.replenish(2);
        assert_eq!(b.available(), 3);
    }

    #[test]
    fn replenish_does_not_exceed_capacity() {
        let b = GlobalRetryBudget::new(4);
        b.replenish(100);
        assert_eq!(b.available(), 4);
    }

    #[test]
    fn replenish_zero_is_noop() {
        let b = GlobalRetryBudget::new(3);
        b.try_consume();
        b.replenish(0);
        assert_eq!(b.available(), 2);
    }

    #[test]
    #[should_panic(expected = "capacity must be > 0")]
    fn zero_capacity_panics() {
        let _ = GlobalRetryBudget::new(0);
    }

    #[test]
    fn clone_shares_state() {
        let b1 = GlobalRetryBudget::new(2);
        let b2 = b1.clone();
        b1.try_consume();
        // b2 observes the same decrement.
        assert_eq!(b2.available(), 1);
    }
}
