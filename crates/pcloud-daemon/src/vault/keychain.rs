//! **PLATFORM: macOS.** Keychain-backed auth token vault.
//!
//! Tier-1 backend for macOS. Uses `security-framework = "2"` generic
//! password items under a `com.pcloud.pcloud-rs` service identifier,
//! scoped to the current user's login keychain.
//!
//! Secrets never touch disk in plaintext; they are handed to the system
//! Keychain, which enforces per-user ACLs. On read the UTF-8 bytes are
//! wrapped in a `SecretString` so zeroization on drop applies.

#![cfg(target_os = "macos")]

use std::io;
use std::path::PathBuf;

use pcloud_secret::secret_string::SecretString;
use security_framework::base::Error as SecError;
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

use super::{AuthToken, PlatformVault, Result as VaultResult};
use crate::auth_vault::AuthVaultError;

/// Keychain service identifier (reverse-DNS, stable across releases).
const SERVICE: &str = "com.pcloud.pcloud-rs";
/// Keychain account identifier — one token slot per user.
const ACCOUNT: &str = "pcloud-auth-token";

/// `errSecItemNotFound` — returned by the Keychain when no generic
/// password item exists for the given (service, account) tuple.
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// macOS Keychain–backed vault.
#[derive(Debug, Clone)]
pub struct KeychainVault {
    /// Reserved for a future fallback path; kept so the constructor
    /// signature matches `FileVault::new`.
    _fallback_path: PathBuf,
    /// Account key used for this vault instance. Production always uses
    /// `ACCOUNT`; tests inject a unique key to avoid concurrent-test
    /// collision on the same Keychain item.
    account: &'static str,
}

impl KeychainVault {
    /// Construct a new `KeychainVault` using the production account key.
    pub fn new(fallback_path: impl Into<PathBuf>) -> Self {
        Self {
            _fallback_path: fallback_path.into(),
            account: ACCOUNT,
        }
    }

    /// Test-only constructor that uses a caller-supplied static account key
    /// so concurrent tests do not race on the production Keychain item.
    #[cfg(test)]
    fn new_with_account(fallback_path: impl Into<PathBuf>, account: &'static str) -> Self {
        Self {
            _fallback_path: fallback_path.into(),
            account,
        }
    }
}

/// Map a `security-framework` error into `AuthVaultError`. The only
/// structured variant in scope is `Io`, so Keychain failures are
/// surfaced as `io::Error` with kind `Other` and the OSStatus code
/// embedded in the message for post-mortem debugging.
fn map_sec_err(err: SecError) -> AuthVaultError {
    AuthVaultError::Io(io::Error::new(
        io::ErrorKind::Other,
        format!("keychain error (OSStatus {}): {}", err.code(), err),
    ))
}

impl PlatformVault for KeychainVault {
    fn load(&self) -> VaultResult<Option<AuthToken>> {
        match get_generic_password(SERVICE, self.account) {
            Ok(bytes) => {
                let utf8 = String::from_utf8(bytes).map_err(|_| {
                    AuthVaultError::InsecureMetadata(
                        "keychain item for pcloud-auth-token is not valid UTF-8",
                    )
                })?;
                Ok(Some(SecretString::new(utf8)))
            }
            Err(err) if err.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(err) => Err(map_sec_err(err)),
        }
    }

    fn store(&self, token: &AuthToken) -> VaultResult<()> {
        use pcloud_secret::ExposeSecret;
        let bytes = token.expose_secret().as_bytes();
        set_generic_password(SERVICE, self.account, bytes).map_err(map_sec_err)
    }

    fn clear(&self) -> VaultResult<()> {
        match delete_generic_password(SERVICE, self.account) {
            Ok(()) => Ok(()),
            Err(err) if err.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(err) => Err(map_sec_err(err)),
        }
    }

    fn backend_name(&self) -> &'static str {
        "keychain"
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use pcloud_secret::ExposeSecret;

    /// Roundtrip: store → load → clear → load against the real login
    /// Keychain. Skipped by cargo test on Linux/Windows via the
    /// `target_os = "macos"` cfg gate above.
    ///
    /// Marked `#[ignore]` because macOS Keychain ACL enforcement means the
    /// test binary needs to be consistently signed/entitled across runs.
    /// Running it as part of a parallel workspace test suite can cause
    /// -25293 (errSecAuthFailed) when a stale item from a prior binary
    /// invocation still holds different ACLs. Run explicitly when needed:
    ///
    ///   cargo test -p pcloud-daemon --lib -- vault::keychain::tests::roundtrip --include-ignored
    #[test]
    #[ignore = "requires exclusive macOS Keychain access; run explicitly with --include-ignored"]
    fn roundtrip() {
        let vault = KeychainVault::new_with_account(
            std::env::temp_dir().join("pcloud-keychain-fallback"),
            "pcloud-auth-token-test-roundtrip",
        );
        // Best-effort pre-clean so prior failed runs don't pollute state.
        let _ = vault.clear();

        let token = SecretString::new("roundtrip-token-value".to_string());
        vault.store(&token).expect("store should succeed");

        let loaded = vault
            .load()
            .expect("load should succeed")
            .expect("token present");
        assert_eq!(loaded.expose_secret(), "roundtrip-token-value");

        vault.clear().expect("clear should succeed");
        assert!(vault.load().expect("load after clear").is_none());

        // Second clear is idempotent (ItemNotFound → Ok).
        vault.clear().expect("second clear should be Ok");
    }
}
