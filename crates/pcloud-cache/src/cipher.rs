//! T2.3.a — Encryption-at-rest for the local cache.
//!
//! # Threat model
//!
//! The defender is the daemon process holding a master key
//! (typically the unwrapped auth-vault key); the attacker reads
//! the on-disk cache directly (forensic image, lost laptop,
//! group-readable temp dir slip). The goal: an attacker without
//! the master key cannot recover any cache page's plaintext.
//!
//! # Construction
//!
//! - **Master → page key**: HKDF-SHA256 (`extract` then `expand`)
//!   with the cache-cipher domain string as `info`. Different
//!   cache layers (page cache vs staging) derive distinct keys
//!   from the same master so a key compromise of one layer does
//!   not unlock the other.
//! - **Per-page seal**: AES-256-GCM with a 12-byte random nonce
//!   produced by `getrandom`. The nonce is prepended to the
//!   ciphertext so the on-disk record is `nonce || ciphertext ||
//!   tag` — self-contained for `open`. Random-nonce GCM is safe
//!   up to ~2^32 pages per key; the cache holds at most 10^6
//!   pages, so the birthday-bound collision probability stays
//!   below 2^-40.
//! - **Plaintext zeroisation**: callers receive plaintext as
//!   `Vec<u8>`. We do not zeroise the returned vec (the cache
//!   layer is *meant* to keep the plaintext live for the page-
//!   cache LRU); that responsibility lives in the layer above.
//!
//! # Why not Argon2 for the cache key
//!
//! Argon2 is for KDF-from-passphrase work; HKDF is the right
//! primitive for KDF-from-already-cryptographically-strong-key.
//! The master here is a 32-byte uniformly random key from the
//! auth vault, not a user-typed passphrase.

// **PLATFORM:** all
// **GATING:** none.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Length of the master key used to derive the cache cipher key.
/// The auth-vault layer hands the daemon a 32-byte master.
pub const MASTER_KEY_LEN: usize = 32;
/// Length of the derived AES-256-GCM key.
pub const CACHE_KEY_LEN: usize = 32;
/// AES-GCM nonce length (12 bytes per RFC 5116).
pub const NONCE_LEN: usize = 12;
/// AES-GCM authentication tag length (16 bytes).
pub const TAG_LEN: usize = 16;
/// Domain-separation label used as HKDF `info` for the page-cache
/// layer. Different cache layers (page vs staging) MUST pass
/// different labels to keep their keys disjoint.
pub const PAGE_CACHE_DOMAIN: &[u8] = b"pcloud-cache::page-cache::v1";
/// Domain-separation label for the staging layer.
pub const STAGING_DOMAIN: &[u8] = b"pcloud-cache::staging::v1";

type HmacSha256 = Hmac<Sha256>;

/// Errors raised by the cipher.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CipherError {
    /// Master key length was not [`MASTER_KEY_LEN`]. The vault
    /// hands a 32-byte master; anything else is a programmer
    /// error.
    #[error("master key must be {MASTER_KEY_LEN} bytes (got {got})")]
    BadMasterKeyLen {
        /// Length of the offending input.
        got: usize,
    },
    /// Nonce generation failed (e.g. `getrandom` unavailable).
    #[error("nonce generation failed: {0}")]
    NonceGen(String),
    /// Sealed record was shorter than the nonce-plus-tag overhead.
    #[error("sealed record too short ({got} < {min})")]
    Truncated {
        /// Bytes actually present.
        got: usize,
        /// Minimum required (`NONCE_LEN + TAG_LEN`).
        min: usize,
    },
    /// AEAD authentication failed — wrong key, corrupted ciphertext,
    /// or tampered tag. Per RFC 5116 the plaintext is unrecoverable.
    #[error("AEAD authentication failed (corrupt or tampered)")]
    AuthFailed,
}

/// AES-256-GCM cipher derived from a master key + domain string.
#[derive(Clone)]
pub struct CacheCipher {
    key: [u8; CACHE_KEY_LEN],
}

impl std::fmt::Debug for CacheCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the key.
        f.debug_struct("CacheCipher").finish_non_exhaustive()
    }
}

impl CacheCipher {
    /// Derive a per-domain cipher from a 32-byte master key.
    ///
    /// `domain` is the HKDF `info` parameter — pass
    /// [`PAGE_CACHE_DOMAIN`] for the page cache or
    /// [`STAGING_DOMAIN`] for the staging layer.
    ///
    /// # Errors
    ///
    /// Returns [`CipherError::BadMasterKeyLen`] when `master.len()
    /// != MASTER_KEY_LEN`.
    pub fn derive(master: &[u8], domain: &[u8]) -> Result<Self, CipherError> {
        if master.len() != MASTER_KEY_LEN {
            return Err(CipherError::BadMasterKeyLen { got: master.len() });
        }
        let key = hkdf_sha256(master, &[], domain, CACHE_KEY_LEN);
        let mut out = [0u8; CACHE_KEY_LEN];
        out.copy_from_slice(&key);
        Ok(Self { key: out })
    }

    /// Seal `plaintext` with a fresh random nonce. Returns the
    /// self-contained on-disk record `nonce || ciphertext || tag`.
    /// `aad` is bound into the AEAD authentication so a record
    /// produced for one page cannot be silently swapped onto
    /// another (typically the AAD is the page id big-endian).
    ///
    /// # Errors
    ///
    /// [`CipherError::NonceGen`] if `getrandom` is unavailable.
    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, CipherError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::getrandom(&mut nonce_bytes)
            .map_err(|err| CipherError::NonceGen(err.to_string()))?;
        let cipher = Aes256Gcm::new(&self.key.into());
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| CipherError::AuthFailed)?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Open a sealed record produced by [`Self::seal`]. `aad` MUST
    /// match the value passed at seal time.
    ///
    /// # Errors
    ///
    /// - [`CipherError::Truncated`] when the input is too short to
    ///   carry a nonce + tag.
    /// - [`CipherError::AuthFailed`] on any authentication failure
    ///   (wrong key, tampered tag, mismatched AAD, byte corruption).
    pub fn open(&self, sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>, CipherError> {
        let min = NONCE_LEN + TAG_LEN;
        if sealed.len() < min {
            return Err(CipherError::Truncated {
                got: sealed.len(),
                min,
            });
        }
        let (nonce_bytes, ct) = sealed.split_at(NONCE_LEN);
        let cipher = Aes256Gcm::new(&self.key.into());
        let nonce = Nonce::from_slice(nonce_bytes);
        cipher
            .decrypt(nonce, Payload { msg: ct, aad })
            .map_err(|_| CipherError::AuthFailed)
    }

    /// Length overhead `seal` adds to its input. Useful for callers
    /// that need to size on-disk pages. Always
    /// `NONCE_LEN + TAG_LEN = 28`.
    #[must_use]
    pub fn overhead() -> usize {
        NONCE_LEN + TAG_LEN
    }
}

/// HKDF-SHA256 (RFC 5869). `salt = []` collapses to the
/// "extract-no-salt" mode which is the standard choice for keys
/// that are already uniformly random.
fn hkdf_sha256(ikm: &[u8], salt: &[u8], info: &[u8], out_len: usize) -> Vec<u8> {
    // Extract.
    let salt_bytes: Vec<u8> = if salt.is_empty() {
        vec![0u8; 32]
    } else {
        salt.to_vec()
    };
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(&salt_bytes).expect("HMAC accepts any length");
    mac.update(ikm);
    let prk = mac.finalize().into_bytes();

    // Expand.
    let mut out = Vec::with_capacity(out_len);
    let mut prev: Vec<u8> = Vec::new();
    let mut counter: u8 = 1;
    while out.len() < out_len {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&prk).expect("HMAC accepts any length");
        mac.update(&prev);
        mac.update(info);
        mac.update(&[counter]);
        let block = mac.finalize().into_bytes();
        prev = block.to_vec();
        out.extend_from_slice(&block);
        counter = counter.wrapping_add(1);
        if counter == 0 {
            // Should be unreachable for sane out_len; HKDF spec
            // caps total output at 255 * 32 = 8160 bytes.
            break;
        }
    }
    out.truncate(out_len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_master() -> [u8; MASTER_KEY_LEN] {
        let mut k = [0u8; MASTER_KEY_LEN];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i * 17 + 3) as u8;
        }
        k
    }

    #[test]
    fn derive_rejects_bad_master_length() {
        let err = CacheCipher::derive(&[0u8; 16], PAGE_CACHE_DOMAIN).unwrap_err();
        match err {
            CipherError::BadMasterKeyLen { got } => assert_eq!(got, 16),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn derive_is_deterministic() {
        let m = fixed_master();
        let a = CacheCipher::derive(&m, PAGE_CACHE_DOMAIN).unwrap();
        let b = CacheCipher::derive(&m, PAGE_CACHE_DOMAIN).unwrap();
        assert_eq!(a.key, b.key);
    }

    #[test]
    fn different_domains_produce_different_keys() {
        let m = fixed_master();
        let a = CacheCipher::derive(&m, PAGE_CACHE_DOMAIN).unwrap();
        let b = CacheCipher::derive(&m, STAGING_DOMAIN).unwrap();
        assert_ne!(a.key, b.key);
    }

    #[test]
    fn seal_open_round_trip() {
        let cipher = CacheCipher::derive(&fixed_master(), PAGE_CACHE_DOMAIN).unwrap();
        let plaintext = b"sensitive cache page contents";
        let aad = 42u64.to_be_bytes();
        let sealed = cipher.seal(plaintext, &aad).unwrap();
        let opened = cipher.open(&sealed, &aad).unwrap();
        assert_eq!(opened, plaintext);
    }

    #[test]
    fn seal_output_is_not_plaintext_on_disk() {
        // Acceptance pivot for T2.3: an attacker reading the
        // on-disk bytes must not recover the plaintext.
        let cipher = CacheCipher::derive(&fixed_master(), PAGE_CACHE_DOMAIN).unwrap();
        let plaintext = b"the quick brown fox jumps over the lazy dog";
        let aad = 7u64.to_be_bytes();
        let sealed = cipher.seal(plaintext, &aad).unwrap();
        // Plaintext bytes do not appear in the sealed record.
        assert!(
            !sealed.windows(plaintext.len()).any(|w| w == plaintext),
            "plaintext leaked into sealed record"
        );
        // Sealed length is plaintext + nonce + tag overhead.
        assert_eq!(sealed.len(), plaintext.len() + CacheCipher::overhead());
    }

    #[test]
    fn nonce_is_fresh_per_seal() {
        let cipher = CacheCipher::derive(&fixed_master(), PAGE_CACHE_DOMAIN).unwrap();
        let aad = 0u64.to_be_bytes();
        let a = cipher.seal(b"same plaintext", &aad).unwrap();
        let b = cipher.seal(b"same plaintext", &aad).unwrap();
        // Different sealed records (different nonces) for the same
        // plaintext — random-nonce GCM property.
        assert_ne!(a, b);
    }

    #[test]
    fn open_with_wrong_aad_fails() {
        let cipher = CacheCipher::derive(&fixed_master(), PAGE_CACHE_DOMAIN).unwrap();
        let sealed = cipher.seal(b"contents", &[1u8]).unwrap();
        let err = cipher.open(&sealed, &[2u8]).unwrap_err();
        assert_eq!(err, CipherError::AuthFailed);
    }

    #[test]
    fn open_with_wrong_key_fails() {
        let a = CacheCipher::derive(&fixed_master(), PAGE_CACHE_DOMAIN).unwrap();
        let mut other_master = fixed_master();
        other_master[0] ^= 0xFF;
        let b = CacheCipher::derive(&other_master, PAGE_CACHE_DOMAIN).unwrap();
        let sealed = a.seal(b"contents", &[]).unwrap();
        let err = b.open(&sealed, &[]).unwrap_err();
        assert_eq!(err, CipherError::AuthFailed);
    }

    #[test]
    fn open_rejects_tampered_ciphertext() {
        let cipher = CacheCipher::derive(&fixed_master(), PAGE_CACHE_DOMAIN).unwrap();
        let mut sealed = cipher.seal(b"contents", &[]).unwrap();
        // Flip a bit in the ciphertext.
        let mid = sealed.len() / 2;
        sealed[mid] ^= 0x01;
        let err = cipher.open(&sealed, &[]).unwrap_err();
        assert_eq!(err, CipherError::AuthFailed);
    }

    #[test]
    fn open_rejects_truncated_record() {
        let cipher = CacheCipher::derive(&fixed_master(), PAGE_CACHE_DOMAIN).unwrap();
        let err = cipher.open(&[0u8; 5], &[]).unwrap_err();
        match err {
            CipherError::Truncated { got, min } => {
                assert_eq!(got, 5);
                assert_eq!(min, NONCE_LEN + TAG_LEN);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn debug_does_not_leak_key() {
        let cipher = CacheCipher::derive(&fixed_master(), PAGE_CACHE_DOMAIN).unwrap();
        let dbg = format!("{cipher:?}");
        assert!(dbg.contains("CacheCipher"));
        // Key must not appear (no hex / no decimal byte sequence).
        assert!(!dbg.contains("key"));
    }

    #[test]
    fn hkdf_sha256_matches_rfc_5869_test_vector_1() {
        // RFC 5869 §A.1 test vector 1.
        let ikm = hex_decode("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
        let salt = hex_decode("000102030405060708090a0b0c");
        let info = hex_decode("f0f1f2f3f4f5f6f7f8f9");
        let l = 42;
        let okm = hkdf_sha256(&ikm, &salt, &info, l);
        let expected = hex_decode(
            "3cb25f25faacd57a90434f64d0362f2a\
             2d2d0a90cf1a5a4c5db02d56ecc4c5bf\
             34007208d5b887185865",
        );
        assert_eq!(okm, expected);
    }

    fn hex_decode(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        let mut out = Vec::with_capacity(s.len() / 2);
        let bytes = s.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            let hi = (bytes[i] as char).to_digit(16).unwrap() as u8;
            let lo = (bytes[i + 1] as char).to_digit(16).unwrap() as u8;
            out.push((hi << 4) | lo);
            i += 2;
        }
        out
    }
}
