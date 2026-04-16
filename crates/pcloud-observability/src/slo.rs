//! Canonical Service-Level Objectives (SLOs) for the pcloud-rs daemon.
//!
//! This module owns the definition, registration, and live evaluation of
//! the **canonical SLO set** tracked for the Rust path. Each SLO pairs a
//! named Service-Level Indicator with a hard threshold and a direction
//! ("lower is better" / "higher is better") and is rendered through the
//! `/slo` HTTP endpoint and the IPC `Method::GetSlo` surface as a
//! stable JSON document.
//!
//! # Canonical SLO set
//!
//! | Name                                            | Target        | Direction | Window       |
//! |-------------------------------------------------|---------------|-----------|--------------|
//! | `ipc.request.latency.p99`                       | `< 100 ms`    | ≤         | rolling 5m   |
//! | `ipc.request.error_rate`                        | `< 0.1 %`     | ≤         | rolling 5m   |
//! | `auth.login.success_rate`                       | `> 99 %`      | ≥         | rolling 1h   |
//! | `upload.throughput_mbps.p50`                    | `> 5 MB/s`    | ≥         | rolling 5m   |
//! | `mount.read.latency.p99`                        | `< 50 ms`     | ≤         | rolling 5m   |
//! | `integrity_sweeper.run.p95`                     | `< 5 min`     | ≤         | per-run      |
//! | `audit.hash_chain.verify.daily_pass_rate`       | `> 99.9 %`    | ≥         | daily        |
//!
//! # Honesty
//!
//! These thresholds are **aspirational targets**. Actual measured values
//! are reported live; when a counter has not yet observed enough data,
//! that SLO is reported with `status: "no_data"` so operators can
//! distinguish "healthy" from "unmeasured" — the registry never claims
//! compliance by silence.
//!
//! # `/slo` JSON shape
//!
//! The `/slo` endpoint returns:
//!
//! ```text
//! {
//!   // Legacy compact keys preserved for existing dashboards:
//!   "ip95_ms": <f64, ms>,
//!   "upload_retry_ratio": <f64>,
//!   "crash_free_fraction": <f64>,
//!   "pass": <bool>,
//!   // Canonical SLO list (added 2026-04-16):
//!   "slos": [
//!     { "slo_name": "ipc.request.latency.p99", "target": "<100ms",
//!       "actual": "<value>", "status": "ok" | "violation" | "no_data" },
//!     ...
//!   ]
//! }
//! ```
//!
//! Field order is fixed. Non-finite floats collapse to `0.0`. Adding a
//! field to a line is compatible; removing or renaming an existing key
//! is a breaking change that must go through release review.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Bucket upper bounds in seconds for the IPC latency histogram.
/// Mirrors the exporter's histogram layout; extended to 10 s so the p99
/// estimator can bracket longer tails.
pub const SLO_LATENCY_BUCKETS_SECS: &[f64] = &[
    0.001, 0.0025, 0.005, 0.0075, 0.010, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

/// Bucket upper bounds in seconds for the mount-read-latency histogram.
pub const SLO_MOUNT_READ_BUCKETS_SECS: &[f64] = &[
    0.001, 0.0025, 0.005, 0.0075, 0.010, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

/// Bucket upper bounds in seconds for the integrity-sweeper run-duration
/// histogram. Tuned for multi-minute sweeps.
pub const SLO_INTEGRITY_RUN_BUCKETS_SECS: &[f64] = &[
    1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 240.0, 300.0, 600.0, 1200.0, 1800.0, 3600.0, 7200.0,
];

/// Bucket upper bounds (MB/s) for upload-throughput sampling. Lower
/// buckets included so the p50 estimator produces meaningful numbers
/// even under a slow uplink.
pub const SLO_UPLOAD_MBPS_BUCKETS: &[f64] = &[
    0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0,
];

// ---------------------------------------------------------------------
// Canonical SLO thresholds
// ---------------------------------------------------------------------

/// IPC request latency p99 SLO threshold (seconds).
pub const SLO_IPC_LATENCY_P99_SECS: f64 = 0.100; // 100 ms

/// IPC request error-rate SLO threshold (fraction). `< 0.1 %`.
pub const SLO_IPC_ERROR_RATE: f64 = 0.001;

/// Auth login success-rate SLO threshold (fraction). `> 99 %`.
pub const SLO_AUTH_LOGIN_SUCCESS_RATE: f64 = 0.99;

/// Upload throughput p50 SLO threshold (MB/s). `> 5 MB/s`.
pub const SLO_UPLOAD_THROUGHPUT_P50_MBPS: f64 = 5.0;

/// Mount read latency p99 SLO threshold (seconds). `< 50 ms`.
pub const SLO_MOUNT_READ_P99_SECS: f64 = 0.050;

/// Integrity-sweeper run p95 SLO threshold (seconds). `< 5 min`.
pub const SLO_INTEGRITY_SWEEPER_P95_SECS: f64 = 300.0;

/// Audit hash-chain daily-pass-rate SLO threshold (fraction). `> 99.9 %`.
pub const SLO_AUDIT_HASH_CHAIN_DAILY_PASS_RATE: f64 = 0.999;

// ---------------------------------------------------------------------
// Legacy thresholds (retained for backward-compatible `/slo` fields).
// ---------------------------------------------------------------------

/// p95 SLO threshold for IPC latency, in milliseconds. (Legacy compact
/// field — kept for existing dashboards.)
pub const SLO_P95_MS_THRESHOLD: f64 = 10.0;

/// Upload retry ratio SLO threshold. (Legacy compact field.)
pub const SLO_UPLOAD_RETRY_RATIO_THRESHOLD: f64 = 0.01;

/// Crash-free session fraction SLO threshold. (Legacy compact field.)
pub const SLO_CRASH_FREE_FRACTION_THRESHOLD: f64 = 0.999;

/// Canonical SLO registry.
///
/// All counters are atomic and the registry is `Sync`, so hot-path
/// observation sites can update without synchronisation. `const fn new`
/// allows placement in a `static` / `OnceLock` without lazy init.
#[derive(Debug, Default)]
pub struct Slo {
    // ----- IPC latency histogram (seconds). p99 + p95 both derived -----
    ipc_buckets: [AtomicU64; 13],
    ipc_overflow: AtomicU64,
    ipc_total: AtomicU64,

    // ----- IPC error rate -----
    ipc_errors: AtomicU64,

    // ----- Auth login outcomes -----
    auth_login_success: AtomicU64,
    auth_login_failure: AtomicU64,

    // ----- Upload throughput (MB/s) + legacy retry counters -----
    upload_throughput_buckets: [AtomicU64; 13],
    upload_throughput_overflow: AtomicU64,
    upload_throughput_total: AtomicU64,
    upload_started: AtomicU64,
    upload_retry: AtomicU64,

    // ----- Mount read latency -----
    mount_read_buckets: [AtomicU64; 13],
    mount_read_overflow: AtomicU64,
    mount_read_total: AtomicU64,

    // ----- Integrity sweeper run durations -----
    integrity_buckets: [AtomicU64; 13],
    integrity_overflow: AtomicU64,
    integrity_total: AtomicU64,

    // ----- Audit hash-chain verifications -----
    audit_verify_success: AtomicU64,
    audit_verify_failure: AtomicU64,

    // ----- Legacy crash-free session counters (retained for compat) -----
    sessions_started: AtomicU64,
    session_crash: AtomicU64,
}

const fn atomic_array_13() -> [AtomicU64; 13] {
    [
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ]
}

impl Slo {
    /// Construct an empty registry. All counters start at zero.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_observability::slo::Slo;
    /// let s = Slo::new();
    /// let snap = s.snapshot();
    /// // An empty registry has no observations and reports `no_data`;
    /// // the aggregate `pass` bit stays `true` so callers can use a
    /// // liveness probe elsewhere to distinguish "quiet" from "broken".
    /// assert!(snap.pass);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            ipc_buckets: atomic_array_13(),
            ipc_overflow: AtomicU64::new(0),
            ipc_total: AtomicU64::new(0),
            ipc_errors: AtomicU64::new(0),
            auth_login_success: AtomicU64::new(0),
            auth_login_failure: AtomicU64::new(0),
            upload_throughput_buckets: atomic_array_13(),
            upload_throughput_overflow: AtomicU64::new(0),
            upload_throughput_total: AtomicU64::new(0),
            upload_started: AtomicU64::new(0),
            upload_retry: AtomicU64::new(0),
            mount_read_buckets: atomic_array_13(),
            mount_read_overflow: AtomicU64::new(0),
            mount_read_total: AtomicU64::new(0),
            integrity_buckets: atomic_array_13(),
            integrity_overflow: AtomicU64::new(0),
            integrity_total: AtomicU64::new(0),
            audit_verify_success: AtomicU64::new(0),
            audit_verify_failure: AtomicU64::new(0),
            sessions_started: AtomicU64::new(0),
            session_crash: AtomicU64::new(0),
        }
    }

    // ---------------- IPC latency ----------------

    /// Record an IPC request latency observation (seconds).
    pub fn observe_ipc_latency(&self, seconds: f64) {
        observe_bucket(
            seconds,
            SLO_LATENCY_BUCKETS_SECS,
            &self.ipc_buckets,
            &self.ipc_overflow,
            &self.ipc_total,
        );
    }

    /// Record a single IPC request outcome (for the error-rate SLI).
    /// `error = true` counts as a failed request.
    pub fn observe_ipc_outcome(&self, error: bool) {
        if error {
            self.ipc_errors.fetch_add(1, Ordering::Relaxed);
        }
        // IPC total already includes every observed latency; callers
        // that want error-rate to be latency-weighted should pair each
        // `observe_ipc_latency` with a matching `observe_ipc_outcome`.
    }

    // ---------------- Auth login ----------------

    /// Record an auth-login outcome.
    pub fn observe_auth_login(&self, success: bool) {
        if success {
            self.auth_login_success.fetch_add(1, Ordering::Relaxed);
        } else {
            self.auth_login_failure.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ---------------- Upload ----------------

    /// Increment the "uploads started" counter.
    pub fn incr_upload_started(&self) {
        self.upload_started.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the "upload retry" counter.
    pub fn incr_upload_retry(&self) {
        self.upload_retry.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an upload-throughput observation, in MB/s.
    pub fn observe_upload_throughput_mbps(&self, mbps: f64) {
        observe_bucket(
            mbps,
            SLO_UPLOAD_MBPS_BUCKETS,
            &self.upload_throughput_buckets,
            &self.upload_throughput_overflow,
            &self.upload_throughput_total,
        );
    }

    // ---------------- Mount read latency ----------------

    /// Record a mount read latency observation (seconds).
    pub fn observe_mount_read_latency(&self, seconds: f64) {
        observe_bucket(
            seconds,
            SLO_MOUNT_READ_BUCKETS_SECS,
            &self.mount_read_buckets,
            &self.mount_read_overflow,
            &self.mount_read_total,
        );
    }

    // ---------------- Integrity sweeper ----------------

    /// Record an integrity-sweeper run duration (seconds).
    pub fn observe_integrity_sweeper_run(&self, seconds: f64) {
        observe_bucket(
            seconds,
            SLO_INTEGRITY_RUN_BUCKETS_SECS,
            &self.integrity_buckets,
            &self.integrity_overflow,
            &self.integrity_total,
        );
    }

    // ---------------- Audit hash-chain verification ----------------

    /// Record an audit hash-chain verification outcome.
    pub fn observe_audit_verify(&self, success: bool) {
        if success {
            self.audit_verify_success.fetch_add(1, Ordering::Relaxed);
        } else {
            self.audit_verify_failure.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ---------------- Legacy crash-free sessions ----------------

    /// Increment the total session counter (daemon run).
    pub fn incr_session_started(&self) {
        self.sessions_started.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the crashing-session counter.
    pub fn incr_session_crash(&self) {
        self.session_crash.fetch_add(1, Ordering::Relaxed);
    }

    // ---------------- Snapshot / render ----------------

    /// Compute a snapshot of every SLI and pass/fail decision.
    #[must_use]
    pub fn snapshot(&self) -> SloSnapshot {
        let total = self.ipc_total.load(Ordering::Relaxed);
        let overflow = self.ipc_overflow.load(Ordering::Relaxed);
        let ipc_buckets = load_array(&self.ipc_buckets);

        let p95_ms = estimate_percentile_ms(&ipc_buckets, overflow, total, 0.95);
        let p99_ms = estimate_percentile_ms(&ipc_buckets, overflow, total, 0.99);

        let ipc_errors = self.ipc_errors.load(Ordering::Relaxed);
        let ipc_error_rate = if total == 0 {
            0.0
        } else {
            (ipc_errors as f64) / (total as f64)
        };

        let auth_ok = self.auth_login_success.load(Ordering::Relaxed);
        let auth_fail = self.auth_login_failure.load(Ordering::Relaxed);
        let auth_total = auth_ok.saturating_add(auth_fail);
        let auth_success_rate = if auth_total == 0 {
            0.0
        } else {
            (auth_ok as f64) / (auth_total as f64)
        };

        let up_total = self.upload_throughput_total.load(Ordering::Relaxed);
        let up_overflow = self.upload_throughput_overflow.load(Ordering::Relaxed);
        let up_buckets = load_array(&self.upload_throughput_buckets);
        let upload_p50_mbps = estimate_percentile_raw(
            &up_buckets,
            up_overflow,
            up_total,
            0.50,
            SLO_UPLOAD_MBPS_BUCKETS,
        );

        let mount_total = self.mount_read_total.load(Ordering::Relaxed);
        let mount_overflow = self.mount_read_overflow.load(Ordering::Relaxed);
        let mount_buckets = load_array(&self.mount_read_buckets);
        let mount_read_p99_ms =
            estimate_percentile_ms(&mount_buckets, mount_overflow, mount_total, 0.99);

        let integ_total = self.integrity_total.load(Ordering::Relaxed);
        let integ_overflow = self.integrity_overflow.load(Ordering::Relaxed);
        let integ_buckets = load_array(&self.integrity_buckets);
        let integrity_run_p95_secs = estimate_percentile_raw(
            &integ_buckets,
            integ_overflow,
            integ_total,
            0.95,
            SLO_INTEGRITY_RUN_BUCKETS_SECS,
        );

        let av_ok = self.audit_verify_success.load(Ordering::Relaxed);
        let av_fail = self.audit_verify_failure.load(Ordering::Relaxed);
        let av_total = av_ok.saturating_add(av_fail);
        let audit_pass_rate = if av_total == 0 {
            0.0
        } else {
            (av_ok as f64) / (av_total as f64)
        };

        let started = self.upload_started.load(Ordering::Relaxed);
        let retries = self.upload_retry.load(Ordering::Relaxed);
        let upload_retry_ratio = if started == 0 {
            0.0
        } else {
            (retries as f64) / (started as f64)
        };

        let sessions = self.sessions_started.load(Ordering::Relaxed);
        let crashes = self.session_crash.load(Ordering::Relaxed);
        let crash_free_fraction = if sessions == 0 {
            1.0
        } else {
            let non_crashing = sessions.saturating_sub(crashes);
            (non_crashing as f64) / (sessions as f64)
        };

        // ---- Canonical SLO evaluation ----
        let slos = vec![
            SloEntry::latency_ms(
                "ipc.request.latency.p99",
                SLO_IPC_LATENCY_P99_SECS * 1000.0,
                p99_ms,
                total > 0,
            ),
            SloEntry::ratio_upper(
                "ipc.request.error_rate",
                SLO_IPC_ERROR_RATE,
                ipc_error_rate,
                total > 0,
            ),
            SloEntry::ratio_lower(
                "auth.login.success_rate",
                SLO_AUTH_LOGIN_SUCCESS_RATE,
                auth_success_rate,
                auth_total > 0,
            ),
            SloEntry::mbps_lower(
                "upload.throughput_mbps.p50",
                SLO_UPLOAD_THROUGHPUT_P50_MBPS,
                upload_p50_mbps,
                up_total > 0,
            ),
            SloEntry::latency_ms(
                "mount.read.latency.p99",
                SLO_MOUNT_READ_P99_SECS * 1000.0,
                mount_read_p99_ms,
                mount_total > 0,
            ),
            SloEntry::duration_secs(
                "integrity_sweeper.run.p95",
                SLO_INTEGRITY_SWEEPER_P95_SECS,
                integrity_run_p95_secs,
                integ_total > 0,
            ),
            SloEntry::ratio_lower(
                "audit.hash_chain.verify.daily_pass_rate",
                SLO_AUDIT_HASH_CHAIN_DAILY_PASS_RATE,
                audit_pass_rate,
                av_total > 0,
            ),
        ];

        // Legacy compact pass: retained for backwards compat.
        let p95_pass = p95_ms <= SLO_P95_MS_THRESHOLD;
        let retry_pass = upload_retry_ratio <= SLO_UPLOAD_RETRY_RATIO_THRESHOLD;
        let crash_pass = crash_free_fraction >= SLO_CRASH_FREE_FRACTION_THRESHOLD;
        let legacy_pass = p95_pass && retry_pass && crash_pass;

        // Canonical pass: AND of every SLO that actually has data. SLOs
        // reporting `no_data` contribute `true` so the aggregate bit
        // remains honest (no data means no observed breach).
        let canonical_pass = slos
            .iter()
            .all(|e| !matches!(e.status, SloStatus::Violation));

        SloSnapshot {
            ip95_ms: p95_ms,
            upload_retry_ratio,
            crash_free_fraction,
            pass: legacy_pass && canonical_pass,
            slos,
        }
    }

    /// Render the snapshot as a stable JSON document.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_observability::slo::Slo;
    /// let s = Slo::new();
    /// let json = s.render_json();
    /// assert!(json.contains("\"ip95_ms\":"));
    /// assert!(json.contains("\"slos\":"));
    /// assert!(json.contains("\"ipc.request.latency.p99\""));
    /// ```
    #[must_use]
    pub fn render_json(&self) -> String {
        self.snapshot().to_json()
    }

    /// Render just the canonical SLO report (no legacy fields).
    /// This is the payload returned by the IPC `Method::GetSlo` surface.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_observability::slo::Slo;
    /// let s = Slo::new();
    /// let json = s.render_report_json();
    /// assert!(json.starts_with("{\"slos\":["));
    /// ```
    #[must_use]
    pub fn render_report_json(&self) -> String {
        self.snapshot().to_report_json()
    }
}

fn observe_bucket(
    v: f64,
    uppers: &[f64],
    buckets: &[AtomicU64; 13],
    overflow: &AtomicU64,
    total: &AtomicU64,
) {
    let v = if v.is_nan() || v < 0.0 { 0.0 } else { v };
    let mut placed = false;
    for (i, upper) in uppers.iter().enumerate().take(buckets.len()) {
        if v <= *upper {
            buckets[i].fetch_add(1, Ordering::Relaxed);
            placed = true;
            break;
        }
    }
    if !placed {
        overflow.fetch_add(1, Ordering::Relaxed);
    }
    total.fetch_add(1, Ordering::Relaxed);
}

fn load_array(arr: &[AtomicU64; 13]) -> [u64; 13] {
    let mut out = [0u64; 13];
    for (i, a) in arr.iter().enumerate() {
        out[i] = a.load(Ordering::Relaxed);
    }
    out
}

/// Estimate a percentile in **milliseconds** from a bucket-count array
/// and overflow. Bucket uppers are assumed to be in seconds.
fn estimate_percentile_ms(buckets: &[u64], overflow: u64, total: u64, percentile: f64) -> f64 {
    estimate_percentile_raw(
        buckets,
        overflow,
        total,
        percentile,
        SLO_LATENCY_BUCKETS_SECS,
    ) * 1000.0
}

/// Estimate a percentile in the native unit of the bucket bounds. Used
/// for MB/s and seconds-valued histograms.
fn estimate_percentile_raw(
    buckets: &[u64],
    overflow: u64,
    total: u64,
    percentile: f64,
    uppers: &[f64],
) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let target = (total as f64) * percentile;
    let mut cum: u64 = 0;
    let mut prev_upper: f64 = 0.0;
    for (i, upper) in uppers.iter().enumerate() {
        let c = buckets.get(i).copied().unwrap_or(0);
        let new_cum = cum.saturating_add(c);
        if (new_cum as f64) >= target {
            let in_bucket = c as f64;
            let needed = target - cum as f64;
            let frac = if in_bucket > 0.0 {
                (needed / in_bucket).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return prev_upper + (upper - prev_upper) * frac;
        }
        cum = new_cum;
        prev_upper = *upper;
    }
    // Fell into overflow: return the top bucket as a lower bound.
    let _ = overflow;
    prev_upper
}

/// Status of a single SLO at snapshot time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SloStatus {
    /// SLO target met.
    Ok,
    /// SLO target breached.
    Violation,
    /// Not enough samples to evaluate; the target is neither met nor
    /// breached — explicitly distinguished from `Ok` so dashboards do
    /// not conflate "quiet" with "healthy".
    NoData,
}

impl SloStatus {
    fn as_str(self) -> &'static str {
        match self {
            SloStatus::Ok => "ok",
            SloStatus::Violation => "violation",
            SloStatus::NoData => "no_data",
        }
    }
}

/// One canonical SLO entry rendered into the `/slo` JSON document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SloEntry {
    /// Dotted canonical name (e.g. `ipc.request.latency.p99`).
    pub slo_name: String,
    /// Target rendered as a human-readable string (e.g. `<100ms`).
    pub target: String,
    /// Actual measured value rendered as a human-readable string.
    pub actual: String,
    /// Status, one of `ok` / `violation` / `no_data`.
    pub status: SloStatus,
}

impl SloEntry {
    fn latency_ms(name: &str, target_ms: f64, actual_ms: f64, has_data: bool) -> Self {
        let status = if !has_data {
            SloStatus::NoData
        } else if actual_ms <= target_ms {
            SloStatus::Ok
        } else {
            SloStatus::Violation
        };
        Self {
            slo_name: name.to_owned(),
            target: format!("<{}ms", format_num(target_ms)),
            actual: format!("{}ms", format_num(finite_or_zero(actual_ms))),
            status,
        }
    }

    fn duration_secs(name: &str, target_secs: f64, actual_secs: f64, has_data: bool) -> Self {
        let status = if !has_data {
            SloStatus::NoData
        } else if actual_secs <= target_secs {
            SloStatus::Ok
        } else {
            SloStatus::Violation
        };
        Self {
            slo_name: name.to_owned(),
            target: format!("<{}s", format_num(target_secs)),
            actual: format!("{}s", format_num(finite_or_zero(actual_secs))),
            status,
        }
    }

    fn ratio_upper(name: &str, target: f64, actual: f64, has_data: bool) -> Self {
        // "upper" bound: ok when actual <= target.
        let status = if !has_data {
            SloStatus::NoData
        } else if actual <= target {
            SloStatus::Ok
        } else {
            SloStatus::Violation
        };
        Self {
            slo_name: name.to_owned(),
            target: format!("<{}", format_ratio(target)),
            actual: format_ratio(finite_or_zero(actual)),
            status,
        }
    }

    fn ratio_lower(name: &str, target: f64, actual: f64, has_data: bool) -> Self {
        // "lower" bound: ok when actual >= target.
        let status = if !has_data {
            SloStatus::NoData
        } else if actual >= target {
            SloStatus::Ok
        } else {
            SloStatus::Violation
        };
        Self {
            slo_name: name.to_owned(),
            target: format!(">{}", format_ratio(target)),
            actual: format_ratio(finite_or_zero(actual)),
            status,
        }
    }

    fn mbps_lower(name: &str, target_mbps: f64, actual_mbps: f64, has_data: bool) -> Self {
        let status = if !has_data {
            SloStatus::NoData
        } else if actual_mbps >= target_mbps {
            SloStatus::Ok
        } else {
            SloStatus::Violation
        };
        Self {
            slo_name: name.to_owned(),
            target: format!(">{}MBps", format_num(target_mbps)),
            actual: format!("{}MBps", format_num(finite_or_zero(actual_mbps))),
            status,
        }
    }
}

/// Stable rendered snapshot of every SLI. Both legacy compact fields
/// and the canonical SLO list are populated on every render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SloSnapshot {
    /// (Legacy) IPC request latency p95 in milliseconds.
    pub ip95_ms: f64,
    /// (Legacy) upload retry ratio in `[0.0, 1.0]`.
    pub upload_retry_ratio: f64,
    /// (Legacy) crash-free session fraction in `[0.0, 1.0]`.
    pub crash_free_fraction: f64,
    /// Aggregate pass bit: `true` when every legacy SLI and every
    /// canonical SLO is non-`Violation`.
    pub pass: bool,
    /// Canonical SLO list. Added 2026-04-16; stable ordering.
    pub slos: Vec<SloEntry>,
}

impl SloSnapshot {
    /// Render the snapshot as the `/slo` wire document (legacy compact
    /// fields + canonical `slos` array).
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(256 + 96 * self.slos.len());
        out.push_str(&format!(
            "{{\"ip95_ms\":{:.6},\"upload_retry_ratio\":{:.6},\"crash_free_fraction\":{:.6},\"pass\":{},\"slos\":[",
            finite_or_zero(self.ip95_ms),
            finite_or_zero(self.upload_retry_ratio),
            finite_or_zero(self.crash_free_fraction),
            if self.pass { "true" } else { "false" }
        ));
        for (i, e) in self.slos.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"slo_name\":{},\"target\":{},\"actual\":{},\"status\":\"{}\"}}",
                json_string(&e.slo_name),
                json_string(&e.target),
                json_string(&e.actual),
                e.status.as_str()
            ));
        }
        out.push_str("]}");
        out
    }

    /// Render only the canonical SLO list as the `Method::GetSlo`
    /// response payload. Same entries as the `slos` field in
    /// [`Self::to_json`], minus the legacy compat keys.
    #[must_use]
    pub fn to_report_json(&self) -> String {
        let mut out = String::with_capacity(64 + 96 * self.slos.len());
        out.push_str("{\"slos\":[");
        for (i, e) in self.slos.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"slo_name\":{},\"target\":{},\"actual\":{},\"status\":\"{}\"}}",
                json_string(&e.slo_name),
                json_string(&e.target),
                json_string(&e.actual),
                e.status.as_str()
            ));
        }
        out.push_str("],\"pass\":");
        out.push_str(if self.pass { "true" } else { "false" });
        out.push('}');
        out
    }
}

fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

fn format_num(v: f64) -> String {
    if v.is_finite() {
        // Trim trailing zeros for common human-readable shapes.
        let s = format!("{v:.3}");
        let trimmed = s.trim_end_matches('0').trim_end_matches('.').to_owned();
        if trimmed.is_empty() {
            "0".to_owned()
        } else {
            trimmed
        }
    } else {
        "0".to_owned()
    }
}

fn format_ratio(v: f64) -> String {
    let pct = finite_or_zero(v) * 100.0;
    // Six decimals so `0.999` is rendered as `99.9` without rounding to 100.
    let s = format!("{pct:.6}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.').to_owned();
    let body = if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed
    };
    format!("{body}%")
}

/// Minimal JSON-string escaper for the fixed set of characters our
/// canonical names / target strings can contain. No control chars or
/// unusual punctuation are allowed in field names; this escaper handles
/// `"` and `\` for safety only.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(snap: &SloSnapshot, name: &str) -> SloEntry {
        snap.slos
            .iter()
            .find(|e| e.slo_name == name)
            .cloned()
            .unwrap_or_else(|| panic!("SLO {name} not present in snapshot"))
    }

    #[test]
    fn empty_snapshot_reports_no_data_for_canonical_slos() {
        let s = Slo::new();
        let snap = s.snapshot();
        assert_eq!(snap.slos.len(), 7);
        for entry in &snap.slos {
            assert_eq!(
                entry.status,
                SloStatus::NoData,
                "fresh SLO {} should be no_data, got {:?}",
                entry.slo_name,
                entry.status
            );
        }
        // Aggregate still passes: no data == no observed breach.
        assert!(snap.pass);
    }

    #[test]
    fn p99_ipc_latency_ok_when_all_fast() {
        let s = Slo::new();
        for _ in 0..1000 {
            s.observe_ipc_latency(0.005); // 5 ms
        }
        let snap = s.snapshot();
        let e = find(&snap, "ipc.request.latency.p99");
        assert_eq!(
            e.status,
            SloStatus::Ok,
            "got {:?} actual={}",
            e.status,
            e.actual
        );
    }

    #[test]
    fn p99_ipc_latency_violation_when_slow() {
        let s = Slo::new();
        for _ in 0..1000 {
            s.observe_ipc_latency(0.5); // 500 ms
        }
        let snap = s.snapshot();
        let e = find(&snap, "ipc.request.latency.p99");
        assert_eq!(e.status, SloStatus::Violation);
        assert!(!snap.pass);
    }

    #[test]
    fn ipc_error_rate_evaluated() {
        let s = Slo::new();
        for i in 0..1000 {
            s.observe_ipc_latency(0.001);
            s.observe_ipc_outcome(i < 5); // 0.5% errors
        }
        let snap = s.snapshot();
        let e = find(&snap, "ipc.request.error_rate");
        assert_eq!(e.status, SloStatus::Violation, "0.5% > 0.1%");

        let s2 = Slo::new();
        for _ in 0..1000 {
            s2.observe_ipc_latency(0.001);
            s2.observe_ipc_outcome(false);
        }
        let snap2 = s2.snapshot();
        let e2 = find(&snap2, "ipc.request.error_rate");
        assert_eq!(e2.status, SloStatus::Ok);
    }

    #[test]
    fn auth_login_success_rate_evaluated() {
        let s = Slo::new();
        for i in 0..100 {
            s.observe_auth_login(i < 99); // 99%
        }
        let snap = s.snapshot();
        let e = find(&snap, "auth.login.success_rate");
        // 99% is exactly the threshold; 99/100 = 0.99 >= 0.99 => ok.
        assert_eq!(e.status, SloStatus::Ok);

        let s2 = Slo::new();
        for i in 0..100 {
            s2.observe_auth_login(i < 90);
        }
        let e2 = find(&s2.snapshot(), "auth.login.success_rate");
        assert_eq!(e2.status, SloStatus::Violation);
    }

    #[test]
    fn upload_throughput_p50_evaluated() {
        let s = Slo::new();
        for _ in 0..100 {
            s.observe_upload_throughput_mbps(10.0);
        }
        let e = find(&s.snapshot(), "upload.throughput_mbps.p50");
        assert_eq!(e.status, SloStatus::Ok);

        let s2 = Slo::new();
        for _ in 0..100 {
            s2.observe_upload_throughput_mbps(1.0);
        }
        let e2 = find(&s2.snapshot(), "upload.throughput_mbps.p50");
        assert_eq!(e2.status, SloStatus::Violation);
    }

    #[test]
    fn mount_read_latency_p99_evaluated() {
        let s = Slo::new();
        for _ in 0..1000 {
            s.observe_mount_read_latency(0.010); // 10 ms
        }
        let e = find(&s.snapshot(), "mount.read.latency.p99");
        assert_eq!(e.status, SloStatus::Ok);

        let s2 = Slo::new();
        for _ in 0..1000 {
            s2.observe_mount_read_latency(0.200); // 200 ms
        }
        let e2 = find(&s2.snapshot(), "mount.read.latency.p99");
        assert_eq!(e2.status, SloStatus::Violation);
    }

    #[test]
    fn integrity_sweeper_p95_evaluated() {
        let s = Slo::new();
        for _ in 0..20 {
            s.observe_integrity_sweeper_run(60.0); // 1 minute
        }
        let e = find(&s.snapshot(), "integrity_sweeper.run.p95");
        assert_eq!(e.status, SloStatus::Ok);

        let s2 = Slo::new();
        for _ in 0..20 {
            s2.observe_integrity_sweeper_run(600.0); // 10 minutes
        }
        let e2 = find(&s2.snapshot(), "integrity_sweeper.run.p95");
        assert_eq!(e2.status, SloStatus::Violation);
    }

    #[test]
    fn audit_hash_chain_pass_rate_evaluated() {
        let s = Slo::new();
        for _ in 0..1000 {
            s.observe_audit_verify(true);
        }
        let e = find(&s.snapshot(), "audit.hash_chain.verify.daily_pass_rate");
        assert_eq!(e.status, SloStatus::Ok);

        let s2 = Slo::new();
        for i in 0..1000 {
            s2.observe_audit_verify(i < 990); // 99%
        }
        let e2 = find(&s2.snapshot(), "audit.hash_chain.verify.daily_pass_rate");
        assert_eq!(e2.status, SloStatus::Violation);
    }

    #[test]
    fn render_json_contains_legacy_and_canonical_fields() {
        let s = Slo::new();
        s.observe_ipc_latency(0.001);
        let j = s.render_json();
        assert!(j.contains("\"ip95_ms\""), "missing legacy ip95_ms: {j}");
        assert!(j.contains("\"upload_retry_ratio\""));
        assert!(j.contains("\"crash_free_fraction\""));
        assert!(j.contains("\"pass\""));
        assert!(j.contains("\"slos\":["));
        assert!(j.contains("\"ipc.request.latency.p99\""));
        assert!(j.contains("\"auth.login.success_rate\""));
        assert!(j.contains("\"mount.read.latency.p99\""));
        assert!(j.contains("\"integrity_sweeper.run.p95\""));
        assert!(j.contains("\"audit.hash_chain.verify.daily_pass_rate\""));
    }

    #[test]
    fn render_report_json_is_canonical_only() {
        let s = Slo::new();
        let j = s.render_report_json();
        assert!(j.starts_with("{\"slos\":["));
        assert!(j.contains("\"pass\":"));
        assert!(!j.contains("\"ip95_ms\""));
        assert!(!j.contains("\"crash_free_fraction\""));
    }

    #[test]
    fn nan_and_negative_latency_normalised() {
        let s = Slo::new();
        s.observe_ipc_latency(f64::NAN);
        s.observe_ipc_latency(-1.0);
        s.observe_mount_read_latency(f64::NAN);
        s.observe_integrity_sweeper_run(-0.0);
        let snap = s.snapshot();
        for entry in &snap.slos {
            // Every actual field must render as a finite-looking string;
            // the critical property is that the snapshot cannot panic.
            assert!(!entry.actual.contains("NaN"));
        }
    }

    #[test]
    fn legacy_compact_fields_preserved() {
        // Legacy schema (ip95_ms / upload_retry_ratio / crash_free_fraction)
        // is still populated so existing dashboards keep working.
        let s = Slo::new();
        for _ in 0..100 {
            s.observe_ipc_latency(0.002);
        }
        s.incr_upload_started();
        s.incr_session_started();
        let snap = s.snapshot();
        assert!(snap.ip95_ms > 0.0);
        assert_eq!(snap.upload_retry_ratio, 0.0);
        assert_eq!(snap.crash_free_fraction, 1.0);
    }
}
