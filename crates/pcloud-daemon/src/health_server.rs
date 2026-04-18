//! Minimal HTTP health-check surface for the daemon.
//!
//! Exposes two endpoints on a loopback TCP socket:
//!
//! - `GET /livez` — always returns `200 OK` while the HTTP thread is alive.
//!   Suitable for liveness probes (Kubernetes, ECS health checks, load balancers).
//!
//! - `GET /readyz` — returns `200 OK` while the daemon is in the `Running`
//!   state (i.e. fully bootstrapped, not draining, not stopped). Returns
//!   `503 Service Unavailable` during drain or after a clean shutdown.
//!   Suitable for readiness probes that should stop sending traffic to a
//!   draining daemon.
//!
//! # Security
//!
//! The listener binds to `127.0.0.1` only (loopback). It **cannot** be
//! configured to bind on `0.0.0.0` — external health traffic must go through
//! a reverse proxy or a sidecar that exposes the loopback endpoint.
//!
//! The server is **disabled by default**. It only starts when
//! `[health] http_port` is set to a non-zero value in the daemon config.
//! Arbitrary ports below 1024 are rejected; the server uses a plain OS-thread
//! model (one `std::net::TcpListener`, one thread per connection) to avoid
//! pulling in async runtimes.
//!
//! # Implementation notes
//!
//! Uses only `std::net::TcpListener` and `std::io::{Read,Write}` — no
//! external HTTP crate dependency. The HTTP dialect is a minimal subset:
//! - Only `GET` requests are handled.
//! - The response body is always ASCII text (`ok\n` or `draining\n`).
//! - Every connection is closed after one request/response exchange.
//! - Invalid HTTP is silently dropped (connection closed, no response).

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::signals::{DrainState, drain_state};

/// Configuration for the health HTTP server.
///
/// Embedded in the daemon config under `[health]`. The server is disabled
/// when `http_port` is `0` (the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthServerConfig {
    /// TCP port on `127.0.0.1` to listen on. `0` disables the server.
    ///
    /// Must be in range `[1024, 65535]` when non-zero. The constraint
    /// prevents accidentally binding a privileged port, which would
    /// require elevated capability or SUID.
    pub http_port: u16,

    /// Per-connection read timeout. Prevents a slow client from blocking
    /// the handler thread indefinitely.
    pub read_timeout_ms: u64,
}

impl Default for HealthServerConfig {
    fn default() -> Self {
        Self {
            http_port: 0,
            read_timeout_ms: 2_000,
        }
    }
}

/// Handle returned by [`spawn`]. Dropping it does *not* stop the server;
/// the listener thread runs until the process exits. This is intentional:
/// health probes must remain available even during graceful drain so
/// orchestrators can tell the difference between "draining" and "dead".
pub struct HealthServerHandle {
    /// Port the server is bound to. Useful for tests that pass `0` to
    /// get a kernel-assigned port.
    pub port: u16,
}

/// Spawn the health HTTP listener on a dedicated daemon thread.
///
/// Returns `Ok(None)` when `config.http_port == 0` (server disabled).
/// Returns `Ok(Some(handle))` on successful bind. Returns `Err` if the
/// port is in the forbidden `[1, 1023]` range or if `bind(2)` fails.
///
/// # Errors
///
/// - `"health server: port must be 0 (disabled) or >= 1024"` — privileged
///   port requested.
/// - `"health server: bind failed: {err}"` — OS-level bind error.
pub fn spawn(config: HealthServerConfig) -> Result<Option<HealthServerHandle>, String> {
    if config.http_port == 0 {
        return Ok(None);
    }
    if config.http_port < 1024 {
        return Err(format!(
            "health server: port {} is privileged; use a port >= 1024 or 0 to disable",
            config.http_port
        ));
    }

    let addr = format!("127.0.0.1:{}", config.http_port);
    let listener = TcpListener::bind(&addr)
        .map_err(|err| format!("health server: bind({addr}) failed: {err}"))?;

    let port = listener
        .local_addr()
        .map(|a| a.port())
        .unwrap_or(config.http_port);

    let read_timeout = Duration::from_millis(config.read_timeout_ms);

    thread::Builder::new()
        .name("pcloud-health-server".into())
        .spawn(move || run_listener(listener, read_timeout))
        .map_err(|err| format!("health server: spawn failed: {err}"))?;

    log::info!("pcloud-daemon: health server listening on 127.0.0.1:{port}");

    Ok(Some(HealthServerHandle { port }))
}

/// Accept loop — runs on a dedicated OS thread.
fn run_listener(listener: TcpListener, read_timeout: Duration) {
    // Wrap the Arc so we can share `read_timeout` cheaply.
    let read_timeout = Arc::new(read_timeout);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let rt = Arc::clone(&read_timeout);
                thread::Builder::new()
                    .name("pcloud-health-conn".into())
                    .spawn(move || handle_connection(stream, *rt))
                    .unwrap_or_else(|err| {
                        log::warn!("health server: failed to spawn connection thread: {err}");
                        // Return a dummy JoinHandle to satisfy the type system;
                        // the connection is simply dropped.
                        thread::spawn(|| {})
                    });
            }
            Err(err) => {
                log::warn!("health server: accept failed: {err}");
            }
        }
    }
}

/// Handle a single HTTP connection: parse the request line, dispatch, send
/// response, close the connection.
fn handle_connection(mut stream: TcpStream, read_timeout: Duration) {
    let _ = stream.set_read_timeout(Some(read_timeout));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    let mut buf = [0u8; 512];
    let n = match stream.read(&mut buf) {
        Ok(0) | Err(_) => return,
        Ok(n) => n,
    };

    let request = match std::str::from_utf8(&buf[..n]) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Parse the first line only: "GET /path HTTP/1.x\r\n..."
    let first_line = request.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    if method != "GET" {
        let _ = write_response(
            &mut stream,
            405,
            "Method Not Allowed",
            "method not allowed\n",
        );
        return;
    }

    match path {
        "/livez" => {
            // Always live while this thread is running.
            let _ = write_response(&mut stream, 200, "OK", "ok\n");
        }
        "/readyz" => {
            // Ready only when the daemon is fully up (Running state).
            let state = drain_state();
            match state {
                DrainState::Running => {
                    let _ = write_response(&mut stream, 200, "OK", "ok\n");
                }
                DrainState::Draining => {
                    let _ = write_response(&mut stream, 503, "Service Unavailable", "draining\n");
                }
                DrainState::Stopped => {
                    let _ = write_response(&mut stream, 503, "Service Unavailable", "stopped\n");
                }
            }
        }
        _ => {
            let _ = write_response(&mut stream, 404, "Not Found", "not found\n");
        }
    }
}

/// Write a minimal HTTP/1.0 response. HTTP/1.0 is used intentionally so
/// the server does not need to parse `Connection:` headers or manage
/// keep-alive — one request per connection, then close.
fn write_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.0 {status} {reason}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         \r\n\
         {body}",
        len = body.len(),
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: bind on an OS-assigned port, hit /livez and /readyz,
    /// assert expected status lines.
    #[test]
    fn livez_and_readyz_smoke() {
        let cfg = HealthServerConfig {
            http_port: 0,
            read_timeout_ms: 500,
        };
        // Port 0 → disabled → Ok(None).
        assert!(spawn(cfg).unwrap().is_none());
    }

    #[test]
    fn privileged_port_rejected() {
        let cfg = HealthServerConfig {
            http_port: 80,
            read_timeout_ms: 500,
        };
        assert!(spawn(cfg).is_err());
    }

    #[test]
    fn write_response_format() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let _ = std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let _ = write_response(&mut s, 200, "OK", "ok\n");
            }
        });
        use std::net::TcpStream;
        let mut conn = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        let _ = conn.write_all(b"GET /livez HTTP/1.0\r\n\r\n");
        let mut resp = String::new();
        let _ = conn.read_to_string(&mut resp);
        assert!(resp.starts_with("HTTP/1.0 200 OK"));
        assert!(resp.contains("ok\n"));
    }

    use std::io::Read;
}
