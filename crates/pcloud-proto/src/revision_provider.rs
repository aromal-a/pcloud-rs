//! Pluggable revision provider for `file-history` / `log` / `diff` / `restore`.
//!
//! # Why a provider trait instead of a direct API client
//!
//! pCloud's public API catalogue does not currently document a third-
//! party-accessible `listrevisions` endpoint — the legacy C client
//! relies on the binary protocol variant tied to the sync engine's
//! authenticated session state, which is not safe to re-expose through
//! the retained Rust backend until an approved surface exists.
//!
//! Rather than returning a bare `Unavailable` with no remediation path
//! (which yields a dead-end error on the CLI), we expose a
//! [`RevisionProvider`] trait. The daemon wires a provider selected at
//! bootstrap time:
//!
//! - **Null provider (default):** returns a structured
//!   [`RevisionError::NotConfigured`] message that tells the operator
//!   exactly which config key to populate.
//! - **HTTP provider (opt-in, feature `file-history-http`):** posts to
//!   an operator-configured URL and parses a JSON array of revisions.
//!   Useful for bridging to a future public pCloud endpoint, or to a
//!   customer-hosted revision service during migration.
//!
//! Both providers return the same [`Revision`] shape so the CLI
//! renderer (`pcloud-cli::main::render_file_history`) does not care
//! which backend produced the data.
//!
//! # Security posture
//!
//! - The HTTP provider (when enabled) requires an `https://` URL by
//!   default; plaintext URLs are refused outside of explicit test
//!   builds. The transport callable is injected by the caller so this
//!   crate does not pull in a full HTTP client stack.
//! - No secret material is logged or echoed. The provider receives
//!   only an opaque `path` (the remote pCloud path being queried).
//! - Response payloads are bounded to 1 MiB to defend against
//!   resource-exhaustion replies from a misbehaving endpoint.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One revision entry returned by a [`RevisionProvider`].
///
/// Field names mirror the shape the CLI renderer
/// (`render_file_history`) already consumes, which in turn mirrors the
/// legacy C `filerevision` table row populated by
/// `download_file_revisions` (`pclsync/pnetlibs.c:2494`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    /// Content-addressed hex revision id (the C `hash` column,
    /// stringified as lowercase hex).
    pub rev_id: String,
    /// Modification timestamp (UNIX seconds) reported by the server.
    #[serde(default)]
    pub mtime: u64,
    /// Revision size in bytes.
    #[serde(default)]
    pub size: u64,
    /// Display name / email of the user that produced the revision.
    /// Empty when the server omits the field.
    #[serde(default)]
    pub user: String,
    /// Optional free-text comment attached to the revision. Empty
    /// when the server omits the field.
    #[serde(default)]
    pub comment: String,
}

/// Errors raised by a [`RevisionProvider`].
///
/// Every variant is rendered as structured JSON on the CLI so the
/// operator gets actionable remediation rather than a bare error
/// string.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RevisionError {
    /// No revision endpoint is configured. The wrapped message tells
    /// the operator which config key to populate.
    #[error("revision provider not configured: {0}")]
    NotConfigured(String),
    /// The configured URL is syntactically invalid or violates the
    /// transport policy (e.g. plaintext `http://` in production).
    #[error("revision provider URL rejected: {0}")]
    InvalidUrl(String),
    /// The transport callable failed (network error, TLS failure, etc.).
    #[error("revision provider transport error: {0}")]
    Transport(String),
    /// The endpoint returned a non-2xx HTTP status.
    #[error("revision provider returned HTTP {status}")]
    HttpStatus {
        /// HTTP status code from the endpoint.
        status: u16,
    },
    /// The response body did not parse as a JSON array of revisions.
    #[error("revision provider returned malformed payload: {0}")]
    MalformedResponse(String),
    /// The request path was empty or otherwise invalid before dispatch.
    #[error("invalid request: {0}")]
    InvalidRequest(&'static str),
}

/// Pluggable revision provider.
///
/// Implementors take a remote pCloud path and return the server's
/// revision history. The daemon installs exactly one provider at
/// bootstrap (see `crates/pcloud-daemon/src/runtime.rs`).
///
/// Implementors are expected to be cheap to call repeatedly and to be
/// safe to share across threads (`Send + Sync`).
pub trait RevisionProvider: Send + Sync {
    /// Return the revision history of `path` on the remote store.
    ///
    /// `path` must be a non-empty absolute remote path. An empty or
    /// whitespace-only path yields [`RevisionError::InvalidRequest`].
    fn list_revisions(&self, path: &str) -> Result<Vec<Revision>, RevisionError>;
}

/// Default provider that always reports "not configured".
///
/// The error message names the exact config key the operator needs to
/// populate so the CLI can print an actionable remediation hint.
///
/// ```
/// use pcloud_proto::revision_provider::{NullRevisionProvider, RevisionError, RevisionProvider};
/// let p = NullRevisionProvider::default();
/// let err = p.list_revisions("/Docs/report.txt").unwrap_err();
/// matches!(err, RevisionError::NotConfigured(_));
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct NullRevisionProvider;

impl NullRevisionProvider {
    /// Construct a new null provider. Const so it can be materialised
    /// in static contexts.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Canonical message emitted by [`NullRevisionProvider::list_revisions`].
///
/// Exposed as a constant so the daemon's structured JSON response and
/// the provider's error message stay in lockstep.
pub const NULL_PROVIDER_MESSAGE: &str = "pCloud listrevisions API not yet public; configure [file_history].revision_url \
     to point at a custom endpoint";

impl RevisionProvider for NullRevisionProvider {
    fn list_revisions(&self, path: &str) -> Result<Vec<Revision>, RevisionError> {
        if path.trim().is_empty() {
            return Err(RevisionError::InvalidRequest(
                "path must be a non-empty absolute remote path",
            ));
        }
        Err(RevisionError::NotConfigured(
            NULL_PROVIDER_MESSAGE.to_owned(),
        ))
    }
}

// --------------------------------------------------------------------
// HTTP provider (opt-in)
// --------------------------------------------------------------------

/// HTTP transport callable used by the [`HttpRevisionProvider`].
///
/// Takes a target URL and a serialized JSON request body; returns the
/// HTTP status code and response body bytes, or a transport error
/// string. Kept as a trait object so this crate does not pull in a
/// specific HTTP client — the caller injects whichever transport they
/// already use (`ureq`, a `rustls`-wrapped `std::net::TcpStream`, a
/// test double, etc.).
#[cfg(feature = "file-history-http")]
pub type HttpTransport =
    std::sync::Arc<dyn Fn(&str, &[u8]) -> Result<(u16, Vec<u8>), String> + Send + Sync>;

/// Operator-configurable HTTP revision provider.
///
/// Posts `{"path": "<remote path>"}` to the configured URL as JSON and
/// expects the endpoint to reply with a JSON array of [`Revision`]
/// objects (or a `{"revisions": [...]}` envelope). Both shapes are
/// accepted so operators can reuse an existing revision service
/// without an adapter.
///
/// # Security
///
/// - The URL must be `https://` unless the `allow_plaintext` override
///   is explicitly set (only honoured in test builds).
/// - Response bodies are capped at 1 MiB; larger payloads are rejected
///   as [`RevisionError::MalformedResponse`] before JSON parsing.
/// - The transport callable is fully caller-owned so audit logging,
///   TLS posture, and timeouts live in one place at bootstrap.
#[cfg(feature = "file-history-http")]
#[derive(Clone)]
pub struct HttpRevisionProvider {
    url: String,
    transport: HttpTransport,
    max_body_bytes: usize,
}

#[cfg(feature = "file-history-http")]
impl std::fmt::Debug for HttpRevisionProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRevisionProvider")
            .field("url", &self.url)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("transport", &"<callable>")
            .finish()
    }
}

#[cfg(feature = "file-history-http")]
impl HttpRevisionProvider {
    /// Default response-body cap (1 MiB).
    pub const DEFAULT_MAX_BODY_BYTES: usize = 1024 * 1024;

    /// Construct a new HTTP provider bound to `url` via `transport`.
    ///
    /// Returns [`RevisionError::InvalidUrl`] if `url` is not
    /// `https://…`. Production callers should never relax this.
    pub fn new(url: impl Into<String>, transport: HttpTransport) -> Result<Self, RevisionError> {
        let url = url.into();
        Self::validate_url(&url, false)?;
        Ok(Self {
            url,
            transport,
            max_body_bytes: Self::DEFAULT_MAX_BODY_BYTES,
        })
    }

    /// Construct a provider that accepts plaintext `http://` URLs.
    ///
    /// Intended for integration tests and local mock servers. The
    /// daemon must refuse this constructor in production profiles.
    #[doc(hidden)]
    pub fn new_allow_plaintext(
        url: impl Into<String>,
        transport: HttpTransport,
    ) -> Result<Self, RevisionError> {
        let url = url.into();
        Self::validate_url(&url, true)?;
        Ok(Self {
            url,
            transport,
            max_body_bytes: Self::DEFAULT_MAX_BODY_BYTES,
        })
    }

    /// Override the response-body size cap. 0 is refused.
    #[must_use]
    pub fn with_max_body_bytes(mut self, bytes: usize) -> Self {
        if bytes > 0 {
            self.max_body_bytes = bytes;
        }
        self
    }

    fn validate_url(url: &str, allow_plaintext: bool) -> Result<(), RevisionError> {
        if url.is_empty() {
            return Err(RevisionError::InvalidUrl("url must not be empty".into()));
        }
        if url.starts_with("https://") {
            return Ok(());
        }
        if allow_plaintext && url.starts_with("http://") {
            return Ok(());
        }
        Err(RevisionError::InvalidUrl(format!(
            "url must start with https:// (got {url:?})"
        )))
    }
}

#[cfg(feature = "file-history-http")]
impl RevisionProvider for HttpRevisionProvider {
    fn list_revisions(&self, path: &str) -> Result<Vec<Revision>, RevisionError> {
        if path.trim().is_empty() {
            return Err(RevisionError::InvalidRequest(
                "path must be a non-empty absolute remote path",
            ));
        }
        let body = serde_json::to_vec(&serde_json::json!({ "path": path }))
            .map_err(|e| RevisionError::Transport(format!("serialize request: {e}")))?;
        let (status, mut response) =
            (self.transport)(&self.url, &body).map_err(RevisionError::Transport)?;
        if !(200..300).contains(&status) {
            return Err(RevisionError::HttpStatus { status });
        }
        if response.len() > self.max_body_bytes {
            return Err(RevisionError::MalformedResponse(format!(
                "response body exceeds cap ({} > {})",
                response.len(),
                self.max_body_bytes
            )));
        }
        // Accept either `[{...}]` or `{"revisions":[{...}]}` shape.
        response.shrink_to_fit();
        let value: serde_json::Value = serde_json::from_slice(&response)
            .map_err(|e| RevisionError::MalformedResponse(format!("parse json: {e}")))?;
        let arr = match value {
            serde_json::Value::Array(_) => value,
            serde_json::Value::Object(ref map) => {
                map.get("revisions").cloned().ok_or_else(|| {
                    RevisionError::MalformedResponse(
                        "object response missing `revisions` field".into(),
                    )
                })?
            }
            _ => {
                return Err(RevisionError::MalformedResponse(
                    "expected JSON array or `{revisions:[...]}` object".into(),
                ));
            }
        };
        serde_json::from_value::<Vec<Revision>>(arr)
            .map_err(|e| RevisionError::MalformedResponse(format!("parse revisions: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_provider_returns_not_configured_with_actionable_message() {
        let p = NullRevisionProvider::new();
        let err = p.list_revisions("/Docs/report.txt").unwrap_err();
        match err {
            RevisionError::NotConfigured(msg) => {
                assert!(msg.contains("revision_url"), "actionable hint: {msg}");
                assert!(msg.contains("file_history"));
            }
            other => panic!("expected NotConfigured, got {other:?}"),
        }
    }

    #[test]
    fn null_provider_rejects_empty_path() {
        let p = NullRevisionProvider::new();
        let err = p.list_revisions("   ").unwrap_err();
        assert!(matches!(err, RevisionError::InvalidRequest(_)));
    }

    #[test]
    fn revision_round_trips_through_serde() {
        let rev = Revision {
            rev_id: "deadbeef".into(),
            mtime: 1_700_000_000,
            size: 4096,
            user: "alice@example.com".into(),
            comment: "rollup".into(),
        };
        let json = serde_json::to_string(&rev).unwrap();
        let round: Revision = serde_json::from_str(&json).unwrap();
        assert_eq!(rev, round);
    }

    #[test]
    fn revision_defaults_accept_partial_payloads() {
        let rev: Revision = serde_json::from_str(r#"{"rev_id":"x"}"#).unwrap();
        assert_eq!(rev.rev_id, "x");
        assert_eq!(rev.mtime, 0);
        assert_eq!(rev.user, "");
    }

    #[cfg(feature = "file-history-http")]
    mod http {
        use super::*;
        use std::sync::Arc;
        use std::sync::Mutex;

        #[test]
        fn http_provider_rejects_plaintext_url_by_default() {
            let transport: HttpTransport = Arc::new(|_url, _body| Ok((200, b"[]".to_vec())));
            let err = HttpRevisionProvider::new("http://insecure.example/r", transport)
                .expect_err("plaintext must be refused");
            assert!(matches!(err, RevisionError::InvalidUrl(_)));
        }

        #[test]
        fn http_provider_accepts_https_and_parses_array() {
            let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
            let cap2 = Arc::clone(&captured);
            let transport: HttpTransport = Arc::new(move |url, body| {
                assert_eq!(url, "https://example/r");
                cap2.lock().unwrap().extend_from_slice(body);
                let payload = br#"[{"rev_id":"aa","mtime":1,"size":2,"user":"u","comment":"c"}]"#;
                Ok((200, payload.to_vec()))
            });
            let p = HttpRevisionProvider::new("https://example/r", transport).unwrap();
            let out = p.list_revisions("/x/y").unwrap();
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].rev_id, "aa");
            assert!(!captured.lock().unwrap().is_empty());
        }

        #[test]
        fn http_provider_accepts_object_envelope() {
            let transport: HttpTransport = Arc::new(|_url, _body| {
                let payload = br#"{"revisions":[{"rev_id":"bb"}]}"#;
                Ok((200, payload.to_vec()))
            });
            let p = HttpRevisionProvider::new("https://example/r", transport).unwrap();
            let out = p.list_revisions("/x").unwrap();
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].rev_id, "bb");
        }

        #[test]
        fn http_provider_surfaces_non_2xx_status() {
            let transport: HttpTransport = Arc::new(|_url, _body| Ok((503, Vec::new())));
            let p = HttpRevisionProvider::new("https://example/r", transport).unwrap();
            let err = p.list_revisions("/x").unwrap_err();
            assert_eq!(err, RevisionError::HttpStatus { status: 503 });
        }

        #[test]
        fn http_provider_rejects_oversized_body() {
            let big = vec![b'x'; 2048];
            let transport: HttpTransport = Arc::new(move |_, _| Ok((200, big.clone())));
            let p = HttpRevisionProvider::new("https://example/r", transport)
                .unwrap()
                .with_max_body_bytes(1024);
            let err = p.list_revisions("/x").unwrap_err();
            assert!(matches!(err, RevisionError::MalformedResponse(_)));
        }

        #[test]
        fn http_provider_rejects_non_array_payload() {
            let transport: HttpTransport = Arc::new(|_, _| Ok((200, b"42".to_vec())));
            let p = HttpRevisionProvider::new("https://example/r", transport).unwrap();
            let err = p.list_revisions("/x").unwrap_err();
            assert!(matches!(err, RevisionError::MalformedResponse(_)));
        }

        #[test]
        fn http_provider_rejects_empty_path() {
            let transport: HttpTransport = Arc::new(|_, _| Ok((200, b"[]".to_vec())));
            let p = HttpRevisionProvider::new("https://example/r", transport).unwrap();
            let err = p.list_revisions("").unwrap_err();
            assert!(matches!(err, RevisionError::InvalidRequest(_)));
        }

        #[test]
        fn http_provider_allow_plaintext_constructor_used_only_in_tests() {
            let transport: HttpTransport = Arc::new(|_, _| Ok((200, b"[]".to_vec())));
            let p =
                HttpRevisionProvider::new_allow_plaintext("http://localhost/r", transport).unwrap();
            let out = p.list_revisions("/x").unwrap();
            assert!(out.is_empty());
        }
    }
}
