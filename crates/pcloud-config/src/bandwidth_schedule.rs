//! T1.4 — Time-of-day + metered-network bandwidth scheduling.
//!
//! # Why a separate module
//!
//! The base bandwidth cap is already a single `Option<u64>` carried by
//! `pcloud_engine::transfers::bandwidth::BandwidthLimiter` (and shared
//! with the proto/backends through the `BandwidthPacer`). This module
//! adds **conditional** caps: a list of time-of-day rules plus an
//! optional metered-network override. The decision function
//! `BandwidthScheduleConfig::current_cap` collapses the rules + the
//! current minute-of-day + the metered hint into a single
//! `Option<u64>` that the daemon hands to
//! `BandwidthPacer::set_limit`.
//!
//! Keeping the rule set in `pcloud-config` (instead of `pcloud-engine`)
//! means the schedule loads from TOML alongside the rest of the daemon
//! config and validates at bootstrap time.
//!
//! # Decision precedence (T1.4 plan)
//!
//! 1. If the host is currently on a metered network and
//!    `metered_cap_bytes_per_sec` is `Some(n)`, that cap wins
//!    unconditionally.
//! 2. Otherwise the **first matching** rule (in declaration order) at
//!    the current minute-of-day on the current weekday wins.
//! 3. Otherwise `default_cap_bytes_per_sec` (which may itself be
//!    `None`, meaning unlimited) wins.
//!
//! Wrap-around windows (e.g. 22:00 → 06:00 = "overnight quiet hours")
//! are supported by allowing `end_minute_of_day < start_minute_of_day`.
//!
//! # Wire shape
//!
//! ```toml
//! [bandwidth.schedule]
//! enabled = true
//! default_cap_bytes_per_sec = 5_000_000   # 5 MB/s baseline
//! metered_cap_bytes_per_sec = 256_000     # 256 KB/s on metered links
//!
//! [[bandwidth.schedule.rules]]
//! # Quiet hours: 22:00 → 06:00 every day, throttle to 1 MB/s.
//! start_minute_of_day = 1320
//! end_minute_of_day   = 360
//! cap_bytes_per_sec   = 1_000_000
//!
//! [[bandwidth.schedule.rules]]
//! # Workday boost: Mon-Fri 09:00 → 17:00, unlimited.
//! start_minute_of_day = 540
//! end_minute_of_day   = 1020
//! cap_bytes_per_sec   = 0          # 0 = unlimited
//! days = ["mon", "tue", "wed", "thu", "fri"]
//! ```
//!
//! `cap_bytes_per_sec = 0` means "unlimited for this window" so a rule
//! can punch through a tighter `default_cap_bytes_per_sec`.

// **PLATFORM:** all
// **GATING:** none (portable; metered-network detection is wired
// elsewhere — this module only consumes the boolean hint).

use serde::{Deserialize, Serialize};

/// Number of minutes in a day. `start_minute_of_day` and
/// `end_minute_of_day` must be strictly less than this.
pub const MINUTES_PER_DAY: u32 = 24 * 60;

/// Day-of-week selector for [`BandwidthRule::days`].
///
/// Serialized as a lowercase three-letter abbreviation (`"mon"`,
/// `"tue"`, …) so TOML files stay readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    /// Monday.
    Mon,
    /// Tuesday.
    Tue,
    /// Wednesday.
    Wed,
    /// Thursday.
    Thu,
    /// Friday.
    Fri,
    /// Saturday.
    Sat,
    /// Sunday.
    Sun,
}

impl Weekday {
    /// Map a `chrono`-style weekday integer (`0` = Monday … `6` =
    /// Sunday) to the enum. Returns `None` outside that range.
    #[must_use]
    pub fn from_iso(idx: u8) -> Option<Self> {
        match idx {
            0 => Some(Self::Mon),
            1 => Some(Self::Tue),
            2 => Some(Self::Wed),
            3 => Some(Self::Thu),
            4 => Some(Self::Fri),
            5 => Some(Self::Sat),
            6 => Some(Self::Sun),
            _ => None,
        }
    }
}

/// One time-of-day bandwidth rule. The window is `[start, end)`
/// minutes-of-day inclusive-exclusive, where `start > end` means the
/// window wraps past midnight.
///
/// `cap_bytes_per_sec`:
/// - `Some(0)` → unlimited inside the window (overrides a tighter
///   default cap).
/// - `Some(n)` → cap to `n` bytes/sec inside the window.
/// - `None` → defer to `default_cap_bytes_per_sec` (rule is a no-op
///   for the cap; useful for documenting "the default applies here").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BandwidthRule {
    /// Inclusive start minute-of-day, `0..=1439`.
    pub start_minute_of_day: u32,
    /// Exclusive end minute-of-day, `0..=1439`. May be less than
    /// `start_minute_of_day` to express a wrap-around window
    /// (e.g. 22:00 → 06:00).
    pub end_minute_of_day: u32,
    /// Cap in bytes per second inside the window. `Some(0)` means
    /// unlimited. `None` defers to the schedule default.
    #[serde(default)]
    pub cap_bytes_per_sec: Option<u64>,
    /// Days of the week this rule applies to. Empty vec = every day
    /// (the convenient default for "always" rules like quiet hours).
    #[serde(default)]
    pub days: Vec<Weekday>,
}

impl BandwidthRule {
    /// Returns `true` if `(weekday, minute)` falls inside this rule's
    /// window.
    #[must_use]
    pub fn matches(&self, weekday: Weekday, minute: u32) -> bool {
        if !self.days.is_empty() && !self.days.contains(&weekday) {
            return false;
        }
        if self.start_minute_of_day == self.end_minute_of_day {
            // Zero-width window matches nothing. Validation rejects
            // this shape, but `matches` stays defensive.
            return false;
        }
        if self.start_minute_of_day < self.end_minute_of_day {
            // Same-day window.
            minute >= self.start_minute_of_day && minute < self.end_minute_of_day
        } else {
            // Wrap-around window: matches the tail of one day OR the
            // head of the next. Note: weekday enforcement uses the
            // *current* weekday — a 22:00→06:00 rule with
            // `days = ["mon"]` is interpreted as "starts at Mon 22:00,
            // continues into Tue 06:00", and the Mon-only filter
            // means the rule is OFF on Tuesday morning. Operators who
            // need explicit early-Tue coverage should add a second
            // rule. Documented because it is non-obvious.
            minute >= self.start_minute_of_day || minute < self.end_minute_of_day
        }
    }
}

/// `[bandwidth.schedule]` config section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BandwidthScheduleConfig {
    /// Master switch. When `false` the daemon never consults this
    /// config and the base `BandwidthLimiter` cap (set elsewhere)
    /// applies unconditionally.
    #[serde(default)]
    pub enabled: bool,
    /// Cap applied when no rule matches the current
    /// `(weekday, minute)`. `None` means unlimited.
    #[serde(default)]
    pub default_cap_bytes_per_sec: Option<u64>,
    /// Cap applied when the host reports that the current network is
    /// metered. Wins unconditionally over the time-rule decision when
    /// the daemon's metered detector reports `true`. `None` disables
    /// metered-aware throttling.
    #[serde(default)]
    pub metered_cap_bytes_per_sec: Option<u64>,
    /// Time-of-day rules, evaluated in declaration order. The first
    /// matching rule wins.
    #[serde(default)]
    pub rules: Vec<BandwidthRule>,
}

impl BandwidthScheduleConfig {
    /// Decide the current cap given the host's wall-clock minute and
    /// metered-network hint.
    ///
    /// Returns `None` when bandwidth is unlimited under the active
    /// decision. The `cap_bytes_per_sec = Some(0)` sentinel inside a
    /// rule is normalised here back to `None` (= unlimited).
    ///
    /// When `enabled = false`, returns `None` and the caller should
    /// fall back to its base limiter setting.
    #[must_use]
    pub fn current_cap(&self, weekday: Weekday, minute: u32, on_metered: bool) -> Option<u64> {
        if !self.enabled {
            return None;
        }
        if on_metered {
            if let Some(n) = self.metered_cap_bytes_per_sec {
                return Self::normalize_cap(Some(n));
            }
        }
        for rule in &self.rules {
            if rule.matches(weekday, minute) {
                if let Some(n) = rule.cap_bytes_per_sec {
                    return Self::normalize_cap(Some(n));
                }
                // Rule matched but explicitly defers to the default.
                break;
            }
        }
        Self::normalize_cap(self.default_cap_bytes_per_sec)
    }

    /// `Some(0)` is the operator-facing sentinel for "unlimited inside
    /// this window", which collapses to `None` at the limiter API.
    fn normalize_cap(raw: Option<u64>) -> Option<u64> {
        match raw {
            Some(0) => None,
            other => other,
        }
    }

    /// Validate the schedule. Rejects out-of-range minutes and
    /// zero-width rules. Overlapping rules are intentionally NOT
    /// rejected — the "first match wins" semantics make overlap
    /// meaningful (a more-specific rule before a catch-all).
    ///
    /// # Errors
    ///
    /// Returns a static description of the first violation.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.enabled {
            // Schedule off → no-op; do not penalise stray rules.
            return Ok(());
        }
        for rule in &self.rules {
            if rule.start_minute_of_day >= MINUTES_PER_DAY {
                return Err("bandwidth.schedule: start_minute_of_day must be less than 1440");
            }
            if rule.end_minute_of_day >= MINUTES_PER_DAY {
                return Err("bandwidth.schedule: end_minute_of_day must be less than 1440");
            }
            if rule.start_minute_of_day == rule.end_minute_of_day {
                return Err(
                    "bandwidth.schedule: a rule must have a non-empty window (start != end)",
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(start: u32, end: u32, cap: Option<u64>) -> BandwidthRule {
        BandwidthRule {
            start_minute_of_day: start,
            end_minute_of_day: end,
            cap_bytes_per_sec: cap,
            days: Vec::new(),
        }
    }

    #[test]
    fn default_is_disabled_and_unlimited() {
        let cfg = BandwidthScheduleConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.current_cap(Weekday::Wed, 600, false), None);
    }

    #[test]
    fn disabled_short_circuits_even_with_rules() {
        let cfg = BandwidthScheduleConfig {
            enabled: false,
            default_cap_bytes_per_sec: Some(1024),
            rules: vec![rule(0, 1440 - 1, Some(2048))],
            ..Default::default()
        };
        assert_eq!(cfg.current_cap(Weekday::Wed, 600, false), None);
    }

    #[test]
    fn metered_overrides_time_rule() {
        let cfg = BandwidthScheduleConfig {
            enabled: true,
            default_cap_bytes_per_sec: Some(5_000_000),
            metered_cap_bytes_per_sec: Some(256_000),
            rules: vec![rule(0, 1439, Some(0))], // unlimited window
        };
        // Without metered: rule's `Some(0)` → unlimited.
        assert_eq!(cfg.current_cap(Weekday::Wed, 600, false), None);
        // With metered: 256 KB/s wins.
        assert_eq!(cfg.current_cap(Weekday::Wed, 600, true), Some(256_000));
    }

    #[test]
    fn first_matching_rule_wins() {
        let cfg = BandwidthScheduleConfig {
            enabled: true,
            default_cap_bytes_per_sec: Some(5_000_000),
            rules: vec![
                rule(540, 1020, Some(10_000_000)),
                rule(0, 1439, Some(1_000_000)),
            ],
            ..Default::default()
        };
        // Inside the first rule's window: 10 MB/s.
        assert_eq!(cfg.current_cap(Weekday::Wed, 600, false), Some(10_000_000));
        // Outside the first rule but inside the second: 1 MB/s.
        assert_eq!(cfg.current_cap(Weekday::Wed, 1200, false), Some(1_000_000));
    }

    #[test]
    fn wrap_around_window_matches_both_halves() {
        let cfg = BandwidthScheduleConfig {
            enabled: true,
            default_cap_bytes_per_sec: Some(5_000_000),
            rules: vec![rule(1320, 360, Some(1_000_000))], // 22:00 → 06:00
            ..Default::default()
        };
        // 23:00 — inside the tail half.
        assert_eq!(cfg.current_cap(Weekday::Wed, 1380, false), Some(1_000_000));
        // 03:00 — inside the head half (next-day continuation).
        assert_eq!(cfg.current_cap(Weekday::Wed, 180, false), Some(1_000_000));
        // 12:00 — outside the wrap window, default applies.
        assert_eq!(cfg.current_cap(Weekday::Wed, 720, false), Some(5_000_000));
    }

    #[test]
    fn weekday_filter_excludes_non_listed_days() {
        let workday = BandwidthRule {
            start_minute_of_day: 540,
            end_minute_of_day: 1020,
            cap_bytes_per_sec: Some(10_000_000),
            days: vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
            ],
        };
        let cfg = BandwidthScheduleConfig {
            enabled: true,
            default_cap_bytes_per_sec: Some(1_000_000),
            rules: vec![workday],
            ..Default::default()
        };
        // Wednesday at noon: workday rule fires.
        assert_eq!(cfg.current_cap(Weekday::Wed, 720, false), Some(10_000_000));
        // Saturday at noon: not a workday → default applies.
        assert_eq!(cfg.current_cap(Weekday::Sat, 720, false), Some(1_000_000));
    }

    #[test]
    fn rule_cap_zero_means_unlimited() {
        let cfg = BandwidthScheduleConfig {
            enabled: true,
            default_cap_bytes_per_sec: Some(1_000_000),
            rules: vec![rule(540, 1020, Some(0))], // unlimited boost window
            ..Default::default()
        };
        assert_eq!(cfg.current_cap(Weekday::Wed, 720, false), None);
    }

    #[test]
    fn rule_cap_none_defers_to_default() {
        let cfg = BandwidthScheduleConfig {
            enabled: true,
            default_cap_bytes_per_sec: Some(2_000_000),
            rules: vec![rule(540, 1020, None)], // documents "default applies"
            ..Default::default()
        };
        assert_eq!(cfg.current_cap(Weekday::Wed, 720, false), Some(2_000_000));
    }

    #[test]
    fn validate_rejects_out_of_range_minute() {
        let cfg = BandwidthScheduleConfig {
            enabled: true,
            rules: vec![rule(1500, 0, Some(1_000_000))],
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_zero_width_window() {
        let cfg = BandwidthScheduleConfig {
            enabled: true,
            rules: vec![rule(720, 720, Some(1_000_000))],
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_disabled_skips_rule_check() {
        let cfg = BandwidthScheduleConfig {
            enabled: false,
            rules: vec![rule(1500, 0, Some(1_000_000))],
            ..Default::default()
        };
        // Even with malformed rules, disabled config is valid — the
        // operator may keep them around for a later toggle.
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn weekday_from_iso_round_trips() {
        for (idx, expected) in [
            (0, Weekday::Mon),
            (1, Weekday::Tue),
            (5, Weekday::Sat),
            (6, Weekday::Sun),
        ] {
            assert_eq!(Weekday::from_iso(idx), Some(expected));
        }
        assert_eq!(Weekday::from_iso(7), None);
    }

    #[test]
    fn serde_roundtrip_default_is_minimal() {
        let cfg = BandwidthScheduleConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: BandwidthScheduleConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn serde_roundtrip_with_rule() {
        let cfg = BandwidthScheduleConfig {
            enabled: true,
            default_cap_bytes_per_sec: Some(5_000_000),
            metered_cap_bytes_per_sec: Some(256_000),
            rules: vec![BandwidthRule {
                start_minute_of_day: 1320,
                end_minute_of_day: 360,
                cap_bytes_per_sec: Some(1_000_000),
                days: vec![Weekday::Mon, Weekday::Sun],
            }],
        };
        cfg.validate().unwrap();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: BandwidthScheduleConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
