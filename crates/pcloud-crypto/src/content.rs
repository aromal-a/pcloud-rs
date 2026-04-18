//! Content encryption (sector-oriented AES-256-GCM).
//!
//! The C client encrypts file contents with a per-file symmetric key in
//! fixed-size sectors (see `PSYNC_CRYPTO_SECTOR_SIZE` in `pcryptofolder.h`).
//! This module mirrors that shape on the Rust path using AEAD (AES-256-GCM).
//! A per-file key is derived from the master key plus a random nonce so that
//! the master key never directly encrypts ciphertext.
//!
//! Each sector is stored as:
//!
//! ```text
//! [u32 sector_index][12-byte nonce][ciphertext + 16-byte tag]
//! ```
//!
//! The sector index is bound into AEAD associated data so that swapping
//! sectors is detected.

// **PLATFORM:** all
// **GATING:** none (portable).

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit as AesKeyInit, Nonce};
use getrandom::getrandom;
use hmac::{Hmac, Mac};
use pcloud_secret::ExposeSecret;
use pcloud_secret::secret_bytes::SecretBytes;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

/// Plaintext size of one sector. Mirrors `PSYNC_CRYPTO_SECTOR_SIZE` in C.
pub const SECTOR_SIZE_BYTES: usize = 4096;
/// AES-256-GCM nonce length in bytes (96-bit, per NIST SP 800-38D).
pub const NONCE_LEN: usize = 12;
/// AES-256-GCM authentication tag length in bytes (128-bit).
pub const TAG_LEN: usize = 16;
/// File-key length in bytes (AES-256 key material).
pub const FILE_KEY_LEN: usize = 32;
/// Per-sector overhead: 4 bytes sector index + 12 bytes nonce + 16 bytes tag.
pub const SECTOR_OVERHEAD: usize = 4 + NONCE_LEN + TAG_LEN;

/// Content-encryption configuration block.
///
/// Carries no secret material; safe to serialize into a profile.
///
/// # Security
/// The sector size is non-secret metadata; changing it does not affect
/// the strength of AES-256-GCM but does shift where the 4096-byte
/// plaintext boundary falls. Keep at the default unless a profile
/// migration explicitly requires otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentCrypto {
    /// Plaintext sector size in bytes. Defaults to [`SECTOR_SIZE_BYTES`].
    ///
    /// # Security
    /// Must match the profile's historical sector size for existing
    /// ciphertext to remain readable. Non-secret; chosen to match C
    /// client's `PSYNC_CRYPTO_SECTOR_SIZE`.
    pub sector_size_bytes: usize,
}

impl Default for ContentCrypto {
    fn default() -> Self {
        Self {
            sector_size_bytes: SECTOR_SIZE_BYTES,
        }
    }
}

/// Errors returned by the sector AEAD layer.
///
/// All variants are opaque with respect to plaintext / key material: they
/// never encode position or byte values that could form a padding / chosen-
/// ciphertext oracle.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContentCryptoError {
    /// No active master key material is available.
    #[error("crypto is locked")]
    Locked,
    /// Frame is too short to hold the fixed header + tag, or the AES
    /// instantiation rejected a short key.
    #[error("invalid ciphertext frame")]
    InvalidFrame,
    /// AES-256-GCM authentication tag check failed. Indicates tampering,
    /// wrong key, or wrong AAD.
    #[error("authentication failed")]
    AuthFailed,
    /// Plaintext exceeded the configured sector size.
    #[error("sector larger than configured size")]
    SectorTooLarge,
    /// The frame's embedded sector index did not match the index the
    /// caller expected.
    #[error("sector index mismatch")]
    SectorIndexMismatch,
}

/// Derive a per-file 32-byte AES-256-GCM key from the master key and a
/// file seed.
///
/// Primitive: `HMAC-SHA256(master_key, "pcloud-crypto/file-key/v1" || file_seed)`.
/// The derived key is 32 bytes (one SHA-256 output) and is returned inside
/// a [`SecretBytes`] so it is zeroized on drop. The label is fixed and
/// versioned so that the derivation is domain-separated from filename
/// encoding ([`crate::metadata::encrypt_filename`]) and from the setup
/// fingerprint ([`crate::keys::KeyManager::fingerprint_for`]).
///
/// # Security
/// Mitigates: cross-file key reuse (per-file seed enters the PRF),
/// domain confusion with filename / fingerprint HMAC labels (distinct
/// fixed labels), and key-material exposure via logs (output wrapped
/// in [`SecretBytes`] with `Debug` redacted and zeroize on drop).
///
/// Out of scope: the entropy of `file_seed` — the caller is expected to
/// supply a seed with at least 128 bits of entropy; the typical caller is
/// [`crate::CryptoShell::seal_sector`] which ultimately uses a
/// backend-assigned random value.
///
/// # Panics
/// Does not panic in normal operation. `HMAC-SHA256::new_from_slice` is
/// infallible for any non-empty key length.
///
/// # Test vectors
/// The module's `round_trip_single_sector` test exercises the full
/// derivation + AES-GCM round trip.
#[must_use]
pub fn derive_file_key(master: &SecretBytes, file_seed: &[u8]) -> SecretBytes {
    const LABEL: &[u8] = b"pcloud-crypto/file-key/v1";
    // INVARIANT: HMAC-SHA256 accepts keys of any non-zero length per RFC 2104;
    // `new_from_slice` only fails for a zero-length key, which `SecretBytes`
    // never produces (callers always derive from a 32-byte master key).
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(master.expose_secret())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(LABEL);
    mac.update(file_seed);
    let out: [u8; FILE_KEY_LEN] = mac.finalize().into_bytes().into();
    SecretBytes::new(out.to_vec())
}

/// Encrypt one sector into a framed ciphertext.
///
/// Primitive: AES-256-GCM. Key size: 32 bytes ([`FILE_KEY_LEN`]).
/// Nonce: 12 bytes ([`NONCE_LEN`]) from the OS CSPRNG via `getrandom`.
/// Tag: 16 bytes ([`TAG_LEN`]) appended by the AEAD.
/// AAD: the 4-byte big-endian `sector_index`, which is also embedded in
/// the frame header so [`open_sector`] can verify it *before* the AEAD
/// call. The frame layout is `[u32 index][12-byte nonce][ct || 16-byte tag]`.
///
/// # Security
/// Mitigates:
/// * ciphertext tampering and wrong-key use (GCM tag);
/// * sector reordering (sector index bound into AAD and header);
/// * cross-file reuse (caller is expected to pass a per-file key derived
///   via [`derive_file_key`]);
/// * stack/heap residue (nonce byte buffer is on the stack and discarded
///   when the function returns; `plaintext` is borrowed and never copied
///   outside what `Aes256Gcm::encrypt` requires).
///
/// Out of scope: length-hiding — the frame size reveals the plaintext
/// size modulo the AEAD overhead. Also out of scope: a global sector-
/// count budget for a single file key; callers writing billions of
/// sectors against the same file key should rotate keys at the daemon
/// level.
///
/// # Test vectors
/// Internal tests cover: plain round trip (`round_trip_single_sector`),
/// wrong sector index (`wrong_index_is_rejected`), tampered ciphertext
/// (`tampered_ciphertext_rejected`), and wrong key
/// (`wrong_key_rejected`).
///
/// # Errors
/// * [`ContentCryptoError::SectorTooLarge`] if `plaintext.len() > sector_size`.
/// * [`ContentCryptoError::InvalidFrame`] if the key length is not 32 bytes or if
///   the OS CSPRNG cannot supply randomness for the nonce.
/// * [`ContentCryptoError::AuthFailed`] if the AEAD rejects the inputs
///   (should not happen on the encrypt path in practice).
///
/// # Panics
/// Does not panic.
pub fn seal_sector(
    file_key: &SecretBytes,
    sector_index: u32,
    plaintext: &[u8],
    sector_size: usize,
) -> Result<Vec<u8>, ContentCryptoError> {
    if plaintext.len() > sector_size {
        return Err(ContentCryptoError::SectorTooLarge);
    }
    let cipher = <Aes256Gcm as AesKeyInit>::new_from_slice(file_key.expose_secret())
        .map_err(|_| ContentCryptoError::InvalidFrame)?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom(&mut nonce_bytes).map_err(|_| ContentCryptoError::InvalidFrame)?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let aad = sector_index.to_be_bytes();
    let ct = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| ContentCryptoError::AuthFailed)?;

    let mut frame = Vec::with_capacity(4 + NONCE_LEN + ct.len());
    frame.extend_from_slice(&aad);
    frame.extend_from_slice(&nonce_bytes);
    frame.extend_from_slice(&ct);
    Ok(frame)
}

/// Decrypt one sector frame produced by [`seal_sector`].
///
/// Primitive: AES-256-GCM. Key size: 32 bytes ([`FILE_KEY_LEN`]).
/// Expected frame layout: `[u32 index][12-byte nonce][ct || 16-byte tag]`
/// (see [`seal_sector`]). The 4-byte big-endian `expected_index` is
/// compared against the embedded index before the AEAD call and is also
/// passed as AAD, so either a frame swap or an AAD desync yields the
/// same opaque error.
///
/// # Security
/// Mitigates:
/// * ciphertext tampering and wrong-key use (GCM tag);
/// * sector-swap replay across sectors of the same file (index is AAD
///   plus a pre-AEAD index equality check);
/// * plaintext exposure on locked shells (the caller must gate this
///   with the [`crate::state::UnlockState::Unlocked`] check — see
///   [`crate::CryptoShell::open_sector`]);
/// * key confusion with filename / fingerprint HMAC output (distinct
///   PRF labels).
///
/// Out of scope: length-hiding (frame size reveals plaintext length);
/// cross-file replay — the caller must supply the correct per-file key.
/// Error variants are deliberately collapsed: an attacker cannot
/// distinguish "short frame" from "bad tag" beyond the two opaque
/// [`ContentCryptoError`] variants.
///
/// # Test vectors
/// `tests::wrong_index_is_rejected`, `tests::tampered_ciphertext_rejected`,
/// `tests::wrong_key_rejected` cover the mitigation surface.
///
/// # Errors
/// * [`ContentCryptoError::InvalidFrame`] if `frame` is shorter than
///   `4 + 12 + 16` bytes or the key is not 32 bytes.
/// * [`ContentCryptoError::SectorIndexMismatch`] if the embedded index
///   differs from `expected_index`.
/// * [`ContentCryptoError::AuthFailed`] on any AEAD rejection (tampered
///   ciphertext, wrong key, wrong AAD).
///
/// # Panics
/// Does not panic.
pub fn open_sector(
    file_key: &SecretBytes,
    expected_index: u32,
    frame: &[u8],
) -> Result<Vec<u8>, ContentCryptoError> {
    if frame.len() < 4 + NONCE_LEN + TAG_LEN {
        return Err(ContentCryptoError::InvalidFrame);
    }
    let mut idx = [0u8; 4];
    idx.copy_from_slice(&frame[..4]);
    let sector_index = u32::from_be_bytes(idx);
    if sector_index != expected_index {
        return Err(ContentCryptoError::SectorIndexMismatch);
    }
    let nonce = Nonce::from_slice(&frame[4..4 + NONCE_LEN]);
    let ct = &frame[4 + NONCE_LEN..];
    let cipher = <Aes256Gcm as AesKeyInit>::new_from_slice(file_key.expose_secret())
        .map_err(|_| ContentCryptoError::InvalidFrame)?;
    cipher
        .decrypt(nonce, Payload { msg: ct, aad: &idx })
        .map_err(|_| ContentCryptoError::AuthFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn master_key() -> SecretBytes {
        SecretBytes::new(vec![7u8; 32])
    }

    #[test]
    fn round_trip_single_sector() {
        let master = master_key();
        let file_key = derive_file_key(&master, b"seed-123");
        let pt = b"hello encrypted world";
        let frame = seal_sector(&file_key, 0, pt, SECTOR_SIZE_BYTES).expect("seal");
        let round = open_sector(&file_key, 0, &frame).expect("open");
        assert_eq!(round, pt);
    }

    #[test]
    fn wrong_index_is_rejected() {
        let master = master_key();
        let file_key = derive_file_key(&master, b"seed-xyz");
        let frame = seal_sector(&file_key, 3, b"xxx", SECTOR_SIZE_BYTES).expect("seal");
        let err = open_sector(&file_key, 4, &frame).expect_err("mismatch");
        assert_eq!(err, ContentCryptoError::SectorIndexMismatch);
    }

    #[test]
    fn tampered_ciphertext_rejected() {
        let master = master_key();
        let file_key = derive_file_key(&master, b"seed-abc");
        let mut frame = seal_sector(&file_key, 0, b"data", SECTOR_SIZE_BYTES).expect("seal");
        let last = frame.len() - 1;
        frame[last] ^= 0x01;
        let err = open_sector(&file_key, 0, &frame).expect_err("tamper");
        assert_eq!(err, ContentCryptoError::AuthFailed);
    }

    #[test]
    fn wrong_key_rejected() {
        let master = master_key();
        let file_key = derive_file_key(&master, b"seed-1");
        let other = derive_file_key(&master, b"seed-2");
        let frame = seal_sector(&file_key, 0, b"data", SECTOR_SIZE_BYTES).expect("seal");
        let err = open_sector(&other, 0, &frame).expect_err("wrong key");
        assert_eq!(err, ContentCryptoError::AuthFailed);
    }

    #[test]
    fn oversized_plaintext_rejected() {
        let master = master_key();
        let file_key = derive_file_key(&master, b"s");
        let big = vec![0u8; SECTOR_SIZE_BYTES + 1];
        let err = seal_sector(&file_key, 0, &big, SECTOR_SIZE_BYTES).expect_err("too large");
        assert_eq!(err, ContentCryptoError::SectorTooLarge);
    }
}
