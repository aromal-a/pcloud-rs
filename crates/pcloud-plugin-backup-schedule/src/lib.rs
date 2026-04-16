#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]

//! Backup scheduler plugin.
//!
//! This crate implements a user-level cron-like scheduler that runs inside
//! the pcloud-rs daemon and issues [`PluginOperation::RequestSyncResume`]
//! against configured sync roots on a user-defined cadence.
//!
//! # Schedule DSL
//!
//! Two schedule formats are accepted:
//!
//! 1. Native cron (5-field POSIX-like): `"0 18 * * 5"` — Fridays at 18:00.
//! 2. A small natural-language DSL built from a whitelisted verb set:
//!    `every`, `hourly`, `daily`, `weekly`, `monthly`, `at`, `on`.
//!    Examples:
//!    - `"hourly"`
//!    - `"daily at 03:00"`
//!    - `"weekly on monday at 09:15"`
//!    - `"every friday 18:00"`
//!
//! The natural form is translated to a canonical cron expression; the
//! parser rejects any token outside the whitelist so the DSL cannot grow
//! into a shell-like mini-language by accident.
//!
//! # Security posture
//!
//! * `#![forbid(unsafe_code)]` — no unsafe in the crate.
//! * The plugin only ever emits [`PluginOperation::RequestSyncResume`];
//!   it cannot pause, read secrets, or exfiltrate state. The host
//!   continues to enforce capability checks at dispatch time.
//! * The maximum number of configured schedules is bounded
//!   ([`MAX_SCHEDULES`]) to prevent pathological configs from eating
//!   scheduler time.

use std::collections::VecDeque;

use chrono::{DateTime, TimeZone, Utc};
use cron::Schedule as CronSchedule;
use pcloud_plugin_api::{
    Plugin, PluginCapability, PluginContext, PluginError, PluginManifest, PluginOperation,
    PluginOperationResponse, PluginSignature,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::str::FromStr;

/// Maximum number of schedules a single config may contain.
pub const MAX_SCHEDULES: usize = 32;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced by the backup-schedule plugin.
#[derive(Debug, thiserror::Error)]
pub enum BackupScheduleError {
    /// The schedule string could not be parsed as either a cron expression
    /// or a whitelisted natural-language phrase.
    #[error("invalid schedule expression: {0}")]
    InvalidSchedule(String),

    /// Configuration would exceed [`MAX_SCHEDULES`].
    #[error("too many schedules configured: max = {max}, got = {got}")]
    TooMany {
        /// Maximum supported schedules.
        max: usize,
        /// Number the caller attempted to configure.
        got: usize,
    },

    /// Two schedules share the same `name`.
    #[error("duplicate schedule name: {0}")]
    DuplicateName(String),

    /// Name passed to a remove/update API was not present in the config.
    #[error("no schedule found with name: {0}")]
    NotFound(String),

    /// Plugin initialization failed.
    #[error("initialization failed: {0}")]
    Initialization(String),
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// One schedule entry as materialized from `[plugins.backup_schedule]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleEntry {
    /// Human-readable, unique (within the config) identifier.
    pub name: String,
    /// Raw schedule expression (cron or natural DSL).
    pub schedule: String,
    /// Sync root id this schedule targets.
    pub sync_root_id: u64,
    /// If `false`, the entry is parsed and kept but never fires.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Full plugin configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupScheduleConfig {
    /// Ordered list of schedule entries.
    #[serde(default)]
    pub entries: Vec<ScheduleEntry>,
}

impl BackupScheduleConfig {
    /// Validate that the config obeys [`MAX_SCHEDULES`] and has unique
    /// names. Does not parse the schedule strings themselves; use
    /// [`parse_schedule`] for that.
    pub fn validate(&self) -> Result<(), BackupScheduleError> {
        if self.entries.len() > MAX_SCHEDULES {
            return Err(BackupScheduleError::TooMany {
                max: MAX_SCHEDULES,
                got: self.entries.len(),
            });
        }
        let mut seen = BTreeSet::new();
        for e in &self.entries {
            if !seen.insert(e.name.clone()) {
                return Err(BackupScheduleError::DuplicateName(e.name.clone()));
            }
        }
        Ok(())
    }

    /// Add an entry, enforcing uniqueness and the 32-schedule cap.
    pub fn add(&mut self, entry: ScheduleEntry) -> Result<(), BackupScheduleError> {
        if self.entries.iter().any(|e| e.name == entry.name) {
            return Err(BackupScheduleError::DuplicateName(entry.name));
        }
        if self.entries.len() >= MAX_SCHEDULES {
            return Err(BackupScheduleError::TooMany {
                max: MAX_SCHEDULES,
                got: self.entries.len() + 1,
            });
        }
        // Validate the schedule parses before inserting.
        let _ = parse_schedule(&entry.schedule)?;
        self.entries.push(entry);
        Ok(())
    }

    /// Remove an entry by name.
    pub fn remove(&mut self, name: &str) -> Result<ScheduleEntry, BackupScheduleError> {
        let idx = self
            .entries
            .iter()
            .position(|e| e.name == name)
            .ok_or_else(|| BackupScheduleError::NotFound(name.to_string()))?;
        Ok(self.entries.remove(idx))
    }

    /// Iterate entries.
    pub fn iter(&self) -> std::slice::Iter<'_, ScheduleEntry> {
        self.entries.iter()
    }
}

// ---------------------------------------------------------------------------
// Clock abstraction (so tests are deterministic)
// ---------------------------------------------------------------------------

/// A monotonic-ish wall-clock source the plugin uses to evaluate
/// schedules. Production uses [`SystemClock`]; tests use [`ManualClock`].
pub trait Clock: Send {
    /// Current wall-clock time (UTC).
    fn now(&self) -> DateTime<Utc>;
}

/// Real system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Test-controllable clock.
#[derive(Debug, Clone)]
pub struct ManualClock {
    current: DateTime<Utc>,
}

impl ManualClock {
    /// Start at the given UTC time.
    pub fn new(start: DateTime<Utc>) -> Self {
        Self { current: start }
    }

    /// Advance by `secs` seconds.
    pub fn advance_secs(&mut self, secs: i64) {
        self.current += chrono::Duration::seconds(secs);
    }

    /// Jump to an absolute time.
    pub fn set(&mut self, t: DateTime<Utc>) {
        self.current = t;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        self.current
    }
}

// ---------------------------------------------------------------------------
// DSL / cron parsing
// ---------------------------------------------------------------------------

/// A parsed, validated schedule.
#[derive(Debug, Clone)]
pub struct ParsedSchedule {
    cron_expr: String,
    schedule: CronSchedule,
}

impl ParsedSchedule {
    /// The canonical cron expression (7-field form used by the `cron` crate
    /// internally: sec min hour dom mon dow year).
    pub fn as_cron(&self) -> &str {
        &self.cron_expr
    }

    /// Return the next firing time strictly after `after`, if any.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.schedule.after(&after).next()
    }
}

/// Parse either a 5-field POSIX-like cron expression, a 6- or 7-field
/// extended cron expression, or a natural-language phrase drawn from
/// the whitelisted verb set.
pub fn parse_schedule(input: &str) -> Result<ParsedSchedule, BackupScheduleError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(BackupScheduleError::InvalidSchedule(
            "empty expression".into(),
        ));
    }

    // Try cron first (covers 5/6/7 field forms).
    if looks_like_cron(trimmed) {
        let canonical = canonicalize_cron(trimmed)?;
        let sched = CronSchedule::from_str(&canonical)
            .map_err(|e| BackupScheduleError::InvalidSchedule(format!("cron: {e}")))?;
        return Ok(ParsedSchedule {
            cron_expr: canonical,
            schedule: sched,
        });
    }

    // Otherwise natural DSL.
    let canonical = natural_to_cron(trimmed)?;
    let sched = CronSchedule::from_str(&canonical).map_err(|e| {
        BackupScheduleError::InvalidSchedule(format!("dsl translated to bad cron: {e}"))
    })?;
    Ok(ParsedSchedule {
        cron_expr: canonical,
        schedule: sched,
    })
}

fn looks_like_cron(s: &str) -> bool {
    // Heuristic: cron is a sequence of whitespace-separated tokens where
    // each token begins with a digit, `*`, or `?`. Natural DSL starts
    // with letters (e.g. "every", "hourly"). We also allow `/` `-` `,`
    // inside tokens.
    let first = s.split_whitespace().next().unwrap_or("");
    let field_count = s.split_whitespace().count();
    if !(5..=7).contains(&field_count) {
        return false;
    }
    first
        .chars()
        .next()
        .map(|c| c.is_ascii_digit() || c == '*' || c == '?')
        .unwrap_or(false)
}

fn canonicalize_cron(s: &str) -> Result<String, BackupScheduleError> {
    // The `cron` crate expects 7 fields: sec min hour dom mon dow year.
    // We accept the common 5-field form `min hour dom mon dow` by
    // prepending `0` (seconds) and appending `*` (year). 6-field forms
    // (with seconds) have `0 ..` prepended with year=*. 7-field forms
    // are passed through.
    let fields: Vec<&str> = s.split_whitespace().collect();
    let canonical = match fields.len() {
        5 => format!("0 {} *", fields.join(" ")),
        6 => format!("{} *", fields.join(" ")),
        7 => fields.join(" "),
        n => {
            return Err(BackupScheduleError::InvalidSchedule(format!(
                "expected 5, 6, or 7 cron fields, got {n}"
            )));
        }
    };
    Ok(canonical)
}

/// Translate a whitelisted natural-language expression to a cron string.
/// Grammar (case-insensitive):
///
/// ```text
/// expr      := "hourly"
///            | "daily" [ "at" time ]
///            | "weekly" [ "on" dow ] [ "at" time ]
///            | "monthly" [ "on" dom ] [ "at" time ]
///            | "every" dow [ "at" time | time ]
/// time      := HH ":" MM
/// dow       := monday | tuesday | ... | sunday
/// dom       := 1..=31
/// ```
fn natural_to_cron(input: &str) -> Result<String, BackupScheduleError> {
    let lower = input.to_ascii_lowercase();
    let mut tokens: VecDeque<&str> = lower.split_whitespace().collect();

    let verb = tokens
        .pop_front()
        .ok_or_else(|| BackupScheduleError::InvalidSchedule("empty".into()))?;

    // Accept only the whitelisted verb set for the leading token.
    match verb {
        "hourly" => {
            reject_trailing(&tokens)?;
            // minute 0 every hour
            Ok("0 0 * * * * *".into())
        }
        "daily" => {
            let (hh, mm) = opt_at_time(&mut tokens)?.unwrap_or((0, 0));
            reject_trailing(&tokens)?;
            Ok(format!("0 {mm} {hh} * * * *"))
        }
        "weekly" => {
            let dow = opt_on_dow(&mut tokens)?.unwrap_or(1); // default Monday
            let (hh, mm) = opt_at_time(&mut tokens)?.unwrap_or((0, 0));
            reject_trailing(&tokens)?;
            Ok(format!("0 {mm} {hh} * * {} *", dow_name(dow)))
        }
        "monthly" => {
            let dom = opt_on_dom(&mut tokens)?.unwrap_or(1);
            let (hh, mm) = opt_at_time(&mut tokens)?.unwrap_or((0, 0));
            reject_trailing(&tokens)?;
            Ok(format!("0 {mm} {hh} {dom} * * *"))
        }
        "every" => {
            // "every <dow> [at] HH:MM" or "every <dow> HH:MM"
            let dow_tok = tokens.pop_front().ok_or_else(|| {
                BackupScheduleError::InvalidSchedule("every: missing day-of-week".into())
            })?;
            let dow = parse_dow(dow_tok)?;
            // Optional "at" — consume if present.
            if tokens.front().copied() == Some("at") {
                tokens.pop_front();
            }
            let (hh, mm) = if let Some(tok) = tokens.pop_front() {
                parse_hhmm(tok)?
            } else {
                (0, 0)
            };
            reject_trailing(&tokens)?;
            Ok(format!("0 {mm} {hh} * * {} *", dow_name(dow)))
        }
        other => Err(BackupScheduleError::InvalidSchedule(format!(
            "unrecognized verb '{other}' (allowed: every, hourly, daily, weekly, monthly, at, on)"
        ))),
    }
}

fn reject_trailing(tokens: &VecDeque<&str>) -> Result<(), BackupScheduleError> {
    if let Some(extra) = tokens.front() {
        return Err(BackupScheduleError::InvalidSchedule(format!(
            "unexpected trailing token: {extra}"
        )));
    }
    Ok(())
}

fn opt_at_time(tokens: &mut VecDeque<&str>) -> Result<Option<(u32, u32)>, BackupScheduleError> {
    if tokens.front().copied() == Some("at") {
        tokens.pop_front();
        let tok = tokens.pop_front().ok_or_else(|| {
            BackupScheduleError::InvalidSchedule("expected HH:MM after 'at'".into())
        })?;
        return Ok(Some(parse_hhmm(tok)?));
    }
    Ok(None)
}

fn opt_on_dow(tokens: &mut VecDeque<&str>) -> Result<Option<u32>, BackupScheduleError> {
    if tokens.front().copied() == Some("on") {
        tokens.pop_front();
        let tok = tokens.pop_front().ok_or_else(|| {
            BackupScheduleError::InvalidSchedule("expected day after 'on'".into())
        })?;
        return Ok(Some(parse_dow(tok)?));
    }
    Ok(None)
}

fn opt_on_dom(tokens: &mut VecDeque<&str>) -> Result<Option<u32>, BackupScheduleError> {
    if tokens.front().copied() == Some("on") {
        tokens.pop_front();
        let tok = tokens.pop_front().ok_or_else(|| {
            BackupScheduleError::InvalidSchedule("expected day after 'on'".into())
        })?;
        let n: u32 = tok.parse().map_err(|_| {
            BackupScheduleError::InvalidSchedule(format!("day-of-month not an integer: {tok}"))
        })?;
        if !(1..=31).contains(&n) {
            return Err(BackupScheduleError::InvalidSchedule(format!(
                "day-of-month out of range: {n}"
            )));
        }
        return Ok(Some(n));
    }
    Ok(None)
}

fn parse_hhmm(tok: &str) -> Result<(u32, u32), BackupScheduleError> {
    let (hh, mm) = tok.split_once(':').ok_or_else(|| {
        BackupScheduleError::InvalidSchedule(format!("expected HH:MM, got {tok}"))
    })?;
    let h: u32 = hh
        .parse()
        .map_err(|_| BackupScheduleError::InvalidSchedule(format!("bad hour: {hh}")))?;
    let m: u32 = mm
        .parse()
        .map_err(|_| BackupScheduleError::InvalidSchedule(format!("bad minute: {mm}")))?;
    if h > 23 {
        return Err(BackupScheduleError::InvalidSchedule(format!(
            "hour out of range: {h}"
        )));
    }
    if m > 59 {
        return Err(BackupScheduleError::InvalidSchedule(format!(
            "minute out of range: {m}"
        )));
    }
    Ok((h, m))
}

fn parse_dow(tok: &str) -> Result<u32, BackupScheduleError> {
    match tok {
        "sunday" | "sun" => Ok(0),
        "monday" | "mon" => Ok(1),
        "tuesday" | "tue" | "tues" => Ok(2),
        "wednesday" | "wed" => Ok(3),
        "thursday" | "thu" | "thur" | "thurs" => Ok(4),
        "friday" | "fri" => Ok(5),
        "saturday" | "sat" => Ok(6),
        _ => Err(BackupScheduleError::InvalidSchedule(format!(
            "unknown day-of-week: {tok}"
        ))),
    }
}

fn dow_name(n: u32) -> &'static str {
    match n {
        0 => "SUN",
        1 => "MON",
        2 => "TUE",
        3 => "WED",
        4 => "THU",
        5 => "FRI",
        6 => "SAT",
        _ => "MON",
    }
}

// ---------------------------------------------------------------------------
// Scheduler core
// ---------------------------------------------------------------------------

/// Runtime state for a single scheduled entry.
#[derive(Debug, Clone)]
struct RuntimeEntry {
    entry: ScheduleEntry,
    parsed: ParsedSchedule,
    /// Last tick observed; used to detect boundaries that fell between
    /// `last_tick` and `now`.
    last_tick: DateTime<Utc>,
}

/// The backup scheduler plugin.
pub struct BackupSchedulePlugin {
    entries: Vec<RuntimeEntry>,
    pending: VecDeque<PluginOperation>,
    clock: Box<dyn Clock>,
    started_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for BackupSchedulePlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackupSchedulePlugin")
            .field("entries", &self.entries)
            .field("pending", &self.pending.len())
            .field("started_at", &self.started_at)
            .finish()
    }
}

impl BackupSchedulePlugin {
    /// Build a new plugin from a validated config. Uses [`SystemClock`].
    pub fn new(config: BackupScheduleConfig) -> Result<Self, BackupScheduleError> {
        Self::new_with_clock(config, Box::new(SystemClock))
    }

    /// Build with an explicit clock (used in tests).
    pub fn new_with_clock(
        config: BackupScheduleConfig,
        clock: Box<dyn Clock>,
    ) -> Result<Self, BackupScheduleError> {
        config.validate()?;
        let now = clock.now();
        let mut entries = Vec::with_capacity(config.entries.len());
        for e in config.entries {
            let parsed = parse_schedule(&e.schedule)?;
            entries.push(RuntimeEntry {
                entry: e,
                parsed,
                last_tick: now,
            });
        }
        Ok(Self {
            entries,
            pending: VecDeque::new(),
            clock,
            started_at: Some(now),
        })
    }

    /// Feed a wall-clock tick to the scheduler. Any schedules whose next
    /// firing moment falls in `(last_tick, now]` are queued as
    /// [`PluginOperation::RequestSyncResume`].
    pub fn tick(&mut self) {
        let now = self.clock.now();
        for re in self.entries.iter_mut() {
            if !re.entry.enabled {
                re.last_tick = now;
                continue;
            }
            // Walk forward from last_tick until we pass `now`, enqueueing
            // every boundary crossed. Typically zero or one per tick; the
            // loop guards against long gaps (e.g. suspended host).
            let mut cursor = re.last_tick;
            let mut guard = 0u32;
            while let Some(next) = re.parsed.next_after(cursor) {
                if next > now {
                    break;
                }
                self.pending.push_back(PluginOperation::RequestSyncResume {
                    sync_root_id: re.entry.sync_root_id,
                });
                cursor = next;
                guard += 1;
                if guard > 1024 {
                    // Defensive cap: never let one entry monopolize a tick.
                    break;
                }
            }
            re.last_tick = now;
        }
    }

    /// Number of operations currently queued and waiting for the host to
    /// pull them via [`Plugin::next_operation`].
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Direct test hook: expose the current entries.
    pub fn entries(&self) -> impl Iterator<Item = &ScheduleEntry> {
        self.entries.iter().map(|re| &re.entry)
    }
}

impl Plugin for BackupSchedulePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "pcloud-plugin-backup-schedule".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            display_name: "Backup Schedule".into(),
            requested_capabilities: BTreeSet::from([PluginCapability::SyncControl]),
        }
    }

    fn signature(&self) -> Option<PluginSignature> {
        None
    }

    fn on_load(&mut self, _context: &PluginContext) -> Result<(), PluginError> {
        // Nothing to initialize beyond what the constructor did.
        Ok(())
    }

    fn next_operation(&mut self) -> Option<PluginOperation> {
        // Opportunistically tick on every poll; the host's poll cadence
        // is the effective resolution of the scheduler.
        self.tick();
        self.pending.pop_front()
    }

    fn on_response(&mut self, _response: &PluginOperationResponse) {
        // We fire-and-forget: a `SyncControlAck` simply means the host
        // accepted the request.
    }
}

// ---------------------------------------------------------------------------
// CLI-facing helpers
// ---------------------------------------------------------------------------

/// Commands the CLI surface (`pcloudc backup schedule ...`) issues
/// against the backend. Kept as a plain enum so it can be serialized over
/// the existing IPC channel without pulling in CLI types here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BackupScheduleCliCommand {
    /// List all schedules currently in config.
    List,
    /// Add a schedule.
    Add {
        /// Unique name.
        name: String,
        /// Schedule expression (cron or natural DSL).
        schedule: String,
        /// Sync root id.
        sync_root_id: u64,
    },
    /// Remove a schedule by name.
    Remove {
        /// Name to remove.
        name: String,
    },
}

/// Result body the CLI returns to the user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackupScheduleCliReply {
    /// `List` reply.
    List {
        /// Snapshot of all entries.
        entries: Vec<ScheduleEntry>,
    },
    /// Successful mutation.
    Ok,
    /// Error with a human-readable description.
    Error {
        /// Stringified error.
        message: String,
    },
}

/// Apply a CLI command to a `BackupScheduleConfig` in memory. Persistence
/// is the caller's responsibility — the daemon's config store writes the
/// result back to `[plugins.backup_schedule]`.
pub fn apply_cli(
    config: &mut BackupScheduleConfig,
    cmd: BackupScheduleCliCommand,
) -> BackupScheduleCliReply {
    match cmd {
        BackupScheduleCliCommand::List => BackupScheduleCliReply::List {
            entries: config.entries.clone(),
        },
        BackupScheduleCliCommand::Add {
            name,
            schedule,
            sync_root_id,
        } => match config.add(ScheduleEntry {
            name,
            schedule,
            sync_root_id,
            enabled: true,
        }) {
            Ok(()) => BackupScheduleCliReply::Ok,
            Err(e) => BackupScheduleCliReply::Error {
                message: e.to_string(),
            },
        },
        BackupScheduleCliCommand::Remove { name } => match config.remove(&name) {
            Ok(_) => BackupScheduleCliReply::Ok,
            Err(e) => BackupScheduleCliReply::Error {
                message: e.to_string(),
            },
        },
    }
}

// Silence unused import warning when chrono::TimeZone is only used in tests.
#[allow(dead_code)]
fn _keep_timezone_imported() -> DateTime<Utc> {
    Utc.timestamp_opt(0, 0)
        .single()
        .expect("epoch is always a valid timestamp")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parses_cron_and_natural_expressions() {
        // 5-field cron.
        let p = parse_schedule("0 18 * * 5").expect("cron parse");
        assert!(p.as_cron().split_whitespace().count() == 7);

        // 7-field cron (seconds + year).
        let p = parse_schedule("0 0 12 * * * *").expect("7-field cron");
        assert_eq!(p.as_cron(), "0 0 12 * * * *");

        // Natural: every friday 18:00.
        let p = parse_schedule("every friday 18:00").expect("every fri 18:00");
        assert!(p.as_cron().contains("FRI"));
        assert!(p.as_cron().contains(" 18 "));

        // Natural: daily at 03:00.
        let p = parse_schedule("daily at 03:00").expect("daily");
        assert!(p.as_cron().starts_with("0 0 3 "));

        // Natural: hourly.
        let p = parse_schedule("hourly").expect("hourly");
        assert!(p.as_cron().contains(" * * * "));

        // Natural: weekly on monday at 09:15.
        let p = parse_schedule("weekly on monday at 09:15").expect("weekly");
        assert!(p.as_cron().contains("MON"));
        assert!(p.as_cron().contains(" 15 9 "));

        // Rejected verb: must not parse as a schedule.
        let e = parse_schedule("run every minute").unwrap_err();
        matches!(e, BackupScheduleError::InvalidSchedule(_));

        // Totally bogus string.
        parse_schedule("xyzzy").unwrap_err();
    }

    #[test]
    fn schedule_fires_at_expected_boundaries() {
        // Start clock at 2026-04-17 17:59:00 UTC (Friday).
        let start = Utc.with_ymd_and_hms(2026, 4, 17, 17, 59, 0).unwrap();
        let clock = ManualClock::new(start);
        let mut cfg = BackupScheduleConfig::default();
        cfg.add(ScheduleEntry {
            name: "friday-backup".into(),
            schedule: "every friday 18:00".into(),
            sync_root_id: 42,
            enabled: true,
        })
        .expect("add");

        // Use a Mutex-shared ManualClock so we can advance it while the
        // plugin holds a `Box<dyn Clock>` to the same underlying state.
        use std::sync::{Arc, Mutex};
        struct MClock(Arc<Mutex<ManualClock>>);
        impl Clock for MClock {
            fn now(&self) -> DateTime<Utc> {
                self.0.lock().unwrap().now()
            }
        }

        let shared = Arc::new(Mutex::new(clock));
        let mut plugin =
            BackupSchedulePlugin::new_with_clock(cfg, Box::new(MClock(shared.clone()))).unwrap();
        assert_eq!(plugin.pending_len(), 0);

        // Tick with no advance — nothing fires.
        plugin.tick();
        assert_eq!(plugin.pending_len(), 0);

        // Advance 61 seconds — crosses the 18:00 boundary exactly once.
        shared.lock().unwrap().advance_secs(61);
        plugin.tick();
        assert_eq!(plugin.pending_len(), 1);
        match plugin.next_operation() {
            Some(PluginOperation::RequestSyncResume { sync_root_id }) => {
                assert_eq!(sync_root_id, 42);
            }
            other => panic!("expected RequestSyncResume, got {other:?}"),
        }

        // Advance 24 hours (Saturday 18:00:01) — no additional fire (we
        // schedule every Friday only).
        shared.lock().unwrap().advance_secs(24 * 3600);
        plugin.tick();
        // `next_operation` also ticks, so drain explicitly:
        let ops: Vec<_> = std::iter::from_fn(|| plugin.next_operation()).collect();
        assert!(
            ops.is_empty(),
            "no additional fire expected before next Friday, got {ops:?}"
        );
    }

    #[test]
    fn disabled_schedule_does_not_fire() {
        use std::sync::{Arc, Mutex};
        struct MClock(Arc<Mutex<ManualClock>>);
        impl Clock for MClock {
            fn now(&self) -> DateTime<Utc> {
                self.0.lock().unwrap().now()
            }
        }

        let start = Utc.with_ymd_and_hms(2026, 4, 17, 17, 59, 0).unwrap();
        let shared = Arc::new(Mutex::new(ManualClock::new(start)));
        let mut cfg = BackupScheduleConfig::default();
        cfg.entries.push(ScheduleEntry {
            name: "disabled".into(),
            schedule: "every friday 18:00".into(),
            sync_root_id: 7,
            enabled: false,
        });

        let mut plugin =
            BackupSchedulePlugin::new_with_clock(cfg, Box::new(MClock(shared.clone()))).unwrap();
        shared.lock().unwrap().advance_secs(3600); // past 18:00
        plugin.tick();
        assert_eq!(
            plugin.pending_len(),
            0,
            "disabled schedule must not enqueue"
        );
    }

    #[test]
    fn cli_add_and_remove_persist_in_config() {
        let mut cfg = BackupScheduleConfig::default();

        let reply = apply_cli(
            &mut cfg,
            BackupScheduleCliCommand::Add {
                name: "nightly".into(),
                schedule: "daily at 02:30".into(),
                sync_root_id: 11,
            },
        );
        assert_eq!(reply, BackupScheduleCliReply::Ok);
        assert_eq!(cfg.entries.len(), 1);
        assert_eq!(cfg.entries[0].name, "nightly");
        assert_eq!(cfg.entries[0].sync_root_id, 11);
        assert!(cfg.entries[0].enabled);

        // Adding duplicate name must error.
        let reply = apply_cli(
            &mut cfg,
            BackupScheduleCliCommand::Add {
                name: "nightly".into(),
                schedule: "hourly".into(),
                sync_root_id: 12,
            },
        );
        assert!(matches!(reply, BackupScheduleCliReply::Error { .. }));
        assert_eq!(cfg.entries.len(), 1);

        // List.
        let reply = apply_cli(&mut cfg, BackupScheduleCliCommand::List);
        match reply {
            BackupScheduleCliReply::List { entries } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].name, "nightly");
            }
            other => panic!("expected List, got {other:?}"),
        }

        // Remove.
        let reply = apply_cli(
            &mut cfg,
            BackupScheduleCliCommand::Remove {
                name: "nightly".into(),
            },
        );
        assert_eq!(reply, BackupScheduleCliReply::Ok);
        assert!(cfg.entries.is_empty());

        // Removing non-existent must error.
        let reply = apply_cli(
            &mut cfg,
            BackupScheduleCliCommand::Remove {
                name: "ghost".into(),
            },
        );
        assert!(matches!(reply, BackupScheduleCliReply::Error { .. }));

        // Round-trip through JSON so we know serde works.
        let json = serde_json::to_string(&cfg).unwrap();
        let back: BackupScheduleConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn cap_of_32_enforced() {
        let mut cfg = BackupScheduleConfig::default();
        for i in 0..MAX_SCHEDULES {
            cfg.add(ScheduleEntry {
                name: format!("s{i}"),
                schedule: "hourly".into(),
                sync_root_id: i as u64,
                enabled: true,
            })
            .unwrap();
        }
        let err = cfg
            .add(ScheduleEntry {
                name: "overflow".into(),
                schedule: "hourly".into(),
                sync_root_id: 999,
                enabled: true,
            })
            .unwrap_err();
        assert!(matches!(err, BackupScheduleError::TooMany { .. }));
    }
}
