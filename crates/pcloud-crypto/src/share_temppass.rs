//! Temporary-password (“temppass”) key-rewrap for crypto-folder sharing.
//!
//! # C parity surface
//!
//! Retained C entry points, both in `pclsync/psynclib.c`:
//!
//! * `psync_crypto_share_folder` (line 1322) – when the caller supplies a
//!   `temppass`, the C client calls `pcryptofolder_change_pass_unlocked`
//!   with `PSYNC_CRYPTO_FLAG_TEMP_PASS`. That function re-wraps the
//!   currently-unlocked user private key under the temporary passphrase
//!   and signs the re-wrapped blob. The base64 of both (`privenc`,
//!   `sign`) is then forwarded as extra params to the `sharefolder`
//!   request so the invitee can, after entering the temppass, recover
//!   the wrapping needed to decrypt shared folder content.
//! * `psync_crypto_account_teamshare` (line 1372) – identical flow, but
//!   the extra params are attached to `account_teamshare`.
//!
//! # Rust security posture
//!
//! This module mirrors the **shape** of the C flow without reintroducing
//! any of its weaker defaults:
//!
//! * Temppass input is a [`SecretString`] and is zeroized on drop.
//! * The intermediate key-encryption key is held in `SecretBytes`
//!   and is zeroized on drop.
//! * The plaintext "wrapped payload" (i.e. the active master key material
//!   we are rewrapping so that the invitee can later unwrap under the
//!   same temppass) is never copied outside a `SecretBytes` buffer.
//! * No part of the derivation is ever persisted to disk.
//! * No part of the derivation is ever logged.
//! * The wrapping AEAD is AES-256-GCM, not the C-era AES-CTR + separate
//!   SHA signature combination. Integrity is enforced by the AEAD tag
//!   plus a detached HMAC-SHA256 "signature" computed under the active
//!   master key, matching the `privenc` + `sign` two-blob shape that
//!   the wire expects.
//! * All cross-peer equality checks use [`subtle::ConstantTimeEq`].
//!
//! The active Rust crypto path (see [`crate::keys::KeyManager`]) does
//! not yet store an RSA-4096 keypair in the form the C client expects,
//! so the "signature" produced here is an HMAC-SHA256 tag under the
//! active master key rather than an RSA signature under the user
//! private key. This is clearly documented and is strictly stronger
//! than "no authentication at all". When RSA keypair mirroring lands
//! under bd-1du.5, `TemppassBlob::sign` is the single place to swap
//! to `prsa_sign_sha256_hash`.

// **PLATFORM:** all
// **GATING:** none (portable).

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use getrandom::getrandom;
use hmac::{Hmac, Mac};
use pcloud_secret::secret_bytes::SecretBytes;
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;

use crate::crypto_util::{base64_decode as b64_crate_decode, base64_encode as b64_crate_encode};
use crate::keys::KeyManager;
use crate::{CryptoBackend, CryptoError, CryptoShell};

/// Versioned on-wire blob layout. Bumping this invalidates old blobs.
const TEMPPASS_BLOB_VERSION: u8 = 1;
const TEMPPASS_SALT_LEN: usize = 16;
const TEMPPASS_NONCE_LEN: usize = 12;
const TEMPPASS_HMAC_LEN: usize = 32;
const TEMPPASS_SIG_LABEL: &[u8] = b"pcloud-crypto/share-temppass/sig/v1";
const TEMPPASS_AAD: &[u8] = b"pcloud-crypto/share-temppass/aad/v1";

/// Errors specific to the share-temppass wrap/unwrap path.
///
/// These are intentionally collapsed into a single opaque
/// [`CryptoError::WrongPassword`] at the public API boundary (see the
/// `From` impl below) so a caller cannot distinguish tampering from a
/// plain wrong-password.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TemppassError {
    /// Crypto is locked; the master key needed to wrap/unwrap is not resident.
    #[error("crypto is locked; cannot derive a temppass share blob")]
    Locked,
    /// The temporary password is empty.
    #[error("temppass must not be empty")]
    EmptyPassword,
    /// The on-wire blob is malformed (wrong version, truncated, bad layout).
    #[error("temppass blob is malformed")]
    Malformed,
    /// Detached HMAC-SHA256 signature verification failed.
    #[error("temppass signature verification failed")]
    BadSignature,
    /// AES-256-GCM unwrap failed: wrong password or tampered ciphertext/AAD.
    #[error("temppass unwrap failed: wrong password or tampered blob")]
    Unwrap,
    /// Base64 decoding of a wire field failed.
    #[error("base64 decode failed")]
    Base64,
    /// The active crypto backend is [`CryptoBackend::PclsyncCompat`] but
    /// the share-invite handoff requires the RSA-4096-OAEP wrap flow
    /// that the retained Rust path has not yet implemented. Issuing a
    /// symmetric HMAC-SHA256 blob here would produce an envelope that
    /// pCloud C clients cannot decrypt (silent data loss for invitees).
    ///
    /// The Enhanced backend is unaffected; see the module docs.
    /// Tracked under `pcloud-rs-ncx.89` (RSA-4096-OAEP wrap for crypto
    /// share invitation — C-interop parity), filed from audit-06 ncx.5.
    #[error(
        "crypto share invitation requires RSA-4096; not yet supported on the PclsyncCompat backend"
    )]
    RsaBackendRequired,
}

impl From<TemppassError> for CryptoError {
    fn from(err: TemppassError) -> Self {
        match err {
            TemppassError::Locked => CryptoError::Locked,
            TemppassError::EmptyPassword => CryptoError::EmptyPassword,
            // Surface the backend-gate as `NotYetWired` so the daemon
            // dispatch layer returns a distinct, non-opaque error to the
            // operator (instead of the generic "wrong password" oracle).
            // Silent garbage-blob issuance is precisely the audit-06
            // ncx.5 failure mode this guard exists to prevent.
            TemppassError::RsaBackendRequired => CryptoError::NotYetWired,
            // Everything else maps to WrongPassword-ish; the caller sees a
            // single opaque "temppass derivation failed" signal and never
            // learns whether salt was wrong, tag mismatched, etc.
            _ => CryptoError::WrongPassword,
        }
    }
}

/// Base64-encoded `(privatekey, signature)` pair ready for the wire.
/// Matches the shape of C's `privenc` + `sign` in `psynclib.c` @ 1353-1354
/// and 1404-1405.
///
/// # Security
/// Both fields are non-secret *in isolation*: `private_key_b64` is the
/// AES-256-GCM-wrapped master key under a temppass-derived key-
/// encryption-key (12-byte nonce, 16-byte tag, fixed versioned AAD,
/// 16-byte salt) and is only unwrappable by a party holding the
/// temppass. `signature_b64` is an HMAC-SHA256 tag proving the
/// currently-unlocked master-key holder produced the blob. Neither
/// carries the temppass itself nor the plaintext master key.
///
/// Per ADR-0007 this wire pair is never persisted on disk; it is
/// ephemeral state passed to the share call and dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemppassWire {
    /// Base64 encoding of the versioned AES-256-GCM blob (version || salt ||
    /// nonce || ciphertext||tag). Non-secret in isolation, but carries the
    /// master key wrapped under the temporary password.
    ///
    /// # Security
    /// Confidentiality of the wrapped master key relies on Argon2id key
    /// derivation from the temppass plus AES-256-GCM authenticated
    /// encryption. Disclosure of this field to a third party who does
    /// not hold the temppass does not break confidentiality.
    pub private_key_b64: String,
    /// Base64 encoding of the detached HMAC-SHA256 signature over
    /// `TEMPPASS_SIG_LABEL || private_key_bytes`, keyed by the master key.
    ///
    /// # Security
    /// Proves the blob came from a session that currently holds the
    /// master key. The HMAC-SHA256 key is borrowed from the active
    /// `SecretBytes` and never copied.
    pub signature_b64: String,
}

/// In-memory raw temppass blob. Never serialized directly — always
/// go through [`TemppassWire`] on the way out.
///
/// # Security
/// The `Debug` impl redacts the ciphertext field (see the
/// `tests::debug_impl_redacts_ciphertext` test). The struct carries no
/// `Clone` derivation; callers must be explicit to duplicate. The
/// ciphertext itself is AES-256-GCM output that decodes to the master
/// key only when the correct temppass is supplied.
pub struct TemppassBlob {
    version: u8,
    salt: [u8; TEMPPASS_SALT_LEN],
    nonce: [u8; TEMPPASS_NONCE_LEN],
    ciphertext: Vec<u8>,
}

impl std::fmt::Debug for TemppassBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TemppassBlob")
            .field("version", &self.version)
            .field("len", &self.ciphertext.len())
            .field("ct", &"<redacted>")
            .finish()
    }
}

impl TemppassBlob {
    fn encode(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(1 + TEMPPASS_SALT_LEN + TEMPPASS_NONCE_LEN + self.ciphertext.len());
        out.push(self.version);
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.ciphertext);
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self, TemppassError> {
        if bytes.len() < 1 + TEMPPASS_SALT_LEN + TEMPPASS_NONCE_LEN + 16 {
            return Err(TemppassError::Malformed);
        }
        let version = bytes[0];
        if version != TEMPPASS_BLOB_VERSION {
            return Err(TemppassError::Malformed);
        }
        let mut salt = [0u8; TEMPPASS_SALT_LEN];
        salt.copy_from_slice(&bytes[1..1 + TEMPPASS_SALT_LEN]);
        let mut nonce = [0u8; TEMPPASS_NONCE_LEN];
        nonce.copy_from_slice(
            &bytes[1 + TEMPPASS_SALT_LEN..1 + TEMPPASS_SALT_LEN + TEMPPASS_NONCE_LEN],
        );
        let ciphertext = bytes[1 + TEMPPASS_SALT_LEN + TEMPPASS_NONCE_LEN..].to_vec();
        Ok(Self {
            version,
            salt,
            nonce,
            ciphertext,
        })
    }

    /// HMAC-SHA256 signature over the encoded blob, keyed by the active
    /// master key material. Documented substitute for the C-era
    /// `prsa_sign_sha256_hash(crypto_privkey, …)` until the Rust active
    /// path gains an RSA-4096 keypair under bd-1du.5.
    fn sign(&self, master: &SecretBytes) -> [u8; TEMPPASS_HMAC_LEN] {
        // INVARIANT: HMAC-SHA256 accepts keys of any non-zero length per RFC 2104;
        // callers always pass a 32-byte derived master key.
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(master.expose_secret())
            .expect("HMAC-SHA256 accepts any key length");
        mac.update(TEMPPASS_SIG_LABEL);
        mac.update(&self.encode());
        let out: [u8; TEMPPASS_HMAC_LEN] = mac.finalize().into_bytes().into();
        out
    }

    fn verify(&self, master: &SecretBytes, signature: &[u8]) -> Result<(), TemppassError> {
        if signature.len() != TEMPPASS_HMAC_LEN {
            return Err(TemppassError::BadSignature);
        }
        let expected = self.sign(master);
        if expected.ct_eq(signature).unwrap_u8() == 1 {
            Ok(())
        } else {
            Err(TemppassError::BadSignature)
        }
    }
}

/// Derive a temppass share blob from the active master-key material.
///
/// Mirrors the semantic of C's `pcryptofolder_change_pass_unlocked(
/// temppass, PSYNC_CRYPTO_FLAG_TEMP_PASS, &priv_key, &signature)`.
///
/// Primitive stack:
/// * Argon2id (crate defaults: `m = 19456`, `t = 2`, `p = 1`) to derive
///   the key-encryption-key from the 16-byte random salt + temppass.
/// * AES-256-GCM to wrap the master-key bytes (12-byte random nonce,
///   16-byte tag, fixed versioned AAD, so nonce-collision risk is
///   bounded by `2^-96` per call).
/// * HMAC-SHA256 under the active master key for the detached
///   signature (32-byte output).
///
/// Inputs:
///
/// * `shell` — must be [`CryptoShell::is_started`], else
///   [`TemppassError::Locked`] is returned **without touching any key
///   material**. This is how the C client's `PSYNC_CRYPTO_NOT_STARTED`
///   error surfaces to the share call.
/// * `temppass` — the invitee-facing temporary passphrase
///   ([`SecretString`], zeroize on drop, no-`Clone` discipline). Empty
///   is rejected.
///
/// Output: base64-encoded `(private_key, signature)` pair ready to be
/// attached to `sharefolder` / `account_teamshare`.
///
/// # Security
/// Mitigates: temppass exposure on the wire (temppass is never sent,
/// only its Argon2id output is used to wrap), cross-share replay
/// (fresh 16-byte salt + 12-byte nonce on every call — see
/// `tests::distinct_invocations_produce_distinct_wires`), and
/// impersonation of the blob origin (HMAC-SHA256 under the active
/// master key). Per ADR-0007 the temppass is never persisted on disk.
///
/// Out of scope: the strength of the temppass itself — a weak
/// temppass is vulnerable to offline Argon2id-cost guessing once an
/// attacker captures the blob. The HMAC signature is a symmetric
/// substitute for the C client's RSA signature; it proves
/// *current-master-key-holder* origin, not the user-identity binding
/// an RSA key would provide. The RSA swap is tracked under bd-1du.5.
///
/// # Test vectors
/// Round-trip: `tests::round_trip_share_then_accept_recovers_master_material`.
/// Tamper: `tests::tampered_blob_fails_signature`.
/// Wrong temppass: `tests::wrong_temppass_is_rejected_and_does_not_leak_partial_plaintext`.
///
/// # Errors
/// [`TemppassError::Locked`], [`TemppassError::EmptyPassword`],
/// [`TemppassError::Malformed`] on CSPRNG / AEAD init failure.
///
/// # Panics
/// Does not panic.
pub fn derive_temppass_wire(
    shell: &CryptoShell,
    temppass: &SecretString,
) -> Result<TemppassWire, TemppassError> {
    if temppass.is_empty() {
        return Err(TemppassError::EmptyPassword);
    }

    // Preserve the existing "locked crypto is rejected without ever
    // touching key material" contract. The Locked check comes before
    // the backend guard so a caller on a locked shell always sees the
    // same Locked error regardless of which backend is configured —
    // this avoids leaking backend state to a caller who hasn't even
    // unlocked yet.
    let master = shell
        .keys
        .active_key_material
        .as_ref()
        .ok_or(TemppassError::Locked)?;

    // Audit-06 ncx.5 guard: refuse to issue an HMAC-SHA256 wrap blob
    // when the caller is running on the C-compatible backend. The C
    // invitee decrypts using RSA-4096-OAEP against their own public key
    // and would silently fail to unwrap the symmetric substitute, with
    // no user-visible error. Fail loudly and early instead. See
    // STATUS.md rows 124/142 and follow-up bead `pcloud-rs-ncx.89`
    // which tracks the real RSA-4096-OAEP wrap implementation.
    //
    // The Enhanced backend continues to work: it is wire-incompatible
    // with the C clients by design (see `CryptoBackend::Enhanced`
    // docstring), so the symmetric HMAC substitute is the intended
    // authentication primitive for that backend until the full RSA
    // flow lands.
    if matches!(shell.effective_backend(), CryptoBackend::PclsyncCompat) {
        return Err(TemppassError::RsaBackendRequired);
    }

    // 1. Fresh random salt and nonce.
    let mut salt = [0u8; TEMPPASS_SALT_LEN];
    let mut nonce = [0u8; TEMPPASS_NONCE_LEN];
    getrandom(&mut salt).map_err(|_| TemppassError::Malformed)?;
    getrandom(&mut nonce).map_err(|_| TemppassError::Malformed)?;

    // 2. Derive a key-encryption-key from the temppass.
    let kek = KeyManager::derive_key_material_with_salt(temppass, &salt);

    // 3. Wrap the active master key material under AES-256-GCM.
    //    (In the C client this is the user's RSA private key; the Rust
    //     active path keeps a symmetric master key, and bd-1du.5
    //     documents that swap.)
    let cipher =
        Aes256Gcm::new_from_slice(kek.expose_secret()).map_err(|_| TemppassError::Malformed)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: master.expose_secret(),
                aad: TEMPPASS_AAD,
            },
        )
        .map_err(|_| TemppassError::Malformed)?;

    let blob = TemppassBlob {
        version: TEMPPASS_BLOB_VERSION,
        salt,
        nonce,
        ciphertext,
    };

    // 4. Sign the encoded blob with the active master key.
    let signature = blob.sign(master);

    // 5. Base64 on the way out.
    Ok(TemppassWire {
        private_key_b64: b64_crate_encode(&blob.encode()),
        signature_b64: b64_crate_encode(&signature),
    })
}

/// Recipient-side inverse used by tests to prove round-trip. Given a
/// temppass, unwrap the blob and return the recovered master-key
/// material as `SecretBytes`. Requires the verifier master key
/// material to validate the detached signature.
///
/// This is NOT part of the retained C surface that runs in production
/// on the invitee device — the invitee in C decrypts with its own RSA
/// flow. The function exists here so the round-trip property test can
/// prove we haven't produced an unrecoverable blob.
///
/// # Security
/// Mitigates: blob tampering (HMAC-SHA256 verification is performed
/// *before* any AEAD unwrap, using constant-time `ct_eq`);
/// chosen-ciphertext oracles (wrong-key and wrong-password both
/// collapse to [`TemppassError::Unwrap`], wrong-origin collapses to
/// [`TemppassError::BadSignature`] — the caller never learns *which*
/// part of the blob failed, matching the C-client's single-error
/// surface). The recovered master key is wrapped in `SecretBytes` so
/// it zeroizes on drop; it is not `Clone` and must be handled via
/// `clone_secret()` if duplication is intentional.
///
/// Out of scope: this helper verifies under a caller-supplied
/// `verifier_master`; the production invitee flow uses the invitee's
/// own RSA public key (tracked under bd-1du.5). The symmetric
/// verification here proves the blob came from the expected peer only
/// when both sides already share the master-key material.
///
/// # Errors
/// [`TemppassError::EmptyPassword`], [`TemppassError::Base64`],
/// [`TemppassError::Malformed`], [`TemppassError::BadSignature`],
/// [`TemppassError::Unwrap`].
///
/// # Panics
/// Does not panic.
pub fn accept_temppass_wire(
    wire: &TemppassWire,
    temppass: &SecretString,
    verifier_master: &SecretBytes,
) -> Result<SecretBytes, TemppassError> {
    if temppass.is_empty() {
        return Err(TemppassError::EmptyPassword);
    }
    let blob_bytes = b64_crate_decode(&wire.private_key_b64).map_err(|_| TemppassError::Base64)?;
    let signature = b64_crate_decode(&wire.signature_b64).map_err(|_| TemppassError::Base64)?;
    let blob = TemppassBlob::decode(&blob_bytes)?;
    blob.verify(verifier_master, &signature)?;

    let kek = KeyManager::derive_key_material_with_salt(temppass, &blob.salt);
    let cipher =
        Aes256Gcm::new_from_slice(kek.expose_secret()).map_err(|_| TemppassError::Unwrap)?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&blob.nonce),
            Payload {
                msg: &blob.ciphertext,
                aad: TEMPPASS_AAD,
            },
        )
        .map_err(|_| TemppassError::Unwrap)?;
    Ok(SecretBytes::new(plaintext))
}

// base64 encode/decode now delegated to crate::crypto_util (which wraps the
// `base64` workspace dependency). The hand-rolled implementation was removed
// in audit-04 fix P3/M5.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CryptoShell;
    use pcloud_secret::secret_string::SecretString;

    fn started_shell(pw: &str) -> CryptoShell {
        let mut s = CryptoShell::default();
        s.setup(SecretString::new(pw), None).unwrap();
        s.start(SecretString::new(pw)).unwrap();
        s
    }

    #[test]
    fn locked_crypto_is_rejected_without_touching_material() {
        let mut shell = CryptoShell::default();
        shell.setup(SecretString::new("master"), None).unwrap();
        // Not started on purpose.
        let err = derive_temppass_wire(&shell, &SecretString::new("temp")).unwrap_err();
        assert_eq!(err, TemppassError::Locked);
        assert_eq!(CryptoError::from(err), CryptoError::Locked);
    }

    #[test]
    fn empty_temppass_rejected() {
        let shell = started_shell("master");
        let err = derive_temppass_wire(&shell, &SecretString::new("")).unwrap_err();
        assert_eq!(err, TemppassError::EmptyPassword);
    }

    #[test]
    fn round_trip_share_then_accept_recovers_master_material() {
        let shell = started_shell("master");
        let master_clone = shell
            .keys
            .active_key_material
            .as_ref()
            .unwrap()
            .clone_secret();

        let wire = derive_temppass_wire(&shell, &SecretString::new("invitee-temp")).unwrap();
        // base64 shape
        assert!(!wire.private_key_b64.is_empty());
        assert!(!wire.signature_b64.is_empty());
        // signature is HMAC-SHA256 -> 32 bytes -> 44 b64 chars w/ padding
        assert_eq!(wire.signature_b64.len(), 44);

        let recovered =
            accept_temppass_wire(&wire, &SecretString::new("invitee-temp"), &master_clone).unwrap();
        assert_eq!(recovered, master_clone);
    }

    #[test]
    fn wrong_temppass_is_rejected_and_does_not_leak_partial_plaintext() {
        let shell = started_shell("master");
        let master_clone = shell
            .keys
            .active_key_material
            .as_ref()
            .unwrap()
            .clone_secret();
        let wire = derive_temppass_wire(&shell, &SecretString::new("correct")).unwrap();
        let err =
            accept_temppass_wire(&wire, &SecretString::new("wrong"), &master_clone).unwrap_err();
        assert_eq!(err, TemppassError::Unwrap);
    }

    #[test]
    fn unauthorized_recipient_signature_check_fails() {
        let shell = started_shell("master");
        let wire = derive_temppass_wire(&shell, &SecretString::new("temp")).unwrap();
        // Impersonator holds a different master.
        let impostor_master = SecretBytes::new(vec![0xAAu8; 32]);
        let err =
            accept_temppass_wire(&wire, &SecretString::new("temp"), &impostor_master).unwrap_err();
        assert_eq!(err, TemppassError::BadSignature);
    }

    #[test]
    fn tampered_blob_fails_signature() {
        let shell = started_shell("master");
        let master_clone = shell
            .keys
            .active_key_material
            .as_ref()
            .unwrap()
            .clone_secret();
        let mut wire = derive_temppass_wire(&shell, &SecretString::new("temp")).unwrap();
        // Flip a single character in the blob.
        let mut bytes = b64_crate_decode(&wire.private_key_b64).unwrap();
        bytes[20] ^= 0x01;
        wire.private_key_b64 = b64_crate_encode(&bytes);
        let err =
            accept_temppass_wire(&wire, &SecretString::new("temp"), &master_clone).unwrap_err();
        assert_eq!(err, TemppassError::BadSignature);
    }

    #[test]
    fn distinct_invocations_produce_distinct_wires() {
        // Salt + nonce freshness property: two derivations with the same
        // temppass against the same shell must not collide.
        let shell = started_shell("master");
        let w1 = derive_temppass_wire(&shell, &SecretString::new("temp")).unwrap();
        let w2 = derive_temppass_wire(&shell, &SecretString::new("temp")).unwrap();
        assert_ne!(w1.private_key_b64, w2.private_key_b64);
        assert_ne!(w1.signature_b64, w2.signature_b64);
    }

    #[test]
    fn malformed_blob_rejected() {
        let shell = started_shell("master");
        let master_clone = shell
            .keys
            .active_key_material
            .as_ref()
            .unwrap()
            .clone_secret();
        let bad = TemppassWire {
            private_key_b64: b64_crate_encode(&[1, 2, 3]),
            signature_b64: b64_crate_encode(&[0u8; 32]),
        };
        let err =
            accept_temppass_wire(&bad, &SecretString::new("temp"), &master_clone).unwrap_err();
        assert_eq!(err, TemppassError::Malformed);
    }

    #[test]
    fn debug_impl_redacts_ciphertext() {
        let blob = TemppassBlob {
            version: 1,
            salt: [0u8; 16],
            nonce: [0u8; 12],
            ciphertext: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let rendered = format!("{:?}", blob);
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("DEADBEEF"));
        assert!(!rendered.contains("deadbeef"));
    }

    #[test]
    fn pclsync_compat_backend_refuses_to_issue_temppass_wire() {
        // Audit-06 ncx.5 guard: the HMAC-SHA256 detached signature we
        // emit for Enhanced is wire-incompatible with the C client's
        // RSA-4096-OAEP wrap. The guard must fire *before* any key
        // material is touched so a compromised temppass (or a malicious
        // share request) cannot be used as a derivation oracle either.
        let mut shell = started_shell("master");
        // Force the backend selector to the C-compat path (the shell
        // was set up under Enhanced by default; the profile loader
        // in production would have recorded PclsyncCompat directly).
        shell.backend = Some(CryptoBackend::PclsyncCompat);

        let err = derive_temppass_wire(&shell, &SecretString::new("invitee-temp")).unwrap_err();
        assert_eq!(err, TemppassError::RsaBackendRequired);
        assert_eq!(CryptoError::from(err), CryptoError::NotYetWired);
    }

    #[test]
    fn enhanced_backend_still_issues_temppass_wire() {
        // Regression fence: the guard must not accidentally reject the
        // Enhanced backend, which is intentionally wire-incompatible
        // with C clients and uses the symmetric HMAC substitute by
        // design. This is the round-trip path the Enhanced backend
        // relies on until RSA-4096-OAEP wrap lands for both backends.
        let shell = started_shell("master"); // setup() defaults to Enhanced
        assert_eq!(shell.effective_backend(), CryptoBackend::Enhanced);
        let wire = derive_temppass_wire(&shell, &SecretString::new("invitee-temp"))
            .expect("enhanced backend must still produce a wire blob");
        assert!(!wire.private_key_b64.is_empty());
        assert!(!wire.signature_b64.is_empty());
    }

    #[test]
    fn base64_round_trip() {
        // Exercises the crypto_util wrappers that replaced the hand-rolled impl.
        for sample in [
            &[][..],
            &[0][..],
            &[0, 1][..],
            &[0, 1, 2][..],
            b"hello, world",
        ] {
            let enc = b64_crate_encode(sample);
            let dec = b64_crate_decode(&enc).unwrap();
            assert_eq!(dec, sample);
        }
    }
}
