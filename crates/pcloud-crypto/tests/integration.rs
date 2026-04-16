#![allow(clippy::pedantic)]
//! Integration tests for the active crypto path.
//!
//! These tests simulate the key life-cycle expected by the Rust daemon:
//!
//! 1. `setup` the crypto subsystem with a password.
//! 2. `start` it later with the same password.
//! 3. Create encrypted folders and round-trip sector content.
//! 4. `stop` it and confirm that all sensitive material is inaccessible.
//! 5. `reset` it and confirm a clean slate.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_crypto::{CryptoError, CryptoShell, state::UnlockState};
use pcloud_secret::secret_string::SecretString;

fn pw(s: &str) -> SecretString {
    SecretString::new(s)
}

#[test]
fn full_lifecycle_encrypts_and_decrypts() {
    let mut crypto = CryptoShell::default();

    // Before setup, every operation requiring a key must fail.
    assert!(matches!(
        crypto.mkdir(None, "top", None),
        Err(CryptoError::Locked)
    ));
    assert!(matches!(
        crypto.seal_sector(b"x", 0, b"y"),
        Err(CryptoError::Locked)
    ));

    // Setup + start.
    crypto
        .setup(pw("passphrase-with-entropy"), Some("hint".into()))
        .expect("setup");
    crypto.start(pw("passphrase-with-entropy")).expect("start");
    assert_eq!(crypto.unlock_state, UnlockState::Unlocked);

    // Create folders.
    let top = crypto.mkdir(None, "top", None).expect("mkdir top");
    let child = crypto
        .mkdir(Some(top.folder_id), "child", None)
        .expect("mkdir child");
    assert_ne!(top.encrypted_name, child.encrypted_name);

    // Deterministic encrypted name.
    let top2 = crypto
        .mkdir(None, "top-two", Some(999))
        .expect("mkdir top-two");
    assert_eq!(top2.folder_id, 999);

    // Round-trip sector.
    let frame = crypto
        .seal_sector(b"file-seed-42", 0, b"the quick brown fox")
        .expect("seal");
    let round = crypto
        .open_sector(b"file-seed-42", 0, &frame)
        .expect("open");
    assert_eq!(round, b"the quick brown fox");

    // Stop locks access even though folders remain catalogued.
    crypto.stop();
    assert!(!crypto.is_started());
    assert_eq!(
        crypto.open_sector(b"file-seed-42", 0, &frame).unwrap_err(),
        CryptoError::Locked
    );
    assert_eq!(crypto.folders.len(), 3);

    // Restart and confirm the same sector still decrypts.
    crypto
        .start(pw("passphrase-with-entropy"))
        .expect("restart");
    let round2 = crypto
        .open_sector(b"file-seed-42", 0, &frame)
        .expect("open after restart");
    assert_eq!(round2, b"the quick brown fox");

    // Wrong password must not unlock, even with valid setup.
    crypto.stop();
    assert!(matches!(
        crypto.start(pw("wrong")),
        Err(CryptoError::WrongPassword)
    ));
    assert!(!crypto.is_started());

    // Reset removes everything.
    crypto
        .start(pw("passphrase-with-entropy"))
        .expect("final start");
    crypto.reset();
    assert!(!crypto.is_setup());
    assert!(!crypto.is_started());
    assert!(crypto.folders.is_empty());
    assert_eq!(crypto.unlock_state, UnlockState::NotSetup);
}

#[test]
fn policy_flag_blocks_unsafe_persistence() {
    let mut crypto = CryptoShell::default();
    crypto.policy.persist_master_key = true;
    assert!(matches!(
        crypto.setup(pw("any"), None),
        Err(CryptoError::UnsafePolicy)
    ));
}

#[test]
fn sector_tamper_detected_across_restart() {
    let mut crypto = CryptoShell::default();
    crypto.setup(pw("p"), None).unwrap();
    crypto.start(pw("p")).unwrap();
    let mut frame = crypto.seal_sector(b"seed", 0, b"payload").unwrap();
    // Flip one byte in the ciphertext region (after 4-byte index + 12-byte nonce).
    frame[20] ^= 0xFF;
    let err = crypto.open_sector(b"seed", 0, &frame).unwrap_err();
    assert!(matches!(err, CryptoError::Content(_)));
}

#[test]
fn folder_registry_exposes_ids() {
    let mut crypto = CryptoShell::default();
    crypto.setup(pw("p"), None).unwrap();
    crypto.start(pw("p")).unwrap();
    let a = crypto.mkdir(None, "a", None).unwrap().folder_id;
    let _b = crypto.mkdir(None, "b", None).unwrap().folder_id;
    let ids = crypto.folder_ids();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&a));
    assert_eq!(crypto.any_folder_id(), Some(ids[0]));
}
