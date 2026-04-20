//! Metric families and a self-contained Prometheus text-format exporter.
//!
//! # Design notes
//!
//! - The scalar [`MetricsRegistry`] struct is always available (feature-free).
//!   The feature-gated `prometheus-exporter` adds the full metric family
//!   set, sanitized labels, and a text-format exporter.
//! - NO label carries user-identifiable or secret content. Labels are
//!   constrained to low-cardinality enums / sanitized method names.
//! - The exporter intentionally does not depend on the `prometheus` crate
//!   so that enabling metrics does not expand the daemon's dependency
//!   graph. The emitted text matches the documented 0.0.4 Prometheus
//!   exposition format consumed by standard scrapers.
//!
//! # Metric families (reference)
//!
//! | Name                              | Type      | Labels              | Cardinality bound |
//! |-----------------------------------|-----------|---------------------|-------------------|
//! | `pcloud_request_count`            | counter   | `method`, `status`  | O(methods × statuses), sanitised & length-capped |
//! | `pcloud_request_latency_seconds`  | histogram | `method`            | O(methods), buckets `DEFAULT_LATENCY_BUCKETS` |
//! | `pcloud_auth_attempts_total`      | counter   | `result`            | 4 (see `AuthResult`) |
//! | `pcloud_transfer_bytes_total`     | counter   | `direction`         | 2 (see `TransferDirection`) |
//! | `pcloud_crypto_lock_state`        | gauge     | —                   | 1 |
//! | `pcloud_sync_root_count`          | gauge     | —                   | 1 |
//! | `pcloud_ipc_connected_clients`    | gauge     | —                   | 1 |
//! | `pcloud_panic_count`              | counter   | —                   | 1 |
//!
//! ## Naming conventions
//!
//! All families follow Prometheus naming guidance:
//!
//! - snake_case,
//! - `pcloud_` prefix (single-namespace),
//! - `_total` suffix for monotonic counters that accumulate across restarts,
//! - `_seconds` unit suffix on durations,
//! - histogram `_bucket` / `_sum` / `_count` lines emitted together.
//!
//! ## Label sanitiser (post-P0.4, opaque-on-invalid-char)
//!
//! All dynamic label values pass through the internal sanitiser. Policy:
//!
//! 1. Allowed characters: ASCII alphanumeric plus `_ - . : /`.
//! 2. If ANY disallowed character is present, the ENTIRE value is replaced
//!    with the opaque token `"invalid"`. Partial preservation is explicitly
//!    refused; a naive char-by-char substitution could still leak a
//!    substring such as `evil` from an input like `ok" label="evil`.
//! 3. Empty inputs become `"_"`.
//! 4. Otherwise-clean inputs are length-capped at 64 chars to bound
//!    cardinality and prevent memory growth via attacker-chosen method
//!    names.
//!
//! This is the P0.4 hardening contract: mis-use returns an opaque token
//! instead of passing the bad character through, which makes
//! label-injection (quotes, newlines, backslashes) structurally impossible
//! in the emitted exposition.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Scalar-only metrics view that is available regardless of the
/// `prometheus-exporter` feature.
///
/// This is the minimum always-available surface: a couple of coarse
/// counters plus an enable/disable flag. The full metric family set
/// (counters, gauges, histograms with Prometheus rendering) lives in
/// `MetricFamilies` and requires the `prometheus-exporter` feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsRegistry {
    /// When `false` the daemon skips scrape rendering and exporter start-up.
    pub enabled: bool,
    /// Nominal scrape interval advertised to operators. The exporter itself
    /// is pull-driven and does not schedule renders.
    pub export_interval_secs: u64,
    /// Total number of audit events recorded through the shell since boot.
    pub emitted_events: u64,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self {
            enabled: true,
            export_interval_secs: 15,
            emitted_events: 0,
        }
    }
}

#[cfg(feature = "prometheus-exporter")]
pub use families::{
    AuthResult, CryptoLockState, DEFAULT_LATENCY_BUCKETS, HistogramHandle, MetricFamilies,
    TransferDirection, register_histogram,
};

#[cfg(feature = "prometheus-exporter")]
mod families {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    /// Histogram bucket upper bounds (in seconds) for
    /// `pcloud_request_latency_seconds`.
    ///
    /// These match the Prometheus client library defaults (11 buckets
    /// spanning 5 ms – 10 s). An implicit `+Inf` bucket is emitted on top
    /// of this set so total observations are always preserved.
    ///
    /// The bucket layout is load-bearing for the rendered exposition:
    /// cumulative `_bucket{le="..."}` counts are produced in the same order
    /// as this slice, and `_sum` / `_count` lines follow.
    pub const DEFAULT_LATENCY_BUCKETS: &[f64] = &[
        0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ];

    /// Direction label for the `pcloud_transfer_bytes_total` counter.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TransferDirection {
        /// Bytes the client has uploaded to pCloud.
        Upload,
        /// Bytes the client has downloaded from pCloud.
        Download,
    }

    impl TransferDirection {
        fn as_label(self) -> &'static str {
            match self {
                TransferDirection::Upload => "upload",
                TransferDirection::Download => "download",
            }
        }
    }

    /// Result label for the `pcloud_auth_attempts_total` counter.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum AuthResult {
        /// Authentication completed successfully.
        Success,
        /// Credentials were rejected or an API-level error was returned.
        Failure,
        /// Two-factor authentication is required to proceed.
        TwoFactorRequired,
        /// The API rate-limited the login attempt.
        RateLimited,
    }

    impl AuthResult {
        fn as_label(self) -> &'static str {
            match self {
                AuthResult::Success => "success",
                AuthResult::Failure => "failure",
                AuthResult::TwoFactorRequired => "tfa_required",
                AuthResult::RateLimited => "rate_limited",
            }
        }
    }

    /// Current state of the crypto subsystem, published as the
    /// `pcloud_crypto_lock_state` gauge.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CryptoLockState {
        /// Crypto has never been initialised for this account.
        Unsetup,
        /// Crypto is set up but the private key is not currently loaded.
        Locked,
        /// The private key is loaded and crypto folders are accessible.
        Unlocked,
    }

    impl CryptoLockState {
        /// Integer value rendered into the `pcloud_crypto_lock_state` gauge.
        ///
        /// The mapping is stable and documented alongside the gauge HELP
        /// text so operators can alert on `state == 0` (locked).
        pub fn as_value(self) -> i64 {
            match self {
                CryptoLockState::Unsetup => -1,
                CryptoLockState::Locked => 0,
                CryptoLockState::Unlocked => 1,
            }
        }
    }

    #[derive(Debug, Default, Clone)]
    struct Histogram {
        buckets: Vec<u64>,
        sum: f64,
        count: u64,
    }

    impl Histogram {
        fn new_buckets() -> Self {
            Self {
                buckets: vec![0; DEFAULT_LATENCY_BUCKETS.len()],
                sum: 0.0,
                count: 0,
            }
        }

        fn observe(&mut self, v: f64) {
            for (i, upper) in DEFAULT_LATENCY_BUCKETS.iter().enumerate() {
                if v <= *upper {
                    self.buckets[i] = self.buckets[i].saturating_add(1);
                }
            }
            self.sum += v;
            self.count = self.count.saturating_add(1);
        }
    }

    /// Atomic bucket-array histogram used by user-registered histograms.
    ///
    /// Bucket upper bounds are fixed at registration time and stored as a
    /// `Vec<f64>`. Each bucket is a monotonic `AtomicU64`; `observe`
    /// performs a branch-free (linear scan) bucket selection and updates
    /// `sum_bits` (f64-as-u64 bits, CAS loop) and `count` atomically.
    ///
    /// Handles are `Clone + Send + Sync`: registration returns an `Arc`-
    /// backed handle and a weak reference is retained by the global
    /// registry so [`MetricFamilies::render_prometheus`] can fold every
    /// live user histogram into the Prometheus exposition output without
    /// any additional wiring.
    pub struct HistogramHandle {
        inner: Arc<HistogramInner>,
    }

    impl Clone for HistogramHandle {
        fn clone(&self) -> Self {
            Self {
                inner: Arc::clone(&self.inner),
            }
        }
    }

    impl std::fmt::Debug for HistogramHandle {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("HistogramHandle")
                .field("name", &self.inner.name)
                .field("bucket_count", &self.inner.buckets_upper.len())
                .finish()
        }
    }

    struct HistogramInner {
        name: String,
        /// Upper bound of each bucket, in observation units (seconds).
        buckets_upper: Vec<f64>,
        /// Cumulative counts per bucket (aligned with `buckets_upper`).
        bucket_counts: Vec<AtomicU64>,
        /// Sum of observations, stored as `f64::to_bits` in a u64 slot and
        /// updated via a CAS loop to avoid a mutex on the hot path.
        sum_bits: AtomicU64,
        /// Total observations.
        count: AtomicU64,
    }

    impl HistogramHandle {
        /// Record a single observation. `value_seconds` may be any finite
        /// f64; NaN is treated as a miss (counted in `+Inf` only).
        pub fn observe(&self, value_seconds: f64) {
            // Linear scan over <=10 buckets: increment exactly the
            // smallest bucket whose upper bound is >= the observation. The
            // renderer produces cumulative `le` counts on the fly.
            for (i, upper) in self.inner.buckets_upper.iter().enumerate() {
                if value_seconds <= *upper {
                    self.inner.bucket_counts[i].fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }
            // CAS-update f64 sum via bit pattern.
            let mut old = self.inner.sum_bits.load(Ordering::Relaxed);
            loop {
                let new = (f64::from_bits(old) + value_seconds).to_bits();
                match self.inner.sum_bits.compare_exchange_weak(
                    old,
                    new,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(cur) => old = cur,
                }
            }
            self.inner.count.fetch_add(1, Ordering::Relaxed);
        }

        /// Histogram name (for diagnostics / tests).
        #[must_use]
        pub fn name(&self) -> &str {
            &self.inner.name
        }
    }

    fn user_histograms() -> &'static Mutex<Vec<Arc<HistogramInner>>> {
        static REG: OnceLock<Mutex<Vec<Arc<HistogramInner>>>> = OnceLock::new();
        REG.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Register a user-level histogram with the given name and bucket
    /// upper bounds (in seconds).
    ///
    /// If a histogram with the same name is already registered, the
    /// existing handle is returned (idempotent — safe to call from a
    /// `OnceLock`-guarded initialiser on every process, which is how the
    /// write-path emits `flush_latency_seconds`).
    ///
    /// The returned [`HistogramHandle`] is cheap to clone; it holds an
    /// `Arc` on the underlying atomic bucket array. The handle is also
    /// retained by the global registry so the Prometheus exposition body
    /// produced by [`MetricFamilies::render_prometheus`] includes every
    /// live user histogram without any further wiring.
    #[must_use]
    pub fn register_histogram(name: &str, buckets: &[f64]) -> HistogramHandle {
        let mut guard = crate::LockExt::lock_or_poisoned(
            user_histograms(),
            "metrics::MetricFamilies::register_histogram",
        );
        if let Some(existing) = guard.iter().find(|h| h.name == name) {
            return HistogramHandle {
                inner: Arc::clone(existing),
            };
        }
        let inner = Arc::new(HistogramInner {
            name: name.to_owned(),
            buckets_upper: buckets.to_vec(),
            bucket_counts: (0..buckets.len()).map(|_| AtomicU64::new(0)).collect(),
            sum_bits: AtomicU64::new(0.0_f64.to_bits()),
            count: AtomicU64::new(0),
        });
        guard.push(Arc::clone(&inner));
        HistogramHandle { inner }
    }

    fn render_user_histograms(out: &mut String) {
        let guard = match user_histograms().lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        for h in guard.iter() {
            let name = &h.name;
            out.push_str(&format!(
                "# HELP {name} User-registered histogram.\n# TYPE {name} histogram\n"
            ));
            let mut cum: u64 = 0;
            for (i, bound) in h.buckets_upper.iter().enumerate() {
                cum = cum.saturating_add(h.bucket_counts[i].load(Ordering::Relaxed));
                out.push_str(&format!("{name}_bucket{{le=\"{bound}\"}} {cum}\n"));
            }
            let total = h.count.load(Ordering::Relaxed);
            let sum = f64::from_bits(h.sum_bits.load(Ordering::Relaxed));
            out.push_str(&format!("{name}_bucket{{le=\"+Inf\"}} {total}\n"));
            out.push_str(&format!("{name}_sum {sum}\n"));
            out.push_str(&format!("{name}_count {total}\n"));
        }
    }

    /// Full metric family set rendered by the Prometheus exporter.
    ///
    /// Each field corresponds to a documented metric:
    /// - `request_count` — `pcloud_request_count` counter, labelled by
    ///   sanitised method and status.
    /// - `request_latency` — `pcloud_request_latency_seconds` histogram,
    ///   labelled by sanitised method.
    /// - `auth_attempts` — `pcloud_auth_attempts_total` counter, labelled by
    ///   [`AuthResult`].
    /// - `transfer_bytes` — `pcloud_transfer_bytes_total` counter, labelled
    ///   by [`TransferDirection`].
    /// - `crypto_lock_state` — `pcloud_crypto_lock_state` gauge.
    /// - `sync_root_count` — `pcloud_sync_root_count` gauge.
    /// - `ipc_connected_clients` — `pcloud_ipc_connected_clients` gauge.
    /// - `panic_count` — `pcloud_panic_count` counter.
    #[derive(Debug, Clone, Default)]
    pub struct MetricFamilies {
        /// Counter keyed by (sanitised method, sanitised status).
        pub request_count: BTreeMap<(String, String), u64>,
        request_latency: BTreeMap<String, Histogram>,
        /// Counter keyed by the static [`AuthResult`] label.
        pub auth_attempts: BTreeMap<&'static str, u64>,
        /// Counter keyed by the static [`TransferDirection`] label.
        pub transfer_bytes: BTreeMap<&'static str, u64>,
        /// Latest value of the `pcloud_crypto_lock_state` gauge.
        pub crypto_lock_state: i64,
        /// Latest value of the `pcloud_sync_root_count` gauge.
        pub sync_root_count: u64,
        /// Latest value of the `pcloud_ipc_connected_clients` gauge.
        pub ipc_connected_clients: i64,
        /// Cumulative process panic counter.
        pub panic_count: u64,
    }

    impl MetricFamilies {
        /// Record a completed IPC request: increments the request counter
        /// and pushes the latency observation into the per-method histogram.
        ///
        /// `method` and `status` are passed through the internal
        /// `sanitize_label` helper (see the module-level "Label sanitiser"
        /// section): ASCII alnum plus a small punctuation set, else the
        /// whole label becomes `"invalid"`.
        pub fn observe_request(&mut self, method: &str, status: &str, latency_seconds: f64) {
            let key = (sanitize_label(method), sanitize_label(status));
            let current = self.request_count.get(&key).copied().unwrap_or(0);
            self.request_count
                .insert(key.clone(), current.saturating_add(1));
            let h = self
                .request_latency
                .entry(key.0)
                .or_insert_with(Histogram::new_buckets);
            h.observe(latency_seconds);
        }

        /// Increment the `pcloud_auth_attempts_total` counter for the
        /// supplied [`AuthResult`].
        pub fn record_auth(&mut self, result: AuthResult) {
            let label = result.as_label();
            let current = self.auth_attempts.get(label).copied().unwrap_or(0);
            self.auth_attempts.insert(label, current.saturating_add(1));
        }

        /// Add `bytes` to the `pcloud_transfer_bytes_total` counter for the
        /// supplied [`TransferDirection`].
        pub fn add_transfer_bytes(&mut self, direction: TransferDirection, bytes: u64) {
            let label = direction.as_label();
            let current = self.transfer_bytes.get(label).copied().unwrap_or(0);
            self.transfer_bytes
                .insert(label, current.saturating_add(bytes));
        }

        /// Publish the latest crypto lock state to the gauge.
        pub fn set_crypto_lock_state(&mut self, state: CryptoLockState) {
            self.crypto_lock_state = state.as_value();
        }

        /// Publish the number of configured sync roots to the gauge.
        pub fn set_sync_root_count(&mut self, n: u64) {
            self.sync_root_count = n;
        }

        /// Publish the number of currently connected IPC peers to the gauge.
        pub fn set_connected_clients(&mut self, n: i64) {
            self.ipc_connected_clients = n;
        }

        /// Increment the process-wide panic counter.
        pub fn incr_panic(&mut self) {
            self.panic_count = self.panic_count.saturating_add(1);
        }

        /// Render every metric family to Prometheus 0.0.4 text exposition
        /// format.
        ///
        /// The output contains `# HELP` and `# TYPE` comments for every
        /// family and emits histograms with both per-bucket counts and the
        /// `+Inf` overflow line plus `_sum` / `_count`.
        pub fn render_prometheus(&self) -> String {
            let mut out = String::new();
            out.push_str(
                "# HELP pcloud_request_count Number of IPC requests by method and status.\n",
            );
            out.push_str("# TYPE pcloud_request_count counter\n");
            for ((method, status), v) in &self.request_count {
                out.push_str(&format!(
                    "pcloud_request_count{{method=\"{method}\",status=\"{status}\"}} {v}\n"
                ));
            }

            out.push_str("# HELP pcloud_request_latency_seconds Request latency histogram.\n");
            out.push_str("# TYPE pcloud_request_latency_seconds histogram\n");
            for (method, h) in &self.request_latency {
                let mut cum: u64 = 0;
                for (i, bound) in DEFAULT_LATENCY_BUCKETS.iter().enumerate() {
                    cum = cum.saturating_add(h.buckets[i]);
                    out.push_str(&format!(
                        "pcloud_request_latency_seconds_bucket{{method=\"{method}\",le=\"{bound}\"}} {cum}\n"
                    ));
                }
                out.push_str(&format!(
                    "pcloud_request_latency_seconds_bucket{{method=\"{method}\",le=\"+Inf\"}} {}\n",
                    h.count
                ));
                out.push_str(&format!(
                    "pcloud_request_latency_seconds_sum{{method=\"{method}\"}} {}\n",
                    h.sum
                ));
                out.push_str(&format!(
                    "pcloud_request_latency_seconds_count{{method=\"{method}\"}} {}\n",
                    h.count
                ));
            }

            out.push_str("# HELP pcloud_auth_attempts_total Auth attempts by result.\n");
            out.push_str("# TYPE pcloud_auth_attempts_total counter\n");
            for (label, v) in &self.auth_attempts {
                out.push_str(&format!(
                    "pcloud_auth_attempts_total{{result=\"{label}\"}} {v}\n"
                ));
            }

            out.push_str("# HELP pcloud_transfer_bytes_total Bytes transferred by direction.\n");
            out.push_str("# TYPE pcloud_transfer_bytes_total counter\n");
            for (label, v) in &self.transfer_bytes {
                out.push_str(&format!(
                    "pcloud_transfer_bytes_total{{direction=\"{label}\"}} {v}\n"
                ));
            }

            out.push_str("# HELP pcloud_crypto_lock_state Crypto lock gauge (-1=unsetup, 0=locked, 1=unlocked).\n");
            out.push_str("# TYPE pcloud_crypto_lock_state gauge\n");
            out.push_str(&format!(
                "pcloud_crypto_lock_state {}\n",
                self.crypto_lock_state
            ));

            out.push_str("# HELP pcloud_sync_root_count Configured sync roots.\n");
            out.push_str("# TYPE pcloud_sync_root_count gauge\n");
            out.push_str(&format!(
                "pcloud_sync_root_count {}\n",
                self.sync_root_count
            ));

            out.push_str("# HELP pcloud_ipc_connected_clients Currently connected IPC peers.\n");
            out.push_str("# TYPE pcloud_ipc_connected_clients gauge\n");
            out.push_str(&format!(
                "pcloud_ipc_connected_clients {}\n",
                self.ipc_connected_clients
            ));

            out.push_str("# HELP pcloud_panic_count Process panic counter.\n");
            out.push_str("# TYPE pcloud_panic_count counter\n");
            out.push_str(&format!("pcloud_panic_count {}\n", self.panic_count));

            render_user_histograms(&mut out);

            out
        }
    }

    /// Conservative label sanitiser implementing the post-P0.4 policy.
    ///
    /// # Contract
    ///
    /// - Allowed characters: ASCII alphanumeric plus `_ - . : /`.
    /// - If ANY disallowed character appears anywhere, the ENTIRE label is
    ///   replaced by the opaque token `"invalid"` (opaque-on-invalid-char).
    /// - An empty input becomes `"_"` so Prometheus never receives an
    ///   empty label value.
    /// - Clean inputs are length-capped at 64 chars to bound cardinality.
    ///
    /// # Why opaque-on-invalid rather than char-scrub
    ///
    /// A naive per-character scrub would leak substrings. Feeding
    /// `ok" label="evil` into a scrubber that replaces `"`, space, and `=`
    /// with `_` still exposes the literal `evil` in the output. Replacing
    /// the whole label blocks this split-token exfiltration and makes
    /// label-injection (quotes, newlines, backslashes) structurally
    /// impossible in the rendered exposition.
    ///
    /// # Cardinality
    ///
    /// Combined with the 64-char cap, bounded enum label sets
    /// ([`AuthResult`], [`TransferDirection`]), and the fact that the
    /// daemon only passes `&'static` IPC method names, the overall label
    /// cardinality is small and audit-friendly.
    fn sanitize_label(s: &str) -> String {
        let bad_char = s
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/')));
        if bad_char {
            return "invalid".to_owned();
        }
        if s.is_empty() {
            return "_".to_owned();
        }
        // Only length-truncate for otherwise-clean labels.
        let mut out = String::with_capacity(s.len().min(64));
        for (i, c) in s.chars().enumerate() {
            if i >= 64 {
                break;
            }
            out.push(c);
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn counter_increments_on_observe() {
            let mut m = MetricFamilies::default();
            m.observe_request("GetStatus", "ok", 0.003);
            m.observe_request("GetStatus", "ok", 0.015);
            assert_eq!(
                m.request_count
                    .get(&("GetStatus".to_owned(), "ok".to_owned()))
                    .copied(),
                Some(2)
            );
        }

        #[test]
        fn auth_and_transfer_gauges_render() {
            let mut m = MetricFamilies::default();
            m.record_auth(AuthResult::Success);
            m.record_auth(AuthResult::Failure);
            m.add_transfer_bytes(TransferDirection::Upload, 1024);
            m.add_transfer_bytes(TransferDirection::Download, 2048);
            m.set_crypto_lock_state(CryptoLockState::Unlocked);
            m.set_sync_root_count(3);
            m.set_connected_clients(2);
            m.incr_panic();
            let text = m.render_prometheus();
            assert!(text.contains("pcloud_auth_attempts_total{result=\"success\"} 1"));
            assert!(text.contains("pcloud_auth_attempts_total{result=\"failure\"} 1"));
            assert!(text.contains("pcloud_transfer_bytes_total{direction=\"upload\"} 1024"));
            assert!(text.contains("pcloud_transfer_bytes_total{direction=\"download\"} 2048"));
            assert!(text.contains("pcloud_crypto_lock_state 1"));
            assert!(text.contains("pcloud_sync_root_count 3"));
            assert!(text.contains("pcloud_ipc_connected_clients 2"));
            assert!(text.contains("pcloud_panic_count 1"));
        }

        #[test]
        fn label_sanitizer_rejects_non_alnum() {
            let mut m = MetricFamilies::default();
            m.observe_request("Get\"Status", "ok\nbad", 0.001);
            let text = m.render_prometheus();
            assert!(!text.contains("\"Status"));
            assert!(!text.contains("ok\nbad"));
        }

        #[test]
        fn histogram_observes_and_renders_prometheus() {
            let h = register_histogram(
                "test_obs_histogram_xyz_seconds",
                &[0.05, 0.1, 0.25, 0.5, 1.0],
            );
            h.observe(0.03);
            h.observe(0.2);
            h.observe(2.0);
            // Idempotent re-registration returns a handle sharing state.
            let h2 = register_histogram("test_obs_histogram_xyz_seconds", &[0.05]);
            assert_eq!(h2.name(), "test_obs_histogram_xyz_seconds");

            let m = MetricFamilies::default();
            let text = m.render_prometheus();
            assert!(
                text.contains("# TYPE test_obs_histogram_xyz_seconds histogram"),
                "missing TYPE line: {text}"
            );
            assert!(
                text.contains("test_obs_histogram_xyz_seconds_bucket{le=\"0.05\"} 1"),
                "bad bucket 0.05: {text}"
            );
            assert!(
                text.contains("test_obs_histogram_xyz_seconds_bucket{le=\"0.25\"} 2"),
                "bad bucket 0.25: {text}"
            );
            assert!(
                text.contains("test_obs_histogram_xyz_seconds_bucket{le=\"+Inf\"} 3"),
                "bad +Inf: {text}"
            );
            assert!(
                text.contains("test_obs_histogram_xyz_seconds_count 3"),
                "bad count: {text}"
            );
            assert!(
                text.contains("test_obs_histogram_xyz_seconds_sum "),
                "bad sum: {text}"
            );
        }

        #[test]
        fn long_label_is_truncated() {
            let mut m = MetricFamilies::default();
            let long = "x".repeat(256);
            m.observe_request(&long, "ok", 0.001);
            let text = m.render_prometheus();
            assert!(!text.contains(&long));
        }
    }
}
