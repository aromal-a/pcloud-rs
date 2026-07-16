//! Transport traits and type shims over `pcloud-transport`: the typed
//! request/response plumbing every API module uses. Production
//! profiles reject downgrade away from TLS.
//!
//! ## Role in the request pipeline
//!
//! Given an [`EncodedRequest`] produced by [`crate::binary_api`],
//! [`BinaryApiTransport`] opens (or, for the TLS variant, wraps) a
//! TCP connection to the configured pCloud endpoint, writes the
//! request bytes plus any out-of-band body, reads the four-byte
//! response length prefix, reads exactly that many body bytes, and
//! hands the combined frame to [`crate::parse_response_frame`]. The
//! typed [`Value`] tree is then returned to the caller.
//!
//! ## Security considerations
//!
//! - **TLS is mandatory in production.** [`TransportConfig::use_tls`]
//!   must be `true`; the daemon bootstrap enforces this and rejects
//!   any attempt to downgrade to plaintext. The plaintext code path
//!   exists only to support local integration tests.
//! - **Certificate verification** uses `rustls` + `webpki-roots`;
//!   there is no "accept any certificate" switch. An invalid server
//!   name yields [`TransportError::InvalidServerName`] with the
//!   offending value for diagnostics.
//! - **Timeouts** bound every read and write via a deadline loop so
//!   that a stuck server cannot wedge a caller indefinitely.
//! - **Retry policy** for recoverable I/O errors
//!   (`Interrupted`, `WouldBlock`) is a fixed short backoff; long
//!   retry / rate-limit behaviour lives in
//!   [`crate::resilient_transport`].
//!
//! Portable; no platform gating.

use std::{
    io::{self, Read, Write},
    net::{IpAddr, TcpStream, ToSocketAddrs},
    sync::{Arc, RwLock},
    thread,
    time::{Duration, Instant},
};

use rustls::pki_types::ServerName;
use rustls::{ClientConnection, StreamOwned};
use thiserror::Error;

use crate::tls::shared_config;
use crate::{
    EncodedRequest, FrameParseError, ResponseParseError,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    binary_api::parse_response_frame_len,
    parse_response_frame,
    response::{ParseLimits, Value},
};

/// Runtime configuration for a [`BinaryApiTransport`].
///
/// ## Lifecycle
///
/// Instantiated at daemon bootstrap from the user's configuration
/// and wrapped in the transport's internal `Arc<RwLock<_>>` so that
/// API-server hints returned by the server (e.g. via `login`'s
/// `apiserver` field) can atomically update host/port without tearing
/// down the transport. See [`ApiServerHintConsumer`].
///
/// ## Security notes
///
/// `use_tls` is a **private** field. Callers cannot construct a
/// `TransportConfig` with TLS disabled by struct-literal — the only
/// ways to obtain a config are [`Self::production`] (TLS-on, the only
/// safe choice for any deployed profile) and [`Self::dev_plaintext`]
/// (TLS-off, expressly named so any accidental production use is
/// obvious at the call site). This closes the audit-04 H-1 gap where
/// a public boolean let in-process callers silently bypass the
/// bootstrap TLS gate.
///
/// ## Keep-alive policy
///
/// The transport does **not** pool connections. Every `execute` / `execute_with_body`
/// call opens a fresh TCP (+ TLS) session. This matches the historical pCloud
/// binary-protocol client behaviour and keeps the retry/error model simple.
/// If connection setup latency becomes a bottleneck, a pooling layer should
/// be added above this struct rather than inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportConfig {
    /// DNS name or literal IP used for the TCP connect.
    ///
    /// For TLS this is also used as the default SNI value if
    /// [`Self::server_name`] is empty.
    pub host: String,
    /// TCP port (typically 443 for TLS, 8398 for plaintext).
    pub port: u16,
    /// Server name presented during the TLS handshake (SNI + peer
    /// certificate verification).
    ///
    /// Should match the CN / SAN of the server certificate.
    pub server_name: String,
    /// Private TLS flag. Set at construction via [`Self::production`]
    /// or [`Self::dev_plaintext`]; read via [`Self::use_tls`]. Kept
    /// private so that struct-literal construction cannot bypass the
    /// bootstrap TLS gate.
    use_tls: bool,
    /// Timeout for the initial `TcpStream::connect_timeout` call.
    pub connect_timeout: Duration,
    /// Deadline applied to each read syscall and to the overall deadline loop.
    pub read_timeout: Duration,
    /// Deadline applied to each write syscall.
    ///
    /// Defaults to [`Self::read_timeout`] for backward compatibility. Operators
    /// that want to allow larger upload chunks before giving up can raise this
    /// independently of the read timeout.
    pub write_timeout: Duration,
    /// Sleep duration injected between `Interrupted` / `WouldBlock` retries
    /// inside the write/read deadline loops. The default is 10 ms, which
    /// amortizes syscall overhead without burning CPU on tight-loop retries.
    /// Tests may set this to `Duration::ZERO` for instant retries.
    pub interrupt_retry_delay: Duration,
    /// Hard upper bound on the entire request/response cycle, from the first
    /// write byte to the last read byte. If the total time spent on a single
    /// `execute` call exceeds this value the call returns
    /// [`TransportError::Io`] with `ErrorKind::TimedOut`. The default is
    /// 5 minutes, which is generous enough to accommodate large uploads while
    /// still preventing an indefinitely wedged connection from blocking a
    /// caller forever.
    pub total_request_timeout: Duration,
    /// Maximum number of bytes allowed in a single framed response.
    ///
    /// Protects against a malicious or malfunctioning server sending an
    /// arbitrarily large length prefix that would cause an OOM allocation.
    /// The default is 64 MiB, which comfortably accommodates all known
    /// pCloud binary-protocol responses.
    pub max_response_bytes: usize,
}

impl TransportConfig {
    /// Default `TcpStream::connect_timeout` used by the constructors.
    pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
    /// Default per-syscall read timeout used by the constructors.
    pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);
    /// Default per-syscall write timeout used by the constructors.
    /// Matches `DEFAULT_READ_TIMEOUT` for backward compatibility.
    pub const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
    /// Default `Interrupted` / `WouldBlock` retry backoff.
    pub const DEFAULT_INTERRUPT_RETRY_DELAY: Duration = Duration::from_millis(10);
    /// Default whole-request deadline.
    pub const DEFAULT_TOTAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
    /// Default 64 MiB response-frame cap.
    pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

    /// Build a production (TLS-on) transport config.
    ///
    /// This is the only constructor appropriate for any deployed
    /// profile. `server_name` should match the host certificate's
    /// CN/SAN — pass the same value as `host` for the common case.
    #[must_use]
    pub fn production(host: impl Into<String>, port: u16, server_name: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port,
            server_name: server_name.into(),
            use_tls: true,
            connect_timeout: Self::DEFAULT_CONNECT_TIMEOUT,
            read_timeout: Self::DEFAULT_READ_TIMEOUT,
            write_timeout: Self::DEFAULT_WRITE_TIMEOUT,
            interrupt_retry_delay: Self::DEFAULT_INTERRUPT_RETRY_DELAY,
            total_request_timeout: Self::DEFAULT_TOTAL_REQUEST_TIMEOUT,
            max_response_bytes: Self::DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Build a plaintext (TLS-off) transport config for local
    /// integration tests and development-mode endpoints.
    ///
    /// The explicit `dev_plaintext` name makes any accidental use in a
    /// production context obvious at the call site. The daemon
    /// bootstrap rejects plaintext transports in production profiles
    /// (see `pcloud-config::api::ApiMode::Plaintext`).
    #[must_use]
    pub fn dev_plaintext(
        host: impl Into<String>,
        port: u16,
        server_name: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            server_name: server_name.into(),
            use_tls: false,
            connect_timeout: Self::DEFAULT_CONNECT_TIMEOUT,
            read_timeout: Self::DEFAULT_READ_TIMEOUT,
            write_timeout: Self::DEFAULT_WRITE_TIMEOUT,
            interrupt_retry_delay: Self::DEFAULT_INTERRUPT_RETRY_DELAY,
            total_request_timeout: Self::DEFAULT_TOTAL_REQUEST_TIMEOUT,
            max_response_bytes: Self::DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Construct a transport config with explicit TLS selection and
    /// caller-supplied timeouts.
    ///
    /// This is the daemon/backend hot path: the daemon has already
    /// validated the deployment profile via `pcloud-config::ApiMode`
    /// (which rejects plaintext in production), so by the time the
    /// backend calls here the `use_tls` decision is policy-correct.
    /// Keeping a single bool parameter (rather than two constructors)
    /// lets the backend switch on `ApiMode` without conditional
    /// struct-literal duplication.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn with_tls(
        use_tls: bool,
        host: impl Into<String>,
        port: u16,
        server_name: impl Into<String>,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            server_name: server_name.into(),
            use_tls,
            connect_timeout,
            read_timeout,
            write_timeout: read_timeout,
            interrupt_retry_delay: Self::DEFAULT_INTERRUPT_RETRY_DELAY,
            total_request_timeout: Self::DEFAULT_TOTAL_REQUEST_TIMEOUT,
            max_response_bytes: Self::DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Read-only accessor for the private TLS flag.
    #[must_use]
    #[inline]
    pub fn use_tls(&self) -> bool {
        self.use_tls
    }
}

/// Synchronous, TLS-capable transport for the pCloud binary
/// protocol.
///
/// ## Design choices
///
/// - **`Clone` via `Arc<RwLock<_>>`**: every clone shares the same
///   live config, so an `apply_api_server_hint` on one clone updates
///   every other clone immediately. Callers typically hold a single
///   transport handle per account and clone it into worker threads.
/// - **Synchronous I/O** is deliberate — the binary channel is
///   dominated by request latency, so async buys little and costs a
///   runtime dependency.
/// - **`&[u8]` body parameter** on [`Self::execute_with_body`] (as
///   opposed to a trait object) keeps the hot path free of dynamic
///   dispatch and allocation.
#[derive(Debug, Clone)]
pub struct BinaryApiTransport {
    config: Arc<RwLock<TransportConfig>>,
}

/// Error emitted by any [`BinaryApiTransport`] operation.
///
/// Combines connection-establishment faults, TLS handshake faults,
/// plain I/O faults, and protocol-layer faults into a single typed
/// enum so that callers can write a single `match` and surface a
/// helpful message.
///
/// The enum is not `#[non_exhaustive]` because every variant
/// corresponds to a distinct failure mode the caller should handle
/// explicitly; silent catch-alls would hide real bugs.
#[derive(Debug, Error)]
pub enum TransportError {
    /// Host / port resolved to zero socket addresses.
    ///
    /// Usually indicates a DNS failure or an empty host string.
    #[error("invalid socket address for {host}:{port}")]
    InvalidAddress {
        /// Hostname component of the rejected address.
        host: String,
        /// Port component of the rejected address.
        port: u16,
    },
    /// TCP `connect()` returned an error before the stream was
    /// established.
    ///
    /// Distinct from [`Self::Io`] so callers can retry with a
    /// different endpoint without conflating mid-request failures.
    #[error("tcp connect failed: {0}")]
    Connect(#[source] io::Error),
    /// Applying timeouts to the freshly opened socket failed.
    #[error("socket configuration failed: {0}")]
    SocketConfig(#[source] io::Error),
    /// Generic read / write / flush failure once the stream was
    /// established.
    #[error("i/o failed: {0}")]
    Io(#[from] io::Error),
    /// Rustls reported a handshake or session failure.
    ///
    /// Certificate mismatches, protocol-version mismatches, and
    /// fatal alerts all surface here.
    #[error("tls setup failed: {0}")]
    Tls(#[from] rustls::Error),
    /// Configured `server_name` was not a valid DNS name.
    #[error("invalid tls server name '{0}'")]
    InvalidServerName(String),
    /// Response length prefix was invalid (see
    /// [`FrameParseError`]).
    #[error("response header was invalid: {0}")]
    ResponseHeader(#[from] FrameParseError),
    /// Response body failed to parse (see
    /// [`ResponseParseError`]).
    #[error("response body was invalid: {0}")]
    ResponseBody(#[from] ResponseParseError),
    /// Response frame exceeds the maximum permitted size.
    ///
    /// Protects against a malicious or malfunctioning server sending an
    /// arbitrarily large length prefix that would cause an OOM allocation.
    #[error("response frame too large: {actual} bytes exceeds {limit}-byte limit")]
    ResponseTooLarge {
        /// Actual frame length as reported by the server.
        actual: usize,
        /// Configured maximum (currently 64 MiB).
        limit: usize,
    },
}

impl BinaryApiTransport {
    /// Construct a transport bound to the given configuration.
    ///
    /// The config is wrapped in an `Arc<RwLock<_>>` so every clone
    /// observes subsequent API-server hint updates.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use pcloud_proto::transport::{BinaryApiTransport, TransportConfig};
    ///
    /// let transport = BinaryApiTransport::new(
    ///     TransportConfig::production("bineapi.pcloud.com", 443, "bineapi.pcloud.com"),
    /// );
    /// ```
    #[must_use]
    pub fn new(config: TransportConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
        }
    }

    /// Return a snapshot clone of the currently-active transport
    /// configuration.
    ///
    /// Useful for logging and for UI surfaces that want to display
    /// the effective endpoint after API-server hints have been
    /// applied. The returned value is a detached clone; later
    /// updates will not be reflected.
    #[must_use]
    pub fn config(&self) -> TransportConfig {
        // SAFETY: the write-side critical section in `apply_api_server_hint`
        // only performs infallible field assignments on `TransportConfig`
        // and cannot panic while holding the lock; therefore the
        // `RwLock` can never become poisoned on this path.
        self.config
            .read()
            .expect("transport config lock should not be poisoned")
            .clone()
    }
}

impl ProtocolTransport for BinaryApiTransport {
    type Error = TransportError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        self.execute_with_body(request, &[])
    }
}

impl BinaryApiTransport {
    /// Send a framed request and attach an out-of-band body after
    /// the frame.
    ///
    /// Used by upload / checksum methods that encode a `body_len`
    /// field in the frame header and stream the blob bytes on the
    /// same socket immediately afterwards. `body` is written
    /// verbatim; the caller is responsible for matching its length
    /// against the `raw_body_len` passed to
    /// [`crate::encode_request`].
    ///
    /// # Errors
    ///
    /// - Any variant of [`TransportError`]. Connection errors leave
    ///   the socket in an unusable state; the caller must not retry
    ///   on the same transport instance without a fresh request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use pcloud_proto::binary_api::{BinaryParam, BinaryParamValue, encode_request};
    /// # use pcloud_proto::transport::{BinaryApiTransport, TransportConfig};
    /// # let transport = BinaryApiTransport::new(
    /// #     TransportConfig::production("bineapi.pcloud.com", 443, "bineapi.pcloud.com"),
    /// # );
    /// let req = encode_request("upload_write", &[], Some(4)).unwrap();
    /// let _ = transport.execute_with_body(&req, b"data");
    /// ```
    pub fn execute_with_body(
        &self,
        request: &EncodedRequest,
        body: &[u8],
    ) -> Result<Value, TransportError> {
        let config = self.config();
        if !config.use_tls() && !is_loopback_host(&config.host) {
            return Err(TransportError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "plaintext binary API transport is only permitted for loopback hosts, got '{}'",
                    config.host
                ),
            )));
        }
        let stream = connect_socket(&config)?;
        if config.use_tls() {
            execute_tls(stream, &config, request, body)
        } else {
            execute_plain(stream, &config, request, body)
        }
    }
}

impl ApiServerHintConsumer for BinaryApiTransport {
    fn apply_api_server_hint(&self, api_server: &str) {
        if api_server.trim().is_empty() {
            return;
        }

        let (host, port) = parse_api_server_hint(api_server);

        // Safety gate: only accept API-server hints that point to known-safe
        // pCloud domains. An attacker who can inject a forged hint must not
        // be able to redirect traffic to an arbitrary host.
        if !is_known_safe_host(&host) {
            // Log the rejection so operators can diagnose unexpected hint
            // sources without silently losing the steering signal.
            // audit-06 LOW transport P3-D5 / ncx.83.
            log::warn!(
                "apply_api_server_hint: rejected non-allowlisted host '{host}'; \
                 hint ignored to prevent traffic redirection"
            );
            return;
        }

        // SAFETY: the only other holder of this lock is `Self::config`,
        // which performs an infallible clone. No code path holds the
        // lock across a panic, so poisoning is unreachable.
        let mut config = self
            .config
            .write()
            .expect("transport config lock should not be poisoned");
        config.host = host.clone();
        config.server_name = host;
        if let Some(port) = port {
            config.port = port;
        }
    }
}

/// Returns `true` when the host is a known-safe pCloud API endpoint.
///
/// Delegates to the canonical implementation in
/// [`pcloud_config::api::is_known_safe_host`] which only accepts proper
/// subdomains (`*.pcloud.com`, `*.pcloud.link`). Bare apex domains and
/// literal IP addresses are rejected — the pCloud API only issues
/// subdomain hints. Test overrides (plaintext, loopback) bypass this
/// check because they never call `apply_api_server_hint`.
fn is_known_safe_host(host: &str) -> bool {
    pcloud_config::api::is_known_safe_host(host)
}

fn is_loopback_host(host: &str) -> bool {
    let trimmed = host.trim_matches(['[', ']']);
    trimmed.eq_ignore_ascii_case("localhost")
        || trimmed
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Attempt a TCP connect to each resolved address in turn, returning
/// the first successful stream (happy-eyeballs-style sequential
/// fallback). The per-address timeout is the configured
/// `connect_timeout`. On total failure the last connection error is
/// returned.
fn connect_socket(config: &TransportConfig) -> Result<TcpStream, TransportError> {
    let addresses: Vec<_> = (config.host.as_str(), config.port)
        .to_socket_addrs()
        .map_err(TransportError::Connect)?
        .collect();

    if addresses.is_empty() {
        return Err(TransportError::InvalidAddress {
            host: config.host.clone(),
            port: config.port,
        });
    }

    let mut last_err: Option<io::Error> = None;
    for address in &addresses {
        match TcpStream::connect_timeout(address, config.connect_timeout) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(config.read_timeout))
                    .map_err(TransportError::SocketConfig)?;
                stream
                    .set_write_timeout(Some(config.write_timeout))
                    .map_err(TransportError::SocketConfig)?;
                return Ok(stream);
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }

    Err(TransportError::Connect(last_err.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::AddrNotAvailable, "no addresses resolved")
    })))
}

fn execute_plain(
    mut stream: TcpStream,
    config: &TransportConfig,
    request: &EncodedRequest,
    body: &[u8],
) -> Result<Value, TransportError> {
    send_and_receive(
        &mut stream,
        request,
        body,
        config.total_request_timeout,
        config.interrupt_retry_delay,
        config.max_response_bytes,
    )
}

fn execute_tls(
    stream: TcpStream,
    config: &TransportConfig,
    request: &EncodedRequest,
    body: &[u8],
) -> Result<Value, TransportError> {
    let tls_config = shared_config();
    let server_name = ServerName::try_from(config.server_name.clone())
        .map_err(|_| TransportError::InvalidServerName(config.server_name.clone()))?;
    let connection = ClientConnection::new(tls_config, server_name).map_err(TransportError::Tls)?;
    let mut tls_stream = StreamOwned::new(connection, stream);
    send_and_receive(
        &mut tls_stream,
        request,
        body,
        config.total_request_timeout,
        config.interrupt_retry_delay,
        config.max_response_bytes,
    )
}

fn send_and_receive<S>(
    stream: &mut S,
    request: &EncodedRequest,
    body: &[u8],
    timeout: Duration,
    interrupt_delay: Duration,
    max_response_bytes: usize,
) -> Result<Value, TransportError>
where
    S: Read + Write,
{
    write_all_with_deadline(stream, request.bytes.as_slice(), timeout, interrupt_delay)?;
    if !body.is_empty() {
        write_all_with_deadline(stream, body, timeout, interrupt_delay)?;
    }
    flush_with_deadline(stream, timeout, interrupt_delay)?;

    let mut header = [0u8; 4];
    read_exact_with_deadline(stream, &mut header, timeout, interrupt_delay)?;
    let frame_len = parse_response_frame_len(&header)? as usize;
    if frame_len > max_response_bytes {
        return Err(TransportError::ResponseTooLarge {
            actual: frame_len,
            limit: max_response_bytes,
        });
    }
    let mut body = vec![0u8; frame_len];
    read_exact_with_deadline(stream, &mut body, timeout, interrupt_delay)?;

    let mut frame = Vec::with_capacity(4 + frame_len);
    frame.extend_from_slice(&header);
    frame.extend_from_slice(&body);

    parse_response_frame(&frame, &ParseLimits::default()).map_err(TransportError::ResponseBody)
}

fn write_all_with_deadline<S>(
    stream: &mut S,
    mut buf: &[u8],
    timeout: Duration,
    interrupt_delay: Duration,
) -> Result<(), TransportError>
where
    S: Write,
{
    let deadline = Instant::now() + timeout;
    while !buf.is_empty() {
        match stream.write(buf) {
            Ok(0) => {
                return Err(TransportError::Io(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write request bytes",
                )));
            }
            Ok(written) => buf = &buf[written..],
            Err(err) if is_retryable_io(&err) && Instant::now() < deadline => {
                backoff(interrupt_delay);
            }
            Err(err) => return Err(TransportError::Io(err)),
        }
    }
    Ok(())
}

fn flush_with_deadline<S>(
    stream: &mut S,
    timeout: Duration,
    interrupt_delay: Duration,
) -> Result<(), TransportError>
where
    S: Write,
{
    let deadline = Instant::now() + timeout;
    loop {
        match stream.flush() {
            Ok(()) => return Ok(()),
            Err(err) if is_retryable_io(&err) && Instant::now() < deadline => {
                backoff(interrupt_delay);
            }
            Err(err) => return Err(TransportError::Io(err)),
        }
    }
}

fn read_exact_with_deadline<S>(
    stream: &mut S,
    mut buf: &mut [u8],
    timeout: Duration,
    interrupt_delay: Duration,
) -> Result<(), TransportError>
where
    S: Read,
{
    let deadline = Instant::now() + timeout;
    while !buf.is_empty() {
        match stream.read(buf) {
            Ok(0) => {
                return Err(TransportError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "failed to fill whole buffer",
                )));
            }
            Ok(read) => {
                let (_, remainder) = buf.split_at_mut(read);
                buf = remainder;
            }
            Err(err) if is_retryable_io(&err) && Instant::now() < deadline => {
                backoff(interrupt_delay);
            }
            Err(err) => return Err(TransportError::Io(err)),
        }
    }
    Ok(())
}

/// Returns `true` for I/O errors that are safe to retry inside the
/// per-request deadline loop.
///
/// Only `Interrupted` (EINTR — system call interrupted by a signal) and
/// `WouldBlock` (EAGAIN — non-blocking socket not yet ready) qualify.
/// Errors such as `BrokenPipe`, `ConnectionReset`, `ConnectionAborted`,
/// and `TimedOut` indicate that the connection or the request is
/// irrecoverably broken and must not be silently swallowed by the inner
/// retry loop — they should surface immediately so the outer
/// `pcloud-resilience` retry layer can decide whether to re-attempt
/// with a fresh connection.
fn is_retryable_io(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    )
}

fn backoff(delay: Duration) {
    if !delay.is_zero() {
        thread::sleep(delay);
    }
}

fn parse_api_server_hint(api_server: &str) -> (String, Option<u16>) {
    let trimmed = api_server.trim();
    if let Some((host, port)) = trimmed.rsplit_once(':') {
        if let Ok(port) = port.parse::<u16>() {
            return (host.to_owned(), Some(port));
        }
    }
    (trimmed.to_owned(), None)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };

    use crate::{
        BinaryParam, BinaryParamValue,
        auth_api::{ApiServerHintConsumer, ProtocolTransport},
        encode_request,
    };

    use super::{BinaryApiTransport, TransportConfig};

    #[test]
    fn plaintext_transport_executes_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener
            .local_addr()
            .expect("listener should have local addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client should connect");
            let mut request_buf = [0u8; 128];
            let _ = stream.read(&mut request_buf).expect("request should read");

            let frame = [
                10u8, 0, 0, 0, 16, 106, b'r', b'e', b's', b'u', b'l', b't', 200, 255,
            ];
            stream.write_all(&frame).expect("response should write");
        });

        let request = encode_request(
            "noop",
            &[BinaryParam {
                name: "ping".to_owned(),
                value: BinaryParamValue::Bool(true),
            }],
            Some(0),
        )
        .expect("request should encode");

        let transport = BinaryApiTransport::new({
            let mut cfg = TransportConfig::dev_plaintext(
                address.ip().to_string(),
                address.port(),
                "localhost",
            );
            cfg.connect_timeout = Duration::from_secs(2);
            cfg.read_timeout = Duration::from_secs(2);
            cfg.total_request_timeout = Duration::from_secs(30);
            cfg
        });

        let response = transport
            .execute(&request)
            .expect("transport should succeed");
        let hash = response.as_hash().expect("response should be a hash");
        assert_eq!(hash.get_number("result"), Some(0));
        server.join().expect("server thread should finish");
    }

    #[test]
    fn plaintext_transport_rejects_non_loopback_host() {
        let request = encode_request("noop", &[], None).expect("request should encode");
        let transport = BinaryApiTransport::new(TransportConfig::dev_plaintext(
            "example.com",
            80,
            "example.com",
        ));

        let err = transport
            .execute(&request)
            .expect_err("plaintext transport to non-loopback host must fail");

        match err {
            super::TransportError::Io(io) => {
                assert_eq!(io.kind(), std::io::ErrorKind::PermissionDenied);
                assert!(io.to_string().contains("example.com"));
            }
            other => panic!("expected permission-denied Io error, got {other:?}"),
        }
    }

    #[test]
    fn api_server_hint_updates_transport_host_and_port() {
        let transport = BinaryApiTransport::new(TransportConfig::production(
            "bineapi.pcloud.com",
            443,
            "bineapi.pcloud.com",
        ));

        transport.apply_api_server_hint("bineapi-eu.pcloud.com:8443");

        let config = transport.config();
        assert_eq!(config.host, "bineapi-eu.pcloud.com");
        assert_eq!(config.server_name, "bineapi-eu.pcloud.com");
        assert_eq!(config.port, 8443);
    }

    #[test]
    fn api_server_hint_rejected_for_non_pcloud_domain() {
        let transport = BinaryApiTransport::new(TransportConfig::production(
            "bineapi.pcloud.com",
            443,
            "bineapi.pcloud.com",
        ));

        // Attacker-supplied redirect to a non-pCloud host must be silently ignored.
        transport.apply_api_server_hint("evil.attacker.example.com:443");

        let config = transport.config();
        // Host must not have changed.
        assert_eq!(config.host, "bineapi.pcloud.com");
        assert_eq!(config.port, 443);
    }

    #[test]
    fn is_known_safe_host_matches_pcloud_domains() {
        use super::is_known_safe_host;
        // Subdomains of known pCloud apex domains are accepted.
        assert!(is_known_safe_host("bineapi.pcloud.com"));
        assert!(is_known_safe_host("api.pcloud.link"));
        // Bare apex domains are NOT accepted — the pCloud API only issues
        // subdomain hints (consistent with pcloud_config::api::is_known_safe_host).
        assert!(!is_known_safe_host("pcloud.com"));
        assert!(!is_known_safe_host("pcloud.link"));
        assert!(!is_known_safe_host("evil.example.com"));
        assert!(!is_known_safe_host("notpcloud.com"));
        assert!(!is_known_safe_host("pcloud.com.evil.io"));
        assert!(!is_known_safe_host("192.168.1.1"));
    }
}
