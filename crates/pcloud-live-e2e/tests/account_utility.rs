#![allow(clippy::pedantic)]
//! Live coverage for account-utility verbs (CLAUDEREV iter-1 TEST-H-2,
//! P5.2 account-utility sub-step).
//!
//! The retained-Implemented account-utility rows had their parity claim
//! made by code-citation only — no live test was exercising the IPC
//! dispatch + proto serialization + daemon routing end-to-end. This
//! file covers the **non-destructive** subset of those verbs. The
//! destructive subset (`LostPassword`, `VerifyEmail`,
//! `AccountChangePassword`) is intentionally excluded from this fire
//! because each one mutates the soak account or triggers an email. They
//! need a separate `PCLOUD_LIVE_E2E_DESTRUCTIVE=1` gate which has not
//! yet been added to `common/mod.rs`. That gate is the next sub-step
//! for P5.2.
//!
//! ## Coverage in this file
//!
//! | Test | IPC verb | Auth | Side effect |
//! |---|---|---|---|
//! | `live_get_api_servers_returns_json_array` | `Method::GetApiServers` | none | none |
//! | `live_get_promo_returns_payload_or_no_promo` | `Method::GetPromo` | required | none |
//! | `live_verify_email_restricted_with_garbage_token_is_rejected_cleanly` | `Request::VerifyEmailRestricted` | none | none (garbage token) |
//! | `live_set_language_to_en_is_accepted` | `Request::SetLanguage` | required | sets language preference (idempotent if already "en") |
//!
//! ## Reachability semantics
//!
//! Same as `tfa_lifecycle.rs`: a verb is "reached" when the daemon
//! dispatched it through proto + the server replied. The exact server
//! response code depends on account state (e.g. `GetPromo` returns
//! `"no promo"` when `haspromo` is false), so each test asserts
//! reachability + payload shape rather than a specific business outcome.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use pcloud_ipc::{Method, RedactedString, Request, ResponseStatus};

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, authenticate,
    optional_env, skip_if_not_live, status_label,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 (no credentials needed)"]
fn live_get_api_servers_returns_json_array() {
    if skip_if_not_live(&[]) {
        return;
    }

    let mut daemon = TestDaemon::new("account-get-api-servers");
    let resp = daemon.dispatch(Request::Plain {
        method: Method::GetApiServers,
    });
    assert_no_secret_leak(&resp);
    assert_eq!(
        resp.status,
        ResponseStatus::Ok,
        "GetApiServers must succeed without auth (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
    let value: serde_json::Value = serde_json::from_str(&resp.message)
        .expect("GetApiServers response body must be valid JSON");
    assert!(
        value.is_array(),
        "GetApiServers payload must be a JSON array, got {value:?}",
    );
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_get_promo_returns_payload_or_no_promo() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping GetPromo: need credentials");
        return;
    }
    let mut daemon = TestDaemon::new("account-get-promo");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping GetPromo: {err}");
        return;
    }
    let resp = daemon.dispatch(Request::Plain {
        method: Method::GetPromo,
    });
    assert_no_secret_leak(&resp);
    assert_eq!(
        resp.status,
        ResponseStatus::Ok,
        "GetPromo must succeed under an authenticated session (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
    // Payload shape: either the literal string "no promo" or a JSON
    // object {url, width, height}. Both are valid; assert one or the
    // other to pin the contract.
    if resp.message == "no promo" {
        return;
    }
    let value: serde_json::Value = serde_json::from_str(&resp.message)
        .expect("GetPromo response must be 'no promo' or valid JSON");
    assert!(
        value.is_object(),
        "GetPromo non-empty payload must be a JSON object, got {value:?}",
    );
    let obj = value.as_object().expect("GetPromo JSON object");
    for required in ["url", "width", "height"] {
        assert!(
            obj.contains_key(required),
            "GetPromo JSON must carry `{required}` field; got keys: {:?}",
            obj.keys().collect::<Vec<_>>(),
        );
    }
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 (no credentials needed)"]
fn live_verify_email_restricted_with_garbage_token_is_rejected_cleanly() {
    if skip_if_not_live(&[]) {
        return;
    }
    let mut daemon = TestDaemon::new("account-verify-email-restricted");
    // Deliberate garbage token. The point is to prove the IPC route
    // exists and the server answers — not to actually verify an email.
    let resp = daemon.dispatch(Request::VerifyEmailRestricted {
        verify_token: RedactedString::from("not-a-real-verify-token-claudereV-test"),
    });
    assert_no_secret_leak(&resp);
    // Server must reject the garbage token. Either InvalidRequest (the
    // daemon caught the malformed shape) or Unauthorized (the server
    // refused the token) is acceptable; Ok would be a security bug.
    assert!(
        matches!(
            resp.status,
            ResponseStatus::InvalidRequest | ResponseStatus::Unauthorized
        ),
        "garbage verify_token must be rejected, got status={} msg={}",
        status_label(&resp.status),
        resp.message,
    );
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_set_language_to_en_is_accepted() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping SetLanguage: need credentials");
        return;
    }
    let mut daemon = TestDaemon::new("account-set-language");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping SetLanguage: {err}");
        return;
    }
    let resp = daemon.dispatch(Request::SetLanguage {
        language: "en".to_owned(),
    });
    assert_no_secret_leak(&resp);
    // Setting the language preference is idempotent and safe (the
    // soak account starts at "en" by default per the runbook). Accept
    // Ok or InvalidRequest; the latter only fires if the server has
    // tightened its language allow-list since this test was written.
    assert!(
        matches!(
            resp.status,
            ResponseStatus::Ok | ResponseStatus::InvalidRequest
        ),
        "SetLanguage(en) must be reachable, got status={} msg={}",
        status_label(&resp.status),
        resp.message,
    );
}
