//! Feature-gated glue between the runtime shell and the
//! [`pcloud_observability::exporter`] HTTP scrape listener.
//!
//! The listener runs on its own thread and cannot borrow the non-Sync
//! [`RuntimeShell`], so this module exposes a small shared snapshot
//! ([`MetricsBridge`]) that the serve loop refreshes whenever state
//! changes. `GET /metrics` and `GET /health` read from this snapshot.
//!
//! Security posture:
//! - bind defaults to `127.0.0.1`,
//! - wildcard bind requires **both** `PCLOUD_METRICS_BIND_ALL=1` and the
//!   daemon running under `Environment::Development`,
//! - no runtime state crosses the wall except the already-sanitized
//!   Prometheus text and a boolean liveness bit.

#![cfg(feature = "metrics")]

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use pcloud_config::Environment;
use pcloud_observability::exporter::{ExporterConfig, ExporterHandle, ExporterSnapshot, spawn};
use pcloud_observability::slo::Slo;

use crate::RuntimeShell;

#[derive(Debug, Default, Clone)]
struct Snapshot {
    prometheus_text: String,
    is_clean: bool,
    slo_json: Option<String>,
}

/// Thread-safe snapshot shared with the scrape listener.
///
/// The SLO registry is optional: callers that do not need `/slo` can use
/// [`MetricsBridge::new`]. [`MetricsBridge::with_slo`] wires an SLO
/// registry so the scrape listener can serve live JSON on `/slo`.
#[derive(Clone, Default)]
pub struct MetricsBridge {
    inner: Arc<Mutex<Snapshot>>,
    slo: Option<Arc<Slo>>,
}

impl MetricsBridge {
    /// Create an empty bridge with no SLO registry attached. The
    /// snapshot starts empty and must be populated via
    /// [`MetricsBridge::refresh`] before scrapes see useful data.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach an [`Slo`] registry. `/slo` will report from it on scrape.
    #[must_use]
    pub fn with_slo(mut self, slo: Arc<Slo>) -> Self {
        self.slo = Some(slo);
        self
    }

    /// Clone of the wired SLO registry, if any. Hot-path instrumentation
    /// (IPC dispatch, transfer backend) uses this handle to push samples.
    #[must_use]
    pub fn slo(&self) -> Option<Arc<Slo>> {
        self.slo.clone()
    }

    /// Replace the snapshot from the current runtime state. Called from
    /// the serve loop on a cadence or on material state transitions.
    pub fn refresh(&self, runtime: &RuntimeShell) {
        let text = runtime.observability.families.render_prometheus();
        let clean = !runtime.control.shutdown_requested;
        let slo_json = self.slo.as_ref().map(|s| s.render_json());
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Snapshot {
                prometheus_text: text,
                is_clean: clean,
                slo_json,
            };
        }
    }

    fn read(&self) -> ExporterSnapshot {
        match self.inner.lock() {
            Ok(g) => ExporterSnapshot {
                prometheus_text: g.prometheus_text.clone(),
                is_clean: g.is_clean,
                slo_json: g.slo_json.clone(),
            },
            Err(poisoned) => {
                let g = poisoned.into_inner();
                ExporterSnapshot {
                    prometheus_text: g.prometheus_text.clone(),
                    is_clean: false,
                    slo_json: g.slo_json.clone(),
                }
            }
        }
    }
}

/// Drive the normal IPC serve loop while keeping the bridge snapshot
/// current. Each iteration refreshes the Prometheus text + liveness bit
/// so scrapes see values no staler than one IPC accept cycle.
pub fn serve_with_metrics(
    bound: &pcloud_ipc::BoundIpcServer,
    runtime: &mut RuntimeShell,
    bridge: &MetricsBridge,
) -> Result<(), pcloud_ipc::IpcTransportError> {
    use crate::signals::{self, DrainState};
    use pcloud_ipc::{Method, Request, Response, ResponseStatus};
    use std::io;
    use std::time::Duration;

    let drain_timeout = Duration::from_secs(u64::from(runtime.config.upgrade.drain_timeout_secs));
    let mut drain_deadline: Option<std::time::Instant> = None;
    let poll_interval = Duration::from_millis(100);

    loop {
        bridge.refresh(runtime);
        let shutdown_observed =
            runtime.control.shutdown_requested || crate::signals::shutdown_requested();
        if shutdown_observed {
            runtime.control.shutdown_requested = true;
            bridge.refresh(runtime);
            if signals::begin_drain() || drain_deadline.is_none() {
                drain_deadline = Some(std::time::Instant::now() + drain_timeout);
            }
            let drained = signals::in_flight() == 0;
            let timed_out = drain_deadline
                .map(|d| std::time::Instant::now() >= d)
                .unwrap_or(false);
            if drained || timed_out {
                return Ok(());
            }
            std::thread::sleep(poll_interval);
        }
        if crate::signals::take_reload_request() {
            // Reserved; no-op today. Mirrors serve_until_shutdown.
        }
        let slo_handle = bridge.slo();
        match bound.serve_once(|request| {
            // Drain gate: reject ordinary traffic while shutting down.
            if signals::drain_state() == DrainState::Draining {
                let accept = matches!(
                    request,
                    Request::Plain {
                        method: Method::DrainStatus
                            | Method::Shutdown
                            | Method::GetHealth
                            | Method::Health,
                    }
                );
                if !accept {
                    return Response {
                        status: ResponseStatus::Unavailable,
                        message: "daemon draining, retry".to_owned(),
                    };
                }
            }
            let _guard = signals::InFlightGuard::new();
            let start = Instant::now();
            let resp = crate::dispatch(runtime, request);
            if let Some(slo) = slo_handle.as_ref() {
                slo.observe_ipc_latency(start.elapsed().as_secs_f64());
            }
            resp
        }) {
            Ok(()) => {}
            Err(pcloud_ipc::IpcTransportError::Io(err))
                if err.kind() == io::ErrorKind::Interrupted =>
            {
                continue;
            }
            Err(other) => return Err(other),
        }
    }
}

// Upload retry/started counters are exposed through [`MetricsBridge::slo`].
// TODO(bd-1du): wire `slo.incr_upload_started()` and
// `slo.incr_upload_retry()` into `crates/pcloud-daemon/src/transfer_backend.rs`
// at the points where an upload session is created and where a chunk is
// retried. Exact integration must land alongside P0.3 so retry classification
// matches the retry policy chosen there. Until then, the SLO ratio stays 0.0
// and the endpoint reports `pass: true` for that SLI.

/// Start the HTTP scrape listener bound to the bridge. The returned
/// [`ExporterHandle`] stops the listener on drop and shares the provided
/// `shutdown` flag so SIGTERM/SIGINT propagates to the accept loop.
pub fn spawn_from_env(
    env_kind: Environment,
    shutdown: Arc<AtomicBool>,
    bridge: MetricsBridge,
) -> std::io::Result<ExporterHandle> {
    let allow_wildcard = matches!(env_kind, Environment::Development);
    let cfg = ExporterConfig::from_env(allow_wildcard);
    spawn(cfg, shutdown, move || bridge.read())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_observability::ObservabilityShell;
    use pcloud_observability::metrics::{AuthResult, MetricFamilies};

    fn bridge_with(text: &str, clean: bool) -> MetricsBridge {
        let b = MetricsBridge::new();
        if let Ok(mut g) = b.inner.lock() {
            *g = Snapshot {
                prometheus_text: text.to_owned(),
                is_clean: clean,
                slo_json: None,
            };
        }
        b
    }

    #[test]
    fn bridge_read_returns_current_snapshot() {
        let b = bridge_with("pcloud_x 1\n", true);
        let snap = b.read();
        assert_eq!(snap.prometheus_text, "pcloud_x 1\n");
        assert!(snap.is_clean);
    }

    #[test]
    fn spawn_binds_loopback_in_production() {
        let shutdown = Arc::new(AtomicBool::new(false));
        // Force port 0 via env so we don't collide.
        // SAFETY: test is single-threaded w.r.t. these vars; other tests do
        // not read PCLOUD_METRICS_PORT concurrently.
        unsafe {
            std::env::set_var("PCLOUD_METRICS_PORT", "0");
            std::env::remove_var("PCLOUD_METRICS_BIND_ALL");
        }
        let bridge = MetricsBridge::new();
        let handle =
            spawn_from_env(Environment::Production, shutdown, bridge).expect("spawn exporter");
        assert!(handle.local_addr().ip().is_loopback());
        drop(handle);
        // SAFETY: single-threaded cleanup; no concurrent env readers in this test.
        unsafe {
            std::env::remove_var("PCLOUD_METRICS_PORT");
        }
    }

    #[test]
    fn bridge_with_slo_renders_json_snapshot() {
        let slo = Arc::new(pcloud_observability::slo::Slo::new());
        slo.observe_ipc_latency(0.001);
        slo.incr_upload_started();
        slo.incr_session_started();
        let b = MetricsBridge::new().with_slo(Arc::clone(&slo));
        // Directly refresh the inner snapshot via the slo_json field.
        if let Ok(mut g) = b.inner.lock() {
            *g = Snapshot {
                prometheus_text: String::new(),
                is_clean: true,
                slo_json: Some(slo.render_json()),
            };
        }
        let snap = b.read();
        let js = snap.slo_json.expect("slo_json wired");
        assert!(js.contains("\"ip95_ms\""));
        assert!(js.contains("\"upload_retry_ratio\""));
        assert!(js.contains("\"crash_free_fraction\""));
    }

    #[test]
    fn bridge_reflects_refreshed_prometheus_text() {
        // Build a fake ObservabilityShell and exercise rendering glue.
        let mut obs = ObservabilityShell::default();
        let mut fams = MetricFamilies::default();
        fams.record_auth(AuthResult::Success);
        obs.families = fams;
        let rendered = obs.families.render_prometheus();
        let b = MetricsBridge::new();
        if let Ok(mut g) = b.inner.lock() {
            *g = Snapshot {
                prometheus_text: rendered.clone(),
                is_clean: true,
                slo_json: None,
            };
        }
        let snap = b.read();
        assert!(snap.prometheus_text.contains("pcloud_auth_attempts_total"));
    }
}
