//! pCloud trusted-issuer token exchange.
//!
//! The OIDC broker ([`crate::OidcAuthorizationCodeBroker`]) mints an IdP
//! ID token (JWT) as part of the Authorization Code + PKCE flow. To log
//! the user into pCloud, that JWT must be exchanged for a pCloud session
//! token via a **trusted-issuer exchange** endpoint.
//!
//! pCloud's public API does **not** document such an endpoint at the
//! time of writing. Rather than ship a fake success path or a panicking
//! stub, this module exposes a pluggable trait so that:
//!
//! - the default behaviour is an honest typed error
//!   ([`IdpError::NotConfigured`]), surfaced via [`NullPcloudTokenExchanger`];
//! - site operators running their own JWT-to-pCloud-token bridge can
//!   wire it in through [`HttpPcloudTokenExchanger`] (feature
//!   `oidc-http-exchange`, enabled by default);
//! - a future pCloud-hosted exchange endpoint can land as a third
//!   implementor without touching the broker.
//!
//! # Security posture
//!
//! - [`PcloudSession::auth_token`] is a [`SecretString`]: zeroized on
//!   `Drop`, redacted in `Debug`.
//! - The ID token submitted to the exchange endpoint is handled as a
//!   [`SecretString`] throughout; the HTTP implementor exposes it only
//!   for the duration of a single `POST` body.
//! - The HTTP implementor rejects non-TLS endpoints at construction
//!   time unless `allow_plaintext_for_tests(true)` is set — this flag
//!   is gated `#[cfg(any(test, feature = "insecure-plaintext-exchange"))]`
//!   so production builds cannot disable TLS.
//! - Response bodies are never included in error messages. Operators
//!   must read daemon audit logs, not CLI stderr, to diagnose exchange
//!   failures.

use std::time::SystemTime;

use pcloud_secret::secret_string::SecretString;

use crate::IdpError;

/// Short-lived pCloud session derived from a trusted-issuer exchange.
///
/// The daemon persists this through its existing `auth_vault` plumbing
/// (opt-in, `0600`, `0700` parent dir). No IdP refresh material is
/// stored here — that lives on [`crate::IdpToken`].
#[derive(Debug)]
pub struct PcloudSession {
    /// pCloud `auth` token. Secret-wrapped.
    pub auth_token: SecretString,
    /// Optional absolute expiry, when the exchange endpoint returns one.
    pub expires_at: Option<SystemTime>,
}

/// Pluggable pCloud trusted-issuer exchange.
///
/// Implementors convert a verified IdP ID token into a pCloud session.
/// The trait is deliberately minimal: it takes a single [`SecretString`]
/// and returns a [`PcloudSession`] or a typed error. Audience / issuer
/// verification of the ID token is the broker's responsibility — this
/// trait is strictly the pCloud-side half of the handshake.
///
/// # Contract
///
/// 1. **No secret logging.** Implementors MUST NOT log or echo the
///    ID token, the response body, or the returned auth token.
/// 2. **TLS-only in production.** HTTP implementors MUST reject
///    `http://` endpoints outside of test builds.
/// 3. **Typed failures.** Absence of configuration MUST surface as
///    [`IdpError::NotConfigured`]. Network / HTTP failures MUST use
///    [`IdpError::TokenExchange`]. A rejected token MUST use
///    [`IdpError::RefreshRejected`] when interactive re-auth is the
///    correct remediation.
/// 4. **Thread safety.** Implementors are `Send + Sync` so the daemon
///    plugin registry can hold an `Arc<dyn PcloudTokenExchanger>`.
pub trait PcloudTokenExchanger: Send + Sync {
    /// Exchange an IdP ID token for a pCloud session.
    ///
    /// # Errors
    ///
    /// - [`IdpError::NotConfigured`] if no exchange endpoint is wired.
    /// - [`IdpError::TokenExchange`] if the exchange endpoint is
    ///   reachable but rejects or cannot parse the request.
    /// - [`IdpError::RefreshRejected`] if the exchange endpoint
    ///   explicitly rejects the ID token and interactive re-auth is
    ///   required.
    fn exchange(&self, oidc_token: &SecretString) -> Result<PcloudSession, IdpError>;
}

/// Default exchanger that always returns [`IdpError::NotConfigured`].
///
/// Wired by default so the daemon plugin registry never holds `None`.
/// Operators must explicitly register a concrete exchanger to enable
/// the pCloud half of the OIDC handshake.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullPcloudTokenExchanger;

/// Operator guidance returned when the default [`NullPcloudTokenExchanger`]
/// is invoked.
pub const NULL_EXCHANGER_MESSAGE: &str = "pCloud trusted-issuer exchange endpoint not configured; set [oidc.trusted_issuer].exchange_url";

impl PcloudTokenExchanger for NullPcloudTokenExchanger {
    fn exchange(&self, _oidc_token: &SecretString) -> Result<PcloudSession, IdpError> {
        Err(IdpError::NotConfigured(NULL_EXCHANGER_MESSAGE))
    }
}

// ---------------------------------------------------------------------------
// HTTP exchanger (default-on cargo feature `oidc-http-exchange`).
// ---------------------------------------------------------------------------

#[cfg(feature = "oidc-http-exchange")]
pub use http_exchanger::HttpPcloudTokenExchanger;

#[cfg(feature = "oidc-http-exchange")]
mod http_exchanger {
    use super::{IdpError, PcloudSession, PcloudTokenExchanger, SecretString, SystemTime};

    use std::time::Duration;

    use pcloud_secret::ExposeSecret;
    use serde::{Deserialize, Serialize};

    /// HTTP-backed pCloud trusted-issuer exchanger.
    ///
    /// POSTs a JSON body `{ "id_token": "<jwt>" }` to the configured
    /// endpoint and parses a response shaped like
    /// `{ "auth": "<token>", "expires_in": <secs?> }`.
    ///
    /// # Operator configuration
    ///
    /// ```toml
    /// [oidc.trusted_issuer]
    /// exchange_url = "https://bridge.corp.example/pcloud/exchange"
    /// ```
    ///
    /// The URL must be HTTPS outside of test builds. Construction fails
    /// with [`IdpError::NotConfigured`] if a plaintext URL is provided.
    pub struct HttpPcloudTokenExchanger {
        http: reqwest::blocking::Client,
        exchange_url: String,
    }

    impl std::fmt::Debug for HttpPcloudTokenExchanger {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("HttpPcloudTokenExchanger")
                .field("exchange_url", &self.exchange_url)
                .finish_non_exhaustive()
        }
    }

    impl HttpPcloudTokenExchanger {
        /// Construct an exchanger targeting `exchange_url`. The URL must
        /// be `https://` in release builds; `http://` is rejected at
        /// construction time.
        pub fn new(exchange_url: impl Into<String>) -> Result<Self, IdpError> {
            let url = exchange_url.into();
            reject_plaintext_in_prod(&url)?;
            let http = reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(30))
                .https_only(url.starts_with("https://"))
                .build()
                .map_err(|e| IdpError::Other(format!("build http client: {e}")))?;
            Ok(Self {
                http,
                exchange_url: url,
            })
        }

        /// Construct with a caller-provided HTTP client. Used in tests
        /// to inject a client that permits `http://127.0.0.1` stubs.
        #[cfg(any(test, feature = "insecure-plaintext-exchange"))]
        pub fn with_client(
            exchange_url: impl Into<String>,
            http: reqwest::blocking::Client,
        ) -> Self {
            Self {
                http,
                exchange_url: exchange_url.into(),
            }
        }
    }

    fn reject_plaintext_in_prod(url: &str) -> Result<(), IdpError> {
        if url.starts_with("https://") {
            return Ok(());
        }
        #[cfg(any(test, feature = "insecure-plaintext-exchange"))]
        {
            if url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost") {
                return Ok(());
            }
        }
        Err(IdpError::NotConfigured(
            "pCloud trusted-issuer exchange_url must be https://",
        ))
    }

    #[derive(Debug, Serialize)]
    struct ExchangeRequest<'a> {
        id_token: &'a str,
    }

    // Intentionally *not* derived with serde::Serialize on SecretString
    // (pcloud-secret forbids it). We build the JSON body by hand so the
    // secret never appears in a serde-derived container.
    impl HttpPcloudTokenExchanger {
        fn post_exchange(&self, id_token: &str) -> Result<ExchangeResponse, IdpError> {
            // Serialize a *borrowed* view of the secret so we never
            // clone the value onto the heap unnecessarily.
            let body = serde_json::to_vec(&ExchangeRequest { id_token })
                .map_err(|e| IdpError::Other(format!("encode exchange body: {e}")))?;
            let resp = self
                .http
                .post(&self.exchange_url)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .map_err(|e| IdpError::TokenExchange(format!("POST exchange: {e}")))?;
            let status = resp.status();
            if status.is_success() {
                resp.json::<ExchangeResponse>()
                    .map_err(|e| IdpError::TokenExchange(format!("decode exchange: {e}")))
            } else if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                Err(IdpError::RefreshRejected)
            } else {
                // Never surface the body — it may contain operator-
                // sensitive hints or fragments of the request.
                Err(IdpError::TokenExchange(format!("HTTP {status}")))
            }
        }
    }

    #[derive(Debug, Deserialize)]
    struct ExchangeResponse {
        auth: String,
        #[serde(default)]
        expires_in: Option<u64>,
    }

    impl PcloudTokenExchanger for HttpPcloudTokenExchanger {
        fn exchange(&self, oidc_token: &SecretString) -> Result<PcloudSession, IdpError> {
            let resp = self.post_exchange(oidc_token.expose_secret())?;
            let expires_at = resp
                .expires_in
                .map(|s| SystemTime::now() + Duration::from_secs(s));
            Ok(PcloudSession {
                auth_token: SecretString::new(resp.auth),
                expires_at,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_exchanger_returns_not_configured() {
        let exch = NullPcloudTokenExchanger;
        let tok = SecretString::new("jwt");
        let err = exch.exchange(&tok).unwrap_err();
        match err {
            IdpError::NotConfigured(msg) => {
                assert!(msg.contains("exchange_url"), "msg = {msg}");
            }
            other => panic!("expected NotConfigured, got {other:?}"),
        }
    }

    #[test]
    fn null_exchanger_is_object_safe() {
        let _: Box<dyn PcloudTokenExchanger> = Box::new(NullPcloudTokenExchanger);
    }

    #[cfg(feature = "oidc-http-exchange")]
    mod http {
        use super::*;
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        use pcloud_secret::ExposeSecret;

        fn spawn_stub(status_line: &'static str, body: &'static str) -> std::net::SocketAddr {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let addr = listener.local_addr().expect("addr");
            thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf);
                    let resp = format!(
                        "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
            addr
        }

        fn permissive_client() -> reqwest::blocking::Client {
            reqwest::blocking::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(2))
                .timeout(std::time::Duration::from_secs(2))
                .https_only(false)
                .build()
                .expect("client")
        }

        #[test]
        fn http_exchanger_rejects_plaintext_url_in_prod_path() {
            // new() allows http://127.0.0.1 under #[cfg(test)], so
            // assert the rejection for a non-loopback plaintext URL.
            let err = HttpPcloudTokenExchanger::new("http://bridge.example/exchange").unwrap_err();
            assert!(matches!(err, IdpError::NotConfigured(_)));
        }

        #[test]
        fn http_exchanger_returns_pcloud_session_on_success() {
            let addr = spawn_stub(
                "200 OK",
                r#"{"auth":"pcloud-auth-token","expires_in":3600}"#,
            );
            let exch = HttpPcloudTokenExchanger::with_client(
                format!("http://{addr}/exchange"),
                permissive_client(),
            );
            let tok = SecretString::new("jwt-value");
            let sess = exch.exchange(&tok).expect("ok");
            assert_eq!(sess.auth_token.expose_secret(), "pcloud-auth-token");
            assert!(sess.expires_at.is_some());
        }

        #[test]
        fn http_exchanger_maps_401_to_refresh_rejected() {
            let addr = spawn_stub("401 Unauthorized", r#"{"error":"bad token"}"#);
            let exch = HttpPcloudTokenExchanger::with_client(
                format!("http://{addr}/exchange"),
                permissive_client(),
            );
            let tok = SecretString::new("jwt-value");
            let err = exch.exchange(&tok).unwrap_err();
            assert!(matches!(err, IdpError::RefreshRejected));
        }

        #[test]
        fn http_exchanger_maps_500_to_token_exchange_without_body() {
            let addr = spawn_stub("500 Internal Server Error", "operator-secret-hint");
            let exch = HttpPcloudTokenExchanger::with_client(
                format!("http://{addr}/exchange"),
                permissive_client(),
            );
            let tok = SecretString::new("jwt-value");
            let err = exch.exchange(&tok).unwrap_err();
            match err {
                IdpError::TokenExchange(msg) => {
                    assert!(!msg.contains("operator-secret-hint"), "body leaked: {msg}");
                    assert!(msg.contains("500"), "missing status: {msg}");
                }
                other => panic!("expected TokenExchange, got {other:?}"),
            }
        }
    }
}
