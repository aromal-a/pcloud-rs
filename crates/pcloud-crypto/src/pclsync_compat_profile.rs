//! PclsyncCompat profile persistence and runtime state (Wave 2 Stage 2+3).
//!
//! This module is gated on the `pclsync-v2` feature. It owns:
//!
//! 1. The on-disk `PclsyncCompatProfile`: salt + wrapped priv_key_ver1 blob +
//!    pub_key_ver1 blob + non-secret pub-key fingerprint (for constant-time
//!    wrong-password rejection) + flags. Wire-layout-equivalent to what the
//!    legacy C client uploads via `crypto_setuserkeys`
//!    (see `C_CODE/pclsync/pcryptofolder.c:72-83` — the `priv_key_ver1` and
//!    `pub_key_ver1` C structs).
//!
//! 2. The runtime `PclsyncCompatState`: live RSA private key unwrapped from
//!    the priv_key_ver1 blob, plus lazy caches of folder/file `SymKeyVer1`
//!    values obtained via `crypto_getfolderkey` (Stage 4 wiring). Not
//!    persisted (`#[serde(skip)]` on the holding field in `CryptoShell`).
//!
//! ## On-disk layout reference (priv_key_ver1)
//!
//! From `C_CODE/pclsync/pcryptofolder.c:72-77`:
//! ```c
//! typedef struct {
//!   uint32_t type;     // PSYNC_CRYPTO_TYPE_RSA4096_64BYTESALT_20000IT = 0
//!   uint32_t flags;
//!   unsigned char salt[PSYNC_CRYPTO_PBKDF2_SALT_LEN];   // 64
//!   unsigned char key[];                                 // AES-256-CTR
//!                                                        // wrap of PKCS#1
//!                                                        // DER priv key
//! } priv_key_ver1;
//! ```
//!
//! PBKDF2 iterations are **hardcoded** at 20000
//! (`pcryptofolder.c:1831, 1858` → `psymkey_generate(..., 20000)`).
//! There is **no iteration or length field** in the struct — the outer length
//! is carried by whatever framing (server response / local DB blob) holds it.
//!
//! ## On-disk layout reference (pub_key_ver1)
//!
//! From `C_CODE/pclsync/pcryptofolder.c:79-83`:
//! ```c
//! typedef struct {
//!   uint32_t type;     // PSYNC_CRYPTO_PUB_TYPE_RSA4096 = 0
//!   uint32_t flags;
//!   unsigned char key[];  // PKCS#1 DER public key
//! } pub_key_ver1;
//! ```
//!
//! ## Counter-start for AES-CTR priv-key wrap
//!
//! The counter starts at **0**. See `pcryptofolder.c:1845` (decrypt) and
//! `:1867` (re-encrypt) both call `pcrypto_ctr_encdec_decode(enc, ..., 0)`.
//! Our `pclsync_modes::aes256_ctr_pclsync_xor_inplace` takes a `block_index`
//! argument; we pass `0` to match.

use pcloud_secret::secret_string::SecretString;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::pclsync_kdf;
use crate::pclsync_modes;
use crate::pclsync_rsa;

/// PSYNC_CRYPTO_TYPE_RSA4096_64BYTESALT_20000IT (see
/// `C_CODE/pclsync/psettings.h:173`).
pub const PSYNC_CRYPTO_TYPE_RSA4096_64BYTESALT_20000IT: u32 = 0;

/// PSYNC_CRYPTO_PUB_TYPE_RSA4096 (see `C_CODE/pclsync/psettings.h:174`).
pub const PSYNC_CRYPTO_PUB_TYPE_RSA4096: u32 = 0;

/// PSYNC_CRYPTO_PBKDF2_SALT_LEN (see `C_CODE/pclsync/psettings.h:169`).
pub const PCLSYNC_PBKDF2_SALT_LEN: usize = 64;

/// Fixed PBKDF2 iteration count for PclsyncCompat — hardcoded in the C
/// client (`pcryptofolder.c:1831` etc.). Exposed for parity proofs.
pub const PCLSYNC_PBKDF2_ITERATIONS: u32 = 20000;

/// Errors originating from the PclsyncCompat profile codec.
#[derive(Debug, thiserror::Error)]
pub enum PclsyncCompatError {
    /// Input blob is shorter than the fixed priv_key_ver1 header
    /// (`type u32 + flags u32 + salt[64]` = 72 bytes).
    #[error("priv_key_ver1 blob truncated")]
    PrivKeyTruncated,
    /// Input blob is shorter than the fixed pub_key_ver1 header
    /// (`type u32 + flags u32` = 8 bytes).
    #[error("pub_key_ver1 blob truncated")]
    PubKeyTruncated,
    /// `type` field did not match PSYNC_CRYPTO_TYPE_RSA4096_64BYTESALT_20000IT.
    #[error("unsupported priv_key_ver1 type: {0}")]
    UnsupportedPrivType(u32),
    /// `type` field did not match PSYNC_CRYPTO_PUB_TYPE_RSA4096.
    #[error("unsupported pub_key_ver1 type: {0}")]
    UnsupportedPubType(u32),
    /// Underlying RSA primitive returned an error (DER parse, keygen, etc.).
    #[error("rsa primitive error: {0}")]
    Rsa(#[from] pclsync_rsa::PclsyncRsaError),
    /// getrandom host failure during profile setup.
    #[error("OS RNG failure during pclsync-compat setup")]
    Rng,
}

/// Persisted PclsyncCompat profile — survives daemon restarts.
///
/// The `priv_key_ver1_blob` field **contains ciphertext** (AES-256-CTR over
/// the PKCS#1 DER-serialised RSA private key, keyed by PBKDF2(password,
/// salt, 20000)). The salt is held as a field of the blob itself; storing
/// it separately would be redundant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PclsyncCompatProfile {
    /// Exact bytes of the `priv_key_ver1` struct as the C client would
    /// upload them via `crypto_setuserkeys`. Contains ciphertext priv key.
    pub priv_key_ver1_blob: Vec<u8>,
    /// Exact bytes of the `pub_key_ver1` struct as the C client would
    /// upload. Safe-to-log public material.
    pub pub_key_ver1_blob: Vec<u8>,
    /// Non-secret HMAC-keyed fingerprint of the pub-key bytes, used by
    /// `start_pclsync_compat` to reject wrong passwords **before** the
    /// unwrapped priv key is ever exposed to higher layers. The HMAC key
    /// is the low 32 bytes of the derived KEK — ie an attacker who already
    /// knows the password can reproduce it, but an attacker who does not
    /// learns nothing from observing this value.
    pub pub_fingerprint: [u8; 32],
    /// `flags` field copied from the priv_key_ver1 struct. Exposed so
    /// `CryptoShell::priv_key_flags()` can report it.
    pub flags: u32,
}

impl PclsyncCompatProfile {
    /// Parse the raw `priv_key_ver1` header. Returns (type, flags, salt,
    /// ciphertext-priv-der-bytes).
    ///
    /// Cited from `C_CODE/pclsync/pcryptofolder.c:72-77` (struct) and
    /// `:270-284` (server-side parsing uses `offsetof(priv_key_ver1, key)`
    /// for header length, same as here).
    #[allow(clippy::type_complexity)]
    pub fn parse_priv_blob(
        blob: &[u8],
    ) -> Result<(u32, u32, [u8; PCLSYNC_PBKDF2_SALT_LEN], Vec<u8>), PclsyncCompatError> {
        const HEADER_LEN: usize = 4 + 4 + PCLSYNC_PBKDF2_SALT_LEN;
        if blob.len() < HEADER_LEN {
            return Err(PclsyncCompatError::PrivKeyTruncated);
        }
        let typ = u32::from_le_bytes(blob[0..4].try_into().expect("checked len"));
        let flags = u32::from_le_bytes(blob[4..8].try_into().expect("checked len"));
        if typ != PSYNC_CRYPTO_TYPE_RSA4096_64BYTESALT_20000IT {
            return Err(PclsyncCompatError::UnsupportedPrivType(typ));
        }
        let mut salt = [0u8; PCLSYNC_PBKDF2_SALT_LEN];
        salt.copy_from_slice(&blob[8..HEADER_LEN]);
        let ct = blob[HEADER_LEN..].to_vec();
        Ok((typ, flags, salt, ct))
    }

    /// Build a raw `priv_key_ver1` blob from header fields + ciphertext-DER.
    pub fn build_priv_blob(
        flags: u32,
        salt: &[u8; PCLSYNC_PBKDF2_SALT_LEN],
        ciphertext_der: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 4 + PCLSYNC_PBKDF2_SALT_LEN + ciphertext_der.len());
        out.extend_from_slice(&PSYNC_CRYPTO_TYPE_RSA4096_64BYTESALT_20000IT.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(salt);
        out.extend_from_slice(ciphertext_der);
        out
    }

    /// Parse the raw `pub_key_ver1` header. Returns (type, flags, pub-der).
    pub fn parse_pub_blob(blob: &[u8]) -> Result<(u32, u32, Vec<u8>), PclsyncCompatError> {
        const HEADER_LEN: usize = 4 + 4;
        if blob.len() < HEADER_LEN {
            return Err(PclsyncCompatError::PubKeyTruncated);
        }
        let typ = u32::from_le_bytes(blob[0..4].try_into().expect("checked len"));
        let flags = u32::from_le_bytes(blob[4..8].try_into().expect("checked len"));
        if typ != PSYNC_CRYPTO_PUB_TYPE_RSA4096 {
            return Err(PclsyncCompatError::UnsupportedPubType(typ));
        }
        Ok((typ, flags, blob[HEADER_LEN..].to_vec()))
    }

    /// Build a raw `pub_key_ver1` blob.
    pub fn build_pub_blob(flags: u32, pub_der: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 4 + pub_der.len());
        out.extend_from_slice(&PSYNC_CRYPTO_PUB_TYPE_RSA4096.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(pub_der);
        out
    }
}

/// HMAC-SHA-256 fingerprint used to reject wrong passwords in constant time.
///
/// `key = first 32 bytes of derived KEK`, `msg = pub_key_ver1_blob`.
fn pub_fingerprint(kek_key: &[u8; 32], pub_blob: &[u8]) -> [u8; 32] {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(kek_key)
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(pub_blob);
    mac.finalize().into_bytes().into()
}

/// Create a brand-new PclsyncCompat profile: generate RSA-4096 keypair,
/// wrap the priv key under PBKDF2(password, salt, 20000), serialise both
/// the priv_key_ver1 and pub_key_ver1 blobs, and compute the
/// wrong-password fingerprint.
///
/// The plaintext DER priv-key material is zeroised before return; only
/// the encrypted blob escapes.
pub fn generate_profile(
    password: &SecretString,
) -> Result<PclsyncCompatProfile, PclsyncCompatError> {
    // 1. Fresh 64-byte salt.
    let mut salt = [0u8; PCLSYNC_PBKDF2_SALT_LEN];
    getrandom::getrandom(&mut salt).map_err(|_| PclsyncCompatError::Rng)?;

    // 2. Derive KEK.
    let kek = pclsync_kdf::derive_kek(password, &salt);

    // 3. Generate RSA-4096 keypair.
    let keypair = pclsync_rsa::generate_keypair()?;
    let (priv_key, pub_key) = keypair.into_parts();

    // 4. Serialise both halves to PKCS#1 DER.
    let mut priv_der = pclsync_rsa::serialize_priv_key_der(&priv_key)?;
    let pub_der = pclsync_rsa::serialize_pub_key_der(&pub_key)?;

    // 5. Wrap priv DER with AES-256-CTR (counter starts at 0 — see
    //    `pcryptofolder.c:1845` and `:1867` where the C client calls
    //    `pcrypto_ctr_encdec_decode(..., 0)`).
    pclsync_modes::aes256_ctr_pclsync_xor_inplace(
        &kek.key,
        &kek.iv,
        0,
        &mut priv_der,
    );

    // 6. Build pub_key_ver1 + priv_key_ver1 blobs (flags = 0 for fresh profile).
    let flags: u32 = 0;
    let pub_blob = PclsyncCompatProfile::build_pub_blob(flags, &pub_der);
    let priv_blob = PclsyncCompatProfile::build_priv_blob(flags, &salt, &priv_der);
    priv_der.zeroize();

    // 7. Compute non-secret pub fingerprint for wrong-password rejection.
    let fpr = pub_fingerprint(&kek.key,&pub_blob);

    Ok(PclsyncCompatProfile {
        priv_key_ver1_blob: priv_blob,
        pub_key_ver1_blob: pub_blob,
        pub_fingerprint: fpr,
        flags,
    })
}

/// Unwrap the stored profile under the given password, returning the
/// runtime state on success. Rejects wrong passwords in constant time
/// **before** the RSA private key is parsed, via the stored pub-key
/// fingerprint.
pub fn unlock_profile(
    password: &SecretString,
    profile: &PclsyncCompatProfile,
) -> Result<PclsyncCompatState, PclsyncCompatError> {
    // 1. Parse priv header and extract salt + ciphertext.
    let (_typ, _flags, salt, mut ct_der) =
        PclsyncCompatProfile::parse_priv_blob(&profile.priv_key_ver1_blob)?;

    // 2. Derive KEK from password + salt.
    let kek = pclsync_kdf::derive_kek(password, &salt);

    // 3. Constant-time pub fingerprint check — reject wrong password
    //    BEFORE exposing the unwrapped priv key.
    let expected = pub_fingerprint(&kek.key,&profile.pub_key_ver1_blob);
    let fp_ok: bool = expected.ct_eq(&profile.pub_fingerprint).into();
    if !fp_ok {
        ct_der.zeroize();
        return Err(PclsyncCompatError::Rsa(
            pclsync_rsa::PclsyncRsaError::Oaep, // coerced: "wrong key / bad padding"
        ));
    }

    // 4. Unwrap priv DER (counter = 0, same as setup).
    pclsync_modes::aes256_ctr_pclsync_xor_inplace(
        &kek.key,
        &kek.iv,
        0,
        &mut ct_der,
    );

    // 5. Parse the RSA priv key.
    let priv_key = pclsync_rsa::parse_priv_key_der(&ct_der).inspect_err(|_| {
        ct_der.zeroize();
    })?;
    ct_der.zeroize();

    Ok(PclsyncCompatState::new(priv_key))
}

/// Runtime-only PclsyncCompat state. Holds the unwrapped RSA private
/// key and lazy caches of folder/file symmetric keys obtained via
/// `crypto_getfolderkey` (Stage 4 will populate these).
pub struct PclsyncCompatState {
    priv_key: rsa::RsaPrivateKey,
    folder_keys: std::collections::HashMap<u64, pclsync_rsa::SymKeyVer1>,
    file_keys: std::collections::HashMap<u64, pclsync_rsa::SymKeyVer1>,
}

impl PclsyncCompatState {
    fn new(priv_key: rsa::RsaPrivateKey) -> Self {
        Self {
            priv_key,
            folder_keys: std::collections::HashMap::new(),
            file_keys: std::collections::HashMap::new(),
        }
    }

    /// Borrow the live RSA private key (for `crypto_getfolderkey` unwrap).
    #[must_use]
    pub fn priv_key(&self) -> &rsa::RsaPrivateKey {
        &self.priv_key
    }

    /// Insert a folder sym-key cache entry (called by the daemon after
    /// `crypto_getfolderkey` returns). Overwrites any prior entry for
    /// the same folder id.
    pub fn cache_folder_key(&mut self, folder_id: u64, sym: pclsync_rsa::SymKeyVer1) {
        self.folder_keys.insert(folder_id, sym);
    }

    /// Insert a file sym-key cache entry.
    pub fn cache_file_key(&mut self, file_id: u64, sym: pclsync_rsa::SymKeyVer1) {
        self.file_keys.insert(file_id, sym);
    }

    /// Look up a cached folder sym-key.
    #[must_use]
    pub fn folder_key(&self, folder_id: u64) -> Option<&pclsync_rsa::SymKeyVer1> {
        self.folder_keys.get(&folder_id)
    }

    /// Look up a cached file sym-key.
    #[must_use]
    pub fn file_key(&self, file_id: u64) -> Option<&pclsync_rsa::SymKeyVer1> {
        self.file_keys.get(&file_id)
    }
}

impl core::fmt::Debug for PclsyncCompatState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PclsyncCompatState")
            .field("priv_key", &"<redacted rsa-4096>")
            .field("folder_keys", &format_args!("{} entries", self.folder_keys.len()))
            .field("file_keys", &format_args!("{} entries", self.file_keys.len()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_then_unlock_roundtrip() {
        let pw = SecretString::new("hunter2-pclsync");
        let profile = generate_profile(&pw).expect("generate");
        // priv_key_ver1: 4 + 4 + 64 + at-least-something.
        assert!(profile.priv_key_ver1_blob.len() > 4 + 4 + 64);
        // pub_key_ver1: 4 + 4 + DER.
        assert!(profile.pub_key_ver1_blob.len() > 4 + 4);
        let state = unlock_profile(&pw, &profile).expect("unlock");
        // Priv key should round-trip: check modulus size.
        use rsa::traits::PublicKeyParts;
        assert_eq!(state.priv_key().n().bits(), 4096);
    }

    #[test]
    fn wrong_password_rejected_in_constant_time() {
        let pw = SecretString::new("hunter2-pclsync");
        let wrong = SecretString::new("nothunter");
        let profile = generate_profile(&pw).expect("generate");
        let err = unlock_profile(&wrong, &profile).expect_err("wrong pw");
        // We coerce to Rsa(Oaep) in the constant-time-reject branch.
        assert!(matches!(err, PclsyncCompatError::Rsa(_)));
    }

    #[test]
    fn priv_blob_round_trip() {
        let salt = [0x42u8; PCLSYNC_PBKDF2_SALT_LEN];
        let ct = vec![0xAAu8; 2048];
        let blob = PclsyncCompatProfile::build_priv_blob(7, &salt, &ct);
        let (typ, flags, salt2, ct2) =
            PclsyncCompatProfile::parse_priv_blob(&blob).expect("parse");
        assert_eq!(typ, PSYNC_CRYPTO_TYPE_RSA4096_64BYTESALT_20000IT);
        assert_eq!(flags, 7);
        assert_eq!(salt2, salt);
        assert_eq!(ct2, ct);
    }

    #[test]
    fn pub_blob_round_trip() {
        let der = vec![0xBBu8; 550];
        let blob = PclsyncCompatProfile::build_pub_blob(3, &der);
        let (typ, flags, der2) =
            PclsyncCompatProfile::parse_pub_blob(&blob).expect("parse");
        assert_eq!(typ, PSYNC_CRYPTO_PUB_TYPE_RSA4096);
        assert_eq!(flags, 3);
        assert_eq!(der2, der);
    }

    #[test]
    fn priv_blob_truncated_rejects() {
        let tiny = [0u8; 16];
        assert!(matches!(
            PclsyncCompatProfile::parse_priv_blob(&tiny).unwrap_err(),
            PclsyncCompatError::PrivKeyTruncated
        ));
    }

    #[test]
    fn priv_blob_wrong_type_rejects() {
        let mut blob = vec![0u8; 4 + 4 + PCLSYNC_PBKDF2_SALT_LEN];
        blob[0..4].copy_from_slice(&42u32.to_le_bytes());
        assert!(matches!(
            PclsyncCompatProfile::parse_priv_blob(&blob).unwrap_err(),
            PclsyncCompatError::UnsupportedPrivType(42)
        ));
    }
}
