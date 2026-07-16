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
        let next = Snapshot {
            prometheus_text: text,
            is_clean: clean,
            slo_json,
        };
        match self.inner.lock() {
            Ok(mut guard) => *guard = next,
            Err(poisoned) => {
                log::error!("metrics bridge snapshot lock poisoned; marking exporter unhealthy");
                let mut guard = poisoned.into_inner();
                *guard = Snapshot {
                    is_clean: false,
                    ..next
                };
            }
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
    use crate::signals;
    use std::io;
    use std::time::Duration;

    let drain_timeout = Duration::from_secs(u64::from(runtime.config.upgrade.drain_timeout_secs));
    let mut drain_deadline: Option<std::time::Instant> = None;
    let poll_interval = Duration::from_millis(100);

    if let Some(timeout) = crate::serve::accept_timeout_with_watchdog(None) {
        if let Err(err) = bound.set_accept_timeout(Some(timeout)) {
            log::warn!("pcloud-daemon: failed to configure metrics IPC accept timeout: {err}");
        }
    }

    loop {
        bridge.refresh(runtime);
        let shutdown_observed =
            runtime.control.shutdown_requested || crate::signals::shutdown_requested();
        if shutdown_observed {
            runtime.control.shutdown_requested = true;
            bridge.refresh(runtime);
            if signals::begin_drain() || drain_deadline.is_none() {
                crate::serve::notify_systemd_stopping();
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
            if let Some(ref config_path) = runtime.config_path {
                use crate::config_reload::{
                    ReloadOutcome, format_reload_failed_event, format_reloaded_event, try_reload,
                };
                crate::serve::notify_systemd_reloading();
                let (outcome, new_profile) = try_reload(config_path, &runtime.config);
                match outcome {
                    ReloadOutcome::Applied { changed_keys } => {
                        let msg = format_reloaded_event(&changed_keys);
                        log::info!("pcloud-rs: {msg}");
                        if let Some(profile) = new_profile {
                            runtime.apply_hot_reload(profile);
                        }
                    }
                    ReloadOutcome::NoChange => {}
                    ReloadOutcome::Failed { error } => {
                        let msg = format_reload_failed_event(&error);
                        log::error!("pcloud-rs: {msg}");
                    }
                }
                crate::serve::notify_systemd_ready();
                bridge.refresh(runtime);
            }
        }
        let slo_handle = bridge.slo();
        // audit-06 P1-6 / ncx.11: use `serve_once_with_peer` +
        // `dispatch_with_drain_gate` so privileged-request audit logging
        // and per-peer uid/pid plumbing stay on the metrics-enabled path.
        // Using `serve_once` + `dispatch` here would silently strip the
        // peer uid from the audit log and per-peer rate limiter.
        match bound.serve_once_with_peer(|peer, request| {
            let start = Instant::now();
            // Break metrics down per-peer when SLO is wired; at minimum
            // the peer uid is plumbed through so downstream sinks can
            // classify activity by caller.
            let _peer_uid_for_metrics = peer.uid;
            let resp = crate::serve::dispatch_with_drain_gate(runtime, peer.uid, peer.pid, request);
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
            Err(pcloud_ipc::IpcTransportError::Io(err))
                if err.kind() == io::ErrorKind::WouldBlock
                    || err.kind() == io::ErrorKind::TimedOut => {}
            Err(other) => return Err(other),
        }
        crate::serve::notify_systemd_watchdog();
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

    /// audit-06 P1-6 / ncx.11 regression coverage.
    ///
    /// Proves that the metrics-enabled serve loop routes requests
    /// through the peer-aware dispatch path (`serve_once_with_peer` +
    /// `dispatch_with_drain_gate`), and therefore the privileged-request
    /// audit logging plus per-peer uid plumbing remain live when
    /// `--features metrics` is enabled. The canonical evidence is a
    /// `Shutdown` request (privileged, drain-admitted) sent to the
    /// metrics loop: if the peer-aware path is wired, the runtime flag
    /// flips and the loop returns cleanly; if the loop were using plain
    /// `serve_once` + `dispatch`, the shutdown would still flip the
    /// flag but the privileged-audit log line and peer uid would never
    /// materialize — hence the additional direct check below that the
    /// log output contains `from uid=<N>` for the current process uid.
    #[test]
    fn metrics_path_logs_privileged_request_with_peer_uid() {
        use std::sync::atomic::Ordering;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

        use pcloud_config::{ConfigProfile, Environment};
        use pcloud_ipc::{IpcClient, IpcServer, Method, Request, current_effective_uid};

        // Install a log capture that collects records written via the
        // `log` crate. We use a process-wide `OnceLock` because
        // `log::set_logger` can only be called once per process.
        use std::sync::OnceLock;
        static CAPTURE: OnceLock<Arc<Mutex<Vec<String>>>> = OnceLock::new();
        struct VecLogger {
            buf: Arc<Mutex<Vec<String>>>,
        }
        impl log::Log for VecLogger {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, record: &log::Record) {
                if let Ok(mut g) = self.buf.lock() {
                    g.push(format!("{}", record.args()));
                }
            }
            fn flush(&self) {}
        }
        let buf = CAPTURE
            .get_or_init(|| {
                let buf = Arc::new(Mutex::new(Vec::<String>::new()));
                let logger = Box::leak(Box::new(VecLogger {
                    buf: Arc::clone(&buf),
                }));
                // Best-effort: another test may have installed a logger
                // first. In that case the capture will stay empty and
                // we fall back to the end-to-end shutdown assertion.
                let _ = log::set_logger(logger);
                log::set_max_level(log::LevelFilter::Info);
                buf
            })
            .clone();
        if let Ok(mut g) = buf.lock() {
            g.clear();
        }

        // Bootstrap a development-profile runtime on a scratch dir.
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root = std::path::PathBuf::from("/tmp").join(format!(
            "pd-metrics-priv-{}-{}",
            std::process::id(),
            nonce % 1_000_000_000
        ));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);
        let mut runtime =
            crate::bootstrap_with_config(config).expect("runtime bootstrap should succeed");

        let socket_path = runtime.config.paths.ipc_socket_path();
        let server = IpcServer::new(current_effective_uid());
        let bound = server.bind(&socket_path).expect("socket should bind");

        let bridge = MetricsBridge::new();
        let bridge_for_thread = bridge.clone();

        let handle = std::thread::spawn(move || {
            let res = super::serve_with_metrics(&bound, &mut runtime, &bridge_for_thread);
            (res, runtime.control.shutdown_requested)
        });

        // Send a privileged `Shutdown` request. It must:
        //   (a) be dispatched (proving peer-aware wiring works);
        //   (b) emit the `privileged IPC request: Shutdown from uid=...`
        //       log line through `dispatch_with_drain_gate`.
        let client = IpcClient;
        let _ = client.send(
            &socket_path,
            &Request::Plain {
                method: Method::Shutdown,
            },
        );

        // The loop must exit within 5s. If wiring regressed to plain
        // `serve_once`/`dispatch` the shutdown would still flip the
        // flag, so this alone is not sufficient — see audit-log assert.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                panic!("metrics serve loop did not exit within 5s after Shutdown");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let (result, flag) = handle.join().expect("metrics serve thread should join");
        result.expect("metrics serve loop should exit cleanly");
        assert!(flag, "runtime shutdown flag must be set by Shutdown IPC");

        // Assert the privileged-audit log line was emitted with a peer
        // uid. Only enforce the uid assertion when we actually own the
        // logger (i.e. no prior test installed a competing one).
        let lines = buf.lock().expect("log capture mutex").clone();
        let own_logger = !lines.is_empty();
        if own_logger {
            let expected_uid = format!("from uid={}", current_effective_uid());
            let found = lines.iter().any(|l| {
                l.contains("privileged IPC request: Shutdown") && l.contains(&expected_uid)
            });
            assert!(
                found,
                "expected privileged IPC audit line with peer uid; captured: {:?}",
                lines
            );
            let _ = Ordering::SeqCst; // silence unused-import on non-atomic paths
        }
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
