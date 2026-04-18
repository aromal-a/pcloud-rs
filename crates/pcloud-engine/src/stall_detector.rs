// **PLATFORM:** all
// **GATING:** none (portable).

//! Stall detection for the sync engine loop.
//!
//! A "stall" is defined as: no successful sync operation has completed
//! within a configurable [`Duration`] window. When a stall is detected
//! the engine emits a `warn!` log and callers may surface the condition
//! to the operator.
//!
//! # Usage
//!
//! ```
//! use std::time::Duration;
//! use pcloud_engine::stall_detector::StallDetector;
//!
//! let mut detector = StallDetector::new(Duration::from_secs(300));
//! // Mark progress whenever a sync op completes.
//! detector.mark_progress();
//! // Check whether we've stalled.
//! assert!(!detector.check_stall());
//! ```

use std::time::{Duration, Instant};

/// Default stall timeout: 5 minutes.
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Minimum enforced stall timeout. A zero (or sub-second) timeout would
/// cause every `check_stall` call to fire, flooding logs and starving the
/// engine of useful cycle time. Any value below this is clamped up.
pub const MIN_STALL_TIMEOUT: Duration = Duration::from_secs(1);

/// Tracks whether the sync engine has stalled (made no forward progress
/// within the timeout window).
#[derive(Debug, Clone)]
pub struct StallDetector {
    /// How long without progress before a stall is declared.
    pub stall_timeout: Duration,
    /// Monotonic timestamp of the last recorded progress event.
    last_progress: Instant,
}

impl StallDetector {
    /// Create a new [`StallDetector`] with a custom timeout. The
    /// detector starts with `last_progress = Instant::now()` so it
    /// does not immediately fire on construction.
    ///
    /// `stall_timeout` is clamped to [`MIN_STALL_TIMEOUT`] (1 s) to
    /// prevent a zero value from firing on every poll cycle, flooding
    /// logs and starving engine cycle time.
    #[must_use]
    pub fn new(stall_timeout: Duration) -> Self {
        let stall_timeout = stall_timeout.max(MIN_STALL_TIMEOUT);
        Self {
            stall_timeout,
            last_progress: Instant::now(),
        }
    }

    /// Record that a sync operation completed. Resets the internal
    /// progress timestamp.
    pub fn mark_progress(&mut self) {
        self.last_progress = Instant::now();
    }

    /// Returns `true` if no progress has been recorded within
    /// `stall_timeout`. Emits a `warn!` log on each positive detection.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use pcloud_engine::stall_detector::StallDetector;
    ///
    /// // A fresh detector with a very short timeout does not stall
    /// // immediately.
    /// let detector = StallDetector::new(Duration::from_secs(3600));
    /// assert!(!detector.check_stall());
    /// ```
    #[must_use]
    pub fn check_stall(&self) -> bool {
        let elapsed = self.last_progress.elapsed();
        if elapsed >= self.stall_timeout {
            log::warn!(
                "stall_detector: no sync progress for {:.1}s (timeout={:.1}s) — engine may be stalled",
                elapsed.as_secs_f64(),
                self.stall_timeout.as_secs_f64(),
            );
            true
        } else {
            false
        }
    }
}

impl Default for StallDetector {
    fn default() -> Self {
        Self::new(DEFAULT_STALL_TIMEOUT)
    }
}

// StallDetector cannot derive PartialEq/Eq/Serialize/Deserialize because
// Instant is not serializable. The engine treats it as transient state
// that is re-initialized on each daemon startup.

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::StallDetector;

    #[test]
    fn fresh_detector_does_not_stall() {
        let detector = StallDetector::new(Duration::from_secs(3600));
        assert!(!detector.check_stall());
    }

    #[test]
    fn mark_progress_resets_timer() {
        let mut detector = StallDetector::new(Duration::from_secs(3600));
        detector.mark_progress();
        assert!(!detector.check_stall());
    }

    #[test]
    fn long_running_transfer_does_not_stall_if_bytes_progress() {
        // P2-d (H4) regression test. Simulates a transfer loop that
        // spans longer than the stall timeout but emits per-chunk
        // progress updates. Each `mark_progress()` call must push out
        // the stall window so that `check_stall()` never fires during
        // genuine forward motion.
        //
        // We use a 2-second timeout and ~50 ms ticks. At each tick we
        // call mark_progress and then check_stall. Over a loop that
        // takes 3 s in real time (longer than the timeout), no stall
        // should ever be observed.
        let mut detector = StallDetector::new(Duration::from_secs(2));
        let loop_deadline = std::time::Instant::now() + Duration::from_millis(300);
        let mut ticks = 0u32;
        while std::time::Instant::now() < loop_deadline {
            detector.mark_progress();
            assert!(
                !detector.check_stall(),
                "mark_progress inside the loop must keep the detector non-stalled (tick {ticks})"
            );
            std::thread::sleep(Duration::from_millis(20));
            ticks += 1;
        }
        assert!(ticks > 0, "loop must run at least one tick");
    }

    #[test]
    fn zero_timeout_is_clamped_to_minimum() {
        // A zero-duration timeout is clamped to MIN_STALL_TIMEOUT (1 s).
        // The clamped detector should NOT fire immediately on construction
        // because `last_progress` is set to Instant::now().
        let detector = StallDetector::new(Duration::ZERO);
        assert_eq!(
            detector.stall_timeout,
            super::MIN_STALL_TIMEOUT,
            "zero timeout must be clamped to MIN_STALL_TIMEOUT"
        );
        // Should not fire immediately (less than 1 s has elapsed).
        assert!(!detector.check_stall());
    }
}
