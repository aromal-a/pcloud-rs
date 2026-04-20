//! Injectable monotonic clock abstraction.
//!
//! Every resilience primitive in this crate reads the current instant via a
//! [`Clock`] so tests can advance time deterministically instead of calling
//! `std::thread::sleep`.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pcloud_observability::LockExt;

/// Monotonic clock abstraction.
///
/// Implementations must return instants that never go backwards. The trait
/// is `Send + Sync + 'static` so clocks can be shared across threads.
pub trait Clock: Send + Sync + 'static {
    /// Returns the current monotonic instant.
    fn now(&self) -> Instant;
}

/// Production clock backed by [`Instant::now`].
///
/// # Example
///
/// ```
/// use pcloud_resilience::{Clock, SystemClock};
///
/// let c = SystemClock;
/// let t0 = c.now();
/// let t1 = c.now();
/// assert!(t1 >= t0);
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    #[inline]
    fn now(&self) -> Instant {
        Instant::now()
    }
}

impl<C: Clock + ?Sized> Clock for Arc<C> {
    #[inline]
    fn now(&self) -> Instant {
        (**self).now()
    }
}

/// Manually-advanced clock for deterministic tests.
///
/// [`ManualClock::advance`] moves the virtual time forward by the supplied
/// duration. The underlying instant is an anchor [`Instant`] captured at
/// construction so values returned by [`Clock::now`] are real `Instant`s
/// and can be compared with other monotonic timestamps.
#[derive(Debug, Clone)]
pub struct ManualClock {
    inner: Arc<Mutex<Instant>>,
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ManualClock {
    /// Creates a clock anchored at the current real monotonic instant.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use pcloud_resilience::Clock;
    /// use pcloud_resilience::clock::ManualClock;
    ///
    /// let c = ManualClock::new();
    /// let t0 = c.now();
    /// c.advance(Duration::from_millis(500));
    /// assert!(c.now() - t0 >= Duration::from_millis(500));
    /// ```
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Creates a clock anchored at the supplied instant.
    pub fn at(anchor: Instant) -> Self {
        Self {
            inner: Arc::new(Mutex::new(anchor)),
        }
    }

    /// Advances virtual time by `delta`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use pcloud_resilience::Clock;
    /// use pcloud_resilience::clock::ManualClock;
    ///
    /// let c = ManualClock::new();
    /// let start = c.now();
    /// c.advance(Duration::from_secs(10));
    /// assert!(c.now() - start >= Duration::from_secs(10));
    /// ```
    pub fn advance(&self, delta: Duration) {
        let mut guard = self
            .inner
            .lock_or_poisoned("resilience::clock::ManualClock::advance");
        *guard += delta;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        *self
            .inner
            .lock_or_poisoned("resilience::clock::ManualClock::now")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_monotonic() {
        let c = ManualClock::new();
        let t0 = c.now();
        c.advance(Duration::from_millis(250));
        let t1 = c.now();
        assert!(t1 - t0 >= Duration::from_millis(250));
    }

    #[test]
    fn system_clock_is_monotonic() {
        let c = SystemClock;
        let t0 = c.now();
        let t1 = c.now();
        assert!(t1 >= t0);
    }

    #[test]
    fn arc_clock_forwards() {
        let c: Arc<dyn Clock> = Arc::new(ManualClock::new());
        let _ = c.now();
    }
}
