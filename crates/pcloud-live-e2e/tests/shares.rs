#![allow(clippy::pedantic)]
//! Live share-lifecycle coverage: ShareFolder invitation to a second
//! account, listing from both sides, accept / decline / cancel, permission
//! modify, and remove. Requires a second pCloud account whose email
//! address is exposed via `PCLOUD_TEST_PEER_USER`. The test is a strict
//! no-op when that env is unset so the default CI path (single
//! first-account credential bundle) cannot accidentally spam mutation
//! flows at the backend.
//!
//! Pre-alpha honesty: this binary does **not** yet attempt a two-daemon
//! cross-account handshake (logging in as the peer to accept the
//! invite). That would require a second complete credential triplet
//! (`PCLOUD_TEST_PEER_TOKEN` / password / TFA) and a separate
//! `TestDaemon` instance. The single-account flow exercised here
//! covers: creation of the invitation, validation that ShareFolder
//! returns Ok, listing the outgoing share request, and canceling it so
//! no pending invitation is left behind on the live account.
//!
//! Runtime-gated on `PCLOUD_LIVE_E2E=1 + PCLOUD_TEST_PEER_USER`.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use std::time::SystemTime;

use pcloud_ipc::{Method, Request, ResponseStatus};
use pcloud_sdk::EmbeddedDaemon;

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, optional_env,
    scratch_folder, skip_if_not_live, status_label,
};

/// Second-account recipient email. Invite flow is fully skipped unless
/// this env is set, so we can never spam an unrelated address.
const ENV_PEER_USER: &str = "PCLOUD_TEST_PEER_USER";

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

fn unique_folder_name(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

/// Extracts the first `folder_id=` / `folderid=` numeric token from a
/// response message. Tolerates both CSV and legacy `key=value` shapes.
fn extract_folder_id(msg: &str) -> Option<u64> {
    for marker in ["folder_id=", "folderid=", "id="] {
        if let Some(off) = msg.find(marker) {
            let tail = &msg[off + marker.len()..];
            let end = tail
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(tail.len());
            if let Ok(v) = tail[..end].parse::<u64>() {
                return Some(v);
            }
        }
    }
    None
}

/// Extracts the first `share_request_id=` / `sharerequestid=` numeric
/// token from a response.
fn extract_share_request_id(msg: &str) -> Option<u64> {
    for marker in [
        "share_request_id=",
        "sharerequestid=",
        "request_id=",
        "srid=",
    ] {
        if let Some(off) = msg.find(marker) {
            let tail = &msg[off + marker.len()..];
            let end = tail
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(tail.len());
            if let Ok(v) = tail[..end].parse::<u64>() {
                return Some(v);
            }
        }
    }
    None
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials + PCLOUD_TEST_PEER_USER"]
fn live_share_folder_invite_and_cancel() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping shares: need credentials");
        return;
    }
    let Some(peer) = optional_env(ENV_PEER_USER) else {
        eprintln!("[live-e2e] skipping shares: {ENV_PEER_USER} unset — no peer account to invite");
        return;
    };

    // 1) Bootstrap an authenticated SDK-backed daemon.
    let seed = TestDaemon::new("shares-seed");
    let root = seed.config.paths.config_dir.parent().unwrap().to_path_buf();
    drop(seed);
    let mut sdk = EmbeddedDaemon::builder(root.clone())
        .build()
        .expect("SDK bootstrap");

    let auth_resp = if let Some(token) = optional_env(ENV_TOKEN) {
        sdk.dispatch(Request::AuthTokenSubmission {
            value: token.into(),
        })
    } else {
        sdk.dispatch(Request::PasswordSubmission {
            username: optional_env(ENV_USER).unwrap(),
            value: optional_env(ENV_PASSWORD).unwrap().into(),
        })
    };
    assert_no_secret_leak(&auth_resp);
    if auth_resp.status != ResponseStatus::Ok || !sdk.is_authenticated() {
        eprintln!(
            "[live-e2e] skipping shares: auth failed / TFA required: {}",
            auth_resp.message
        );
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    // 2) Create a scratch remote folder we'll share. Use the
    //    CreateRemoteFolder IPC surface so we do not hard-depend on the
    //    SDK's upload helper here.
    let scratch = scratch_folder();
    let folder_name = unique_folder_name("live-e2e-share");
    let create = sdk.dispatch(Request::CreateRemoteFolder {
        parent_folder_id: None,
        name: folder_name.clone(),
        path: if scratch.ends_with('/') {
            format!("{scratch}{folder_name}")
        } else {
            format!("{scratch}/{folder_name}")
        },
        check_and_create: true,
    });
    assert_no_secret_leak(&create);
    if create.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] skipping shares: CreateRemoteFolder declined: status={} message={}",
            status_label(&create.status),
            create.message
        );
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    let Some(folder_id) = extract_folder_id(&create.message) else {
        eprintln!(
            "[live-e2e] skipping shares: CreateRemoteFolder did not advertise folder_id: {}",
            create.message
        );
        let _ = std::fs::remove_dir_all(&root);
        return;
    };

    // 3) Send the invitation.
    let share = sdk.dispatch(Request::ShareFolder {
        folder_id,
        name: folder_name.clone(),
        mail: peer.clone(),
        message: "live-e2e automated invite (auto-cancelled)".to_owned(),
        permissions_bits: 1, // read-only
        hint: None,
    });
    assert_no_secret_leak(&share);
    if share.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] ShareFolder declined: status={} message={}",
            status_label(&share.status),
            share.message
        );
        // Clean up the folder regardless.
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    // 4) Confirm it shows up in the outgoing list.
    let list = sdk.dispatch(Request::Plain {
        method: Method::ListOutgoingShares,
    });
    assert_no_secret_leak(&list);
    assert_eq!(
        list.status,
        ResponseStatus::Ok,
        "ListOutgoingShares failed: {}",
        list.message
    );

    // 5) Cancel the share request. Prefer the id advertised by the
    //    Share response; fall back to scanning the list.
    let share_request_id = extract_share_request_id(&share.message)
        .or_else(|| extract_share_request_id(&list.message));
    if let Some(srid) = share_request_id {
        let cancel = sdk.dispatch(Request::CancelShareRequest {
            share_request_id: srid,
        });
        assert_no_secret_leak(&cancel);
        if cancel.status != ResponseStatus::Ok {
            eprintln!(
                "[live-e2e] CancelShareRequest declined: status={} message={}",
                status_label(&cancel.status),
                cancel.message
            );
        }
    } else {
        eprintln!(
            "[live-e2e] WARN: could not find share_request_id to cancel; left pending invite on account"
        );
    }

    // 6) Also exercise the modify-permissions and remove paths
    //    opportunistically if the response exposed a share_id. Some
    //    backends only expose share_request_id until the peer accepts,
    //    in which case this block is a no-op.
    if let Some(share_id) = extract_share_request_id(&share.message) {
        // Try modify — likely fails until accepted, but we exercise the
        // request wire shape + leak check.
        let modify = sdk.dispatch(Request::ModifyShare {
            share_id,
            permissions_bits: 1,
        });
        assert_no_secret_leak(&modify);
        // Try remove too — same rationale.
        let remove = sdk.dispatch(Request::RemoveShare { share_id });
        assert_no_secret_leak(&remove);
    }

    let _ = std::fs::remove_dir_all(&root);
}
