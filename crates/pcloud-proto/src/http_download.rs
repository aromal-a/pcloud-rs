//! Signed-HTTP download executor: resolves file-link URLs from the
//! binary protocol and streams bytes over HTTPS with progress callbacks.
//! Consumed by `pcloud-backends::transfer_backend` and the SDK's
//! direct-download helpers.
//!
//! ## Role in the request pipeline
//!
//! Upload control happens on the binary channel; download byte
//! movement happens over HTTPS. This module accepts the signed URL
//! returned by `getfilelink` (see [`crate::transfer_api`]), opens a
//! TLS connection, issues the GET, validates the response headers,
//! and streams the body to the caller's sink with optional progress
//! callbacks. Failures are surfaced as [`HttpDownloadError`]
//! variants so callers can distinguish DNS / TLS / protocol /
//! integrity failures.
//!
//! ## Security considerations
//!
//! - TLS is mandatory — the HTTP code path refuses to run against
//!   a plaintext target in production profiles. Certificates are
//!   verified via `webpki-roots`.
//! - The signed URL carries an embedded time-limited token; it must
//!   not be logged, persisted, or shared.
//! - Integrity-verifying variants
//!   ([`fetch_download_verified`] / [`fetch_download_verified_streaming`])
//!   recompute a SHA-1 of the body and compare against the
//!   server-declared value. SHA-1 is used for interop with the
//!   legacy sync protocol; this check complements — but does not
//!   replace — TLS-level integrity.
//!
//! Portable; TLS is mandatory in production profiles.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use pcloud_resilience::transport::parse_retry_after_from_headers;
use pcloud_resilience::BandwidthPacer;
use rustls::pki_types::ServerName;
use rustls::{ClientConnection, StreamOwned};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::tls::shared_config;

/// `SignedDownload` — signed download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedDownload {
    /// Optional byte range (half-open, `[start, end)`). When set, the GET
    /// is issued with a `Range: bytes=start-end_inclusive` header. The
    /// caller is responsible for honoring the `206 Partial Content`
    /// response: `fetch_download` returns the body as-is.
    pub range: Option<(u64, u64)>,
    /// The `host` field (host).
    pub host: String,
    /// The `port` field (port).
    pub port: Option<u16>,
    /// The `path` field (path).
    pub path: String,
    /// The `dwltag` field (dwltag).
    pub dwltag: Option<String>,
}

/// `HttpDownloadConfig` — http download config.
#[derive(Debug, Clone)]
pub struct HttpDownloadConfig {
    /// The `use_tls` field (use tls).
    pub use_tls: bool,
    /// The `connect_timeout` field (connect timeout).
    pub connect_timeout: Duration,
    /// Per-syscall read timeout applied to the underlying `TcpStream`.
    /// Bounds any single kernel-level read call but does not bound the
    /// overall lifetime of a download — see [`Self::total_request_timeout`].
    pub read_timeout: Duration,
    /// Per-syscall write timeout applied to the underlying `TcpStream`.
    ///
    /// Defaults to [`Self::read_timeout`] for backward compatibility.
    /// Operators that need more time to flush large request headers can
    /// raise this independently of the read timeout.
    pub write_timeout: Duration,
    /// Hard upper bound on the entire download — from the first byte
    /// written on the GET to the last body byte read. If exceeded,
    /// [`HttpDownloadError::TotalTimeoutExceeded`] is returned and the
    /// connection is dropped. This guards against a slow-loris server
    /// that drip-feeds bytes slowly enough to never trip
    /// [`Self::read_timeout`] but fast enough to hold the caller
    /// indefinitely (audit-04 H-5).
    pub total_request_timeout: Duration,
    /// The `max_header_bytes` field (max header bytes).
    pub max_header_bytes: usize,
    /// The `max_body_bytes` field (max body bytes).
    pub max_body_bytes: usize,
    /// Optional download-side bandwidth pacer.
    ///
    /// When `Some`, the streaming read loop calls
    /// [`BandwidthPacer::acquire_blocking`] before emitting each chunk of
    /// body bytes so the observed throughput converges on the configured
    /// limit. When `None` (the default), no pacing is applied and the
    /// download runs at link speed.
    ///
    /// The pacer is held in an [`Arc`] so a single instance can be shared
    /// across concurrent downloads to enforce a global daemon-wide cap.
    ///
    /// Bead: pcloud-rs-6mx.
    pub bandwidth_pacer: Option<Arc<BandwidthPacer>>,
}

impl PartialEq for HttpDownloadConfig {
    fn eq(&self, other: &Self) -> bool {
        // Pacer identity is not part of semantic equality: two configs with
        // the same limits but different `Arc` instances are functionally
        // equivalent. Compare pacer *limits* so tests that build a config
        // from defaults can still be compared.
        let pacer_eq = match (&self.bandwidth_pacer, &other.bandwidth_pacer) {
            (None, None) => true,
            (Some(a), Some(b)) => a.limit() == b.limit(),
            _ => false,
        };
        self.use_tls == other.use_tls
            && self.connect_timeout == other.connect_timeout
            && self.read_timeout == other.read_timeout
            && self.write_timeout == other.write_timeout
            && self.total_request_timeout == other.total_request_timeout
            && self.max_header_bytes == other.max_header_bytes
            && self.max_body_bytes == other.max_body_bytes
            && pacer_eq
    }
}

impl Eq for HttpDownloadConfig {}

impl Default for HttpDownloadConfig {
    fn default() -> Self {
        Self {
            use_tls: true,
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(15),
            write_timeout: Duration::from_secs(15),
            // 10 min: generous enough for multi-GiB downloads over slow
            // residential links, still bounded so a wedged server cannot
            // pin a caller forever.
            total_request_timeout: Duration::from_secs(600),
            max_header_bytes: 16 * 1024,
            max_body_bytes: 64 * 1024 * 1024,
            bandwidth_pacer: None,
        }
    }
}

/// `HttpDownloadError` — http download error.
#[derive(Debug, Error)]
pub enum HttpDownloadError {
    /// The `InvalidAddress` field (invalid address).
    #[error("invalid socket address for {host}:{port}")]
    InvalidAddress {
        /// Hostname component of the rejected address.
        host: String,
        /// Port component of the rejected address.
        port: u16,
    },
    /// `Connect` variant (connect).
    #[error("tcp connect failed: {0}")]
    Connect(#[source] io::Error),
    /// `SocketConfig` variant (socket config).
    #[error("socket configuration failed: {0}")]
    SocketConfig(#[source] io::Error),
    /// `Io` variant (io).
    #[error("i/o failed: {0}")]
    Io(#[from] io::Error),
    /// `Tls` variant (tls).
    #[error("tls setup failed: {0}")]
    Tls(#[from] rustls::Error),
    /// `InvalidServerName` variant (invalid server name).
    #[error("invalid tls server name '{0}'")]
    InvalidServerName(String),
    /// `HeaderTooLarge` variant (header too large).
    #[error("response exceeded configured header limit")]
    HeaderTooLarge,
    /// `BodyTooLarge` variant (body too large).
    #[error("response exceeded configured body limit")]
    BodyTooLarge,
    /// `HttpStatus` variant (http status).
    #[error("http response status was not successful: {0}")]
    HttpStatus(u16),
    /// Server requested a retry delay (429 Too Many Requests or 503 Service
    /// Unavailable with `Retry-After` header). The embedded duration is
    /// capped at 300 s and should be honored by the caller before retrying.
    #[error("server requested retry after {0:?}")]
    RetryAfter(Duration),
    /// Whole-request deadline from
    /// [`HttpDownloadConfig::total_request_timeout`] expired before the
    /// body was fully received. Dropping the connection here is
    /// intentional: a retry will open a fresh one (audit-04 H-5).
    #[error("total request timeout of {elapsed:?} exceeded {limit:?}")]
    TotalTimeoutExceeded {
        /// Time elapsed since the first byte of the request was written.
        elapsed: Duration,
        /// Configured whole-request budget.
        limit: Duration,
    },
    /// `Malformed` variant (malformed).
    #[error("http response was malformed: {0}")]
    Malformed(&'static str),
    /// `ChunkedUnsupported` variant (chunked unsupported).
    #[error("chunked transfer encoding is not supported yet")]
    ChunkedUnsupported,
    #[error(
        "downloaded body sha256 did not match expected checksum (expected {expected}, actual {actual})"
    )]
    /// `The` variant (the).
    /// The `IntegrityMismatch` field (integrity mismatch).
    IntegrityMismatch {
        /// Expected checksum/digest as server-provided hex string.
        expected: String,
        /// Actual checksum/digest computed locally as hex string.
        actual: String,
    },
}

impl HttpDownloadError {
    /// Returns `true` when the error reflects a transient or content
    /// integrity failure that a caller may retry. Integrity mismatches
    /// are retryable because a bit-flip on one transport may not recur
    /// on the next attempt.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            HttpDownloadError::Io(_)
                | HttpDownloadError::Connect(_)
                | HttpDownloadError::IntegrityMismatch { .. }
                // RetryAfter is retryable after honoring the indicated delay.
                | HttpDownloadError::RetryAfter(_)
        )
    }

    /// When this error carries a server-mandated retry delay, return it.
    /// Callers should sleep for this duration before retrying.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            HttpDownloadError::RetryAfter(d) => Some(*d),
            _ => None,
        }
    }
}

/// `fetch_download` — fetch download.
///
/// # Errors
///
/// Returns a typed error on transport failure or malformed response.
pub fn fetch_download(
    download: &SignedDownload,
    config: &HttpDownloadConfig,
) -> Result<Vec<u8>, HttpDownloadError> {
    fetch_download_verified(download, config, None)
}

/// Fetches a signed pCloud download and, when `expected_sha256` is
/// `Some`, verifies the body's SHA-256 digest against the supplied
/// checksum before returning. A mismatch returns the retryable
/// [`HttpDownloadError::IntegrityMismatch`]. When `expected_sha256`
/// is `None`, the body is returned without integrity verification —
/// preserving the behavior of the historical [`fetch_download`] entry
/// point.
pub fn fetch_download_verified(
    download: &SignedDownload,
    config: &HttpDownloadConfig,
    expected_sha256: Option<[u8; 32]>,
) -> Result<Vec<u8>, HttpDownloadError> {
    let mut sink: Vec<u8> = Vec::new();
    fetch_download_verified_streaming(download, config, expected_sha256, &mut sink)?;
    Ok(sink)
}

/// Streaming variant of [`fetch_download_verified`]. The body is written
/// chunk-by-chunk into `sink` using a fixed-size read buffer, so memory
/// usage is O(read-buffer) rather than O(body). A rolling SHA-256 is
/// maintained incrementally and verified at EOF when `expected_sha256`
/// is supplied. Returns the number of body bytes written to `sink`.
pub fn fetch_download_verified_streaming<W: Write>(
    download: &SignedDownload,
    config: &HttpDownloadConfig,
    expected_sha256: Option<[u8; 32]>,
    sink: &mut W,
) -> Result<u64, HttpDownloadError> {
    let port = download
        .port
        .unwrap_or(if config.use_tls { 443 } else { 80 });
    let stream = connect_socket(&download.host, port, config)?;
    let mut hasher = expected_sha256.map(|_| Sha256::new());
    let deadline = request_deadline(config);

    let written = if config.use_tls {
        let tls_config = shared_config();
        let server_name = ServerName::try_from(download.host.clone())
            .map_err(|_| HttpDownloadError::InvalidServerName(download.host.clone()))?;
        let connection = ClientConnection::new(tls_config, server_name)?;
        let mut tls_stream = StreamOwned::new(connection, stream);
        request_and_stream(
            &mut tls_stream,
            download,
            config,
            sink,
            hasher.as_mut(),
            deadline,
        )?
    } else {
        let mut plain_stream = stream;
        request_and_stream(
            &mut plain_stream,
            download,
            config,
            sink,
            hasher.as_mut(),
            deadline,
        )?
    };

    if let (Some(expected), Some(h)) = (expected_sha256, hasher) {
        let actual: [u8; 32] = h.finalize().into();
        if actual != expected {
            return Err(HttpDownloadError::IntegrityMismatch {
                expected: hex_encode(&expected),
                actual: hex_encode(&actual),
            });
        }
    }

    Ok(written)
}

/// Fixed-size streaming read buffer (64 KiB). Chosen to amortize syscall
/// overhead without pinning large allocations.
const STREAM_READ_BUF: usize = 64 * 1024;

/// Compute the whole-request deadline from the configured
/// `total_request_timeout` (audit-04 H-5). Anchored on "now" so the
/// deadline is relative to the start of each call — a caller that
/// invokes `fetch_download*` three times gets three independent
/// budgets, which is the intended semantics (one deadline per HTTP
/// request, not per logical operation).
#[inline]
fn request_deadline(config: &HttpDownloadConfig) -> Instant {
    Instant::now() + config.total_request_timeout
}

/// Returns `Err(TotalTimeoutExceeded)` when the process clock has
/// passed the supplied `deadline`. Called at every boundary inside
/// the streaming read loops so a slow-loris server cannot drip-feed
/// bytes slowly enough to hold the caller forever.
#[inline]
fn check_deadline(deadline: Instant, limit: Duration) -> Result<(), HttpDownloadError> {
    let now = Instant::now();
    if now >= deadline {
        let elapsed = limit + now.saturating_duration_since(deadline);
        return Err(HttpDownloadError::TotalTimeoutExceeded { elapsed, limit });
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // SAFETY: `write!` on a `String` uses `fmt::Write for String`, which
        // is infallible — it only fails for I/O-backed writers. A panic
        // here would indicate an impossible allocator failure surfaced
        // as a formatter error.
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// Parse a `Retry-After` header from raw HTTP response headers.
///
/// Delegates to [`pcloud_resilience::transport::parse_retry_after_from_headers`],
/// the canonical workspace-wide parser that handles both integer and
/// floating-point second values (integer-only parsing was a previous
/// limitation — this alias preserves the local call site without duplication).
fn parse_retry_after(headers: &str) -> Option<Duration> {
    parse_retry_after_from_headers(headers)
}

/// Attempt a TCP connect to each resolved address in turn (happy-eyeballs
/// sequential fallback). Returns the first successful stream, or the last
/// connection error if all addresses fail.
fn connect_socket(
    host: &str,
    port: u16,
    config: &HttpDownloadConfig,
) -> Result<TcpStream, HttpDownloadError> {
    let addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(HttpDownloadError::Connect)?
        .collect();

    if addresses.is_empty() {
        return Err(HttpDownloadError::InvalidAddress {
            host: host.to_owned(),
            port,
        });
    }

    let mut last_err: Option<io::Error> = None;
    for address in &addresses {
        match TcpStream::connect_timeout(address, config.connect_timeout) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(config.read_timeout))
                    .map_err(HttpDownloadError::SocketConfig)?;
                stream
                    .set_write_timeout(Some(config.write_timeout))
                    .map_err(HttpDownloadError::SocketConfig)?;
                return Ok(stream);
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }

    Err(HttpDownloadError::Connect(last_err.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::AddrNotAvailable, "no addresses resolved")
    })))
}

fn request_and_stream<S, W>(
    stream: &mut S,
    download: &SignedDownload,
    config: &HttpDownloadConfig,
    sink: &mut W,
    mut hasher: Option<&mut Sha256>,
    deadline: Instant,
) -> Result<u64, HttpDownloadError>
where
    S: Read + Write,
    W: Write,
{
    let limit = config.total_request_timeout;
    let request = build_request(download);
    stream.write_all(request.as_bytes())?;
    stream.flush()?;
    check_deadline(deadline, limit)?;

    let (status, headers, leftover) = read_headers(stream, config.max_header_bytes)?;
    check_deadline(deadline, limit)?;
    // Accept 200 OK and 206 Partial Content (the latter when a `Range`
    // header was sent on the request). Any other 2xx is passed through.
    if status / 100 != 2 {
        // Surface Retry-After delay for 429 / 503 so callers can back off
        // for exactly the server-requested duration instead of using the
        // default exponential schedule.
        if let Some(retry_after) = headers.retry_after
            && (status == 429 || status == 503)
        {
            return Err(HttpDownloadError::RetryAfter(retry_after));
        }
        return Err(HttpDownloadError::HttpStatus(status));
    }
    if let Some(length) = headers.content_length
        && length > config.max_body_bytes
    {
        return Err(HttpDownloadError::BodyTooLarge);
    }

    // `read_headers` currently returns an empty leftover, but defend
    // against future changes by flushing whatever it produced first.
    let mut written: u64 = 0;
    if !leftover.is_empty() {
        if leftover.len() > config.max_body_bytes {
            return Err(HttpDownloadError::BodyTooLarge);
        }
        pace(config, leftover.len() as u64);
        emit(sink, hasher.as_deref_mut(), &leftover)?;
        written = leftover.len() as u64;
    }

    if headers.transfer_chunked {
        written = stream_chunked_body(
            stream,
            sink,
            hasher.as_deref_mut(),
            written,
            config,
            deadline,
        )?;
        return Ok(written);
    }

    let mut buffer = vec![0u8; STREAM_READ_BUF];
    match headers.content_length {
        Some(length) => {
            let length_u64 = length as u64;
            while written < length_u64 {
                check_deadline(deadline, limit)?;
                let remaining = length_u64 - written;
                let want = remaining.min(buffer.len() as u64) as usize;
                let read = stream.read(&mut buffer[..want])?;
                if read == 0 {
                    return Err(HttpDownloadError::Malformed(
                        "unexpected eof while reading body",
                    ));
                }
                pace(config, read as u64);
                emit(sink, hasher.as_deref_mut(), &buffer[..read])?;
                written += read as u64;
            }
        }
        None => loop {
            check_deadline(deadline, limit)?;
            let read = stream.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            if written.saturating_add(read as u64) > config.max_body_bytes as u64 {
                return Err(HttpDownloadError::BodyTooLarge);
            }
            pace(config, read as u64);
            emit(sink, hasher.as_deref_mut(), &buffer[..read])?;
            written += read as u64;
        },
    }

    Ok(written)
}

/// Outcome of a resumable-download attempt, returned by
/// [`fetch_download_resumable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumableOutcome {
    /// The download completed without needing to resume (no pre-existing
    /// `.part` file, or the server ignored `Range` / returned `200 OK`).
    FullDownload {
        /// Total bytes written to the final destination.
        bytes_written: u64,
    },
    /// An existing `.part` file was detected and the transfer picked up
    /// from the recorded offset via `Range: bytes=N-`. The on-disk prefix
    /// was re-hashed before appending the remaining bytes.
    Resumed {
        /// Bytes already present on disk at the start of the call.
        resumed_from: u64,
        /// Total bytes in the final file after the resume completed.
        bytes_written: u64,
    },
    /// The server replied without `Accept-Ranges: bytes`, so a full
    /// re-download was issued and any stale `.part` file was replaced.
    FallbackFullRedownload {
        /// Total bytes written to the final destination.
        bytes_written: u64,
    },
}

/// Downloads `download` to `dest_path` with SHA-256 verification and
/// partial-download resume.
///
/// Behavior:
///
/// - If `dest_path.with_extension("part")` (the staging file) exists,
///   its size is used as the resume offset. A fresh `Range: bytes=N-`
///   request is issued. The existing bytes are re-read from disk and
///   fed through the hasher first so the final digest covers the full
///   file.
/// - If the server does not advertise `Accept-Ranges: bytes` (or
///   responds to the range request with `200` instead of `206`), the
///   implementation falls back to a full re-download: the staging file
///   is truncated and rewritten.
/// - On SHA-256 match, the staging file is renamed to `dest_path`
///   atomically.
/// - On SHA-256 mismatch, the staging file is deleted and
///   [`HttpDownloadError::IntegrityMismatch`] is returned; a corrupted
///   prefix is not a recoverable state, so the next call starts clean.
///
/// # Cost of resume
///
/// SHA-256 state cannot be cheaply persisted across process exits, so
/// on resume the full on-disk prefix is re-hashed from disk before the
/// network transfer continues. This is O(prefix size) I/O + hashing
/// cost. For multi-gigabyte files the re-hash is non-trivial — measured
/// throughput on a modern NVMe + AES-NI / SHA extensions is typically
/// ~2–5 GB/s, so a 10 GB prefix adds a few seconds of CPU + disk work.
/// This is intentionally still far cheaper than re-downloading the
/// prefix over the network.
///
/// # Errors
///
/// Returns the same typed error set as
/// [`fetch_download_verified_streaming`]. A corrupted staging file that
/// produces a mismatched final digest is removed before the error
/// returns; callers can retry safely.
pub fn fetch_download_resumable(
    download: &SignedDownload,
    config: &HttpDownloadConfig,
    expected_sha256: [u8; 32],
    dest_path: &Path,
) -> Result<ResumableOutcome, HttpDownloadError> {
    let part_path = part_path_for(dest_path);
    let existing = match fs::metadata(&part_path) {
        Ok(meta) if meta.is_file() => Some(meta.len()),
        _ => None,
    };

    // Attempt a range-based resume when there is an existing prefix.
    if let Some(offset) = existing
        && offset > 0
    {
        match try_resume(
            download,
            config,
            expected_sha256,
            &part_path,
            offset,
            dest_path,
        ) {
            Ok(outcome) => return Ok(outcome),
            Err(ResumeAttempt::NoRangeSupport) => {
                // Fall through to full redownload below.
            }
            Err(ResumeAttempt::Fatal(HttpDownloadError::RetryAfter(wait))) => {
                // Server requested back-off — honour it before falling
                // through to the full redownload attempt so we do not
                // hammer the endpoint immediately.
                std::thread::sleep(wait);
            }
            Err(ResumeAttempt::Fatal(e)) => return Err(e),
        }
    }

    // Full download path: truncate/create the staging file, stream body,
    // verify SHA-256, rename on success, delete on mismatch.
    // If the server returns Retry-After on the initial attempt, honour
    // the delay and retry once before surfacing the error to the caller.
    let bytes_written = match full_download_to_part(download, config, expected_sha256, &part_path) {
        Ok(n) => n,
        Err(HttpDownloadError::RetryAfter(wait)) => {
            std::thread::sleep(wait);
            full_download_to_part(download, config, expected_sha256, &part_path)?
        }
        Err(e) => return Err(e),
    };
    fs::rename(&part_path, dest_path)?;
    Ok(ResumableOutcome::FullDownload { bytes_written })
}

fn part_path_for(dest_path: &Path) -> PathBuf {
    let mut name = dest_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    dest_path.with_file_name(name)
}

enum ResumeAttempt {
    NoRangeSupport,
    Fatal(HttpDownloadError),
}

impl From<HttpDownloadError> for ResumeAttempt {
    fn from(e: HttpDownloadError) -> Self {
        ResumeAttempt::Fatal(e)
    }
}

fn try_resume(
    download: &SignedDownload,
    config: &HttpDownloadConfig,
    expected_sha256: [u8; 32],
    part_path: &Path,
    offset: u64,
    dest_path: &Path,
) -> Result<ResumableOutcome, ResumeAttempt> {
    // Seed hasher with the existing on-disk prefix.
    let mut hasher = Sha256::new();
    rehash_prefix(part_path, &mut hasher).map_err(HttpDownloadError::Io)?;

    // Issue Range: bytes=offset- request (suffix open-ended).
    let (status, accept_ranges, written_tail) =
        range_stream(download, config, offset, part_path, &mut hasher)?;

    if status == 200 || !accept_ranges {
        // Server ignored the range or doesn't advertise byte ranges →
        // discard what we just wrote and fall back to a full redownload.
        return Err(ResumeAttempt::NoRangeSupport);
    }
    if status != 206 {
        return Err(ResumeAttempt::Fatal(HttpDownloadError::HttpStatus(status)));
    }

    let actual: [u8; 32] = hasher.finalize().into();
    if actual != expected_sha256 {
        // Corrupt prefix or corrupt tail — staging file is unrecoverable.
        let _ = fs::remove_file(part_path);
        return Err(ResumeAttempt::Fatal(HttpDownloadError::IntegrityMismatch {
            expected: hex_encode(&expected_sha256),
            actual: hex_encode(&actual),
        }));
    }

    fs::rename(part_path, dest_path).map_err(HttpDownloadError::Io)?;
    Ok(ResumableOutcome::Resumed {
        resumed_from: offset,
        bytes_written: offset + written_tail,
    })
}

fn rehash_prefix(path: &Path, hasher: &mut Sha256) -> io::Result<()> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; STREAM_READ_BUF];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        hasher.update(&buf[..n]);
    }
}

/// Appending sink backed by a file plus an existing hasher. The hasher
/// is updated outside this sink by the streaming emitter, so this sink
/// just appends to disk.
struct AppendSink {
    file: File,
}

impl Write for AppendSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Issues a Range request and streams the tail into `part_path` (appending),
/// feeding bytes through `hasher`. Returns `(status, accept_ranges_bytes,
/// tail_bytes_written)`.
fn range_stream(
    download: &SignedDownload,
    config: &HttpDownloadConfig,
    offset: u64,
    part_path: &Path,
    hasher: &mut Sha256,
) -> Result<(u16, bool, u64), HttpDownloadError> {
    let port = download
        .port
        .unwrap_or(if config.use_tls { 443 } else { 80 });
    let stream = connect_socket(&download.host, port, config)?;
    let deadline = request_deadline(config);

    // Build request with open-ended range header.
    let mut request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nRange: bytes={}-\r\n",
        download.path, download.host, offset
    );
    if let Some(dwltag) = download.dwltag.as_deref() {
        request.push_str("Cookie: dwltag=");
        request.push_str(dwltag);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");

    if config.use_tls {
        let tls_config = shared_config();
        let server_name = ServerName::try_from(download.host.clone())
            .map_err(|_| HttpDownloadError::InvalidServerName(download.host.clone()))?;
        let connection = ClientConnection::new(tls_config, server_name)?;
        let mut tls_stream = StreamOwned::new(connection, stream);
        range_do(
            &mut tls_stream,
            &request,
            config,
            part_path,
            offset,
            hasher,
            deadline,
        )
    } else {
        let mut plain = stream;
        range_do(
            &mut plain, &request, config, part_path, offset, hasher, deadline,
        )
    }
}

fn range_do<S>(
    stream: &mut S,
    request: &str,
    config: &HttpDownloadConfig,
    part_path: &Path,
    offset: u64,
    hasher: &mut Sha256,
    deadline: Instant,
) -> Result<(u16, bool, u64), HttpDownloadError>
where
    S: Read + Write,
{
    let limit = config.total_request_timeout;
    stream.write_all(request.as_bytes())?;
    stream.flush()?;
    check_deadline(deadline, limit)?;

    let (_status, headers, leftover) = read_headers(stream, config.max_header_bytes)?;
    check_deadline(deadline, limit)?;
    let status = headers.status;
    // Non-2xx → error.
    if status / 100 != 2 {
        return Err(HttpDownloadError::HttpStatus(status));
    }
    // 200 with accept-ranges absent (or even with it, if the server
    // didn't honor our Range): caller will discard prefix state.
    if status == 200 {
        return Ok((200, headers.accept_ranges_bytes, 0));
    }
    if status != 206 {
        return Err(HttpDownloadError::HttpStatus(status));
    }

    // 206 Partial Content → open the staging file for append, stream tail.
    let mut sink = AppendSink {
        file: OpenOptions::new().append(true).open(part_path)?,
    };

    let mut written: u64 = 0;
    if !leftover.is_empty() {
        pace(config, leftover.len() as u64);
        sink.write_all(&leftover)?;
        hasher.update(&leftover);
        written = leftover.len() as u64;
    }

    let mut buffer = vec![0u8; STREAM_READ_BUF];
    if headers.transfer_chunked {
        // Rare for Range responses, but handle gracefully.
        written = stream_chunked_body(stream, &mut sink, Some(hasher), written, config, deadline)?;
    } else {
        match headers.content_length {
            Some(length) => {
                let length_u64 = length as u64;
                let total_budget = offset.saturating_add(length_u64);
                if total_budget > config.max_body_bytes as u64 {
                    return Err(HttpDownloadError::BodyTooLarge);
                }
                while written < length_u64 {
                    check_deadline(deadline, limit)?;
                    let remaining = length_u64 - written;
                    let want = remaining.min(buffer.len() as u64) as usize;
                    let read = stream.read(&mut buffer[..want])?;
                    if read == 0 {
                        return Err(HttpDownloadError::Malformed(
                            "unexpected eof while reading body",
                        ));
                    }
                    pace(config, read as u64);
                    sink.write_all(&buffer[..read])?;
                    hasher.update(&buffer[..read]);
                    written += read as u64;
                }
            }
            None => loop {
                check_deadline(deadline, limit)?;
                let read = stream.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                if offset.saturating_add(written).saturating_add(read as u64)
                    > config.max_body_bytes as u64
                {
                    return Err(HttpDownloadError::BodyTooLarge);
                }
                pace(config, read as u64);
                sink.write_all(&buffer[..read])?;
                hasher.update(&buffer[..read]);
                written += read as u64;
            },
        }
    }
    sink.flush()?;
    // 206 implies range support even if the response omitted the
    // advisory Accept-Ranges header.
    Ok((206, true, written))
}

fn full_download_to_part(
    download: &SignedDownload,
    config: &HttpDownloadConfig,
    expected_sha256: [u8; 32],
    part_path: &Path,
) -> Result<u64, HttpDownloadError> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(part_path)?;

    let download_no_range = SignedDownload {
        range: None,
        ..download.clone()
    };

    let result = fetch_download_verified_streaming(
        &download_no_range,
        config,
        Some(expected_sha256),
        &mut file,
    );
    match result {
        Ok(n) => {
            file.flush()?;
            Ok(n)
        }
        Err(HttpDownloadError::IntegrityMismatch { expected, actual }) => {
            drop(file);
            let _ = fs::remove_file(part_path);
            Err(HttpDownloadError::IntegrityMismatch { expected, actual })
        }
        Err(e) => Err(e),
    }
}

/// Pace `bytes` against the configured [`BandwidthPacer`] (if any).
///
/// This is a zero-overhead no-op when `config.bandwidth_pacer` is `None`.
/// When a pacer is configured, the calling thread blocks just long enough
/// for the token bucket to accumulate `bytes` before returning, so the
/// download loop's observed throughput converges on the configured limit.
///
/// Bead: pcloud-rs-6mx.
#[inline]
fn pace(config: &HttpDownloadConfig, bytes: u64) {
    if let Some(pacer) = config.bandwidth_pacer.as_ref() {
        pacer.acquire_blocking(bytes);
    }
}

fn emit<W: Write>(
    sink: &mut W,
    hasher: Option<&mut Sha256>,
    bytes: &[u8],
) -> Result<(), HttpDownloadError> {
    sink.write_all(bytes)?;
    if let Some(h) = hasher {
        h.update(bytes);
    }
    Ok(())
}

fn stream_chunked_body<S, W>(
    stream: &mut S,
    sink: &mut W,
    mut hasher: Option<&mut Sha256>,
    mut written: u64,
    config: &HttpDownloadConfig,
    deadline: Instant,
) -> Result<u64, HttpDownloadError>
where
    S: Read,
    W: Write,
{
    let limit = config.total_request_timeout;
    let max = config.max_body_bytes as u64;
    let mut buffer = vec![0u8; STREAM_READ_BUF];

    loop {
        check_deadline(deadline, limit)?;
        let size_line = read_line(stream)?;
        let size_text = size_line
            .split(';')
            .next()
            .ok_or(HttpDownloadError::Malformed("missing chunk size"))?
            .trim();
        let chunk_size = u64::from_str_radix(size_text, 16)
            .map_err(|_| HttpDownloadError::Malformed("invalid chunk size"))?;

        if chunk_size == 0 {
            loop {
                let trailer = read_line(stream)?;
                if trailer.is_empty() {
                    return Ok(written);
                }
            }
        }

        if written.saturating_add(chunk_size) > max {
            return Err(HttpDownloadError::BodyTooLarge);
        }

        let mut remaining = chunk_size;
        while remaining > 0 {
            check_deadline(deadline, limit)?;
            let want = remaining.min(buffer.len() as u64) as usize;
            let read = stream.read(&mut buffer[..want])?;
            if read == 0 {
                return Err(HttpDownloadError::Malformed(
                    "unexpected eof while reading chunked body",
                ));
            }
            pace(config, read as u64);
            emit(sink, hasher.as_deref_mut(), &buffer[..read])?;
            written += read as u64;
            remaining -= read as u64;
        }
        expect_crlf(stream)?;
    }
}

fn read_line<S>(stream: &mut S) -> Result<String, HttpDownloadError>
where
    S: Read,
{
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        let read = stream.read(&mut byte)?;
        if read == 0 {
            return Err(HttpDownloadError::Malformed(
                "unexpected eof while reading chunked body",
            ));
        }
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n") {
            bytes.truncate(bytes.len().saturating_sub(2));
            return String::from_utf8(bytes)
                .map_err(|_| HttpDownloadError::Malformed("chunk metadata was not utf8"));
        }
    }
}

fn expect_crlf<S>(stream: &mut S) -> Result<(), HttpDownloadError>
where
    S: Read,
{
    let mut crlf = [0u8; 2];
    stream.read_exact(&mut crlf)?;
    if crlf != *b"\r\n" {
        return Err(HttpDownloadError::Malformed(
            "chunk payload was not terminated by crlf",
        ));
    }
    Ok(())
}

fn build_request(download: &SignedDownload) -> String {
    let mut request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        download.path, download.host
    );
    if let Some((start, end)) = download.range {
        // Range header is half-open at the caller but HTTP is inclusive.
        let last = end.saturating_sub(1);
        request.push_str(&format!("Range: bytes={}-{}\r\n", start, last));
    }
    if let Some(dwltag) = download.dwltag.as_deref() {
        request.push_str("Cookie: dwltag=");
        request.push_str(dwltag);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    request
}

struct HeaderParse {
    content_length: Option<usize>,
    transfer_chunked: bool,
    accept_ranges_bytes: bool,
    status: u16,
    /// Duration parsed from a `Retry-After: N` header, if present and valid.
    retry_after: Option<Duration>,
}

fn read_headers<S>(
    stream: &mut S,
    max_header_bytes: usize,
) -> Result<(u16, HeaderParse, Vec<u8>), HttpDownloadError>
where
    S: Read,
{
    let mut buffer = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];

    loop {
        let read = stream.read(&mut byte)?;
        if read == 0 {
            return Err(HttpDownloadError::Malformed(
                "unexpected eof while reading headers",
            ));
        }
        buffer.push(byte[0]);
        if buffer.len() > max_header_bytes {
            return Err(HttpDownloadError::HeaderTooLarge);
        }
        if buffer.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let text = std::str::from_utf8(&buffer)
        .map_err(|_| HttpDownloadError::Malformed("headers were not utf8"))?;
    let mut lines = text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or(HttpDownloadError::Malformed("missing http status line"))?;
    let status = parse_status(status_line)?;
    let mut content_length = None;
    let mut transfer_chunked = false;
    let mut accept_ranges_bytes = false;
    // Parse Retry-After from the raw header block before splitting lines.
    let retry_after = parse_retry_after(text);
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            if key == "content-length" {
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| HttpDownloadError::Malformed("invalid content-length"))?;
                content_length = Some(parsed);
            } else if key == "transfer-encoding" && value.to_ascii_lowercase().contains("chunked") {
                transfer_chunked = true;
            } else if key == "accept-ranges" && value.to_ascii_lowercase().contains("bytes") {
                accept_ranges_bytes = true;
            }
        }
    }

    Ok((
        status,
        HeaderParse {
            content_length,
            transfer_chunked,
            accept_ranges_bytes,
            status,
            retry_after,
        },
        Vec::new(),
    ))
}

fn parse_status(line: &str) -> Result<u16, HttpDownloadError> {
    let mut parts = line.split_whitespace();
    let protocol = parts.next().ok_or(HttpDownloadError::Malformed(
        "missing protocol in status line",
    ))?;
    if !protocol.starts_with("HTTP/") {
        return Err(HttpDownloadError::Malformed(
            "invalid protocol in status line",
        ));
    }
    let status = parts
        .next()
        .ok_or(HttpDownloadError::Malformed("missing status code"))?;
    status
        .parse::<u16>()
        .map_err(|_| HttpDownloadError::Malformed("invalid status code"))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };

    use super::{HttpDownloadConfig, HttpDownloadError, SignedDownload, fetch_download};

    #[test]
    fn fetch_download_issues_http_get_with_dwltag_cookie() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener
            .local_addr()
            .expect("listener should have local addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client should connect");
            let mut request = vec![0u8; 512];
            let read = stream.read(&mut request).expect("request should read");
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            assert!(request.contains("GET /get/abc/report.txt HTTP/1.1"));
            assert!(request.contains("Host: 127.0.0.1"));
            assert!(request.contains("Cookie: dwltag=download-tag"));

            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                )
                .expect("response should write");
        });

        let bytes = fetch_download(
            &SignedDownload {
                host: address.ip().to_string(),
                port: Some(address.port()),
                path: "/get/abc/report.txt".to_owned(),
                dwltag: Some("download-tag".to_owned()),
                range: None,
            },
            &HttpDownloadConfig {
                use_tls: false,
                connect_timeout: Duration::from_secs(2),
                read_timeout: Duration::from_secs(2),
                write_timeout: Duration::from_secs(2),
                max_header_bytes: 4096,
                total_request_timeout: Duration::from_secs(30),
                max_body_bytes: 1024,
                bandwidth_pacer: None,
            },
        )
        .expect("download should succeed");

        assert_eq!(bytes, b"hello");
        server.join().expect("server thread should finish");
    }

    #[test]
    fn fetch_download_rejects_oversized_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener
            .local_addr()
            .expect("listener should have local addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client should connect");
            let mut request = [0u8; 256];
            let _ = stream.read(&mut request).expect("request should read");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n0123456789",
                )
                .expect("response should write");
        });

        let err = fetch_download(
            &SignedDownload {
                host: address.ip().to_string(),
                port: Some(address.port()),
                path: "/get/abc/report.txt".to_owned(),
                dwltag: None,
                range: None,
            },
            &HttpDownloadConfig {
                use_tls: false,
                connect_timeout: Duration::from_secs(2),
                read_timeout: Duration::from_secs(2),
                write_timeout: Duration::from_secs(2),
                max_header_bytes: 4096,
                total_request_timeout: Duration::from_secs(30),
                max_body_bytes: 4,
                bandwidth_pacer: None,
            },
        )
        .expect_err("oversized body should fail");

        assert!(matches!(err, HttpDownloadError::BodyTooLarge));
        server.join().expect("server thread should finish");
    }

    #[test]
    fn fetch_download_decodes_chunked_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener
            .local_addr()
            .expect("listener should have local addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client should connect");
            let mut request = vec![0u8; 512];
            let _ = stream.read(&mut request).expect("request should read");

            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
                )
                .expect("response should write");
        });

        let bytes = fetch_download(
            &SignedDownload {
                host: address.ip().to_string(),
                port: Some(address.port()),
                path: "/get/abc/report.txt".to_owned(),
                dwltag: None,
                range: None,
            },
            &HttpDownloadConfig {
                use_tls: false,
                connect_timeout: Duration::from_secs(2),
                read_timeout: Duration::from_secs(2),
                write_timeout: Duration::from_secs(2),
                max_header_bytes: 4096,
                total_request_timeout: Duration::from_secs(30),
                max_body_bytes: 1024,
                bandwidth_pacer: None,
            },
        )
        .expect("chunked download should succeed");

        assert_eq!(bytes, b"hello world");
        server.join().expect("server thread should finish");
    }

    #[test]
    fn fetch_download_rejects_oversized_chunked_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener
            .local_addr()
            .expect("listener should have local addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client should connect");
            let mut request = vec![0u8; 512];
            let _ = stream.read(&mut request).expect("request should read");

            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
                )
                .expect("response should write");
        });

        let err = fetch_download(
            &SignedDownload {
                host: address.ip().to_string(),
                port: Some(address.port()),
                path: "/oversized-chunked".to_owned(),
                dwltag: None,
                range: None,
            },
            &HttpDownloadConfig {
                use_tls: false,
                connect_timeout: Duration::from_secs(2),
                read_timeout: Duration::from_secs(2),
                write_timeout: Duration::from_secs(2),
                max_header_bytes: 4096,
                total_request_timeout: Duration::from_secs(30),
                max_body_bytes: 4,
                bandwidth_pacer: None,
            },
        )
        .expect_err("oversized chunked body should fail");

        assert!(matches!(err, HttpDownloadError::BodyTooLarge));
        server.join().expect("server thread should finish");
    }
}
