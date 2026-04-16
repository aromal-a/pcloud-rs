//! Continuous reconcile worker (sync row 65 — `psync_start_sync`).
//!
//! Mirrors the behavior of the C `psyncer` thread (`pclsync/psyncer.c`)
//! plus the local-scan cadence in `pclsync/plocalscan.c`: a single
//! supervised loop that drives a periodic local-scan pass and yields
//! between cadences while the diff worker handles remote events.
//!
//! ## Why a pure tick, not a spawned tokio task here
//!
//! The daemon today does **not** own a top-level tokio runtime
//! (see `crate::session_lifecycle` and `pcloud-daemon::refresh_loop`
//! for the established pattern). We follow the same pattern: a pure,
//! synchronous [`crate::reconcile_worker::ReconcileWorker::tick`] that the embedding runtime
//! invokes on its own cadence (`std::thread::spawn` + `parking_lot`,
//! `tokio::spawn` + `tokio::time::sleep`, or driven directly from a
//! deterministic test). All timing is read from an injected
//! [`pcloud_resilience::clock::Clock`] so tests advance virtual time deterministically without
//! sleeping.
//!
//! ## Cadence
//!
//! The default `scan_interval` is `300s` to match the reconcile cadence
//! requested by the parity work (the C `PSYNC_LOCALSCAN_RESCAN_INTERVAL`
//! is more aggressive at 10s but only fires after change events; in the
//! Rust path we cover the change-event path with the diff worker and
//! reserve the periodic full-tree pass for slower cadences).

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;
use std::time::{Duration, Instant};

use pcloud_model::ids::SyncId;
use pcloud_resilience::clock::{Clock, SystemClock};

/// Default seconds between two reconcile (local-scan) passes.
pub const RECONCILE_DEFAULT_INTERVAL_SECS: u64 = 300;

/// Outcome of one [`ReconcileWorker::tick`].
///
/// The tick function is a **pure state machine step**: it does not
/// perform I/O and does not block. The caller inspects the outcome and
/// decides whether to fan out to the local scanner or go back to
/// sleep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileTickOutcome {
    /// Worker was idle; not enough time has elapsed since the previous
    /// scan to fire a new one. The caller should wait out its loop
    /// timer and tick again; it must **not** start a scan.
    Idle,
    /// Local-scan threshold crossed; the embedding runtime should
    /// invoke its scan callback for each sync root in `sync_ids`. The
    /// worker records the fire time internally so the next tick returns
    /// [`Self::Idle`] until the interval elapses again.
    RunScan {
        /// The sync roots that should be scanned this tick. Contains a
        /// snapshot of [`ReconcileWorker::tracked`] taken at the instant
        /// the tick fired; late `track`/`untrack` calls do not affect
        /// the returned list.
        sync_ids: Vec<SyncId>,
    },
    /// Worker is paused because no sync roots are currently tracked.
    /// The caller should do nothing until `track` is called; this
    /// outcome is idempotent and safe to poll.
    NoSyncRoots,
}

/// Continuous reconcile worker for a set of sync roots.
///
/// Holds:
/// * the injected [`pcloud_resilience::clock::Clock`],
/// * the last-fired scan instant,
/// * the configured scan interval,
/// * the current set of tracked sync roots.
///
/// The embedding runtime adds/removes sync roots as the daemon mutates
/// its sync graph, and calls [`tick`](Self::tick) on its preferred
/// cadence (typically every few seconds; the worker decides whether to
/// fire a scan or stay idle).
#[derive(Clone)]
pub struct ReconcileWorker {
    clock: Arc<dyn Clock>,
    interval: Duration,
    last_scan_at: Option<Instant>,
    tracked: Vec<SyncId>,
}

impl std::fmt::Debug for ReconcileWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReconcileWorker")
            .field("interval", &self.interval)
            .field("tracked_count", &self.tracked.len())
            .field("has_run", &self.last_scan_at.is_some())
            .finish()
    }
}

impl Default for ReconcileWorker {
    fn default() -> Self {
        Self::new(Duration::from_secs(RECONCILE_DEFAULT_INTERVAL_SECS))
    }
}

impl ReconcileWorker {
    /// Build a reconcile worker that fires every `interval` against the
    /// system clock.
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            clock: Arc::new(SystemClock),
            interval,
            last_scan_at: None,
            tracked: Vec::new(),
        }
    }

    /// Build a reconcile worker with an injected clock (deterministic
    /// tests).
    pub fn with_clock(interval: Duration, clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            interval,
            last_scan_at: None,
            tracked: Vec::new(),
        }
    }

    /// Configured scan interval.
    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Sync roots currently tracked by this worker.
    #[must_use]
    pub fn tracked(&self) -> &[SyncId] {
        &self.tracked
    }

    /// Register a sync root for periodic reconciliation.
    pub fn track(&mut self, sync_id: SyncId) -> bool {
        if self.tracked.contains(&sync_id) {
            return false;
        }
        self.tracked.push(sync_id);
        true
    }

    /// Stop tracking a sync root (called on remove/pause).
    pub fn untrack(&mut self, sync_id: SyncId) -> bool {
        let before = self.tracked.len();
        self.tracked.retain(|id| *id != sync_id);
        self.tracked.len() != before
    }

    /// Run one iteration of the reconcile state machine.
    ///
    /// # Semantics
    ///
    /// * Returns [`ReconcileTickOutcome::NoSyncRoots`] if nothing is
    ///   being tracked.
    /// * Returns [`ReconcileTickOutcome::RunScan`] on the **first** tick
    ///   after tracking begins (cold start) and on any tick after the
    ///   configured interval has elapsed since the last fire. The
    ///   `last_scan_at` instant is updated before the outcome is
    ///   returned, so a sequence of rapid ticks will only fire once
    ///   per interval.
    /// * Returns [`ReconcileTickOutcome::Idle`] on every tick where the
    ///   interval has not elapsed yet.
    ///
    /// # Call frequency
    ///
    /// The caller decides how often to tick (every few seconds is
    /// typical); the worker's own `interval` only controls how often
    /// ticks fire. Over-ticking is cheap — the worker just returns
    /// `Idle` until the threshold is reached.
    pub fn tick(&mut self) -> ReconcileTickOutcome {
        if self.tracked.is_empty() {
            return ReconcileTickOutcome::NoSyncRoots;
        }
        let now = self.clock.now();
        let due = match self.last_scan_at {
            None => true,
            Some(last) => now.duration_since(last) >= self.interval,
        };
        if !due {
            return ReconcileTickOutcome::Idle;
        }
        self.last_scan_at = Some(now);
        ReconcileTickOutcome::RunScan {
            sync_ids: self.tracked.clone(),
        }
    }

    /// Force-rewind the last-scan timer so the next tick fires
    /// immediately. Used when an out-of-band event signals a sync root
    /// needs immediate reconciliation.
    pub fn request_scan(&mut self) {
        self.last_scan_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_resilience::clock::ManualClock;

    fn worker(interval_secs: u64) -> (ReconcileWorker, ManualClock) {
        let clock = ManualClock::new();
        let arc: Arc<dyn Clock> = Arc::new(clock.clone());
        let w = ReconcileWorker::with_clock(Duration::from_secs(interval_secs), arc);
        (w, clock)
    }

    #[test]
    fn idle_when_no_sync_roots() {
        let (mut w, _c) = worker(300);
        assert_eq!(w.tick(), ReconcileTickOutcome::NoSyncRoots);
    }

    #[test]
    fn fires_first_tick_then_idles_until_threshold() {
        let (mut w, c) = worker(300);
        assert!(w.track(SyncId::new(1)));
        assert!(!w.track(SyncId::new(1))); // duplicate

        let outcome = w.tick();
        assert_eq!(
            outcome,
            ReconcileTickOutcome::RunScan {
                sync_ids: vec![SyncId::new(1)]
            }
        );

        // Immediately after, worker is idle.
        assert_eq!(w.tick(), ReconcileTickOutcome::Idle);

        // Advance below threshold: still idle.
        c.advance(Duration::from_secs(299));
        assert_eq!(w.tick(), ReconcileTickOutcome::Idle);

        // Cross threshold: fires again.
        c.advance(Duration::from_secs(2));
        match w.tick() {
            ReconcileTickOutcome::RunScan { sync_ids } => {
                assert_eq!(sync_ids, vec![SyncId::new(1)]);
            }
            other => panic!("expected RunScan, got {other:?}"),
        }
    }

    #[test]
    fn untrack_drops_sync_root_from_next_run() {
        let (mut w, c) = worker(60);
        w.track(SyncId::new(1));
        w.track(SyncId::new(2));
        let _ = w.tick();
        c.advance(Duration::from_secs(60));
        w.untrack(SyncId::new(1));
        match w.tick() {
            ReconcileTickOutcome::RunScan { sync_ids } => {
                assert_eq!(sync_ids, vec![SyncId::new(2)]);
            }
            other => panic!("expected RunScan, got {other:?}"),
        }
    }

    #[test]
    fn request_scan_forces_next_tick_to_fire() {
        let (mut w, _c) = worker(3600);
        w.track(SyncId::new(7));
        let _ = w.tick();
        assert_eq!(w.tick(), ReconcileTickOutcome::Idle);
        w.request_scan();
        match w.tick() {
            ReconcileTickOutcome::RunScan { sync_ids } => {
                assert_eq!(sync_ids, vec![SyncId::new(7)]);
            }
            other => panic!("expected RunScan, got {other:?}"),
        }
    }
}
