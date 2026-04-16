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
    net::{TcpStream, ToSocketAddrs},
    sync::{Arc, RwLock},
    thread,
    time::{Duration, Instant},
};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use thiserror::Error;

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
/// [`Self::use_tls`] **must** be `true` in any non-test profile. The
/// production bootstrap path refuses to construct a transport with
/// `use_tls = false`; this field remains public only so that local
/// test harnesses can exercise the plaintext code path.
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
    /// If `true`, wrap the TCP stream in rustls with
    /// `webpki-roots`.
    ///
    /// Must be `true` outside of tests. The field is *not* checked
    /// here — enforcement lives in the daemon bootstrap — so this
    /// struct remains usable for local integration tests.
    pub use_tls: bool,
    /// Timeout for the initial `TcpStream::connect_timeout` call.
    pub connect_timeout: Duration,
    /// Deadline applied to each read / write syscall **and** to the
    /// overall deadline loop that drives them.
    pub read_timeout: Duration,
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
    /// use std::time::Duration;
    /// use pcloud_proto::transport::{BinaryApiTransport, TransportConfig};
    ///
    /// let transport = BinaryApiTransport::new(TransportConfig {
    ///     host: "bineapi.pcloud.com".into(),
    ///     port: 443,
    ///     server_name: "bineapi.pcloud.com".into(),
    ///     use_tls: true,
    ///     connect_timeout: Duration::from_secs(10),
    ///     read_timeout: Duration::from_secs(30),
    /// });
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
    /// # use std::time::Duration;
    /// # let transport = BinaryApiTransport::new(TransportConfig {
    /// #     host: "".into(), port: 0, server_name: "".into(), use_tls: true,
    /// #     connect_timeout: Duration::ZERO, read_timeout: Duration::ZERO,
    /// # });
    /// let req = encode_request("upload_write", &[], Some(4)).unwrap();
    /// let _ = transport.execute_with_body(&req, b"data");
    /// ```
    pub fn execute_with_body(
        &self,
        request: &EncodedRequest,
        body: &[u8],
    ) -> Result<Value, TransportError> {
        let config = self.config();
        let stream = connect_socket(&config)?;
        if config.use_tls {
            execute_tls(stream, &config, request, body)
        } else {
            execute_plain(stream, request, body)
        }
    }
}

impl ApiServerHintConsumer for BinaryApiTransport {
    fn apply_api_server_hint(&self, api_server: &str) {
        if api_server.trim().is_empty() {
            return;
        }

        let (host, port) = parse_api_server_hint(api_server);
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

fn connect_socket(config: &TransportConfig) -> Result<TcpStream, TransportError> {
    let mut addresses = (config.host.as_str(), config.port)
        .to_socket_addrs()
        .map_err(TransportError::Connect)?;
    let address = addresses
        .next()
        .ok_or_else(|| TransportError::InvalidAddress {
            host: config.host.clone(),
            port: config.port,
        })?;
    let stream = TcpStream::connect_timeout(&address, config.connect_timeout)
        .map_err(TransportError::Connect)?;
    stream
        .set_read_timeout(Some(config.read_timeout))
        .map_err(TransportError::SocketConfig)?;
    stream
        .set_write_timeout(Some(config.read_timeout))
        .map_err(TransportError::SocketConfig)?;
    Ok(stream)
}

fn execute_plain(
    mut stream: TcpStream,
    request: &EncodedRequest,
    body: &[u8],
) -> Result<Value, TransportError> {
    send_and_receive(&mut stream, request, body, Duration::from_secs(15))
}

fn execute_tls(
    stream: TcpStream,
    config: &TransportConfig,
    request: &EncodedRequest,
    body: &[u8],
) -> Result<Value, TransportError> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let tls_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_name = ServerName::try_from(config.server_name.clone())
        .map_err(|_| TransportError::InvalidServerName(config.server_name.clone()))?;
    let connection =
        ClientConnection::new(Arc::new(tls_config), server_name).map_err(TransportError::Tls)?;
    let mut tls_stream = StreamOwned::new(connection, stream);
    send_and_receive(&mut tls_stream, request, body, config.read_timeout)
}

fn send_and_receive<S>(
    stream: &mut S,
    request: &EncodedRequest,
    body: &[u8],
    timeout: Duration,
) -> Result<Value, TransportError>
where
    S: Read + Write,
{
    write_all_with_deadline(stream, &request.bytes, timeout)?;
    if !body.is_empty() {
        write_all_with_deadline(stream, body, timeout)?;
    }
    flush_with_deadline(stream, timeout)?;

    let mut header = [0u8; 4];
    read_exact_with_deadline(stream, &mut header, timeout)?;
    let frame_len = parse_response_frame_len(&header)? as usize;
    let mut body = vec![0u8; frame_len];
    read_exact_with_deadline(stream, &mut body, timeout)?;

    let mut frame = Vec::with_capacity(4 + frame_len);
    frame.extend_from_slice(&header);
    frame.extend_from_slice(&body);

    parse_response_frame(&frame, &ParseLimits::default()).map_err(TransportError::ResponseBody)
}

fn write_all_with_deadline<S>(
    stream: &mut S,
    mut buf: &[u8],
    timeout: Duration,
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
            Err(err) if is_retryable_io(&err) && Instant::now() < deadline => backoff(),
            Err(err) => return Err(TransportError::Io(err)),
        }
    }
    Ok(())
}

fn flush_with_deadline<S>(stream: &mut S, timeout: Duration) -> Result<(), TransportError>
where
    S: Write,
{
    let deadline = Instant::now() + timeout;
    loop {
        match stream.flush() {
            Ok(()) => return Ok(()),
            Err(err) if is_retryable_io(&err) && Instant::now() < deadline => backoff(),
            Err(err) => return Err(TransportError::Io(err)),
        }
    }
}

fn read_exact_with_deadline<S>(
    stream: &mut S,
    mut buf: &mut [u8],
    timeout: Duration,
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
            Err(err) if is_retryable_io(&err) && Instant::now() < deadline => backoff(),
            Err(err) => return Err(TransportError::Io(err)),
        }
    }
    Ok(())
}

fn is_retryable_io(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    )
}

fn backoff() {
    thread::sleep(Duration::from_millis(10));
}

fn parse_api_server_hint(api_server: &str) -> (String, Option<u16>) {
    let trimmed = api_server.trim();
    if let Some((host, port)) = trimmed.rsplit_once(':')
        && let Ok(port) = port.parse::<u16>()
    {
        return (host.to_owned(), Some(port));
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

        let transport = BinaryApiTransport::new(TransportConfig {
            host: address.ip().to_string(),
            port: address.port(),
            server_name: "localhost".to_owned(),
            use_tls: false,
            connect_timeout: Duration::from_secs(2),
            read_timeout: Duration::from_secs(2),
        });

        let response = transport
            .execute(&request)
            .expect("transport should succeed");
        let hash = response.as_hash().expect("response should be a hash");
        assert_eq!(hash.get_number("result"), Some(0));
        server.join().expect("server thread should finish");
    }

    #[test]
    fn api_server_hint_updates_transport_host_and_port() {
        let transport = BinaryApiTransport::new(TransportConfig {
            host: "bineapi.pcloud.com".to_owned(),
            port: 443,
            server_name: "bineapi.pcloud.com".to_owned(),
            use_tls: true,
            connect_timeout: Duration::from_secs(2),
            read_timeout: Duration::from_secs(2),
        });

        transport.apply_api_server_hint("bineapi-eu.pcloud.com:8443");

        let config = transport.config();
        assert_eq!(config.host, "bineapi-eu.pcloud.com");
        assert_eq!(config.server_name, "bineapi-eu.pcloud.com");
        assert_eq!(config.port, 8443);
    }
}
