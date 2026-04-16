//! **PLATFORM: Linux (opt-in).** Freedesktop Secret Service–backed vault.
//!
//! Opt-in under `PCLOUD_VAULT=secret-service`. The default Linux backend
//! remains `FileVault` because Secret Service pulls in a D-Bus dependency
//! and a running `gnome-keyring` / `kwallet` / equivalent daemon, neither
//! of which are safe defaults for a headless service.
//!
//! Dependency: `secret-service = "4"` with the `rt-async-io-crypto-rust`
//! feature so the underlying `zbus` stack uses `async-io` (no tokio
//! runtime dependency introduced by this backend).
//!
//! # Async-bridge strategy
//!
//! The `secret-service` crate is fundamentally async (built on `zbus`),
//! but it ships a first-class `blocking` module that internally drives
//! the async API on a per-connection basis. Vault calls are rare (login,
//! logout, and startup-time rehydration) and are *not* on any hot path,
//! so the blocking API is the correct choice: it avoids spawning a
//! dedicated tokio runtime per call, avoids cross-runtime contamination
//! when the daemon's caller is already inside a tokio reactor, and keeps
//! the `PlatformVault` trait synchronous (matching the existing
//! `FileVault`).
//!
//! # Security
//!
//! The token is always wrapped in [`SecretString`] before returning, so
//! zeroization on drop is enforced by the type system. Attributes never
//! contain the token. Errors are mapped via [`std::io::Error`] so the
//! existing `AuthVaultError::Io` variant transparently carries the
//! backend failure reason without widening the error enum's public
//! surface.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::io;

use pcloud_secret::secret_string::SecretString;
use secret_service::blocking::SecretService;
use secret_service::{EncryptionType, Error as SsError};

use super::{AuthToken, PlatformVault, Result as VaultResult};

/// Attribute key identifying the owning application.
const ATTR_SERVICE: &str = "service";
/// Attribute key identifying which credential within the service.
const ATTR_ACCOUNT: &str = "account";
/// Stable value for the `service` attribute.
const SERVICE_VALUE: &str = "com.pcloud.pcloud-rs";
/// Stable value for the `account` attribute.
const ACCOUNT_VALUE: &str = "pcloud-auth-token";
/// User-visible label applied to newly created items.
const ITEM_LABEL: &str = "pcloud auth token";
/// MIME type recorded for the stored secret.
const ITEM_CONTENT_TYPE: &str = "text/plain; charset=utf8";

/// Freedesktop Secret Service–backed vault.
///
/// Persists exactly one item per user whose attributes match
/// `{service: "com.pcloud.pcloud-rs", account: "pcloud-auth-token"}`.
#[derive(Debug, Clone, Default)]
pub struct SecretServiceVault {
    _priv: (),
}

impl SecretServiceVault {
    /// Construct a new `SecretServiceVault`.
    ///
    /// Connection to the session D-Bus is deferred until the first call
    /// because establishing a session is observable (it may prompt the
    /// user for a keyring password).
    pub fn new() -> Self {
        Self::default()
    }

    fn search_attrs() -> HashMap<&'static str, &'static str> {
        HashMap::from([(ATTR_SERVICE, SERVICE_VALUE), (ATTR_ACCOUNT, ACCOUNT_VALUE)])
    }
}

/// Map a [`secret_service::Error`] into the `AuthVaultError::Io`
/// variant. The redacted message never contains secret bytes.
fn ss_err(context: &str, err: SsError) -> super::VaultError {
    super::VaultError::Io(io::Error::other(format!(
        "secret-service backend: {context}: {err}"
    )))
}

/// Map an arbitrary error message.
fn io_err(msg: impl Into<String>) -> super::VaultError {
    super::VaultError::Io(io::Error::other(msg.into()))
}

impl PlatformVault for SecretServiceVault {
    fn load(&self) -> VaultResult<Option<AuthToken>> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(|e| ss_err("connect", e))?;
        let collection = match ss.get_default_collection() {
            Ok(c) => c,
            // No default collection yet: treat as "no token stored".
            Err(SsError::NoResult) => return Ok(None),
            Err(e) => return Err(ss_err("get_default_collection", e)),
        };

        // Unlock on demand so that `get_secret` does not fail on a
        // locked keyring. The Secret Service daemon is responsible for
        // prompting the user; we do not handle prompts here.
        if collection.is_locked().map_err(|e| ss_err("is_locked", e))? {
            collection.unlock().map_err(|e| ss_err("unlock", e))?;
        }

        let items = collection
            .search_items(Self::search_attrs())
            .map_err(|e| ss_err("search_items", e))?;

        match items.len() {
            0 => Ok(None),
            1 => {
                let item = &items[0];
                if item.is_locked().map_err(|e| ss_err("item.is_locked", e))? {
                    item.unlock().map_err(|e| ss_err("item.unlock", e))?;
                }
                let bytes = item
                    .get_secret()
                    .map_err(|e| ss_err("item.get_secret", e))?;
                let s = String::from_utf8(bytes).map_err(|_| {
                    // Do not leak raw bytes or length into the error.
                    io_err("secret-service backend: stored auth token is not valid UTF-8")
                })?;
                Ok(Some(SecretString::new(s)))
            }
            n => Err(io_err(format!(
                "secret-service backend: expected exactly one matching item, found {n}"
            ))),
        }
    }

    fn store(&self, token: &AuthToken) -> VaultResult<()> {
        use pcloud_secret::ExposeSecret;

        let ss = SecretService::connect(EncryptionType::Dh).map_err(|e| ss_err("connect", e))?;
        let collection = ss
            .get_default_collection()
            .map_err(|e| ss_err("get_default_collection", e))?;

        if collection.is_locked().map_err(|e| ss_err("is_locked", e))? {
            collection.unlock().map_err(|e| ss_err("unlock", e))?;
        }

        // `replace = true` causes the daemon to overwrite any existing
        // item with the same attributes atomically.
        collection
            .create_item(
                ITEM_LABEL,
                Self::search_attrs(),
                token.expose_secret().as_bytes(),
                true,
                ITEM_CONTENT_TYPE,
            )
            .map_err(|e| ss_err("create_item", e))?;

        Ok(())
    }

    fn clear(&self) -> VaultResult<()> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(|e| ss_err("connect", e))?;
        let collection = match ss.get_default_collection() {
            Ok(c) => c,
            // Missing collection → nothing to clear (idempotent).
            Err(SsError::NoResult) => return Ok(()),
            Err(e) => return Err(ss_err("get_default_collection", e)),
        };

        if collection.is_locked().map_err(|e| ss_err("is_locked", e))? {
            collection.unlock().map_err(|e| ss_err("unlock", e))?;
        }

        let items = collection
            .search_items(Self::search_attrs())
            .map_err(|e| ss_err("search_items", e))?;

        // Idempotent: delete every matching item, tolerate zero matches.
        for item in &items {
            if item.is_locked().map_err(|e| ss_err("item.is_locked", e))? {
                item.unlock().map_err(|e| ss_err("item.unlock", e))?;
            }
            item.delete().map_err(|e| ss_err("item.delete", e))?;
        }

        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "secret-service"
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use pcloud_secret::ExposeSecret;

    /// Live round-trip against the user's Secret Service daemon.
    ///
    /// Gated by `PCLOUD_VAULT_SS_TEST=1` and skipped (with a note) if
    /// D-Bus is unreachable — CI runners without a running session bus
    /// will surface `Error::Zbus(...)` on `SecretService::connect`, and
    /// this test converts that into a skip rather than a hard failure.
    #[test]
    fn roundtrip_via_secret_service() {
        if std::env::var("PCLOUD_VAULT_SS_TEST").ok().as_deref() != Some("1") {
            eprintln!(
                "skipping roundtrip_via_secret_service: set PCLOUD_VAULT_SS_TEST=1 to enable"
            );
            return;
        }

        // Probe D-Bus availability; skip if unavailable.
        if SecretService::connect(EncryptionType::Dh).is_err() {
            eprintln!(
                "skipping roundtrip_via_secret_service: session D-Bus / Secret Service not available"
            );
            return;
        }

        let vault = SecretServiceVault::new();

        // Ensure a clean slate.
        vault.clear().expect("clear (pre)");

        assert!(vault.load().expect("load empty").is_none());

        let token = SecretString::new("pcloud-test-token-abc123".to_string());
        vault.store(&token).expect("store");

        let loaded = vault.load().expect("load after store");
        let loaded = loaded.expect("token present");
        assert_eq!(loaded.expose_secret(), token.expose_secret());

        // Overwrite path.
        let token2 = SecretString::new("pcloud-test-token-xyz789".to_string());
        vault.store(&token2).expect("store (overwrite)");
        let loaded2 = vault
            .load()
            .expect("load after overwrite")
            .expect("present");
        assert_eq!(loaded2.expose_secret(), token2.expose_secret());

        // Clear twice — must be idempotent.
        vault.clear().expect("clear");
        vault.clear().expect("clear (idempotent)");
        assert!(vault.load().expect("load after clear").is_none());
    }

    #[test]
    fn backend_name_is_stable() {
        assert_eq!(SecretServiceVault::new().backend_name(), "secret-service");
    }
}
