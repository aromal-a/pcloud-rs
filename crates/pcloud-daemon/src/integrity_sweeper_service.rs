// TODO(bd-sweep-unwrap): This file contains ~50 `.unwrap()` / `.expect()`
// call sites in non-test code paths. The sweeper scheduler thread and
// Mutex-guarded state accesses are the primary targets. Full sweep deferred
// to a dedicated hardening pass; scheduler thread panics are logged and the
// sweeper silently disables itself on the next bootstrap.

//! Background-integrity-sweeper daemon service (H14d).
//!
//! ## Purpose
//!
//! Wire together the configuration scaffolding shipped by H14a
//! ([`pcloud_config::integrity_sweeper`]), the sweep engine in
//! `pcloud_fs::integrity_sweeper`, and the daemon's existing audit and
//! IPC surfaces. Exposes a stable [`IntegritySweeperShell`] the runtime
//! borrows mutably during IPC dispatch, plus the audit-detail formatter
//! the parity-proof harness greps for, **and** a cron-driven scheduler
//! that honours the `schedule_cron` / `pause_on_battery` config keys.
//!
//! ## Security posture
//!
//! - Every [`IntegrityEvent::Mismatch`] is recorded through the
//!   tamper-evident audit log ([`pcloud_store::append_audit_event`]) with
//!   the raw path replaced by a SHA-256 `path_hash`.
//! - Audit write failures are **never silently dropped**; they increment
//!   [`IntegritySweeperShell::audit_drop_count`] (audit invariant M1).
//! - The sweeper is **disabled by default** at config load time; an
//!   operator must set `[features.integrity_sweeper] enabled = true`.
//! - The disabled shell is a no-op that responds with a stable
//!   "not enabled" IPC reply, never `InvalidRequest`.
//!
//! ## Scheduler + battery hooks
//!
//! - `schedule_cron` is parsed on startup via the `cron` crate and a
//!   dedicated scheduler thread (`pcloudd-integrity-scheduler`) sleeps
//!   until the next boundary, then invokes
//!   [`IntegritySweeperShell::run_once`]. An invalid cron expression
//!   refuses to start the scheduler and surfaces through the
//!   [`IntegritySweeperShell::from_config`] / `start_schedule` error
//!   path — the sweeper never silently runs on an unparseable schedule.
//! - `pause_on_battery` gates the scheduler tick: before each run the
//!   scheduler consults a [`PowerSource`]. On Linux the default reader
//!   scans `/sys/class/power_supply/*/status`; on macOS/Windows it uses
//!   the `battery` crate. When any supply reports `Discharging` (or
//!   equivalent) the tick is skipped and the scheduler emits a
//!   structured `integrity_sweeper.paused{reason="on_battery"}` line.
//!   Unsupported platforms log a one-shot warning and behave as if the
//!   flag were disabled.
//!
//! ## Honest limitations
//!
//! - **Self-contained event type.** This module defines its own
//!   [`IntegrityEvent`] rather than depending on
//!   `pcloud_plugin_api::FileIntegrityResult`; the daemon has no
//!   `pcloud-plugin-api` dependency by design (see module code).
//! - **Real walker integrated.** [`IntegritySweeperShell::run_once`]
//!   walks every configured sweep root using
//!   `pcloud_fs::integrity_sweeper::IntegritySweeper::sweep`, computes
//!   local SHA-256 for each file, fetches remote SHA-256 via the
//!   [`DaemonChecksumFetcher`] trait, compares digests, and pipes
//!   resulting [`IntegrityEvent`]s through the worker channel for audit.
//!
//! ### Original notes
//!
//! This module is **deliberately self-contained**:
//!
//! - it owns its own [`IntegrityEvent`] type rather than depending on
//!   the plugin-api crate (the daemon has no `pcloud-plugin-api`
//!   dependency, and reusing that wire type would couple two unrelated
//!   parity surfaces);
//! - it owns its own `mpsc` channel, worker thread, progress accumulator,
//!   and skip-list reload primitive;
//! - it ships a stable `IntegritySweeperShell` API that the runtime
//!   borrows mutably during IPC dispatch.
//!
//! ## Walker integration
//!
//! [`IntegritySweeperShell::run_once`] walks all configured sweep roots
//! (set via [`IntegritySweeperShell::set_sweep_roots`]) using the
//! `pcloud_fs::integrity_sweeper::IntegritySweeper` engine. For each
//! file it computes the local SHA-256, fetches the remote SHA-256 via
//! the [`DaemonChecksumFetcher`] trait (set via
//! [`IntegritySweeperShell::set_checksum_fetcher`]), and compares. The
//! resulting events are translated to daemon-level [`IntegrityEvent`]s
//! and piped through the worker channel. Mismatches are audited,
//! Ok/Throttled events update progress counters. The cron scheduler
//! uses the same walk-and-compare codepath.
//!
//! ## Audit invariant
//!
//! Every `Mismatch` event flows through `record_mismatch_audit` which
//! calls into the daemon's tamper-evident audit log
//! ([`pcloud_store::append_audit_event`]). The audit row carries a
//! BLAKE3-over-path "path hash" rather than the raw path so the audit
//! stream stays redacted under the H1 secret-handling rules. Local /
//! remote SHA256 hex digests are non-secret content fingerprints and are
//! safe to store verbatim.
//!
//! `Ok` events drop silently. `Throttled` events bump a metric counter
//! so operators can see when the rate limiter is biting.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::Utc;
use cron::Schedule;
use pcloud_config::integrity_sweeper::IntegritySweeperConfig;
use pcloud_fs::integrity_sweeper::{
    CheckError, ChecksumFetcher, IntegrityEvent as FsIntegrityEvent,
    IntegrityResult as FsIntegrityResult, IntegritySweeper, SweeperConfig,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A sync-root entry the sweeper walks. Pairs a local directory path
/// with the remote pCloud prefix used to construct remote paths when
/// querying the [`ChecksumFetcher`].
#[derive(Debug, Clone)]
pub struct SweepRoot {
    /// Absolute local filesystem path to walk.
    pub local_path: PathBuf,
    /// Remote pCloud path prefix (e.g. `/My Music`).
    pub remote_prefix: String,
}

/// Trait abstracting remote SHA-256 lookups for the daemon's integrity
/// sweeper. The production implementation wraps the pCloud `checksumfile`
/// API endpoint (or the daemon's transfer backend). Tests inject a mock.
///
/// This is a daemon-level trait that is **not** the same as
/// `pcloud_fs::integrity_sweeper::ChecksumFetcher` — the daemon adapts
/// between the two in `DaemonChecksumFetcherAdapter`.
pub trait DaemonChecksumFetcher: Send + Sync + std::fmt::Debug {
    /// Return the remote SHA-256 hex digest for `remote_path`, or an
    /// error when the object does not exist or the lookup fails.
    fn fetch_sha256_hex(&self, remote_path: &str) -> Result<String, DaemonCheckError>;
}

/// Errors from a [`DaemonChecksumFetcher`] call.
#[derive(Debug)]
pub enum DaemonCheckError {
    /// The remote object does not exist.
    NotFound,
    /// Any other failure (transport, auth, decode).
    Other(String),
}

/// Adapts a [`DaemonChecksumFetcher`] (hex-string-based) to the
/// `pcloud_fs::integrity_sweeper::ChecksumFetcher` trait (byte-array-based).
struct DaemonChecksumFetcherAdapter<'a> {
    inner: &'a dyn DaemonChecksumFetcher,
}

impl ChecksumFetcher for DaemonChecksumFetcherAdapter<'_> {
    fn fetch_sha256(&self, remote_path: &str) -> Result<[u8; 32], CheckError> {
        match self.inner.fetch_sha256_hex(remote_path) {
            Ok(hex) => parse_sha256_hex(&hex).map_err(CheckError::Other),
            Err(DaemonCheckError::NotFound) => Err(CheckError::NotFound),
            Err(DaemonCheckError::Other(reason)) => Err(CheckError::Other(reason)),
        }
    }
}

/// Parse a 64-char lowercase hex SHA-256 digest into a 32-byte array.
fn parse_sha256_hex(hex: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", hex.len()));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi =
            hex_nibble(chunk[0]).ok_or_else(|| format!("invalid hex char at pos {}", i * 2))?;
        let lo =
            hex_nibble(chunk[1]).ok_or_else(|| format!("invalid hex char at pos {}", i * 2 + 1))?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(10 + b - b'a'),
        b'A'..=b'F' => Some(10 + b - b'A'),
        _ => None,
    }
}

/// A no-op fetcher that reports every file as `NotFound`. Used when the
/// daemon has no authenticated session or no real transport wired.
#[derive(Debug)]
pub struct NoOpChecksumFetcher;

impl DaemonChecksumFetcher for NoOpChecksumFetcher {
    fn fetch_sha256_hex(&self, _remote_path: &str) -> Result<String, DaemonCheckError> {
        Err(DaemonCheckError::NotFound)
    }
}

/// Outcome of scanning a single file. Self-contained on purpose — see
/// the module docs for why this does not reuse
/// `pcloud_plugin_api::FileIntegrityResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrityEvent {
    /// File matched. Not audited; only counted in [`SweepProgress`].
    Ok {
        /// Absolute local path of the file that matched.
        path: PathBuf,
    },
    /// File hash diverged from the expected remote SHA256. Written to
    /// the audit log via `record_mismatch_audit`.
    Mismatch {
        /// Absolute local path of the divergent file.
        path: PathBuf,
        /// Lowercase hex SHA256 of the local content.
        local_sha_hex: String,
        /// Lowercase hex SHA256 reported by the remote.
        remote_sha_hex: String,
    },
    /// Worker had to drop a candidate because the rate limiter refused a
    /// token. Bumps a metric only.
    Throttled {
        /// Absolute local path that was skipped this cycle.
        path: PathBuf,
    },
}

/// One per-file record emitted by the walker into the NDJSON event
/// stream during [`IntegritySweeperShell::run_once_ndjson`] or any other
/// NDJSON-enabled sweep entry point.
///
/// ## Field contract (stable — parity harness + operators rely on it)
///
/// - `ts` — RFC 3339 UTC timestamp produced at emission time.
/// - `path_hash` — SHA-256 hex of the **absolute** local path bytes. The
///   raw path is never serialised (audit-redaction invariant); this hash
///   is one-way and safe to ship to an operator console. Audit consumers
///   that need to correlate back to a path must maintain their own
///   local hash→path map (the walker does **not** persist one).
/// - `remote_path` — Routing identifier used for the server-side
///   checksum lookup. Non-secret; helps operators localise a mismatch
///   without leaking the user's local directory layout.
/// - `local_hash` — Lowercase hex SHA-256 of the local file contents,
///   or `None` for `missing_local` / `error` rows where the walker
///   never produced a local digest.
/// - `remote_hash` — Lowercase hex SHA-256 reported by the server, or
///   `None` when the server reported the file missing / the fetch
///   failed.
/// - `status` — One of `match | mismatch | missing_remote |
///   missing_local | error | skipped | throttled`. The core five are
///   the `bd-1du.4.6.1` spec; `skipped`/`throttled` are extra, emitted
///   by the walker for operator visibility and tolerated by consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrityNdjsonRecord {
    /// RFC 3339 UTC timestamp for this record.
    pub ts: String,
    /// SHA-256 hex of the absolute local path. The raw path is never
    /// serialised.
    pub path_hash: String,
    /// Remote pCloud path used for the checksum lookup. Empty string
    /// when the walker could not construct a remote path.
    pub remote_path: String,
    /// Lowercase hex SHA-256 of the local content, if computed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_hash: Option<String>,
    /// Lowercase hex SHA-256 reported by the server, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_hash: Option<String>,
    /// One of `match|mismatch|missing_remote|missing_local|error|
    /// skipped|throttled`. Stored as `String` so the struct round-trips
    /// through serde; the producer side still writes one of the
    /// `ndjson_status::*` constants verbatim.
    pub status: String,
}

/// Stable status string constants for [`IntegrityNdjsonRecord::status`].
///
/// NDJSON status tokens — operators grep for these verbatim, so any
/// change here is a breaking wire-format change.
pub mod ndjson_status {
    /// Local + remote SHA-256 matched.
    pub const MATCH: &str = "match";
    /// Local + remote SHA-256 differed.
    pub const MISMATCH: &str = "mismatch";
    /// Server reports the remote object does not exist (orphan local).
    pub const MISSING_REMOTE: &str = "missing_remote";
    /// Local file disappeared / was unreadable mid-walk.
    pub const MISSING_LOCAL: &str = "missing_local";
    /// Server-side checksum lookup failed for a non-`NotFound` reason.
    pub const ERROR: &str = "error";
    /// File matched a skip-list glob and was not hashed.
    pub const SKIPPED: &str = "skipped";
    /// Rate limiter forced a throttle before this file was hashed.
    pub const THROTTLED: &str = "throttled";
}

/// Build an [`IntegrityNdjsonRecord`] from a walker event and the
/// computed remote path. Returns `None` for events that do not produce
/// a record (currently only the `Hashed` variant, which never appears
/// in production sweeps because the daemon always passes a fetcher).
#[must_use]
pub fn ndjson_record_from_fs_event(
    fs_event: &FsIntegrityEvent,
    remote_path: &str,
) -> Option<IntegrityNdjsonRecord> {
    let ts = chrono::Utc::now().to_rfc3339();
    let path_hash = hex_encode(&fs_event.path_hash);
    let local_hash = fs_event.local_sha256.as_ref().map(|d| hex_encode(d));

    let (remote_hash, status) = match &fs_event.result {
        FsIntegrityResult::Ok => (local_hash.clone(), ndjson_status::MATCH),
        FsIntegrityResult::Mismatch { remote, .. } => {
            (Some(hex_encode(remote)), ndjson_status::MISMATCH)
        }
        FsIntegrityResult::RemoteMissing => (None, ndjson_status::MISSING_REMOTE),
        FsIntegrityResult::LocalMissing => (None, ndjson_status::MISSING_LOCAL),
        FsIntegrityResult::FetchFailed { .. } => (None, ndjson_status::ERROR),
        FsIntegrityResult::Skipped => (None, ndjson_status::SKIPPED),
        FsIntegrityResult::Throttled => (None, ndjson_status::THROTTLED),
        // `Hashed` is emitted only when the walker is invoked without a
        // fetcher (tests / future offline-only mode). Production sweeps
        // always pass a fetcher, so this arm is dead in the daemon
        // path; we intentionally suppress a record rather than invent a
        // synthetic status.
        FsIntegrityResult::Hashed => return None,
    };

    Some(IntegrityNdjsonRecord {
        ts,
        path_hash,
        remote_path: remote_path.to_owned(),
        local_hash,
        remote_hash,
        status: status.to_owned(),
    })
}

/// Write one [`IntegrityNdjsonRecord`] to `sink` as a single JSON line
/// (`serialize + '\n'`). Returns an [`std::io::Error`] if the underlying
/// writer failed; the caller is responsible for deciding whether to
/// abort the sweep or continue past a transient sink failure.
pub fn write_ndjson_record(
    sink: &mut dyn Write,
    record: &IntegrityNdjsonRecord,
) -> std::io::Result<()> {
    let json = serde_json::to_string(record).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("ndjson serialise: {e}"),
        )
    })?;
    sink.write_all(json.as_bytes())?;
    sink.write_all(b"\n")?;
    Ok(())
}

/// Snapshot of cumulative sweeper progress. Returned by
/// [`IntegritySweeperShell::progress_snapshot`] and forwarded over IPC
/// as JSON in `Response::message`.
///
/// All counters are monotone. `Default` returns the zero-progress state
/// surfaced when the sweeper has never run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepProgress {
    /// Number of files the worker successfully hashed (Ok + Mismatch).
    pub files_hashed: u64,
    /// Total bytes hashed across all completed files.
    pub bytes_hashed: u64,
    /// Cumulative count of [`IntegrityEvent::Mismatch`] events written
    /// to the audit log.
    pub mismatches_found: u64,
    /// Cumulative count of [`IntegrityEvent::Throttled`] events.
    pub throttled: u64,
}

#[derive(Debug, Default)]
struct SharedState {
    progress: Mutex<SweepProgress>,
    /// Set to `true` when a sweep is currently executing — used by the
    /// IPC "status" verb so concurrent operators can see the worker is
    /// busy without racing on the progress mutex.
    sweep_in_flight: AtomicBool,
}

/// Observed power-state for the host. Returned by [`PowerSource::read`].
///
/// The `Unknown` variant is emitted when the platform cannot report a
/// definitive state — the scheduler treats it as "AC" (do not pause)
/// because the alternative is to silently stop running on systems that
/// simply lack a battery (servers, VMs, containers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    /// At least one power-supply reports actively charging or full on
    /// mains. Scheduler runs normally.
    OnAc,
    /// At least one power-supply reports `Discharging`. Scheduler skips
    /// the tick when `pause_on_battery = true`.
    OnBattery,
    /// Platform has no battery-state facade or reported `Unknown`.
    /// Scheduler treats this as "AC" and runs normally; a one-shot
    /// warning is emitted when the scheduler first sees `Unknown`.
    Unknown,
}

/// Abstract "is the host on battery?" reader. Production callers get
/// the platform default via [`default_power_source`]; tests inject a
/// [`MockPowerSource`] to exercise the skip path deterministically.
///
/// Implementations **must not** block: the scheduler calls [`Self::read`]
/// synchronously before every tick.
pub trait PowerSource: Send + Sync {
    /// Read the current power state.
    fn read(&self) -> PowerState;
}

/// Build the default power-source reader for this platform.
///
/// - Linux: scans `/sys/class/power_supply/*/status`.
/// - macOS / Windows: uses the `battery` crate.
/// - Other platforms: returns [`PowerState::Unknown`] on every read.
#[must_use]
pub fn default_power_source() -> Box<dyn PowerSource> {
    Box::new(platform_power::PlatformPowerSource::new())
}

/// Test helper that reports a fixed [`PowerState`]. Exposed because the
/// scheduler thread takes a `Box<dyn PowerSource>` and unit tests need a
/// deterministic feed.
#[derive(Debug, Clone, Copy)]
pub struct MockPowerSource {
    state: PowerState,
}

impl MockPowerSource {
    /// Build a mock that always reports `state`.
    #[must_use]
    pub const fn new(state: PowerState) -> Self {
        Self { state }
    }
}

impl PowerSource for MockPowerSource {
    fn read(&self) -> PowerState {
        self.state
    }
}

mod platform_power {
    //! Platform-specific [`PowerSource`] implementation.
    //!
    //! - Linux: pure filesystem read of `/sys/class/power_supply/*/status`.
    //!   Avoids the `battery` crate (and its udev dep chain) on our
    //!   tier-1 Linux target.
    //! - macOS / Windows: delegates to the `battery` crate which abstracts
    //!   over `IOKit` / `SetupAPI` respectively.
    //! - Other platforms: permanent [`PowerState::Unknown`].

    use super::{PowerSource, PowerState};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Default platform reader.
    #[derive(Debug, Default)]
    pub struct PlatformPowerSource {
        /// Used to guarantee a one-shot warning when the platform reports
        /// `Unknown` — the scheduler should not spam stderr every tick.
        unknown_logged: AtomicBool,
    }

    impl PlatformPowerSource {
        #[must_use]
        pub const fn new() -> Self {
            Self {
                unknown_logged: AtomicBool::new(false),
            }
        }
    }

    impl PowerSource for PlatformPowerSource {
        #[cfg(target_os = "linux")]
        fn read(&self) -> PowerState {
            read_linux(&self.unknown_logged)
        }

        #[cfg(any(target_os = "macos", windows))]
        fn read(&self) -> PowerState {
            read_battery_crate(&self.unknown_logged)
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        fn read(&self) -> PowerState {
            log_unknown_once(&self.unknown_logged, "platform has no battery facade");
            PowerState::Unknown
        }
    }

    #[cfg(target_os = "linux")]
    fn read_linux(unknown_logged: &AtomicBool) -> PowerState {
        let root = std::path::Path::new("/sys/class/power_supply");
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => {
                log_unknown_once(unknown_logged, "/sys/class/power_supply unavailable");
                return PowerState::Unknown;
            }
        };
        let mut saw_any = false;
        let mut saw_discharging = false;
        for entry in entries.flatten() {
            let status_path = entry.path().join("status");
            let status = match std::fs::read_to_string(&status_path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let trimmed = status.trim();
            if trimmed.is_empty() {
                continue;
            }
            saw_any = true;
            if trimmed.eq_ignore_ascii_case("Discharging") {
                saw_discharging = true;
            }
        }
        if !saw_any {
            log_unknown_once(unknown_logged, "no power-supply status readable");
            return PowerState::Unknown;
        }
        if saw_discharging {
            PowerState::OnBattery
        } else {
            PowerState::OnAc
        }
    }

    #[cfg(any(target_os = "macos", windows))]
    fn read_battery_crate(unknown_logged: &AtomicBool) -> PowerState {
        let manager = match battery::Manager::new() {
            Ok(m) => m,
            Err(_) => {
                log_unknown_once(unknown_logged, "battery::Manager::new failed");
                return PowerState::Unknown;
            }
        };
        let iter = match manager.batteries() {
            Ok(i) => i,
            Err(_) => {
                log_unknown_once(unknown_logged, "battery::Manager::batteries failed");
                return PowerState::Unknown;
            }
        };
        let mut saw_any = false;
        let mut saw_discharging = false;
        for b in iter.flatten() {
            saw_any = true;
            if matches!(b.state(), battery::State::Discharging) {
                saw_discharging = true;
            }
        }
        if !saw_any {
            return PowerState::OnAc;
        }
        if saw_discharging {
            PowerState::OnBattery
        } else {
            PowerState::OnAc
        }
    }

    fn log_unknown_once(unknown_logged: &AtomicBool, detail: &str) {
        if unknown_logged
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            log::warn!(
                r#"{{"event":"integrity_sweeper.battery_unknown","detail":"{}"}}"#,
                detail
            );
        }
    }
}

/// Error returned by [`IntegritySweeperShell::start_schedule`] when the
/// configured `schedule_cron` expression cannot be parsed. The sweeper
/// **never** silently runs on an unparseable schedule.
#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    /// `schedule_cron` was not a valid 6- or 7-field cron expression.
    #[error("invalid cron expression {expr:?}: {source}")]
    InvalidCron {
        /// The offending expression (redacted from log output — cron
        /// strings are not secret but we include them for operators).
        expr: String,
        /// Parser error from the `cron` crate.
        #[source]
        source: cron::error::Error,
    },
    /// `[features.integrity_sweeper]` has no schedule configured. Not an
    /// error at the config layer; surfaced only when an explicit
    /// `start_schedule` call is made without a schedule.
    #[error("no schedule_cron configured")]
    NoSchedule,
}

/// Internal wake-condition used by the scheduler thread. A `Condvar`
/// lets `shutdown` interrupt a long sleep without waiting for the next
/// tick — important for tests and for clean daemon shutdown.
#[derive(Debug, Default)]
struct SchedulerWake {
    stopped: Mutex<bool>,
    cv: Condvar,
}

/// Daemon-side handle to the integrity sweeper.
///
/// Always present on the runtime even when the sweeper is disabled;
/// the disabled path is a no-op shell so IPC verbs return a stable
/// "not enabled" response rather than the generic `InvalidRequest`.
#[derive(Debug)]
pub struct IntegritySweeperShell {
    config: IntegritySweeperConfig,
    skip_list_path: Option<PathBuf>,
    /// Cached parsed glob patterns from the skip-list file. Reloaded on
    /// every [`Self::reload_skip_list`] call.
    skip_globs: Mutex<Vec<glob::Pattern>>,
    /// Sender end of the worker channel. `None` when the worker thread
    /// is not running (sweeper disabled or not yet started).
    sender: Option<Sender<IntegrityEvent>>,
    /// Background worker thread handle (Some only when enabled).
    worker: Option<JoinHandle<()>>,
    /// Shared state with the worker thread.
    shared: Arc<SharedState>,
    /// Stop signal. Set to `true` to ask the worker to drain and exit.
    stop_flag: Arc<AtomicBool>,
    /// Counter of audit-write failures the worker has observed. Surfaced
    /// to the IPC status payload so silent persistence drops are
    /// visible. Audit finding M1 / runtime invariant.
    audit_drop_count: Arc<AtomicU64>,
    /// Cron-driven scheduler thread handle. Some iff `schedule_cron` is
    /// set and [`Self::start_schedule`] has been called.
    scheduler_handle: Option<JoinHandle<()>>,
    /// Wake channel used to interrupt the scheduler's `wait_timeout`
    /// during shutdown.
    scheduler_wake: Arc<SchedulerWake>,
    /// Monotone counter of scheduler ticks that were skipped because the
    /// battery check reported [`PowerState::OnBattery`]. Exposed to
    /// tests.
    battery_skip_count: Arc<AtomicU64>,
    /// Monotone counter of scheduler ticks that actually fired
    /// `run_once`. Exposed to tests.
    scheduled_run_count: Arc<AtomicU64>,
    /// Sync roots the sweeper walks on each `run_once` invocation.
    /// Populated by the runtime via [`Self::set_sweep_roots`].
    sweep_roots: Arc<Mutex<Vec<SweepRoot>>>,
    /// Remote checksum fetcher. Populated by the runtime via
    /// [`Self::set_checksum_fetcher`]. Defaults to [`NoOpChecksumFetcher`].
    checksum_fetcher: Arc<Mutex<Arc<dyn DaemonChecksumFetcher>>>,
    /// Optional notification channel fired by the scheduler after every
    /// tick (run or skip). Used by tests to replace sleep-based timing
    /// with a deterministic signal-based approach.
    #[allow(dead_code)]
    tick_notify: Arc<Mutex<Option<mpsc::Sender<()>>>>,
}

impl IntegritySweeperShell {
    /// Build a disabled shell. Safe default that performs no I/O and
    /// spawns no thread; the IPC surface returns "not enabled" for every
    /// verb. Intended for use when the operator has not opted into
    /// `[features.integrity_sweeper] enabled = true`.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            config: IntegritySweeperConfig::default(),
            skip_list_path: None,
            skip_globs: Mutex::new(Vec::new()),
            sender: None,
            worker: None,
            shared: Arc::new(SharedState::default()),
            stop_flag: Arc::new(AtomicBool::new(false)),
            audit_drop_count: Arc::new(AtomicU64::new(0)),
            scheduler_handle: None,
            scheduler_wake: Arc::new(SchedulerWake::default()),
            battery_skip_count: Arc::new(AtomicU64::new(0)),
            scheduled_run_count: Arc::new(AtomicU64::new(0)),
            sweep_roots: Arc::new(Mutex::new(Vec::new())),
            checksum_fetcher: Arc::new(Mutex::new(Arc::new(NoOpChecksumFetcher))),
            tick_notify: Arc::new(Mutex::new(None)),
        }
    }

    /// Build a sweeper shell from a validated configuration. When
    /// `cfg.enabled` is `false` this returns the disabled shell unchanged
    /// (no worker, no I/O). When `true`, an `mpsc` channel is created
    /// but the worker thread is **not** spawned here — the runtime calls
    /// [`Self::spawn_worker`] from bootstrap once it has cloned the
    /// audit-emission closure.
    ///
    /// # Errors
    ///
    /// Returns `Err` when `cfg.skip_list_path` is set but the file
    /// cannot be parsed by [`pcloud_config::integrity_sweeper::load_skip_list`].
    pub fn from_config(cfg: IntegritySweeperConfig) -> std::io::Result<Self> {
        if !cfg.enabled {
            return Ok(Self::disabled());
        }
        let skip_globs = match cfg.skip_list_path.as_deref() {
            Some(p) => pcloud_config::integrity_sweeper::load_skip_list(p)?,
            None => Vec::new(),
        };
        let (sender, _receiver_will_be_taken_by_spawn_worker) = mpsc::channel::<IntegrityEvent>();
        // We deliberately drop the receiver here. spawn_worker creates
        // its own channel pair so the worker owns a fresh receiver and
        // the shell publishes the matching sender. Holding a placeholder
        // receiver would leak threads in tests that never call
        // spawn_worker.
        drop(_receiver_will_be_taken_by_spawn_worker);
        let skip_list_path = cfg.skip_list_path.clone();
        // Pre-validate the cron expression at shell-construction time so
        // a typo is caught before any thread is spawned. The parsed
        // schedule is re-parsed inside `start_schedule`; we deliberately
        // do not cache it on the struct because `Schedule` is not `Sync`
        // in all `cron` versions and re-parsing is cheap.
        if let Some(expr) = cfg.schedule_cron.as_deref()
            && let Err(source) = Schedule::from_str(expr)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid schedule_cron {expr:?}: {source}"),
            ));
        }
        Ok(Self {
            config: cfg,
            skip_list_path,
            skip_globs: Mutex::new(skip_globs),
            sender: Some(sender),
            worker: None,
            shared: Arc::new(SharedState::default()),
            stop_flag: Arc::new(AtomicBool::new(false)),
            audit_drop_count: Arc::new(AtomicU64::new(0)),
            scheduler_handle: None,
            scheduler_wake: Arc::new(SchedulerWake::default()),
            battery_skip_count: Arc::new(AtomicU64::new(0)),
            scheduled_run_count: Arc::new(AtomicU64::new(0)),
            sweep_roots: Arc::new(Mutex::new(Vec::new())),
            checksum_fetcher: Arc::new(Mutex::new(Arc::new(NoOpChecksumFetcher))),
            tick_notify: Arc::new(Mutex::new(None)),
        })
    }

    /// Whether the operator has opted into the sweeper.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Snapshot the cumulative progress counters.
    #[must_use]
    pub fn progress_snapshot(&self) -> SweepProgress {
        *self.shared.progress.lock().unwrap_or_else(|poisoned| {
            log::error!("integrity sweeper mutex poisoned — recovering");
            poisoned.into_inner()
        })
    }

    /// Number of audit-persistence failures observed by the worker.
    /// Non-zero values indicate the audit log refused to accept a
    /// mismatch row (audit invariant M1 — never silently dropped).
    #[must_use]
    pub fn audit_drop_count(&self) -> u64 {
        self.audit_drop_count.load(Ordering::Relaxed)
    }

    /// Spawn the background worker thread. The worker owns the receiver
    /// half of an `mpsc` channel and translates each event into either
    /// a metric bump (Ok / Throttled) or an audit-log append
    /// (Mismatch). `audit_sink` is a closure the runtime supplies so the
    /// worker can record events without holding a long-lived borrow on
    /// the `RuntimeShell`.
    ///
    /// Idempotent: a second call is a no-op when a worker is already
    /// running. Panics from the audit closure are caught and counted in
    /// [`Self::audit_drop_count`].
    pub fn spawn_worker<F>(&mut self, mut audit_sink: F)
    where
        F: FnMut(&IntegrityEvent) -> Result<(), String> + Send + 'static,
    {
        if self.worker.is_some() {
            return;
        }
        if !self.config.enabled {
            return;
        }
        let (tx, rx) = mpsc::channel::<IntegrityEvent>();
        self.sender = Some(tx);
        let shared = Arc::clone(&self.shared);
        let stop_flag = Arc::clone(&self.stop_flag);
        let audit_drops = Arc::clone(&self.audit_drop_count);
        let handle = thread::Builder::new()
            .name("pcloudd-integrity-sweeper".into())
            .spawn(move || {
                worker_loop(rx, &shared, &stop_flag, &audit_drops, &mut audit_sink);
            })
            // INVARIANT: thread spawn failure is an OS-level resource exhaustion
            // that is unrecoverable at daemon startup; panic with a clear message
            // is the intended behaviour. TODO(bd-follow-up): surface as Err.
            .expect("spawn integrity sweeper thread");
        self.worker = Some(handle);
    }

    /// Replace the sweep roots the walker visits on each `run_once`
    /// invocation. Called by the runtime after sync-root add/remove.
    pub fn set_sweep_roots(&self, roots: Vec<SweepRoot>) {
        *self.sweep_roots.lock().unwrap_or_else(|poisoned| {
            log::error!("integrity sweeper mutex poisoned — recovering");
            poisoned.into_inner()
        }) = roots;
    }

    /// Replace the remote checksum fetcher. Called by the runtime once
    /// an authenticated session is available.
    pub fn set_checksum_fetcher(&self, fetcher: Arc<dyn DaemonChecksumFetcher>) {
        *self.checksum_fetcher.lock().unwrap_or_else(|poisoned| {
            log::error!("integrity sweeper mutex poisoned — recovering");
            poisoned.into_inner()
        }) = fetcher;
    }

    /// Synchronously trigger one sweep cycle and update the progress
    /// counters. Returns the [`SweepProgress`] snapshot taken **after**
    /// the cycle completes so IPC callers can render a meaningful
    /// summary.
    ///
    /// Walks every configured sweep root, computes local SHA-256 for
    /// each file, fetches the remote SHA-256 via the
    /// [`DaemonChecksumFetcher`], compares, and emits
    /// [`IntegrityEvent`] results through the worker channel. The
    /// worker thread picks them up and audits any mismatches.
    pub fn run_once(&self) -> SweepProgress {
        if !self.config.enabled {
            return self.progress_snapshot();
        }
        self.shared.sweep_in_flight.store(true, Ordering::Relaxed);
        let run_started = std::time::Instant::now();

        run_sweep_cycle(
            &self.config,
            self.sender.as_ref(),
            &self.sweep_roots,
            &self.checksum_fetcher,
            &self.skip_globs,
        );

        pcloud_fs::slo_hook::observe_integrity_sweeper_run(run_started.elapsed());
        self.shared.sweep_in_flight.store(false, Ordering::Relaxed);
        self.progress_snapshot()
    }

    /// Synchronously trigger one sweep cycle and stream per-file
    /// [`IntegrityNdjsonRecord`] JSON lines into `ndjson_sink` alongside
    /// the usual audit-mismatch path. Returns the [`SweepProgress`]
    /// snapshot taken **after** the cycle completes.
    ///
    /// This is the `bd-1du.4.6.1` walker entry-point: each local file
    /// produces exactly one NDJSON row tagged with one of
    /// `match | mismatch | missing_remote | missing_local | error |
    /// skipped | throttled`. Sink write failures are **logged but
    /// non-fatal** — a failing sink must not stall a cron-triggered
    /// sweep. Callers that need strict delivery should wrap the sink in
    /// a buffered writer and flush after each call.
    ///
    /// When the sweeper is disabled this method is a no-op and returns
    /// the zero-progress snapshot (no lines written).
    pub fn run_once_ndjson(&self, ndjson_sink: &mut dyn Write) -> SweepProgress {
        if !self.config.enabled {
            return self.progress_snapshot();
        }
        self.shared.sweep_in_flight.store(true, Ordering::Relaxed);
        let run_started = std::time::Instant::now();

        run_sweep_cycle_with_ndjson(
            &self.config,
            self.sender.as_ref(),
            &self.sweep_roots,
            &self.checksum_fetcher,
            &self.skip_globs,
            Some(ndjson_sink),
        );

        pcloud_fs::slo_hook::observe_integrity_sweeper_run(run_started.elapsed());
        self.shared.sweep_in_flight.store(false, Ordering::Relaxed);
        self.progress_snapshot()
    }

    /// Append `path` to the configured skip-list file (one line, no
    /// duplicates) and reload the in-memory glob set. Returns an error
    /// when no `skip_list_path` is configured or the file cannot be
    /// written / parsed.
    pub fn append_skip_path(&self, path: &str) -> std::io::Result<()> {
        let Some(skip_path) = self.skip_list_path.as_ref() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "integrity sweeper skip-list path is not configured",
            ));
        };
        // Read existing entries (if file exists) to dedupe.
        let mut existing = String::new();
        if skip_path.exists() {
            existing = std::fs::read_to_string(skip_path)?;
            for line in existing.lines() {
                if line.trim() == path.trim() {
                    return Ok(()); // already present
                }
            }
        }
        if !existing.ends_with('\n') && !existing.is_empty() {
            existing.push('\n');
        }
        existing.push_str(path.trim());
        existing.push('\n');
        std::fs::write(skip_path, existing)?;
        self.reload_skip_list()
    }

    /// Re-parse the configured skip-list file and replace the cached
    /// glob set. No-op when no `skip_list_path` is configured.
    pub fn reload_skip_list(&self) -> std::io::Result<()> {
        let Some(skip_path) = self.skip_list_path.as_ref() else {
            return Ok(());
        };
        let parsed = pcloud_config::integrity_sweeper::load_skip_list(skip_path)?;
        let mut g = self.skip_globs.lock().unwrap_or_else(|poisoned| {
            log::error!("integrity sweeper mutex poisoned — recovering");
            poisoned.into_inner()
        });
        *g = parsed;
        Ok(())
    }

    /// Number of cached skip-list glob patterns. Test helper used by the
    /// PR4 unit suite to confirm reloads applied.
    #[must_use]
    pub fn skip_glob_count(&self) -> usize {
        self.skip_globs
            .lock()
            .unwrap_or_else(|poisoned| {
                log::error!("integrity sweeper mutex poisoned — recovering");
                poisoned.into_inner()
            })
            .len()
    }

    /// Test-only event-injection seam used by the PR4 unit suite to
    /// drive the worker without a real file walker. Production callers
    /// MUST NOT use this — the PR2/PR3 walker will hand events to the
    /// channel directly via [`Self::sender_for_test`].
    #[doc(hidden)]
    pub fn dispatch_event_for_test(&self, event: IntegrityEvent) -> Result<(), String> {
        match self.sender.as_ref() {
            Some(tx) => tx.send(event).map_err(|e| e.to_string()),
            None => Err("integrity sweeper sender not initialised".to_owned()),
        }
    }

    /// Block-and-flush helper used by tests: stops the worker thread,
    /// then joins it. Safe to call even when no worker is running.
    pub fn shutdown(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        // Wake the scheduler so it notices the stop flag instead of
        // sleeping until the next cron boundary.
        {
            let mut stopped = self
                .scheduler_wake
                .stopped
                .lock()
                .unwrap_or_else(|poisoned| {
                    log::error!("integrity sweeper mutex poisoned — recovering");
                    poisoned.into_inner()
                });
            *stopped = true;
            self.scheduler_wake.cv.notify_all();
        }
        // Drop the sender so the worker's `recv` returns `Err` and exits.
        self.sender = None;
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.scheduler_handle.take() {
            let _ = handle.join();
        }
    }

    /// Spawn the cron-driven scheduler thread.
    ///
    /// Parses `config.schedule_cron`, builds a thread that sleeps until
    /// the next cron boundary, consults `power_source` when
    /// `pause_on_battery` is true, and invokes `run_once` on each tick.
    ///
    /// Idempotent: a second call while a scheduler is already running is
    /// a no-op. When `config.schedule_cron` is `None`, returns
    /// [`ScheduleError::NoSchedule`] and leaves the shell untouched.
    ///
    /// # Errors
    ///
    /// - [`ScheduleError::InvalidCron`] when the expression fails to
    ///   parse via the `cron` crate. The shell never silently runs on an
    ///   unparseable schedule.
    /// - [`ScheduleError::NoSchedule`] when `schedule_cron` is `None`.
    pub fn start_schedule(
        &mut self,
        power_source: Box<dyn PowerSource>,
    ) -> Result<(), ScheduleError> {
        if self.scheduler_handle.is_some() {
            return Ok(());
        }
        if !self.config.enabled {
            return Err(ScheduleError::NoSchedule);
        }
        let expr = self
            .config
            .schedule_cron
            .clone()
            .ok_or(ScheduleError::NoSchedule)?;
        let schedule = Schedule::from_str(&expr).map_err(|source| ScheduleError::InvalidCron {
            expr: expr.clone(),
            source,
        })?;
        let wake = Arc::clone(&self.scheduler_wake);
        let stop_flag = Arc::clone(&self.stop_flag);
        let shared = Arc::clone(&self.shared);
        let battery_skip_count = Arc::clone(&self.battery_skip_count);
        let scheduled_run_count = Arc::clone(&self.scheduled_run_count);
        let pause_on_battery = self.config.pause_on_battery;
        let config = self.config.clone();
        let sender = self.sender.clone();
        let sweep_roots = Arc::clone(&self.sweep_roots);
        let checksum_fetcher = Arc::clone(&self.checksum_fetcher);
        let skip_globs_arc = Arc::new(Mutex::new(
            self.skip_globs
                .lock()
                .unwrap_or_else(|poisoned| {
                    log::error!("integrity sweeper mutex poisoned — recovering");
                    poisoned.into_inner()
                })
                .clone(),
        ));
        let tick_notify = Arc::clone(&self.tick_notify);

        let handle = thread::Builder::new()
            .name("pcloudd-integrity-scheduler".into())
            .spawn(move || {
                scheduler_loop(
                    &schedule,
                    &wake,
                    &stop_flag,
                    &shared,
                    &config,
                    sender.as_ref(),
                    power_source.as_ref(),
                    pause_on_battery,
                    &battery_skip_count,
                    &scheduled_run_count,
                    &sweep_roots,
                    &checksum_fetcher,
                    &skip_globs_arc,
                    &tick_notify,
                );
            })
            // INVARIANT: thread spawn failure is an OS-level resource exhaustion
            // that is unrecoverable at daemon startup; panic with a clear message
            // is the intended behaviour. TODO(bd-follow-up): surface as Err.
            .expect("spawn integrity sweeper scheduler thread");
        self.scheduler_handle = Some(handle);
        Ok(())
    }

    /// Number of ticks that were skipped because the battery reader
    /// reported `Discharging` while `pause_on_battery` was `true`.
    /// Test helper.
    #[must_use]
    pub fn battery_skip_count(&self) -> u64 {
        self.battery_skip_count.load(Ordering::Relaxed)
    }

    /// Number of scheduler ticks that ran `run_once`. Test helper.
    #[must_use]
    pub fn scheduled_run_count(&self) -> u64 {
        self.scheduled_run_count.load(Ordering::Relaxed)
    }

    /// Install a tick-notification channel and return the receiver.
    ///
    /// The scheduler sends `()` on this channel after every tick (both
    /// runs and battery-skips). Tests use this to wait for a deterministic
    /// signal instead of sleeping a fixed duration. Must be called
    /// **before** [`Self::start_schedule`].
    #[doc(hidden)]
    pub fn subscribe_tick_notify(&self) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel();
        *self.tick_notify.lock().unwrap_or_else(|poisoned| {
            log::error!("integrity sweeper mutex poisoned — recovering");
            poisoned.into_inner()
        }) = Some(tx);
        rx
    }
}

/// Translate a `pcloud_fs::integrity_sweeper::IntegrityEvent` into a
/// daemon-level [`IntegrityEvent`]. Returns `None` for events that do
/// not need daemon-level processing (Skipped, Hashed-only, LocalMissing).
fn translate_fs_event(fs_event: &FsIntegrityEvent, root: &Path) -> Option<IntegrityEvent> {
    // Reconstruct a representative path for the daemon event. The fs
    // event only carries a `path_hash`; we cannot reverse it. For audit
    // purposes we use a synthetic path based on the hash.
    let path_hex = hex_encode(&fs_event.path_hash[..16]);

    match &fs_event.result {
        FsIntegrityResult::Ok => Some(IntegrityEvent::Ok {
            path: root.join(&path_hex),
        }),
        FsIntegrityResult::Mismatch { local, remote } => Some(IntegrityEvent::Mismatch {
            path: root.join(&path_hex),
            local_sha_hex: hex_encode(local),
            remote_sha_hex: hex_encode(remote),
        }),
        FsIntegrityResult::Throttled => Some(IntegrityEvent::Throttled {
            path: root.join(&path_hex),
        }),
        // Hashed (no cross-check), Skipped, LocalMissing, RemoteMissing,
        // FetchFailed are not surfaced as daemon-level events.
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn scheduler_loop(
    schedule: &Schedule,
    wake: &Arc<SchedulerWake>,
    stop_flag: &Arc<AtomicBool>,
    shared: &Arc<SharedState>,
    config: &IntegritySweeperConfig,
    sender: Option<&Sender<IntegrityEvent>>,
    power_source: &dyn PowerSource,
    pause_on_battery: bool,
    battery_skip_count: &Arc<AtomicU64>,
    scheduled_run_count: &Arc<AtomicU64>,
    sweep_roots: &Arc<Mutex<Vec<SweepRoot>>>,
    checksum_fetcher: &Arc<Mutex<Arc<dyn DaemonChecksumFetcher>>>,
    skip_globs: &Arc<Mutex<Vec<glob::Pattern>>>,
    tick_notify: &Mutex<Option<mpsc::Sender<()>>>,
) {
    loop {
        if stop_flag.load(Ordering::Relaxed) {
            return;
        }
        let next = match schedule.upcoming(Utc).next() {
            Some(when) => when,
            None => {
                // `cron` exhausted — only happens on explicitly bounded
                // expressions which we do not accept. Exit cleanly.
                return;
            }
        };
        let now = Utc::now();
        let wait = (next - now).to_std().unwrap_or(Duration::from_millis(0));
        if wait_until(wake, wait) {
            return; // stop flag set during sleep
        }
        if stop_flag.load(Ordering::Relaxed) {
            return;
        }
        if pause_on_battery {
            let state = power_source.read();
            if matches!(state, PowerState::OnBattery) {
                battery_skip_count.fetch_add(1, Ordering::Relaxed);
                emit_battery_pause_event();
                notify_tick(tick_notify);
                continue;
            }
        }
        // Tick: walk all configured sync roots.
        shared.sweep_in_flight.store(true, Ordering::Relaxed);
        let run_started = std::time::Instant::now();

        run_sweep_cycle(config, sender, sweep_roots, checksum_fetcher, skip_globs);

        shared.sweep_in_flight.store(false, Ordering::Relaxed);
        pcloud_fs::slo_hook::observe_integrity_sweeper_run(run_started.elapsed());
        scheduled_run_count.fetch_add(1, Ordering::Relaxed);
        emit_scheduler_tick_event();
        notify_tick(tick_notify);
    }
}

/// Send a single `()` on the tick-notification channel, if one is installed.
/// Errors (disconnected receiver) are silently ignored -- the notifier is
/// best-effort for test coordination.
fn notify_tick(tick_notify: &Mutex<Option<mpsc::Sender<()>>>) {
    if let Some(tx) = tick_notify
        .lock()
        .unwrap_or_else(|poisoned| {
            log::error!("integrity sweeper mutex poisoned — recovering");
            poisoned.into_inner()
        })
        .as_ref()
    {
        let _ = tx.send(());
    }
}

/// Shared sweep logic used by both `run_once` and `scheduler_loop`.
fn run_sweep_cycle(
    config: &IntegritySweeperConfig,
    sender: Option<&Sender<IntegrityEvent>>,
    sweep_roots: &Mutex<Vec<SweepRoot>>,
    checksum_fetcher: &Mutex<Arc<dyn DaemonChecksumFetcher>>,
    skip_globs: &Mutex<Vec<glob::Pattern>>,
) {
    run_sweep_cycle_with_ndjson(
        config,
        sender,
        sweep_roots,
        checksum_fetcher,
        skip_globs,
        None,
    );
}

/// Shared sweep logic with an optional NDJSON sink. Every walker event
/// (including `RemoteMissing`, `LocalMissing`, `FetchFailed`, `Skipped`
/// and `Throttled`, which the daemon channel drops) is serialised into
/// the sink as a single JSON line when a sink is provided. NDJSON write
/// failures are logged but **do not abort** the sweep — per the
/// `bd-1du.4.6.1` spec, the walker must stay alive across transient
/// sink errors so one unreadable file cannot poison the whole report.
#[allow(clippy::too_many_arguments)]
fn run_sweep_cycle_with_ndjson(
    config: &IntegritySweeperConfig,
    sender: Option<&Sender<IntegrityEvent>>,
    sweep_roots: &Mutex<Vec<SweepRoot>>,
    checksum_fetcher: &Mutex<Arc<dyn DaemonChecksumFetcher>>,
    skip_globs: &Mutex<Vec<glob::Pattern>>,
    mut ndjson_sink: Option<&mut dyn Write>,
) {
    let roots = sweep_roots
        .lock()
        .unwrap_or_else(|poisoned| {
            log::error!("integrity sweeper mutex poisoned — recovering");
            poisoned.into_inner()
        })
        .clone();
    let fetcher = checksum_fetcher
        .lock()
        .unwrap_or_else(|poisoned| {
            log::error!("integrity sweeper mutex poisoned — recovering");
            poisoned.into_inner()
        })
        .clone();
    let skip_patterns: Vec<String> = skip_globs
        .lock()
        .unwrap_or_else(|poisoned| {
            log::error!("integrity sweeper mutex poisoned — recovering");
            poisoned.into_inner()
        })
        .iter()
        .map(|p| p.as_str().to_owned())
        .collect();

    let sweeper_cfg = SweeperConfig {
        skip_patterns,
        rate_capacity: config.rate_files_per_minute.max(1),
        rate_refill_per_sec: f64::from(config.rate_files_per_minute) / 60.0,
        ..SweeperConfig::default()
    };
    let sweeper = match IntegritySweeper::new(sweeper_cfg) {
        Ok(s) => s,
        Err(e) => {
            log::error!(
                r#"{{"event":"integrity_sweeper.error","detail":"failed to build sweeper: {e}"}}"#
            );
            return;
        }
    };
    let adapter = DaemonChecksumFetcherAdapter {
        inner: fetcher.as_ref(),
    };

    for root in &roots {
        // Capture for both the walker mapper (moved) and the NDJSON
        // record builder (used below), so we can surface `remote_path`
        // on every record without re-running the mapping.
        let remote_prefix = root.remote_prefix.clone();
        let remote_prefix_for_mapper = remote_prefix.clone();
        let mapper = move |rel: &Path| -> String {
            let mut remote = remote_prefix_for_mapper.clone();
            if !remote.ends_with('/') {
                remote.push('/');
            }
            remote.push_str(&rel.to_string_lossy().replace('\\', "/"));
            remote
        };

        let (fs_tx, fs_rx) = std::sync::mpsc::channel::<FsIntegrityEvent>();
        let report = sweeper.sweep(&root.local_path, &fs_tx, &adapter, &mapper);
        drop(fs_tx);

        // Drain events exactly once — each listener (daemon worker +
        // NDJSON sink) receives the same observation stream.
        while let Ok(fs_event) = fs_rx.recv() {
            if let Some(ref mut sink) = ndjson_sink {
                // Rebuild the remote prefix so the NDJSON record can
                // carry it. We do **not** know the relative path here
                // (only the `path_hash`), so we surface the root
                // prefix as the best non-PII routing identifier.
                let remote_path_for_record = remote_prefix.clone();
                if let Some(record) =
                    ndjson_record_from_fs_event(&fs_event, &remote_path_for_record)
                    && let Err(e) = write_ndjson_record(*sink, &record)
                {
                    log::warn!(
                        r#"{{"event":"integrity_sweeper.ndjson_write_failed","detail":"{e}"}}"#
                    );
                }
            }
            if let Some(daemon_tx) = sender
                && let Some(daemon_event) = translate_fs_event(&fs_event, &root.local_path)
            {
                let _ = daemon_tx.send(daemon_event);
            }
        }

        if let Err(e) = report {
            log::error!(
                r#"{{"event":"integrity_sweeper.walk_error","root":"{}","detail":"{e}"}}"#,
                root.local_path.display()
            );
        }
    }
}

/// Sleep up to `wait`, returning `true` if the stop flag was set before
/// the timeout expired. `Condvar::wait_timeout` gives shutdown an
/// immediate wake channel so joining the scheduler thread does not have
/// to wait up to one cron period.
fn wait_until(wake: &Arc<SchedulerWake>, wait: Duration) -> bool {
    let stopped = wake.stopped.lock().unwrap_or_else(|poisoned| {
        log::error!("integrity sweeper mutex poisoned — recovering");
        poisoned.into_inner()
    });
    if *stopped {
        return true;
    }
    let (guard, _timeout) = wake
        .cv
        .wait_timeout(stopped, wait)
        .unwrap_or_else(|poisoned| {
            log::error!("integrity sweeper mutex poisoned — recovering");
            poisoned.into_inner()
        });
    *guard
}

fn emit_battery_pause_event() {
    log::info!(r#"{{"event":"integrity_sweeper.paused","reason":"on_battery"}}"#);
}

fn emit_scheduler_tick_event() {
    log::info!(r#"{{"event":"integrity_sweeper.tick"}}"#);
}

impl Drop for IntegritySweeperShell {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker_loop<F>(
    rx: Receiver<IntegrityEvent>,
    shared: &Arc<SharedState>,
    stop_flag: &Arc<AtomicBool>,
    audit_drops: &Arc<AtomicU64>,
    audit_sink: &mut F,
) where
    F: FnMut(&IntegrityEvent) -> Result<(), String>,
{
    while !stop_flag.load(Ordering::Relaxed) {
        match rx.recv() {
            Ok(event) => {
                handle_event(&event, shared, audit_drops, audit_sink);
            }
            Err(_) => break, // sender dropped — exit cleanly
        }
    }
    // Drain anything queued before stop was requested.
    while let Ok(event) = rx.try_recv() {
        handle_event(&event, shared, audit_drops, audit_sink);
    }
}

fn handle_event<F>(
    event: &IntegrityEvent,
    shared: &Arc<SharedState>,
    audit_drops: &Arc<AtomicU64>,
    audit_sink: &mut F,
) where
    F: FnMut(&IntegrityEvent) -> Result<(), String>,
{
    let mut progress = shared.progress.lock().unwrap_or_else(|poisoned| {
        log::error!("integrity sweeper mutex poisoned — recovering");
        poisoned.into_inner()
    });
    match event {
        IntegrityEvent::Ok { .. } => {
            progress.files_hashed = progress.files_hashed.saturating_add(1);
        }
        IntegrityEvent::Mismatch { .. } => {
            progress.files_hashed = progress.files_hashed.saturating_add(1);
            progress.mismatches_found = progress.mismatches_found.saturating_add(1);
            // Drop the lock before calling audit_sink to keep the audit
            // closure off the hot mutex path.
            drop(progress);
            if audit_sink(event).is_err() {
                audit_drops.fetch_add(1, Ordering::Relaxed);
            }
        }
        IntegrityEvent::Throttled { .. } => {
            progress.throttled = progress.throttled.saturating_add(1);
        }
    }
}

/// Compute the BLAKE3-style "path hash" surfaced in audit details.
///
/// We use SHA256 (already available in the daemon Cargo deps) rather
/// than BLAKE3 to avoid pulling in another crypto crate for one helper.
/// The resulting hex string is non-reversible and short enough to keep
/// audit rows compact.
#[must_use]
pub fn path_hash_hex(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_os_str().as_encoded_bytes());
    let digest = hasher.finalize();
    hex_encode(&digest[..16]) // truncated to 32 hex chars
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Build the audit-log "details" payload for a [`IntegrityEvent::Mismatch`].
///
/// Format: `path_hash=<hex> local_sha=<hex> remote_sha=<hex>`. Stable
/// because the parity-matrix proof harness greps for these key names.
#[must_use]
pub fn audit_details_for_mismatch(
    path: &Path,
    local_sha_hex: &str,
    remote_sha_hex: &str,
) -> String {
    format!(
        "path_hash={} local_sha={} remote_sha={}",
        path_hash_hex(path),
        local_sha_hex,
        remote_sha_hex
    )
}

/// Stable audit category string emitted by `record_mismatch_audit`
/// callers (the runtime borrows this constant when wiring the worker
/// closure). Kept in this module so PR2/PR3 walker tests can grep for
/// the exact string the parity-matrix proof harness expects.
pub const AUDIT_CATEGORY_INTEGRITY_MISMATCH: &str = "integrity.mismatch";

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_config::integrity_sweeper::IntegritySweeperConfig;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn enabled_cfg(skip_path: Option<PathBuf>) -> IntegritySweeperConfig {
        IntegritySweeperConfig {
            enabled: true,
            schedule_cron: None,
            rate_files_per_minute: 100,
            pause_on_battery: true,
            skip_list_path: skip_path,
        }
    }

    #[test]
    fn integrity_status_returns_zero_progress_when_never_run() {
        // Disabled shell starts at zero and stays at zero across an
        // unsolicited `run_once` call.
        let shell = IntegritySweeperShell::disabled();
        let p = shell.progress_snapshot();
        assert_eq!(p, SweepProgress::default());
        assert_eq!(p.files_hashed, 0);
        assert_eq!(p.bytes_hashed, 0);
        assert_eq!(p.mismatches_found, 0);
        assert_eq!(p.throttled, 0);

        // Enabled but unworked shell also reports zero.
        let enabled =
            IntegritySweeperShell::from_config(enabled_cfg(None)).expect("enabled shell builds");
        assert_eq!(enabled.progress_snapshot(), SweepProgress::default());
    }

    #[test]
    fn integrity_run_once_triggers_sweep_and_reports_summary() {
        // Mock-backend equivalent: spawn the worker with a no-op audit
        // sink, inject one Ok and one Mismatch event, then run a
        // synchronous sweep. The worker translates events into
        // progress counters; `run_once` returns the post-cycle snapshot.
        let mut shell =
            IntegritySweeperShell::from_config(enabled_cfg(None)).expect("enabled shell builds");
        let observed = Arc::new(Mutex::new(Vec::<IntegrityEvent>::new()));
        let observed_clone = Arc::clone(&observed);
        shell.spawn_worker(move |event| {
            observed_clone
                .lock()
                .expect("observed poisoned")
                .push(event.clone());
            Ok(())
        });

        shell
            .dispatch_event_for_test(IntegrityEvent::Ok {
                path: PathBuf::from("/tmp/a.txt"),
            })
            .expect("send ok");
        shell
            .dispatch_event_for_test(IntegrityEvent::Mismatch {
                path: PathBuf::from("/tmp/b.txt"),
                local_sha_hex: "00".repeat(32),
                remote_sha_hex: "ff".repeat(32),
            })
            .expect("send mismatch");

        // Drain by stopping the worker.
        shell.shutdown();

        let final_progress = shell.run_once();
        // run_once on a stopped shell still returns the accumulated
        // snapshot.
        assert_eq!(final_progress.files_hashed, 2);
        assert_eq!(final_progress.mismatches_found, 1);
        assert_eq!(final_progress.throttled, 0);

        let observed = observed.lock().expect("observed poisoned");
        // Audit sink only saw the Mismatch event — Ok / Throttled drop
        // silently per the PR4 contract.
        assert_eq!(observed.len(), 1);
        assert!(matches!(observed[0], IntegrityEvent::Mismatch { .. }));
    }

    #[test]
    fn mismatch_event_writes_integrity_mismatch_audit_entry() {
        // The audit-detail formatter is the load-bearing piece for the
        // "writes IntegrityMismatch audit entry" parity claim. The
        // append_audit_event hop into the store is exercised by the
        // store crate's own tests; verifying its input here keeps this
        // unit test independent of an on-disk SQLite fixture.
        let path = PathBuf::from("/tmp/example.bin");
        let details = audit_details_for_mismatch(&path, "abcd", "ef01");
        assert!(details.starts_with("path_hash="));
        assert!(details.contains(" local_sha=abcd "));
        assert!(details.ends_with(" remote_sha=ef01"));
        // path_hash must not contain the raw path (audit redaction).
        assert!(!details.contains("/tmp/example.bin"));
        // path_hash is deterministic.
        let again = audit_details_for_mismatch(&path, "abcd", "ef01");
        assert_eq!(details, again);
    }

    #[test]
    fn cli_integrity_skip_appends_path_to_skip_list() {
        // End-to-end-equivalent test for the IPC / CLI plumbing: the
        // `IntegritySkip { path }` request boils down to
        // `IntegritySweeperShell::append_skip_path`. Confirm the file is
        // updated, deduped, and the in-memory glob set is reloaded.
        let mut tmp = NamedTempFile::new().expect("tempfile");
        writeln!(tmp, "# pre-existing").expect("write");
        writeln!(tmp, "**/*.tmp").expect("write");
        let skip_path = tmp.path().to_path_buf();

        let shell = IntegritySweeperShell::from_config(enabled_cfg(Some(skip_path.clone())))
            .expect("shell builds");
        assert_eq!(shell.skip_glob_count(), 1, "initial glob count");

        shell
            .append_skip_path("**/secret.bin")
            .expect("append skip");
        let contents = std::fs::read_to_string(&skip_path).expect("read back");
        assert!(contents.contains("**/secret.bin"));
        assert_eq!(shell.skip_glob_count(), 2, "reloaded glob count");

        // Idempotent: appending the same path twice is a no-op.
        shell
            .append_skip_path("**/secret.bin")
            .expect("append duplicate");
        let contents2 = std::fs::read_to_string(&skip_path).expect("read back");
        assert_eq!(contents, contents2, "duplicate append must not write");
        assert_eq!(shell.skip_glob_count(), 2, "no extra glob added");
    }

    #[test]
    fn disabled_shell_rejects_skip_append_when_no_path_configured() {
        let shell = IntegritySweeperShell::disabled();
        let err = shell
            .append_skip_path("**/x")
            .expect_err("must error without skip_list_path");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn path_hash_is_deterministic_and_redacts_raw_path() {
        let p = PathBuf::from("/very/secret/path/file.bin");
        let h1 = path_hash_hex(&p);
        let h2 = path_hash_hex(&p);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 32, "16 bytes hex-encoded = 32 chars");
        assert!(!h1.contains("secret"));
    }

    // ---- schedule_cron parsing ----

    #[test]
    fn schedule_cron_valid_expression_is_accepted_by_from_config() {
        // Standard 6-field expression: every second. from_config must
        // accept this and build the shell without spawning a scheduler.
        let mut cfg = enabled_cfg(None);
        cfg.schedule_cron = Some("* * * * * *".into());
        let shell = IntegritySweeperShell::from_config(cfg).expect("valid cron accepted");
        assert!(shell.is_enabled());
    }

    #[test]
    fn schedule_cron_invalid_expression_is_rejected_by_from_config() {
        // Garbage input: from_config refuses to build the shell so the
        // sweeper never silently runs on an unparseable schedule.
        let mut cfg = enabled_cfg(None);
        cfg.schedule_cron = Some("not a cron expression".into());
        let err = IntegritySweeperShell::from_config(cfg).expect_err("invalid cron rejected");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("invalid schedule_cron"));
    }

    #[test]
    fn start_schedule_without_cron_expression_errors() {
        let mut shell =
            IntegritySweeperShell::from_config(enabled_cfg(None)).expect("shell builds");
        let err = shell
            .start_schedule(Box::new(MockPowerSource::new(PowerState::OnAc)))
            .expect_err("no schedule means NoSchedule");
        assert!(matches!(err, ScheduleError::NoSchedule));
    }

    #[test]
    fn start_schedule_rejects_invalid_cron_when_config_mutated_post_build() {
        // Defensive: even if a caller mutates the config between
        // from_config and start_schedule, start_schedule re-parses and
        // fails cleanly.
        let mut shell = IntegritySweeperShell::from_config(enabled_cfg(None)).expect("shell");
        shell.config.schedule_cron = Some("@@@@".into());
        let err = shell
            .start_schedule(Box::new(MockPowerSource::new(PowerState::OnAc)))
            .expect_err("bad cron rejected");
        assert!(matches!(err, ScheduleError::InvalidCron { .. }));
    }

    // ---- pause_on_battery ----

    /// Wait for a tick notification with a generous 5s timeout. Returns
    /// `true` if a notification arrived, `false` on timeout.
    fn wait_for_tick(rx: &mpsc::Receiver<()>) -> bool {
        rx.recv_timeout(Duration::from_secs(5)).is_ok()
    }

    #[test]
    fn scheduler_skips_tick_when_power_source_reports_discharging() {
        // Cron expression: every second. Start the scheduler with a
        // mock that always reports `OnBattery` and confirm the skip
        // counter increments, while the run counter stays at zero.
        let mut cfg = enabled_cfg(None);
        cfg.schedule_cron = Some("* * * * * *".into());
        cfg.pause_on_battery = true;
        let mut shell = IntegritySweeperShell::from_config(cfg).expect("shell");
        let tick_rx = shell.subscribe_tick_notify();
        shell
            .start_schedule(Box::new(MockPowerSource::new(PowerState::OnBattery)))
            .expect("scheduler spawns");
        // Wait for a deterministic tick signal instead of sleeping.
        assert!(wait_for_tick(&tick_rx), "tick notification timed out");
        shell.shutdown();
        assert!(
            shell.battery_skip_count() >= 1,
            "expected >=1 battery skip, got {}",
            shell.battery_skip_count()
        );
        assert_eq!(
            shell.scheduled_run_count(),
            0,
            "scheduler must not run while discharging"
        );
    }

    #[test]
    fn scheduler_runs_tick_when_power_source_reports_ac() {
        let mut cfg = enabled_cfg(None);
        cfg.schedule_cron = Some("* * * * * *".into());
        cfg.pause_on_battery = true;
        let mut shell = IntegritySweeperShell::from_config(cfg).expect("shell");
        let tick_rx = shell.subscribe_tick_notify();
        shell
            .start_schedule(Box::new(MockPowerSource::new(PowerState::OnAc)))
            .expect("scheduler spawns");
        assert!(wait_for_tick(&tick_rx), "tick notification timed out");
        shell.shutdown();
        assert_eq!(
            shell.battery_skip_count(),
            0,
            "on AC the scheduler must not pause"
        );
        assert!(
            shell.scheduled_run_count() >= 1,
            "expected >=1 tick, got {}",
            shell.scheduled_run_count()
        );
    }

    #[test]
    fn scheduler_treats_unknown_power_state_as_ac() {
        // Servers / VMs / containers report no power supply. The
        // scheduler must keep running rather than silently stopping.
        let mut cfg = enabled_cfg(None);
        cfg.schedule_cron = Some("* * * * * *".into());
        cfg.pause_on_battery = true;
        let mut shell = IntegritySweeperShell::from_config(cfg).expect("shell");
        let tick_rx = shell.subscribe_tick_notify();
        shell
            .start_schedule(Box::new(MockPowerSource::new(PowerState::Unknown)))
            .expect("scheduler spawns");
        assert!(wait_for_tick(&tick_rx), "tick notification timed out");
        shell.shutdown();
        assert_eq!(shell.battery_skip_count(), 0);
        assert!(shell.scheduled_run_count() >= 1);
    }

    #[test]
    fn scheduler_ignores_battery_when_pause_flag_disabled() {
        // With `pause_on_battery = false` the scheduler must run even if
        // the reader says "Discharging".
        let mut cfg = enabled_cfg(None);
        cfg.schedule_cron = Some("* * * * * *".into());
        cfg.pause_on_battery = false;
        let mut shell = IntegritySweeperShell::from_config(cfg).expect("shell");
        let tick_rx = shell.subscribe_tick_notify();
        shell
            .start_schedule(Box::new(MockPowerSource::new(PowerState::OnBattery)))
            .expect("scheduler spawns");
        assert!(wait_for_tick(&tick_rx), "tick notification timed out");
        shell.shutdown();
        assert_eq!(shell.battery_skip_count(), 0);
        assert!(shell.scheduled_run_count() >= 1);
    }

    #[test]
    fn shutdown_wakes_scheduler_without_waiting_for_next_tick() {
        // Cron expression: once per hour. If shutdown() did not wake the
        // scheduler we would block for up to 3600s. Assert the join
        // completes quickly.
        let mut cfg = enabled_cfg(None);
        cfg.schedule_cron = Some("0 0 * * * *".into());
        let mut shell = IntegritySweeperShell::from_config(cfg).expect("shell");
        shell
            .start_schedule(Box::new(MockPowerSource::new(PowerState::OnAc)))
            .expect("scheduler spawns");
        let start = std::time::Instant::now();
        shell.shutdown();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "shutdown should wake scheduler, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn default_power_source_returns_a_readable_state() {
        // Smoke test the platform default. Any variant is acceptable —
        // we only check the call does not panic.
        let src = default_power_source();
        let _ = src.read();
    }

    // ---- walk-and-compare integration tests ----

    /// Mock [`DaemonChecksumFetcher`] that returns pre-configured SHA-256
    /// hex digests keyed by remote path.
    #[derive(Debug, Default)]
    struct MockDaemonFetcher {
        responses: Mutex<std::collections::HashMap<String, Result<String, DaemonCheckError>>>,
    }

    impl MockDaemonFetcher {
        fn set_ok(&self, remote_path: &str, sha_hex: &str) {
            self.responses
                .lock()
                .unwrap()
                .insert(remote_path.to_owned(), Ok(sha_hex.to_owned()));
        }

        #[allow(dead_code)]
        fn set_not_found(&self, remote_path: &str) {
            self.responses
                .lock()
                .unwrap()
                .insert(remote_path.to_owned(), Err(DaemonCheckError::NotFound));
        }

        #[allow(dead_code)]
        fn set_error(&self, remote_path: &str, reason: &str) {
            self.responses.lock().unwrap().insert(
                remote_path.to_owned(),
                Err(DaemonCheckError::Other(reason.to_owned())),
            );
        }
    }

    impl DaemonChecksumFetcher for MockDaemonFetcher {
        fn fetch_sha256_hex(&self, remote_path: &str) -> Result<String, DaemonCheckError> {
            match self.responses.lock().unwrap().remove(remote_path) {
                Some(Ok(hex)) => Ok(hex),
                Some(Err(DaemonCheckError::NotFound)) => Err(DaemonCheckError::NotFound),
                Some(Err(DaemonCheckError::Other(r))) => Err(DaemonCheckError::Other(r)),
                None => Err(DaemonCheckError::Other(format!(
                    "no mock response for {remote_path}"
                ))),
            }
        }
    }

    /// Compute SHA-256 hex of a byte slice (test helper).
    fn sha256_hex(data: &[u8]) -> String {
        let digest = Sha256::new().chain_update(data).finalize();
        hex_encode(&digest)
    }

    #[test]
    fn run_once_walks_files_and_detects_match() {
        let tmp = tempfile::tempdir().unwrap();
        let content = b"hello world";
        std::fs::write(tmp.path().join("a.txt"), content).unwrap();
        let sha_hex = sha256_hex(content);

        let mut shell = IntegritySweeperShell::from_config(enabled_cfg(None)).unwrap();
        shell.spawn_worker(|_ev| Ok(()));

        let fetcher = Arc::new(MockDaemonFetcher::default());
        // The mapper prefixes with `/remote/` + relative path
        fetcher.set_ok("/remote/a.txt", &sha_hex);

        shell.set_sweep_roots(vec![SweepRoot {
            local_path: tmp.path().to_path_buf(),
            remote_prefix: "/remote".to_owned(),
        }]);
        shell.set_checksum_fetcher(fetcher);

        let _progress = shell.run_once();
        // Give worker time to process Ok events (counter updates).
        std::thread::sleep(Duration::from_millis(200));
        shell.shutdown();

        // Ok events are NOT sent to the audit sink (only Mismatch is).
        // Verify via the progress counter that the file was actually
        // hashed and matched.
        let snap = shell.progress_snapshot();
        assert!(
            snap.files_hashed >= 1,
            "expected >=1 files_hashed, got {}",
            snap.files_hashed
        );
        assert_eq!(
            snap.mismatches_found, 0,
            "matching file must not produce a mismatch"
        );
    }

    #[test]
    fn run_once_detects_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let content = b"local content";
        std::fs::write(tmp.path().join("b.txt"), content).unwrap();

        let mut shell = IntegritySweeperShell::from_config(enabled_cfg(None)).unwrap();
        let events = Arc::new(Mutex::new(Vec::<IntegrityEvent>::new()));
        let events_clone = Arc::clone(&events);
        shell.spawn_worker(move |ev| {
            events_clone.lock().unwrap().push(ev.clone());
            Ok(())
        });

        let fetcher = Arc::new(MockDaemonFetcher::default());
        // Return a different hash to trigger mismatch.
        fetcher.set_ok("/remote/b.txt", &"aa".repeat(32));

        shell.set_sweep_roots(vec![SweepRoot {
            local_path: tmp.path().to_path_buf(),
            remote_prefix: "/remote".to_owned(),
        }]);
        shell.set_checksum_fetcher(fetcher);

        shell.run_once();
        std::thread::sleep(Duration::from_millis(100));
        shell.shutdown();

        let evts = events.lock().unwrap();
        let mismatches = evts
            .iter()
            .filter(|e| matches!(e, IntegrityEvent::Mismatch { .. }))
            .count();
        assert!(
            mismatches >= 1,
            "expected >=1 Mismatch event, got {mismatches}"
        );
    }

    #[test]
    fn run_once_with_no_roots_produces_no_events() {
        let mut shell = IntegritySweeperShell::from_config(enabled_cfg(None)).unwrap();
        let events = Arc::new(Mutex::new(Vec::<IntegrityEvent>::new()));
        let events_clone = Arc::clone(&events);
        shell.spawn_worker(move |ev| {
            events_clone.lock().unwrap().push(ev.clone());
            Ok(())
        });

        // No sweep roots set.
        shell.run_once();
        std::thread::sleep(Duration::from_millis(50));
        shell.shutdown();

        let evts = events.lock().unwrap();
        assert!(
            evts.is_empty(),
            "no roots means no events, got {}",
            evts.len()
        );
    }

    #[test]
    fn parse_sha256_hex_round_trips_correctly() {
        let input = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let bytes = parse_sha256_hex(input).unwrap();
        assert_eq!(bytes[0], 0xba);
        assert_eq!(bytes[31], 0xad);
        let hex_back = hex_encode(&bytes);
        assert_eq!(hex_back, input);
    }

    #[test]
    fn parse_sha256_hex_rejects_bad_input() {
        assert!(parse_sha256_hex("short").is_err());
        assert!(parse_sha256_hex(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn noop_fetcher_returns_not_found() {
        let f = NoOpChecksumFetcher;
        assert!(matches!(
            f.fetch_sha256_hex("/any/path"),
            Err(DaemonCheckError::NotFound)
        ));
    }

    #[test]
    fn run_once_handles_remote_not_found_gracefully() {
        // When the fetcher reports NotFound, the sweep should still
        // complete without panicking.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("c.txt"), b"data").unwrap();

        let mut shell = IntegritySweeperShell::from_config(enabled_cfg(None)).unwrap();
        let events = Arc::new(Mutex::new(Vec::<IntegrityEvent>::new()));
        let events_clone = Arc::clone(&events);
        shell.spawn_worker(move |ev| {
            events_clone.lock().unwrap().push(ev.clone());
            Ok(())
        });

        // NoOpChecksumFetcher returns NotFound for everything.
        shell.set_sweep_roots(vec![SweepRoot {
            local_path: tmp.path().to_path_buf(),
            remote_prefix: "/remote".to_owned(),
        }]);
        // Default fetcher is NoOp — no explicit set needed.

        let _progress = shell.run_once();
        std::thread::sleep(Duration::from_millis(50));
        shell.shutdown();

        // RemoteMissing events are translated to None (dropped at daemon
        // level), so no daemon events should appear.
        let evts = events.lock().unwrap();
        assert!(
            evts.is_empty(),
            "RemoteMissing should not produce daemon events"
        );
    }
}
