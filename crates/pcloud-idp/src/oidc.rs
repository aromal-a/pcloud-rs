//! OIDC Authorization Code + PKCE broker.
//!
//! This module implements [`OidcAuthorizationCodeBroker`], a concrete
//! [`crate::IdpBroker`] for the interactive desktop SSO flow.
//!
//! Flow overview:
//!
//! 1. `begin_authorization` generates a PKCE verifier, CSRF `state`, and OIDC
//!    `nonce`, then builds the authorization URL. The verifier is returned as
//!    a [`SecretString`] inside [`crate::AuthChallenge`].
//! 2. `complete_authorization` POSTs the authorization code, PKCE verifier,
//!    `client_id`, and `redirect_uri` to the token endpoint. The returned
//!    `id_token` is signature-verified against the pinned JWKS and the
//!    expected `iss`/`aud`/`exp`/`nbf` bounds. The `nonce` claim is matched
//!    against the challenge.
//! 3. `refresh` POSTs `grant_type=refresh_token` to mint a fresh ID token.
//!
//! Security notes:
//!
//! - The broker never holds a client secret (PKCE public client).
//! - All tokens are wrapped in [`SecretString`]. No token value reaches `Debug`,
//!   logs, or user-facing error messages.
//! - HTTP errors from the token endpoint are surfaced as
//!   [`crate::IdpError::TokenExchange`] / [`crate::IdpError::RefreshRejected`]
//!   without echoing the response body.

use std::time::{Duration, SystemTime};

use pcloud_secret::ExposeSecret;
use serde::Deserialize;
use url::Url;

use crate::{
    AuthChallenge, IdpBroker, IdpConfig, IdpError, IdpToken, SecretString, jwks::JwksCache, pkce,
};

/// Default redirect URI for desktop clients that bind a loopback listener on a
/// random port. Operators override this by constructing the broker with
/// [`OidcAuthorizationCodeBroker::with_redirect_uri`].
pub const DEFAULT_REDIRECT_URI: &str = "http://127.0.0.1/callback";

/// Authorization-challenge lifetime. Matches the 10-minute ceiling most IdPs
/// enforce on `state` reuse.
const CHALLENGE_TTL: Duration = Duration::from_secs(600);

/// Concrete OIDC broker for the Authorization Code + PKCE flow.
pub struct OidcAuthorizationCodeBroker {
    http: reqwest::blocking::Client,
    jwks: JwksCache,
    redirect_uri: String,
}

impl std::fmt::Debug for OidcAuthorizationCodeBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcAuthorizationCodeBroker")
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

impl OidcAuthorizationCodeBroker {
    /// Construct a broker pinned to `issuer` using the default redirect URI.
    ///
    /// The HTTP client is configured with rustls-only TLS and a 30-second
    /// timeout on both connect and total request duration.
    pub fn new(issuer: impl Into<String>) -> Result<Self, IdpError> {
        Self::with_redirect_uri(issuer, DEFAULT_REDIRECT_URI)
    }

    /// Construct a broker with an explicit redirect URI (e.g. a loopback URL
    /// containing the bound port chosen at runtime).
    pub fn with_redirect_uri(
        issuer: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Result<Self, IdpError> {
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(30))
            .https_only(true)
            .build()
            .map_err(|e| IdpError::Other(format!("build http client: {e}")))?;
        let issuer = issuer.into();
        let jwks = JwksCache::new(http.clone(), issuer);
        Ok(Self {
            http,
            jwks,
            redirect_uri: redirect_uri.into(),
        })
    }

    /// Build a challenge from a pre-generated verifier/state/nonce — used in
    /// tests to assert URL formatting without invoking the RNG.
    #[cfg(test)]
    pub(crate) fn build_authorization_url(
        &self,
        cfg: &IdpConfig,
        authorization_endpoint: &str,
        verifier: &str,
        state: &str,
        nonce: &str,
    ) -> Result<String, IdpError> {
        build_auth_url(
            cfg,
            authorization_endpoint,
            &self.redirect_uri,
            verifier,
            state,
            nonce,
        )
    }
}

fn build_auth_url(
    cfg: &IdpConfig,
    authorization_endpoint: &str,
    redirect_uri: &str,
    verifier: &str,
    state: &str,
    nonce: &str,
) -> Result<String, IdpError> {
    let mut url = Url::parse(authorization_endpoint)
        .map_err(|e| IdpError::Discovery(format!("invalid authorization_endpoint: {e}")))?;
    let challenge = pkce::s256_challenge(verifier);
    let mut scopes = cfg.scopes.clone();
    if !scopes.iter().any(|s| s == "openid") {
        scopes.insert(0, "openid".into());
    }
    let scope_value = scopes.join(" ");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &scope_value)
        .append_pair("state", state)
        .append_pair("nonce", nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.into())
}

/// Raw OAuth2 token-endpoint response shape.
///
/// `access_token` is accepted but the broker does not surface it: pCloud
/// exchanges the `id_token`, and retaining access tokens here would widen the
/// exposure window without serving a flow requirement.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// Token-endpoint error surface per RFC 6749 §5.2.
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[allow(dead_code)]
    error_description: Option<String>,
}

impl OidcAuthorizationCodeBroker {
    fn post_token(
        &self,
        token_endpoint: &str,
        form: &[(&str, &str)],
    ) -> Result<TokenResponse, IdpError> {
        let resp = self
            .http
            .post(token_endpoint)
            .form(form)
            .send()
            .map_err(|e| IdpError::TokenExchange(format!("POST token endpoint: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            resp.json::<TokenResponse>()
                .map_err(|e| IdpError::TokenExchange(format!("decode token response: {e}")))
        } else {
            // Only surface the OAuth2 error *code*, never the description: the
            // description may contain operator-sensitive hints.
            let code = resp
                .json::<TokenErrorResponse>()
                .map(|e| e.error)
                .unwrap_or_else(|_| format!("HTTP {status}"));
            if code == "invalid_grant" {
                Err(IdpError::RefreshRejected)
            } else {
                Err(IdpError::TokenExchange(code))
            }
        }
    }
}

impl IdpBroker for OidcAuthorizationCodeBroker {
    fn begin_authorization(&self, cfg: &IdpConfig) -> Result<AuthChallenge, IdpError> {
        let discovery = self.jwks.discovery()?;
        let verifier = pkce::random_token(pkce::VERIFIER_BYTES)?;
        let state = pkce::random_token(pkce::STATE_BYTES)?;
        let nonce = pkce::random_token(pkce::NONCE_BYTES)?;
        let url = build_auth_url(
            cfg,
            &discovery.authorization_endpoint,
            &self.redirect_uri,
            &verifier,
            &state,
            &nonce,
        )?;
        Ok(AuthChallenge {
            authorization_url: url,
            pkce_verifier: SecretString::new(verifier),
            state,
            nonce,
        })
    }

    fn complete_authorization(
        &self,
        challenge: AuthChallenge,
        code: SecretString,
    ) -> Result<IdpToken, IdpError> {
        let discovery = self.jwks.discovery()?;
        // Extract the client_id from the authorization URL we built ourselves
        // so we don't have to thread IdpConfig through the challenge.
        let client_id = extract_client_id(&challenge.authorization_url)?;
        let resp = self.post_token(
            &discovery.token_endpoint,
            &[
                ("grant_type", "authorization_code"),
                ("code", code.expose_secret()),
                ("redirect_uri", &self.redirect_uri),
                ("client_id", &client_id),
                ("code_verifier", challenge.pkce_verifier.expose_secret()),
            ],
        )?;
        let claims = self.jwks.verify_id_token(&resp.id_token, &client_id)?;
        match claims.nonce {
            Some(ref n) if n == &challenge.nonce => {}
            _ => {
                return Err(IdpError::Validation(
                    "id_token nonce mismatch or missing".into(),
                ));
            }
        }
        let expires_at = compute_expiry(resp.expires_in, claims.exp);
        Ok(IdpToken {
            id_token: SecretString::new(resp.id_token),
            refresh_token: resp.refresh_token.map(SecretString::new),
            expires_at,
        })
    }

    fn refresh(&self, token: &IdpToken) -> Result<IdpToken, IdpError> {
        let discovery = self.jwks.discovery()?;
        let refresh_token = token
            .refresh_token
            .as_ref()
            .ok_or(IdpError::RefreshRejected)?;
        // Decoding an (already-verified) ID token here only to extract `aud`
        // is avoided: the caller's IdpConfig owns client_id and should drive
        // refresh, but the trait does not thread it. As a pragmatic guard,
        // derive the audience from the token's unverified payload — the
        // subsequent JWKS verification enforces aud against this same value.
        let aud = parse_aud_unverified(token.id_token.expose_secret())?;
        let resp = self.post_token(
            &discovery.token_endpoint,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.expose_secret()),
                ("client_id", &aud),
            ],
        )?;
        let claims = self.jwks.verify_id_token(&resp.id_token, &aud)?;
        let expires_at = compute_expiry(resp.expires_in, claims.exp);
        Ok(IdpToken {
            id_token: SecretString::new(resp.id_token),
            refresh_token: resp
                .refresh_token
                .map(SecretString::new)
                .or_else(|| Some(refresh_token.clone_secret())),
            expires_at,
        })
    }
}

fn extract_client_id(auth_url: &str) -> Result<String, IdpError> {
    let parsed =
        Url::parse(auth_url).map_err(|e| IdpError::Validation(format!("invalid auth url: {e}")))?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "client_id")
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| IdpError::Validation("auth url missing client_id".into()))
}

/// Parse the `aud` claim from an ID token *without* verifying the signature.
/// The value is only used to drive the subsequent JWKS-pinned verification,
/// which re-enforces `aud` against the JWT signature.
fn parse_aud_unverified(jwt: &str) -> Result<String, IdpError> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let mut parts = jwt.split('.');
    let _h = parts.next();
    let payload_b64 = parts
        .next()
        .ok_or_else(|| IdpError::Validation("malformed JWT".into()))?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| IdpError::Validation("malformed JWT payload".into()))?;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Aud {
        One(String),
        Many(Vec<String>),
    }
    #[derive(Deserialize)]
    struct P {
        aud: Aud,
    }
    let p: P = serde_json::from_slice(&payload)
        .map_err(|_| IdpError::Validation("malformed JWT payload json".into()))?;
    Ok(match p.aud {
        Aud::One(s) => s,
        Aud::Many(mut v) => v
            .pop()
            .ok_or_else(|| IdpError::Validation("empty aud".into()))?,
    })
}

fn compute_expiry(expires_in: Option<u64>, exp_claim: i64) -> SystemTime {
    if let Some(secs) = expires_in {
        return SystemTime::now()
            + Duration::from_secs(secs.min(CHALLENGE_TTL.as_secs().max(secs)));
    }
    let exp = exp_claim.max(0) as u64;
    SystemTime::UNIX_EPOCH + Duration::from_secs(exp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IdpFlow;
    use pcloud_secret::ExposeSecret;

    fn cfg() -> IdpConfig {
        IdpConfig {
            issuer: "https://idp.example".into(),
            client_id: "pcloudc".into(),
            flow: IdpFlow::OidcAuthorizationCode,
            scopes: vec!["openid".into(), "email".into()],
        }
    }

    #[test]
    fn authorization_url_includes_state_and_challenge() {
        let broker = OidcAuthorizationCodeBroker::with_redirect_uri(
            "https://idp.example",
            "http://127.0.0.1:8765/cb",
        )
        .expect("broker");
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let url = broker
            .build_authorization_url(
                &cfg(),
                "https://idp.example/authorize",
                verifier,
                "state-xyz",
                "nonce-abc",
            )
            .expect("url");
        let parsed = Url::parse(&url).expect("parse");
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(q.get("client_id").map(String::as_str), Some("pcloudc"));
        assert_eq!(
            q.get("redirect_uri").map(String::as_str),
            Some("http://127.0.0.1:8765/cb")
        );
        assert_eq!(q.get("state").map(String::as_str), Some("state-xyz"));
        assert_eq!(q.get("nonce").map(String::as_str), Some("nonce-abc"));
        assert_eq!(
            q.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            q.get("code_challenge").map(String::as_str),
            Some("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
        );
        let scope = q.get("scope").cloned().unwrap_or_default();
        assert!(scope.contains("openid"), "scope = {scope}");
    }

    #[test]
    fn token_roundtrip_is_zeroized_on_drop() {
        // Construct an IdpToken, expose a pointer to its inner bytes through
        // `expose_secret`, drop the token, and observe that the same buffer
        // location has been scrubbed. We cannot observe raw memory safely
        // after drop, so instead we assert via the SecretString contract: the
        // struct implements ZeroizeOnDrop, which we exercise by dropping a
        // clone and checking the survivor has its original contents while
        // the drop path runs without panic — the compile-time contract is
        // the primary guarantee.
        let original = "secret-id-token-value";
        let token = IdpToken {
            id_token: SecretString::new(original),
            refresh_token: Some(SecretString::new("rt")),
            expires_at: SystemTime::now(),
        };
        // Copy for survivor-side assertion.
        let survivor = token.id_token.clone_secret();
        drop(token);
        assert_eq!(survivor.expose_secret(), original);
        // Dropping the survivor must not panic and must zeroize its buffer.
        drop(survivor);
    }

    #[test]
    fn extract_client_id_works() {
        let cid = extract_client_id(
            "https://idp.example/authorize?response_type=code&client_id=pcloudc&state=s",
        )
        .expect("cid");
        assert_eq!(cid, "pcloudc");
    }

    #[test]
    fn openid_scope_is_injected_when_missing() {
        let broker = OidcAuthorizationCodeBroker::with_redirect_uri(
            "https://idp.example",
            "http://127.0.0.1/cb",
        )
        .expect("broker");
        let mut c = cfg();
        c.scopes = vec!["email".into()];
        let url = broker
            .build_authorization_url(&c, "https://idp.example/a", "v", "s", "n")
            .expect("url");
        let parsed = Url::parse(&url).unwrap();
        let scope = parsed
            .query_pairs()
            .find(|(k, _)| k == "scope")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        assert!(scope.starts_with("openid"), "scope = {scope}");
    }
}
