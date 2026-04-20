//! Background-integrity-sweeper configuration scaffolding (H14a).
//!
//! ## Purpose
//!
//! Host the **opt-in** [`IntegritySweeperConfig`] feature block, the
//! [`load_skip_list`] helper that parses a newline-delimited glob file,
//! and the [`RatedTokenBucket`] primitive the daemon worker uses to
//! throttle itself to a configured files-per-minute budget.
//!
//! ## Security posture
//!
//! - **Off by default.** [`IntegritySweeperConfig::default`] returns
//!   `enabled = false`, no schedule, and no skip list — a daemon that
//!   has never seen this block behaves exactly as it did before this
//!   module existed.
//! - **Fail-closed parser.** [`load_skip_list`] refuses an
//!   `io::ErrorKind::InvalidData` rather than silently dropping an
//!   unparseable glob; a typo must not cause the sweeper to scrub a
//!   file the operator believed was excluded.
//! - **Predictable ceilings.** [`RatedTokenBucket`] uses a
//!   per-minute token budget with a hard cap at the configured
//!   capacity, so a long host pause does not create an unbounded burst
//!   on resume.
//!
//! ## Scheduler + battery coupling
//!
//! - This module is still pure configuration — it never spawns a thread.
//!   The daemon worker that consumes the config lives in
//!   `pcloud_daemon::integrity_sweeper_service`.
//! - `schedule_cron` is **honoured** by the daemon scheduler: it is
//!   parsed via the `cron` crate and an invalid expression causes the
//!   shell to refuse to start. See the consumer module for the full
//!   scheduler semantics.
//! - `pause_on_battery` is **honoured** by the daemon scheduler: it
//!   consults a platform power-source reader (Linux `/sys/class/power_supply`,
//!   macOS/Windows via the `battery` crate) and skips the tick while
//!   any supply reports `Discharging`.
//!
//! See `docs/parity/integrity-sweeper.md` for the rollout plan.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::{
    fs,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use pcloud_observability::LockExt;
use serde::{Deserialize, Serialize};

/// Configuration block for the background integrity sweeper.
///
/// Persists as the `[features.integrity_sweeper]` table of the profile
/// envelope. Every field uses `#[serde(default)]` so older on-disk
/// envelopes that predate this block continue to load cleanly. The
/// defaults returned by [`IntegritySweeperConfig::default`] are
/// intentionally **off and safe**: nothing scrubs anything in the
/// background unless an operator explicitly opts in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegritySweeperConfig {
    /// Master switch. Default: `false`. Valid values: `true`, `false`.
    /// While `false`, no background worker is spawned and no I/O is
    /// performed by the sweeper subsystem regardless of the other
    /// fields. **Security:** keeping this `false` is the secure default
    /// because a background scrub touches every file in scope and may
    /// observably affect disk wear and battery on laptops.
    #[serde(default)]
    pub enabled: bool,
    /// Optional cron-style schedule string. Default: `None` — the
    /// sweeper runs **on demand only** when an operator triggers it
    /// through the IPC surface. When `Some`, the daemon scheduler
    /// thread uses this string as its periodic trigger, invoking
    /// `run_once` at each boundary. Accepts standard 6- or 7-field cron
    /// expressions per the [`cron` crate](https://docs.rs/cron) (second,
    /// minute, hour, dom, month, dow, and optional year).
    /// **Security:** an invalid cron expression is rejected at
    /// `IntegritySweeperShell::from_config` time; the scheduler refuses
    /// to start and the parse error is surfaced to the operator. This
    /// field is **wired** — the scheduler honours it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_cron: Option<String>,
    /// Token-bucket budget in files per minute. Default: `100`. The
    /// sweeper worker calls [`RatedTokenBucket::try_acquire`] before
    /// every file and skips work when no token is available, capping
    /// CPU and disk pressure to a predictable upper bound. A value of
    /// `0` disables work entirely (every `try_acquire` returns
    /// `false`).
    #[serde(default = "default_rate_files_per_minute")]
    pub rate_files_per_minute: u32,
    /// When `true`, the daemon scheduler pauses the sweep tick while the
    /// host reports running on battery. Default: `true`. On Linux the
    /// scheduler reads `/sys/class/power_supply/*/status`; on macOS and
    /// Windows it uses the `battery` crate. Platforms without a
    /// battery-state facade emit a one-shot warning and behave as if the
    /// flag were disabled (servers, VMs, and containers therefore keep
    /// running unchanged). This field is **wired** — the scheduler
    /// honours it.
    #[serde(default = "default_pause_on_battery")]
    pub pause_on_battery: bool,
    /// Optional path to a newline-delimited file of glob patterns the
    /// sweeper must exclude. Default: `None` (no skip list, every file
    /// in scope is eligible). Loaded by [`load_skip_list`]; invalid
    /// globs cause that helper to return an `io::Error` rather than
    /// silently dropping the offending entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_list_path: Option<PathBuf>,
}

const fn default_rate_files_per_minute() -> u32 {
    100
}

const fn default_pause_on_battery() -> bool {
    true
}

impl Default for IntegritySweeperConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            schedule_cron: None,
            rate_files_per_minute: default_rate_files_per_minute(),
            pause_on_battery: default_pause_on_battery(),
            skip_list_path: None,
        }
    }
}

/// Read a newline-delimited glob skip-list from `path` and return the
/// parsed [`glob::Pattern`] entries.
///
/// Lines that are empty or start with `#` are ignored. Any line that
/// fails [`glob::Pattern::new`] surfaces as an `io::Error` of kind
/// [`io::ErrorKind::InvalidData`] — invalid input is **not** silently
/// dropped, because a typo in a skip list could otherwise cause the
/// sweeper to scrub a file the operator believed was excluded.
///
/// # Errors
///
/// - [`io::ErrorKind::NotFound`] / other I/O errors propagate from
///   [`fs::File::open`] and the underlying read.
/// - [`io::ErrorKind::InvalidData`] when any non-comment, non-empty
///   line fails to parse as a glob pattern. The error message includes
///   the 1-based line number and the offending text.
pub fn load_skip_list(path: &Path) -> io::Result<Vec<glob::Pattern>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for (idx, line) in reader.lines().enumerate() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match glob::Pattern::new(trimmed) {
            Ok(p) => out.push(p),
            Err(e) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "invalid glob pattern at line {}: {:?} ({})",
                        idx + 1,
                        trimmed,
                        e
                    ),
                ));
            }
        }
    }
    Ok(out)
}

/// Monotonic clock abstraction used by [`RatedTokenBucket`] so tests
/// can advance time deterministically via [`ManualClock`].
///
/// The trait is intentionally tiny — only "what time is it now?". The
/// sweeper worker (PR2) will inject [`SystemClock`] in production and
/// a [`ManualClock`] in tests.
pub trait Clock: Send + Sync {
    /// Return the current monotonic instant.
    fn now(&self) -> Instant;
}

/// Default monotonic clock. Always reads [`Instant::now`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Test-only manual clock that returns the configured instant. Use
/// [`ManualClock::advance`] to move time forward in tests.
#[derive(Debug)]
pub struct ManualClock {
    inner: Mutex<Instant>,
}

impl ManualClock {
    /// Build a manual clock pinned at `start`.
    #[must_use]
    pub fn new(start: Instant) -> Self {
        Self {
            inner: Mutex::new(start),
        }
    }

    /// Advance the manual clock by `delta`.
    pub fn advance(&self, delta: Duration) {
        let mut g = self
            .inner
            .lock_or_poisoned("config::integrity_sweeper::ManualClock::advance");
        *g += delta;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        *self
            .inner
            .lock_or_poisoned("config::integrity_sweeper::ManualClock::now")
    }
}

/// A simple rate-limiter primitive backing the sweeper's
/// `rate_files_per_minute` budget.
///
/// Tokens accrue continuously at `tokens_per_minute / 60` per second up
/// to the configured per-minute capacity. [`RatedTokenBucket::try_acquire`]
/// consumes one token if available and otherwise returns `false`. The
/// implementation is **lock-based** rather than atomic-only because the
/// sweeper worker is not on a hot path and correctness on overflow is
/// easier to audit this way.
///
/// A `tokens_per_minute` value of `0` permanently disables work — every
/// `try_acquire` call returns `false`.
#[derive(Debug)]
pub struct RatedTokenBucket {
    tokens_per_minute: u32,
    state: Mutex<BucketState>,
}

#[derive(Debug)]
struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

impl RatedTokenBucket {
    /// Build a bucket that refills at `tokens_per_minute` per minute,
    /// using [`SystemClock`] internally and starting full.
    #[must_use]
    pub fn new(tokens_per_minute: u32) -> Self {
        Self::with_clock(tokens_per_minute, &SystemClock)
    }

    /// Build a bucket using the provided [`Clock`] for the initial
    /// timestamp. Useful from tests with a [`ManualClock`].
    #[must_use]
    pub fn with_clock(tokens_per_minute: u32, clock: &dyn Clock) -> Self {
        Self {
            tokens_per_minute,
            state: Mutex::new(BucketState {
                tokens: f64::from(tokens_per_minute),
                last_refill: clock.now(),
            }),
        }
    }

    /// Configured capacity in tokens per minute.
    #[must_use]
    pub const fn tokens_per_minute(&self) -> u32 {
        self.tokens_per_minute
    }

    /// Attempt to consume one token using the system clock.
    pub fn try_acquire(&self) -> bool {
        self.try_acquire_with(&SystemClock)
    }

    /// Attempt to consume one token using `clock` for the refill
    /// computation. Tests pass a [`ManualClock`] here.
    pub fn try_acquire_with(&self, clock: &dyn Clock) -> bool {
        if self.tokens_per_minute == 0 {
            return false;
        }
        let mut state = self
            .state
            .lock_or_poisoned("config::integrity_sweeper::RatedTokenBucket::try_acquire_with");
        let now = clock.now();
        let elapsed = now
            .saturating_duration_since(state.last_refill)
            .as_secs_f64();
        let per_second = f64::from(self.tokens_per_minute) / 60.0;
        let capacity = f64::from(self.tokens_per_minute);
        state.tokens = (state.tokens + elapsed * per_second).min(capacity);
        state.last_refill = now;
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn config_defaults_are_off_and_safe() {
        let cfg = IntegritySweeperConfig::default();
        assert!(!cfg.enabled, "sweeper must be off by default");
        assert!(
            cfg.schedule_cron.is_none(),
            "sweeper must be on-demand by default"
        );
        assert_eq!(cfg.rate_files_per_minute, 100);
        assert!(
            cfg.pause_on_battery,
            "battery pause must default to true (laptop-safe default)"
        );
        assert!(cfg.skip_list_path.is_none());
    }

    #[test]
    fn config_round_trips_through_serde_with_missing_block() {
        // Empty TOML-like JSON (no fields) must still deserialize via
        // serde defaults.
        let cfg: IntegritySweeperConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, IntegritySweeperConfig::default());
    }

    #[test]
    fn config_accepts_schedule_cron_string_verbatim() {
        // The config layer stores `schedule_cron` as an opaque string;
        // cron parsing happens in the daemon consumer. This test only
        // guards the serde contract so operator configs with cron
        // expressions round-trip without data loss.
        let src = r#"{"enabled":true,"schedule_cron":"0 0 3 * * *"}"#;
        let cfg: IntegritySweeperConfig = serde_json::from_str(src).unwrap();
        assert_eq!(cfg.schedule_cron.as_deref(), Some("0 0 3 * * *"));
    }

    #[test]
    fn skip_list_parses_globs_and_rejects_invalid() {
        let mut good = NamedTempFile::new().unwrap();
        writeln!(good, "# comment line").unwrap();
        writeln!(good).unwrap();
        writeln!(good, "**/*.tmp").unwrap();
        writeln!(good, "node_modules/**").unwrap();
        let parsed = load_skip_list(good.path()).unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].matches("foo/bar.tmp"));
        assert!(parsed[1].matches("node_modules/x/y"));

        // Unmatched bracket: glob crate rejects this.
        let mut bad = NamedTempFile::new().unwrap();
        writeln!(bad, "valid/**").unwrap();
        writeln!(bad, "broken[abc").unwrap();
        let err = load_skip_list(bad.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("line 2"),
            "error should pinpoint the offending line, got: {err}"
        );
    }

    #[test]
    fn skip_list_missing_file_propagates_io_error() {
        let err = load_skip_list(Path::new("/nonexistent/path/skip.list")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn rate_limiter_zero_capacity_never_emits() {
        let bucket = RatedTokenBucket::new(0);
        for _ in 0..10 {
            assert!(!bucket.try_acquire());
        }
    }

    #[test]
    fn rate_limiter_emits_expected_tokens_over_elapsed_time() {
        // 60 tokens/minute == 1 token/second. Start full, drain, then
        // advance the manual clock and verify the refill matches.
        let start = Instant::now();
        let clock = ManualClock::new(start);
        let bucket = RatedTokenBucket::with_clock(60, &clock);

        // Drain all 60 starting tokens.
        let mut emitted = 0;
        for _ in 0..200 {
            if bucket.try_acquire_with(&clock) {
                emitted += 1;
            } else {
                break;
            }
        }
        assert_eq!(emitted, 60, "should drain initial capacity exactly");
        assert!(!bucket.try_acquire_with(&clock), "bucket must be empty");

        // Advance 5s -> +5 tokens.
        clock.advance(Duration::from_secs(5));
        let mut after_5s = 0;
        for _ in 0..10 {
            if bucket.try_acquire_with(&clock) {
                after_5s += 1;
            } else {
                break;
            }
        }
        assert_eq!(after_5s, 5, "expected exactly 5 tokens after 5s");

        // Advance well past capacity -> capped at 60.
        clock.advance(Duration::from_secs(600));
        let mut after_overflow = 0;
        for _ in 0..200 {
            if bucket.try_acquire_with(&clock) {
                after_overflow += 1;
            } else {
                break;
            }
        }
        assert_eq!(
            after_overflow, 60,
            "bucket must cap at configured per-minute capacity"
        );
    }
}
