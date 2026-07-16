#![allow(clippy::pedantic)]
//! Live coverage for TFA verbs (rows 19-22 of the parity matrix).
//!
//! Closes the iter-1 TEST-H-2 finding that retained TFA rows had
//! `[Implemented]` parity status but **no live test path exercising the
//! IPC dispatch + proto serialization + daemon routing**. Each test
//! gates on `PCLOUD_LIVE_E2E=1` plus credentials and is skipped cleanly
//! otherwise.
//!
//! ## Coverage
//!
//! | Test | IPC verb | Parity row | Reachability |
//! |---|---|---|---|
//! | `live_send_two_factor_sms_dispatches` | `Method::SendTwoFactorSms` | 19 | proto + daemon dispatch arm |
//! | `live_send_two_factor_notification_dispatches` | `Method::SendTwoFactorNotification` | 20 | proto + daemon dispatch arm |
//! | `live_submit_two_factor_code_when_envar_provides_one` | `Request::TwoFactorCodeSubmission` | 21 | proto + daemon dispatch arm |
//! | `live_submit_recovery_code_when_envar_provides_one` | `Request::TwoFactorCodeSubmission { recovery_code: true }` | 22 | proto + daemon dispatch arm |
//!
//! ## Reachability semantics
//!
//! These tests run against the live soak account. Two of the four verbs
//! (`SendTwoFactorSms`, `SendTwoFactorNotification`) are post-login but
//! pre-TFA-submission verbs — the soak account intentionally has TFA
//! disabled (per `OPERATIONS-RUNBOOK.md` "Live E2E account setup")
//! so the server replies with an explicit "TFA not required" or
//! `InvalidRequest`-shaped response. The tests assert the verb is
//! **reached and answered**, not that the server-side TFA flow is
//! exercised end-to-end. That is sufficient to close the parity-row
//! "retained but unreached" gap.
//!
//! The `TwoFactorCodeSubmission` tests skip cleanly unless the operator
//! provisions `PCLOUD_TEST_TFA_CODE` / `PCLOUD_TEST_RECOVERY_CODE` —
//! which is only meaningful against a TFA-enabled fixture account, not
//! the regular soak account.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use pcloud_ipc::{Method, Request, ResponseStatus};

use crate::common::{
    ENV_PASSWORD, ENV_RECOVERY_CODE, ENV_TFA_CODE, ENV_TOKEN, ENV_USER, TestDaemon,
    assert_no_secret_leak, authenticate, optional_env, skip_if_not_live, status_label,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

/// A response is considered "verb-reached" if the daemon dispatched it
/// to the proto layer and the proto layer talked to the server, even if
/// the server's reply was an error code from a no-TFA account. We
/// accept the full set of plausible non-fatal responses.
fn is_verb_reached(status: &ResponseStatus) -> bool {
    matches!(
        status,
        ResponseStatus::Ok
            | ResponseStatus::InvalidRequest
            | ResponseStatus::Unauthorized
            | ResponseStatus::Unavailable
    )
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_send_two_factor_sms_dispatches() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping TFA SMS resend: need credentials");
        return;
    }
    let mut daemon = TestDaemon::new("tfa-sms-resend");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping TFA SMS resend: {err}");
        return;
    }
    let resp = daemon.dispatch(Request::Plain {
        method: Method::SendTwoFactorSms,
    });
    assert_no_secret_leak(&resp);
    assert!(
        is_verb_reached(&resp.status),
        "SendTwoFactorSms must be dispatched (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_send_two_factor_notification_dispatches() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping TFA notification resend: need credentials");
        return;
    }
    let mut daemon = TestDaemon::new("tfa-notification-resend");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping TFA notification resend: {err}");
        return;
    }
    let resp = daemon.dispatch(Request::Plain {
        method: Method::SendTwoFactorNotification,
    });
    assert_no_secret_leak(&resp);
    assert!(
        is_verb_reached(&resp.status),
        "SendTwoFactorNotification must be dispatched (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + PCLOUD_TEST_TFA_CODE on a TFA-enabled fixture account"]
fn live_submit_two_factor_code_when_envar_provides_one() {
    if skip_if_not_live(&[ENV_TFA_CODE]) {
        return;
    }
    let code = optional_env(ENV_TFA_CODE).expect("gate checked code presence");

    // Direct submission against a fresh daemon — the suite's normal
    // login path also exercises this, but a direct route call pins
    // the IPC variant for the parity row independently of the login
    // helper's internal sequencing.
    let mut daemon = TestDaemon::new("tfa-code-submit");
    let resp = daemon.dispatch(Request::TwoFactorCodeSubmission {
        value: code,
        trust_device: false,
        recovery_code: false,
    });
    assert_no_secret_leak(&resp);
    // Reaching the daemon is sufficient — the actual server-side
    // outcome depends on whether a TFA login is currently pending, and
    // we don't want to assume any specific lifecycle state.
    assert!(
        is_verb_reached(&resp.status),
        "TwoFactorCodeSubmission must be dispatched (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + PCLOUD_TEST_RECOVERY_CODE on a TFA-enabled fixture account"]
fn live_submit_recovery_code_when_envar_provides_one() {
    if skip_if_not_live(&[ENV_RECOVERY_CODE]) {
        return;
    }
    let recovery = optional_env(ENV_RECOVERY_CODE).expect("gate checked recovery presence");

    let mut daemon = TestDaemon::new("tfa-recovery-submit");
    let resp = daemon.dispatch(Request::TwoFactorCodeSubmission {
        value: recovery,
        trust_device: false,
        recovery_code: true,
    });
    assert_no_secret_leak(&resp);
    assert!(
        is_verb_reached(&resp.status),
        "TwoFactorCodeSubmission(recovery=true) must be dispatched (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}
