//! T1.6.c — TcpListener accept loop that drives the dispatcher.
//!
//! # Threading model
//!
//! Single-thread, blocking I/O. WebDAV traffic on a local-only
//! listener is low concurrency by definition — the operator's file
//! manager (or `cadaver`) opens a handful of connections, never
//! thousands — so handling them sequentially keeps the surface
//! debuggable and avoids dragging in `tokio` / `mio`. If a future
//! deployment needs concurrency, swapping in a tiny thread pool
//! around `serve_one` is a one-screen change.
//!
//! # Read budget
//!
//! Each connection is allowed at most
//! `http::MAX_HEADER_BYTES` (16 KiB) for the request line +
//! headers, plus the body declared by `Content-Length`, capped by
//! [`ServerConfig::max_put_body_bytes`]. Reads are bounded by an
//! explicit `read_until_double_crlf` walk that aborts the moment
//! the cap is exceeded — there is no allocation in the cap-overflow
//! path beyond the read buffer itself.
//!
//! # Shutdown
//!
//! Test builds use [`TcpServer::serve_one`] which handles one
//! connection and returns; production builds use [`TcpServer::run`]
//! which loops until the listener errors. Both honour an optional
//! `should_stop` flag the caller can flip to ask the experimental server to
//! stop accepting new connections at the next iteration.

// **PLATFORM:** all
// **GATING:** none.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::handler::{IpcBackend, dispatch};
use crate::http::{HttpParseError, MAX_HEADER_BYTES, parse_request};
use crate::{ListenerBinding, ServerConfig};

/// Errors raised while binding or running the WebDAV listener.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// `ServerConfig::validate` failed before binding the listener.
    #[error(transparent)]
    Config(#[from] crate::ConfigError),
    /// Socket binding / accept I/O.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The configured binding is not currently supported on this
    /// build (Unix-socket binding lands in a follow-up; only
    /// `LocalTcp` works in T1.6.c).
    #[error("WebDAV listener binding {0} not yet supported")]
    UnsupportedBinding(&'static str),
}

/// Bound loopback TCP server for the experimental WebDAV subset.
#[derive(Debug)]
pub struct TcpServer {
    listener: TcpListener,
    cfg: ServerConfig,
    /// Optional "stop now" flag the caller can flip from another
    /// thread between connections. Honoured by [`Self::run`] (which
    /// polls before each `accept`).
    stop: Arc<AtomicBool>,
}

impl TcpServer {
    /// Bind a TCP server using `cfg`.
    ///
    /// # Errors
    ///
    /// - [`ServerError::Config`] if validation fails.
    /// - [`ServerError::UnsupportedBinding`] if `cfg.binding` is a
    ///   Unix socket (T1.6.c only ships TCP; the Unix socket path
    ///   is the next sub-step).
    /// - [`ServerError::Io`] if the OS refuses the bind.
    pub fn bind(cfg: ServerConfig) -> Result<Self, ServerError> {
        cfg.validate()?;
        let listener = match &cfg.binding {
            ListenerBinding::LocalTcp { host, port } => {
                let addr = SocketAddr::new(*host, *port);
                TcpListener::bind(addr)?
            }
            ListenerBinding::UnixSocket { .. } => {
                return Err(ServerError::UnsupportedBinding("UnixSocket"));
            }
        };
        Ok(Self {
            listener,
            cfg,
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Local address the OS bound the listener to. Useful in tests
    /// that asked for `port = 0` and need to know which ephemeral
    /// port the kernel picked.
    ///
    /// # Errors
    ///
    /// Propagates the underlying [`TcpListener::local_addr`] error.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Stop flag handle. Flipping `store(true)` causes
    /// [`Self::run`] to return after the in-flight connection (if
    /// any) finishes.
    #[must_use]
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    /// Accept exactly one connection, dispatch it, and return.
    ///
    /// Useful for tests and one-shot smoke probes (`curl -X
    /// PROPFIND ...`).
    ///
    /// # Errors
    ///
    /// Propagates any I/O error from `accept` or the connection
    /// handler.
    pub fn serve_one(&self, backend: &mut dyn IpcBackend) -> Result<(), ServerError> {
        let (stream, _peer) = self.listener.accept()?;
        handle_connection(stream, &self.cfg, backend)
    }

    /// Run the server until the stop flag is set. Errors during
    /// individual connections are logged and the loop continues —
    /// a single misbehaving client must not bring the server down.
    ///
    /// # Errors
    ///
    /// Returns only when `accept` itself fails fatally (e.g. the
    /// listener was forcibly closed).
    pub fn run(&self, backend: &mut dyn IpcBackend) -> Result<(), ServerError> {
        // Short accept timeout so the loop wakes up periodically and
        // can re-check the stop flag without needing an OS-level
        // shutdown signal.
        self.listener.set_nonblocking(false)?;
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            // Use poll-friendly accept with a small read deadline;
            // since stdlib TcpListener does not expose accept-with-
            // timeout, we rely on the stop flag being checked
            // between connections instead of within the accept call.
            // Operators wanting a faster shutdown should also drop
            // the listener (closes the FD) which makes accept fail
            // immediately.
            match self.listener.accept() {
                Ok((stream, _peer)) => {
                    if let Err(err) = handle_connection(stream, &self.cfg, backend) {
                        log::warn!("webdav: connection error: {err}");
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(err) => return Err(ServerError::Io(err)),
            }
        }
    }
}

/// Per-connection handler. Reads one HTTP/1.1 request, dispatches
/// it, writes the response, and drops the stream.
fn handle_connection(
    mut stream: TcpStream,
    cfg: &ServerConfig,
    backend: &mut dyn IpcBackend,
) -> Result<(), ServerError> {
    // Bound the wall-clock cost of a stuck client.
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    let raw = match read_request_bytes(&mut stream, cfg.max_put_body_bytes) {
        Ok(buf) => buf,
        Err(read_err) => {
            // Surface a structured 400 instead of dropping the
            // connection silently — easier to debug from a curl
            // session.
            let resp = read_error_response(&read_err);
            let _ = stream.write_all(&resp);
            return Ok(());
        }
    };
    let resp_bytes = match parse_request(&raw) {
        Ok(req) => dispatch(&req, cfg, backend).serialize(),
        Err(parse_err) => parse_error_response(&parse_err),
    };
    stream.write_all(&resp_bytes)?;
    Ok(())
}

/// Errors specific to reading the request bytes off the wire.
#[derive(Debug)]
enum ReadRequestError {
    HeaderTooLarge,
    BodyTooLarge,
    BadContentLength,
    Io(std::io::Error),
}

impl From<std::io::Error> for ReadRequestError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Read until the `\r\n\r\n` boundary, then read the declared
/// `Content-Length` body. Bounded by the header cap and the
/// configured body cap.
fn read_request_bytes(stream: &mut TcpStream, max_body: u64) -> Result<Vec<u8>, ReadRequestError> {
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 1024];
    // Phase 1: read until we see `\r\n\r\n` or hit the header cap.
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            // Peer closed before we saw the header terminator —
            // treat as bad request rather than IO error so the
            // caller emits 400.
            return Err(ReadRequestError::HeaderTooLarge);
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > MAX_HEADER_BYTES {
            return Err(ReadRequestError::HeaderTooLarge);
        }
    }
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(ReadRequestError::HeaderTooLarge)?;
    let body_start = header_end + 4;
    // Phase 2: figure out how many bytes of body we still need.
    let header_section = &buf[..header_end];
    let header_text =
        std::str::from_utf8(header_section).map_err(|_| ReadRequestError::HeaderTooLarge)?;
    let mut content_length: u64 = 0;
    for line in header_text.split("\r\n").skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v
                    .trim()
                    .parse()
                    .map_err(|_| ReadRequestError::BadContentLength)?;
                break;
            }
        }
    }
    if content_length > max_body {
        return Err(ReadRequestError::BodyTooLarge);
    }
    // Phase 3: read the rest of the body if not already buffered.
    let already = (buf.len() - body_start) as u64;
    if already < content_length {
        let mut remaining = (content_length - already) as usize;
        while remaining > 0 {
            let want = remaining.min(tmp.len());
            let n = stream.read(&mut tmp[..want])?;
            if n == 0 {
                return Err(ReadRequestError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "client closed connection before sending full body",
                )));
            }
            buf.extend_from_slice(&tmp[..n]);
            remaining -= n;
        }
    }
    Ok(buf)
}

fn read_error_response(err: &ReadRequestError) -> Vec<u8> {
    let (status, msg) = match err {
        ReadRequestError::HeaderTooLarge => (431, "Request Header Fields Too Large"),
        ReadRequestError::BodyTooLarge => (413, "Payload Too Large"),
        ReadRequestError::BadContentLength => (400, "Bad Request"),
        ReadRequestError::Io(err) => {
            log::debug!("webdav: failed while reading request: {err}");
            (400, "Bad Request")
        }
    };
    let body = msg.as_bytes();
    let head = format!(
        "HTTP/1.1 {status} {msg}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

fn parse_error_response(err: &HttpParseError) -> Vec<u8> {
    let status = match err {
        HttpParseError::HeadersTooLarge { .. } => 431,
        HttpParseError::TooManyHeaders { .. } => 431,
        HttpParseError::ShortBody => 400,
        _ => 400,
    };
    let msg = err.to_string();
    let body = msg.as_bytes();
    let head = format!(
        "HTTP/1.1 {status} Bad Request\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::{BackendEntry, BackendError, PutOutcome};
    use std::collections::HashMap;
    use std::net::IpAddr;
    use std::thread;

    /// Tiny mock identical in spirit to the dispatcher tests but
    /// with its own copy here so this module stays self-contained.
    #[derive(Default)]
    struct MockBackend {
        files: HashMap<String, (BackendEntry, Vec<u8>)>,
        folders: HashMap<String, BackendEntry>,
    }

    impl MockBackend {
        fn add_folder(&mut self, path: &str) {
            self.folders.insert(
                path.to_owned(),
                BackendEntry {
                    name: leaf(path),
                    is_collection: true,
                    content_length: None,
                    last_modified: None,
                    content_type: None,
                },
            );
        }
        fn add_file(&mut self, path: &str, body: &[u8]) {
            self.files.insert(
                path.to_owned(),
                (
                    BackendEntry {
                        name: leaf(path),
                        is_collection: false,
                        content_length: Some(body.len() as u64),
                        last_modified: None,
                        content_type: Some("text/plain".into()),
                    },
                    body.to_vec(),
                ),
            );
        }
    }

    fn leaf(path: &str) -> String {
        path.rsplit('/').next().unwrap_or("").to_owned()
    }

    impl IpcBackend for MockBackend {
        fn list_folder(&self, path: &str) -> Result<Vec<BackendEntry>, BackendError> {
            if !self.folders.contains_key(path) {
                return Err(BackendError::NotFound);
            }
            let prefix = if path.ends_with('/') {
                path.to_owned()
            } else {
                format!("{path}/")
            };
            let mut out = Vec::new();
            for (p, (entry, _)) in &self.files {
                if p.starts_with(&prefix) && !p[prefix.len()..].contains('/') {
                    out.push(entry.clone());
                }
            }
            for (p, entry) in &self.folders {
                if p == path {
                    continue;
                }
                if p.starts_with(&prefix) && !p[prefix.len()..].contains('/') {
                    out.push(entry.clone());
                }
            }
            out.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(out)
        }
        fn stat(&self, path: &str) -> Result<BackendEntry, BackendError> {
            self.folders
                .get(path)
                .cloned()
                .or_else(|| self.files.get(path).map(|(e, _)| e.clone()))
                .ok_or(BackendError::NotFound)
        }
        fn get_file(&self, path: &str) -> Result<Vec<u8>, BackendError> {
            self.files
                .get(path)
                .map(|(_, body)| body.clone())
                .ok_or(BackendError::NotFound)
        }
        fn put_file(&mut self, _path: &str, _bytes: &[u8]) -> Result<PutOutcome, BackendError> {
            Ok(PutOutcome::Created)
        }
        fn delete(&mut self, _path: &str) -> Result<(), BackendError> {
            Ok(())
        }
        fn mkdir(&mut self, _path: &str) -> Result<(), BackendError> {
            Ok(())
        }
    }

    fn dev_cfg() -> ServerConfig {
        ServerConfig {
            binding: ListenerBinding::LocalTcp {
                host: IpAddr::from([127, 0, 0, 1]),
                port: 0,
            },
            max_put_body_bytes: 4096,
            allow_writes: false,
        }
    }

    #[test]
    fn bind_unix_socket_returns_unsupported() {
        #[cfg(windows)]
        let socket_path = std::path::PathBuf::from(r"C:\tmp\x.sock");
        #[cfg(not(windows))]
        let socket_path = std::path::PathBuf::from("/tmp/x.sock");

        let cfg = ServerConfig {
            binding: ListenerBinding::UnixSocket { path: socket_path },
            ..dev_cfg()
        };
        cfg.validate()
            .expect("fixture must be an absolute path on the current platform");
        let err = TcpServer::bind(cfg).expect_err("UnixSocket not yet supported in T1.6.c");
        assert!(matches!(err, ServerError::UnsupportedBinding("UnixSocket")));
    }

    #[test]
    fn bind_loopback_succeeds_and_reports_addr() {
        let server = TcpServer::bind(dev_cfg()).expect("bind 127.0.0.1:0");
        let addr = server.local_addr().unwrap();
        assert!(addr.ip().is_loopback());
        assert_ne!(addr.port(), 0);
    }

    /// End-to-end smoke: open a TCP server, drive a real
    /// `PROPFIND` over `TcpStream`, parse the response on the
    /// client side, assert it is a `207 Multi-Status` carrying the
    /// expected child hrefs.
    #[test]
    fn propfind_over_tcp_round_trips() {
        let server = TcpServer::bind(dev_cfg()).expect("bind");
        let addr = server.local_addr().unwrap();

        // Start the server in a background thread; serve_one
        // returns after the connection so the join below
        // completes deterministically.
        let server_thread = thread::spawn(move || {
            let mut backend = MockBackend::default();
            backend.add_folder("/dav");
            backend.add_file("/dav/cat.jpg", b"jpegbytes");
            backend.add_file("/dav/notes.txt", b"hi");
            server.serve_one(&mut backend).expect("serve_one ok");
        });

        // Client: connect + send PROPFIND + read until EOF.
        let mut client = TcpStream::connect(addr).expect("connect");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let body = "<D:propfind xmlns:D=\"DAV:\"><D:allprop/></D:propfind>";
        let req = format!(
            "PROPFIND /dav HTTP/1.1\r\nHost: localhost\r\nDepth: 1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        client.write_all(req.as_bytes()).unwrap();

        let mut response = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = client.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                break;
            }
            response.extend_from_slice(&tmp[..n]);
            // Heuristic: stop once we see the terminator + body so
            // the test does not block waiting for the daemon to
            // close (the server handles one connection then drops
            // the stream, which closes it for us — but in case of
            // a buffer race we cap by length).
            if response.windows(4).any(|w| w == b"\r\n\r\n") && response.len() > 256 {
                break;
            }
        }

        let text = std::str::from_utf8(&response).unwrap();
        assert!(text.starts_with("HTTP/1.1 207 Multi-Status\r\n"), "{text}");
        assert!(text.contains("/dav</D:href>"));
        assert!(text.contains("/dav/cat.jpg"));
        assert!(text.contains("/dav/notes.txt"));

        server_thread.join().unwrap();
    }

    /// Server returns a 405 for unknown verbs.
    #[test]
    fn unknown_verb_over_tcp_is_405() {
        let server = TcpServer::bind(dev_cfg()).expect("bind");
        let addr = server.local_addr().unwrap();

        let server_thread = thread::spawn(move || {
            let mut backend = MockBackend::default();
            server.serve_one(&mut backend).expect("serve_one ok");
        });

        let mut client = TcpStream::connect(addr).expect("connect");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client
            .write_all(b"LOCK /dav HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();

        let mut response = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            match client.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&tmp[..n]),
                Err(_) => break,
            }
        }
        let text = std::str::from_utf8(&response).unwrap();
        assert!(text.starts_with("HTTP/1.1 405"), "{text}");
        assert!(text.to_ascii_lowercase().contains("allow:"));

        server_thread.join().unwrap();
    }

    /// Body declared larger than `max_put_body_bytes` is rejected
    /// before the dispatcher runs.
    #[test]
    fn over_cap_body_rejected_with_413() {
        let server = TcpServer::bind(dev_cfg()).expect("bind");
        let addr = server.local_addr().unwrap();

        let server_thread = thread::spawn(move || {
            let mut backend = MockBackend::default();
            server.serve_one(&mut backend).expect("serve_one ok");
        });

        let mut client = TcpStream::connect(addr).expect("connect");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        // 10 MiB declared, but cap is 4 KiB.
        let req = "PUT /dav/big HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10485760\r\n\r\n";
        client.write_all(req.as_bytes()).unwrap();

        let mut response = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            match client.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&tmp[..n]),
                Err(_) => break,
            }
        }
        let text = std::str::from_utf8(&response).unwrap();
        assert!(text.starts_with("HTTP/1.1 413"), "{text}");

        server_thread.join().unwrap();
    }
}
