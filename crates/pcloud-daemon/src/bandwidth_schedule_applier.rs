//! T1.4.b — Drive `BandwidthPacer` from a `BandwidthScheduleConfig`.
//!
//! # What this module does
//!
//! The pure decision function lives in
//! [`pcloud_config::bandwidth_schedule::BandwidthScheduleConfig::current_cap`].
//! This module is the daemon-side glue that:
//!
//! 1. Reads the current wall-clock minute-of-day + weekday.
//! 2. Calls `current_cap` to decide the active cap.
//! 3. Calls [`BandwidthPacer::set_limit`] only when the value changed,
//!    avoiding redundant atomic stores on every tick.
//!
//! The applier is meant to be called once per sync-loop tick (or on a
//! coarser timer). The schedule itself changes at most once per minute,
//! so a tick rate even a few seconds apart is fine.
//!
//! # Why not in `pcloud-config` or `pcloud-resilience`
//!
//! `pcloud-config` deliberately has no runtime dependencies (no pacer,
//! no clock); `pcloud-resilience` deliberately has no config dependency
//! (the pacer is wire-protocol agnostic). The daemon already depends on
//! both, so this is its natural home.
//!
//! # Metered hint
//!
//! `apply_now` takes the metered-network boolean directly. T1.4.c will
//! provide the platform-specific detector that produces this hint;
//! until then call sites pass `false`, which correctly preserves the
//! pre-T1.4 behaviour (only the time-of-day rules + default cap apply).

// **PLATFORM:** all
// **GATING:** none (portable; metered detection is wired separately).

use std::sync::{Arc, Mutex};

use chrono::{Datelike, Local, TimeZone, Timelike};
use pcloud_config::bandwidth_schedule::{BandwidthScheduleConfig, Weekday};
use pcloud_resilience::BandwidthPacer;

/// Drives a `BandwidthPacer` from a `BandwidthScheduleConfig`.
///
/// Cheap to clone: the pacer is `Arc`-shared and the `last_applied`
/// guard is mutex-protected so multiple loop drivers (e.g. one per sync
/// root) do not race the underlying limit.
#[derive(Debug, Clone)]
pub struct BandwidthScheduleApplier {
    pacer: Arc<BandwidthPacer>,
    /// Last cap value successfully applied to `pacer`. `None` means
    /// "unlimited"; `Some(0)` is normalised away by
    /// `BandwidthScheduleConfig::current_cap`, never reached here.
    /// `Mutex` (rather than `AtomicU64`) because the value is
    /// `Option<u64>` and we want a single atomic compare-and-swap of
    /// both branches; the call rate is low (≤1 Hz) so the lock cost is
    /// trivial.
    last_applied: Arc<Mutex<Option<Option<u64>>>>,
}

/// Outcome of a single [`BandwidthScheduleApplier::apply_now`] tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The schedule resolved to a cap that differs from the last
    /// applied value (or this is the very first apply); the pacer was
    /// updated to `cap`.
    Changed {
        /// Cap value that was applied (`None` = unlimited).
        cap: Option<u64>,
    },
    /// The schedule resolved to the same cap as last time; the pacer
    /// was left untouched.
    Unchanged {
        /// Current cap (= `last_applied`).
        cap: Option<u64>,
    },
}

impl BandwidthScheduleApplier {
    /// Construct an applier wrapping a shared `BandwidthPacer`. The
    /// applier does not push an initial cap — the very first
    /// `apply_*` call always emits [`ApplyOutcome::Changed`].
    #[must_use]
    pub fn new(pacer: Arc<BandwidthPacer>) -> Self {
        Self {
            pacer,
            last_applied: Arc::new(Mutex::new(None)),
        }
    }

    /// Apply the schedule using the host's current wall-clock time.
    ///
    /// Convenience wrapper around [`Self::apply_at`]. Uses `Local::now()`
    /// so daylight-saving transitions track the operator's chosen time
    /// zone (the schedule is described in local minute-of-day, not UTC).
    pub fn apply_now(&self, schedule: &BandwidthScheduleConfig, on_metered: bool) -> ApplyOutcome {
        self.apply_at(schedule, Local::now(), on_metered)
    }

    /// Apply the schedule using `now` as the reference time.
    ///
    /// Exposed separately so tests can fast-forward without depending
    /// on system time.
    pub fn apply_at<Tz>(
        &self,
        schedule: &BandwidthScheduleConfig,
        now: chrono::DateTime<Tz>,
        on_metered: bool,
    ) -> ApplyOutcome
    where
        Tz: TimeZone,
    {
        let weekday = match Weekday::from_iso(now.weekday().num_days_from_monday() as u8) {
            Some(w) => w,
            // Practically unreachable — chrono's `num_days_from_monday`
            // is bounded to 0..=6. Stay defensive: an unrecognised
            // value should not crash the loop driver.
            None => {
                return ApplyOutcome::Unchanged {
                    cap: self.current_known_cap(),
                };
            }
        };
        let minute = now.hour() * 60 + now.minute();
        let cap = schedule.current_cap(weekday, minute, on_metered);

        let mut last = self.last_applied.lock().unwrap_or_else(|p| p.into_inner());
        if last.map(|prev| prev == cap).unwrap_or(false) {
            return ApplyOutcome::Unchanged { cap };
        }
        self.pacer.set_limit(cap);
        *last = Some(cap);
        ApplyOutcome::Changed { cap }
    }

    /// Read the cap that was last applied through this applier. Used
    /// by callers that need to render the active cap without
    /// triggering a re-evaluation. Returns `None` if no `apply_*` has
    /// run yet (the pacer is whatever the constructor produced).
    #[must_use]
    pub fn last_applied(&self) -> Option<Option<u64>> {
        *self.last_applied.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn current_known_cap(&self) -> Option<u64> {
        self.last_applied().unwrap_or_else(|| self.pacer.limit())
    }

    /// Borrow the underlying pacer (e.g. to share it with another
    /// runtime that needs to consume tokens).
    #[must_use]
    pub fn pacer(&self) -> Arc<BandwidthPacer> {
        Arc::clone(&self.pacer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pcloud_config::bandwidth_schedule::{BandwidthRule, BandwidthScheduleConfig};

    fn workday_schedule() -> BandwidthScheduleConfig {
        BandwidthScheduleConfig {
            enabled: true,
            default_cap_bytes_per_sec: Some(1_000_000),
            metered_cap_bytes_per_sec: Some(256_000),
            rules: vec![BandwidthRule {
                start_minute_of_day: 540,   // 09:00
                end_minute_of_day: 1020,    // 17:00
                cap_bytes_per_sec: Some(0), // unlimited
                days: vec![Weekday::Wed],
            }],
        }
    }

    fn at(year: i32, month: u32, day: u32, h: u32, m: u32) -> chrono::DateTime<Local> {
        Local
            .with_ymd_and_hms(year, month, day, h, m, 0)
            .single()
            .expect("valid local time")
    }

    #[test]
    fn first_apply_pushes_a_cap() {
        let pacer = Arc::new(BandwidthPacer::new(None));
        let applier = BandwidthScheduleApplier::new(pacer.clone());
        let cfg = workday_schedule();
        // Wednesday 12:00 → unlimited boost window.
        let outcome = applier.apply_at(&cfg, at(2026, 4, 29, 12, 0), false);
        assert_eq!(outcome, ApplyOutcome::Changed { cap: None });
        assert_eq!(pacer.limit(), None);
        assert_eq!(applier.last_applied(), Some(None));
    }

    #[test]
    fn redundant_apply_is_unchanged() {
        let pacer = Arc::new(BandwidthPacer::new(None));
        let applier = BandwidthScheduleApplier::new(pacer.clone());
        let cfg = workday_schedule();
        let _ = applier.apply_at(&cfg, at(2026, 4, 29, 12, 0), false);
        let outcome = applier.apply_at(&cfg, at(2026, 4, 29, 12, 30), false);
        assert!(matches!(outcome, ApplyOutcome::Unchanged { .. }));
    }

    #[test]
    fn cap_change_across_window_boundary_emits_changed() {
        let pacer = Arc::new(BandwidthPacer::new(None));
        let applier = BandwidthScheduleApplier::new(pacer.clone());
        let cfg = workday_schedule();
        // 12:00 → unlimited (rule fires).
        let _ = applier.apply_at(&cfg, at(2026, 4, 29, 12, 0), false);
        // 17:00 → out of window → default 1 MB/s.
        let outcome = applier.apply_at(&cfg, at(2026, 4, 29, 17, 0), false);
        assert_eq!(
            outcome,
            ApplyOutcome::Changed {
                cap: Some(1_000_000)
            }
        );
        assert_eq!(pacer.limit(), Some(1_000_000));
    }

    #[test]
    fn metered_overrides_time_rule_after_apply() {
        let pacer = Arc::new(BandwidthPacer::new(None));
        let applier = BandwidthScheduleApplier::new(pacer.clone());
        let cfg = workday_schedule();
        // 12:00 with metered=true → metered cap wins.
        let outcome = applier.apply_at(&cfg, at(2026, 4, 29, 12, 0), true);
        assert_eq!(outcome, ApplyOutcome::Changed { cap: Some(256_000) });
        assert_eq!(pacer.limit(), Some(256_000));
    }

    #[test]
    fn schedule_disabled_pushes_unlimited() {
        let pacer = Arc::new(BandwidthPacer::new(Some(2_000_000)));
        let applier = BandwidthScheduleApplier::new(pacer.clone());
        let cfg = BandwidthScheduleConfig::default(); // disabled
        let outcome = applier.apply_at(&cfg, at(2026, 4, 29, 12, 0), false);
        // current_cap returns None when disabled → applier pushes None.
        // Operators who want the disabled schedule to leave the existing
        // base cap alone should not call apply_* in the first place.
        assert_eq!(outcome, ApplyOutcome::Changed { cap: None });
        assert_eq!(pacer.limit(), None);
    }

    #[test]
    fn weekday_filter_routes_off_day_to_default() {
        let pacer = Arc::new(BandwidthPacer::new(None));
        let applier = BandwidthScheduleApplier::new(pacer.clone());
        let cfg = workday_schedule();
        // Saturday 12:00 → not Wed, rule does not fire → default 1 MB/s.
        let outcome = applier.apply_at(&cfg, at(2026, 5, 2, 12, 0), false);
        assert_eq!(
            outcome,
            ApplyOutcome::Changed {
                cap: Some(1_000_000)
            }
        );
    }
}
