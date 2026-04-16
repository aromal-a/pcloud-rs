//! Scheduled audit-chain verifier (I04 follow-up).
//!
//! ## Purpose
//!
//! Run the tamper-evident audit-chain verification on a cron schedule
//! inside the daemon process so an offline attacker cannot rewrite the
//! SQLite `audit_events` table without a visible SLO drop and a
//! structured `audit.chain.broken` event. On-demand verification via
//! `pcloudc audit verify` (and the `Request::AuditVerifyChain`
//! IPC verb) remains unchanged; this service adds the *periodic*
//! self-verification the original audit finding called out as missing.
//!
//! ## Security posture
//!
//! - **On by default.** Unlike the integrity sweeper, audit-chain
//!   verification is a read-only walk over an already-persisted table
//!   and the signal is load-bearing. The scheduler runs at 03:00 daily
//!   unless the operator explicitly disables it.
//! - **No secret material persisted.** The optional checkpoint file
//!   records only `{last_run_ts, last_verified_id}`; it is written
//!   `0600` with a `0700` parent. HMAC key material (when
//!   `PCLOUD_AUDIT_HMAC_KEY` is set) is pulled from the process
//!   environment on each run and never touches disk.
//! - **Fail-closed cron parse.** An invalid `schedule_cron` expression
//!   is rejected at [`AuditVerifierShell::start_schedule`] time; the
//!   scheduler never silently fails to run.
//! - **Structured broken-chain event.** On failure the service emits a
//!   single-line JSON record `{"event":"audit.chain.broken",…}` to
//!   stderr (ingested by journald/json-logs) *and* bumps the SLO
//!   failure counter (`observe_audit_verify(false)`), so both metric
//!   and log pipelines see the break.
//!
//! ## Honest scope
//!
//! The service walks the chain via [`pcloud_store::verify_audit_chain`].
//! When the chain is intact it does not emit per-row detail; only the
//! aggregate `(entries_checked, first_id, last_id)` tuple is recorded on
//! the shell's status snapshot. When the chain is broken the broken-row
//! id and the error message from [`pcloud_store::repositories::audit::AuditChainError`]
//! are both captured and surfaced via IPC `Method::GetAuditVerifierStatus`.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::Utc;
use cron::Schedule;
use pcloud_config::audit_verifier::AuditVerifierConfig;
use pcloud_observability::slo::Slo;
use serde::{Deserialize, Serialize};

/// Outcome of the most recent verifier tick. Stored on the shell so
/// `Method::GetAuditVerifierStatus` can render a meaningful summary even
/// when the cron schedule has not yet fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifierOutcome {
    /// No tick has fired since the daemon started.
    NeverRun,
    /// Last tick completed successfully. Records the row count walked.
    Pass {
        /// Number of rows walked by the most recent run.
        chain_length: u64,
    },
    /// Last tick detected a broken chain. The `detail` string is the
    /// `Display` representation of the underlying
    /// [`pcloud_store::repositories::audit::AuditChainError`] so
    /// operators can find the offending row id without re-running
    /// `pcloudc audit verify` by hand.
    Fail {
        /// Chain length observed before the break (i.e. `entries_checked`
        /// returned by the partial walk, or `0` when the first row is
        /// already broken).
        chain_length: u64,
        /// Human-readable failure detail. Example: `"audit chain broken
        /// at id=42: entry_hash mismatch"`.
        detail: String,
    },
}

/// Persisted last-known-good checkpoint. Written after every successful
/// run to the optional `checkpoint_path`. Only row ids and timestamps;
/// no secret material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unix seconds of the checkpoint write.
    pub last_run_ts: i64,
    /// Last row id the verifier walked successfully. Subsequent runs
    /// verify from `last_verified_id + 1` to the current tail.
    pub last_verified_id: Option<i64>,
}

/// Internal wake-condition used by the scheduler thread. A `Condvar`
/// lets `shutdown` interrupt a long sleep without waiting for the next
/// tick.
#[derive(Debug, Default)]
struct SchedulerWake {
    stopped: Mutex<bool>,
    cv: Condvar,
}

/// Shared state read by `Method::GetAuditVerifierStatus` and written by
/// the scheduler thread. `Mutex` rather than atomics because the
/// `VerifierOutcome::Fail { detail }` string cannot be stored in a
/// single atomic cell.
#[derive(Debug, Default)]
struct SharedStatus {
    /// Latest outcome. `VerifierOutcome::NeverRun` on fresh start.
    outcome: Mutex<VerifierOutcomeStorage>,
}

/// Internal mutable storage; `VerifierOutcome` is `Clone` but we want a
/// stable default that serialises to `"never_run"`.
#[derive(Debug)]
struct VerifierOutcomeStorage(VerifierOutcome);

impl Default for VerifierOutcomeStorage {
    fn default() -> Self {
        Self(VerifierOutcome::NeverRun)
    }
}

/// Errors raised by [`AuditVerifierShell::start_schedule`]. Mirrors the
/// integrity sweeper's [`crate::runtime::integrity_sweeper_service::ScheduleError`]
/// shape so operators see a consistent failure vocabulary across the two
/// scheduled services.
#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    /// `schedule_cron` was not a valid 6- or 7-field cron expression.
    #[error("invalid schedule_cron {expr:?}: {source}")]
    InvalidCron {
        /// The offending expression.
        expr: String,
        /// Parser error from the `cron` crate.
        #[source]
        source: cron::error::Error,
    },
    /// `[features.audit_verifier] enabled = false`; the scheduler refuses
    /// to start in disabled mode.
    #[error("audit verifier is disabled")]
    Disabled,
}

/// Test-only pluggable runner abstraction. Production callers use
/// [`StoreVerifierRunner`]; unit tests inject a lambda that returns a
/// canned outcome so we can drive the failure path without staging a
/// real SQLite tamper.
///
/// The trait is `Send + Sync` so the scheduler thread can invoke it.
pub trait VerifierRunner: Send + Sync {
    /// Verify the chain starting at `start_from` (exclusive-lower via
    /// `start_from + 1` semantics when `Some`, genesis when `None`).
    /// Returns `(outcome, latest_id)` where `latest_id` is the highest
    /// row id observed during this run (used to update the checkpoint
    /// on success).
    fn run(&self, start_from: Option<i64>) -> (VerifierOutcome, Option<i64>);
}

/// Production runner: walks the real audit table through
/// [`pcloud_store::verify_audit_chain`]. Pulls the HMAC key from
/// `PCLOUD_AUDIT_HMAC_KEY` on every tick so key rotation is picked up
/// without a daemon restart.
pub struct StoreVerifierRunner {
    db_path: PathBuf,
}

impl StoreVerifierRunner {
    /// Build a runner bound to `db_path`. The path is stored verbatim —
    /// the caller must ensure `StoreProfile::db_path` is owner-only
    /// (`bootstrap_profile` guarantees `0o600`).
    #[must_use]
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }
}

impl VerifierRunner for StoreVerifierRunner {
    fn run(&self, start_from: Option<i64>) -> (VerifierOutcome, Option<i64>) {
        // Pull key on every tick so operator rotations take effect
        // without a daemon restart. Empty / missing key → hash-only walk.
        let key = std::env::var_os("PCLOUD_AUDIT_HMAC_KEY")
            .map(|raw| raw.to_string_lossy().into_owned().into_bytes())
            .filter(|k| !k.is_empty());
        let from = start_from.map(|id| id.saturating_add(1));
        match pcloud_store::verify_audit_chain(&self.db_path, from, None, key) {
            Ok(v) => {
                let outcome = VerifierOutcome::Pass {
                    chain_length: v.entries_checked as u64,
                };
                (outcome, v.last_id)
            }
            Err(err) => {
                // Best-effort capture of the partial walk length. The
                // store helper does not expose it, so we report `0`
                // here; operators can run `pcloudc audit verify` to
                // bisect manually.
                let detail = err.to_string();
                (
                    VerifierOutcome::Fail {
                        chain_length: 0,
                        detail,
                    },
                    None,
                )
            }
        }
    }
}

/// Daemon-side handle to the scheduled audit verifier.
///
/// Always present on the runtime even when disabled; the disabled path
/// is a no-op shell so IPC `Method::GetAuditVerifierStatus` returns a
/// stable payload with `enabled = false` rather than the generic
/// `InvalidRequest`.
#[derive(Debug)]
pub struct AuditVerifierShell {
    config: AuditVerifierConfig,
    shared: Arc<SharedStatus>,
    /// Cumulative successful runs since daemon start.
    total_passes: Arc<AtomicU64>,
    /// Cumulative failed runs since daemon start.
    total_failures: Arc<AtomicU64>,
    /// Unix seconds of the most recent run. `0` when never run.
    last_run_ts: Arc<AtomicI64>,
    /// Set to `true` to interrupt the scheduler's `wait_timeout`.
    stop_flag: Arc<AtomicBool>,
    /// Wake channel used to break out of sleeps on shutdown.
    scheduler_wake: Arc<SchedulerWake>,
    /// Scheduler thread handle (`Some` iff a schedule is running).
    scheduler_handle: Option<JoinHandle<()>>,
    /// Monotone counter of ticks the scheduler actually fired. Test
    /// helper; production code reads `total_passes + total_failures`
    /// instead.
    scheduled_run_count: Arc<AtomicU64>,
    /// Observed id of the last successfully verified row (cached so the
    /// scheduler can skip re-hashing the already-walked prefix when a
    /// checkpoint path is configured).
    last_verified_id: Arc<Mutex<Option<i64>>>,
}

impl AuditVerifierShell {
    /// Build a disabled shell. Safe default that performs no I/O and
    /// spawns no thread.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            config: AuditVerifierConfig {
                enabled: false,
                ..AuditVerifierConfig::default()
            },
            shared: Arc::new(SharedStatus::default()),
            total_passes: Arc::new(AtomicU64::new(0)),
            total_failures: Arc::new(AtomicU64::new(0)),
            last_run_ts: Arc::new(AtomicI64::new(0)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            scheduler_wake: Arc::new(SchedulerWake::default()),
            scheduler_handle: None,
            scheduled_run_count: Arc::new(AtomicU64::new(0)),
            last_verified_id: Arc::new(Mutex::new(None)),
        }
    }

    /// Build a shell from a validated configuration. When
    /// `cfg.enabled = false` this returns the disabled shell. When
    /// `true`, the cron expression is pre-validated so a typo is caught
    /// before any thread is spawned; the scheduler itself is started by
    /// [`AuditVerifierShell::start_schedule`].
    ///
    /// # Errors
    ///
    /// Returns `Err` when `cfg.schedule_cron` fails to parse.
    pub fn from_config(cfg: AuditVerifierConfig) -> std::io::Result<Self> {
        if !cfg.enabled {
            return Ok(Self::disabled());
        }
        if let Err(source) = Schedule::from_str(&cfg.schedule_cron) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "invalid audit_verifier schedule_cron {:?}: {source}",
                    cfg.schedule_cron
                ),
            ));
        }
        // Load a persisted checkpoint if one exists — lets the first
        // scheduled tick skip the already-walked prefix.
        let last_verified_id = match cfg.checkpoint_path.as_deref() {
            Some(p) => load_checkpoint(p).unwrap_or(None),
            None => None,
        };
        Ok(Self {
            config: cfg,
            shared: Arc::new(SharedStatus::default()),
            total_passes: Arc::new(AtomicU64::new(0)),
            total_failures: Arc::new(AtomicU64::new(0)),
            last_run_ts: Arc::new(AtomicI64::new(0)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            scheduler_wake: Arc::new(SchedulerWake::default()),
            scheduler_handle: None,
            scheduled_run_count: Arc::new(AtomicU64::new(0)),
            last_verified_id: Arc::new(Mutex::new(last_verified_id)),
        })
    }

    /// Whether the operator has opted into the scheduled verifier.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Snapshot the current status for the IPC surface.
    #[must_use]
    pub fn status_snapshot(&self) -> pcloud_ipc::AuditVerifierStatusPayload {
        let outcome = self
            .shared
            .outcome
            .lock()
            .map(|g| g.0.clone())
            .unwrap_or(VerifierOutcome::NeverRun);
        let (last_result, chain_length, last_error) = match outcome {
            VerifierOutcome::NeverRun => ("never_run".to_owned(), 0, String::new()),
            VerifierOutcome::Pass { chain_length } => {
                ("pass".to_owned(), chain_length, String::new())
            }
            VerifierOutcome::Fail {
                chain_length,
                detail,
            } => ("fail".to_owned(), chain_length, detail),
        };
        pcloud_ipc::AuditVerifierStatusPayload {
            enabled: self.config.enabled,
            last_run_ts: self.last_run_ts.load(Ordering::Relaxed),
            last_result,
            chain_length,
            last_error,
            total_passes: self.total_passes.load(Ordering::Relaxed),
            total_failures: self.total_failures.load(Ordering::Relaxed),
        }
    }

    /// Synchronously run the verifier once, feeding the SLO
    /// observation and updating the shell snapshot. Returns the
    /// post-run outcome for callers who need the result immediately
    /// (the integration test uses this to drive the broken-chain path
    /// without waiting for cron). Production code reaches this helper
    /// indirectly through [`AuditVerifierShell::start_schedule`].
    pub fn run_once(&self, runner: &dyn VerifierRunner, slo: &Slo) -> VerifierOutcome {
        let start_from = self.last_verified_id.lock().map(|g| *g).unwrap_or(None);
        let (outcome, latest_id) = runner.run(start_from);
        let now_ts = Utc::now().timestamp();
        self.last_run_ts.store(now_ts, Ordering::Relaxed);
        self.scheduled_run_count.fetch_add(1, Ordering::Relaxed);
        match &outcome {
            VerifierOutcome::Pass { chain_length } => {
                self.total_passes.fetch_add(1, Ordering::Relaxed);
                slo.observe_audit_verify(true);
                if let Some(id) = latest_id {
                    if let Ok(mut g) = self.last_verified_id.lock() {
                        *g = Some(id);
                    }
                    if let Some(p) = self.config.checkpoint_path.as_deref() {
                        let _ = save_checkpoint(
                            p,
                            &Checkpoint {
                                last_run_ts: now_ts,
                                last_verified_id: Some(id),
                            },
                        );
                    }
                }
                emit_pass_event(*chain_length);
            }
            VerifierOutcome::Fail {
                chain_length,
                detail,
            } => {
                self.total_failures.fetch_add(1, Ordering::Relaxed);
                slo.observe_audit_verify(false);
                emit_broken_event(*chain_length, detail);
            }
            VerifierOutcome::NeverRun => {
                // The runner must never return NeverRun; treat defensively.
            }
        }
        if let Ok(mut g) = self.shared.outcome.lock() {
            g.0 = outcome.clone();
        }
        outcome
    }

    /// Spawn the cron-driven scheduler thread.
    ///
    /// Idempotent: a second call is a no-op. Returns
    /// [`ScheduleError::Disabled`] when the verifier is disabled so the
    /// caller can surface the misconfiguration rather than silently
    /// doing nothing.
    ///
    /// # Errors
    ///
    /// - [`ScheduleError::InvalidCron`] when the expression fails to
    ///   parse via the `cron` crate.
    /// - [`ScheduleError::Disabled`] when `config.enabled = false`.
    pub fn start_schedule(
        &mut self,
        runner: Arc<dyn VerifierRunner>,
        slo: Arc<Slo>,
    ) -> Result<(), ScheduleError> {
        if self.scheduler_handle.is_some() {
            return Ok(());
        }
        if !self.config.enabled {
            return Err(ScheduleError::Disabled);
        }
        let schedule = Schedule::from_str(&self.config.schedule_cron).map_err(|source| {
            ScheduleError::InvalidCron {
                expr: self.config.schedule_cron.clone(),
                source,
            }
        })?;
        let wake = Arc::clone(&self.scheduler_wake);
        let stop_flag = Arc::clone(&self.stop_flag);
        let shared = Arc::clone(&self.shared);
        let total_passes = Arc::clone(&self.total_passes);
        let total_failures = Arc::clone(&self.total_failures);
        let last_run_ts = Arc::clone(&self.last_run_ts);
        let scheduled_run_count = Arc::clone(&self.scheduled_run_count);
        let last_verified_id = Arc::clone(&self.last_verified_id);
        let checkpoint_path = self.config.checkpoint_path.clone();

        let handle = thread::Builder::new()
            .name("pcloudd-audit-verifier".into())
            .spawn(move || {
                scheduler_loop(
                    &schedule,
                    &wake,
                    &stop_flag,
                    &shared,
                    &total_passes,
                    &total_failures,
                    &last_run_ts,
                    &scheduled_run_count,
                    &last_verified_id,
                    checkpoint_path.as_deref(),
                    runner.as_ref(),
                    slo.as_ref(),
                );
            })
            .expect("spawn audit verifier scheduler thread");
        self.scheduler_handle = Some(handle);
        Ok(())
    }

    /// Signal the scheduler to stop and join its thread. Safe to call
    /// even when no scheduler is running.
    pub fn shutdown(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Ok(mut stopped) = self.scheduler_wake.stopped.lock() {
            *stopped = true;
            self.scheduler_wake.cv.notify_all();
        }
        if let Some(handle) = self.scheduler_handle.take() {
            let _ = handle.join();
        }
    }

    /// Cumulative pass count since daemon start. Test helper.
    #[must_use]
    pub fn total_passes(&self) -> u64 {
        self.total_passes.load(Ordering::Relaxed)
    }

    /// Cumulative fail count since daemon start. Test helper.
    #[must_use]
    pub fn total_failures(&self) -> u64 {
        self.total_failures.load(Ordering::Relaxed)
    }

    /// Number of scheduler ticks that actually fired. Test helper.
    #[must_use]
    pub fn scheduled_run_count(&self) -> u64 {
        self.scheduled_run_count.load(Ordering::Relaxed)
    }
}

impl Drop for AuditVerifierShell {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[allow(clippy::too_many_arguments)]
fn scheduler_loop(
    schedule: &Schedule,
    wake: &Arc<SchedulerWake>,
    stop_flag: &Arc<AtomicBool>,
    shared: &Arc<SharedStatus>,
    total_passes: &Arc<AtomicU64>,
    total_failures: &Arc<AtomicU64>,
    last_run_ts: &Arc<AtomicI64>,
    scheduled_run_count: &Arc<AtomicU64>,
    last_verified_id: &Arc<Mutex<Option<i64>>>,
    checkpoint_path: Option<&Path>,
    runner: &dyn VerifierRunner,
    slo: &Slo,
) {
    loop {
        if stop_flag.load(Ordering::Relaxed) {
            return;
        }
        let next = match schedule.upcoming(Utc).next() {
            Some(when) => when,
            None => return,
        };
        let now = Utc::now();
        let wait = (next - now).to_std().unwrap_or(Duration::from_millis(0));
        if wait_until(wake, wait) {
            return;
        }
        if stop_flag.load(Ordering::Relaxed) {
            return;
        }
        let start_from = last_verified_id.lock().map(|g| *g).unwrap_or(None);
        let (outcome, latest_id) = runner.run(start_from);
        let now_ts = Utc::now().timestamp();
        last_run_ts.store(now_ts, Ordering::Relaxed);
        scheduled_run_count.fetch_add(1, Ordering::Relaxed);
        match &outcome {
            VerifierOutcome::Pass { chain_length } => {
                total_passes.fetch_add(1, Ordering::Relaxed);
                slo.observe_audit_verify(true);
                if let Some(id) = latest_id {
                    if let Ok(mut g) = last_verified_id.lock() {
                        *g = Some(id);
                    }
                    if let Some(p) = checkpoint_path {
                        let _ = save_checkpoint(
                            p,
                            &Checkpoint {
                                last_run_ts: now_ts,
                                last_verified_id: Some(id),
                            },
                        );
                    }
                }
                emit_pass_event(*chain_length);
            }
            VerifierOutcome::Fail {
                chain_length,
                detail,
            } => {
                total_failures.fetch_add(1, Ordering::Relaxed);
                slo.observe_audit_verify(false);
                emit_broken_event(*chain_length, detail);
            }
            VerifierOutcome::NeverRun => {}
        }
        if let Ok(mut g) = shared.outcome.lock() {
            g.0 = outcome;
        }
    }
}

fn wait_until(wake: &Arc<SchedulerWake>, wait: Duration) -> bool {
    let stopped = wake.stopped.lock().expect("audit verifier wake poisoned");
    if *stopped {
        return true;
    }
    let (guard, _timeout) = wake
        .cv
        .wait_timeout(stopped, wait)
        .expect("audit verifier wake poisoned");
    *guard
}

fn emit_pass_event(chain_length: u64) {
    log::info!(r#"{{"event":"audit.chain.verified","chain_length":{chain_length}}}"#);
}

fn emit_broken_event(chain_length: u64, detail: &str) {
    // JSON-escape the detail so operators can ingest this directly from
    // stderr without a second-pass parser. The field names `line`,
    // `expected_hmac`, `got_hmac` are the stable contract documented in
    // the I04 audit finding — even when the underlying error only
    // identifies a row id (the common case) we preserve the key shape.
    let escaped = escape_json_string(detail);
    log::error!(
        r#"{{"event":"audit.chain.broken","chain_length":{chain_length},"line":"{escaped}","expected_hmac":"","got_hmac":""}}"#
    );
}

fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn load_checkpoint(path: &Path) -> std::io::Result<Option<i64>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(path)?;
    let cp: Checkpoint = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(cp.last_verified_id)
}

fn save_checkpoint(path: &Path, cp: &Checkpoint) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(cp)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Hand-rolled runner used by unit tests to drive pass / fail paths
    /// deterministically without staging a real SQLite tamper.
    struct MockRunner {
        outcomes: StdMutex<Vec<(VerifierOutcome, Option<i64>)>>,
    }

    impl MockRunner {
        fn pass_then_fail() -> Self {
            Self {
                outcomes: StdMutex::new(vec![
                    (
                        VerifierOutcome::Fail {
                            chain_length: 3,
                            detail: "audit chain broken at id=4: entry_hash mismatch".to_owned(),
                        },
                        None,
                    ),
                    (VerifierOutcome::Pass { chain_length: 5 }, Some(5)),
                ]),
            }
        }
    }

    impl VerifierRunner for MockRunner {
        fn run(&self, _start_from: Option<i64>) -> (VerifierOutcome, Option<i64>) {
            let mut g = self.outcomes.lock().expect("mock runner poisoned");
            g.pop()
                .unwrap_or((VerifierOutcome::Pass { chain_length: 0 }, None))
        }
    }

    #[test]
    fn default_shell_from_enabled_config() {
        let cfg = AuditVerifierConfig::default();
        let shell = AuditVerifierShell::from_config(cfg).expect("ok");
        assert!(shell.is_enabled());
        let snap = shell.status_snapshot();
        assert!(snap.enabled);
        assert_eq!(snap.last_result, "never_run");
        assert_eq!(snap.chain_length, 0);
        assert_eq!(snap.last_run_ts, 0);
    }

    #[test]
    fn disabled_shell_reports_never_run() {
        let cfg = AuditVerifierConfig {
            enabled: false,
            ..AuditVerifierConfig::default()
        };
        let shell = AuditVerifierShell::from_config(cfg).expect("ok");
        let snap = shell.status_snapshot();
        assert!(!snap.enabled);
        assert_eq!(snap.last_result, "never_run");
    }

    #[test]
    fn invalid_cron_refuses_construction() {
        let cfg = AuditVerifierConfig {
            schedule_cron: "not a cron expression".to_owned(),
            ..AuditVerifierConfig::default()
        };
        let err = AuditVerifierShell::from_config(cfg).expect_err("invalid cron must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn run_once_pass_path_updates_slo_and_snapshot() {
        let cfg = AuditVerifierConfig::default();
        let shell = AuditVerifierShell::from_config(cfg).expect("ok");
        let runner = MockRunner {
            outcomes: StdMutex::new(vec![(VerifierOutcome::Pass { chain_length: 7 }, Some(7))]),
        };
        let slo = Slo::new();
        let outcome = shell.run_once(&runner, &slo);
        assert!(matches!(outcome, VerifierOutcome::Pass { chain_length: 7 }));
        let snap = shell.status_snapshot();
        assert_eq!(snap.last_result, "pass");
        assert_eq!(snap.chain_length, 7);
        assert_eq!(snap.total_passes, 1);
        assert_eq!(snap.total_failures, 0);
    }

    #[test]
    fn run_once_fail_path_updates_slo_and_snapshot() {
        let cfg = AuditVerifierConfig::default();
        let shell = AuditVerifierShell::from_config(cfg).expect("ok");
        let runner = MockRunner::pass_then_fail();
        let slo = Slo::new();
        // First drained entry is the pass (pop from end); second is the fail.
        let _ = shell.run_once(&runner, &slo);
        let second = shell.run_once(&runner, &slo);
        assert!(matches!(second, VerifierOutcome::Fail { .. }));
        let snap = shell.status_snapshot();
        assert_eq!(snap.last_result, "fail");
        assert!(
            snap.last_error.contains("audit chain broken"),
            "unexpected detail: {}",
            snap.last_error
        );
        assert_eq!(snap.total_passes, 1);
        assert_eq!(snap.total_failures, 1);
    }

    #[test]
    fn checkpoint_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cp.json");
        save_checkpoint(
            &path,
            &Checkpoint {
                last_run_ts: 123,
                last_verified_id: Some(42),
            },
        )
        .expect("save");
        let reloaded = load_checkpoint(&path).expect("load");
        assert_eq!(reloaded, Some(42));
    }

    #[test]
    fn checkpoint_missing_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("absent.json");
        let reloaded = load_checkpoint(&path).expect("load");
        assert!(reloaded.is_none());
    }

    #[test]
    fn escape_json_string_handles_control_chars() {
        let escaped = escape_json_string("line\nwith\t\"quotes\"\\back");
        assert_eq!(escaped, r#"line\nwith\t\"quotes\"\\back"#);
    }
}
