//! Metadata (filename) encryption.
//!
//! Filenames are deterministically encoded as
//! `HMAC-SHA256(master_key, "pcloud-crypto/filename/v1" || plaintext)`.
//! This mirrors the property of the C client that encrypted names are
//! deterministic per account so that lookup works, while ensuring the raw
//! plaintext name is never exposed on the wire or on disk.
//!
//! For display the HMAC is hex-encoded.

// **PLATFORM:** all
// **GATING:** none (portable).

use hmac::{Hmac, Mac};
use pcloud_secret::ExposeSecret;
use pcloud_secret::secret_bytes::SecretBytes;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

/// Runtime configuration for deterministic encrypted-filename handling.
///
/// Non-secret; serialized alongside the profile. Whether filenames are
/// encrypted is a per-profile policy decision, but the HMAC key used for
/// the encoding is the master key in [`SecretBytes`] (never persisted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataCrypto {
    /// When `true` (the default), file/folder names are deterministically
    /// encoded via HMAC-SHA256 before being sent to the server.
    pub encrypted_names_enabled: bool,
}

impl Default for MetadataCrypto {
    fn default() -> Self {
        Self {
            encrypted_names_enabled: true,
        }
    }
}

/// Errors from filename encode/decode operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MetadataCryptoError {
    /// The crypto subsystem is locked: the master key is not available, so
    /// no deterministic encoding can be performed.
    #[error("crypto is locked")]
    Locked,
    /// The provided filename is empty or contains a path separator (`/`).
    #[error("invalid filename")]
    InvalidName,
}

const FILENAME_LABEL: &[u8] = b"pcloud-crypto/filename/v1";

/// Encode a plaintext filename to its deterministic encrypted form.
///
/// Primitive: `HMAC-SHA256(master_key, "pcloud-crypto/filename/v1" || name)`.
/// Output: 32 bytes, lowercase-hex encoded (64 ASCII chars). Determinism
/// is required so the server can look up an encrypted folder by encoded
/// name without holding the key.
///
/// # Security
/// Mitigates: plaintext-filename exposure on the wire and at rest on the
/// server (the server only ever sees the hex HMAC tag); filename tampering
/// across accounts (the master key is per-account, so tags do not
/// collide). The fixed label `pcloud-crypto/filename/v1` domain-separates
/// this PRF output from sector keys ([`crate::content::derive_file_key`])
/// and from the setup fingerprint.
/// The master key is borrowed via `SecretBytes` (zeroize on drop,
/// no-`Clone` discipline — explicit `clone_secret()` only) and never
/// escapes the HMAC engine.
///
/// Out of scope: determinism itself leaks repeated filenames — if the
/// same plaintext name is used across folders the same hex tag appears.
/// This is an intentional trade-off with server-side lookup. Filename
/// *length* is fully hidden (output is fixed 64 chars).
///
/// # Test vectors
/// See `tests::encrypt_is_deterministic`, `different_names_differ`,
/// `different_keys_differ`, `rejects_empty_and_path`.
///
/// # Errors
/// Returns [`MetadataCryptoError::InvalidName`] for empty or path-embedded
/// names (`/` is forbidden).
///
/// # Panics
/// `expect()` on `Hmac::new_from_slice` — infallible for any non-empty
/// key length; the caller's `master` always carries a 32-byte Argon2id
/// output.
pub fn encrypt_filename(
    master: &SecretBytes,
    plaintext: &str,
) -> Result<String, MetadataCryptoError> {
    if plaintext.is_empty() || plaintext.contains('/') {
        return Err(MetadataCryptoError::InvalidName);
    }
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(master.expose_secret())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(FILENAME_LABEL);
    mac.update(plaintext.as_bytes());
    let tag = mac.finalize().into_bytes();
    let mut out = String::with_capacity(tag.len() * 2);
    for byte in tag.iter() {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn master() -> SecretBytes {
        SecretBytes::new(vec![0x42u8; 32])
    }

    #[test]
    fn encrypt_is_deterministic() {
        let m = master();
        let a = encrypt_filename(&m, "report.pdf").unwrap();
        let b = encrypt_filename(&m, "report.pdf").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn different_names_differ() {
        let m = master();
        let a = encrypt_filename(&m, "a.txt").unwrap();
        let b = encrypt_filename(&m, "b.txt").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn different_keys_differ() {
        let k1 = SecretBytes::new(vec![1u8; 32]);
        let k2 = SecretBytes::new(vec![2u8; 32]);
        let a = encrypt_filename(&k1, "same.txt").unwrap();
        let b = encrypt_filename(&k2, "same.txt").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn rejects_empty_and_path() {
        let m = master();
        assert!(encrypt_filename(&m, "").is_err());
        assert!(encrypt_filename(&m, "a/b").is_err());
    }
}
