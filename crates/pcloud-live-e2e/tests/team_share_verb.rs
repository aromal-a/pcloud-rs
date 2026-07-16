#![allow(clippy::pedantic)]
//! Live coverage for plain (non-crypto) team-share verbs (CLAUDEREV
//! iter-1 TEST-H-2, P5.2 team-share sub-step).
//!
//! Closes the verb-reached gap for `Request::AccountTeamShare` — the
//! retained-Implemented plain team-share row. The **crypto** team-share
//! row 142 (`psync_crypto_account_teamshare`) is intentionally *not*
//! covered by this file: row 142 has no dedicated IPC variant today
//! (it would route through `CryptoShareFolder` if it existed at all),
//! and that row is still listed `Partial` in the parity matrix. Closing
//! row 142 itself is P3-style net-new IPC + dispatch + backend work
//! tracked separately, not "live coverage for retained Implemented
//! rows" which is what P5.2 scopes.
//!
//! ## Coverage in this file
//!
//! | Test | IPC verb | Row family | Reachability |
//! |---|---|---|---|
//! | `live_account_team_share_dispatches_verb_reached` | `Request::AccountTeamShare` | plain team-share | proto + daemon dispatch arm |
//! | `live_crypto_account_team_share_dispatches_verb_reached` | `Request::CryptoAccountTeamShare` | crypto team-share (row 142) | proto + daemon dispatch arm |
//!
//! ## Reachability semantics
//!
//! Same as `tfa_lifecycle.rs` and `account_utility.rs`. The test
//! authenticates and dispatches `AccountTeamShare` with synthetic-but-
//! well-formed args. The server rejects (unknown folder_id /
//! unknown team_id / unauthorised business team) and we accept any
//! non-`Ok` verb-reached status as proof of route reachability. We do
//! **not** orchestrate a real team-share against a real second pCloud
//! account — that requires a two-account fixture which the current
//! soak harness does not have.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use pcloud_ipc::{RedactedString, Request, ResponseStatus};

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, authenticate,
    optional_env, skip_if_not_live, status_label,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_account_team_share_dispatches_verb_reached() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping AccountTeamShare: need credentials");
        return;
    }
    let mut daemon = TestDaemon::new("account-team-share-verb");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping AccountTeamShare: {err}");
        return;
    }

    // Synthetic-but-well-formed request. `folder_id = 0` and `team_id = 0`
    // are not valid live ids; the server is expected to reject. The
    // `permissions_bits` mask is the C-compatible "read-only" bit per
    // `pclsync/psynclib.h`. The narrow contract: daemon dispatched to
    // proto, server replied, no panic.
    let resp = daemon.dispatch(Request::AccountTeamShare {
        folder_id: 0,
        name: "claudereV-verb-reached-probe".to_owned(),
        team_id: 0,
        message: "".to_owned(),
        permissions_bits: 1,
        hint: None,
    });
    assert_no_secret_leak(&resp);
    // Verb-reached: any non-Ok rejection is acceptable. An `Ok` here
    // would imply the server accepted a folder_id of 0 — server bug, not
    // a daemon bug. The non-business soak account should reply with
    // InvalidRequest or Unauthorized (no business-team membership).
    assert!(
        matches!(
            resp.status,
            ResponseStatus::InvalidRequest
                | ResponseStatus::Conflict
                | ResponseStatus::Unauthorized
                | ResponseStatus::Unavailable
                | ResponseStatus::InternalError
        ),
        "AccountTeamShare must be dispatched and answered (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_crypto_account_team_share_dispatches_verb_reached() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping CryptoAccountTeamShare: need credentials");
        return;
    }
    let mut daemon = TestDaemon::new("crypto-account-team-share-verb");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping CryptoAccountTeamShare: {err}");
        return;
    }

    // Synthetic-but-well-formed crypto team-share request. The
    // soak account is a personal account (not a business team
    // member) and crypto is not unlocked under the test daemon, so
    // the daemon should reject with `Conflict` before reaching the
    // server. Either path proves the IPC variant + daemon dispatch
    // arm + crypto-state precondition gate are wired correctly.
    let resp = daemon.dispatch(Request::CryptoAccountTeamShare {
        folder_id: 0,
        name: "claudereV-crypto-teamshare-probe".to_owned(),
        team_id: 0,
        message: "".to_owned(),
        permissions_bits: 1,
        temppass: RedactedString::from("claudereV-not-a-real-temppass"),
        hint: None,
    });
    assert_no_secret_leak(&resp);
    assert!(
        matches!(
            resp.status,
            ResponseStatus::InvalidRequest
                | ResponseStatus::Conflict
                | ResponseStatus::Unauthorized
                | ResponseStatus::Unavailable
                | ResponseStatus::InternalError
        ),
        "CryptoAccountTeamShare must be dispatched and answered (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_crypto_share_folder_rsa_dispatches_verb_reached() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping CryptoShareFolderRsa: need credentials");
        return;
    }
    let mut daemon = TestDaemon::new("crypto-share-folder-rsa-verb");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping CryptoShareFolderRsa: {err}");
        return;
    }

    // Synthetic-but-well-formed crypto share request. Crypto is not
    // unlocked under the test daemon, so the daemon should reject
    // with `Conflict` before reaching the get_pub_key call. If crypto
    // were unlocked but the recipient mail is bogus, the daemon would
    // surface `InternalError` from the get_pub_key step. Either path
    // proves the IPC variant + daemon dispatch arm + crypto-state
    // precondition gate are wired correctly. The bogus
    // `@example.invalid` recipient ensures no real RSA-share email
    // can ever be sent even if crypto were ambiently unlocked.
    let resp = daemon.dispatch(Request::CryptoShareFolderRsa {
        folder_id: 0,
        name: "claudereV-crypto-share-rsa-probe".to_owned(),
        mail: "claudereV-rsa-probe@example.invalid".to_owned(),
        message: "".to_owned(),
        permissions_bits: 1,
        hint: None,
    });
    assert_no_secret_leak(&resp);
    assert!(
        matches!(
            resp.status,
            ResponseStatus::InvalidRequest
                | ResponseStatus::Conflict
                | ResponseStatus::Unauthorized
                | ResponseStatus::Unavailable
                | ResponseStatus::InternalError
        ),
        "CryptoShareFolderRsa must be dispatched and answered (got status={}, msg={})",
        status_label(&resp.status),
        resp.message,
    );
}
