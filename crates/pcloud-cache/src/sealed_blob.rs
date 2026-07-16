//! T2.3.b — disk-shaped wrapper around [`CacheCipher`].
//!
//! # Why a separate helper
//!
//! `CacheCipher::seal` / `CacheCipher::open` accept any AAD and any
//! plaintext. Callers that write whole blobs to disk (the fs staging
//! layer, the page-cache write-through) all want the same calling
//! convention: `(blob_name, plaintext)` in, `Vec<u8>` ready for
//! `write_all` out (or symmetrically: read the on-disk bytes,
//! decrypt with `(blob_name, sealed_bytes)`). Pulling that into a
//! pair of functions saves every caller from re-deriving the AAD
//! convention and keeps the on-disk record format consistent across
//! the workspace.
//!
//! # Format
//!
//! The sealed bytes use the same wire shape as
//! [`CacheCipher::seal`]: `nonce(12) || ciphertext || tag(16)`. The
//! AAD is the blob name UTF-8 bytes. Two consequences:
//!
//! - An attacker cannot rename a sealed file (e.g. swap
//!   `cat.jpg.sealed` to `secret.txt.sealed`) and still get a
//!   successful decrypt — the AAD mismatch fails the AEAD.
//! - The blob name itself is not encrypted (it's the filename on
//!   disk). Callers who consider the blob name itself sensitive
//!   should hash it before storing.

// **PLATFORM:** all
// **GATING:** none.

use crate::cipher::{CacheCipher, CipherError};

/// Seal `plaintext` for on-disk storage under `blob_name`.
///
/// # Errors
///
/// See [`CipherError`].
pub fn seal_blob_for_disk(
    cipher: &CacheCipher,
    blob_name: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, CipherError> {
    cipher.seal(plaintext, blob_name.as_bytes())
}

/// Open an on-disk record produced by [`seal_blob_for_disk`].
///
/// `blob_name` MUST match the value passed at seal time — otherwise
/// the AEAD fails authentication and returns [`CipherError::AuthFailed`].
///
/// # Errors
///
/// See [`CipherError`].
pub fn open_blob_from_disk(
    cipher: &CacheCipher,
    blob_name: &str,
    sealed: &[u8],
) -> Result<Vec<u8>, CipherError> {
    cipher.open(sealed, blob_name.as_bytes())
}

/// Bytes the wrapper adds to the input. Callers can pre-size on-
/// disk buffers as `plaintext.len() + sealed_blob_overhead()`.
#[must_use]
pub fn sealed_blob_overhead() -> usize {
    CacheCipher::overhead()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cipher::{PAGE_CACHE_DOMAIN, STAGING_DOMAIN};

    fn fixed_master() -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(17).wrapping_add(3);
        }
        k
    }

    #[test]
    fn round_trip_preserves_plaintext() {
        let cipher = CacheCipher::derive(&fixed_master(), STAGING_DOMAIN).unwrap();
        let plaintext = b"a moderately sized blob of bytes";
        let sealed = seal_blob_for_disk(&cipher, "cat.jpg", plaintext).unwrap();
        let opened = open_blob_from_disk(&cipher, "cat.jpg", &sealed).unwrap();
        assert_eq!(opened, plaintext);
    }

    #[test]
    fn rename_attack_fails_aead_check() {
        // Seal under one blob name; open under another. AAD
        // mismatch must fail authentication so an attacker cannot
        // rename a sealed file and still get plaintext.
        let cipher = CacheCipher::derive(&fixed_master(), STAGING_DOMAIN).unwrap();
        let sealed = seal_blob_for_disk(&cipher, "cat.jpg", b"secret").unwrap();
        let err = open_blob_from_disk(&cipher, "decoy.jpg", &sealed).expect_err("rename must fail");
        assert_eq!(err, CipherError::AuthFailed);
    }

    #[test]
    fn cross_domain_decrypt_fails() {
        // Acceptance pivot — a key compromise of the page-cache
        // domain must not unlock staging blobs.
        let m = fixed_master();
        let staging = CacheCipher::derive(&m, STAGING_DOMAIN).unwrap();
        let page = CacheCipher::derive(&m, PAGE_CACHE_DOMAIN).unwrap();
        let sealed = seal_blob_for_disk(&staging, "x", b"contents").unwrap();
        let err = open_blob_from_disk(&page, "x", &sealed).unwrap_err();
        assert_eq!(err, CipherError::AuthFailed);
    }

    #[test]
    fn sealed_record_does_not_contain_plaintext() {
        // T2.3 acceptance: an attacker reading the on-disk bytes
        // must not recover the plaintext.
        let cipher = CacheCipher::derive(&fixed_master(), STAGING_DOMAIN).unwrap();
        let plaintext = b"this is a unique recognizable plaintext marker xyz";
        let sealed = seal_blob_for_disk(&cipher, "blob-1", plaintext).unwrap();
        assert!(
            !sealed.windows(plaintext.len()).any(|w| w == plaintext),
            "plaintext leaked into sealed record"
        );
    }

    #[test]
    fn sealed_blob_overhead_matches_cipher() {
        assert_eq!(sealed_blob_overhead(), CacheCipher::overhead());
        assert_eq!(sealed_blob_overhead(), 28);
    }

    #[test]
    fn empty_plaintext_round_trips() {
        let cipher = CacheCipher::derive(&fixed_master(), STAGING_DOMAIN).unwrap();
        let sealed = seal_blob_for_disk(&cipher, "empty", &[]).unwrap();
        // Empty plaintext still gets a nonce + tag.
        assert_eq!(sealed.len(), sealed_blob_overhead());
        let opened = open_blob_from_disk(&cipher, "empty", &sealed).unwrap();
        assert!(opened.is_empty());
    }

    #[test]
    fn corrupt_sealed_record_fails_open() {
        let cipher = CacheCipher::derive(&fixed_master(), STAGING_DOMAIN).unwrap();
        let mut sealed = seal_blob_for_disk(&cipher, "x", b"contents").unwrap();
        let mid = sealed.len() / 2;
        sealed[mid] ^= 0x01;
        assert_eq!(
            open_blob_from_disk(&cipher, "x", &sealed).unwrap_err(),
            CipherError::AuthFailed
        );
    }
}
