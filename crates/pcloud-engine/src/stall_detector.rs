// **PLATFORM:** all
// **GATING:** none (portable).

//! Stall detection for the sync engine loop.
//!
//! A "stall" is defined as: no sync-loop progress has been recorded
//! within a configurable [`Duration`] window. Two classes of progress
//! event are tracked, and either class resets the stall clock:
//!
//! 1. **Wall-clock progress** via [`StallDetector::mark_progress`] — a
//!    coarse "the loop ran a cycle and did something" signal emitted by
//!    the sync loop after scheduling or after a completion.
//! 2. **Byte-level transfer progress** via
//!    [`StallDetector::observe_bytes`] — fine-grained, per-transfer
//!    byte counters. A long-running upload or download that is steadily
//!    transferring bytes MUST NOT be reported as stalled even if
//!    `mark_progress` happens to not be called during the transfer.
//!
//! Audit-06 §4-opus HIGH regression fix: the audit-05 claim that
//! [`StallDetector`] tracked byte-level progress was not actually
//! realised in source. The `observe_bytes` entry point below is the
//! canonical byte-progress hook; callers on the transfer hot path
//! (see `pcloud-backends::transfer_backend`) must invoke it on every
//! acknowledged chunk.
//!
//! # Usage
//!
//! ```
//! use std::time::Duration;
//! use pcloud_engine::stall_detector::StallDetector;
//!
//! let mut detector = StallDetector::new(Duration::from_secs(300));
//! // Mark coarse cycle progress.
//! detector.mark_progress();
//! // Record bytes transferred for a specific transfer id.
//! detector.observe_bytes("my/file.bin", 4096);
//! // Check whether we've stalled.
//! assert!(!detector.check_stall());
//! ```

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default stall timeout: 5 minutes.
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Minimum enforced stall timeout. A zero (or sub-second) timeout would
/// cause every `check_stall` call to fire, flooding logs and starving the
/// engine of useful cycle time. Any value below this is clamped up.
pub const MIN_STALL_TIMEOUT: Duration = Duration::from_secs(1);

/// Per-transfer byte-progress counter.
#[derive(Debug, Clone, Copy)]
struct ByteProgress {
    /// Total bytes observed for this transfer so far (monotonic).
    total_bytes: u64,
    /// Instant at which the last byte-delta was observed.
    last_progress: Instant,
}

/// Tracks whether the sync engine has stalled (made no forward progress
/// within the timeout window).
///
/// Progress is recorded by either [`Self::mark_progress`] (wall-clock)
/// or [`Self::observe_bytes`] (per-transfer byte deltas). Byte-level
/// tracking is stored in an internal [`Mutex`]-guarded map so the
/// transfer hot path can call `observe_bytes` through a shared
/// reference.
#[derive(Debug)]
pub struct StallDetector {
    /// How long without progress before a stall is declared.
    pub stall_timeout: Duration,
    /// Monotonic timestamp of the last coarse wall-clock progress event.
    last_progress: Instant,
    /// Per-transfer byte-progress counters. Keyed by an opaque transfer
    /// identifier (the sync-loop uses the logical path string).
    byte_progress: Mutex<HashMap<String, ByteProgress>>,
}

impl Clone for StallDetector {
    fn clone(&self) -> Self {
        let map = self
            .byte_progress
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        Self {
            stall_timeout: self.stall_timeout,
            last_progress: self.last_progress,
            byte_progress: Mutex::new(map),
        }
    }
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
            byte_progress: Mutex::new(HashMap::new()),
        }
    }

    /// Record that a sync operation completed (coarse wall-clock
    /// progress). Resets the internal progress timestamp.
    pub fn mark_progress(&mut self) {
        self.last_progress = Instant::now();
    }

    /// Record that `bytes_delta` additional bytes were transferred for
    /// the transfer identified by `transfer_id`. Updates the per-transfer
    /// byte total and bumps the per-transfer last-progress instant.
    ///
    /// Audit-06 §4-opus HIGH regression fix. This entry point is
    /// deliberately `&self` (not `&mut self`) so the transfer hot path
    /// can share the detector across threads without routing every
    /// chunk through an engine-level mutex boundary. The internal
    /// [`Mutex`] is scoped narrowly around the map update.
    ///
    /// A `bytes_delta` of `0` is accepted and merely refreshes the
    /// last-progress instant (useful as a "heartbeat" hook).
    pub fn observe_bytes(&self, transfer_id: &str, bytes_delta: u64) {
        let now = Instant::now();
        let mut guard = match self.byte_progress.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                log::error!(
                    "stall_detector: byte_progress mutex poisoned at {}:{}",
                    file!(),
                    line!()
                );
                poisoned.into_inner()
            }
        };
        let entry = guard
            .entry(transfer_id.to_owned())
            .or_insert(ByteProgress {
                total_bytes: 0,
                last_progress: now,
            });
        entry.total_bytes = entry.total_bytes.saturating_add(bytes_delta);
        entry.last_progress = now;
    }

    /// Drop byte-progress state for `transfer_id`, typically called
    /// after the transfer is acknowledged and retired. Idempotent.
    pub fn forget_transfer(&self, transfer_id: &str) {
        let mut guard = match self.byte_progress.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.remove(transfer_id);
    }

    /// Returns `true` if **neither** the wall-clock progress clock
    /// **nor** any tracked per-transfer byte counter has advanced within
    /// `stall_timeout`. Emits a `warn!` log on each positive detection.
    ///
    /// A long-running transfer that is steadily emitting
    /// [`Self::observe_bytes`] will keep the detector non-stalled even
    /// if [`Self::mark_progress`] is not called in the meantime —
    /// this is the audit-06 §4-opus HIGH regression fix.
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
        let wall_elapsed = self.last_progress.elapsed();
        if wall_elapsed < self.stall_timeout {
            return false;
        }
        // Wall-clock has exceeded the timeout; byte-progress is the
        // tiebreaker. Any active transfer whose last byte-progress was
        // within the timeout window proves we are not stalled.
        let bytes_recent_enough = {
            let guard = match self.byte_progress.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .values()
                .any(|bp| bp.last_progress.elapsed() < self.stall_timeout)
        };
        if bytes_recent_enough {
            return false;
        }
        log::warn!(
            "stall_detector: no sync progress for {:.1}s (timeout={:.1}s) — engine may be stalled",
            wall_elapsed.as_secs_f64(),
            self.stall_timeout.as_secs_f64(),
        );
        true
    }
}

impl Default for StallDetector {
    fn default() -> Self {
        Self::new(DEFAULT_STALL_TIMEOUT)
    }
}

impl StallDetector {
    /// Create a [`StallDetector`] whose clock is pre-offset by `already_elapsed`.
    ///
    /// Use this when the caller knows that some time has already passed since
    /// the last recorded progress event — for example when restoring a daemon
    /// from persistent state that recorded a wall-clock progress timestamp.
    /// By subtracting `already_elapsed` from `Instant::now()` the detector
    /// immediately reflects the real staleness rather than resetting to zero.
    ///
    /// `already_elapsed` is capped at `stall_timeout` so the detector does
    /// not immediately fire on construction (callers should check stall after
    /// construction if they want to surface a cross-restart stall event).
    ///
    /// M-4.7.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use pcloud_engine::stall_detector::StallDetector;
    ///
    /// // Simulate a detector restored 90 s into a 300 s window.
    /// let d = StallDetector::new_with_elapsed(Duration::from_secs(300), Duration::from_secs(90));
    /// // 90 s of the 300 s budget is already consumed; not yet stalled.
    /// assert!(!d.check_stall());
    /// ```
    #[must_use]
    pub fn new_with_elapsed(stall_timeout: Duration, already_elapsed: Duration) -> Self {
        let stall_timeout = stall_timeout.max(MIN_STALL_TIMEOUT);
        // Cap the pre-consumed budget so `last_progress` stays in the past
        // but not so far that the check would immediately fire.
        let budget_consumed = already_elapsed.min(stall_timeout.saturating_sub(Duration::from_millis(1)));
        let last_progress = Instant::now().checked_sub(budget_consumed).unwrap_or_else(Instant::now);
        Self {
            stall_timeout,
            last_progress,
            byte_progress: Mutex::new(HashMap::new()),
        }
    }

    /// Export the last-progress time as a duration elapsed since `Instant::now()`.
    ///
    /// The returned value is suitable for persisting as a wall-clock
    /// offset (e.g. write `SystemTime::now() - elapsed_since_progress` to the
    /// store). On next boot compute `SystemTime::now() - persisted_wall_clock`
    /// and pass the result to [`Self::new_with_elapsed`].
    ///
    /// M-4.7.
    #[must_use]
    pub fn elapsed_since_progress(&self) -> Duration {
        self.last_progress.elapsed()
    }
}

// StallDetector cannot derive PartialEq/Eq/Serialize/Deserialize because
// Instant is not serializable. The engine treats it as transient state
// that is re-initialized on each daemon startup.
//
// M-4.7 note: because `Instant` is not serializable, a `StallDetector`
// constructed via [`StallDetector::new`] always resets its
// `last_progress` clock to the current instant. This means a stall that
// accumulated across a daemon restart is not detected until the new
// instance's timeout elapses again after the restart. For the vast
// majority of cases this is acceptable behavior — a daemon restart is
// itself a form of forward progress and the stall window after a restart
// is bounded by the configured `stall_timeout`.
//
// Callers that need true cross-restart stall tracking should persist the
// last-progress wall-clock time (unix seconds) to the `value_kv` store
// and use [`StallDetector::new_with_elapsed`] on the next boot to
// initialize the detector as if the timeout window started before the
// restart. See `sync_loop_runtime.rs` for the persistence hook.

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

    /// Audit-06 §4-opus HIGH regression fix: the audit-05 claim that a
    /// long-running transfer keeps the stall detector quiet via
    /// byte-progress observations must actually hold.
    ///
    /// The test simulates a transfer that:
    /// (a) does NOT call the coarse `mark_progress` hook, and
    /// (b) emits ~4 KiB observations every 10 ms,
    ///
    /// over a loop window that is **longer** than the configured stall
    /// timeout. `check_stall` MUST return false throughout. A final
    /// silence period then proves the detector correctly reverts to
    /// stalled once byte-progress stops.
    ///
    /// The task specification calls for a 150 s wall-clock run; that is
    /// impractical in CI. The property being proved is the **ratio**
    /// (loop duration > stall timeout) rather than absolute seconds, so
    /// the timings are scaled down to 1.5 s timeout / 2.5 s loop. The
    /// clamp at [`MIN_STALL_TIMEOUT`] (1 s) prevents sub-second
    /// timeouts, so the numbers must not go below that floor.
    #[test]
    fn long_running_transfer_does_not_stall_if_bytes_progress() {
        let stall_timeout = Duration::from_millis(1_500);
        let loop_duration = Duration::from_millis(2_500);
        let tick = Duration::from_millis(10);

        // Construct a detector whose wall-clock is pre-aged right up to
        // the stall boundary. After the first `sleep` below, wall-clock
        // alone will report stalled; only byte-progress can keep it
        // quiet from that point on.
        let detector = StallDetector::new_with_elapsed(
            stall_timeout,
            stall_timeout.saturating_sub(Duration::from_millis(50)),
        );

        // Nudge wall-clock just past the stall window.
        std::thread::sleep(Duration::from_millis(100));

        // Drive byte-progress at 10 ms cadence for > stall_timeout.
        // Despite wall-clock staleness, the detector MUST stay
        // non-stalled the whole time because per-transfer byte-progress
        // is fresh.
        let start = std::time::Instant::now();
        let mut ticks = 0u32;
        while start.elapsed() < loop_duration {
            detector.observe_bytes("transfer-1", 4096);
            assert!(
                !detector.check_stall(),
                "byte-progress at tick {ticks} must keep the detector non-stalled \
                 (elapsed={:?}, stall_timeout={:?})",
                start.elapsed(),
                stall_timeout,
            );
            std::thread::sleep(tick);
            ticks += 1;
        }
        assert!(
            ticks > 50,
            "loop must tick at least 50 times over {loop_duration:?} (got {ticks})"
        );
        assert!(
            start.elapsed() > stall_timeout,
            "by construction the loop must outlast the stall timeout \
             (elapsed={:?}, stall_timeout={:?})",
            start.elapsed(),
            stall_timeout,
        );

        // Final sanity: once byte-progress stops for > stall_timeout
        // the detector MUST flip to stalled. Wait `stall_timeout +
        // slack` so both wall-clock and the last byte-progress instant
        // age out of the window.
        std::thread::sleep(stall_timeout + Duration::from_millis(250));
        assert!(
            detector.check_stall(),
            "after {:?} of silence with a {:?} stall_timeout the detector must report stalled",
            stall_timeout + Duration::from_millis(250),
            stall_timeout,
        );
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

    /// Byte observations are cumulative and per-transfer. Forgetting a
    /// transfer drops its contribution to the liveness calculation.
    ///
    /// Uses a 1 s stall timeout (at the [`super::MIN_STALL_TIMEOUT`]
    /// floor) and 1.2 s sleeps so the wall-clock ages out between
    /// assertions. Shorter timeouts cannot be used — the constructor
    /// clamps them up.
    #[test]
    fn forget_transfer_removes_byte_progress_contribution() {
        let stall_timeout = Duration::from_millis(1_000);
        // Pre-age wall-clock so `observe_bytes` is the only thing
        // holding the detector quiet.
        let detector = StallDetector::new_with_elapsed(
            stall_timeout,
            stall_timeout.saturating_sub(Duration::from_millis(10)),
        );
        detector.observe_bytes("file-a", 1024);
        // Age both wall-clock AND the byte-progress instant past the window.
        std::thread::sleep(stall_timeout + Duration::from_millis(200));
        // Both are stale now — detector must report stalled.
        assert!(detector.check_stall());

        // Fresh byte-progress inside the window flips it back.
        detector.observe_bytes("file-a", 1024);
        assert!(!detector.check_stall());

        // Forgetting the only live transfer must cause the next
        // check_stall (after the timeout elapses) to report stalled.
        detector.forget_transfer("file-a");
        std::thread::sleep(stall_timeout + Duration::from_millis(200));
        assert!(detector.check_stall());
    }
}
