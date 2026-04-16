#![allow(clippy::pedantic)]
//! Integration test for the crypto password-change family.
//!
//! Exercises the end-to-end slice landed by the crypto-password-change
//! agent (see `CLAUDE.md` / `bd-1du.5`):
//!
//! 1. set up crypto,
//! 2. unlock,
//! 3. rotate the passphrase through the unlocked flow
//!    (`Request::CryptoChangePasswordUnlocked`),
//! 4. lock and re-unlock with the NEW passphrase,
//! 5. rotate again through the locked flow
//!    (`Request::CryptoChangePassword` — takes old + new).
//!
//! The daemon is bootstrapped in `Environment::Development` so the
//! `crypto_sendchangeuserprivate` and `crypto_changeuserprivate` binary
//! API calls are intercepted by `DevelopmentCryptoTransport` and do not
//! reach the network. A synthetic auth token is injected directly via the
//! auth vault flow so that an authenticated session is established for
//! the lifetime of the test.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::time::{SystemTime, UNIX_EPOCH};

use pcloud_auth::AuthCommand;
use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::{bootstrap_with_config, dispatch};
use pcloud_ipc::{Method, Request, ResponseStatus};
use pcloud_model::ids::UserId;
use pcloud_secret::secret_string::SecretString;

fn bootstrap_authenticated_shell() -> pcloud_daemon::RuntimeShell {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "pcloud-daemon-crypto-change-pass-{}-{nonce}",
        std::process::id()
    ));
    let config = ConfigProfile::secure_defaults(root, Environment::Development);
    let mut runtime = bootstrap_with_config(config).expect("bootstrap ok");

    // Drive the auth state machine through a synthetic token login so the
    // crypto helpers see an authenticated session. The DevelopmentCrypto-
    // Transport only requires a non-empty `auth` string; it never leaves
    // the process and never hits a real pCloud server.
    runtime
        .auth
        .apply(AuthCommand::LoginWithToken {
            token: SecretString::new("integration-token".to_owned()),
        })
        .expect("login with token");
    runtime
        .auth
        .apply(AuthCommand::MarkAuthenticated {
            user_id: Some(UserId::new(1)),
            auth_token: SecretString::new("integration-token".to_owned()),
        })
        .expect("mark authenticated");
    runtime
}

#[test]
fn change_and_reunlock_cycle_succeeds() {
    let mut runtime = bootstrap_authenticated_shell();

    // --- Step 1: set up + unlock crypto with the initial password.
    let setup = dispatch(
        &mut runtime,
        Request::CryptoSetup {
            password: "initial-pass".to_owned(),
            hint: Some("initial hint".to_owned()),
        },
    );
    assert_eq!(setup.status, ResponseStatus::Ok, "setup: {}", setup.message);

    let unlock = dispatch(
        &mut runtime,
        Request::CryptoUnlock {
            password: "initial-pass".to_owned(),
        },
    );
    assert_eq!(
        unlock.status,
        ResponseStatus::Ok,
        "unlock: {}",
        unlock.message
    );
    assert!(runtime.crypto.is_started());

    // --- Step 2: request the server-side confirmation code (dev transport
    // will return result=0 for any non-empty auth token).
    let send_code = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::SendCryptoChangeUserPrivate,
        },
    );
    assert_eq!(
        send_code.status,
        ResponseStatus::Ok,
        "send_code: {}",
        send_code.message
    );

    // --- Step 3: flags start at 0, and the flags getter must report that.
    let flags_before = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetCryptoPrivKeyFlags,
        },
    );
    assert_eq!(flags_before.status, ResponseStatus::Ok);
    assert!(flags_before.message.contains("flags=0"));

    // --- Step 4: rotate through the unlocked flow, setting the temp-pass flag.
    let rotate = dispatch(
        &mut runtime,
        Request::CryptoChangePasswordUnlocked {
            new_password: "second-pass".to_owned(),
            hint: "second hint".to_owned(),
            code: "CONFIRM-1".to_owned(),
            flags: pcloud_crypto::keys::PRIV_KEY_FLAG_TEMP_PASS,
        },
    );
    assert_eq!(
        rotate.status,
        ResponseStatus::Ok,
        "rotate: {}",
        rotate.message
    );
    assert_eq!(
        runtime.crypto.priv_key_flags(),
        pcloud_crypto::keys::PRIV_KEY_FLAG_TEMP_PASS
    );
    assert!(runtime.crypto.is_started(), "still unlocked after rotation");

    // --- Step 5: lock + re-unlock with the NEW password must succeed.
    let lock = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::LockCrypto,
        },
    );
    assert_eq!(lock.status, ResponseStatus::Ok);

    // Old password must NOT work after rotation.
    let bad_unlock = dispatch(
        &mut runtime,
        Request::CryptoUnlock {
            password: "initial-pass".to_owned(),
        },
    );
    assert_eq!(
        bad_unlock.status,
        ResponseStatus::Unauthorized,
        "old pw: {}",
        bad_unlock.message
    );
    assert!(!runtime.crypto.is_started());

    // New password must unlock.
    let reunlock = dispatch(
        &mut runtime,
        Request::CryptoUnlock {
            password: "second-pass".to_owned(),
        },
    );
    assert_eq!(
        reunlock.status,
        ResponseStatus::Ok,
        "reunlock: {}",
        reunlock.message
    );
    assert!(runtime.crypto.is_started());

    // --- Step 6: rotate again through the locked-path flow, which takes
    // old + new. Wrong old password must be rejected up front.
    let wrong_old = dispatch(
        &mut runtime,
        Request::CryptoChangePassword {
            old_password: "not-the-right-one".to_owned(),
            new_password: "third-pass".to_owned(),
            hint: "third hint".to_owned(),
            code: "CONFIRM-2".to_owned(),
            flags: 0,
        },
    );
    assert_eq!(wrong_old.status, ResponseStatus::Unauthorized);

    let rotate_2 = dispatch(
        &mut runtime,
        Request::CryptoChangePassword {
            old_password: "second-pass".to_owned(),
            new_password: "third-pass".to_owned(),
            hint: "third hint".to_owned(),
            code: "CONFIRM-2".to_owned(),
            flags: 0,
        },
    );
    assert_eq!(
        rotate_2.status,
        ResponseStatus::Ok,
        "rotate_2: {}",
        rotate_2.message
    );
    assert_eq!(runtime.crypto.priv_key_flags(), 0);

    // --- Step 7: full lock + unlock with the latest password.
    let lock_2 = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::LockCrypto,
        },
    );
    assert_eq!(lock_2.status, ResponseStatus::Ok);
    let final_unlock = dispatch(
        &mut runtime,
        Request::CryptoUnlock {
            password: "third-pass".to_owned(),
        },
    );
    assert_eq!(
        final_unlock.status,
        ResponseStatus::Ok,
        "final_unlock: {}",
        final_unlock.message
    );
}

#[test]
fn change_password_without_auth_is_rejected() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "pcloud-daemon-crypto-noauth-{}-{nonce}",
        std::process::id()
    ));
    let config = ConfigProfile::secure_defaults(root, Environment::Development);
    let mut runtime = bootstrap_with_config(config).expect("bootstrap ok");

    // Set up + unlock crypto locally but leave the session unauthenticated.
    let _ = dispatch(
        &mut runtime,
        Request::CryptoSetup {
            password: "p".to_owned(),
            hint: None,
        },
    );
    let _ = dispatch(
        &mut runtime,
        Request::CryptoUnlock {
            password: "p".to_owned(),
        },
    );
    assert!(runtime.crypto.is_started());

    let resp = dispatch(
        &mut runtime,
        Request::CryptoChangePasswordUnlocked {
            new_password: "q".to_owned(),
            hint: "".to_owned(),
            code: "C".to_owned(),
            flags: 0,
        },
    );
    assert_eq!(resp.status, ResponseStatus::Conflict);
    assert!(resp.message.contains("authenticated"));

    let resp_send = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::SendCryptoChangeUserPrivate,
        },
    );
    assert_eq!(resp_send.status, ResponseStatus::Conflict);
}

#[test]
fn change_password_empty_inputs_rejected() {
    let mut runtime = bootstrap_authenticated_shell();
    let _ = dispatch(
        &mut runtime,
        Request::CryptoSetup {
            password: "p".to_owned(),
            hint: None,
        },
    );
    let _ = dispatch(
        &mut runtime,
        Request::CryptoUnlock {
            password: "p".to_owned(),
        },
    );

    let empty_pw = dispatch(
        &mut runtime,
        Request::CryptoChangePasswordUnlocked {
            new_password: "".to_owned(),
            hint: "".to_owned(),
            code: "C".to_owned(),
            flags: 0,
        },
    );
    assert_eq!(empty_pw.status, ResponseStatus::InvalidRequest);

    let empty_code = dispatch(
        &mut runtime,
        Request::CryptoChangePasswordUnlocked {
            new_password: "q".to_owned(),
            hint: "".to_owned(),
            code: "".to_owned(),
            flags: 0,
        },
    );
    assert_eq!(empty_code.status, ResponseStatus::InvalidRequest);
}
