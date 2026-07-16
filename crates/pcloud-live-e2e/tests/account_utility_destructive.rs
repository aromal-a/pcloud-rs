#![allow(clippy::pedantic)]
//! Live coverage for **destructive** account-utility verbs (CLAUDEREV
//! iter-1 TEST-H-2, P5.2 destructive sub-step). Companion to
//! `account_utility.rs` (non-destructive verbs) and `tfa_lifecycle.rs`
//! (TFA verbs).
//!
//! ## What "destructive" means here
//!
//! Each test in this file either triggers an email send or mutates
//! authoritative server-side account state. Passing the master
//! `PCLOUD_LIVE_E2E=1` gate alone is **not** sufficient — operators
//! must additionally set `PCLOUD_LIVE_E2E_DESTRUCTIVE=1` (see the
//! `DESTRUCTIVE_GATE_ENV` constant in `common/mod.rs`). One exception:
//! `live_lost_password_for_invalid_domain_dispatches` targets the IETF-
//! reserved `@example.invalid` TLD, so the IPC verb is reached but no
//! mailbox can ever receive the reset link. That test runs under the
//! regular live gate.
//!
//! ## Coverage
//!
//! | Test | IPC verb | Parity row | Gate |
//! |---|---|---|---|
//! | `live_lost_password_for_invalid_domain_dispatches` | `Request::LostPassword` | account utility | live |
//! | `live_verify_email_dispatches_when_destructive_gate_enabled` | `Method::VerifyEmail` | account utility | destructive |
//! | `live_account_change_password_round_trip` | `Request::AccountChangePassword` | account utility | destructive |
//!
//! The `AccountChangePassword` round-trip rotates the soak account
//! `original → temp → original` with marker-file recovery so a flake
//! mid-test does not lock the account. See `common::AcpRotationMarker`
//! for the on-disk format and the round-trip's docstring for the
//! recovery semantics.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use std::time::SystemTime;

use pcloud_ipc::{Method, Request, ResponseStatus};

use crate::common::{
    AcpPhase, AcpRotationMarker, ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, acp_marker_path,
    assert_no_secret_leak, authenticate, delete_acp_marker, optional_env, read_acp_marker,
    skip_if_not_destructive, skip_if_not_live, status_label, write_acp_marker,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 (no credentials needed; @example.invalid never resolves)"]
fn live_lost_password_for_invalid_domain_dispatches() {
    if skip_if_not_live(&[]) {
        return;
    }
    let mut daemon = TestDaemon::new("account-lost-password");
    // IETF RFC 6761 reserves `.invalid` as a TLD that intentionally
    // does not resolve. Sending a "forgot password" reset to a
    // `@example.invalid` address pins the IPC route + proto serialisation
    // + daemon dispatch arm without delivering email to any real mailbox.
    let resp = daemon.dispatch(Request::LostPassword {
        email: "pcloud-rs-claudereV-test@example.invalid".to_owned(),
    });
    assert_no_secret_leak(&resp);
    // pCloud's policy on unknown / unresolvable emails is to accept the
    // verb (so the request leaks no information about which addresses
    // are registered) — we accept Ok | InvalidRequest. An Unauthorized
    // here would suggest the daemon is failing the request locally
    // before reaching the server, which is still verb-reached.
    assert!(
        matches!(
            resp.status,
            ResponseStatus::Ok | ResponseStatus::InvalidRequest | ResponseStatus::Unauthorized
        ),
        "LostPassword must be dispatched (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + PCLOUD_LIVE_E2E_DESTRUCTIVE=1 + credentials"]
fn live_verify_email_dispatches_when_destructive_gate_enabled() {
    if skip_if_not_destructive(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping VerifyEmail: need credentials");
        return;
    }
    let mut daemon = TestDaemon::new("account-verify-email");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping VerifyEmail: {err}");
        return;
    }
    // VerifyEmail triggers a fresh verification email send to the
    // authenticated account's address. The soak account is already
    // verified per the runbook setup, so the server's response is
    // either Ok (a redundant verification email is sent — harmless on
    // an already-verified account) or InvalidRequest (server refuses
    // the redundant send). Both are verb-reached.
    let resp = daemon.dispatch(Request::Plain {
        method: Method::VerifyEmail,
    });
    assert_no_secret_leak(&resp);
    assert!(
        matches!(
            resp.status,
            ResponseStatus::Ok | ResponseStatus::InvalidRequest
        ),
        "VerifyEmail must be dispatched (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}

/// Live `current → temp → original` round-trip for
/// `Request::AccountChangePassword` with marker-file recovery.
///
/// CLAUDEREV deferred-set D2.2 (fire 46). Closes the iter-1 TEST-H-2
/// `AccountChangePassword` sub-step that fires 26-30 deliberately
/// deferred for the marker-file design.
///
/// ## Flow
///
/// 1. **Recovery**: if a marker file exists from a prior crashed run,
///    authenticate with `temp` (the password the prior run rotated to)
///    and rotate back to `original`. Delete the marker. Test ends here.
/// 2. **Fresh path**: authenticate with `original` (env-supplied);
///    write marker at phase `RotatedToTemp`; dispatch
///    `AccountChangePassword{ current: original, new: temp }`; on
///    success, fresh-auth with `temp`; dispatch
///    `AccountChangePassword{ current: temp, new: original }`; on
///    success, delete marker; sanity-re-authenticate with original.
///
/// ## Marker semantics
///
/// `${TMPDIR}/pcloud-rs-acp-marker-${hash(email)}` (mode `0600`),
/// JSON-encoded `AcpRotationMarker`. Both passwords appear in
/// plaintext on disk for the duration of the test — same disclosure
/// surface as `PCLOUD_TEST_PASSWORD` in the env, no net-new exposure.
///
/// ## Crash safety
///
/// A panic between step 2's marker-write and step 4's marker-delete
/// leaves the marker behind with `phase = RotatedToTemp`. The next
/// invocation reads the marker, picks `temp` as current, rotates back,
/// and deletes the marker. The test then exits cleanly — the operator
/// re-runs to exercise the fresh path, OR can let the next scheduled
/// run handle it.
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + PCLOUD_LIVE_E2E_DESTRUCTIVE=1 + PCLOUD_TEST_USER + PCLOUD_TEST_PASSWORD"]
fn live_account_change_password_round_trip() {
    if skip_if_not_destructive(&[ENV_USER, ENV_PASSWORD]) {
        return;
    }
    let user = optional_env(ENV_USER).expect("gate checked user");
    let original = optional_env(ENV_PASSWORD).expect("gate checked password");

    let marker_path = acp_marker_path(&user);

    // ── Recovery branch ────────────────────────────────────────────
    if let Some(marker) = read_acp_marker(&marker_path) {
        if matches!(marker.phase, AcpPhase::RotatedToTemp) {
            eprintln!(
                "[live-e2e] recovering AccountChangePassword from a prior crashed run \
                 (marker={}); rotating temp -> original",
                marker_path.display()
            );
            let mut daemon = TestDaemon::new("acp-recovery");
            // The temp password is what the server currently expects.
            let resp = daemon.dispatch(Request::PasswordSubmission {
                username: user.clone(),
                value: marker.temp.clone().into(),
            });
            assert_no_secret_leak(&resp);
            assert_eq!(
                resp.status,
                ResponseStatus::Ok,
                "recovery: temp-password auth failed (account may be locked at neither \
                 original nor temp): status={} msg={}",
                status_label(&resp.status),
                resp.message,
            );
            // Rotate back to original.
            let resp = daemon.dispatch(Request::AccountChangePassword {
                current_password: marker.temp.clone().into(),
                new_password: marker.original.clone().into(),
            });
            assert_no_secret_leak(&resp);
            assert_eq!(
                resp.status,
                ResponseStatus::Ok,
                "recovery: rotate-back failed (account WILL stay at temp; manual \
                 intervention required): status={} msg={}",
                status_label(&resp.status),
                resp.message,
            );
            delete_acp_marker(&marker_path);
            return;
        }
    }

    // ── Fresh-path: original → temp → original ─────────────────────
    let nonce = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let temp = format!("claudereV-rotation-temp-{nonce}");

    let mut daemon = TestDaemon::new("acp-fresh-auth");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping AccountChangePassword round-trip: {err}");
        return;
    }

    // Persist the marker BEFORE dispatching the rotation. If the
    // RPC succeeds but the test process dies before deleting the
    // marker, the next run sees `phase = RotatedToTemp` and recovers.
    write_acp_marker(
        &marker_path,
        &AcpRotationMarker {
            original: original.clone(),
            temp: temp.clone(),
            phase: AcpPhase::RotatedToTemp,
        },
    )
    .expect("write rotation marker");

    let resp = daemon.dispatch(Request::AccountChangePassword {
        current_password: original.clone().into(),
        new_password: temp.clone().into(),
    });
    assert_no_secret_leak(&resp);
    assert_eq!(
        resp.status,
        ResponseStatus::Ok,
        "first rotation (original -> temp) failed: status={} msg={}",
        status_label(&resp.status),
        resp.message,
    );

    // Re-authenticate with the new password.
    let mut daemon2 = TestDaemon::new("acp-temp-auth");
    let resp = daemon2.dispatch(Request::PasswordSubmission {
        username: user.clone(),
        value: temp.clone().into(),
    });
    assert_no_secret_leak(&resp);
    assert_eq!(
        resp.status,
        ResponseStatus::Ok,
        "post-rotation temp-password auth failed: status={} msg={}",
        status_label(&resp.status),
        resp.message,
    );

    // Rotate back to original.
    let resp = daemon2.dispatch(Request::AccountChangePassword {
        current_password: temp.clone().into(),
        new_password: original.clone().into(),
    });
    assert_no_secret_leak(&resp);
    assert_eq!(
        resp.status,
        ResponseStatus::Ok,
        "second rotation (temp -> original) failed: status={} msg={}",
        status_label(&resp.status),
        resp.message,
    );

    delete_acp_marker(&marker_path);

    // Final sanity: re-authenticate with the original password.
    let mut daemon3 = TestDaemon::new("acp-final-auth");
    let resp = daemon3.dispatch(Request::PasswordSubmission {
        username: user,
        value: original.into(),
    });
    assert_no_secret_leak(&resp);
    assert_eq!(
        resp.status,
        ResponseStatus::Ok,
        "post-round-trip original-password auth failed (rotation was reported \
         OK but the server doesn't accept the original password): status={} msg={}",
        status_label(&resp.status),
        resp.message,
    );
}
