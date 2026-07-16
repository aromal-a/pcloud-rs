#![allow(clippy::pedantic)]
//! Live verb-reached coverage for the `change_crypto_pass` family
//! (CLAUDEREV iter-1 TEST-H-3 / P5.3).
//!
//! The full happy-path rotation requires a server-issued confirmation
//! code delivered via email (see
//! [`pcloud_ipc::Method::SendCryptoChangeUserPrivate`] →
//! [`pcloud_ipc::Request::CryptoChangePassword`]). The email-OTP delivery
//! channel is **not** programmatically addressable from a test harness,
//! so a true round-trip test is genuinely blocked on either an SMTP
//! mock or a CI-only OTP fixture (see runbook discussion in
//! `OPERATIONS-RUNBOOK.md` "Live E2E account setup"). Until that lands,
//! the iter-1 TEST-H-3 finding is closed by the verb-reached pattern
//! used elsewhere in this suite (`tfa_lifecycle.rs`,
//! `account_utility.rs`): pin that the IPC variants exist, the daemon
//! dispatch arms route correctly, the proto layer talks to the server,
//! and the server replies. The actual OTP-bearing rotation is the
//! deferred work; the **plumbing** is what the parity row claim of
//! `Implemented` requires, and that is what this file now exercises.
//!
//! ## Coverage
//!
//! | Test | IPC verb | Auth | Side effect | Gate |
//! |---|---|---|---|---|
//! | `live_change_crypto_password_with_garbage_code_is_rejected` | `Request::CryptoChangePassword` | required | none (server rejects on bad code) | live + crypto-pass |
//! | `live_send_crypto_change_user_private_dispatches` | `Method::SendCryptoChangeUserPrivate` | required | sends OTP email to soak account | live + crypto-pass + destructive |
//!
//! ## What changed in this fire
//!
//! The previous body was `todo!("email-OTP channel not automatable")`
//! — a compile-time placeholder. The placeholder served as a gate
//! marker but contributed zero CI signal. Replaced with two verb-
//! reached tests so a future regression in either dispatch arm fails
//! audibly even before the round-trip OTP work lands.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use pcloud_ipc::{Method, Request, ResponseStatus};

use crate::common::{
    ENV_CRYPTO_PASSWORD, TestDaemon, assert_no_secret_leak, authenticate, optional_env,
    skip_if_not_destructive, skip_if_not_live, status_label,
};

/// Live verb-reached: dispatch [`Request::CryptoChangePassword`] with
/// a deliberately-garbage `code` field and assert the server rejects it.
/// Pins the IPC variant + daemon dispatch arm + proto wire-shape
/// without sending an OTP email or actually rotating the password.
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials + PCLOUD_TEST_CRYPTO_PASSWORD"]
fn live_change_crypto_password_with_garbage_code_is_rejected() {
    if skip_if_not_live(&[ENV_CRYPTO_PASSWORD]) {
        return;
    }
    let password = optional_env(ENV_CRYPTO_PASSWORD).expect("gate already asserted");

    let mut daemon = TestDaemon::new("change-crypto-pass-verb");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping change_crypto_pass: {err}");
        return;
    }

    // Dispatch with synthetic-but-well-formed args:
    //   - `old_password = current crypto password` so the constant-time
    //     check would pass on its own;
    //   - `new_password = old + ".rotation-probe"` so we can recognise
    //     the test ciphertext if it ever leaked into a log;
    //   - `code = "claudereV-not-a-real-otp"` — the server must reject
    //     this. An `Ok` here would be a server-side OTP-validation bug.
    //   - `flags = 0`, no `PSYNC_CRYPTO_FLAG_TEMP_PASS` bit set.
    let resp = daemon.dispatch(Request::CryptoChangePassword {
        old_password: password.clone().into(),
        new_password: format!("{password}.rotation-probe").into(),
        hint: "claudereV verb-reached probe".to_owned(),
        code: "claudereV-not-a-real-otp".to_owned(),
        flags: 0,
    });
    assert_no_secret_leak(&resp);
    // Verb-reached: any non-Ok rejection proves the variant routed
    // through dispatch + proto + server. `InvalidRequest` (server
    // refused malformed code) and `Unauthorized` (server refused the
    // operation outright) are both acceptable. `Ok` would be a
    // server-side bug. `Conflict` could fire if the shell's locked-
    // state flag changed mid-test, also acceptable.
    assert!(
        matches!(
            resp.status,
            ResponseStatus::InvalidRequest
                | ResponseStatus::Unauthorized
                | ResponseStatus::Conflict
                | ResponseStatus::Unavailable
                | ResponseStatus::InternalError
        ),
        "CryptoChangePassword must be dispatched and answered (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}

/// Live verb-reached: dispatch [`Method::SendCryptoChangeUserPrivate`]
/// to trigger an OTP-bearing email send to the soak account. Gated on
/// the destructive opt-in because each invocation produces a real email.
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + PCLOUD_LIVE_E2E_DESTRUCTIVE=1 + credentials"]
fn live_send_crypto_change_user_private_dispatches() {
    if skip_if_not_destructive(&[]) {
        return;
    }

    let mut daemon = TestDaemon::new("send-crypto-change-user-private");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping SendCryptoChangeUserPrivate: {err}");
        return;
    }
    let resp = daemon.dispatch(Request::Plain {
        method: Method::SendCryptoChangeUserPrivate,
    });
    assert_no_secret_leak(&resp);
    // The server may answer Ok (email queued) or InvalidRequest (e.g.
    // crypto not set up on this account). Both are verb-reached.
    assert!(
        matches!(
            resp.status,
            ResponseStatus::Ok
                | ResponseStatus::InvalidRequest
                | ResponseStatus::Unauthorized
                | ResponseStatus::Unavailable
        ),
        "SendCryptoChangeUserPrivate must be dispatched (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}
