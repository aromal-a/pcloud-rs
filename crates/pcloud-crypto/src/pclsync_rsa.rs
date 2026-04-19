#![forbid(unsafe_code)]
//! # pclsync-compatible RSA-4096 keypair generation and RSAES-OAEP wrap/unwrap
//! (Wave 1, Primitive B)
//!
//! Mirrors the mbedtls defaults used by the legacy C client. Concrete
//! parameters were cited from the C tree before implementation:
//!
//! - `PSYNC_CRYPTO_RSA_SIZE = 4096`
//!   ([`psettings.h:171`][1])
//! - `mbedtls_rsa_init` followed by
//!   `mbedtls_rsa_set_padding(ctx, MBEDTLS_RSA_PKCS_V21, MBEDTLS_MD_SHA1)`
//!   ([`pssl.c:485-486`][2], [`pssl.c:520-521`][3], [`pssl.c:592-593`][4],
//!   [`pssl.c:650-651`][5]). `MBEDTLS_RSA_PKCS_V21` selects RSAES-OAEP;
//!   `MBEDTLS_MD_SHA1` sets BOTH the OAEP hash and the MGF1 hash (mbedtls
//!   derives MGF1 from the same digest when none is configured separately).
//! - `mbedtls_rsa_rsaes_oaep_encrypt(..., label=NULL, label_len=0, ...)`
//!   ([`pssl.c:727-729`][6]) — empty OAEP label.
//! - `mbedtls_rsa_rsaes_oaep_decrypt(..., label=NULL, label_len=0, ...)`
//!   ([`pssl.c:748-750`][7]) — empty OAEP label.
//!
//! `sym_key_ver1` layout (the plaintext fed into OAEP) is defined in
//! [`pcryptofolder.c:85-90`][8]:
//!
//! ```c
//! typedef struct {
//!   uint32_t type;                                          // LE u32
//!   uint32_t flags;                                         // LE u32
//!   unsigned char aeskey[PSYNC_AES256_KEY_SIZE];            // 32 bytes
//!   unsigned char hmackey[PSYNC_CRYPTO_HMAC_SHA512_KEY_LEN];// 128 bytes
//! } sym_key_ver1;
//! ```
//!
//! with `PSYNC_AES256_KEY_SIZE = 32` ([`pssl.h:50`][9]) and
//! `PSYNC_CRYPTO_HMAC_SHA512_KEY_LEN = 128` ([`psettings.h:170`][10]).
//! Total size = 4 + 4 + 32 + 128 = **168 bytes**. The `type` field holds
//! `PSYNC_CRYPTO_SYM_AES256_1024BIT_HMAC = 0` ([`psettings.h:175`][11]).
//!
//! The layout the Wave 1 specification sketched (`u8 version + u8 reserved
//! + u16 aeslen LE + u16 hmaclen LE + aes + hmac`) is **not** what the C
//! client emits; the real wire layout has an 8-byte header of two
//! little-endian u32 fields (`type`, `flags`). We serialize the C struct
//! verbatim so OAEP ciphertexts interop byte-for-byte with the legacy
//! client.
//!
//! [1]: ../../C_CODE/pclsync/psettings.h
//! [2]: ../../C_CODE/pclsync/pssl.c
//! [3]: ../../C_CODE/pclsync/pssl.c
//! [4]: ../../C_CODE/pclsync/pssl.c
//! [5]: ../../C_CODE/pclsync/pssl.c
//! [6]: ../../C_CODE/pclsync/pssl.c
//! [7]: ../../C_CODE/pclsync/pssl.c
//! [8]: ../../C_CODE/pclsync/pcryptofolder.c
//! [9]: ../../C_CODE/pclsync/pssl.h
//! [10]: ../../C_CODE/pclsync/psettings.h
//! [11]: ../../C_CODE/pclsync/psettings.h
//!
//! # Gating
//!
//! This module is only compiled when the `pclsync-v2` feature is active.

// **PLATFORM:** all
// **GATING:** feature = "pclsync-v2"

use pkcs1::{DecodeRsaPrivateKey, DecodeRsaPublicKey, EncodeRsaPrivateKey, EncodeRsaPublicKey};
use rsa::rand_core::OsRng;
pub use rsa::{RsaPrivateKey, RsaPublicKey};
use rsa::Oaep;
use sha1::Sha1;
use static_assertions::const_assert_eq;
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// RSA modulus size in bits (matches `PSYNC_CRYPTO_RSA_SIZE`, `psettings.h:171`).
pub const PCLSYNC_RSA_BITS: usize = 4096;

/// RSA modulus size in bytes.
pub const PCLSYNC_RSA_BYTES: usize = PCLSYNC_RSA_BITS / 8;

/// AES-256 key size (`PSYNC_AES256_KEY_SIZE`, `pssl.h:50`).
pub const PCLSYNC_AES_KEY_LEN: usize = 32;

/// HMAC-SHA-512 key size (`PSYNC_CRYPTO_HMAC_SHA512_KEY_LEN`, `psettings.h:170`).
pub const PCLSYNC_HMAC_KEY_LEN: usize = 128;

/// `sym_key_ver1` header length (two little-endian u32 fields).
pub const PCLSYNC_SYM_KEY_VER1_HEADER: usize = 8;

/// Serialized `sym_key_ver1` size = 4 + 4 + 32 + 128.
pub const PCLSYNC_SYM_KEY_VER1_SIZE: usize =
    PCLSYNC_SYM_KEY_VER1_HEADER + PCLSYNC_AES_KEY_LEN + PCLSYNC_HMAC_KEY_LEN;

// Static layout assertions — these trip at compile time if any constant is
// edited out of lockstep with the C struct.
const_assert_eq!(PCLSYNC_SYM_KEY_VER1_SIZE, 168);
const_assert_eq!(PCLSYNC_RSA_BYTES, 512);

/// `PSYNC_CRYPTO_SYM_AES256_1024BIT_HMAC = 0` (`psettings.h:175`).
pub const PCLSYNC_SYM_TYPE_AES256_1024BIT_HMAC: u32 = 0;

/// Errors returned by the pclsync-v2 RSA primitives.
#[derive(Debug, thiserror::Error)]
pub enum PclsyncRsaError {
    /// RSA key generation failed.
    #[error("RSA-4096 keygen failed: {0}")]
    KeyGen(String),

    /// RSAES-OAEP encrypt/decrypt failed (wrong key, corrupted ciphertext,
    /// padding error, etc.). The legacy C client treats all such failures
    /// as indistinguishable; we do likewise to avoid padding-oracle leaks.
    #[error("RSAES-OAEP operation failed")]
    Oaep,

    /// Ciphertext length was not exactly RSA-4096 modulus size (512 bytes).
    #[error("wrapped key length {got} != expected {expected}")]
    WrongCiphertextLen {
        /// Length of the ciphertext actually supplied.
        got: usize,
        /// Length required (always [`PCLSYNC_RSA_BYTES`]).
        expected: usize,
    },

    /// Buffer fed to [`parse_sym_key_ver1`] was the wrong length.
    #[error("sym_key_ver1 buffer length {got} != expected {expected}")]
    WrongSymKeyLen {
        /// Length of the buffer actually supplied.
        got: usize,
        /// Length required (always [`PCLSYNC_SYM_KEY_VER1_SIZE`]).
        expected: usize,
    },

    /// `type` field in the `sym_key_ver1` header did not match the
    /// supported value `PSYNC_CRYPTO_SYM_AES256_1024BIT_HMAC`.
    #[error("unsupported sym_key_ver1 type field: {0}")]
    UnsupportedSymKeyType(u32),

    /// DER encoding or decoding failed.
    #[error("DER codec error: {0}")]
    Der(String),
}

/// RSA-4096 keypair (private half also caches a derived public half).
pub struct RsaKeyPair {
    private: RsaPrivateKey,
    public: RsaPublicKey,
}

impl RsaKeyPair {
    /// Borrow the private key (never exposed via `Debug`; the `rsa` crate
    /// does not implement `Debug` for it).
    pub fn private(&self) -> &RsaPrivateKey {
        &self.private
    }

    /// Borrow the public half.
    pub fn public(&self) -> &RsaPublicKey {
        &self.public
    }

    /// Split into (private, public). Destroys the pair wrapper.
    pub fn into_parts(self) -> (RsaPrivateKey, RsaPublicKey) {
        (self.private, self.public)
    }
}

/// The plaintext `sym_key_ver1` structure that gets wrapped by RSAES-OAEP.
///
/// `Debug` is manually implemented to redact key material. Both key buffers
/// are zeroized on drop via the derive-wired `ZeroizeOnDrop` impl.
#[derive(ZeroizeOnDrop)]
pub struct SymKeyVer1 {
    /// Matches `sym_key_ver1::type`. Legacy clients always set this to
    /// `PSYNC_CRYPTO_SYM_AES256_1024BIT_HMAC` (= 0). Exposed so higher
    /// layers that round-trip a specific wrapped key can preserve the
    /// original value byte-for-byte.
    pub sym_type: u32,

    /// Matches `sym_key_ver1::flags`. Callers wrapping a fresh per-file
    /// key typically set `0`; folder keys set `PSYNC_CRYPTO_SYM_FLAG_ISDIR`.
    pub flags: u32,

    /// 32-byte AES-256 key. Zeroized on drop.
    ///
    /// `pub(crate)`: external code must not copy raw key material; use
    /// `Arc<SymKeyVer1>` sharing or the `ct_eq` accessor instead. Making the
    /// field crate-internal prevents accidental bypass of the no-`Clone`
    /// discipline (audit-06 LOW-2.4).
    pub(crate) aes_key: [u8; PCLSYNC_AES_KEY_LEN],

    /// 128-byte HMAC-SHA-512 key. Zeroized on drop.
    ///
    /// `pub(crate)`: same rationale as `aes_key` — crate-level access only
    /// to prevent raw key-material copies escaping `pcloud-crypto` (audit-06
    /// LOW-2.4).
    pub(crate) hmac_key: [u8; PCLSYNC_HMAC_KEY_LEN],
}

impl core::fmt::Debug for SymKeyVer1 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SymKeyVer1")
            .field("sym_type", &self.sym_type)
            .field("flags", &self.flags)
            .field("aes_key", &"<redacted 32 bytes>")
            .field("hmac_key", &"<redacted 128 bytes>")
            .finish()
    }
}

impl SymKeyVer1 {
    /// Build a fresh zeroed `sym_key_ver1` shell with `type = 0` and
    /// the caller-supplied `flags`. Key material must then be filled in.
    pub fn new(flags: u32) -> Self {
        Self {
            sym_type: PCLSYNC_SYM_TYPE_AES256_1024BIT_HMAC,
            flags,
            aes_key: [0u8; PCLSYNC_AES_KEY_LEN],
            hmac_key: [0u8; PCLSYNC_HMAC_KEY_LEN],
        }
    }

    /// Test-only deep copy of the key material. Deliberately not a
    /// `Clone` impl — `pcloud-secret/src/lib.rs:26` forbids `Clone` on
    /// secret-bearing types so every duplication is audit-visible.
    /// Production code must share via `Arc<SymKeyVer1>` instead.
    #[cfg(test)]
    pub fn duplicate(&self) -> Self {
        Self {
            sym_type: self.sym_type,
            flags: self.flags,
            aes_key: self.aes_key,
            hmac_key: self.hmac_key,
        }
    }

    /// Constant-time equality over the full struct contents.
    pub fn ct_eq(&self, other: &Self) -> subtle::Choice {
        let mut eq = self.sym_type.ct_eq(&other.sym_type);
        eq &= self.flags.ct_eq(&other.flags);
        eq &= self.aes_key.ct_eq(&other.aes_key);
        eq &= self.hmac_key.ct_eq(&other.hmac_key);
        eq
    }
}

/// Generate a fresh RSA-4096 keypair using the OS RNG (matches
/// `pssl_gen_rsa(PSYNC_CRYPTO_RSA_SIZE)` at `pssl.c:482-495`).
pub fn generate_keypair() -> Result<RsaKeyPair, PclsyncRsaError> {
    let mut rng = OsRng;
    let private = RsaPrivateKey::new(&mut rng, PCLSYNC_RSA_BITS)
        .map_err(|e| PclsyncRsaError::KeyGen(e.to_string()))?;
    let public = RsaPublicKey::from(&private);
    Ok(RsaKeyPair { private, public })
}

/// Wrap a `SymKeyVer1` with RSAES-OAEP-SHA-1 / MGF1-SHA-1 / empty label.
///
/// Mirrors `prsa_encrypt_data` (`pssl.c:718-740`). Returns exactly
/// `PCLSYNC_RSA_BYTES` (= 512) bytes.
pub fn oaep_wrap(
    pubkey: &RsaPublicKey,
    sym: &SymKeyVer1,
) -> Result<Vec<u8>, PclsyncRsaError> {
    let mut plaintext = serialize_sym_key_ver1(sym);
    let mut rng = OsRng;
    let padding = Oaep::new::<Sha1>();
    let ct = pubkey
        .encrypt(&mut rng, padding, &plaintext)
        .map_err(|_| PclsyncRsaError::Oaep);
    plaintext.zeroize();
    let ct = ct?;
    if ct.len() != PCLSYNC_RSA_BYTES {
        return Err(PclsyncRsaError::WrongCiphertextLen {
            got: ct.len(),
            expected: PCLSYNC_RSA_BYTES,
        });
    }
    Ok(ct)
}

/// Unwrap an RSAES-OAEP-wrapped `sym_key_ver1`. Mirrors
/// `prsa_decrypt_data` (`pssl.c:742-758`).
pub fn oaep_unwrap(
    privkey: &RsaPrivateKey,
    wrapped: &[u8],
) -> Result<SymKeyVer1, PclsyncRsaError> {
    if wrapped.len() != PCLSYNC_RSA_BYTES {
        return Err(PclsyncRsaError::WrongCiphertextLen {
            got: wrapped.len(),
            expected: PCLSYNC_RSA_BYTES,
        });
    }
    let padding = Oaep::new::<Sha1>();
    let mut plaintext = privkey
        .decrypt(padding, wrapped)
        .map_err(|_| PclsyncRsaError::Oaep)?;
    let parsed = parse_sym_key_ver1(&plaintext);
    plaintext.zeroize();
    parsed
}

/// Serialize `sym_key_ver1` to the exact 168-byte legacy wire layout:
/// `type:u32_le || flags:u32_le || aes_key[32] || hmac_key[128]`.
pub fn serialize_sym_key_ver1(sym: &SymKeyVer1) -> [u8; PCLSYNC_SYM_KEY_VER1_SIZE] {
    let mut out = [0u8; PCLSYNC_SYM_KEY_VER1_SIZE];
    out[0..4].copy_from_slice(&sym.sym_type.to_le_bytes());
    out[4..8].copy_from_slice(&sym.flags.to_le_bytes());
    out[8..8 + PCLSYNC_AES_KEY_LEN].copy_from_slice(&sym.aes_key);
    out[8 + PCLSYNC_AES_KEY_LEN..].copy_from_slice(&sym.hmac_key);
    out
}

/// Parse `sym_key_ver1` from a byte buffer. Rejects:
/// - any length other than 168 bytes,
/// - `type` values other than `PSYNC_CRYPTO_SYM_AES256_1024BIT_HMAC`.
pub fn parse_sym_key_ver1(buf: &[u8]) -> Result<SymKeyVer1, PclsyncRsaError> {
    if buf.len() != PCLSYNC_SYM_KEY_VER1_SIZE {
        return Err(PclsyncRsaError::WrongSymKeyLen {
            got: buf.len(),
            expected: PCLSYNC_SYM_KEY_VER1_SIZE,
        });
    }
    let sym_type = u32::from_le_bytes(buf[0..4].try_into().expect("checked len"));
    let flags = u32::from_le_bytes(buf[4..8].try_into().expect("checked len"));
    if sym_type != PCLSYNC_SYM_TYPE_AES256_1024BIT_HMAC {
        return Err(PclsyncRsaError::UnsupportedSymKeyType(sym_type));
    }
    let mut aes_key = [0u8; PCLSYNC_AES_KEY_LEN];
    aes_key.copy_from_slice(&buf[8..8 + PCLSYNC_AES_KEY_LEN]);
    let mut hmac_key = [0u8; PCLSYNC_HMAC_KEY_LEN];
    hmac_key.copy_from_slice(&buf[8 + PCLSYNC_AES_KEY_LEN..]);
    Ok(SymKeyVer1 {
        sym_type,
        flags,
        aes_key,
        hmac_key,
    })
}

/// Serialize an `RsaPrivateKey` to PKCS#1 DER (matches
/// `mbedtls_pk_write_key_der` at `pssl.c:567`).
pub fn serialize_priv_key_der(privkey: &RsaPrivateKey) -> Result<Vec<u8>, PclsyncRsaError> {
    privkey
        .to_pkcs1_der()
        .map(|d| d.as_bytes().to_vec())
        .map_err(|e| PclsyncRsaError::Der(e.to_string()))
}

/// Parse a PKCS#1 DER-encoded RSA private key (matches `mbedtls_pk_parse_key`
/// at `pssl.c:644`).
pub fn parse_priv_key_der(der: &[u8]) -> Result<RsaPrivateKey, PclsyncRsaError> {
    RsaPrivateKey::from_pkcs1_der(der).map_err(|e| PclsyncRsaError::Der(e.to_string()))
}

/// Serialize an `RsaPublicKey` to PKCS#1 DER (matches
/// `mbedtls_pk_write_pubkey` at `pssl.c:546`).
pub fn serialize_pub_key_der(pubkey: &RsaPublicKey) -> Result<Vec<u8>, PclsyncRsaError> {
    pubkey
        .to_pkcs1_der()
        .map(|d| d.as_bytes().to_vec())
        .map_err(|e| PclsyncRsaError::Der(e.to_string()))
}

/// Parse a PKCS#1 DER-encoded RSA public key (matches
/// `mbedtls_pk_parse_public_key` at `pssl.c:587`).
pub fn parse_pub_key_der(der: &[u8]) -> Result<RsaPublicKey, PclsyncRsaError> {
    RsaPublicKey::from_pkcs1_der(der).map_err(|e| PclsyncRsaError::Der(e.to_string()))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::traits::PublicKeyParts;

    // Reduce test slowness: one shared keypair parsed from the committed DER
    // fixture (generated out-of-band with `openssl genpkey -algorithm RSA
    // -pkeyopt rsa_keygen_bits:4096 -outform DER -out priv_key.der`). Live
    // keygen is still exercised by `generate_keypair_bits_4096`.
    const PRIV_KEY_DER: &[u8] =
        include_bytes!("../tests/fixtures/pclsync_v2/priv_key.der");

    fn fixture_priv() -> RsaPrivateKey {
        parse_priv_key_der(PRIV_KEY_DER).expect("fixture DER must parse")
    }

    #[test]
    fn sym_key_ver1_size_is_168_bytes() {
        assert_eq!(PCLSYNC_SYM_KEY_VER1_SIZE, 168);
        let sym = SymKeyVer1::new(0);
        let bytes = serialize_sym_key_ver1(&sym);
        assert_eq!(bytes.len(), 168);
    }

    #[test]
    fn sym_key_ver1_roundtrip() {
        let mut sym = SymKeyVer1::new(0x1234_5678);
        for (i, b) in sym.aes_key.iter_mut().enumerate() {
            *b = i as u8;
        }
        for (i, b) in sym.hmac_key.iter_mut().enumerate() {
            *b = (i ^ 0xA5) as u8;
        }
        let bytes = serialize_sym_key_ver1(&sym);
        let parsed = parse_sym_key_ver1(&bytes).expect("parse ok");
        assert_eq!(parsed.sym_type, sym.sym_type);
        assert_eq!(parsed.flags, sym.flags);
        assert_eq!(parsed.aes_key, sym.aes_key);
        assert_eq!(parsed.hmac_key, sym.hmac_key);
        // Verify little-endian header layout.
        assert_eq!(&bytes[0..4], &0u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &0x1234_5678u32.to_le_bytes());
    }

    #[test]
    fn sym_key_ver1_parse_rejects_wrong_len() {
        let short = [0u8; 100];
        assert!(matches!(
            parse_sym_key_ver1(&short),
            Err(PclsyncRsaError::WrongSymKeyLen { got: 100, expected: 168 })
        ));
        let long = [0u8; 200];
        assert!(matches!(
            parse_sym_key_ver1(&long),
            Err(PclsyncRsaError::WrongSymKeyLen { got: 200, expected: 168 })
        ));
    }

    #[test]
    fn sym_key_ver1_parse_rejects_wrong_type() {
        // Set type=2 (not PSYNC_CRYPTO_SYM_AES256_1024BIT_HMAC=0).
        let mut buf = [0u8; PCLSYNC_SYM_KEY_VER1_SIZE];
        buf[0..4].copy_from_slice(&2u32.to_le_bytes());
        match parse_sym_key_ver1(&buf) {
            Err(PclsyncRsaError::UnsupportedSymKeyType(t)) => assert_eq!(t, 2),
            other => panic!("expected UnsupportedSymKeyType, got {other:?}"),
        }
    }

    #[test]
    fn generate_keypair_bits_4096() {
        let kp = generate_keypair().expect("keygen succeeds");
        // Modulus should be exactly 4096 bits; n.bits() returns the bit length.
        assert_eq!(kp.public().n().bits(), PCLSYNC_RSA_BITS);
        // Size in bytes.
        assert_eq!(kp.public().size(), PCLSYNC_RSA_BYTES);
    }

    #[test]
    fn oaep_wrap_unwrap_roundtrip() {
        let priv_key = fixture_priv();
        let pub_key = RsaPublicKey::from(&priv_key);
        let mut sym = SymKeyVer1::new(0xDEAD_BEEF);
        for (i, b) in sym.aes_key.iter_mut().enumerate() {
            *b = (i * 7) as u8;
        }
        for (i, b) in sym.hmac_key.iter_mut().enumerate() {
            *b = (i * 11 + 3) as u8;
        }
        let wrapped = oaep_wrap(&pub_key, &sym).expect("wrap");
        assert_eq!(wrapped.len(), PCLSYNC_RSA_BYTES);
        let unwrapped = oaep_unwrap(&priv_key, &wrapped).expect("unwrap");
        assert_eq!(unwrapped.sym_type, sym.sym_type);
        assert_eq!(unwrapped.flags, sym.flags);
        assert_eq!(unwrapped.aes_key, sym.aes_key);
        assert_eq!(unwrapped.hmac_key, sym.hmac_key);
        // And constant-time eq also agrees.
        assert!(bool::from(unwrapped.ct_eq(&sym)));
    }

    #[test]
    fn oaep_unwrap_rejects_tampered() {
        let priv_key = fixture_priv();
        let pub_key = RsaPublicKey::from(&priv_key);
        let sym = SymKeyVer1::new(0);
        let mut wrapped = oaep_wrap(&pub_key, &sym).expect("wrap");
        // Flip one byte in the ciphertext.
        wrapped[10] ^= 0x01;
        let res = oaep_unwrap(&priv_key, &wrapped);
        assert!(matches!(res, Err(PclsyncRsaError::Oaep)));
    }

    #[test]
    fn oaep_unwrap_rejects_wrong_length() {
        let priv_key = fixture_priv();
        let short = vec![0u8; 511];
        assert!(matches!(
            oaep_unwrap(&priv_key, &short),
            Err(PclsyncRsaError::WrongCiphertextLen { got: 511, expected: 512 })
        ));
    }

    #[test]
    fn priv_key_der_roundtrip() {
        let priv_key = fixture_priv();
        let re_encoded = serialize_priv_key_der(&priv_key).expect("re-encode");
        let reparsed = parse_priv_key_der(&re_encoded).expect("re-parse");
        let re_re_encoded = serialize_priv_key_der(&reparsed).expect("re-encode 2");
        // PKCS#1 DER is canonical for a given key, so two successive
        // encode/decode rounds must be byte-identical.
        assert_eq!(re_encoded, re_re_encoded);
        // And the re-encoded bytes match the fixture byte-for-byte (fixture
        // was generated with OpenSSL which also emits canonical PKCS#1).
        assert_eq!(&re_encoded[..], PRIV_KEY_DER);
    }

    #[test]
    fn pub_key_der_roundtrip() {
        let priv_key = fixture_priv();
        let pub_key = RsaPublicKey::from(&priv_key);
        let der = serialize_pub_key_der(&pub_key).expect("encode pub");
        let reparsed = parse_pub_key_der(&der).expect("parse pub");
        let der2 = serialize_pub_key_der(&reparsed).expect("re-encode pub");
        assert_eq!(der, der2);
    }

    #[test]
    fn sym_key_debug_is_redacted() {
        let mut sym = SymKeyVer1::new(0);
        sym.aes_key[0] = 0xAB;
        sym.hmac_key[0] = 0xCD;
        let s = format!("{:?}", sym);
        assert!(s.contains("redacted"));
        assert!(!s.contains("AB"));
        assert!(!s.contains("CD"));
    }
}
