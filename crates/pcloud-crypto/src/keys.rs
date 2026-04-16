//! Key derivation and wrapping for the Rust crypto path.
//!
//! The password -> master-key derivation uses Argon2 with a per-profile salt.
//! The derived master key is never persisted; only a non-secret setup
//! fingerprint (see `SetupFingerprint`) is stored so that an incorrect
//! password at `start` time can be rejected without attempting any content
//! decryption.

// **PLATFORM:** all
// **GATING:** none (portable).

use argon2::Argon2;
use getrandom::getrandom;
use hmac::{Hmac, Mac};
use pcloud_secret::secret_bytes::SecretBytes;
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

pub(crate) const DERIVATION_SALT_LEN: usize = 16;
pub(crate) const DERIVED_KEY_LEN: usize = 32;
pub(crate) const FINGERPRINT_LEN: usize = 32;

/// Non-secret fingerprint of a setup password.
///
/// Computed as `HMAC-SHA256(derived_key, "pcloud-crypto/fingerprint/v1")`.
/// Storing this rather than the key material itself means a compromised
/// on-disk profile cannot be used to decrypt any content — it can only be
/// used to verify whether a later start-password matches setup.
///
/// # Security
/// Mitigates: on-disk offline attacks on the profile (the 32-byte HMAC
/// output reveals no bits of the 32-byte Argon2id key); wrong-password
/// unlock that would otherwise only fail later during AEAD tag check
/// (the fingerprint lets `start` refuse early with constant-time
/// `subtle::ConstantTimeEq`). Per ADR-0007 the password itself is
/// never persisted; only this non-secret fingerprint is.
///
/// Out of scope: offline password guessing against the fingerprint. The
/// label is public and the HMAC key is the Argon2id output, so an
/// attacker with the on-disk fingerprint can test candidate passwords
/// at Argon2id cost — that cost is the only barrier. Choose strong
/// passwords; see [`crate::psync_password_quality`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupFingerprint(pub [u8; FINGERPRINT_LEN]);

/// In-memory key-state manager for the crypto subsystem.
///
/// Holds only non-secret parameters (salt, TTL, fingerprint, flags) on disk;
/// the derived master key (`active_key_material`) lives in
/// [`SecretBytes`] and is **never** serialized (marked `#[serde(skip)]`).
/// It is zeroized on drop and on every `stop()`/`reset()` transition.
#[derive(Debug, Serialize, Deserialize)]
pub struct KeyManager {
    /// Time-to-live (seconds) for the in-memory key cache after the last
    /// successful authenticated operation. Non-secret.
    pub cache_ttl_secs: u64,
    /// Per-profile Argon2id salt (16 bytes, OS-random). Non-secret, but must
    /// remain stable once `setup()` has recorded a fingerprint or the fingerprint
    /// check on `start()` will fail.
    pub derivation_salt: Vec<u8>,
    /// Fingerprint recorded by [`crate::CryptoShell::setup`] — used to reject
    /// the wrong password on `start` without ever decrypting content with bad
    /// key material.
    pub setup_fingerprint: Option<SetupFingerprint>,
    /// Private-key flags — mirrors the C client's `crypto_private_flags`
    /// row in the `setting` table (read via `psync_crypto_priv_key_flags()`).
    /// Default is `0`. Bit 0 = [`PRIV_KEY_FLAG_TEMP_PASS`] matches the C
    /// `PSYNC_CRYPTO_FLAG_TEMP_PASS` constant.
    #[serde(default)]
    pub private_flags: u64,
    /// Currently unlocked master key material (32-byte Argon2id output).
    ///
    /// **SecretBytes ownership:** owned by this struct while unlocked; zeroized
    /// on drop and cleared on `stop()`/`reset()`. Never serialized or logged.
    #[serde(skip)]
    pub active_key_material: Option<SecretBytes>,
}

/// Bit 0 of [`KeyManager::private_flags`]: the current passphrase is a
/// server-issued temporary password (the user should rotate it). Mirrors
/// the C `PSYNC_CRYPTO_FLAG_TEMP_PASS`.
pub const PRIV_KEY_FLAG_TEMP_PASS: u64 = 1;

impl Default for KeyManager {
    fn default() -> Self {
        let mut derivation_salt = vec![0u8; DERIVATION_SALT_LEN];
        getrandom(&mut derivation_salt)
            .expect("OS randomness should be available for crypto salt generation");

        Self {
            cache_ttl_secs: 300,
            derivation_salt,
            setup_fingerprint: None,
            private_flags: 0,
            active_key_material: None,
        }
    }
}

impl KeyManager {
    /// Derive a 32-byte master key from `password` using this manager's
    /// Argon2id salt (default cost parameters).
    ///
    /// Primitive: Argon2id via `argon2` crate defaults — `m = 19456 KiB`,
    /// `t = 2`, `p = 1`, output length 32 bytes, salt length 16 bytes.
    /// These parameters are the crate
    /// default and are deliberately not weakened for this path; they
    /// balance enterprise-grade offline-guessing resistance against
    /// interactive-unlock latency.
    ///
    /// **SecretBytes ownership:** the caller owns the returned
    /// [`SecretBytes`], which zeroizes on drop. The input `password` is
    /// borrowed and never retained. `SecretString` / `SecretBytes` are
    /// not `Clone`; explicit copies are only made via `clone_secret()`
    /// (opt-in) so accidental duplication is impossible.
    ///
    /// # Security
    /// Mitigates: low-cost offline guessing (Argon2id memory-hard PRF),
    /// rainbow-table attacks (per-profile 16-byte salt), and long-lived
    /// plaintext-key residency (output wrapped in `SecretBytes`, zeroize
    /// on drop). Per ADR-0007 the input password is never persisted.
    ///
    /// Out of scope: coercion of the user into choosing a weak password
    /// (the scorer in [`crate::psync_password_quality`] is advisory);
    /// kernel-level swap capture of Argon2id working memory during
    /// derivation.
    ///
    /// # Panics
    /// Propagates an `expect()` if Argon2id rejects the fixed 32-byte
    /// output length — unreachable under the crate defaults.
    #[must_use]
    pub fn derive_key_material(&self, password: &SecretString) -> SecretBytes {
        Self::derive_key_material_with_salt(password, &self.derivation_salt)
    }

    /// Derive a 32-byte master key using an explicit `salt` (Argon2id default
    /// cost parameters; output length is always 32 bytes).
    ///
    /// **SecretBytes ownership:** caller owns the returned key; `password` is
    /// only borrowed during derivation.
    ///
    /// # Security
    /// Same primitive (Argon2id `m = 19456`, `t = 2`, `p = 1`) and same
    /// zeroize-on-drop contract as [`Self::derive_key_material`]. Used by
    /// password-rotation flows that rotate the salt atomically with the
    /// key, so that the same password produces a distinct key before and
    /// after rotation.
    ///
    /// # Panics
    /// `expect()` on Argon2id failure for the fixed 32-byte output.
    #[must_use]
    pub fn derive_key_material_with_salt(password: &SecretString, salt: &[u8]) -> SecretBytes {
        let mut derived = vec![0u8; DERIVED_KEY_LEN];
        Argon2::default()
            .hash_password_into(password.expose_secret().as_bytes(), salt, &mut derived)
            .expect("fixed argon2 output length should be valid");
        SecretBytes::new(derived)
    }

    /// Compute the setup fingerprint for a given derived master key.
    ///
    /// Primitive: `HMAC-SHA256(key, "pcloud-crypto/fingerprint/v1")`.
    /// Output is 32 bytes (one SHA-256 block) and is non-secret.
    ///
    /// # Security
    /// The fixed label `pcloud-crypto/fingerprint/v1` domain-separates
    /// this PRF output from per-file keys ([`crate::content::derive_file_key`])
    /// and from filename tags ([`crate::metadata::encrypt_filename`]).
    /// The key is borrowed in `SecretBytes` and the bytes never escape
    /// the HMAC engine.
    ///
    /// # Panics
    /// `expect()` on `Hmac::new_from_slice`; this is infallible for any
    /// non-empty key length.
    #[must_use]
    pub fn fingerprint_for(key: &SecretBytes) -> SetupFingerprint {
        const LABEL: &[u8] = b"pcloud-crypto/fingerprint/v1";
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.expose_secret())
            .expect("HMAC-SHA256 accepts any key length");
        mac.update(LABEL);
        let bytes: [u8; FINGERPRINT_LEN] = mac.finalize().into_bytes().into();
        SetupFingerprint(bytes)
    }

    /// Returns true iff the derived key matches the stored setup fingerprint.
    /// Comparison is constant-time.
    ///
    /// # Security
    /// Mitigates: timing side-channels on wrong-password attempts
    /// (`subtle::ConstantTimeEq` performs a byte-by-byte XOR-OR fold,
    /// no early return). The candidate `key` is only borrowed, not
    /// retained; the stored fingerprint is non-secret by construction.
    ///
    /// Out of scope: power-analysis of the HMAC engine itself — the
    /// underlying `hmac` crate is expected to be timing-stable but is
    /// not side-channel-hardened against differential power analysis.
    #[must_use]
    pub fn matches_setup(&self, key: &SecretBytes) -> bool {
        let Some(stored) = self.setup_fingerprint.as_ref() else {
            return false;
        };
        let computed = Self::fingerprint_for(key);
        computed.0.ct_eq(&stored.0).into()
    }
}
