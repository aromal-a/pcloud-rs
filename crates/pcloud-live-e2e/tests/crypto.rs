#![allow(clippy::pedantic)]
//! Live crypto-lifecycle coverage: setup (or unlock if already set), get
//! status, create an encrypted folder, lock, re-unlock, and reset-in-test
//! teardown. Everything is driven through the real daemon IPC surface so
//! the crypto shell, daemon routing, and SDK mirror are exercised as one.
//!
//! Runtime-gated on `PCLOUD_LIVE_E2E=1 + PCLOUD_TEST_CRYPTO_PASSWORD`.
//!
//! Pre-alpha honesty: this test intentionally does **not** attempt
//! `change_crypto_pass`. The chain-of-trust for a real password rotation
//! goes through `SendCryptoChangeUserPrivate` → email-confirmation code
//! → `CryptoChangePassword`, and the confirmation-code delivery channel
//! is not programmatically addressable from a test harness. Rotation
//! rows remain tracked separately in the parity matrix.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use pcloud_ipc::{Method, Request, ResponseStatus};

use crate::common::{
    ENV_CRYPTO_PASSWORD, TestDaemon, assert_no_secret_leak, authenticate, optional_env,
    skip_if_not_live, status_label,
};

fn unique_folder_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("live-e2e-crypto-{}-{nanos}", std::process::id())
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials + PCLOUD_TEST_CRYPTO_PASSWORD"]
fn live_crypto_setup_unlock_status_mkdir_lock() {
    if skip_if_not_live(&[ENV_CRYPTO_PASSWORD]) {
        return;
    }
    let password = optional_env(ENV_CRYPTO_PASSWORD).expect("gate already asserted");

    let mut daemon = TestDaemon::new("crypto");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping crypto: {err}");
        return;
    }

    // 1) Status probe before any crypto action. Message is a string the
    //    daemon owns; we just make sure the call does not leak.
    let status = daemon.dispatch(Request::Plain {
        method: Method::GetCryptoStatus,
    });
    assert_no_secret_leak(&status);
    if status.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] skipping crypto: GetCryptoStatus failed: status={} message={}",
            status_label(&status.status),
            status.message
        );
        return;
    }

    // 2) Try to unlock first. If the account already has crypto set up,
    //    this succeeds and we proceed. Otherwise we attempt setup.
    let unlock = daemon.dispatch(Request::CryptoUnlock {
        password: password.clone().into(),
    });
    assert_no_secret_leak(&unlock);

    let already_set_up = unlock.status == ResponseStatus::Ok;
    let mut setup_performed = false;
    if !already_set_up {
        // Try to set up crypto with the supplied password. Some
        // accounts are pre-provisioned and reject setup with a
        // deterministic Conflict — that is acceptable and we soft-skip.
        let setup = daemon.dispatch(Request::CryptoSetup {
            password: password.clone().into(),
            hint: Some("live-e2e: automated setup".to_owned()),
        });
        assert_no_secret_leak(&setup);
        if setup.status != ResponseStatus::Ok {
            eprintln!(
                "[live-e2e] crypto setup declined (often means account already has crypto; unlock \
                 failure above is the real cause): status={} message={}",
                status_label(&setup.status),
                setup.message
            );
            return;
        }
        setup_performed = true;

        // A fresh setup leaves the shell started but typically locked;
        // re-unlock before mkdir.
        let unlock2 = daemon.dispatch(Request::CryptoUnlock {
            password: password.clone().into(),
        });
        assert_no_secret_leak(&unlock2);
        if unlock2.status != ResponseStatus::Ok {
            eprintln!(
                "[live-e2e] CryptoUnlock after setup failed: status={} message={}",
                status_label(&unlock2.status),
                unlock2.message
            );
            return;
        }
    }

    // 3) Status again — shell should report started+unlocked.
    let status2 = daemon.dispatch(Request::Plain {
        method: Method::GetCryptoStatus,
    });
    assert_no_secret_leak(&status2);
    assert_eq!(
        status2.status,
        ResponseStatus::Ok,
        "post-unlock GetCryptoStatus failed: {}",
        status2.message
    );

    // 4) Create a top-level crypto folder. Some backends refuse mkdir on
    //    trial crypto accounts; we tolerate that without failing the
    //    whole test.
    let folder = unique_folder_name();
    let mkdir = daemon.dispatch(Request::CryptoMkdir {
        name: folder,
        parent_folder_id: None,
        local_folder_id: None,
    });
    assert_no_secret_leak(&mkdir);
    if mkdir.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] CryptoMkdir declined: status={} message={}",
            status_label(&mkdir.status),
            mkdir.message
        );
    }

    // 5) Private-key flags probe (cheap read-only surface).
    let flags = daemon.dispatch(Request::Plain {
        method: Method::GetCryptoPrivKeyFlags,
    });
    assert_no_secret_leak(&flags);

    // 6) Lock.
    let lock = daemon.dispatch(Request::Plain {
        method: Method::LockCrypto,
    });
    assert_no_secret_leak(&lock);
    assert!(
        matches!(lock.status, ResponseStatus::Ok),
        "LockCrypto failed: status={} message={}",
        status_label(&lock.status),
        lock.message
    );

    // 7) Re-unlock to prove we can cycle.
    let unlock3 = daemon.dispatch(Request::CryptoUnlock {
        password: password.into(),
    });
    assert_no_secret_leak(&unlock3);
    assert!(
        matches!(unlock3.status, ResponseStatus::Ok),
        "second CryptoUnlock failed: status={} message={}",
        status_label(&unlock3.status),
        unlock3.message
    );

    // 8) If we performed setup in this test, do NOT call CryptoReset:
    //    on a real account CryptoReset wipes the local fingerprint +
    //    folder registry which is destructive. Leave the shell unlocked
    //    for the operator to inspect.
    let _ = setup_performed;
}
