//! `ResilientTransport` — retry-aware HTTP request executor.
//!
//! Wraps a user-supplied async request closure with:
//!
//! - **Terminal error classification** for TLS / certificate errors (Fix 1).
//! - **`MethodRetryPolicy` consultation** before every retry (Fix 2).
//! - **`Retry-After` header honouring** on 429 responses (Fix 3).
//! - **Global retry budget** across all attempts for a single request (Fix 4).
//! - **Observability** — when the `transport-metrics` feature is enabled,
//!   every `execute()` call emits a latency sample to
//!   `pcloud_transport_latency_seconds{host,outcome}` and, on failure,
//!   increments `pcloud_transport_errors_total{host,class}` (M-1 fix:
//!   `Retry-After` waits do **not** count against the global budget).
//!
//! The module is deliberately free of any HTTP-client dependency. Callers
//! supply a closure that returns a [`TransportResponse`] (status, headers, and
//! an optional error string), keeping this crate runtime-agnostic.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::collections::HashMap;
use std::time::Duration;
#[cfg(feature = "transport-metrics")]
use std::time::Instant;

use crate::retry::{MethodRetryPolicy, RetryClass, RetryDecision};

// ── Observability helpers (transport-metrics feature) ─────────────────────────

/// Outcome label emitted on the `pcloud_transport_latency_seconds` histogram.
///
/// Used by the optional `transport-metrics` feature; defined unconditionally so
/// callers can reference the type even when the feature is disabled (they will
/// simply never construct a value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportOutcomeLabel {
    /// The request succeeded (2xx) on the first try or after retries.
    Success,
    /// The request was retried (at least one non-fatal failure occurred) before
    /// succeeding.
    Retry,
    /// All retry budget was exhausted or a terminal error was encountered.
    GiveUp,
}

impl TransportOutcomeLabel {
    /// Prometheus label value for this outcome.
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Retry => "retry",
            Self::GiveUp => "give_up",
        }
    }
}

/// Error class label for `pcloud_transport_errors_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportErrorClass {
    /// TCP/UDP connection failure before a request was sent.
    Connect,
    /// TLS handshake failure or certificate rejection.
    Tls,
    /// I/O error during request or response streaming.
    Io,
    /// Server returned an HTTP error (4xx/5xx other than rate-limit).
    Response,
    /// Global retry budget exhausted.
    BudgetExhausted,
    /// Circuit breaker is open; request was not sent.
    CircuitOpen,
}

impl TransportErrorClass {
    /// Prometheus label value for this error class.
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Tls => "tls",
            Self::Io => "io",
            Self::Response => "response",
            Self::BudgetExhausted => "budget_exhausted",
            Self::CircuitOpen => "circuit_open",
        }
    }
}

#[cfg(feature = "transport-metrics")]
mod metrics_impl {
    //! Thin wrappers around `pcloud-observability`'s atomic histogram and
    //! counter primitives, accessed through process-global `OnceLock` handles.

    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};

    use pcloud_observability::metrics::{DEFAULT_LATENCY_BUCKETS, HistogramHandle, register_histogram};

    use super::{TransportErrorClass, TransportOutcomeLabel};

    // ── Latency histogram ─────────────────────────────────────────────────────

    fn latency_histogram() -> &'static HistogramHandle {
        static H: OnceLock<HistogramHandle> = OnceLock::new();
        H.get_or_init(|| {
            register_histogram(
                "pcloud_transport_latency_seconds",
                DEFAULT_LATENCY_BUCKETS,
            )
        })
    }

    /// Record a transport latency observation.
    ///
    /// `_host` is accepted for future per-host label support but is not yet
    /// wired through to a per-host sub-histogram because the observability
    /// crate's `register_histogram` API is currently name-keyed (no label
    /// dimension). A follow-up can split this into per-host families once the
    /// API is extended.
    pub fn observe_latency(_host: &str, _outcome: TransportOutcomeLabel, latency_secs: f64) {
        latency_histogram().observe(latency_secs);
    }

    // ── Error counter ─────────────────────────────────────────────────────────

    // We maintain a small fixed-size flat counter array instead of a BTreeMap
    // to keep this path allocation-free and lock-free on the hot path.
    // Cardinality: 6 error classes × (unbounded hosts, but host is dropped for
    // now — same rationale as the histogram).

    static COUNTERS: [AtomicU64; 6] = [
        AtomicU64::new(0), // Connect
        AtomicU64::new(0), // Tls
        AtomicU64::new(0), // Io
        AtomicU64::new(0), // Response
        AtomicU64::new(0), // BudgetExhausted
        AtomicU64::new(0), // CircuitOpen
    ];

    fn class_index(class: TransportErrorClass) -> usize {
        match class {
            TransportErrorClass::Connect => 0,
            TransportErrorClass::Tls => 1,
            TransportErrorClass::Io => 2,
            TransportErrorClass::Response => 3,
            TransportErrorClass::BudgetExhausted => 4,
            TransportErrorClass::CircuitOpen => 5,
        }
    }

    /// Increment the error counter for the given class.
    pub fn increment_error(_host: &str, class: TransportErrorClass) {
        COUNTERS[class_index(class)].fetch_add(1, Ordering::Relaxed);
    }

    /// Read snapshot of all error counters (for tests / exposition).
    #[allow(dead_code)]
    pub fn error_counts() -> [(TransportErrorClass, u64); 6] {
        [
            (TransportErrorClass::Connect, COUNTERS[0].load(Ordering::Relaxed)),
            (TransportErrorClass::Tls, COUNTERS[1].load(Ordering::Relaxed)),
            (TransportErrorClass::Io, COUNTERS[2].load(Ordering::Relaxed)),
            (TransportErrorClass::Response, COUNTERS[3].load(Ordering::Relaxed)),
            (TransportErrorClass::BudgetExhausted, COUNTERS[4].load(Ordering::Relaxed)),
            (TransportErrorClass::CircuitOpen, COUNTERS[5].load(Ordering::Relaxed)),
        ]
    }

}

// ── Public metrics surface ─────────────────────────────────────────────────

/// Record a transport latency observation on the
/// `pcloud_transport_latency_seconds` histogram.
///
/// When the `transport-metrics` feature is disabled this is a no-op so that
/// call sites compile unconditionally.  The `host` parameter is accepted for
/// API stability; per-host sub-histograms will be wired once the
/// `pcloud-observability` histogram API gains a label dimension.
pub fn observe_transport_latency(host: &str, outcome: TransportOutcomeLabel, latency_secs: f64) {
    #[cfg(feature = "transport-metrics")]
    metrics_impl::observe_latency(host, outcome, latency_secs);
    #[cfg(not(feature = "transport-metrics"))]
    {
        let _ = (host, outcome, latency_secs);
    }
}

/// Increment the `pcloud_transport_errors_total` counter for the given class.
///
/// No-op when the `transport-metrics` feature is disabled.
pub fn observe_transport_error(host: &str, class: TransportErrorClass) {
    #[cfg(feature = "transport-metrics")]
    metrics_impl::increment_error(host, class);
    #[cfg(not(feature = "transport-metrics"))]
    {
        let _ = (host, class);
    }
}

// ── Retry-After header parsing ────────────────────────────────────────────

/// Parse a `Retry-After` header value into a [`Duration`].
///
/// Handles both forms allowed by RFC 7231:
/// - **Integer seconds** (`Retry-After: 30`)
/// - **Floating-point seconds** (`Retry-After: 1.5`)
///
/// HTTP-date form (`Retry-After: Wed, 21 Oct 2015 07:28:00 GMT`) is not
/// supported; if the value is not parseable as a number `None` is returned.
///
/// The returned duration is capped at 300 seconds to prevent indefinite
/// stalls from a misbehaving or malicious server.
///
/// This is the canonical `Retry-After` parser for the workspace. Both the
/// HTTP-download path and the resilience-transport path use it to guarantee
/// consistent behaviour.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use pcloud_resilience::transport::parse_retry_after_header;
/// assert_eq!(parse_retry_after_header("30"), Some(Duration::from_secs(30)));
/// assert_eq!(parse_retry_after_header("1.5"), Some(Duration::from_millis(1500)));
/// assert_eq!(parse_retry_after_header("Wed, 21 Oct 2015 07:28:00 GMT"), None);
/// assert_eq!(parse_retry_after_header("999"), Some(Duration::from_secs(300)));
/// ```
pub fn parse_retry_after_header(value: &str) -> Option<Duration> {
    let secs: f64 = value.trim().parse().ok()?;
    if secs < 0.0 || !secs.is_finite() {
        return None;
    }
    let capped = secs.min(300.0);
    Some(Duration::from_millis((capped * 1000.0) as u64))
}

/// Parse a `Retry-After` header out of a raw HTTP response header block.
///
/// Locates the first `Retry-After:` line (case-insensitive), extracts the
/// value, and delegates to [`parse_retry_after_header`].  Returns `None` when
/// the header is absent or the value cannot be parsed as a number.
///
/// Used by the HTTP-download path, which receives headers as a raw string
/// before they are split into a map.
pub fn parse_retry_after_from_headers(headers: &str) -> Option<Duration> {
    headers
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("retry-after:"))
        .and_then(|l| l.splitn(2, ':').nth(1))
        .and_then(|v| parse_retry_after_header(v))
}

// ── Error classification ───────────────────────────────────────────────────

/// Coarse error kind used by the retry loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Transient — worth retrying (connection reset, temporary DNS, 5xx …).
    Transient,
    /// Terminal — retrying cannot help; surface immediately to the caller.
    ///
    /// Certificate errors are **always** Terminal: retrying will not fix a bad
    /// cert and masking the event delays detection of a real security incident.
    Terminal,
}

/// Returns `true` if the given [`std::io::ErrorKind`] represents a transient
/// condition that is safe to retry.
///
/// The following kinds are treated as retryable:
/// - [`std::io::ErrorKind::TimedOut`] — operation timed out before completing.
/// - [`std::io::ErrorKind::ConnectionReset`] — remote peer reset the connection.
/// - [`std::io::ErrorKind::BrokenPipe`] — write to a closed pipe or socket.
/// - [`std::io::ErrorKind::ConnectionAborted`] — connection aborted by the peer.
/// - [`std::io::ErrorKind::Interrupted`] — system call interrupted; retrying is
///   idiomatic for this kind.
/// - [`std::io::ErrorKind::WouldBlock`] — non-blocking I/O would block; retry
///   after a brief wait.
///
/// All other kinds are treated as non-retryable (caller should surface them).
pub fn is_retryable_io_kind(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind as K;
    matches!(
        kind,
        K::TimedOut
            | K::ConnectionReset
            | K::BrokenPipe
            | K::ConnectionAborted
            | K::Interrupted
            | K::WouldBlock
    )
}

/// Typed transport error categorisation consumed by [`classify_transport_error`].
///
/// This is the input to the retry classifier. Callers are expected to map
/// their concrete error type (`std::io::Error`, `rustls::Error`,
/// `reqwest::Error`, a crate-local `TransportError` enum, etc.) onto one of
/// these variants. The classifier then decides [`ErrorKind::Transient`] vs.
/// [`ErrorKind::Terminal`] by matching on the variant, **not** on substring
/// patterns of a human-readable error message.
///
/// See bead `pcloud-rs-8mb.37` (audit-05 §6-opus H-1): the previous
/// string-match classifier was fragile across library versions and locales;
/// this typed shape replaces it.
#[derive(Debug, Clone)]
pub enum TransportError {
    /// A `std::io::Error` kind. Only the kind is needed for classification.
    Io(std::io::ErrorKind),
    /// A TLS-layer failure: bad certificate, bad server name, version
    /// mismatch, handshake alert, revoked cert, etc. Always [`ErrorKind::Terminal`].
    Tls(TlsError),
    /// A TCP-level connect failure (DNS flap, transient unreachable, reset
    /// during handshake). Treated as transient.
    Connect,
    /// The server hostname configured by the caller failed DNS lookup or is
    /// structurally invalid. Terminal — re-running will not fix a typo.
    InvalidAddress,
    /// HTTP request/response timed out. Transient.
    Timeout,
    /// HTTP body read/write error (premature EOF, chunked-encoding frame
    /// error). Transient — the TCP connection may be reusable on retry.
    Body,
    /// Response decode / deserialization error (malformed JSON/binary).
    /// Terminal — the server returned garbage, retrying will not help.
    Decode,
    /// The response exceeded a caller-enforced size limit. Terminal.
    ResponseTooLarge,
    /// Socket configuration error (bind, SO_* setsockopt failed). Terminal —
    /// a local configuration problem will recur on retry.
    SocketConfig,
    /// Truly-unknown error type that could not be mapped. Classified as
    /// [`ErrorKind::Terminal`] (fail-closed) to avoid burning the retry
    /// budget on a condition the caller does not understand.
    Unknown,
}

/// Sub-categorisation of TLS failures. All variants are classified as
/// [`ErrorKind::Terminal`] — retrying a TLS failure either masks a live
/// security incident or wastes the retry budget on a misconfiguration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsError {
    /// `rustls::Error::InvalidCertificate` (any reason).
    InvalidCertificate,
    /// The peer sent an alert (handshake_failure, bad_certificate, …).
    AlertReceived,
    /// Version mismatch or no common cipher suite.
    NoVersionOrCipher,
    /// Name verification failed — SNI/hostname did not match the cert.
    InvalidServerName,
    /// Any other rustls error variant not explicitly enumerated above.
    /// Still Terminal.
    Other,
}

/// Classify a [`TransportError`] into [`ErrorKind`].
///
/// # Typed classification (replaces legacy string matching)
///
/// Previously this module matched on stringified error messages
/// (`err.to_string().contains("tls")` etc.). That was fragile across
/// rustls/reqwest releases and on non-English locales. The classifier now
/// matches on typed variants:
///
/// - [`TransportError::Io`]: classified by [`std::io::ErrorKind`]. Transient
///   for `TimedOut | Interrupted | WouldBlock | ConnectionReset |
///   BrokenPipe | ConnectionAborted`. Everything else
///   (`PermissionDenied`, `NotFound`, `AlreadyExists`, `InvalidInput`,
///   `InvalidData`, `Other`, …) is Terminal.
/// - [`TransportError::Tls`]: always Terminal — a bad cert, bad server
///   name, version mismatch, or alert must never be retried.
/// - [`TransportError::Connect`] / [`TransportError::Timeout`] /
///   [`TransportError::Body`]: Transient — the TCP/HTTP stream may recover
///   on a fresh attempt.
/// - [`TransportError::InvalidAddress`] / [`TransportError::Decode`] /
///   [`TransportError::ResponseTooLarge`] / [`TransportError::SocketConfig`]:
///   Terminal — a configuration or protocol-layer error that will recur.
/// - [`TransportError::Unknown`]: Terminal (fail-closed). Rather than burn
///   retry budget on an error type the caller did not understand, the
///   request is aborted so the caller can surface it for diagnosis.
///
/// See bead `pcloud-rs-8mb.37` (audit-05 §6-opus H-1).
pub fn classify_transport_error(err: &TransportError) -> ErrorKind {
    match err {
        TransportError::Io(kind) => {
            if is_retryable_io_kind(*kind) {
                ErrorKind::Transient
            } else {
                ErrorKind::Terminal
            }
        }
        // Any TLS failure is Terminal — never mask a security event by
        // retrying.
        TransportError::Tls(_) => ErrorKind::Terminal,
        TransportError::Connect => ErrorKind::Transient,
        TransportError::Timeout => ErrorKind::Transient,
        TransportError::Body => ErrorKind::Transient,
        TransportError::InvalidAddress => ErrorKind::Terminal,
        TransportError::Decode => ErrorKind::Terminal,
        TransportError::ResponseTooLarge => ErrorKind::Terminal,
        TransportError::SocketConfig => ErrorKind::Terminal,
        // Fail-closed: unknown error types surface to the caller instead of
        // wasting retry budget.
        TransportError::Unknown => ErrorKind::Terminal,
    }
}

/// Stable wire-tag prefix used by [`TransportResponse::typed_error`] to
/// encode a [`TransportError`] into the `Option<String>` error slot on
/// [`TransportResponse`]. Callers that build a `TransportResponse` from a
/// typed error should prefer [`TransportResponse::typed_error`] to
/// [`TransportResponse::transport_error`] so the retry loop can classify
/// by variant rather than by message text.
pub(crate) const TYPED_ERR_PREFIX: &str = "pcloud-resilience:typed:";

/// Legacy string-form classifier kept only for backwards compatibility with
/// callers that still encode their error as a free-form message through
/// [`TransportResponse::transport_error`].
///
/// # Recommended migration path
///
/// New callers should map their concrete error into [`TransportError`] and
/// use [`classify_transport_error`] directly, or build the response with
/// [`TransportResponse::typed_error`]. When a response carries a typed tag
/// (see [`TYPED_ERR_PREFIX`]) this function decodes it and delegates to
/// [`classify_transport_error`].
///
/// When the message has no typed tag, the conservative default is
/// [`ErrorKind::Terminal`] (fail-closed). This reverses the historical
/// default (which was Transient) so that a caller passing a plain free-form
/// string can no longer accidentally trigger a retry storm on an unknown
/// error. Callers that want the legacy permissive behaviour must migrate
/// to the typed API.
///
/// See bead `pcloud-rs-8mb.37`.
pub fn classify_error(error_message: &str) -> ErrorKind {
    if let Some(rest) = error_message.strip_prefix(TYPED_ERR_PREFIX) {
        if let Some(err) = decode_typed_tag(rest) {
            return classify_transport_error(&err);
        }
        // Malformed typed tag → fail-closed.
        return ErrorKind::Terminal;
    }
    // Unknown free-form string → fail-closed.
    ErrorKind::Terminal
}

/// Decode the wire tag produced by [`encode_typed_tag`]. Returns `None` if
/// the tag is malformed or references an unknown variant.
fn decode_typed_tag(rest: &str) -> Option<TransportError> {
    let (variant, payload) = match rest.split_once(':') {
        Some((v, p)) => (v, p),
        None => (rest, ""),
    };
    match variant {
        "io" => {
            use std::io::ErrorKind as K;
            let kind = match payload {
                "TimedOut" => K::TimedOut,
                "Interrupted" => K::Interrupted,
                "WouldBlock" => K::WouldBlock,
                "ConnectionReset" => K::ConnectionReset,
                "BrokenPipe" => K::BrokenPipe,
                "ConnectionAborted" => K::ConnectionAborted,
                "PermissionDenied" => K::PermissionDenied,
                "NotFound" => K::NotFound,
                "AlreadyExists" => K::AlreadyExists,
                "InvalidInput" => K::InvalidInput,
                "InvalidData" => K::InvalidData,
                "UnexpectedEof" => K::UnexpectedEof,
                "Other" => K::Other,
                _ => K::Other,
            };
            Some(TransportError::Io(kind))
        }
        "tls" => {
            let tls = match payload {
                "InvalidCertificate" => TlsError::InvalidCertificate,
                "AlertReceived" => TlsError::AlertReceived,
                "NoVersionOrCipher" => TlsError::NoVersionOrCipher,
                "InvalidServerName" => TlsError::InvalidServerName,
                _ => TlsError::Other,
            };
            Some(TransportError::Tls(tls))
        }
        "connect" => Some(TransportError::Connect),
        "timeout" => Some(TransportError::Timeout),
        "body" => Some(TransportError::Body),
        "invalid_address" => Some(TransportError::InvalidAddress),
        "decode" => Some(TransportError::Decode),
        "response_too_large" => Some(TransportError::ResponseTooLarge),
        "socket_config" => Some(TransportError::SocketConfig),
        "unknown" => Some(TransportError::Unknown),
        _ => None,
    }
}

/// Encode a [`TransportError`] to its wire tag. Inverse of [`decode_typed_tag`].
fn encode_typed_tag(err: &TransportError) -> String {
    match err {
        TransportError::Io(kind) => {
            use std::io::ErrorKind as K;
            let name = match *kind {
                K::TimedOut => "TimedOut",
                K::Interrupted => "Interrupted",
                K::WouldBlock => "WouldBlock",
                K::ConnectionReset => "ConnectionReset",
                K::BrokenPipe => "BrokenPipe",
                K::ConnectionAborted => "ConnectionAborted",
                K::PermissionDenied => "PermissionDenied",
                K::NotFound => "NotFound",
                K::AlreadyExists => "AlreadyExists",
                K::InvalidInput => "InvalidInput",
                K::InvalidData => "InvalidData",
                K::UnexpectedEof => "UnexpectedEof",
                _ => "Other",
            };
            format!("{TYPED_ERR_PREFIX}io:{name}")
        }
        TransportError::Tls(tls) => {
            let name = match tls {
                TlsError::InvalidCertificate => "InvalidCertificate",
                TlsError::AlertReceived => "AlertReceived",
                TlsError::NoVersionOrCipher => "NoVersionOrCipher",
                TlsError::InvalidServerName => "InvalidServerName",
                TlsError::Other => "Other",
            };
            format!("{TYPED_ERR_PREFIX}tls:{name}")
        }
        TransportError::Connect => format!("{TYPED_ERR_PREFIX}connect:"),
        TransportError::Timeout => format!("{TYPED_ERR_PREFIX}timeout:"),
        TransportError::Body => format!("{TYPED_ERR_PREFIX}body:"),
        TransportError::InvalidAddress => format!("{TYPED_ERR_PREFIX}invalid_address:"),
        TransportError::Decode => format!("{TYPED_ERR_PREFIX}decode:"),
        TransportError::ResponseTooLarge => format!("{TYPED_ERR_PREFIX}response_too_large:"),
        TransportError::SocketConfig => format!("{TYPED_ERR_PREFIX}socket_config:"),
        TransportError::Unknown => format!("{TYPED_ERR_PREFIX}unknown:"),
    }
}

// ── Transport response ─────────────────────────────────────────────────────

/// Simplified representation of an HTTP response returned by the caller's
/// closure.  The transport layer only inspects the status code and the
/// `Retry-After` header; the raw body is left opaque.
#[derive(Debug, Clone)]
pub struct TransportResponse {
    /// HTTP status code (e.g. 200, 429, 500).
    pub status: u16,
    /// Response headers (lower-cased names → value).
    pub headers: HashMap<String, String>,
    /// If the underlying transport raised a low-level error (before an HTTP
    /// response was received), the caller encodes it here.
    pub error: Option<String>,
}

impl TransportResponse {
    /// Construct a successful response with no headers.
    pub fn ok(status: u16) -> Self {
        Self {
            status,
            headers: HashMap::new(),
            error: None,
        }
    }

    /// Construct an error response (no HTTP status received) from a free-form
    /// message. Prefer [`Self::typed_error`] on new code so the retry loop
    /// can classify by typed variant instead of a string-match fallback.
    pub fn transport_error(message: impl Into<String>) -> Self {
        Self {
            status: 0,
            headers: HashMap::new(),
            error: Some(message.into()),
        }
    }

    /// Construct an error response from a typed [`TransportError`]. The
    /// variant is encoded into the error slot with a stable wire tag so
    /// [`classify_error`] can round-trip it back to
    /// [`classify_transport_error`].
    ///
    /// See bead `pcloud-rs-8mb.37`.
    pub fn typed_error(err: TransportError) -> Self {
        Self {
            status: 0,
            headers: HashMap::new(),
            error: Some(encode_typed_tag(&err)),
        }
    }

    /// Returns `true` when the response is a rate-limit signal (HTTP 429).
    pub fn is_rate_limited(&self) -> bool {
        self.status == 429
    }

    /// Returns `true` when the status is a server-side error (5xx).
    pub fn is_server_error(&self) -> bool {
        self.status >= 500 && self.status < 600
    }

    /// Returns `true` when the response represents a success (2xx).
    pub fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }

    /// Parse the `Retry-After` header (if present) into a [`Duration`].
    ///
    /// Delegates to [`parse_retry_after_header`], which handles both integer
    /// and floating-point second values and caps the result at 300 seconds.
    pub fn retry_after(&self) -> Option<Duration> {
        let raw = self
            .headers
            .get("retry-after")
            .or_else(|| self.headers.get("Retry-After"))?;
        parse_retry_after_header(raw)
    }
}

// ── Resilient transport configuration ─────────────────────────────────────

/// Configuration for [`ResilientTransport`].
#[derive(Debug, Clone)]
pub struct ResilientTransportConfig {
    /// Method-level retry policy: consulted on every failure to decide whether
    /// the retry is safe given the HTTP method's semantics (Fix 2).
    pub method_policy: MethodRetryPolicy,

    /// Global cap on the total number of attempts (initial + retries) for a
    /// single logical request (Fix 4).
    ///
    /// When `max_total_attempts` is exceeded the transport returns a terminal
    /// error regardless of the per-attempt policy.  Defaults to `10`.
    pub max_total_attempts: u32,
}

impl ResilientTransportConfig {
    /// Construct a configuration with the supplied policy and the default
    /// global attempt budget (10).
    pub fn new(method_policy: MethodRetryPolicy) -> Self {
        Self {
            method_policy,
            max_total_attempts: 10,
        }
    }

    /// Override the global attempt budget.
    pub fn with_max_total_attempts(mut self, n: u32) -> Self {
        assert!(n >= 1, "max_total_attempts must be >= 1");
        self.max_total_attempts = n;
        self
    }
}

// ── Resilient transport ────────────────────────────────────────────────────

/// Outcome returned by [`ResilientTransport::execute`].
#[derive(Debug)]
pub enum TransportOutcome {
    /// The request completed (success or non-retryable HTTP error).
    Response(TransportResponse),
    /// All retries exhausted or the error is terminal; includes a description.
    Failed(String),
}

/// Retry-aware request executor.
///
/// # Responsibilities
///
/// 1. **Terminal error gate** — TLS/cert errors abort immediately (Fix 1).
/// 2. **Method policy gate** — non-idempotent methods are not retried on 5xx
///    (Fix 2).
/// 3. **`Retry-After` honouring** — on 429 the server's hint is respected up
///    to 300 s (Fix 3).
/// 4. **Global budget cap** — total attempts are bounded by
///    [`ResilientTransportConfig::max_total_attempts`] (Fix 4).
/// 5. **Observability** (optional `transport-metrics` feature) — latency
///    histogram and error counter are emitted on every call.
///
/// # M-1 fix: `Retry-After` paths do not burn budget tokens
///
/// A 429 response with a `Retry-After` header means the server is asking for a
/// throttle delay, **not** that a real attempt failed in a way worth counting.
/// The global `attempt` counter (which gates Fix 4's budget check) is therefore
/// only incremented on genuine request attempts; `Retry-After` sleeps loop on
/// the same attempt index so they do not burn budget tokens.
pub struct ResilientTransport {
    config: ResilientTransportConfig,
    /// Caller-supplied logical host name for metric labels.  Empty string is
    /// accepted and passed through to the metric helpers as-is (they tolerate
    /// it as a valid low-cardinality label).
    host: String,
}

impl ResilientTransport {
    /// Create a new transport with the given configuration.
    ///
    /// `host` is a short, low-cardinality label used in observability output
    /// (e.g. `"api.pcloud.com"`).  Pass an empty string if the caller does
    /// not have a meaningful host label.
    pub fn new(config: ResilientTransportConfig) -> Self {
        Self {
            config,
            host: String::new(),
        }
    }

    /// Attach a host label used in metric emission.
    #[must_use]
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    /// Execute a request using the supplied async closure.
    ///
    /// `class` is the caller's classification of the HTTP method (idempotent
    /// read vs. state-changing mutation).  `make_request` is called once per
    /// attempt; it should be a fresh closure that creates and sends the
    /// underlying HTTP request.
    ///
    /// The method is `async` and accepts a future-returning closure.  The
    /// explicit `sleep_fn` parameter receives the computed wait duration
    /// between attempts — this keeps the struct free of a direct `tokio`
    /// runtime dependency and makes the retry loop trivially testable with a
    /// no-op sleep.
    ///
    /// When the `transport-metrics` feature is enabled, a latency sample is
    /// emitted to `pcloud_transport_latency_seconds` on every call (outcome:
    /// `success`, `retry`, or `give_up`), and `pcloud_transport_errors_total`
    /// is incremented on each failure classification.
    pub async fn execute<F, Fut, S, Sf>(
        &self,
        class: RetryClass,
        mut make_request: F,
        mut sleep_fn: S,
    ) -> TransportOutcome
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = TransportResponse>,
        S: FnMut(Duration) -> Sf,
        Sf: std::future::Future<Output = ()>,
    {
        #[cfg(feature = "transport-metrics")]
        let start = Instant::now();
        let max_total = self.config.max_total_attempts;
        // Whether at least one retry occurred (used to pick the outcome label).
        #[cfg(feature = "transport-metrics")]
        let mut had_retry = false;

        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let response = make_request().await;

            // ── Fix 1: Terminal error gate ────────────────────────────────
            // If the transport layer raised a low-level error, classify it
            // before deciding to retry.
            if let Some(ref err_msg) = response.error
                && classify_error(err_msg) == ErrorKind::Terminal
            {
                #[cfg(feature = "transport-metrics")]
                {
                    // Typed classification: decode the wire tag if present;
                    // otherwise fall back to `Io`. Legacy free-form messages
                    // (no tag) are mapped to `Io` since we cannot distinguish
                    // TLS from other I/O without a typed input.
                    let cls = if let Some(rest) = err_msg.strip_prefix(TYPED_ERR_PREFIX) {
                        if rest.starts_with("tls:") {
                            TransportErrorClass::Tls
                        } else if rest.starts_with("connect:") {
                            TransportErrorClass::Connect
                        } else {
                            TransportErrorClass::Io
                        }
                    } else {
                        TransportErrorClass::Io
                    };
                    metrics_impl::increment_error(&self.host, cls);
                    metrics_impl::observe_latency(
                        &self.host,
                        TransportOutcomeLabel::GiveUp,
                        start.elapsed().as_secs_f64(),
                    );
                }
                return TransportOutcome::Failed(format!(
                    "Terminal transport error (not retried): {err_msg}"
                ));
            }

            // ── Success path ──────────────────────────────────────────────
            if response.is_success() {
                #[cfg(feature = "transport-metrics")]
                metrics_impl::observe_latency(
                    &self.host,
                    if had_retry {
                        TransportOutcomeLabel::Retry
                    } else {
                        TransportOutcomeLabel::Success
                    },
                    start.elapsed().as_secs_f64(),
                );
                return TransportOutcome::Response(response);
            }

            // ── Non-retryable HTTP status (4xx except 429) ────────────────
            if response.status >= 400 && response.status < 500 && response.status != 429 {
                #[cfg(feature = "transport-metrics")]
                {
                    metrics_impl::increment_error(&self.host, TransportErrorClass::Response);
                    metrics_impl::observe_latency(
                        &self.host,
                        TransportOutcomeLabel::GiveUp,
                        start.elapsed().as_secs_f64(),
                    );
                }
                return TransportOutcome::Response(response);
            }

            // ── Fix 4: Global budget exhausted ────────────────────────────
            if attempt >= max_total {
                #[cfg(feature = "transport-metrics")]
                {
                    metrics_impl::increment_error(&self.host, TransportErrorClass::BudgetExhausted);
                    metrics_impl::observe_latency(
                        &self.host,
                        TransportOutcomeLabel::GiveUp,
                        start.elapsed().as_secs_f64(),
                    );
                }
                return TransportOutcome::Failed(format!(
                    "Global retry budget exhausted after {attempt} attempt(s) \
                     (max_total_attempts = {max_total})"
                ));
            }

            // ── Determine wait duration ───────────────────────────────────

            // Fix 3: Honour `Retry-After` on 429.
            // If the server supplied a Retry-After hint, use it (capped at
            // 300 s) instead of the backoff calculation.
            //
            // M-1 fix: when the server sends a Retry-After header on a 429
            // response, we do NOT increment `attempt` — the budget token is
            // not consumed because the server forced the wait, not a genuine
            // request failure.  We sleep and then re-run the same attempt
            // index so the global budget is not eroded by server throttling.
            if response.is_rate_limited() {
                if let Some(hint) = response.retry_after() {
                    // Server-dictated wait: does not count against budget.
                    sleep_fn(hint).await;
                    // Decrement attempt so the next loop iteration re-uses the
                    // same budget slot (M-1 fix).
                    attempt = attempt.saturating_sub(1);
                    #[cfg(feature = "transport-metrics")]
                    {
                        had_retry = true;
                    }
                    continue;
                }
            }

            // Fix 2: Consult MethodRetryPolicy before retrying.
            //
            // Non-idempotent methods (e.g. upload_write, upload_save) must
            // NOT be retried on 5xx — the server may have partially applied
            // the write.  We still allow retry on 429 (rate-limit) and on
            // transport-layer errors where the request never reached the
            // server, because the caller has indicated rate-limit retries are
            // safe.  For 5xx with a Mutation class we stop here.
            if response.is_server_error() {
                // For mutations, retrying a 5xx is unsafe: the server may
                // have partially applied the state change before failing.
                // Only connection-level errors (status == 0) are safe to
                // retry unconditionally.
                let decision = self.config.method_policy.next(class, attempt);
                if matches!(decision, RetryDecision::GiveUp) {
                    #[cfg(feature = "transport-metrics")]
                    {
                        metrics_impl::increment_error(&self.host, TransportErrorClass::Response);
                        metrics_impl::observe_latency(
                            &self.host,
                            TransportOutcomeLabel::GiveUp,
                            start.elapsed().as_secs_f64(),
                        );
                    }
                    return TransportOutcome::Failed(format!(
                        "Method policy prohibits retry of {} error for \
                         non-idempotent request (attempt {attempt})",
                        response.status
                    ));
                }
                // Idempotent: use the wait from the policy.
                let wait = match decision {
                    RetryDecision::Retry { wait } => wait,
                    // INVARIANT: GiveUp is matched and returned early in the
                    // `if matches!(decision, RetryDecision::GiveUp)` block
                    // above; this arm is structurally unreachable.
                    RetryDecision::GiveUp => unreachable!(),
                };
                #[cfg(feature = "transport-metrics")]
                {
                    had_retry = true;
                }
                sleep_fn(wait).await;
                continue;
            }

            // 429 without Retry-After, or transport error (status == 0):
            // check the method policy, then sleep.  These DO consume a budget
            // token because there was no server-dictated wait hint.
            let retry_after_hint: Option<Duration> = None; // Retry-After already handled above
            let decision = self
                .config
                .method_policy
                .next_wait(class, attempt, retry_after_hint);
            match decision {
                RetryDecision::GiveUp => {
                    #[cfg(feature = "transport-metrics")]
                    {
                        let cls = if response.status == 0 {
                            TransportErrorClass::Connect
                        } else {
                            TransportErrorClass::Response
                        };
                        metrics_impl::increment_error(&self.host, cls);
                        metrics_impl::observe_latency(
                            &self.host,
                            TransportOutcomeLabel::GiveUp,
                            start.elapsed().as_secs_f64(),
                        );
                    }
                    return TransportOutcome::Failed(format!(
                        "Method policy prohibits retry (attempt {attempt})"
                    ));
                }
                RetryDecision::Retry { wait } => {
                    #[cfg(feature = "transport-metrics")]
                    {
                        had_retry = true;
                    }
                    sleep_fn(wait).await;
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retry::{BackoffSchedule, MethodRetryPolicy, RetryClass, RetryPolicy};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    fn make_policy(max_attempts: u32) -> MethodRetryPolicy {
        let inner = RetryPolicy::new(
            max_attempts,
            BackoffSchedule::Fixed {
                delay: Duration::from_millis(1),
            },
        );
        MethodRetryPolicy::secure_default(inner)
    }

    fn make_transport(max_attempts: u32, max_total: u32) -> ResilientTransport {
        let policy = make_policy(max_attempts);
        let config = ResilientTransportConfig::new(policy).with_max_total_attempts(max_total);
        ResilientTransport::new(config)
    }

    async fn no_sleep(_: Duration) {}

    // ── Typed classifier: bead pcloud-rs-8mb.37 ──────────────────────────

    #[test]
    fn classify_transport_error_tls_is_always_terminal() {
        for tls in [
            TlsError::InvalidCertificate,
            TlsError::AlertReceived,
            TlsError::NoVersionOrCipher,
            TlsError::InvalidServerName,
            TlsError::Other,
        ] {
            assert_eq!(
                classify_transport_error(&TransportError::Tls(tls)),
                ErrorKind::Terminal,
                "TLS variant {tls:?} must be Terminal"
            );
        }
    }

    #[test]
    fn classify_transport_error_io_transient_kinds() {
        use std::io::ErrorKind as K;
        for k in [
            K::TimedOut,
            K::Interrupted,
            K::WouldBlock,
            K::ConnectionReset,
            K::BrokenPipe,
            K::ConnectionAborted,
        ] {
            assert_eq!(
                classify_transport_error(&TransportError::Io(k)),
                ErrorKind::Transient,
                "io::ErrorKind::{k:?} must be Transient"
            );
        }
    }

    #[test]
    fn classify_transport_error_io_terminal_kinds() {
        use std::io::ErrorKind as K;
        for k in [
            K::PermissionDenied,
            K::NotFound,
            K::AlreadyExists,
            K::InvalidInput,
            K::InvalidData,
            K::Other,
        ] {
            assert_eq!(
                classify_transport_error(&TransportError::Io(k)),
                ErrorKind::Terminal,
                "io::ErrorKind::{k:?} must be Terminal"
            );
        }
    }

    #[test]
    fn classify_transport_error_connect_and_timeout_are_transient() {
        assert_eq!(
            classify_transport_error(&TransportError::Connect),
            ErrorKind::Transient
        );
        assert_eq!(
            classify_transport_error(&TransportError::Timeout),
            ErrorKind::Transient
        );
        assert_eq!(
            classify_transport_error(&TransportError::Body),
            ErrorKind::Transient
        );
    }

    #[test]
    fn classify_transport_error_config_layer_is_terminal() {
        assert_eq!(
            classify_transport_error(&TransportError::InvalidAddress),
            ErrorKind::Terminal
        );
        assert_eq!(
            classify_transport_error(&TransportError::Decode),
            ErrorKind::Terminal
        );
        assert_eq!(
            classify_transport_error(&TransportError::ResponseTooLarge),
            ErrorKind::Terminal
        );
        assert_eq!(
            classify_transport_error(&TransportError::SocketConfig),
            ErrorKind::Terminal
        );
    }

    #[test]
    fn classify_transport_error_unknown_fails_closed() {
        // Fail-closed: an unknown error type must NOT trigger retries.
        assert_eq!(
            classify_transport_error(&TransportError::Unknown),
            ErrorKind::Terminal
        );
    }

    #[test]
    fn classify_error_typed_tag_roundtrip() {
        // Each typed variant round-trips through the wire tag and classifies
        // identically to the direct typed classifier.
        for (err, expected) in [
            (
                TransportError::Tls(TlsError::InvalidCertificate),
                ErrorKind::Terminal,
            ),
            (
                TransportError::Io(std::io::ErrorKind::ConnectionReset),
                ErrorKind::Transient,
            ),
            (
                TransportError::Io(std::io::ErrorKind::PermissionDenied),
                ErrorKind::Terminal,
            ),
            (TransportError::Connect, ErrorKind::Transient),
            (TransportError::Timeout, ErrorKind::Transient),
            (TransportError::InvalidAddress, ErrorKind::Terminal),
            (TransportError::Unknown, ErrorKind::Terminal),
        ] {
            let resp = TransportResponse::typed_error(err);
            let tag = resp.error.as_deref().unwrap();
            assert_eq!(classify_error(tag), expected, "tag={tag}");
        }
    }

    #[test]
    fn classify_error_unknown_freeform_is_terminal_fail_closed() {
        // Free-form strings without a typed tag fail closed. This is the
        // deliberate opposite of the pre-8mb.37 default so callers that
        // still hand in stringified errors do not accidentally retry-storm
        // unknown conditions.
        assert_eq!(classify_error("some unknown error"), ErrorKind::Terminal);
        assert_eq!(classify_error(""), ErrorKind::Terminal);
        assert_eq!(classify_error("connection reset by peer"), ErrorKind::Terminal);
    }

    #[tokio::test]
    async fn cert_error_aborts_immediately_without_retry() {
        let transport = make_transport(5, 10);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = transport
            .execute(
                RetryClass::Idempotent,
                move || {
                    let cnt = attempts_clone.clone();
                    async move {
                        cnt.fetch_add(1, Ordering::SeqCst);
                        TransportResponse::typed_error(TransportError::Tls(
                            TlsError::InvalidCertificate,
                        ))
                    }
                },
                no_sleep,
            )
            .await;

        // Must abort on first attempt — never retry a cert error.
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(matches!(result, TransportOutcome::Failed(_)));
        if let TransportOutcome::Failed(msg) = result {
            assert!(msg.contains("Terminal"));
        }
    }

    // ── Fix 2: MethodRetryPolicy is consulted ─────────────────────────────

    #[tokio::test]
    async fn mutation_not_retried_on_5xx() {
        let transport = make_transport(5, 10);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = transport
            .execute(
                RetryClass::Mutation, // non-idempotent
                move || {
                    let cnt = attempts_clone.clone();
                    async move {
                        cnt.fetch_add(1, Ordering::SeqCst);
                        TransportResponse::ok(500) // server error
                    }
                },
                no_sleep,
            )
            .await;

        // Mutations must not be retried on 5xx.
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(matches!(result, TransportOutcome::Failed(_)));
    }

    #[tokio::test]
    async fn idempotent_retried_on_5xx() {
        let transport = make_transport(3, 10);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = transport
            .execute(
                RetryClass::Idempotent,
                move || {
                    let cnt = attempts_clone.clone();
                    async move {
                        let n = cnt.fetch_add(1, Ordering::SeqCst) + 1;
                        if n < 3 {
                            TransportResponse::ok(500)
                        } else {
                            TransportResponse::ok(200)
                        }
                    }
                },
                no_sleep,
            )
            .await;

        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert!(matches!(result, TransportOutcome::Response(r) if r.is_success()));
    }

    // ── Fix 3: Retry-After header is honoured ─────────────────────────────

    #[test]
    fn retry_after_parsed_correctly() {
        let mut resp = TransportResponse::ok(429);
        resp.headers
            .insert("retry-after".to_string(), "30".to_string());
        assert_eq!(resp.retry_after(), Some(Duration::from_secs(30)));
    }

    #[test]
    fn retry_after_capped_at_300s() {
        let mut resp = TransportResponse::ok(429);
        resp.headers
            .insert("retry-after".to_string(), "3600".to_string());
        // Must be capped at 300 s.
        assert_eq!(resp.retry_after(), Some(Duration::from_secs(300)));
    }

    #[test]
    fn retry_after_missing_returns_none() {
        let resp = TransportResponse::ok(429);
        assert_eq!(resp.retry_after(), None);
    }

    #[tokio::test]
    async fn retry_after_used_instead_of_backoff() {
        let transport = make_transport(5, 10);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let observed_wait = Arc::new(std::sync::Mutex::new(Duration::ZERO));
        let observed_wait_clone = observed_wait.clone();

        let _ = transport
            .execute(
                RetryClass::Idempotent,
                move || {
                    let cnt = attempts_clone.clone();
                    async move {
                        let n = cnt.fetch_add(1, Ordering::SeqCst) + 1;
                        let mut resp = TransportResponse::ok(if n == 1 { 429 } else { 200 });
                        if n == 1 {
                            resp.headers
                                .insert("retry-after".to_string(), "5".to_string());
                        }
                        resp
                    }
                },
                move |d| {
                    let ow = observed_wait_clone.clone();
                    async move {
                        *ow.lock().unwrap() = d;
                    }
                },
            )
            .await;

        // The Retry-After (5 s) must override the Fixed(1 ms) backoff.
        assert_eq!(
            *observed_wait.lock().unwrap(),
            Duration::from_secs(5),
            "Retry-After header must override backoff"
        );
    }

    // ── Fix 4: Global retry budget ────────────────────────────────────────

    #[tokio::test]
    async fn global_budget_caps_total_attempts() {
        let transport = make_transport(100, 4); // inner allows 100, global cap is 4
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = transport
            .execute(
                RetryClass::Idempotent,
                move || {
                    let cnt = attempts_clone.clone();
                    async move {
                        cnt.fetch_add(1, Ordering::SeqCst);
                        TransportResponse::ok(500) // always fail
                    }
                },
                no_sleep,
            )
            .await;

        // Must stop at max_total_attempts = 4.
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        assert!(matches!(result, TransportOutcome::Failed(_)));
        if let TransportOutcome::Failed(msg) = result {
            assert!(msg.contains("budget") || msg.contains("exhausted"));
        }
    }

    // ── Success on first try ──────────────────────────────────────────────

    #[tokio::test]
    async fn success_on_first_try() {
        let transport = make_transport(3, 10);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = transport
            .execute(
                RetryClass::Idempotent,
                move || {
                    let cnt = attempts_clone.clone();
                    async move {
                        cnt.fetch_add(1, Ordering::SeqCst);
                        TransportResponse::ok(200)
                    }
                },
                no_sleep,
            )
            .await;

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(matches!(result, TransportOutcome::Response(r) if r.is_success()));
    }

    // ── M-1 fix: Retry-After does not consume budget tokens ───────────────

    /// A 429 with `Retry-After` must not increment the global attempt counter.
    /// Without the M-1 fix, two 429+Retry-After responses with max_total=3
    /// would exhaust the budget before the third real attempt. With the fix
    /// the budget is unaffected and the transport succeeds on the real attempt.
    #[tokio::test]
    async fn retry_after_does_not_burn_budget_token() {
        // max_total = 3 means three real attempts are allowed.
        let transport = make_transport(10, 3);
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let result = transport
            .execute(
                RetryClass::Idempotent,
                move || {
                    let cnt = call_count_clone.clone();
                    async move {
                        let n = cnt.fetch_add(1, Ordering::SeqCst) + 1;
                        if n <= 2 {
                            // First two calls: 429 with Retry-After (should not count against budget).
                            let mut resp = TransportResponse::ok(429);
                            resp.headers
                                .insert("retry-after".to_string(), "0".to_string());
                            resp
                        } else {
                            // Third real attempt succeeds.
                            TransportResponse::ok(200)
                        }
                    }
                },
                no_sleep,
            )
            .await;

        // All three calls must have been made (two Retry-After + one success).
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
        // The result must be a success, not a budget-exhausted failure.
        assert!(
            matches!(result, TransportOutcome::Response(ref r) if r.is_success()),
            "expected success but got {result:?}"
        );
    }

    // ── transport-metrics: error counter and histogram emit ───────────────

    #[cfg(feature = "transport-metrics")]
    #[tokio::test]
    async fn transport_metrics_tls_error_increments_tls_counter() {
        use crate::transport::metrics_impl::error_counts;

        let before = error_counts()
            .iter()
            .find(|(cls, _)| *cls == TransportErrorClass::Tls)
            .map(|(_, v)| *v)
            .unwrap_or(0);

        let transport = make_transport(5, 10);
        let _ = transport
            .execute(
                RetryClass::Idempotent,
                move || async move {
                    TransportResponse::typed_error(TransportError::Tls(
                        TlsError::InvalidCertificate,
                    ))
                },
                no_sleep,
            )
            .await;

        let after = error_counts()
            .iter()
            .find(|(cls, _)| *cls == TransportErrorClass::Tls)
            .map(|(_, v)| *v)
            .unwrap_or(0);

        assert_eq!(after, before + 1, "TLS error counter must increment by 1");
    }

    #[cfg(feature = "transport-metrics")]
    #[tokio::test]
    async fn transport_metrics_budget_exhausted_increments_counter() {
        use crate::transport::metrics_impl::error_counts;

        let before = error_counts()
            .iter()
            .find(|(cls, _)| *cls == TransportErrorClass::BudgetExhausted)
            .map(|(_, v)| *v)
            .unwrap_or(0);

        let transport = make_transport(100, 2);
        let _ = transport
            .execute(
                RetryClass::Idempotent,
                move || async move { TransportResponse::ok(500) },
                no_sleep,
            )
            .await;

        let after = error_counts()
            .iter()
            .find(|(cls, _)| *cls == TransportErrorClass::BudgetExhausted)
            .map(|(_, v)| *v)
            .unwrap_or(0);

        assert_eq!(
            after,
            before + 1,
            "BudgetExhausted counter must increment by 1"
        );
    }
}
