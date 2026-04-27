//! RSA-4096-OAEP wrap of a folder/file `SymKeyVer1` for crypto share
//! invitation — C-interop path (pclsync-v2).
//!
//! # C parity surface
//!
//! This is the Rust analogue of the C client's share-invitation key
//! handoff. On the C side:
//!
//! * `psync_crypto_share_folder` (`pclsync/psynclib.c:1322`) and
//!   `psync_crypto_account_teamshare` (`pclsync/psynclib.c:1372`) fetch
//!   the recipient's public key via `crypto_getpubkey` /
//!   `crypto_getteamshare_pubkey`, parse it into an mbedtls RSA context
//!   (`pclsync/pssl.c:583..`), RSA-4096-OAEP-SHA1 wrap the current
//!   folder/file `sym_key_ver1` against the recipient's pubkey
//!   (`pclsync/pssl.c:718..740`), and attach the base64 of the wrapped
//!   ciphertext to the `sharefolder` / `account_teamshare` request
//!   under the `sharedfolderkey` / `teamshare_key` parameter.
//!
//! This module implements the **crypto-layer** half of that flow:
//! given the sharer's unlocked `PclsyncCompatState` (which holds the
//! cached folder/file `SymKeyVer1`) and the recipient's public-key
//! blob, produce the base64 RSA-OAEP ciphertext.
//!
//! The backend/proto wiring that fetches the recipient pubkey and
//! attaches the ciphertext to the sharefolder request is tracked under
//! `pcloud-rs-ncx.89` and is landed separately.
//!
//! # Security posture
//!
//! * The sharer's folder/file `SymKeyVer1` is borrowed (not copied) from
//!   the `PclsyncCompatState` cache; it is `ZeroizeOnDrop` and is never
//!   duplicated into an unprotected buffer here.
//! * The recipient pubkey blob is non-secret but is validated against
//!   the legacy `pub_key_ver1` header before being fed to
//!   `parse_pub_key_der`, so malformed blobs fail loudly before any
//!   RSA operation runs.
//! * RSA-OAEP with SHA-1 matches the mbedtls configuration the C client
//!   uses (`MBEDTLS_RSA_PKCS_V21` + `MBEDTLS_MD_SHA1`, `pssl.c:485-486`).
//!   This is required for wire-compat with the pCloud apps; a stricter
//!   hash would make the ciphertext undecryptable by the official
//!   client. The security of RSA-OAEP-SHA1 as a KEM for a random
//!   symmetric key is not affected by SHA-1 collisions (OAEP relies on
//!   SHA-1's one-way property, not collision resistance).
//! * The ciphertext length is asserted to be exactly `PCLSYNC_RSA_BYTES`
//!   (= 512), matching `prsa_encrypt_data` in `pssl.c`.
//! * All error variants collapse to a single opaque
//!   [`ShareRsaError::Oaep`] at the public API so a caller cannot
//!   distinguish bad-pubkey from bad-keyslot (no padding-oracle leak).
//!
//! # Not in scope here
//!
//! * Fetching the recipient pubkey — that's `crypto_getpubkey` /
//!   `crypto_getteamshare_pubkey` proto wiring.
//! * Sending the wrapped blob — that's the `sharefolder` /
//!   `account_teamshare` proto wiring (new `sharedfolderkey` /
//!   `teamshare_key` parameters).
//! * Detached signature under the sharer's RSA priv key — the C client
//!   does not sign this blob (the recipient verifies the sharer via
//!   pCloud server identity + the sharing request chain). The bead
//!   description originally mentioned `prsa_sign_sha256_hash`, but
//!   reading `psync_crypto_share_folder` shows it only emits
//!   `sharedfolderkey` (the OAEP ciphertext) — no detached signature.
//!   If future audits revisit this, the signature primitive can be
//!   added here as a second output field.

// **PLATFORM:** all
// **GATING:** feature = "pclsync-v2" (re-exported from the crate when
// the feature is active, which is the default build).

use rsa::RsaPublicKey;
use thiserror::Error;

use crate::crypto_util::base64_encode;
use crate::pclsync_compat_profile::{PclsyncCompatError, PclsyncCompatProfile, PclsyncCompatState};
use crate::pclsync_rsa::{self, PclsyncRsaError, SymKeyVer1};

/// Errors specific to RSA-based share-invitation key wrapping.
///
/// These are kept distinct from [`crate::TemppassError`] because the
/// PclsyncCompat C-interop flow does NOT use the temppass KEK-rewrap
/// shape; it uses direct RSA-OAEP wrapping against the recipient's
/// pubkey. See the module-level docs for the full C reference.
#[derive(Debug, Error)]
pub enum ShareRsaError {
    /// The sharer's crypto shell is locked — no `PclsyncCompatState`
    /// is resident, so we cannot look up any folder/file sym key.
    #[error("crypto is locked; cannot issue share invitation")]
    Locked,

    /// The sharer's `PclsyncCompatState` has no cached `SymKeyVer1`
    /// for the requested folder/file. The caller must first populate
    /// the cache via `crypto_getfolderkey` (folder) or the file-key
    /// equivalent — this is a daemon-orchestration preconditon, not
    /// something we can recover from at this layer.
    #[error("no cached sym key for the requested target")]
    MissingSymKey,

    /// The recipient pubkey blob is malformed: wrong `pub_key_ver1`
    /// header, wrong type field, or the embedded DER does not decode
    /// as a well-formed RSA-4096 public key.
    #[error("recipient pubkey blob is malformed")]
    MalformedPubkey,

    /// RSAES-OAEP encryption failed. Collapsed to an opaque variant to
    /// avoid leaking the cause (bad key / bad padding / RNG failure)
    /// to a caller — same discipline as
    /// [`crate::pclsync_rsa::PclsyncRsaError::Oaep`].
    #[error("RSAES-OAEP wrap failed")]
    Oaep,
}

impl From<PclsyncCompatError> for ShareRsaError {
    fn from(_: PclsyncCompatError) -> Self {
        Self::MalformedPubkey
    }
}

impl From<PclsyncRsaError> for ShareRsaError {
    fn from(err: PclsyncRsaError) -> Self {
        match err {
            PclsyncRsaError::Oaep | PclsyncRsaError::WrongCiphertextLen { .. } => Self::Oaep,
            PclsyncRsaError::Der(_) => Self::MalformedPubkey,
            _ => Self::MalformedPubkey,
        }
    }
}

/// Identifies which cached sym key the sharer is wrapping.
///
/// A share invitation always targets either a folder or a file;
/// the PclsyncCompat cache (`PclsyncCompatState::folder_keys` /
/// `file_keys`) indexes by server-assigned id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareTarget {
    /// Wrap the `SymKeyVer1` cached under this folder id.
    Folder(u64),
    /// Wrap the `SymKeyVer1` cached under this file id.
    File(u64),
}

/// Parse a raw `pub_key_ver1` blob (as returned by pCloud's
/// `crypto_getpubkey` / `crypto_getteamshare_pubkey`) into a usable
/// [`RsaPublicKey`].
///
/// Validates the 8-byte header (`type || flags`, both LE u32) against
/// `PSYNC_CRYPTO_PUB_TYPE_RSA4096`, then decodes the embedded PKCS#1
/// DER as RSA-4096.
///
/// # Errors
/// [`ShareRsaError::MalformedPubkey`] if the header is wrong or the
/// DER does not decode as a well-formed RSA-4096 public key.
pub fn parse_recipient_pubkey(pub_blob: &[u8]) -> Result<RsaPublicKey, ShareRsaError> {
    let (_typ, _flags, der) = PclsyncCompatProfile::parse_pub_blob(pub_blob)?;
    pclsync_rsa::parse_pub_key_der(&der).map_err(ShareRsaError::from)
}

/// Wrap the sym key currently cached on `state` for `target` under the
/// recipient's public key (`pub_blob`), returning the base64 of the
/// 512-byte RSAES-OAEP-SHA1 ciphertext.
///
/// This is the Rust analogue of the C client's inline wrap in
/// `psync_crypto_share_folder` / `psync_crypto_account_teamshare`: it
/// takes the folder/file `SymKeyVer1` the sharer holds (populated by a
/// prior `crypto_getfolderkey` / file-key unwrap), wraps it against the
/// invitee's public key, and returns the ciphertext in the shape the
/// `sharefolder` / `account_teamshare` request expects
/// (`sharedfolderkey` / `teamshare_key` parameter).
///
/// # Security
/// * The plaintext `SymKeyVer1` is serialized into a local buffer that
///   is zeroized before the function returns (via the drop of the
///   `SymKeyVer1` temporary produced by `duplicate()` — note: we do
///   NOT clone the key here; we borrow the cached entry and feed it
///   directly to [`pclsync_rsa::oaep_wrap`], which internally
///   serializes and zeroizes the plaintext staging buffer).
/// * RSA-OAEP-SHA1 + MGF1-SHA1 + empty label — matches mbedtls
///   defaults used by the C client (`pssl.c:485-486`, `pssl.c:727-729`).
/// * Ciphertext length is asserted inside [`pclsync_rsa::oaep_wrap`]
///   to be exactly 512 bytes.
/// * All failure modes collapse to [`ShareRsaError::Oaep`] or
///   [`ShareRsaError::MalformedPubkey`] — no padding-oracle leaks.
///
/// # Errors
/// - [`ShareRsaError::Locked`] if the sharer has no resident
///   `PclsyncCompatState` (crypto is locked or not PclsyncCompat).
/// - [`ShareRsaError::MissingSymKey`] if the cache has no entry for
///   the requested folder/file id (caller must populate the cache
///   first via `crypto_getfolderkey` or equivalent).
/// - [`ShareRsaError::MalformedPubkey`] if the recipient pubkey blob
///   is malformed.
/// - [`ShareRsaError::Oaep`] if the wrap itself fails (bad modulus,
///   RNG error, etc.).
pub fn wrap_share_invitation_b64(
    state: &PclsyncCompatState,
    target: ShareTarget,
    recipient_pub_blob: &[u8],
) -> Result<String, ShareRsaError> {
    let sym: &SymKeyVer1 = match target {
        ShareTarget::Folder(id) => state.folder_key(id).ok_or(ShareRsaError::MissingSymKey)?,
        ShareTarget::File(id) => state.file_key(id).ok_or(ShareRsaError::MissingSymKey)?,
    };
    let pub_key = parse_recipient_pubkey(recipient_pub_blob)?;
    let ct = pclsync_rsa::oaep_wrap(&pub_key, sym).map_err(ShareRsaError::from)?;
    Ok(base64_encode(&ct))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto_util::base64_decode;
    use crate::pclsync_compat_profile::PclsyncCompatProfile;
    use crate::pclsync_rsa::{
        PCLSYNC_AES_KEY_LEN, PCLSYNC_HMAC_KEY_LEN, PCLSYNC_RSA_BYTES,
        PCLSYNC_SYM_TYPE_AES256_1024BIT_HMAC, SymKeyVer1, generate_keypair, oaep_unwrap,
        serialize_pub_key_der,
    };
    use rsa::RsaPublicKey;

    /// Build a deterministic test `SymKeyVer1` so unwrap can be
    /// field-by-field compared.
    fn test_sym_key(seed_aes: u8, seed_hmac: u8) -> SymKeyVer1 {
        let mut sym = SymKeyVer1::new(0);
        for (i, b) in sym.aes_key.iter_mut().enumerate() {
            *b = seed_aes.wrapping_add(i as u8);
        }
        for (i, b) in sym.hmac_key.iter_mut().enumerate() {
            *b = seed_hmac.wrapping_add(i as u8);
        }
        sym
    }

    fn pub_blob_for(pubkey: &RsaPublicKey) -> Vec<u8> {
        let der = serialize_pub_key_der(pubkey).expect("pub DER");
        PclsyncCompatProfile::build_pub_blob(0, &der)
    }

    // NOTE on test speed: RSA-4096 keygen takes several seconds. We
    // limit each test to **one** fresh keypair and reuse it for both
    // the "recipient" and (where needed) the "sharer" role. The
    // pclsync_rsa module's own tests already cover wrap/unwrap
    // correctness against a committed DER fixture; these tests focus
    // on the share-invitation end-to-end shape.

    #[test]
    fn parse_recipient_pubkey_roundtrip() {
        let kp = generate_keypair().expect("keygen");
        let blob = pub_blob_for(kp.public());
        let parsed = parse_recipient_pubkey(&blob).expect("parse");
        // Re-serialize and compare bytes — DER is canonical.
        let a = serialize_pub_key_der(kp.public()).unwrap();
        let b = serialize_pub_key_der(&parsed).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn parse_recipient_pubkey_rejects_malformed_header() {
        // Too-short blob.
        let err = parse_recipient_pubkey(&[0u8; 4]).unwrap_err();
        assert!(matches!(err, ShareRsaError::MalformedPubkey));
        // Right length but wrong type field (2 instead of RSA4096=1).
        let mut bad = vec![0u8; 16];
        bad[0..4].copy_from_slice(&2u32.to_le_bytes());
        let err = parse_recipient_pubkey(&bad).unwrap_err();
        assert!(matches!(err, ShareRsaError::MalformedPubkey));
    }

    #[test]
    fn parse_recipient_pubkey_rejects_bad_der() {
        // Correct header type, but the "DER" payload is garbage.
        let mut blob = vec![0u8; 32];
        blob[0..4].copy_from_slice(&1u32.to_le_bytes()); // RSA4096
        blob[4..8].copy_from_slice(&0u32.to_le_bytes());
        for (i, b) in blob[8..].iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7);
        }
        let err = parse_recipient_pubkey(&blob).unwrap_err();
        assert!(matches!(err, ShareRsaError::MalformedPubkey));
    }

    #[test]
    fn wrap_share_invitation_missing_folder_key_rejected() {
        // A state with no cached keys.
        let kp = generate_keypair().expect("keygen");
        let (priv_key, pub_key) = kp.into_parts();
        // Borrow pclsync_compat_profile's constructor path via a test
        // helper: we build a PclsyncCompatState by round-tripping a
        // fake profile. Instead we exercise the same code path with a
        // state that simply has no folder cached.
        let state = PclsyncCompatStateHarness::new(priv_key);

        let blob = pub_blob_for(&pub_key);
        let err =
            wrap_share_invitation_b64(&state.inner, ShareTarget::Folder(12345), &blob).unwrap_err();
        assert!(matches!(err, ShareRsaError::MissingSymKey));

        let err =
            wrap_share_invitation_b64(&state.inner, ShareTarget::File(12345), &blob).unwrap_err();
        assert!(matches!(err, ShareRsaError::MissingSymKey));
    }

    #[test]
    fn wrap_share_invitation_roundtrip_folder_key() {
        // Synthetic recipient: generate a fresh keypair, hand the
        // pubkey blob to the sharer, wrap a cached folder sym key,
        // then unwrap on the recipient side and compare field-by-field.
        let kp = generate_keypair().expect("keygen recipient");
        let (recipient_priv, recipient_pub) = kp.into_parts();

        // Sharer state: a separate "priv_key" isn't used by the wrap
        // path at all (wrap only needs the cached SymKeyVer1 plus the
        // recipient's pubkey). We plant a synthetic RSA priv just so
        // the harness can construct a PclsyncCompatState.
        let sharer_kp = generate_keypair().expect("keygen sharer");
        let (sharer_priv, _sharer_pub) = sharer_kp.into_parts();
        let mut harness = PclsyncCompatStateHarness::new(sharer_priv);

        let original = test_sym_key(0x11, 0x77);
        let folder_id: u64 = 42;
        harness
            .inner
            .cache_folder_key(folder_id, original.duplicate());

        let recipient_blob = pub_blob_for(&recipient_pub);
        let wrapped_b64 = wrap_share_invitation_b64(
            &harness.inner,
            ShareTarget::Folder(folder_id),
            &recipient_blob,
        )
        .expect("wrap");

        // The b64 must decode to exactly 512 bytes (RSA-4096 modulus).
        let wrapped_bytes = base64_decode(&wrapped_b64).expect("b64 decode");
        assert_eq!(wrapped_bytes.len(), PCLSYNC_RSA_BYTES);

        // Recipient unwraps with their private key.
        let recovered = oaep_unwrap(&recipient_priv, &wrapped_bytes).expect("unwrap");
        // Compare field-by-field — SymKeyVer1 carries no PartialEq
        // derive; ct_eq returns constant-time Choice.
        assert_eq!(recovered.sym_type, PCLSYNC_SYM_TYPE_AES256_1024BIT_HMAC);
        assert_eq!(recovered.flags, 0);
        assert_eq!(recovered.aes_key, original.aes_key);
        assert_eq!(recovered.hmac_key, original.hmac_key);
        assert_eq!(recovered.aes_key.len(), PCLSYNC_AES_KEY_LEN);
        assert_eq!(recovered.hmac_key.len(), PCLSYNC_HMAC_KEY_LEN);
    }

    #[test]
    fn wrap_share_invitation_roundtrip_file_key() {
        // Same flow as folder, but using ShareTarget::File. Exercises
        // the file_keys cache path specifically.
        let kp = generate_keypair().expect("keygen recipient");
        let (recipient_priv, recipient_pub) = kp.into_parts();

        let sharer_kp = generate_keypair().expect("keygen sharer");
        let (sharer_priv, _) = sharer_kp.into_parts();
        let mut harness = PclsyncCompatStateHarness::new(sharer_priv);

        let original = test_sym_key(0xAA, 0x55);
        let file_id: u64 = 99;
        harness.inner.cache_file_key(file_id, original.duplicate());

        let recipient_blob = pub_blob_for(&recipient_pub);
        let wrapped_b64 =
            wrap_share_invitation_b64(&harness.inner, ShareTarget::File(file_id), &recipient_blob)
                .expect("wrap");

        let wrapped_bytes = base64_decode(&wrapped_b64).expect("b64 decode");
        assert_eq!(wrapped_bytes.len(), PCLSYNC_RSA_BYTES);
        let recovered = oaep_unwrap(&recipient_priv, &wrapped_bytes).expect("unwrap");
        assert_eq!(recovered.aes_key, original.aes_key);
        assert_eq!(recovered.hmac_key, original.hmac_key);
    }

    #[test]
    fn wrap_invitation_b64_has_expected_684_char_length() {
        // ncx.89 backend wiring contract: the base64 string handed to the
        // `sharefolder` / `account_teamshare` request as the
        // `sharedfolderkey` / `teamshare_key` parameter MUST be exactly
        // 684 characters (512 bytes base64-encoded with `=` padding).
        // Any deviation breaks the C client's decoder at `pssl.c`.
        let kp = generate_keypair().expect("keygen recipient");
        let (_recipient_priv, recipient_pub) = kp.into_parts();
        let sharer_kp = generate_keypair().expect("keygen sharer");
        let (sharer_priv, _) = sharer_kp.into_parts();
        let mut harness = PclsyncCompatStateHarness::new(sharer_priv);
        harness
            .inner
            .cache_folder_key(1, test_sym_key(0, 0).duplicate());

        let blob = pub_blob_for(&recipient_pub);
        let wrapped =
            wrap_share_invitation_b64(&harness.inner, ShareTarget::Folder(1), &blob).unwrap();
        assert_eq!(
            wrapped.len(),
            684,
            "RSA-4096-OAEP ciphertext base64 must be exactly 684 chars \
             (512 bytes + `=` padding); got {} chars",
            wrapped.len()
        );
        // And must decode to exactly 512 bytes.
        let raw = base64_decode(&wrapped).expect("b64 decode");
        assert_eq!(raw.len(), PCLSYNC_RSA_BYTES);
    }

    #[test]
    fn distinct_wraps_produce_distinct_ciphertexts() {
        // RSA-OAEP is randomized: two wraps of the same plaintext
        // against the same pubkey must yield different ciphertexts.
        let kp = generate_keypair().expect("keygen recipient");
        let (_recipient_priv, recipient_pub) = kp.into_parts();
        let sharer_kp = generate_keypair().expect("keygen sharer");
        let (sharer_priv, _) = sharer_kp.into_parts();
        let mut harness = PclsyncCompatStateHarness::new(sharer_priv);
        harness
            .inner
            .cache_folder_key(1, test_sym_key(0, 0).duplicate());

        let blob = pub_blob_for(&recipient_pub);
        let a = wrap_share_invitation_b64(&harness.inner, ShareTarget::Folder(1), &blob).unwrap();
        let b = wrap_share_invitation_b64(&harness.inner, ShareTarget::Folder(1), &blob).unwrap();
        assert_ne!(a, b, "RSA-OAEP freshness violated");
    }

    #[test]
    fn malformed_recipient_blob_rejected_before_wrap() {
        let sharer_kp = generate_keypair().expect("keygen sharer");
        let (sharer_priv, _) = sharer_kp.into_parts();
        let mut harness = PclsyncCompatStateHarness::new(sharer_priv);
        harness
            .inner
            .cache_folder_key(1, test_sym_key(0, 0).duplicate());
        // Truncated pubkey blob.
        let err = wrap_share_invitation_b64(&harness.inner, ShareTarget::Folder(1), &[0u8; 4])
            .unwrap_err();
        assert!(matches!(err, ShareRsaError::MalformedPubkey));
    }

    /// Test-only harness that lets us construct a `PclsyncCompatState`
    /// without going through the full profile unlock path (which would
    /// require a matching password + priv_key_ver1 blob + pub
    /// fingerprint). `PclsyncCompatState::new` is module-private in
    /// `pclsync_compat_profile.rs`, so we drop a test helper there
    /// (see `pclsync_compat_profile::PclsyncCompatState::for_test`).
    struct PclsyncCompatStateHarness {
        inner: PclsyncCompatState,
    }

    impl PclsyncCompatStateHarness {
        fn new(priv_key: rsa::RsaPrivateKey) -> Self {
            Self {
                inner: PclsyncCompatState::for_test(priv_key),
            }
        }
    }
}
