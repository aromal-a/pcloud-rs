#![forbid(unsafe_code)]
//! # pcloud-observability
//!
//! Structured logging, audit event sink, health reporting, and metrics
//! for the Rust pcloud-rs path. Logging redacts fields marked sensitive;
//! audit persistence failures are surfaced, not silently swallowed.
//!
//! This crate intentionally avoids pulling in the `prometheus` or `tracing`
//! ecosystems at the root so the daemon's dependency graph stays small and
//! every public surface can be audited for secret leakage. The metric
//! families and HTTP scrape endpoint are gated behind the
//! `prometheus-exporter` cargo feature.
//!
//! See the module-level docs for:
//! - [`metrics`]   — counter/gauge/histogram families and label sanitizer
//! - [`slo`]       — Service-Level Objective registry and `/slo` JSON shape
//! - `exporter`  — std-only scrape listener with loopback-by-default policy
//! - [`logging`]   — redaction-aware log record formatter
//! - [`audit`]     — audit event envelope used by the runtime
//! - [`health`]    — build info and health report rendered by `Method::Health`

// **PLATFORM:** all
// **GATING:** none (portable).

#![deny(missing_docs)]
#![allow(clippy::pedantic)]

pub mod audit;
#[cfg(feature = "prometheus-exporter")]
pub mod exporter;
pub mod health;
pub mod lock_ext;
pub mod logging;
pub mod metrics;
pub mod slo;
#[cfg(feature = "tracing-otlp")]
pub mod tracing;

pub use lock_ext::{LockExt, RwLockExt};

/// Crate name constant, surfaced on health/build responses and log targets.
///
/// # Example
///
/// ```
/// assert_eq!(pcloud_observability::CRATE_NAME, "pcloud-observability");
/// ```
pub const CRATE_NAME: &str = "pcloud-observability";

/// Observability shell embedded inside the daemon runtime. Feature-gated
/// metric families live alongside the always-available scalar registry.
///
/// The shell is intentionally `!PartialEq` because mutable counters make
/// value equality meaningless. Callers should compare specific fields, not
/// the whole shell.
#[derive(Debug, Clone)]
pub struct ObservabilityShell {
    /// Scalar metric registry that is always available regardless of the
    /// `prometheus-exporter` feature. Holds aggregate counters used for
    /// smoke-level health reporting.
    pub metrics: metrics::MetricsRegistry,
    /// Full metric family set (counters, gauges, histograms) populated on
    /// hot paths. Only present when the `prometheus-exporter` feature is
    /// enabled.
    #[cfg(feature = "prometheus-exporter")]
    pub families: metrics::MetricFamilies,
    /// Canonical SLO registry. Always present regardless of feature
    /// flags; the IPC `Method::GetSlo` surface and the `/slo` HTTP
    /// endpoint both render from this instance. The registry uses
    /// atomic counters internally, so hot-path instrumentation can
    /// update it through the shared `Arc` without taking a lock.
    pub slo: std::sync::Arc<slo::Slo>,
    /// Live health snapshot — summary string and last-emitted event —
    /// returned on the daemon's health endpoint.
    pub live_health: health::HealthSnapshot,
    /// Startup audit event captured at process launch. Stored here so the
    /// runtime can publish it through the audit sink on the first tick.
    pub startup_event: audit::AuditEvent,
    /// Unix seconds at which the shell was constructed. Used for uptime
    /// reporting on the health endpoint.
    pub startup_unix_secs: u64,
}

impl Default for ObservabilityShell {
    fn default() -> Self {
        Self {
            metrics: metrics::MetricsRegistry::default(),
            #[cfg(feature = "prometheus-exporter")]
            families: metrics::MetricFamilies::default(),
            slo: std::sync::Arc::new(slo::Slo::new()),
            live_health: health::HealthSnapshot {
                summary: "initializing".to_owned(),
                last_event: None,
            },
            startup_event: audit::AuditEvent {
                category: "daemon.startup".to_owned(),
                details: None,
            },
            startup_unix_secs: health::now_unix_secs(),
        }
    }
}

impl ObservabilityShell {
    /// Record an audit event and update the live health snapshot.
    ///
    /// This increments the `emitted_events` counter on the scalar registry
    /// and replaces the health summary with the supplied category. The
    /// returned [`audit::AuditEvent`] should be published to the audit sink
    /// by the caller — this method never touches persistent storage.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_observability::ObservabilityShell;
    ///
    /// let mut shell = ObservabilityShell::default();
    /// let event = shell.record_event("auth.login.success", None);
    /// assert_eq!(event.category, "auth.login.success");
    /// assert_eq!(shell.live_health.last_event.as_deref(), Some("auth.login.success"));
    /// ```
    pub fn record_event(
        &mut self,
        category: impl Into<String>,
        details: Option<String>,
    ) -> audit::AuditEvent {
        let category = category.into();
        self.metrics.emitted_events = self.metrics.emitted_events.saturating_add(1);
        self.live_health.summary = format!("last_event={category}");
        self.live_health.last_event = Some(category.clone());
        audit::AuditEvent { category, details }
    }

    /// Build a short single-line diagnostic string summarising the state of
    /// the shell. Intended for log messages, not for external API consumers.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_observability::ObservabilityShell;
    /// let shell = ObservabilityShell::default();
    /// let s = shell.summary();
    /// assert!(s.starts_with("observability("));
    /// ```
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "observability(metrics_enabled={}, emitted_events={}, health={})",
            self.metrics.enabled, self.metrics.emitted_events, self.live_health.summary
        )
    }

    /// Build a health report for the `Method::Health` IPC. Includes build
    /// info, uptime, the latest event summary, and (when enabled) a text
    /// Prometheus snapshot.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_observability::ObservabilityShell;
    /// let shell = ObservabilityShell::default();
    /// let report = shell.health_report();
    /// assert_eq!(report.build.name, "pcloud-daemon");
    /// assert!(!report.build.version.is_empty());
    /// ```
    #[must_use]
    pub fn health_report(&self) -> health::HealthReport {
        health::HealthReport {
            build: health::BuildInfo::pcloud_daemon(),
            uptime_secs: health::uptime_from(self.startup_unix_secs),
            summary: self.live_health.summary.clone(),
            metrics_snapshot: {
                #[cfg(feature = "prometheus-exporter")]
                {
                    Some(self.families.render_prometheus())
                }
                #[cfg(not(feature = "prometheus-exporter"))]
                {
                    None
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_events_update_counters() {
        let mut shell = ObservabilityShell::default();
        let start = shell.metrics.emitted_events;
        shell.record_event("auth.login.success", None);
        shell.record_event("transfer.upload.complete", Some("100 bytes".to_owned()));
        assert_eq!(shell.metrics.emitted_events, start + 2);
        assert_eq!(
            shell.live_health.last_event.as_deref(),
            Some("transfer.upload.complete")
        );
    }

    #[test]
    fn health_report_contains_build_info() {
        let shell = ObservabilityShell::default();
        let r = shell.health_report();
        assert_eq!(r.build.name, "pcloud-daemon");
        assert!(!r.build.version.is_empty());
    }

    #[cfg(feature = "prometheus-exporter")]
    #[test]
    fn synthetic_events_update_metric_families() {
        use metrics::{AuthResult, CryptoLockState, TransferDirection};
        let mut shell = ObservabilityShell::default();
        shell.families.observe_request("GetHealth", "ok", 0.001);
        shell.families.record_auth(AuthResult::Success);
        shell.families.record_auth(AuthResult::Failure);
        shell
            .families
            .add_transfer_bytes(TransferDirection::Upload, 4096);
        shell
            .families
            .set_crypto_lock_state(CryptoLockState::Locked);
        shell.families.set_sync_root_count(2);
        shell.families.set_connected_clients(1);
        shell.families.incr_panic();

        let snap = shell.health_report().metrics_snapshot.unwrap();
        assert!(snap.contains("pcloud_request_count{method=\"GetHealth\",status=\"ok\"} 1"));
        assert!(snap.contains("pcloud_auth_attempts_total{result=\"success\"} 1"));
        assert!(snap.contains("pcloud_auth_attempts_total{result=\"failure\"} 1"));
        assert!(snap.contains("pcloud_transfer_bytes_total{direction=\"upload\"} 4096"));
        assert!(snap.contains("pcloud_crypto_lock_state 0"));
        assert!(snap.contains("pcloud_sync_root_count 2"));
        assert!(snap.contains("pcloud_ipc_connected_clients 1"));
        assert!(snap.contains("pcloud_panic_count 1"));
    }
}
