#![allow(clippy::pedantic)]
//! Enterprise Identity Provider (IdP) broker trait scaffold.
//!
//! This crate defines the abstract surface used by `pcloud-daemon` and
//! `pcloud-cli` to federate login against an enterprise IdP (Okta, Azure AD,
//! Keycloak, Ping, Google Workspace, ...) instead of pCloud's password auth.
//!
//! # Why
//!
//! Enterprises typically disallow password-based logins to third-party SaaS
//! tools. They mandate federated SSO via OIDC or SAML, often layered with
//! conditional access, device posture, and MFA enforced at the IdP. The broker
//! defined here sits between the user's org IdP and pCloud so that:
//!
//! - the broker never observes the user's password,
//! - the ID token is wrapped in [`pcloud_secret::secret_string::SecretString`] and redacted
//!   in logs,
//! - the pCloud session is a short-lived derivative tied to a refreshable
//!   IdP session.
//!
//! # Scope of this crate
//!
//! **This crate is the trait scaffold plus a concrete OIDC Authorization
//! Code + PKCE broker.** Legacy stubs that previously panicked have been
//! replaced with typed [`IdpError::NotConfigured`] errors. A future crate
//! (`pcloud-idp-saml`) will add the SAML bridge implementation.
//!
//! # Trait surface
//!
//! See [`IdpBroker`] for the three-step flow:
//!
//! 1. [`IdpBroker::begin_authorization`] constructs an [`AuthChallenge`]
//!    (authorization URL, PKCE verifier, state, nonce).
//! 2. [`IdpBroker::complete_authorization`] exchanges the authorization
//!    code returned by the IdP for an [`IdpToken`].
//! 3. [`IdpBroker::refresh`] trades a refresh token for a fresh ID token
//!    without user interaction.
//!
//! # Security posture
//!
//! - ID tokens and refresh tokens are stored as [`pcloud_secret::secret_string::SecretString`]
//!   so they are zeroized on drop and redacted in `Debug` output.
//! - JWKS pinning, issuer validation, and nonce binding are responsibilities
//!   of concrete implementations and are specified in the design doc.
//! - This trait does not permit password submission: there is no
//!   password-grant entry point.
//!
//! # Threats mitigated
//!
//! - **Token leakage via logs / panics**: all tokens ride in
//!   [`SecretString`] which zeroizes on drop and redacts on `Debug`.
//! - **Authorization-code injection / replay**: `state` and `nonce` must be
//!   verified on callback, and [`AuthChallenge`] is consumed by
//!   `complete_authorization` so the PKCE verifier cannot be reused.
//! - **IdP impersonation**: implementations MUST pin JWKS and validate
//!   `iss`/`aud`/`exp`. The scaffold does not provide a default — it is the
//!   implementor's responsibility, documented per-method on [`IdpBroker`].
//! - **Password disclosure to the broker**: the trait exposes no
//!   password-grant entry point; the user authenticates directly with the
//!   IdP.
//!
//! # Not yet implemented
//!
//! - OIDC Authorization Code + PKCE completion is scaffolded in
//!   [`oidc::OidcAuthorizationCodeBroker`] but network I/O (discovery, token
//!   exchange, JWKS fetching) is deferred to `pcloud-idp-oidc`.
//! - SAML bridging ([`IdpFlow::SamlBridge`]) lives in `pcloud-idp-saml`.
//! - Device-code and LDAP flows are declared but not yet broker-ready.
//!
//! # pCloud trusted-issuer exchange
//!
//! The OIDC broker mints an IdP ID token; turning that into a pCloud
//! session requires a trusted-issuer exchange endpoint. pCloud's public
//! API does **not** document such an endpoint at the time of writing, so
//! the exchange is pluggable through the [`PcloudTokenExchanger`] trait:
//!
//! - [`NullPcloudTokenExchanger`] (always available) returns
//!   [`IdpError::NotConfigured`]. This is the default and makes the
//!   "no exchange available" state explicit instead of panicking.
//! - [`HttpPcloudTokenExchanger`] (feature `oidc-http-exchange`, enabled
//!   by default) POSTs the ID token to a configurable URL and parses a
//!   pCloud session response. Site operators that run their own bridge
//!   service can wire it in; no pCloud-hosted exchange endpoint is
//!   officially documented today.
//!
//! # bd tracker
//!
//! - Parity tracking: `bd-1du` family; this crate is scaffolding so it does
//!   not have a dedicated parity row yet. See `RUST-PLANS/` for the OIDC
//!   broker implementation sequencing.
//!
//! # How to enable
//!
//! The daemon wires a broker implementation through its plugin registry.
//! In operator config:
//!
//! ```toml
//! [auth.idp]
//! issuer = "https://corp.okta.com"
//! client_id = "pcloudc"
//! flow = "oidc_authorization_code"
//! scopes = ["openid", "email", "profile"]
//! ```
//!
//! The daemon selects the matching broker at startup; absent a broker
//! crate, [`UnimplementedBroker`] is registered so plugin lookup does not
//! return `None`.
//!
//! # Example
//!
//! ```
//! use pcloud_idp::{IdpConfig, IdpFlow};
//!
//! let cfg = IdpConfig {
//!     issuer: "https://corp.okta.com".into(),
//!     client_id: "pcloudc".into(),
//!     flow: IdpFlow::OidcAuthorizationCode,
//!     scopes: vec!["openid".into(), "email".into()],
//! };
//! assert_eq!(cfg.flow, IdpFlow::OidcAuthorizationCode);
//! ```
//!
//! ```
//! use pcloud_idp::{IdpBroker, IdpConfig, IdpError, IdpFlow, UnimplementedBroker};
//!
//! // The scaffold is object-safe so the daemon can hold a `Box<dyn IdpBroker>`.
//! let b: Box<dyn IdpBroker> = Box::new(UnimplementedBroker);
//! let cfg = IdpConfig {
//!     issuer: "https://idp.example".into(),
//!     client_id: "pcloudc".into(),
//!     flow: IdpFlow::OidcAuthorizationCode,
//!     scopes: vec!["openid".into()],
//! };
//! // The unregistered broker surfaces a typed error, not a panic.
//! let err = b.begin_authorization(&cfg).unwrap_err();
//! assert!(matches!(err, IdpError::NotConfigured(_)));
//! ```
//!
//! ```
//! use pcloud_idp::IdpError;
//! // RefreshRejected is the one variant callers must handle specially:
//! // it signals that interactive re-auth is required.
//! let e = IdpError::RefreshRejected;
//! assert!(format!("{e}").contains("interactive"));
//! ```

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use std::time::SystemTime;

pub use pcloud_secret::secret_string::SecretString;

pub mod exchange;
pub(crate) mod jwks;
pub mod oidc;
pub(crate) mod pkce;

#[cfg(feature = "oidc-http-exchange")]
pub use exchange::HttpPcloudTokenExchanger;
pub use exchange::{NullPcloudTokenExchanger, PcloudSession, PcloudTokenExchanger};

pub use oidc::OidcAuthorizationCodeBroker;

/// Supported federation flows.
///
/// The primary flow is [`IdpFlow::OidcAuthorizationCode`] with PKCE. The
/// others exist to document the operator surface and keep the design doc and
/// crate aligned; concrete implementations land in follow-on crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdpFlow {
    /// OIDC Authorization Code flow with PKCE. Primary flow for interactive
    /// desktop login.
    OidcAuthorizationCode,
    /// OIDC Device Authorization Grant (RFC 8628). Used when the host running
    /// `pcloudc` has no browser, e.g. headless Linux servers.
    DeviceCode,
    /// SAML 2.0 bridged to OIDC by an intermediate broker. pCloud-native SAML
    /// ingestion is deferred; see the design doc for rationale.
    SamlBridge,
    /// LDAP simple-bind used only to seed an OIDC enrollment on first use.
    /// Subsequent logins upgrade to [`IdpFlow::OidcAuthorizationCode`].
    Ldap,
}

/// Operator-facing IdP configuration.
///
/// This struct mirrors the `[auth.idp]` section of `config.toml`. It does not
/// carry any secrets: the client secret is never stored here because the
/// primary flow is PKCE, which is a public-client flow.
#[derive(Debug, Clone)]
pub struct IdpConfig {
    /// OIDC issuer URL, e.g. `https://corp.okta.com`. Used to fetch
    /// `.well-known/openid-configuration` and derive the JWKS URI.
    pub issuer: String,
    /// OAuth 2.0 client ID registered with the IdP for this tenant.
    pub client_id: String,
    /// Selected federation flow.
    pub flow: IdpFlow,
    /// Requested OIDC scopes. Implementations must ensure `openid` is present.
    pub scopes: Vec<String>,
}

/// Server-bound authorization challenge returned by
/// [`IdpBroker::begin_authorization`].
///
/// The challenge owns the ephemeral PKCE verifier (a [`SecretString`]) and the
/// state/nonce values that must be replayed on callback. The broker implementation
/// is responsible for verifying state and nonce on completion.
#[derive(Debug)]
pub struct AuthChallenge {
    /// Fully-formed authorization URL the user's browser should open.
    pub authorization_url: String,
    /// PKCE `code_verifier`. Secret; never logged, never sent to pCloud.
    pub pkce_verifier: SecretString,
    /// Opaque CSRF `state` value. Must match on callback.
    pub state: String,
    /// OIDC `nonce`. Must match the `nonce` claim in the returned ID token.
    pub nonce: String,
}

/// Federated identity token material returned by the IdP.
///
/// The `id_token` is the JWT that the pCloud "trusted-issuer" exchange
/// endpoint consumes. The `refresh_token` (when issued) is used by
/// [`IdpBroker::refresh`] to mint a new `id_token` without user interaction.
#[derive(Debug)]
pub struct IdpToken {
    /// OIDC ID token (JWT). Secret-wrapped to ensure redacted logging.
    pub id_token: SecretString,
    /// OIDC refresh token, when the IdP issues one. Optional by spec.
    pub refresh_token: Option<SecretString>,
    /// Absolute expiry of the ID token, derived from the `exp` claim.
    pub expires_at: SystemTime,
}

/// Errors surfaced by an [`IdpBroker`] implementation.
///
/// Variants are intentionally coarse for the scaffold; concrete implementations
/// may wrap richer causes via [`IdpError::Other`].
#[derive(Debug, thiserror::Error)]
pub enum IdpError {
    /// The IdP metadata document or JWKS could not be fetched or validated.
    #[error("IdP discovery failed: {0}")]
    Discovery(String),
    /// The authorization response failed state/nonce/signature validation.
    #[error("authorization validation failed: {0}")]
    Validation(String),
    /// Token exchange with the IdP failed.
    #[error("token exchange failed: {0}")]
    TokenExchange(String),
    /// The refresh token was rejected; the user must re-authenticate.
    #[error("refresh rejected; interactive re-auth required")]
    RefreshRejected,
    /// A required broker capability is not wired up. Surfaced by
    /// [`NullPcloudTokenExchanger`] and by [`UnimplementedBroker`] when a
    /// concrete implementor has not been registered yet.
    ///
    /// The message is static operator-facing guidance and never carries
    /// secret material.
    #[error("not configured: {0}")]
    NotConfigured(&'static str),
    /// Catch-all for implementation-specific errors.
    #[error("idp error: {0}")]
    Other(String),
}

/// Broker trait implemented per federation flow.
///
/// Implementors live in dedicated crates (`pcloud-idp-oidc`, `pcloud-idp-saml`,
/// ...) so that `pcloud-daemon` can depend on this scaffold without pulling
/// the full IdP client transitive graph.
///
/// # Contract
///
/// Implementors MUST uphold the following invariants:
///
/// 1. **No password grant.** The broker never accepts a user password. Only
///    code / refresh-token exchanges are supported.
/// 2. **JWKS pinning and issuer validation.** Before accepting an ID token,
///    implementors MUST validate its signature against a pinned JWKS and
///    enforce `iss`, `aud`, `exp`, and `nonce`.
/// 3. **State / nonce / PKCE verifier replay protection.** An
///    [`AuthChallenge`] is single-use — implementors MUST consume it on
///    `complete_authorization` and MUST NOT persist the PKCE verifier.
/// 4. **Secret hygiene.** ID tokens and refresh tokens remain in
///    [`SecretString`] for their entire lifetime inside the broker, are
///    never written to logs, and are never returned in error messages.
/// 5. **Thread safety.** Implementors are `Send + Sync` and callable
///    concurrently from the daemon's async runtime.
/// 6. **Failure discipline.** A rejected refresh MUST surface as
///    [`IdpError::RefreshRejected`] so the daemon can prompt interactive
///    re-auth instead of silently retrying.
pub trait IdpBroker: Send + Sync {
    /// Construct the authorization challenge the user's browser will follow.
    ///
    /// Implementations MUST:
    ///
    /// - fetch and pin the IdP metadata and JWKS per the design doc,
    /// - generate a cryptographically random PKCE verifier, state, and nonce,
    /// - bind the challenge to the configured `issuer` and `client_id`.
    ///
    /// # Errors
    ///
    /// - [`IdpError::Discovery`] if metadata or JWKS cannot be fetched.
    /// - [`IdpError::Other`] for implementation-specific failures.
    ///
    /// # Security
    ///
    /// The returned [`AuthChallenge`] carries the PKCE verifier as a
    /// [`SecretString`]; callers MUST NOT log the challenge.
    fn begin_authorization(&self, cfg: &IdpConfig) -> Result<AuthChallenge, IdpError>;

    /// Complete the authorization flow by exchanging `code` for an [`IdpToken`].
    ///
    /// `challenge` is consumed to prevent replay. Implementations MUST verify
    /// the returned ID token's signature against the pinned JWKS, validate the
    /// `iss`, `aud`, `exp`, and `nonce` claims, and zeroize the PKCE verifier.
    ///
    /// # Errors
    ///
    /// - [`IdpError::Validation`] if `state`, `nonce`, or signature is
    ///   incorrect.
    /// - [`IdpError::TokenExchange`] if the token endpoint rejects the code.
    ///
    /// # Security
    ///
    /// `code` is taken by value as a [`SecretString`] so it is zeroized after
    /// the exchange completes. The returned token material is likewise
    /// redacted.
    fn complete_authorization(
        &self,
        challenge: AuthChallenge,
        code: SecretString,
    ) -> Result<IdpToken, IdpError>;

    /// Refresh an [`IdpToken`] using its refresh token.
    ///
    /// Returns [`IdpError::RefreshRejected`] when the IdP rejects the refresh
    /// token and the daemon must prompt for interactive re-authentication.
    ///
    /// # Errors
    ///
    /// - [`IdpError::RefreshRejected`] if the refresh token is invalid or
    ///   revoked; the caller must drive the user back through
    ///   `begin_authorization`.
    /// - [`IdpError::TokenExchange`] for transient refresh failures.
    ///
    /// # Security
    ///
    /// The input token's refresh secret is read through [`SecretString`]
    /// access; the broker MUST NOT write it to disk in clear or include it
    /// in error diagnostics.
    fn refresh(&self, token: &IdpToken) -> Result<IdpToken, IdpError>;
}

/// No-op broker used by the scaffold and by tests that need a trait object.
///
/// Every method returns [`IdpError::NotConfigured`]. The default plugin
/// registry wires this broker when no concrete implementor has been
/// registered, so the daemon surfaces an honest typed error to callers
/// instead of panicking. This mirrors the same "fail closed with a typed
/// error" pattern used by [`NullPcloudTokenExchanger`].
#[derive(Debug, Default)]
pub struct UnimplementedBroker;

/// Human-readable operator guidance returned by [`UnimplementedBroker`]
/// when a concrete [`IdpBroker`] has not been wired in the plugin
/// registry. Kept as a module-level constant so callers can match on it.
pub const UNIMPLEMENTED_BROKER_MESSAGE: &str = "no IdP broker registered; set [auth.idp] and register a concrete broker \
     (e.g. OidcAuthorizationCodeBroker)";

impl IdpBroker for UnimplementedBroker {
    fn begin_authorization(&self, _cfg: &IdpConfig) -> Result<AuthChallenge, IdpError> {
        Err(IdpError::NotConfigured(UNIMPLEMENTED_BROKER_MESSAGE))
    }

    fn complete_authorization(
        &self,
        _challenge: AuthChallenge,
        _code: SecretString,
    ) -> Result<IdpToken, IdpError> {
        Err(IdpError::NotConfigured(UNIMPLEMENTED_BROKER_MESSAGE))
    }

    fn refresh(&self, _token: &IdpToken) -> Result<IdpToken, IdpError> {
        Err(IdpError::NotConfigured(UNIMPLEMENTED_BROKER_MESSAGE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip() {
        let cfg = IdpConfig {
            issuer: "https://corp.okta.com".into(),
            client_id: "pcloudc".into(),
            flow: IdpFlow::OidcAuthorizationCode,
            scopes: vec!["openid".into(), "email".into()],
        };
        assert_eq!(cfg.flow, IdpFlow::OidcAuthorizationCode);
        assert_eq!(cfg.scopes.len(), 2);
    }

    #[test]
    fn broker_is_object_safe() {
        let b: Box<dyn IdpBroker> = Box::new(UnimplementedBroker);
        // Exercise trait-object dispatch without triggering the unimplemented
        // body: simply drop the boxed trait object.
        drop(b);
    }

    #[test]
    fn unimplemented_broker_returns_not_configured_instead_of_panicking() {
        let b = UnimplementedBroker;
        let cfg = IdpConfig {
            issuer: "https://idp.example".into(),
            client_id: "pcloudc".into(),
            flow: IdpFlow::OidcAuthorizationCode,
            scopes: vec!["openid".into()],
        };
        let err = b.begin_authorization(&cfg).unwrap_err();
        assert!(matches!(err, IdpError::NotConfigured(_)));
        let code = SecretString::new("authz-code");
        let err = b
            .complete_authorization(
                AuthChallenge {
                    authorization_url: "https://idp.example/authorize?client_id=pcloudc".into(),
                    pkce_verifier: SecretString::new("verifier"),
                    state: "s".into(),
                    nonce: "n".into(),
                },
                code,
            )
            .unwrap_err();
        assert!(matches!(err, IdpError::NotConfigured(_)));
        let tok = IdpToken {
            id_token: SecretString::new("jwt"),
            refresh_token: None,
            expires_at: SystemTime::now(),
        };
        let err = b.refresh(&tok).unwrap_err();
        assert!(matches!(err, IdpError::NotConfigured(_)));
    }
}
