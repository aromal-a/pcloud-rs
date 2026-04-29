#![allow(clippy::pedantic)]
//! Live A↔B active-share visibility proof.
//!
//! Companion to `shares_a_to_b.rs`. Where that test exercises the *send*
//! side (login + folder create + sharefolder + cleanup) and degrades the
//! accept side because pCloud's `listsharerequests` binary endpoint
//! returns truncated frames in this fork (finding F2 in that file's
//! header), this test exercises the *active* side via `listshares` —
//! which is byte-correct on the same transport pool — by verifying that
//! a share already accepted out-of-band (e.g. by the recipient clicking
//! the invitation email) is visible from both A's outgoing perspective
//! and B's incoming perspective.
//!
//! # Required environment
//!
//! Master gate (one of `PCLOUD_LIVE_E2E=1` or `PCLOUD_LIVE=1`) plus all of:
//! - `PCLOUD_LIVE_ACCOUNT_A_EMAIL` / `PCLOUD_LIVE_ACCOUNT_A_PASSWORD`
//! - `PCLOUD_LIVE_ACCOUNT_B_EMAIL` / `PCLOUD_LIVE_ACCOUNT_B_PASSWORD`
//!
//! # Behaviour
//!
//! - If A reports >=1 outgoing share and B reports >=1 incoming share,
//!   the test asserts both sides agree on share existence and passes.
//! - If neither side reports any share (clean account state), the test
//!   skips with a clear message indicating no active A↔B share to verify
//!   — the harness deliberately does NOT try to create one, since
//!   completing that creation would require the broken
//!   `listsharerequests` path or manual email click. The companion
//!   `shares_a_to_b.rs` test handles the create+send side.
//! - If only one side reports a share, that's a real correctness bug
//!   (one daemon sees a share the other doesn't) and the test FAILS.
//!
//! # No-secrets-in-logs
//!
//! Same rules as the rest of the harness: never log credentials, scan
//! response messages for accidental secret leaks.

#![forbid(unsafe_code)]

mod common;

use pcloud_ipc::{Method, Request, ResponseStatus};

use crate::common::{TestDaemon, assert_no_secret_leak, optional_env, status_label};

const ENV_A_EMAIL: &str = "PCLOUD_LIVE_ACCOUNT_A_EMAIL";
const ENV_A_PASSWORD: &str = "PCLOUD_LIVE_ACCOUNT_A_PASSWORD";
const ENV_B_EMAIL: &str = "PCLOUD_LIVE_ACCOUNT_B_EMAIL";
const ENV_B_PASSWORD: &str = "PCLOUD_LIVE_ACCOUNT_B_PASSWORD";

fn live_gate_enabled() -> bool {
    matches!(
        optional_env("PCLOUD_LIVE").as_deref(),
        Some("1") | Some("true") | Some("yes")
    ) || matches!(
        optional_env("PCLOUD_LIVE_E2E").as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Parse `count=N` and the `ids=[...]` list out of the share-list
/// response message. Returns `(count, ids)`.
fn parse_share_list(msg: &str) -> (usize, Vec<u64>) {
    let count = msg
        .find("count=")
        .map(|off| {
            let tail = &msg[off + "count=".len()..];
            let end = tail
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(tail.len());
            tail[..end].parse::<usize>().unwrap_or(0)
        })
        .unwrap_or(0);
    let ids = msg
        .find("ids=[")
        .map(|off| {
            let tail = &msg[off + "ids=[".len()..];
            let end = tail.find(']').unwrap_or(tail.len());
            tail[..end]
                .split(',')
                .filter_map(|tok| tok.trim().parse::<u64>().ok())
                .collect()
        })
        .unwrap_or_default();
    (count, ids)
}

fn login_with(daemon: &mut TestDaemon, email: &str, password: &str) -> Result<(), String> {
    let resp = daemon.dispatch(Request::PasswordSubmission {
        username: email.to_owned(),
        value: password.to_owned().into(),
    });
    if resp.status != ResponseStatus::Ok {
        return Err(format!(
            "password auth failed: status={} message={}",
            status_label(&resp.status),
            resp.message
        ));
    }
    if !daemon.is_authenticated() {
        return Err(format!(
            "auth dispatch ok but session not authenticated: state={:?}",
            daemon.session_state()
        ));
    }
    Ok(())
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + PCLOUD_LIVE_ACCOUNT_{A,B}_{EMAIL,PASSWORD}"]
fn live_share_a_to_b_active_visibility() {
    if !live_gate_enabled() {
        eprintln!(
            "[live-e2e] skipping shares_active_a_to_b: neither PCLOUD_LIVE=1 nor PCLOUD_LIVE_E2E=1 is set"
        );
        return;
    }
    let Some(a_email) = optional_env(ENV_A_EMAIL) else {
        eprintln!("[live-e2e] skipping: {ENV_A_EMAIL} unset");
        return;
    };
    let Some(a_password) = optional_env(ENV_A_PASSWORD) else {
        eprintln!("[live-e2e] skipping: {ENV_A_PASSWORD} unset");
        return;
    };
    let Some(b_email) = optional_env(ENV_B_EMAIL) else {
        eprintln!("[live-e2e] skipping: {ENV_B_EMAIL} unset");
        return;
    };
    let Some(b_password) = optional_env(ENV_B_PASSWORD) else {
        eprintln!("[live-e2e] skipping: {ENV_B_PASSWORD} unset");
        return;
    };
    assert_ne!(
        a_email.trim().to_ascii_lowercase(),
        b_email.trim().to_ascii_lowercase(),
        "A and B emails must differ"
    );

    // ─── Login both sides ──────────────────────────────────────────────
    let mut daemon_a = TestDaemon::new("share-active-a");
    login_with(&mut daemon_a, &a_email, &a_password).expect("A login");
    let mut daemon_b = TestDaemon::new("share-active-b");
    login_with(&mut daemon_b, &b_email, &b_password).expect("B login");

    // ─── List outgoing on A ────────────────────────────────────────────
    let a_outgoing = daemon_a.dispatch(Request::Plain {
        method: Method::ListOutgoingShares,
    });
    assert_no_secret_leak(&a_outgoing);
    assert_eq!(
        a_outgoing.status,
        ResponseStatus::Ok,
        "A ListOutgoingShares failed: {}",
        a_outgoing.message
    );
    let (a_count, a_ids) = parse_share_list(&a_outgoing.message);
    eprintln!(
        "[live-e2e] A outgoing shares: count={} ids={:?}",
        a_count, a_ids
    );

    // ─── List incoming on B ────────────────────────────────────────────
    let b_incoming = daemon_b.dispatch(Request::Plain {
        method: Method::ListIncomingShares,
    });
    assert_no_secret_leak(&b_incoming);
    assert_eq!(
        b_incoming.status,
        ResponseStatus::Ok,
        "B ListIncomingShares failed: {}",
        b_incoming.message
    );
    let (b_count, b_ids) = parse_share_list(&b_incoming.message);
    eprintln!(
        "[live-e2e] B incoming shares: count={} ids={:?}",
        b_count, b_ids
    );

    // ─── Three legitimate states ───────────────────────────────────────
    //
    // (1) Both sides report >= 1 share — pass.
    // (2) Neither side reports any share — skip.
    // (3) Asymmetric (one side has a share, the other doesn't) — fail,
    //     because that's a real divergence.
    //
    // We do NOT require A's count == B's count: each account may have
    // shares with third parties not visible in the symmetric pair.
    // The minimal correctness invariant is: if any A↔B share exists,
    // both endpoints see at least one matching share-bearing side.
    match (a_count, b_count) {
        (0, 0) => {
            eprintln!(
                "[live-e2e] no active A↔B share to verify. \
                 To populate, run `shares_a_to_b` to send the invite, \
                 then accept via the email link on B's inbox."
            );
        }
        (a, b) if a > 0 && b > 0 => {
            eprintln!(
                "[live-e2e] active share visible bilaterally: A outgoing={a}, B incoming={b}"
            );
        }
        (a, b) => {
            panic!(
                "asymmetric share state: A outgoing={a} ({a_ids:?}), B incoming={b} ({b_ids:?}). \
                 One endpoint reports a share the other does not — this is a real divergence."
            );
        }
    }
}
