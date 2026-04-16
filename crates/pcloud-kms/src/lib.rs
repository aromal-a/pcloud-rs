#![deny(missing_docs)]
//! # pcloud-kms — Enterprise KMS / HSM key-wrapping integration
//!
//! This crate pulls the crypto-folder data-encryption-key (DEK) wrapping
//! operation out of the client and into a managed Key Management Service or
//! hardware HSM. See [`docs/enterprise/kms.md`] at the workspace root for
//! the full design narrative, threat model, IAM rules, and operator
//! configuration schema.
//!
//! ## Feature gating
//!
//! Real provider implementations live behind Cargo features so the default
//! workspace build stays light:
//!
//! - `aws`   — AWS KMS (`aws-sdk-kms` + `tokio`).
//! - `vault` — HashiCorp Vault Transit (`reqwest` blocking + `base64`).
//!
//! `Pkcs11Hsm` is intentionally left as a `NotImplemented` stub — a real
//! PKCS#11 backend cannot be validated without a working HSM.
//!
//! ## Security rules
//!
//! - plaintext DEKs never leave [`PlaintextDek`] (zeroized on drop);
//! - auth credentials are wrapped in `pcloud_secret::SecretString`;
//! - Debug formatters redact all secret material;
//! - providers never log plaintext DEKs, wrapped DEKs, or credentials.
//!
//! [`docs/enterprise/kms.md`]: ../../../docs/enterprise/kms.md

#![forbid(unsafe_code)]
#![allow(clippy::pedantic)]

use core::fmt;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use zeroize::Zeroize;

/// Errors returned by any [`KmsProvider`] implementation.
///
/// The variants are intentionally coarse. Providers map vendor-specific
/// error codes into this taxonomy so the caller (`pcloud-crypto`) can make
/// uniform decisions — retry, escalate, fall back to the offline cache, or
/// drop to read-only mode.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum KmsError {
    /// The KMS endpoint could not be reached. Transient.
    ///
    /// On this error, the daemon should consult the offline cache
    /// (see `docs/enterprise/kms.md §4`) before failing.
    #[error("KMS unreachable: {0}")]
    Unreachable(String),

    /// The caller's credentials were rejected. Non-transient without
    /// operator intervention.
    #[error("KMS authentication failed")]
    AuthFailed,

    /// The KMS refused the operation under its key policy / IAM.
    ///
    /// Example: device policy allows `Encrypt` but not `Decrypt`.
    #[error("KMS policy denied operation")]
    PolicyDenied,

    /// The requested key identifier does not exist or is not accessible.
    #[error("KMS key not found: {0}")]
    KeyNotFound(String),

    /// The KMS returned a response that failed integrity / format checks.
    ///
    /// This is treated as fatal; do not retry blindly.
    #[error("KMS returned a malformed or untrusted response")]
    Malformed,

    /// The provider has not been implemented yet.
    #[error("KMS provider `{0}` is not yet implemented in this build")]
    NotImplemented(&'static str),

    /// Catch-all for vendor errors that do not map cleanly to the above.
    #[error("KMS error: {0}")]
    Other(String),
}

/// An opaque identifier for the wrapping key inside the KMS.
///
/// The string form is provider-specific:
///
/// - AWS KMS: `arn:aws:kms:<region>:<acct>:key/<uuid>`
/// - HashiCorp Vault: `transit/keys/<name>`
/// - PKCS#11: `slot=<n>;label=<key-label>`
///
/// The content is **not** a secret and may appear in logs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KeyId(pub String);

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A wrapped (ciphertext) data-encryption-key as returned by the KMS.
///
/// This blob is what gets stored in pCloud crypto-folder metadata. It is
/// opaque: only the KMS that produced it can unwrap it. The byte layout
/// is provider-defined.
///
/// Not secret — it is ciphertext — so no zeroization is required.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WrappedDek(pub Vec<u8>);

/// A plaintext data-encryption-key.
///
/// Lives in memory only long enough to encrypt a sector. This wrapper
/// zeroizes on drop; callers MUST NOT log it or persist it.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct PlaintextDek(pub Vec<u8>);

impl PlaintextDek {
    /// Borrow the plaintext bytes. The returned slice MUST be treated as
    /// sensitive — never log it, never persist it.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// Audit-visible duplication of the plaintext DEK.
    ///
    /// Used by [`KmsProvider::unwrap_cached`] to hand callers an owned copy
    /// of a cached plaintext DEK without handing out the cache's interior.
    #[must_use]
    pub fn clone_secret(&self) -> Self {
        Self(self.0.clone())
    }
}

impl fmt::Debug for PlaintextDek {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlaintextDek")
            .field("len", &self.0.len())
            .field("bytes", &"<redacted>")
            .finish()
    }
}

/// Default TTL for cached unwrapped DEKs.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(300);

/// The operations a KMS-backed wrapping provider must support.
///
/// The trait is deliberately tiny: the client never asks the KMS to
/// generate or export the wrapping key. The wrapping key stays inside
/// the KMS / HSM for its entire lifetime.
pub trait KmsProvider: Send + Sync {
    /// Human-readable provider name, for logs and telemetry.
    fn name(&self) -> &'static str;

    /// Wrap a plaintext DEK using the provider's managed wrapping key.
    ///
    /// `context` is an optional associated-data string (folder id, device
    /// id) bound to the ciphertext by AEAD-capable providers. Providers
    /// that do not support it must ignore it.
    fn encrypt_dek(
        &self,
        key_id: &KeyId,
        dek: &PlaintextDek,
        context: Option<&str>,
    ) -> Result<WrappedDek, KmsError>;

    /// Unwrap a previously wrapped DEK.
    ///
    /// `context` MUST match the value passed to [`Self::encrypt_dek`].
    fn decrypt_dek(
        &self,
        key_id: &KeyId,
        wrapped: &WrappedDek,
        context: Option<&str>,
    ) -> Result<PlaintextDek, KmsError>;

    /// Lightweight liveness probe.
    fn health_check(&self) -> Result<(), KmsError>;

    /// Cache-backed unwrap.
    ///
    /// Returns a cached plaintext DEK if the wrapped blob has been
    /// unwrapped within the TTL. Otherwise calls [`Self::decrypt_dek`]
    /// and memoizes the result. The cache is process-local, guarded by a
    /// `Mutex`, and keyed on `(provider_name, key_id, wrapped_bytes,
    /// context)`.
    ///
    /// The cache is **not** persisted — it lives only inside this process.
    /// Cache entries are zeroized on eviction because `PlaintextDek`
    /// zeroizes on drop.
    fn unwrap_cached(
        &self,
        key_id: &KeyId,
        wrapped: &WrappedDek,
        context: Option<&str>,
        ttl: Duration,
    ) -> Result<PlaintextDek, KmsError> {
        let key = CacheKey {
            provider: self.name(),
            key_id: key_id.0.clone(),
            wrapped: wrapped.0.clone(),
            context: context.map(str::to_owned),
        };
        if let Some(dek) = cache_lookup(&key, ttl, Instant::now()) {
            return Ok(dek);
        }
        let fresh = self.decrypt_dek(key_id, wrapped, context)?;
        let clone = fresh.clone_secret();
        cache_store(key, fresh, Instant::now());
        Ok(clone)
    }
}

// -------------------------------------------------------------------------
// Process-local plaintext DEK cache.
// -------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    provider: &'static str,
    key_id: String,
    wrapped: Vec<u8>,
    context: Option<String>,
}

struct CacheEntry {
    dek: PlaintextDek,
    inserted: Instant,
}

fn cache() -> &'static Mutex<HashMap<CacheKey, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_lookup(key: &CacheKey, ttl: Duration, now: Instant) -> Option<PlaintextDek> {
    let mut guard = cache().lock().ok()?;
    let entry = guard.get(key)?;
    if now.duration_since(entry.inserted) <= ttl {
        return Some(entry.dek.clone_secret());
    }
    // Expired — evict (drop zeroizes).
    guard.remove(key);
    None
}

fn cache_store(key: CacheKey, dek: PlaintextDek, now: Instant) {
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, CacheEntry { dek, inserted: now });
    }
}

/// Evict a specific `(provider, key_id, wrapped, context)` entry from the
/// process-local DEK cache.
///
/// Called by [`CryptoShell::stop`](../pcloud_crypto/struct.CryptoShell.html#method.stop)
/// so that locking the crypto shell drops any resident plaintext DEK
/// without waiting for the TTL to expire. The `PlaintextDek` zeroizes on
/// drop, so removing the entry is sufficient.
///
/// Returns `true` if an entry was actually removed, `false` if the entry
/// was not present (already evicted by TTL, or never cached).
pub fn evict_cached_dek(
    provider: &'static str,
    key_id: &KeyId,
    wrapped: &WrappedDek,
    context: Option<&str>,
) -> bool {
    let key = CacheKey {
        provider,
        key_id: key_id.0.clone(),
        wrapped: wrapped.0.clone(),
        context: context.map(str::to_owned),
    };
    if let Ok(mut guard) = cache().lock() {
        return guard.remove(&key).is_some();
    }
    false
}

#[cfg(test)]
fn cache_insert_at(key: CacheKey, dek: PlaintextDek, inserted: Instant) {
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, CacheEntry { dek, inserted });
    }
}

// -------------------------------------------------------------------------
// NullKms — safe default when no KMS is configured.
// -------------------------------------------------------------------------

/// No-op provider used when KMS integration is disabled.
///
/// `NullKms` is not a fallback for a failed KMS — it is the explicit
/// "we are not using a KMS" mode. Every real operation returns
/// [`KmsError::NotImplemented`] so that a misconfigured deployment fails
/// loudly rather than silently dropping to the insecure local-Argon2 path.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullKms;

impl KmsProvider for NullKms {
    fn name(&self) -> &'static str {
        "null"
    }

    fn encrypt_dek(
        &self,
        _key_id: &KeyId,
        _dek: &PlaintextDek,
        _context: Option<&str>,
    ) -> Result<WrappedDek, KmsError> {
        Err(KmsError::NotImplemented("null"))
    }

    fn decrypt_dek(
        &self,
        _key_id: &KeyId,
        _wrapped: &WrappedDek,
        _context: Option<&str>,
    ) -> Result<PlaintextDek, KmsError> {
        Err(KmsError::NotImplemented("null"))
    }

    fn health_check(&self) -> Result<(), KmsError> {
        Ok(())
    }
}

// -------------------------------------------------------------------------
// AWS KMS provider.
// -------------------------------------------------------------------------

/// AWS KMS provider.
///
/// Builds an `aws-sdk-kms` client against the default credential provider
/// chain (IMDSv2, env, SSO, SDK config). Credentials are **never** read
/// from the pcloud-rs config file.
///
/// ### Async bridge
///
/// `aws-sdk-kms` is async. [`KmsProvider`] is sync because the rest of the
/// daemon talks to it from blocking crypto paths. We bridge as follows:
///
/// - If called outside any tokio runtime
///   ([`tokio::runtime::Handle::try_current`] fails), we spin up a local
///   current-thread runtime per call. Call rate is very low (once per
///   folder open / close), so runtime construction cost is not on any
///   hot path.
/// - If called from inside a tokio runtime, we offload to a fresh OS
///   thread that owns its own current-thread runtime. This avoids the
///   reentrancy deadlock of calling `Handle::block_on` on the same
///   current-thread runtime, and avoids requiring the
///   `rt-multi-thread` tokio feature (`block_in_place` would demand it).
#[cfg(feature = "aws")]
pub struct AwsKms {
    client: OnceLock<aws_sdk_kms::Client>,
    region: String,
}

#[cfg(feature = "aws")]
impl fmt::Debug for AwsKms {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsKms")
            .field("region", &self.region)
            .field("client", &"<aws-sdk-kms client>")
            .finish()
    }
}

#[cfg(feature = "aws")]
impl AwsKms {
    /// Construct a new AWS KMS provider bound to the given region.
    ///
    /// The underlying SDK client is created lazily on first use so
    /// construction does no network I/O.
    #[must_use]
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            client: OnceLock::new(),
            region: region.into(),
        }
    }

    fn client(&self) -> &aws_sdk_kms::Client {
        self.client.get_or_init(|| {
            let region = aws_config::Region::new(self.region.clone());
            let cfg = run_async(async move {
                aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .region(region)
                    .load()
                    .await
            });
            aws_sdk_kms::Client::new(&cfg)
        })
    }

    fn encryption_context(
        context: Option<&str>,
    ) -> Option<std::collections::HashMap<String, String>> {
        context.map(|c| {
            let mut m = std::collections::HashMap::new();
            m.insert("pcloud_context".to_string(), c.to_string());
            m
        })
    }
}

#[cfg(feature = "aws")]
fn run_async<F>(fut: F) -> F::Output
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        // Inside an existing runtime: offload to a fresh OS thread with
        // its own current-thread runtime so we never re-enter the caller's.
        std::thread::scope(|s| {
            s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build local tokio runtime");
                rt.block_on(fut)
            })
            .join()
            .expect("kms async bridge thread panicked")
        })
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build local tokio runtime");
        rt.block_on(fut)
    }
}

#[cfg(feature = "aws")]
fn map_aws_err<E: std::fmt::Display>(e: E) -> KmsError {
    // Best-effort classification. The SDK's error taxonomy is rich but
    // opaque behind generics; we stringify and inspect for the common
    // cases rather than plumbing every SdkError variant.
    let s = e.to_string();
    let lower = s.to_ascii_lowercase();
    if lower.contains("notfound") || lower.contains("not found") {
        KmsError::KeyNotFound(s)
    } else if lower.contains("accessdenied") || lower.contains("unauthorized") {
        KmsError::PolicyDenied
    } else if lower.contains("invalidsignature") || lower.contains("signature") {
        KmsError::AuthFailed
    } else if lower.contains("timeout") || lower.contains("dispatch") || lower.contains("connect") {
        KmsError::Unreachable(s)
    } else {
        KmsError::Other(s)
    }
}

#[cfg(feature = "aws")]
impl KmsProvider for AwsKms {
    fn name(&self) -> &'static str {
        "aws-kms"
    }

    fn encrypt_dek(
        &self,
        key_id: &KeyId,
        dek: &PlaintextDek,
        context: Option<&str>,
    ) -> Result<WrappedDek, KmsError> {
        let client = self.client().clone();
        let arn = key_id.0.clone();
        let plaintext = aws_sdk_kms::primitives::Blob::new(dek.expose().to_vec());
        let enc_ctx = Self::encryption_context(context);
        let out = run_async(async move {
            let mut req = client.encrypt().key_id(arn).plaintext(plaintext);
            if let Some(ctx) = enc_ctx {
                req = req.set_encryption_context(Some(ctx));
            }
            req.send().await
        })
        .map_err(map_aws_err)?;

        let blob = out.ciphertext_blob.ok_or(KmsError::Malformed)?.into_inner();
        Ok(WrappedDek(blob))
    }

    fn decrypt_dek(
        &self,
        key_id: &KeyId,
        wrapped: &WrappedDek,
        context: Option<&str>,
    ) -> Result<PlaintextDek, KmsError> {
        let client = self.client().clone();
        let arn = key_id.0.clone();
        let ct = aws_sdk_kms::primitives::Blob::new(wrapped.0.clone());
        let enc_ctx = Self::encryption_context(context);
        let out = run_async(async move {
            let mut req = client.decrypt().key_id(arn).ciphertext_blob(ct);
            if let Some(ctx) = enc_ctx {
                req = req.set_encryption_context(Some(ctx));
            }
            req.send().await
        })
        .map_err(map_aws_err)?;

        let pt = out.plaintext.ok_or(KmsError::Malformed)?.into_inner();
        Ok(PlaintextDek(pt))
    }

    fn health_check(&self) -> Result<(), KmsError> {
        // Lightweight call — list_keys with a limit of 1. Avoids an
        // Encrypt/Decrypt audit entry on the hot path.
        let client = self.client().clone();
        run_async(async move { client.list_keys().limit(1).send().await })
            .map(|_| ())
            .map_err(map_aws_err)
    }
}

// -------------------------------------------------------------------------
// HashiCorp Vault Transit provider.
// -------------------------------------------------------------------------

/// HashiCorp Vault Transit provider.
///
/// Talks to `/v1/transit/encrypt/<key>` and `/v1/transit/decrypt/<key>` on
/// a Vault server. The token is held in `pcloud_secret::SecretString`
/// and sent in the `X-Vault-Token` header on each request.
///
/// Uses blocking `reqwest` to keep the [`KmsProvider`] trait sync. Call
/// frequency is low enough (once per folder open) that this is not a
/// performance concern.
#[cfg(feature = "vault")]
pub struct HashicorpVault {
    vault_url: String,
    token: pcloud_secret::secret_string::SecretString,
    transit_key: String,
    client: reqwest::blocking::Client,
}

#[cfg(feature = "vault")]
impl fmt::Debug for HashicorpVault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HashicorpVault")
            .field("vault_url", &self.vault_url)
            .field("transit_key", &self.transit_key)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[cfg(feature = "vault")]
impl HashicorpVault {
    /// Construct a new Vault Transit provider.
    ///
    /// `vault_url` is the base URL (e.g. `https://vault.example.com:8200`),
    /// `token` is the Vault auth token, and `transit_key` is the Transit
    /// engine key name (not path).
    ///
    /// # Errors
    ///
    /// Returns [`KmsError::Other`] if the blocking `reqwest` client
    /// cannot be constructed.
    pub fn new(
        vault_url: impl Into<String>,
        token: pcloud_secret::secret_string::SecretString,
        transit_key: impl Into<String>,
    ) -> Result<Self, KmsError> {
        let client = reqwest::blocking::Client::builder()
            .use_rustls_tls()
            .build()
            .map_err(|e| KmsError::Other(e.to_string()))?;
        Ok(Self {
            vault_url: vault_url.into().trim_end_matches('/').to_string(),
            token,
            transit_key: transit_key.into(),
            client,
        })
    }

    fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, KmsError> {
        use pcloud_secret::ExposeSecret;
        let url = format!("{}{}", self.vault_url, path);
        let resp = self
            .client
            .post(&url)
            .header("X-Vault-Token", self.token.expose_secret())
            .json(body)
            .send()
            .map_err(|e| {
                if e.is_timeout() || e.is_connect() {
                    KmsError::Unreachable(e.to_string())
                } else {
                    KmsError::Other(e.to_string())
                }
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(KmsError::AuthFailed);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(KmsError::KeyNotFound(self.transit_key.clone()));
        }
        if status != reqwest::StatusCode::OK {
            return Err(KmsError::Other(format!("vault HTTP {}", status.as_u16())));
        }
        resp.json::<serde_json::Value>()
            .map_err(|_| KmsError::Malformed)
    }
}

#[cfg(feature = "vault")]
impl KmsProvider for HashicorpVault {
    fn name(&self) -> &'static str {
        "hashicorp-vault"
    }

    fn encrypt_dek(
        &self,
        _key_id: &KeyId,
        dek: &PlaintextDek,
        context: Option<&str>,
    ) -> Result<WrappedDek, KmsError> {
        use base64::Engine;
        let pt_b64 = base64::engine::general_purpose::STANDARD.encode(dek.expose());
        let mut body = serde_json::json!({ "plaintext": pt_b64 });
        if let Some(ctx) = context {
            let ctx_b64 = base64::engine::general_purpose::STANDARD.encode(ctx.as_bytes());
            body["context"] = serde_json::Value::String(ctx_b64);
        }
        let path = format!("/v1/transit/encrypt/{}", self.transit_key);
        let resp = self.post_json(&path, &body)?;
        let ct = resp
            .get("data")
            .and_then(|d| d.get("ciphertext"))
            .and_then(|v| v.as_str())
            .ok_or(KmsError::Malformed)?;
        Ok(WrappedDek(ct.as_bytes().to_vec()))
    }

    fn decrypt_dek(
        &self,
        _key_id: &KeyId,
        wrapped: &WrappedDek,
        context: Option<&str>,
    ) -> Result<PlaintextDek, KmsError> {
        use base64::Engine;
        let ciphertext = std::str::from_utf8(&wrapped.0).map_err(|_| KmsError::Malformed)?;
        let mut body = serde_json::json!({ "ciphertext": ciphertext });
        if let Some(ctx) = context {
            let ctx_b64 = base64::engine::general_purpose::STANDARD.encode(ctx.as_bytes());
            body["context"] = serde_json::Value::String(ctx_b64);
        }
        let path = format!("/v1/transit/decrypt/{}", self.transit_key);
        let resp = self.post_json(&path, &body)?;
        let pt_b64 = resp
            .get("data")
            .and_then(|d| d.get("plaintext"))
            .and_then(|v| v.as_str())
            .ok_or(KmsError::Malformed)?;
        let pt = base64::engine::general_purpose::STANDARD
            .decode(pt_b64)
            .map_err(|_| KmsError::Malformed)?;
        Ok(PlaintextDek(pt))
    }

    fn health_check(&self) -> Result<(), KmsError> {
        let url = format!("{}/v1/sys/health", self.vault_url);
        let resp = self.client.get(&url).send().map_err(|e| {
            if e.is_timeout() || e.is_connect() {
                KmsError::Unreachable(e.to_string())
            } else {
                KmsError::Other(e.to_string())
            }
        })?;
        // Vault returns 200 for active, 429 for standby; both mean alive.
        if resp.status().is_success() || resp.status().as_u16() == 429 {
            Ok(())
        } else {
            Err(KmsError::Unreachable(format!(
                "vault health HTTP {}",
                resp.status().as_u16()
            )))
        }
    }
}

// -------------------------------------------------------------------------
// PKCS#11 HSM provider.
//
// Behind the `pkcs11` Cargo feature. When the feature is off, the crate
// still exports a `Pkcs11Hsm` type whose constructor returns
// `KmsError::NotImplemented("pkcs11")` and points the operator at the
// feature flag — so a misconfigured build fails loudly instead of
// silently disabling HSM integration.
// -------------------------------------------------------------------------

#[cfg(not(feature = "pkcs11"))]
mod pkcs11_stub {
    use super::{KeyId, KmsError, KmsProvider, PlaintextDek, WrappedDek};

    /// PKCS#11 HSM provider (disabled in this build — enable the `pkcs11`
    /// Cargo feature to compile real HSM support via `cryptoki`).
    #[derive(Debug)]
    pub struct Pkcs11Hsm {
        _key_id: KeyId,
    }

    impl Pkcs11Hsm {
        /// Attempt to construct a PKCS#11 HSM provider.
        ///
        /// # Errors
        ///
        /// Always returns [`KmsError::NotImplemented`] when the `pkcs11`
        /// feature is disabled. The error message tells the operator
        /// which feature to enable.
        pub fn new(key_id: KeyId) -> Result<Self, KmsError> {
            let _ = key_id;
            Err(KmsError::NotImplemented(
                "pkcs11 (rebuild with --features pkcs11)",
            ))
        }

        /// Attempt to construct a PKCS#11 HSM provider from a vendor
        /// module path (e.g. `/usr/lib/softhsm/libsofthsm2.so`).
        ///
        /// # Errors
        ///
        /// Always returns [`KmsError::NotImplemented`] when the `pkcs11`
        /// feature is disabled.
        pub fn new_from_module(
            module_path: &str,
            slot_id: u64,
            pin: pcloud_secret::secret_string::SecretString,
            key_label: &str,
        ) -> Result<Self, KmsError> {
            let _ = (module_path, slot_id, pin, key_label);
            Err(KmsError::NotImplemented(
                "pkcs11 (rebuild with --features pkcs11)",
            ))
        }
    }

    impl KmsProvider for Pkcs11Hsm {
        fn name(&self) -> &'static str {
            "pkcs11"
        }
        fn encrypt_dek(
            &self,
            _key_id: &KeyId,
            _dek: &PlaintextDek,
            _context: Option<&str>,
        ) -> Result<WrappedDek, KmsError> {
            Err(KmsError::NotImplemented(
                "pkcs11 (rebuild with --features pkcs11)",
            ))
        }
        fn decrypt_dek(
            &self,
            _key_id: &KeyId,
            _wrapped: &WrappedDek,
            _context: Option<&str>,
        ) -> Result<PlaintextDek, KmsError> {
            Err(KmsError::NotImplemented(
                "pkcs11 (rebuild with --features pkcs11)",
            ))
        }
        fn health_check(&self) -> Result<(), KmsError> {
            Err(KmsError::NotImplemented(
                "pkcs11 (rebuild with --features pkcs11)",
            ))
        }
    }
}

#[cfg(not(feature = "pkcs11"))]
pub use pkcs11_stub::Pkcs11Hsm;

#[cfg(feature = "pkcs11")]
mod pkcs11_real {
    //! Real PKCS#11 provider.
    //!
    //! Key properties:
    //!
    //! - the wrapping key **never leaves the HSM** — every encrypt/decrypt
    //!   is delegated to the device via C_EncryptInit/C_Encrypt and
    //!   C_DecryptInit/C_Decrypt;
    //! - the user PIN is held in [`pcloud_secret::secret_string::SecretString`]
    //!   and redacted in `Debug`;
    //! - the cryptoki session is created per call so a long-held session
    //!   does not accumulate state across concurrent callers.
    //!
    //! Supported mechanism: `CKM_AES_GCM` with an AEAD-style binding of
    //! the KMS `context` parameter as additional authenticated data. If a
    //! deployment needs RSA-OAEP (single-shot, ~245-byte limit) we can
    //! add it behind the same provider; AES-GCM is the standard choice
    //! for DEK wrapping in HSM-backed envelope encryption.
    use super::{KeyId, KmsError, KmsProvider, PlaintextDek, WrappedDek};
    use cryptoki::context::{CInitializeArgs, Pkcs11};
    use cryptoki::mechanism::Mechanism;
    use cryptoki::mechanism::aead::GcmParams;
    use cryptoki::object::{Attribute, ObjectHandle};
    use cryptoki::session::UserType;
    use cryptoki::slot::Slot;
    use cryptoki::types::AuthPin;
    use pcloud_secret::ExposeSecret;
    use std::fmt;
    use std::sync::Mutex;

    /// PKCS#11 HSM provider.
    ///
    /// Wraps a [`cryptoki::context::Pkcs11`] handle against a vendor
    /// `.so` / `.dylib` PKCS#11 module, authenticates to a slot with a
    /// user PIN, and performs all wrapping operations inside the HSM.
    pub struct Pkcs11Hsm {
        // Guarded by a Mutex so concurrent calls do not race on the
        // underlying module state. The cryptoki crate itself serialises
        // where required; we take a conservative posture here.
        ctx: Mutex<Pkcs11>,
        slot: Slot,
        pin: pcloud_secret::secret_string::SecretString,
        key_label: String,
    }

    impl fmt::Debug for Pkcs11Hsm {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Pkcs11Hsm")
                .field("slot", &self.slot)
                .field("key_label", &self.key_label)
                .field("pin", &"<redacted>")
                .finish()
        }
    }

    fn map_err<E: fmt::Display>(e: E) -> KmsError {
        let s = e.to_string();
        let lower = s.to_ascii_lowercase();
        if lower.contains("pin_incorrect") || lower.contains("pin incorrect") {
            KmsError::AuthFailed
        } else if lower.contains("not_found") || lower.contains("key_handle_invalid") {
            KmsError::KeyNotFound(s)
        } else if lower.contains("user_not_logged_in") || lower.contains("session_handle_invalid") {
            KmsError::PolicyDenied
        } else if lower.contains("device_error") || lower.contains("token_not_present") {
            KmsError::Unreachable(s)
        } else {
            KmsError::Other(s)
        }
    }

    impl Pkcs11Hsm {
        /// Back-compat constructor used when the caller has no module
        /// path or PIN and only wants the "feature off" error shape.
        ///
        /// # Errors
        /// Always returns [`KmsError::NotImplemented`]. Use
        /// [`Self::new_from_module`] for real HSM configuration.
        pub fn new(key_id: KeyId) -> Result<Self, KmsError> {
            let _ = key_id;
            Err(KmsError::NotImplemented(
                "pkcs11 (use Pkcs11Hsm::new_from_module)",
            ))
        }

        /// Construct a new PKCS#11 HSM provider.
        ///
        /// - `module_path`: path to the vendor PKCS#11 shared library
        ///   (e.g. `/usr/lib/softhsm/libsofthsm2.so`).
        /// - `slot_id`: numeric slot id on the token.
        /// - `pin`: user PIN (held in `SecretString`, zeroized on drop).
        /// - `key_label`: `CKA_LABEL` of the wrapping AES key inside the
        ///   HSM. The key must already exist — this provider never
        ///   generates or exports wrapping keys.
        ///
        /// # Errors
        /// Returns [`KmsError::Unreachable`] if the module cannot be
        /// loaded or the slot cannot be opened, and
        /// [`KmsError::AuthFailed`] if the PIN is rejected.
        pub fn new_from_module(
            module_path: &str,
            slot_id: u64,
            pin: pcloud_secret::secret_string::SecretString,
            key_label: &str,
        ) -> Result<Self, KmsError> {
            let ctx = Pkcs11::new(module_path).map_err(map_err)?;
            ctx.initialize(CInitializeArgs::OsThreads)
                .map_err(map_err)?;
            // Locate the slot.
            let slots = ctx.get_slots_with_token().map_err(map_err)?;
            let slot = slots
                .into_iter()
                .find(|s| s.id() == slot_id)
                .ok_or_else(|| KmsError::KeyNotFound(format!("pkcs11 slot {slot_id}")))?;
            // Probe login once to fail fast on a bad PIN.
            let session = ctx.open_rw_session(slot).map_err(map_err)?;
            session
                .login(
                    UserType::User,
                    Some(&AuthPin::new(pin.expose_secret().into())),
                )
                .map_err(map_err)?;
            // Logout + drop session; real work runs in per-call sessions.
            let _ = session.logout();
            drop(session);
            Ok(Self {
                ctx: Mutex::new(ctx),
                slot,
                pin,
                key_label: key_label.to_string(),
            })
        }

        fn find_key(&self, session: &cryptoki::session::Session) -> Result<ObjectHandle, KmsError> {
            let template = vec![Attribute::Label(self.key_label.as_bytes().to_vec())];
            let found = session.find_objects(&template).map_err(map_err)?;
            found
                .into_iter()
                .next()
                .ok_or_else(|| KmsError::KeyNotFound(self.key_label.clone()))
        }
    }

    impl KmsProvider for Pkcs11Hsm {
        fn name(&self) -> &'static str {
            "pkcs11"
        }

        fn encrypt_dek(
            &self,
            _key_id: &KeyId,
            dek: &PlaintextDek,
            context: Option<&str>,
        ) -> Result<WrappedDek, KmsError> {
            let ctx = self
                .ctx
                .lock()
                .map_err(|_| KmsError::Other("pkcs11 context lock poisoned".to_string()))?;
            let session = ctx.open_rw_session(self.slot).map_err(map_err)?;
            session
                .login(
                    UserType::User,
                    Some(&AuthPin::new(self.pin.expose_secret().into())),
                )
                .map_err(map_err)?;
            let key = self.find_key(&session)?;

            // 12-byte random IV for AES-GCM. `GcmParams::new` wants a
            // `&mut [u8]` so the HSM can replace the IV when the vendor
            // module generates one on its own side.
            let mut iv = [0u8; 12];
            getrandom::getrandom(&mut iv)
                .map_err(|e: getrandom::Error| KmsError::Other(e.to_string()))?;

            let aad: Vec<u8> = context.map(|c| c.as_bytes().to_vec()).unwrap_or_default();
            let mut iv_slice = iv;
            let gcm_params = GcmParams::new(&mut iv_slice, &aad, 128.into())
                .map_err(|e| KmsError::Other(e.to_string()))?;
            let mech = Mechanism::AesGcm(gcm_params);

            let ciphertext = session.encrypt(&mech, key, dek.expose()).map_err(map_err)?;
            let _ = session.logout();

            // Wire format: 1-byte version || 12-byte IV || ciphertext||tag.
            // `iv_slice` may have been rewritten by the vendor module in
            // `GcmParams::new`; store whatever the HSM chose, not our
            // original randomness, so decrypt_dek reuses the same IV.
            let mut out = Vec::with_capacity(1 + iv_slice.len() + ciphertext.len());
            out.push(0x01);
            out.extend_from_slice(&iv_slice);
            out.extend_from_slice(&ciphertext);
            Ok(WrappedDek(out))
        }

        fn decrypt_dek(
            &self,
            _key_id: &KeyId,
            wrapped: &WrappedDek,
            context: Option<&str>,
        ) -> Result<PlaintextDek, KmsError> {
            if wrapped.0.len() < 1 + 12 + 16 {
                return Err(KmsError::Malformed);
            }
            if wrapped.0[0] != 0x01 {
                return Err(KmsError::Malformed);
            }
            // Owned mutable copy of the IV (GcmParams wants &mut [u8]).
            let mut iv = [0u8; 12];
            iv.copy_from_slice(&wrapped.0[1..13]);
            let ct = wrapped.0[13..].to_vec();

            let ctx = self
                .ctx
                .lock()
                .map_err(|_| KmsError::Other("pkcs11 context lock poisoned".to_string()))?;
            let session = ctx.open_rw_session(self.slot).map_err(map_err)?;
            session
                .login(
                    UserType::User,
                    Some(&AuthPin::new(self.pin.expose_secret().into())),
                )
                .map_err(map_err)?;
            let key = self.find_key(&session)?;

            let aad: Vec<u8> = context.map(|c| c.as_bytes().to_vec()).unwrap_or_default();
            let gcm_params = GcmParams::new(&mut iv, &aad, 128.into())
                .map_err(|e| KmsError::Other(e.to_string()))?;
            let mech = Mechanism::AesGcm(gcm_params);

            let plaintext = session.decrypt(&mech, key, &ct).map_err(map_err)?;
            let _ = session.logout();
            Ok(PlaintextDek(plaintext))
        }

        fn health_check(&self) -> Result<(), KmsError> {
            let ctx = self
                .ctx
                .lock()
                .map_err(|_| KmsError::Other("pkcs11 context lock poisoned".to_string()))?;
            let session = ctx.open_ro_session(self.slot).map_err(map_err)?;
            // A read-only session that opens cleanly is a sufficient
            // liveness signal — we avoid logging in just to ping.
            drop(session);
            Ok(())
        }
    }

    // No manual Drop impl: `cryptoki::context::Pkcs11` already owns the
    // module handle and will finalize on its own Drop. Adding a manual
    // `ctx.finalize()` here would require moving out of the Mutex guard
    // (finalize takes `self` by value) and is redundant.
}

#[cfg(feature = "pkcs11")]
pub use pkcs11_real::Pkcs11Hsm;

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_provider_reports_name() {
        assert_eq!(NullKms.name(), "null");
    }

    #[test]
    fn null_provider_refuses_ops() {
        let p = NullKms;
        let id = KeyId("local://none".to_string());
        let dek = PlaintextDek(vec![0u8; 32]);
        assert!(matches!(
            p.encrypt_dek(&id, &dek, None),
            Err(KmsError::NotImplemented("null"))
        ));
    }

    #[test]
    fn null_provider_health_is_ok() {
        assert!(NullKms.health_check().is_ok());
    }

    #[test]
    fn plaintext_dek_debug_is_redacted() {
        let dek = PlaintextDek(vec![0xAA; 8]);
        let s = format!("{dek:?}");
        assert!(s.contains("<redacted>"));
        assert!(!s.contains("AA"));
    }

    #[test]
    fn pkcs11_constructor_fails_loudly() {
        // Either the stub (feature off) or the real provider's no-arg
        // constructor must return `NotImplemented`. The exact static
        // string differs so operators can see which build they have.
        match Pkcs11Hsm::new(KeyId("slot=0;label=x".into())) {
            Err(KmsError::NotImplemented(msg)) => {
                assert!(msg.contains("pkcs11"));
            }
            Err(other) => panic!("unexpected error: {other:?}"),
            Ok(_) => panic!("Pkcs11Hsm::new must not succeed without a module path"),
        }
    }

    // Mock KMS used by the CryptoShell injection test below.
    // Proves the trait object boundary works end-to-end.
    struct MockProvider {
        calls: std::sync::atomic::AtomicUsize,
    }
    impl KmsProvider for MockProvider {
        fn name(&self) -> &'static str {
            "mock"
        }
        fn encrypt_dek(
            &self,
            _k: &KeyId,
            dek: &PlaintextDek,
            _c: Option<&str>,
        ) -> Result<WrappedDek, KmsError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Reversible, non-cryptographic transform for test only.
            let mut w = dek.expose().to_vec();
            w.reverse();
            Ok(WrappedDek(w))
        }
        fn decrypt_dek(
            &self,
            _k: &KeyId,
            wrapped: &WrappedDek,
            _c: Option<&str>,
        ) -> Result<PlaintextDek, KmsError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut p = wrapped.0.clone();
            p.reverse();
            Ok(PlaintextDek(p))
        }
        fn health_check(&self) -> Result<(), KmsError> {
            Ok(())
        }
    }

    #[test]
    fn trait_object_dyn_dispatch_roundtrips() {
        // Confirms that `Box<dyn KmsProvider>` (the exact shape the
        // CryptoShell factory takes) does a clean encrypt/decrypt cycle
        // through a non-trivial provider.
        let p: Box<dyn KmsProvider> = Box::new(MockProvider {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let id = KeyId("mock-key".into());
        let dek = PlaintextDek(vec![1, 2, 3, 4, 5]);
        let w = p.encrypt_dek(&id, &dek, Some("ctx")).unwrap();
        assert_ne!(w.0, dek.expose());
        let back = p.decrypt_dek(&id, &w, Some("ctx")).unwrap();
        assert_eq!(back.expose(), dek.expose());
    }

    #[cfg(feature = "pkcs11")]
    #[test]
    fn pkcs11_bad_module_path_is_unreachable_or_other() {
        // Feature-gated sanity test. We deliberately pass a path that
        // does not exist so the cryptoki `new` call fails at load time.
        // The error must be mapped into our taxonomy — not a panic and
        // not a silent pass.
        use pcloud_secret::secret_string::SecretString;
        let res = Pkcs11Hsm::new_from_module(
            "/nonexistent/pkcs11/module-for-pcloud-kms-tests.so",
            0,
            SecretString::new("0000"),
            "kek",
        );
        match res {
            Err(KmsError::Unreachable(_))
            | Err(KmsError::Other(_))
            | Err(KmsError::KeyNotFound(_)) => {}
            other => panic!("expected taxonomy error, got {other:?}"),
        }
    }

    // ---- cache tests ----

    /// Test provider that counts decrypt_dek invocations.
    struct CountingProvider {
        name: &'static str,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl CountingProvider {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
    impl KmsProvider for CountingProvider {
        fn name(&self) -> &'static str {
            self.name
        }
        fn encrypt_dek(
            &self,
            _k: &KeyId,
            dek: &PlaintextDek,
            _c: Option<&str>,
        ) -> Result<WrappedDek, KmsError> {
            Ok(WrappedDek(dek.expose().to_vec()))
        }
        fn decrypt_dek(
            &self,
            _k: &KeyId,
            wrapped: &WrappedDek,
            _c: Option<&str>,
        ) -> Result<PlaintextDek, KmsError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(PlaintextDek(wrapped.0.clone()))
        }
        fn health_check(&self) -> Result<(), KmsError> {
            Ok(())
        }
    }

    #[test]
    fn cache_returns_plaintext_within_ttl() {
        let p = CountingProvider::new("counting-within");
        let id = KeyId("cache-within-k".into());
        let wrapped = WrappedDek(vec![1, 2, 3, 4]);

        let a = p
            .unwrap_cached(&id, &wrapped, None, Duration::from_secs(60))
            .unwrap();
        let b = p
            .unwrap_cached(&id, &wrapped, None, Duration::from_secs(60))
            .unwrap();

        assert_eq!(a.expose(), b.expose());
        assert_eq!(
            p.call_count(),
            1,
            "second call must hit the cache, not the provider"
        );
    }

    #[test]
    fn cache_expires_after_ttl() {
        // Simulate expiry with a manual clock: insert an entry with an
        // `inserted` time far in the past, then query with a short TTL.
        let p = CountingProvider::new("counting-expiry");
        let id = KeyId("cache-expiry-k".into());
        let wrapped = WrappedDek(vec![9, 9, 9]);

        let key = CacheKey {
            provider: p.name(),
            key_id: id.0.clone(),
            wrapped: wrapped.0.clone(),
            context: None,
        };
        let stale = Instant::now()
            .checked_sub(Duration::from_secs(3600))
            .expect("instant subtraction");
        cache_insert_at(key, PlaintextDek(vec![0xDE, 0xAD]), stale);

        // TTL of 1s — the stale entry is 3600s old, so it must be evicted
        // and the provider must be called.
        let out = p
            .unwrap_cached(&id, &wrapped, None, Duration::from_secs(1))
            .unwrap();
        assert_eq!(out.expose(), &[9, 9, 9]);
        assert_eq!(p.call_count(), 1);
    }

    #[test]
    fn cache_distinguishes_context() {
        let p = CountingProvider::new("counting-ctx");
        let id = KeyId("cache-ctx-k".into());
        let wrapped = WrappedDek(vec![7]);
        let _ = p
            .unwrap_cached(&id, &wrapped, Some("a"), Duration::from_secs(60))
            .unwrap();
        let _ = p
            .unwrap_cached(&id, &wrapped, Some("b"), Duration::from_secs(60))
            .unwrap();
        assert_eq!(
            p.call_count(),
            2,
            "different contexts must miss the cache independently"
        );
    }

    // ---- provider-specific integration tests (opt-in) ----

    #[cfg(feature = "aws")]
    #[test]
    #[ignore = "requires AWS creds + PCLOUD_KMS_AWS_TEST=1 + PCLOUD_KMS_AWS_KEY_ARN"]
    fn aws_wrap_unwrap_roundtrip() {
        if std::env::var("PCLOUD_KMS_AWS_TEST").ok().as_deref() != Some("1") {
            return;
        }
        let arn =
            std::env::var("PCLOUD_KMS_AWS_KEY_ARN").expect("PCLOUD_KMS_AWS_KEY_ARN must be set");
        let region =
            std::env::var("PCLOUD_KMS_AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
        let kms = AwsKms::new(region);
        let dek = PlaintextDek(vec![0x42; 32]);
        let wrapped = kms
            .encrypt_dek(&KeyId(arn.clone()), &dek, Some("roundtrip"))
            .expect("encrypt_dek");
        let unwrapped = kms
            .decrypt_dek(&KeyId(arn), &wrapped, Some("roundtrip"))
            .expect("decrypt_dek");
        assert_eq!(unwrapped.expose(), dek.expose());
    }

    #[cfg(feature = "vault")]
    #[test]
    #[ignore = "requires live Vault + PCLOUD_KMS_VAULT_TEST=1"]
    fn vault_wrap_unwrap_roundtrip() {
        use pcloud_secret::secret_string::SecretString;
        if std::env::var("PCLOUD_KMS_VAULT_TEST").ok().as_deref() != Some("1") {
            return;
        }
        let url = std::env::var("PCLOUD_KMS_VAULT_URL").expect("PCLOUD_KMS_VAULT_URL");
        let token = std::env::var("PCLOUD_KMS_VAULT_TOKEN").expect("PCLOUD_KMS_VAULT_TOKEN");
        let key = std::env::var("PCLOUD_KMS_VAULT_KEY").expect("PCLOUD_KMS_VAULT_KEY");
        let v = HashicorpVault::new(url, SecretString::new(token), key.clone())
            .expect("construct vault");
        let dek = PlaintextDek(vec![0x11; 32]);
        let wrapped = v
            .encrypt_dek(&KeyId(key.clone()), &dek, Some("rt"))
            .expect("encrypt");
        let unwrapped = v
            .decrypt_dek(&KeyId(key), &wrapped, Some("rt"))
            .expect("decrypt");
        assert_eq!(unwrapped.expose(), dek.expose());
    }
}
