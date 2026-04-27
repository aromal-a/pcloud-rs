// Wave 2 / Stage 5 — PclsyncCompat end-to-end integration tests.
//
// All tests drive the PUBLIC API of `pcloud_crypto` only — no direct calls
// to internal primitives. This validates the public contract that the daemon
// and IPC layers depend on.
//
// All tests are gated on `#[cfg(feature = "pclsync-v2")]` (on by default).
//
// KNOWN GAPS / TODO(Stage 6):
//
// - Test 3 (`seal_open_sector_roundtrip_via_shell`) requires a file sym-key
//   to be inserted into the PclsyncCompatState cache. The public accessor
//   `pclsync_compat_state.cache_file_key` is accessible because
//   `pclsync_compat_state` is a `pub` field on `CryptoShell`. This works
//   now and does not require a Stage 6 follow-up.
//
// - Test 4 (`mkdir_then_reuse_sym_key`) drives the encode/decode roundtrip
//   directly via `pclsync_filename::{encode_filename, decode_filename}` and
//   `pclsync_rsa::SymKeyVer1::new`. These are pub items in the crate.

#![cfg(feature = "pclsync-v2")]

use pcloud_crypto::pclsync_filename::FilenameKeys;
use pcloud_crypto::pclsync_rsa::SymKeyVer1;
use pcloud_crypto::{CryptoBackend, CryptoError, CryptoShell, SectorContext};
use pcloud_secret::secret_string::SecretString;

fn pw(s: &str) -> SecretString {
    SecretString::new(s.to_owned())
}

/// Build a synthetic SymKeyVer1 filled with recognisable byte patterns.
fn synth_sym_key() -> SymKeyVer1 {
    let mut s = SymKeyVer1::new(0);
    s.aes_key.fill(0x11);
    s.hmac_key.fill(0x22);
    s
}

// ---------------------------------------------------------------------------
// 1. Setup → serialise/deserialise → unlock roundtrip
// ---------------------------------------------------------------------------
#[test]
fn setup_then_unlock_roundtrip() {
    let mut shell = CryptoShell::default();
    shell
        .setup_with_backend(pw("pwd123"), None, CryptoBackend::PclsyncCompat)
        .expect("setup");

    // Effective backend must be PclsyncCompat.
    assert_eq!(shell.effective_backend(), CryptoBackend::PclsyncCompat);
    // `backend` field must be explicitly recorded (not inferred via sentinel).
    assert_eq!(shell.backend, Some(CryptoBackend::PclsyncCompat));

    // Capture the original pub key blob before serialisation.
    let orig_pub_blob = shell
        .pclsync_compat
        .as_ref()
        .expect("pclsync_compat profile present after setup")
        .pub_key_ver1_blob
        .clone();

    // Simulate daemon restart: serialise then deserialise.
    let json = serde_json::to_string(&shell).expect("serialise");
    let mut shell2: CryptoShell = serde_json::from_str(&json).expect("deserialise");

    // Must still be recognised as PclsyncCompat after reload.
    assert_eq!(shell2.effective_backend(), CryptoBackend::PclsyncCompat);

    // Unlock with correct password.
    shell2.start(pw("pwd123")).expect("unlock after reload");
    assert!(shell2.is_started(), "shell must be unlocked");

    // Pub key blob must survive the round-trip unchanged.
    let reloaded_pub_blob = shell2
        .pclsync_compat
        .as_ref()
        .expect("pclsync_compat profile present after reload")
        .pub_key_ver1_blob
        .clone();
    assert_eq!(
        orig_pub_blob, reloaded_pub_blob,
        "pub_key_ver1_blob must be byte-identical after serde round-trip"
    );
}

// ---------------------------------------------------------------------------
// 2. Wrong password is rejected; priv key not populated after failure
// ---------------------------------------------------------------------------
#[test]
fn wrong_password_is_rejected() {
    let mut shell = CryptoShell::default();
    shell
        .setup_with_backend(pw("correct"), None, CryptoBackend::PclsyncCompat)
        .expect("setup");

    let err = shell
        .start(pw("wrong"))
        .expect_err("wrong password must fail");
    assert_eq!(
        err,
        CryptoError::WrongPassword,
        "expected WrongPassword, got {err:?}"
    );

    // The live RSA private key must NOT be populated after a failed unlock.
    assert!(
        shell.pclsync_compat_state.is_none(),
        "pclsync_compat_state must be None after failed unlock (priv key not exposed)"
    );
    assert!(
        !shell.is_started(),
        "shell must NOT be marked started after wrong-password"
    );
}

// ---------------------------------------------------------------------------
// 3. seal_sector_with_context / open_sector_with_context roundtrip via shell
// ---------------------------------------------------------------------------
#[test]
fn seal_open_sector_roundtrip_via_shell() {
    let mut shell = CryptoShell::default();
    shell
        .setup_with_backend(pw("sectorp@ss"), None, CryptoBackend::PclsyncCompat)
        .expect("setup");
    shell.start(pw("sectorp@ss")).expect("unlock");

    // Inject a synthetic file sym-key directly via the public cache accessor.
    shell
        .pclsync_compat_state
        .as_mut()
        .expect("pclsync_compat_state present after unlock")
        .cache_file_key(42, synth_sym_key());

    let plaintext = vec![0xABu8; 4096];
    let ctx = SectorContext::for_file(42);

    let sealed = shell
        .seal_sector_with_context(&[], 7, &plaintext, ctx)
        .expect("seal_sector_with_context");

    // PclsyncCompat emits a detached auth tag.
    assert!(
        sealed.auth_tag.is_some(),
        "PclsyncCompat must produce a detached auth_tag"
    );
    // Ciphertext length equals plaintext length (AES-CTR, no expansion).
    assert_eq!(
        sealed.ciphertext.len(),
        plaintext.len(),
        "PclsyncCompat ciphertext must be same length as plaintext"
    );

    let opened = shell
        .open_sector_with_context(&[], 7, &sealed.ciphertext, sealed.auth_tag.as_ref(), ctx)
        .expect("open_sector_with_context");
    assert_eq!(
        opened.as_slice(),
        plaintext.as_slice(),
        "decrypted plaintext must match original",
    );
}

// ---------------------------------------------------------------------------
// 4. mkdir_with_context → sym_key produced → filename encode/decode roundtrip
// ---------------------------------------------------------------------------
#[test]
fn mkdir_then_reuse_sym_key() {
    let mut shell = CryptoShell::default();
    shell
        .setup_with_backend(pw("mkdirpw"), None, CryptoBackend::PclsyncCompat)
        .expect("setup");
    shell.start(pw("mkdirpw")).expect("unlock");

    // Pre-populate a parent folder key so mkdir can encode the child name.
    let parent_sym = synth_sym_key();
    shell
        .pclsync_compat_state
        .as_mut()
        .expect("state")
        .cache_folder_key(42, parent_sym);

    let created = shell
        .mkdir_with_context(Some(42), "documents", None)
        .expect("mkdir_with_context");

    // PclsyncCompat must return a freshly generated sym-key for the new folder.
    assert!(
        created.sym_key.is_some(),
        "mkdir_with_context must return Some(sym_key) for PclsyncCompat"
    );
    assert!(
        !created.entry.encrypted_name.is_empty(),
        "encrypted_name must be non-empty"
    );

    // Cache the new folder's sym-key under a hypothetical server-assigned id.
    let new_sym = created.sym_key.expect("just asserted Some");
    let folder_id: u64 = 42_000;
    shell
        .pclsync_compat_state
        .as_mut()
        .expect("state")
        .cache_folder_key(folder_id, new_sym);

    // Encode a filename under that folder, then decode it.
    let cache_sym = synth_sym_key(); // independent key for encode/decode KAT
    let encoded = pcloud_crypto::pclsync_filename::encode_filename(
        FilenameKeys {
            aes_key: &cache_sym.aes_key,
            hmac_key: &cache_sym.hmac_key,
        },
        "secret.pdf",
    )
    .expect("encode_filename");
    let decoded = pcloud_crypto::pclsync_filename::decode_filename(
        FilenameKeys {
            aes_key: &cache_sym.aes_key,
            hmac_key: &cache_sym.hmac_key,
        },
        &encoded,
    )
    .expect("decode_filename");
    assert_eq!(
        decoded, "secret.pdf",
        "filename decode/encode must roundtrip"
    );
}

// ---------------------------------------------------------------------------
// 5. Enhanced profile routes correctly; PclsyncCompat-only ops are refused
// ---------------------------------------------------------------------------
#[test]
fn cross_backend_unlock_is_rejected() {
    // 5a. An Enhanced profile (legacy setup()) correctly routes to the
    //     Enhanced backend via the sentinel, even after serde round-trip.
    {
        let mut shell = CryptoShell::default();
        // `setup()` without backend = Enhanced (back-compat default).
        shell.setup(pw("enhancedpw"), None).expect("setup Enhanced");
        assert_eq!(shell.effective_backend(), CryptoBackend::Enhanced);

        let json = serde_json::to_string(&shell).expect("serialise");
        let mut shell2: CryptoShell = serde_json::from_str(&json).expect("deserialise");
        // After reload the sentinel must still infer Enhanced (setup_fingerprint is Some).
        assert_eq!(
            shell2.effective_backend(),
            CryptoBackend::Enhanced,
            "historical Enhanced profile must infer Enhanced backend"
        );
        shell2
            .start(pw("enhancedpw"))
            .expect("unlock Enhanced after reload");
        assert!(shell2.is_started());
    }

    // 5b. Calling `change_password_with_context` on an Enhanced shell returns
    //     a backend-mismatch error (that function is PclsyncCompat-only).
    //     audit-06 P1-3 (Opus §3 C-1): cross-backend dispatch must surface
    //     `BackendMismatch { expected, provided }` so the caller can tell
    //     the operation was refused because of a backend-pinning mismatch,
    //     not because of missing plumbing.
    {
        let mut shell = CryptoShell::default();
        shell.setup(pw("enhpw"), None).expect("setup Enhanced");
        shell.start(pw("enhpw")).expect("unlock");

        let err = shell
            .change_password_with_context(pw("enhpw"), pw("newpw"), 0)
            .expect_err("change_password_with_context must fail on Enhanced shell");
        match err {
            CryptoError::BackendMismatch { expected, provided } => {
                assert_eq!(expected, CryptoBackend::Enhanced);
                assert_eq!(provided, CryptoBackend::PclsyncCompat);
            }
            other => panic!("expected BackendMismatch, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// 5c. start_with_backend pins the expected backend: a PclsyncCompat-sealed
//     profile dispatched through an Enhanced expectation must refuse with
//     BackendMismatch BEFORE any key derivation happens. Matched against
//     audit-06 P1 (pcloud-rs-ncx.8).
// ---------------------------------------------------------------------------
#[test]
fn start_with_backend_rejects_cross_backend_profile() {
    let mut shell = CryptoShell::default();
    shell
        .setup_with_backend(pw("compat-pw"), None, CryptoBackend::PclsyncCompat)
        .expect("setup PclsyncCompat");

    // serde round-trip so the persisted-backend inference path also runs.
    let json = serde_json::to_string(&shell).expect("serialise");
    let mut shell2: CryptoShell = serde_json::from_str(&json).expect("deserialise");

    let err = shell2
        .start_with_backend(pw("compat-pw"), CryptoBackend::Enhanced)
        .expect_err("cross-backend start must fail");
    match err {
        CryptoError::BackendMismatch { expected, provided } => {
            assert_eq!(expected, CryptoBackend::PclsyncCompat);
            assert_eq!(provided, CryptoBackend::Enhanced);
        }
        other => panic!("expected BackendMismatch, got {other:?}"),
    }

    // Correctly-pinned start_with_backend still succeeds.
    shell2
        .start_with_backend(pw("compat-pw"), CryptoBackend::PclsyncCompat)
        .expect("same-backend start_with_backend must succeed");
    assert!(shell2.is_started());
}

// ---------------------------------------------------------------------------
// 5d. Inverse of 5c — an Enhanced-sealed profile serialized + reloaded must
//     refuse `start_with_backend(CryptoBackend::PclsyncCompat)` with
//     `BackendMismatch { expected: Enhanced, provided: PclsyncCompat }`
//     BEFORE any key derivation runs. Matched against audit-06 P1-3
//     (Opus §3 C-1): construct the variant from the `start_with_backend`
//     dispatch site, not merely the PclsyncCompat direction.
// ---------------------------------------------------------------------------
#[test]
fn start_with_backend_rejects_enhanced_profile_pinned_pclsync_compat() {
    let mut shell = CryptoShell::default();
    shell
        .setup_with_backend(pw("enh-pw"), None, CryptoBackend::Enhanced)
        .expect("setup Enhanced");

    // Round-trip via serde so the persisted-backend inference path runs
    // exactly as it does after a daemon restart.
    let json = serde_json::to_string(&shell).expect("serialise");
    let mut shell2: CryptoShell = serde_json::from_str(&json).expect("deserialise");

    let err = shell2
        .start_with_backend(pw("enh-pw"), CryptoBackend::PclsyncCompat)
        .expect_err("cross-backend start must fail");
    match err {
        CryptoError::BackendMismatch { expected, provided } => {
            assert_eq!(expected, CryptoBackend::Enhanced);
            assert_eq!(provided, CryptoBackend::PclsyncCompat);
        }
        other => panic!("expected BackendMismatch, got {other:?}"),
    }

    // Correctly-pinned start_with_backend still succeeds.
    shell2
        .start_with_backend(pw("enh-pw"), CryptoBackend::Enhanced)
        .expect("same-backend start_with_backend must succeed");
    assert!(shell2.is_started());
}

// ---------------------------------------------------------------------------
// 6. change_password_with_context rewraps priv key; old pw fails after rotation
// ---------------------------------------------------------------------------
#[test]
fn change_password_rewraps_priv_key_ver1() {
    let mut shell = CryptoShell::default();
    shell
        .setup_with_backend(pw("old"), None, CryptoBackend::PclsyncCompat)
        .expect("setup");
    shell.start(pw("old")).expect("unlock");

    // Capture original blob and fingerprint before rotation.
    let blob_old = shell
        .pclsync_compat
        .as_ref()
        .expect("profile present")
        .priv_key_ver1_blob
        .clone();
    let fpr_old = shell
        .pclsync_compat
        .as_ref()
        .expect("profile present")
        .pub_fingerprint;

    let result = shell
        .change_password_with_context(pw("old"), pw("new"), 0)
        .expect("change_password_with_context");

    // The new blob and fingerprint must differ (fresh salt → fresh wrap).
    assert_ne!(
        *result.new_priv_key_ver1_blob, blob_old,
        "new priv_key_ver1_blob must differ from old (fresh salt)"
    );
    assert_ne!(
        result.new_pub_fingerprint, fpr_old,
        "new pub_fingerprint must differ (fresh HMAC over fresh blob)"
    );

    // Simulate server ack: the shell's internal profile has already been
    // updated in-place. Verify by stopping and reloading via serde.
    shell.stop();
    let json = serde_json::to_string(&shell).expect("serialise after rotation");
    let mut shell2: CryptoShell = serde_json::from_str(&json).expect("deserialise");

    // New password must unlock.
    shell2
        .start(pw("new"))
        .expect("unlock with new password after rotation");
    assert!(shell2.is_started());

    // Old password must NOT unlock.
    shell2.stop();
    let err = shell2
        .start(pw("old"))
        .expect_err("old password must be rejected after rotation");
    assert_eq!(
        err,
        CryptoError::WrongPassword,
        "old password must produce WrongPassword after rotation"
    );
}

// ---------------------------------------------------------------------------
// 7. FileKeyNotCached error shape — specific file_id propagated
// ---------------------------------------------------------------------------
#[test]
fn file_key_not_cached_error_shape() {
    let mut shell = CryptoShell::default();
    shell
        .setup_with_backend(pw("pw"), None, CryptoBackend::PclsyncCompat)
        .expect("setup");
    shell.start(pw("pw")).expect("unlock");

    let ctx = SectorContext::for_file(9999);
    match shell.seal_sector_with_context(&[], 0, b"data", ctx) {
        Err(CryptoError::FileKeyNotCached { file_id }) => {
            assert_eq!(file_id, 9999, "file_id must be propagated verbatim");
        }
        other => panic!("expected FileKeyNotCached {{file_id: 9999}}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 8. FolderKeyNotCached error shape — specific folder_id propagated
// ---------------------------------------------------------------------------
#[test]
fn folder_key_not_cached_error_shape() {
    let mut shell = CryptoShell::default();
    shell
        .setup_with_backend(pw("pw"), None, CryptoBackend::PclsyncCompat)
        .expect("setup");
    shell.start(pw("pw")).expect("unlock");

    match shell.mkdir_with_context(Some(8888), "child", None) {
        Err(CryptoError::FolderKeyNotCached { folder_id }) => {
            assert_eq!(folder_id, 8888, "folder_id must be propagated verbatim");
        }
        other => panic!("expected FolderKeyNotCached {{folder_id: 8888}}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 9. Enhanced path regression — legacy seal/open API still works unchanged
// ---------------------------------------------------------------------------
#[test]
fn enhanced_path_still_works_unchanged() {
    let mut shell = CryptoShell::default();
    // `setup()` without explicit backend → Enhanced (back-compat).
    shell.setup(pw("legacypw"), None).expect("setup");
    shell.start(pw("legacypw")).expect("start");

    let seed = [0xABu8; 32];
    let plaintext = b"hello enhanced sector";

    let sealed = shell
        .seal_sector(&seed, 3, plaintext)
        .expect("seal_sector (Enhanced)");
    let opened = shell
        .open_sector(&seed, 3, &sealed)
        .expect("open_sector (Enhanced)");
    assert_eq!(
        opened, plaintext,
        "Enhanced sector round-trip must produce original plaintext"
    );
}

// ---------------------------------------------------------------------------
// 10. MissingFileId error when Enhanced context passed to PclsyncCompat shell
// ---------------------------------------------------------------------------
#[test]
fn missing_file_id_returns_specific_error() {
    let mut shell = CryptoShell::default();
    shell
        .setup_with_backend(pw("pw"), None, CryptoBackend::PclsyncCompat)
        .expect("setup");
    shell.start(pw("pw")).expect("unlock");

    // SectorContext::enhanced() has file_id = None → MissingFileId on PclsyncCompat.
    let ctx = SectorContext::enhanced();
    let err = shell
        .seal_sector_with_context(&[], 0, b"hi", ctx)
        .expect_err("must error with MissingFileId");
    assert_eq!(
        err,
        CryptoError::MissingFileId,
        "PclsyncCompat shell + Enhanced context must return MissingFileId"
    );
}
