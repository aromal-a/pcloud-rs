//! OIDC discovery and JWKS fetch/cache with RS256-only ID token verification.
//!
//! Security posture:
//!
//! - **RS256 only.** The header `alg` is checked against an explicit allow-list
//!   containing only `RS256`. `alg=none` is rejected before any cryptographic
//!   work. Symmetric `HS*` algorithms are rejected because they require a
//!   pre-shared secret that a public client cannot hold safely.
//! - **Issuer pin.** Each verification asserts the `iss` claim matches the
//!   configured issuer byte-for-byte (post discovery normalisation).
//! - **Audience pin.** Each verification asserts the `aud` claim contains the
//!   configured client ID.
//! - **`exp` / `nbf` windows** are enforced by `jsonwebtoken::Validation`.
//! - **JWKS cache** has a 1-hour TTL. A `kid` miss triggers a forced refresh
//!   exactly once to handle key rotation without becoming a DoS amplifier.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use pcloud_observability::LockExt;
use serde::Deserialize;

use crate::IdpError;

/// Default JWKS cache TTL (1 hour).
pub(crate) const JWKS_TTL: Duration = Duration::from_secs(3600);

/// Subset of the OIDC discovery document the broker needs.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DiscoveryDocument {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

/// A single key in a JWKS document. Only RSA keys are usable by this broker;
/// other `kty` values are retained so the cache round-trips faithfully but
/// are filtered out at verification time.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // `alg` is retained for JWKS round-trip fidelity.
pub(crate) struct Jwk {
    #[serde(default)]
    pub kid: Option<String>,
    pub kty: String,
    #[serde(default)]
    pub alg: Option<String>,
    /// RSA modulus (base64url, unpadded).
    #[serde(default)]
    pub n: Option<String>,
    /// RSA exponent (base64url, unpadded).
    #[serde(default)]
    pub e: Option<String>,
}

/// Top-level JWKS document shape.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct JwkSet {
    pub keys: Vec<Jwk>,
}

/// Validated ID token claims surfaced to the broker. Only the fields the
/// broker acts on are modelled; unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // `iss`/`aud`/`nbf` are enforced by `jsonwebtoken::Validation`;
// they are surfaced here only for audit/logging hooks added later.
pub(crate) struct IdTokenClaims {
    pub iss: String,
    /// `aud` may be a string or an array of strings per RFC 7519; both are
    /// handled via `Audience`.
    pub aud: Audience,
    pub exp: i64,
    #[serde(default)]
    pub nbf: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

/// Audience deserializer that accepts either a single string or a list.
#[derive(Debug, Clone)]
pub(crate) struct Audience(#[allow(dead_code)] pub Vec<String>);

impl<'de> Deserialize<'de> for Audience {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            One(String),
            Many(Vec<String>),
        }
        Ok(match Raw::deserialize(d)? {
            Raw::One(s) => Audience(vec![s]),
            Raw::Many(v) => Audience(v),
        })
    }
}

/// JWKS cache with a TTL and a bounded forced-refresh policy. The cache stores
/// the discovery document alongside the JWKS so a single `refresh` path can
/// recover from rotations in either side.
///
/// # Poisoned-mutex policy
///
/// All `state.lock().expect(...)` sites in this module intentionally panic on
/// poison. Poison implies a panic occurred while the cache was mid-mutation —
/// at that point the cached JWKS/discovery may be half-written and cannot be
/// trusted for signature verification. Propagating the panic fails the
/// security-sensitive path closed (deny-by-default) rather than silently
/// operating on corrupt cache state. Tracked under pcloud-rs-f0r (LockExt
/// helper sweep) for a uniform `.lock_or_poisoned()` migration.
pub(crate) struct JwksCache {
    http: reqwest::blocking::Client,
    issuer: String,
    ttl: Duration,
    state: Mutex<CacheState>,
}

struct CacheState {
    discovery: Option<DiscoveryDocument>,
    keys: Vec<Jwk>,
    fetched_at: Option<Instant>,
}

impl JwksCache {
    pub(crate) fn new(http: reqwest::blocking::Client, issuer: String) -> Self {
        Self {
            http,
            issuer,
            ttl: JWKS_TTL,
            state: Mutex::new(CacheState {
                discovery: None,
                keys: Vec::new(),
                fetched_at: None,
            }),
        }
    }

    /// Force-refresh the discovery document and JWKS.
    pub(crate) fn refresh(&self) -> Result<DiscoveryDocument, IdpError> {
        let disc_url = format!(
            "{}/.well-known/openid-configuration",
            self.issuer.trim_end_matches('/')
        );
        let disc: DiscoveryDocument = self
            .http
            .get(&disc_url)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json())
            .map_err(|e| IdpError::Discovery(format!("fetch discovery: {e}")))?;
        if disc.issuer.trim_end_matches('/') != self.issuer.trim_end_matches('/') {
            return Err(IdpError::Discovery(format!(
                "issuer mismatch: configured {} vs metadata {}",
                self.issuer, disc.issuer
            )));
        }
        let jwks: JwkSet = self
            .http
            .get(&disc.jwks_uri)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json())
            .map_err(|e| IdpError::Discovery(format!("fetch jwks: {e}")))?;
        let mut state = self
            .state
            .lock_or_poisoned("idp::jwks::JwksCache::refresh");
        state.discovery = Some(disc.clone());
        state.keys = jwks.keys;
        state.fetched_at = Some(Instant::now());
        Ok(disc)
    }

    /// Return the cached discovery document, refreshing if absent or stale.
    pub(crate) fn discovery(&self) -> Result<DiscoveryDocument, IdpError> {
        {
            let state = self
                .state
                .lock_or_poisoned("idp::jwks::JwksCache::discovery");
            if let (Some(d), Some(t)) = (&state.discovery, state.fetched_at)
                && t.elapsed() < self.ttl
            {
                return Ok(d.clone());
            }
        }
        self.refresh()
    }

    /// Look up a JWK by `kid`, forcing a single refresh on miss.
    fn lookup_key(&self, kid: Option<&str>) -> Result<Jwk, IdpError> {
        {
            let state = self
                .state
                .lock_or_poisoned("idp::jwks::JwksCache::lookup_key");
            if let Some(k) = pick_key(&state.keys, kid) {
                return Ok(k.clone());
            }
        }
        self.refresh()?;
        let state = self
            .state
            .lock_or_poisoned("idp::jwks::JwksCache::lookup_key");
        pick_key(&state.keys, kid)
            .cloned()
            .ok_or_else(|| IdpError::Validation("no matching JWKS key".into()))
    }

    /// Verify an ID token: header.alg ∈ {RS256}, signature via the JWKS,
    /// `iss`/`aud`/`exp`/`nbf` enforced. Returns the parsed claims on success.
    pub(crate) fn verify_id_token(
        &self,
        id_token: &str,
        audience: &str,
    ) -> Result<IdTokenClaims, IdpError> {
        let header = decode_header(id_token)
            .map_err(|e| IdpError::Validation(format!("bad JWT header: {e}")))?;
        // RS256-only. Explicitly reject `alg=none` and any non-RS256 alg.
        if header.alg != Algorithm::RS256 {
            return Err(IdpError::Validation(format!(
                "rejected JWT alg {:?}; RS256 required",
                header.alg
            )));
        }
        let key = self.lookup_key(header.kid.as_deref())?;
        if key.kty != "RSA" {
            return Err(IdpError::Validation(format!(
                "JWKS key kty {} not RSA",
                key.kty
            )));
        }
        let (n, e) = match (key.n.as_deref(), key.e.as_deref()) {
            (Some(n), Some(e)) => (n, e),
            _ => return Err(IdpError::Validation("JWKS key missing n/e".into())),
        };
        let decoding_key = DecodingKey::from_rsa_components(n, e)
            .map_err(|e| IdpError::Validation(format!("invalid RSA JWK: {e}")))?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[self.issuer.trim_end_matches('/')]);
        validation.set_audience(&[audience]);
        // jsonwebtoken enforces exp by default; also validate nbf.
        validation.validate_nbf = true;
        let data = decode::<IdTokenClaims>(id_token, &decoding_key, &validation)
            .map_err(|e| IdpError::Validation(format!("id_token verify failed: {e}")))?;
        Ok(data.claims)
    }
}

fn pick_key<'a>(keys: &'a [Jwk], kid: Option<&str>) -> Option<&'a Jwk> {
    match kid {
        Some(k) => keys.iter().find(|j| j.kid.as_deref() == Some(k)),
        None => keys.first(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde_json::json;

    /// Forge a JWT with `alg=none` and a valid-looking payload, confirm the
    /// verifier refuses it before touching any key material.
    #[test]
    fn id_token_signature_rejected_if_alg_none() {
        let header = json!({ "alg": "none", "typ": "JWT" }).to_string();
        let payload = json!({
            "iss": "https://idp.example",
            "aud": "pcloudc",
            "exp": 9_999_999_999i64,
        })
        .to_string();
        let h = URL_SAFE_NO_PAD.encode(header);
        let p = URL_SAFE_NO_PAD.encode(payload);
        let jwt = format!("{h}.{p}.");

        let cache = JwksCache::new(
            reqwest::blocking::Client::builder()
                .build()
                .expect("client"),
            "https://idp.example".into(),
        );
        let err = cache
            .verify_id_token(&jwt, "pcloudc")
            .expect_err("alg=none must be rejected");
        // Rejection may come either from the explicit RS256 allow-list check
        // or from `jsonwebtoken`'s refusal to parse `alg=none` in the header.
        // Either way, we require an `IdpError::Validation` and the JWT must
        // never be treated as authentic.
        match err {
            IdpError::Validation(msg) => {
                let lower = msg.to_lowercase();
                assert!(
                    lower.contains("alg") || lower.contains("none") || lower.contains("header"),
                    "expected alg/none/header rejection, got: {msg}"
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn audience_deser_accepts_string_and_array() {
        #[derive(Deserialize)]
        struct W {
            aud: Audience,
        }
        let a: W = serde_json::from_str(r#"{"aud":"x"}"#).unwrap();
        assert_eq!(a.aud.0, vec!["x".to_string()]);
        let b: W = serde_json::from_str(r#"{"aud":["x","y"]}"#).unwrap();
        assert_eq!(b.aud.0, vec!["x".to_string(), "y".to_string()]);
    }
}
