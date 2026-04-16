#![allow(clippy::pedantic)]
//! Property tests for pcloud-crypto sector seal/unseal.
//!
//! `seal_sector` / `open_sector` encrypt fixed-size sectors with AES-256-GCM,
//! binding the sector index into the AEAD associated data.
//!
//! Tests:
//! - `unseal_inverts_seal`: for any plaintext fitting a sector and any 32-byte
//!   key, `open_sector(key, idx, seal_sector(key, idx, pt)) == pt`.
//! - `aad_mismatch_fails`: sealing at index A and attempting to open at
//!   index B (B != A) must fail with `SectorIndexMismatch` or `AuthFailed`.
//! - `wrong_key_fails`: opening a frame with a different key must fail.
//!
//! Case counts are capped to 128 to keep runtime well within CI budgets.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_crypto::content::{ContentCryptoError, SECTOR_SIZE_BYTES, open_sector, seal_sector};
use pcloud_secret::secret_bytes::SecretBytes;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// AEAD round-trip identity: decrypting what we encrypted, with the same
    /// key and sector index, recovers the original plaintext byte-for-byte.
    ///
    /// Plaintext is bounded by `SECTOR_SIZE_BYTES` (4096) — the configured
    /// sector size — since `seal_sector` rejects oversized inputs.
    #[test]
    fn unseal_inverts_seal(
        plaintext in prop::collection::vec(any::<u8>(), 0..=SECTOR_SIZE_BYTES),
        key in any::<[u8; 32]>(),
        sector_index in any::<u32>(),
    ) {
        let key_sb = SecretBytes::new(key.to_vec());
        let frame = seal_sector(&key_sb, sector_index, &plaintext, SECTOR_SIZE_BYTES)
            .expect("seal must succeed for in-range plaintext");
        let recovered = open_sector(&key_sb, sector_index, &frame)
            .expect("open must succeed with matching key and index");
        prop_assert_eq!(recovered, plaintext);
    }

    /// Sealing at index A then attempting to open at index B (B != A) must
    /// fail. The frame records its own index in bytes [0..4], so the parser
    /// may either reject with `SectorIndexMismatch` (before AEAD) or with
    /// `AuthFailed` (if the frame is tampered to claim a different index).
    #[test]
    fn aad_mismatch_fails(
        plaintext in prop::collection::vec(any::<u8>(), 0..=1024),
        key in any::<[u8; 32]>(),
        idx_a in any::<u32>(),
        idx_b in any::<u32>(),
    ) {
        prop_assume!(idx_a != idx_b);
        let key_sb = SecretBytes::new(key.to_vec());
        let frame = seal_sector(&key_sb, idx_a, &plaintext, SECTOR_SIZE_BYTES)
            .expect("seal must succeed");
        let err = open_sector(&key_sb, idx_b, &frame)
            .expect_err("open with mismatched index must fail");
        prop_assert!(matches!(
            err,
            ContentCryptoError::SectorIndexMismatch | ContentCryptoError::AuthFailed
        ));

        // Also verify that tampering the encoded index in the frame to
        // match idx_b forces AEAD to reject (the sector index is in AAD).
        let mut tampered = frame.clone();
        tampered[..4].copy_from_slice(&idx_b.to_be_bytes());
        let err2 = open_sector(&key_sb, idx_b, &tampered)
            .expect_err("tampered AAD must be rejected by AEAD");
        prop_assert_eq!(err2, ContentCryptoError::AuthFailed);
    }

    /// Opening with a key that differs from the sealing key must fail auth.
    #[test]
    fn wrong_key_fails(
        plaintext in prop::collection::vec(any::<u8>(), 1..=512),
        key_a in any::<[u8; 32]>(),
        key_b in any::<[u8; 32]>(),
        sector_index in any::<u32>(),
    ) {
        prop_assume!(key_a != key_b);
        let sb_a = SecretBytes::new(key_a.to_vec());
        let sb_b = SecretBytes::new(key_b.to_vec());
        let frame = seal_sector(&sb_a, sector_index, &plaintext, SECTOR_SIZE_BYTES)
            .expect("seal must succeed");
        let err = open_sector(&sb_b, sector_index, &frame)
            .expect_err("wrong key must fail");
        prop_assert_eq!(err, ContentCryptoError::AuthFailed);
    }
}
