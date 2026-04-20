//! Zero-dependency HTTP responder for Prometheus scraping.
//!
//! # Bind policy (security)
//!
//! The default bind address is `127.0.0.1:${PCLOUD_METRICS_PORT:-9353}`.
//! Wildcard binding (`0.0.0.0`) is **double-gated** and requires ALL of:
//!
//! 1. [`ExporterConfig::allow_wildcard`] set to `true` by the caller —
//!    the daemon only sets this when the resolved environment is
//!    `Environment::Development`.
//! 2. The environment variable `PCLOUD_METRICS_BIND_ALL=1` set in the
//!    process environment.
//!
//! Either gate missing ⇒ loopback-only. This makes accidental exposure in
//! production a two-step mistake rather than a one-flag mistake.
//!
//! # Endpoints
//!
//! - `GET /metrics` — `text/plain; version=0.0.4` Prometheus exposition.
//! - `GET /health`  — `200 ok` when the daemon reports clean, else `503 not ready`.
//! - `GET /slo`     — `application/json` SLO snapshot (see [`crate::slo`]);
//!   returns `503 {"error":"slo_not_configured"}` when no SLO body is
//!   provided by the snapshot closure.
//!
//! Any other path returns `404`. Any non-`GET` method returns `405`.
//! Responses carry `Cache-Control: no-store` and `Connection: close`.
//!
//! # Design
//!
//! - std-only (`std::net::TcpListener`); no hyper / axum / tokio.
//! - Per-connection read/write timeouts bound slow-loris exposure.
//! - Request line + headers are drained with an 8 KiB header cap.
//! - Cooperative shutdown via a shared [`AtomicBool`] flag; the accept
//!   loop is non-blocking with a 100 ms sleep so signals trigger prompt
//!   exit and the listener thread joins cleanly on drop.
//! - Bodies come from a caller-supplied snapshot closure; this module
//!   never reaches into runtime state and never logs labels, so secret
//!   labels cannot leak through this endpoint.
//!
//! # Environment variables
//!
//! | Variable                  | Meaning                                     |
//! |---------------------------|---------------------------------------------|
//! | [`ENV_METRICS_PORT`]      | Override listen port (default 9353)         |
//! | [`ENV_METRICS_BIND_ALL`]  | `=1` enables wildcard bind (dev-gated)      |
//!
//! Feature-gated under `prometheus-exporter` (same gate as the metric
//! families). The daemon re-exports this behind its own `metrics` feature.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Default scrape port. Chosen from the unregistered 9300-9399 block and
/// does not collide with common Prometheus defaults.
pub const DEFAULT_METRICS_PORT: u16 = 9353;

/// Environment variable overriding the listen port.
pub const ENV_METRICS_PORT: &str = "PCLOUD_METRICS_PORT";
/// Environment variable (+ dev-only gate) enabling wildcard binding.
pub const ENV_METRICS_BIND_ALL: &str = "PCLOUD_METRICS_BIND_ALL";

/// Snapshot returned by the runtime on every scrape. Intentionally owned
/// strings/bools so the closure can sample lock-free state under a mutex
/// without holding a borrow across the write to the socket.
pub struct ExporterSnapshot {
    /// Rendered Prometheus 0.0.4 exposition body for `GET /metrics`.
    pub prometheus_text: String,
    /// Liveness bit driving `GET /health`. `true` renders `200 ok`,
    /// `false` renders `503 not ready`.
    pub is_clean: bool,
    /// JSON body returned by `GET /slo`. May be `None` if the caller has
    /// not wired an SLO registry; the endpoint then reports `503`.
    pub slo_json: Option<String>,
}

impl ExporterSnapshot {
    /// Backwards-compatible constructor that does not provide `/slo` data.
    #[must_use]
    pub fn new(prometheus_text: String, is_clean: bool) -> Self {
        Self {
            prometheus_text,
            is_clean,
            slo_json: None,
        }
    }
}

/// Configuration for the exporter listener.
///
/// The default bind address is `127.0.0.1:${port}`. Wildcard binding
/// (`0.0.0.0`) requires BOTH `allow_wildcard = true` AND the environment
/// variable `PCLOUD_METRICS_BIND_ALL=1`. This double-gate prevents
/// accidental exposure in production: the daemon only sets
/// `allow_wildcard` in `Environment::Development`.
#[derive(Debug, Clone)]
pub struct ExporterConfig {
    /// TCP port. `0` tells the OS to choose an ephemeral port (used in
    /// tests).
    pub port: u16,
    /// If `true`, the caller permits wildcard bind when
    /// `PCLOUD_METRICS_BIND_ALL=1` is set. Callers MUST only set this in
    /// `Environment::Development`.
    pub allow_wildcard: bool,
}

impl ExporterConfig {
    /// Build a config from environment variables. `allow_wildcard` must be
    /// wired by the daemon based on its resolved `Environment`.
    #[must_use]
    pub fn from_env(allow_wildcard: bool) -> Self {
        let port = std::env::var(ENV_METRICS_PORT)
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(DEFAULT_METRICS_PORT);
        Self {
            port,
            allow_wildcard,
        }
    }

    fn resolve_bind_addr(&self) -> SocketAddr {
        let wildcard =
            self.allow_wildcard && std::env::var(ENV_METRICS_BIND_ALL).as_deref() == Ok("1");
        let ip = if wildcard {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        };
        SocketAddr::new(ip, self.port)
    }
}

/// Handle to a running exporter. Dropping the handle signals the listener
/// to stop and joins the accept thread.
pub struct ExporterHandle {
    shutdown: Arc<AtomicBool>,
    local_addr: SocketAddr,
    join: Option<JoinHandle<()>>,
}

impl ExporterHandle {
    /// Return the address the listener is bound to. Useful for tests that
    /// request ephemeral ports via `port: 0`.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Request shutdown and join the accept thread.
    pub fn shutdown(mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for ExporterHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Start the HTTP scrape listener. The provided `snapshot_fn` is called
/// on every request to produce the exposition body and liveness bit.
///
/// Returns a [`TcpListener`]-based server; `shutdown` shares the
/// daemon's signal-driven shutdown flag when the caller passes one in.
pub fn spawn<F>(
    config: ExporterConfig,
    shutdown: Arc<AtomicBool>,
    snapshot_fn: F,
) -> std::io::Result<ExporterHandle>
where
    F: Fn() -> ExporterSnapshot + Send + Sync + 'static,
{
    let addr = config.resolve_bind_addr();
    let listener = TcpListener::bind(addr)?;
    // Short accept timeout so shutdown flips promptly.
    listener.set_nonblocking(false)?;
    let local_addr = listener.local_addr()?;
    let shutdown_for_thread = Arc::clone(&shutdown);
    let snapshot = Arc::new(snapshot_fn);

    let join = thread::Builder::new()
        .name("pcloud-metrics-http".into())
        .spawn(move || accept_loop(listener, shutdown_for_thread, snapshot))?;

    Ok(ExporterHandle {
        shutdown,
        local_addr,
        join: Some(join),
    })
}

fn accept_loop<F>(listener: TcpListener, shutdown: Arc<AtomicBool>, snapshot: Arc<F>)
where
    F: Fn() -> ExporterSnapshot + Send + Sync + 'static,
{
    // Poll accept with a short timeout so shutdown propagates.
    // SAFETY: `set_nonblocking(true)` on a freshly-created `TcpListener` only
    // fails for kernel-level socket misconfiguration. If this panics, the
    // host socket subsystem is broken and the exporter cannot function.
    listener
        .set_nonblocking(true)
        .expect("set_nonblocking on listener");

    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let snap = Arc::clone(&snapshot);
                // Per-connection thread; bound lifetime by timeouts below.
                let _ = thread::Builder::new()
                    .name("pcloud-metrics-conn".into())
                    .spawn(move || handle_connection(stream, snap.as_ref()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(_) => {
                // Transient accept error; loop and re-check shutdown.
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn handle_connection<F>(mut stream: TcpStream, snapshot_fn: &F)
where
    F: Fn() -> ExporterSnapshot,
{
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let (method, path) = match read_request_line(&mut stream) {
        Some(p) => p,
        None => {
            write_response(
                &mut stream,
                400,
                "text/plain; charset=utf-8",
                b"bad request",
            );
            return;
        }
    };

    if method != "GET" {
        write_response(
            &mut stream,
            405,
            "text/plain; charset=utf-8",
            b"method not allowed",
        );
        return;
    }

    match path.as_str() {
        "/metrics" => {
            let snap = snapshot_fn();
            write_response(
                &mut stream,
                200,
                "text/plain; version=0.0.4; charset=utf-8",
                snap.prometheus_text.as_bytes(),
            );
        }
        "/health" => {
            let snap = snapshot_fn();
            if snap.is_clean {
                write_response(&mut stream, 200, "text/plain; charset=utf-8", b"ok");
            } else {
                write_response(&mut stream, 503, "text/plain; charset=utf-8", b"not ready");
            }
        }
        "/slo" => {
            let snap = snapshot_fn();
            match snap.slo_json {
                Some(body) => write_response(
                    &mut stream,
                    200,
                    "application/json; charset=utf-8",
                    body.as_bytes(),
                ),
                None => write_response(
                    &mut stream,
                    503,
                    "application/json; charset=utf-8",
                    b"{\"error\":\"slo_not_configured\"}",
                ),
            }
        }
        _ => {
            write_response(&mut stream, 404, "text/plain; charset=utf-8", b"not found");
        }
    }
}

/// Read the request line and discard headers up to the terminating blank
/// line. Caps total bytes read to prevent slow-loris / header flooding.
fn read_request_line(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let mut parts = trimmed.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();

    // Drain remaining headers, bounded.
    let mut drained = 0usize;
    loop {
        let mut header = String::new();
        let n = reader.read_line(&mut header).ok()?;
        if n == 0 {
            break;
        }
        drained = drained.saturating_add(n);
        if drained > 8 * 1024 {
            break;
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
    }
    // Discard any buffered body bytes on the underlying stream.
    let mut sink = [0u8; 0];
    let _ = reader.get_mut().read(&mut sink);
    Some((method, path))
}

fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         Cache-Control: no-store\r\n\
         \r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start_ephemeral<F>(snapshot_fn: F) -> ExporterHandle
    where
        F: Fn() -> ExporterSnapshot + Send + Sync + 'static,
    {
        let cfg = ExporterConfig {
            port: 0,
            allow_wildcard: false,
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        spawn(cfg, shutdown, snapshot_fn).expect("spawn exporter")
    }

    fn scrape(addr: SocketAddr, path: &str) -> (String, String) {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        let mut buf = String::new();
        stream.read_to_string(&mut buf).unwrap();
        let mut split = buf.splitn(2, "\r\n\r\n");
        let headers = split.next().unwrap_or("").to_owned();
        let body = split.next().unwrap_or("").to_owned();
        (headers, body)
    }

    #[test]
    fn metrics_endpoint_returns_prometheus_text() {
        let body_text = "# HELP pcloud_test 1\n# TYPE pcloud_test counter\npcloud_test 42\n";
        let bt = body_text.to_owned();
        let h = start_ephemeral(move || ExporterSnapshot {
            prometheus_text: bt.clone(),
            is_clean: true,
            slo_json: None,
        });
        let addr = h.local_addr();
        let (headers, body) = scrape(addr, "/metrics");
        assert!(headers.starts_with("HTTP/1.1 200"), "headers={headers}");
        assert!(
            headers.contains("text/plain; version=0.0.4"),
            "headers={headers}"
        );
        assert!(body.contains("pcloud_test 42"));
    }

    #[test]
    fn health_ok_and_unready() {
        use std::sync::Mutex;
        let clean = Arc::new(Mutex::new(true));
        let c2 = Arc::clone(&clean);
        let h = start_ephemeral(move || ExporterSnapshot {
            prometheus_text: String::new(),
            is_clean: *c2.lock().unwrap(),
            slo_json: None,
        });
        let addr = h.local_addr();
        let (headers, body) = scrape(addr, "/health");
        assert!(headers.starts_with("HTTP/1.1 200"));
        assert_eq!(body, "ok");

        *clean.lock().unwrap() = false;
        let (headers, _) = scrape(addr, "/health");
        assert!(headers.starts_with("HTTP/1.1 503"), "headers={headers}");
    }

    #[test]
    fn unknown_path_returns_404() {
        let h = start_ephemeral(|| ExporterSnapshot {
            prometheus_text: String::new(),
            is_clean: true,
            slo_json: None,
        });
        let (headers, _) = scrape(h.local_addr(), "/nope");
        assert!(headers.starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn non_get_rejected() {
        let h = start_ephemeral(|| ExporterSnapshot {
            prometheus_text: String::new(),
            is_clean: true,
            slo_json: None,
        });
        let mut stream = TcpStream::connect(h.local_addr()).unwrap();
        stream
            .write_all(b"POST /metrics HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut buf = String::new();
        stream.read_to_string(&mut buf).unwrap();
        assert!(buf.starts_with("HTTP/1.1 405"), "buf={buf}");
    }

    #[test]
    fn slo_endpoint_returns_json_schema() {
        use crate::slo::Slo;
        // Populate some data so the SLO snapshot is non-trivial.
        let slo = Arc::new(Slo::new());
        for _ in 0..10 {
            slo.observe_ipc_latency(0.001);
            slo.incr_upload_started();
            slo.incr_session_started();
        }
        let slo_scrape = Arc::clone(&slo);
        let h = start_ephemeral(move || ExporterSnapshot {
            prometheus_text: String::new(),
            is_clean: true,
            slo_json: Some(slo_scrape.render_json()),
        });
        let (headers, body) = scrape(h.local_addr(), "/slo");
        assert!(headers.starts_with("HTTP/1.1 200"), "headers={headers}");
        assert!(headers.contains("application/json"), "headers={headers}");
        // Schema assertions only — values are tested in the slo module.
        assert!(body.contains("\"ip95_ms\""), "body={body}");
        assert!(body.contains("\"upload_retry_ratio\""), "body={body}");
        assert!(body.contains("\"crash_free_fraction\""), "body={body}");
        assert!(
            body.contains("\"pass\":true") || body.contains("\"pass\":false"),
            "body={body}"
        );
    }

    #[test]
    fn slo_endpoint_503_when_not_configured() {
        let h = start_ephemeral(|| ExporterSnapshot {
            prometheus_text: String::new(),
            is_clean: true,
            slo_json: None,
        });
        let (headers, body) = scrape(h.local_addr(), "/slo");
        assert!(headers.starts_with("HTTP/1.1 503"), "headers={headers}");
        assert!(body.contains("slo_not_configured"));
    }

    #[test]
    fn loopback_bind_by_default() {
        let cfg = ExporterConfig {
            port: 0,
            allow_wildcard: false,
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let h = spawn(cfg, shutdown, || ExporterSnapshot {
            prometheus_text: String::new(),
            is_clean: true,
            slo_json: None,
        })
        .expect("spawn");
        assert!(h.local_addr().ip().is_loopback());
    }
}
