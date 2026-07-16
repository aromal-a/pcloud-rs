#![allow(clippy::pedantic)]
//! Live A→B share lifecycle: account A creates a folder, invites account B,
//! account B accepts, both sides verify the share is active, account A then
//! revokes the share and deletes the folder.
//!
//! This is the cross-account proof referenced by the existing single-account
//! `shares.rs` header — it closes the "two-daemon cross-account handshake"
//! gap that file documented as deferred.
//!
//! # Required environment
//!
//! Master gate (one of):
//! - `PCLOUD_LIVE_E2E=1`
//! - `PCLOUD_LIVE=1`
//!
//! Account A (folder owner / inviter):
//! - `PCLOUD_LIVE_ACCOUNT_A_EMAIL`
//! - `PCLOUD_LIVE_ACCOUNT_A_PASSWORD`
//!
//! Account B (invitee / acceptor):
//! - `PCLOUD_LIVE_ACCOUNT_B_EMAIL`
//! - `PCLOUD_LIVE_ACCOUNT_B_PASSWORD`
//!
//! Optional:
//! - `PCLOUD_LIVE_REGION` — `eu` or `us`. Auto-discovered otherwise.
//!
//! # Running
//!
//! ```text
//! set -a; source .env; set +a
//! PCLOUD_LIVE_E2E=1 cargo test -p pcloud-live-e2e --test shares_a_to_b -- --ignored --nocapture
//! ```
//!
//! # Teardown invariant
//!
//! On both successful completion and panicked failure the test attempts to:
//! 1. revoke the active share on A's side (`RemoveShare`),
//! 2. delete the scratch folder recursively on A's side
//!    (`FolderDeleteByPath { recursive: true }`).
//!
//! If either step fails the test logs the leftover so an operator can clean
//! up by hand. The test never assumes the backend will GC.

#![forbid(unsafe_code)]

mod common;

use std::time::{Duration, SystemTime};

use pcloud_ipc::{Method, Request, ResponseStatus};

use crate::common::{
    TestDaemon, assert_no_secret_leak, optional_env, release_gate_enabled, status_label,
};

const ENV_A_EMAIL: &str = "PCLOUD_LIVE_ACCOUNT_A_EMAIL";
const ENV_A_PASSWORD: &str = "PCLOUD_LIVE_ACCOUNT_A_PASSWORD";
const ENV_B_EMAIL: &str = "PCLOUD_LIVE_ACCOUNT_B_EMAIL";
const ENV_B_PASSWORD: &str = "PCLOUD_LIVE_ACCOUNT_B_PASSWORD";

/// Master live gate: accept either the new `PCLOUD_LIVE` opt-in or the
/// legacy `PCLOUD_LIVE_E2E` form documented elsewhere in the harness.
fn live_gate_enabled() -> bool {
    matches!(
        optional_env("PCLOUD_LIVE").as_deref(),
        Some("1") | Some("true") | Some("yes")
    ) || matches!(
        optional_env("PCLOUD_LIVE_E2E").as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn unique_folder_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("live-e2e-share-a-to-b-{}-{nanos}", std::process::id())
}

/// Extracts the first numeric `folder_id=` / `folderid=` / `id=` token
/// from a daemon response message.
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

/// Extracts the share-request id from a `ShareFolder` response. The daemon
/// formats it as `sharerequestid=Some(NNNN)` (Debug of `Option<u64>`); we
/// handle both `Some(NNNN)` and bare `NNNN` so future format tweaks don't
/// silently break the extractor.
fn extract_share_request_id(msg: &str) -> Option<u64> {
    for marker in [
        "sharerequestid=",
        "share_request_id=",
        "request_id=",
        "srid=",
    ] {
        if let Some(off) = msg.find(marker) {
            let mut tail = &msg[off + marker.len()..];
            // Skip optional `Some(` wrapper.
            if let Some(rest) = tail.strip_prefix("Some(") {
                tail = rest;
            }
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

/// Parse the comma-separated id list out of a `list_share_requests`
/// response message, e.g. `"share_requests: direction=incoming, count=2, ids=[123, 456]"`.
fn parse_id_list(msg: &str) -> Vec<u64> {
    let Some(start) = msg.find("ids=[") else {
        return Vec::new();
    };
    let after = &msg[start + "ids=[".len()..];
    let Some(end) = after.find(']') else {
        return Vec::new();
    };
    after[..end]
        .split(',')
        .filter_map(|tok| tok.trim().parse::<u64>().ok())
        .collect()
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
fn live_share_a_to_b_full_lifecycle() {
    if !live_gate_enabled() {
        assert!(
            !release_gate_enabled(),
            "release share gate requires PCLOUD_LIVE_E2E=1"
        );
        eprintln!(
            "[live-e2e] skipping shares_a_to_b: neither PCLOUD_LIVE=1 nor PCLOUD_LIVE_E2E=1 is set"
        );
        return;
    }
    let Some(a_email) = optional_env(ENV_A_EMAIL) else {
        assert!(
            !release_gate_enabled(),
            "release share gate requires {ENV_A_EMAIL}"
        );
        eprintln!("[live-e2e] skipping shares_a_to_b: {ENV_A_EMAIL} unset");
        return;
    };
    let Some(a_password) = optional_env(ENV_A_PASSWORD) else {
        assert!(
            !release_gate_enabled(),
            "release share gate requires {ENV_A_PASSWORD}"
        );
        eprintln!("[live-e2e] skipping shares_a_to_b: {ENV_A_PASSWORD} unset");
        return;
    };
    let Some(b_email) = optional_env(ENV_B_EMAIL) else {
        assert!(
            !release_gate_enabled(),
            "release share gate requires {ENV_B_EMAIL}"
        );
        eprintln!("[live-e2e] skipping shares_a_to_b: {ENV_B_EMAIL} unset");
        return;
    };
    let Some(b_password) = optional_env(ENV_B_PASSWORD) else {
        assert!(
            !release_gate_enabled(),
            "release share gate requires {ENV_B_PASSWORD}"
        );
        eprintln!("[live-e2e] skipping shares_a_to_b: {ENV_B_PASSWORD} unset");
        return;
    };

    // Defensive: A and B must be distinct accounts. A self-share would
    // either be rejected by the backend or would silently auto-accept and
    // mask a bug in the cross-account handshake.
    assert_ne!(
        a_email.trim().to_ascii_lowercase(),
        b_email.trim().to_ascii_lowercase(),
        "A and B emails must differ"
    );

    // ─── Account A: log in and create the scratch folder ───────────────
    let mut daemon_a = TestDaemon::new("share-a-to-b-a");
    login_with(&mut daemon_a, &a_email, &a_password).expect("A login");

    // Userinfo probe so we fail fast if the session is half-built.
    let info = daemon_a.dispatch(Request::Plain {
        method: Method::GetUserInfo,
    });
    assert_no_secret_leak(&info);
    assert_eq!(
        info.status,
        ResponseStatus::Ok,
        "A userinfo probe failed: {}",
        info.message
    );

    let folder_name = unique_folder_name();
    let folder_path = format!("/{folder_name}");
    let create = daemon_a.dispatch(Request::CreateRemoteFolder {
        parent_folder_id: Some(0),
        name: folder_name.clone(),
        path: folder_path.clone(),
        check_and_create: true,
    });
    assert_no_secret_leak(&create);
    assert_eq!(
        create.status,
        ResponseStatus::Ok,
        "A CreateRemoteFolder failed: status={} message={}",
        status_label(&create.status),
        create.message
    );
    let folder_id = extract_folder_id(&create.message).unwrap_or_else(|| {
        panic!(
            "A CreateRemoteFolder response had no folder_id: {}",
            create.message
        )
    });

    // ─── Account A: send share invite to B ─────────────────────────────
    let share = daemon_a.dispatch(Request::ShareFolder {
        folder_id,
        name: folder_name.clone(),
        mail: b_email.clone(),
        message: "live-e2e A→B handshake".to_owned(),
        permissions_bits: 1, // read-only
        hint: None,
    });
    assert_no_secret_leak(&share);
    if share.status != ResponseStatus::Ok {
        // Best-effort folder cleanup before bailing.
        let _ = daemon_a.dispatch(Request::FolderDeleteById {
            folder_id,
            recursive: true,
        });
        panic!(
            "A ShareFolder failed: status={} message={}",
            status_label(&share.status),
            share.message
        );
    }
    eprintln!("[live-e2e] A ShareFolder ok: {}", share.message);
    let outgoing_request_id = extract_share_request_id(&share.message);

    // ─── Health-check pCloud's share-request endpoints on A's side ─────
    //
    // Two upstream findings (2026-04-29) make a full A→B handshake
    // currently unverifiable end-to-end via this CLI:
    //
    //   F1. `sharefolder` no longer echoes `sharerequestid` in the
    //       response hash. Parser at `pcloud-proto/src/shares_api.rs`
    //       returns `None` (we already see this in `outgoing_request_id`
    //       above).
    //
    //   F2. `listsharerequests` returns a truncated binary frame —
    //       `transport failed: i/o failed: failed to fill whole buffer`.
    //       Repros for BOTH the inviter and the invitee on the same
    //       transport pool, in the same session as a successful
    //       `listshares` call. Suspected upstream API change or a binary
    //       protocol incompatibility specific to this method.
    //
    // The send side (login, folder create, sharefolder, folder
    // cleanup) is fully verifiable; the accept side requires either F1
    // or F2 to be fixed. When both are broken we degrade to a
    // "send-only proven" pass and document the gap.
    let a_outgoing_pending = daemon_a.dispatch(Request::Plain {
        method: Method::ListOutgoingShareRequests,
    });
    let send_id_advertised = outgoing_request_id.is_some();
    let outgoing_endpoint_healthy = a_outgoing_pending.status == ResponseStatus::Ok;
    eprintln!(
        "[live-e2e] A ListOutgoingShareRequests: status={} message={}",
        status_label(&a_outgoing_pending.status),
        a_outgoing_pending.message
    );

    // If the F1 parser fix surfaced the request id, we can drive the
    // accept handshake on B's side WITHOUT calling the broken
    // `listsharerequests` (F2). That's the happy path now that F1 is
    // fixed; we still keep the F1+F2 degraded fallback for safety.
    if send_id_advertised {
        eprintln!(
            "[live-e2e] F1-fixed path: A advertised sharerequestid={:?}, \
             will drive B's accept directly",
            outgoing_request_id
        );
    } else if !send_id_advertised && !outgoing_endpoint_healthy {
        eprintln!(
            "[live-e2e] DEGRADED: send side proven (login + folder create + \
             sharefolder accepted by backend), accept-side handshake skipped \
             due to upstream findings F1+F2. See test header for details."
        );

        // Optional attended-flow opt-in: keep the folder + pending invite
        // intact so the operator can accept via email and then run
        // `shares_active_a_to_b` to verify bilateral visibility.
        let keep = !release_gate_enabled()
            && matches!(
                optional_env("PCLOUD_LIVE_KEEP_ARTIFACTS").as_deref(),
                Some("1") | Some("true") | Some("yes")
            );
        if keep {
            eprintln!(
                "[live-e2e] PCLOUD_LIVE_KEEP_ARTIFACTS set — leaving folder \
                 {folder_path} (id={folder_id}) and pending invite to {b_email} \
                 in place. Accept via the invitation email, then run \
                 `cargo test -p pcloud-live-e2e --test shares_active_a_to_b -- \
                 --ignored --nocapture` to verify bilateral visibility."
            );
            return;
        }

        // Best-effort cleanup of the orphan folder.
        let delete = daemon_a.dispatch(Request::FolderDeleteById {
            folder_id,
            recursive: true,
        });
        if delete.status != ResponseStatus::Ok {
            eprintln!(
                "[live-e2e] WARN A FolderDeleteById({folder_id}) declined: status={} message={}",
                status_label(&delete.status),
                delete.message
            );
        }
        assert!(
            !release_gate_enabled(),
            "release share gate requires a complete A-to-B accept/revoke lifecycle; \
             the share request id and outgoing-request endpoint were both unavailable"
        );
        return;
    }

    // Wrap the rest in a closure so a panic still hits teardown via the
    // catch_unwind below.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // ─── Account B: log in and accept the invite ───────────────────
        let mut daemon_b = TestDaemon::new("share-a-to-b-b");
        login_with(&mut daemon_b, &b_email, &b_password).expect("B login");

        // Probe B's session with userinfo before any share-specific call.
        // If the email-verification gate or a region-steering issue is
        // breaking B's transport, this surfaces as a clear `userinfo`
        // failure rather than a confusing `listsharerequests` EOF.
        let b_info = daemon_b.dispatch(Request::Plain {
            method: Method::GetUserInfo,
        });
        assert_no_secret_leak(&b_info);
        if b_info.status != ResponseStatus::Ok {
            panic!(
                "B userinfo probe failed (likely email not verified or region steering): \
                 status={} message={}",
                status_label(&b_info.status),
                b_info.message
            );
        }
        eprintln!("[live-e2e] B userinfo ok: {}", b_info.message);

        // Small grace for backend propagation between accounts. We don't
        // rely on this for correctness — the loop below polls.
        std::thread::sleep(Duration::from_millis(500));

        // Find B's pending request id. If A's response advertised one,
        // trust it; otherwise scan B's incoming list.
        // Prefer the id A's `sharefolder` response gave us — it's
        // authoritative and avoids B's broken `listsharerequests` path
        // (finding F2). When A didn't advertise an id (older backends),
        // fall back to polling B's incoming-requests list.
        let b_request_id = if let Some(id) = outgoing_request_id {
            eprintln!(
                "[live-e2e] using A-advertised sharerequestid={id} \
                 (skipping B listsharerequests poll due to F2)"
            );
            id
        } else {
            let mut surfaced: Option<u64> = None;
            let mut last_msg = String::new();
            let mut last_status = String::new();
            for attempt in 0..20 {
                let list = daemon_b.dispatch(Request::Plain {
                    method: Method::ListIncomingShareRequests,
                });
                assert_no_secret_leak(&list);
                last_msg = list.message.clone();
                last_status = status_label(&list.status).to_owned();
                if list.status == ResponseStatus::Ok {
                    let ids = parse_id_list(&list.message);
                    if !ids.is_empty() {
                        surfaced = ids.into_iter().max();
                        break;
                    }
                }
                if attempt < 19 {
                    std::thread::sleep(Duration::from_millis(1500));
                }
            }
            surfaced.unwrap_or_else(|| {
                panic!(
                    "B ListIncomingShareRequests never surfaced the pending invite \
                     after 30s. last_status={last_status} last_message={last_msg}"
                )
            })
        };

        let accept = daemon_b.dispatch(Request::AcceptShareRequest {
            share_request_id: b_request_id,
            to_folder_id: 0, // attach under B's root
            name: None,
        });
        assert_no_secret_leak(&accept);
        assert_eq!(
            accept.status,
            ResponseStatus::Ok,
            "B AcceptShareRequest failed: status={} message={}",
            status_label(&accept.status),
            accept.message
        );
        eprintln!("[live-e2e] B AcceptShareRequest ok: {}", accept.message);

        // pCloud propagates the accept across the share index
        // asynchronously; poll both sides until the active share
        // surfaces in `listshares`. Typical propagation in the live
        // backend is 1–5s; we allow up to 30s.
        let mut a_outgoing_ids: Vec<u64> = Vec::new();
        let mut b_incoming_ids: Vec<u64> = Vec::new();
        for attempt in 0..30 {
            let outgoing = daemon_a.dispatch(Request::Plain {
                method: Method::ListOutgoingShares,
            });
            assert_no_secret_leak(&outgoing);
            assert_eq!(
                outgoing.status,
                ResponseStatus::Ok,
                "A ListOutgoingShares failed: {}",
                outgoing.message
            );
            a_outgoing_ids = parse_id_list(&outgoing.message);

            let incoming = daemon_b.dispatch(Request::Plain {
                method: Method::ListIncomingShares,
            });
            assert_no_secret_leak(&incoming);
            assert_eq!(
                incoming.status,
                ResponseStatus::Ok,
                "B ListIncomingShares failed: {}",
                incoming.message
            );
            b_incoming_ids = parse_id_list(&incoming.message);

            if !a_outgoing_ids.is_empty() && !b_incoming_ids.is_empty() {
                eprintln!(
                    "[live-e2e] active share visible bilaterally after \
                     {}s: A outgoing ids={a_outgoing_ids:?}, B incoming ids={b_incoming_ids:?}",
                    attempt
                );
                break;
            }
            if attempt < 29 {
                std::thread::sleep(Duration::from_millis(1000));
            }
        }
        assert!(
            !a_outgoing_ids.is_empty(),
            "A ListOutgoingShares stayed empty for 30s after B accepted"
        );
        assert!(
            !b_incoming_ids.is_empty(),
            "B ListIncomingShares stayed empty for 30s after B accepted"
        );

        // Return the active share id A holds so teardown can revoke it.
        a_outgoing_ids.into_iter().max()
    }));

    // ─── Teardown (always runs) ─────────────────────────────────────────
    let mut revoke_ok = false;
    if let Ok(Some(active_share_id)) = result.as_ref() {
        let revoke = daemon_a.dispatch(Request::RemoveShare {
            share_id: *active_share_id,
        });
        assert_no_secret_leak(&revoke);
        revoke_ok = revoke.status == ResponseStatus::Ok;
        if revoke.status != ResponseStatus::Ok {
            eprintln!(
                "[live-e2e] WARN A RemoveShare({active_share_id}) declined: status={} message={}",
                status_label(&revoke.status),
                revoke.message
            );
        }
    }

    let delete = daemon_a.dispatch(Request::FolderDeleteById {
        folder_id,
        recursive: true,
    });
    assert_no_secret_leak(&delete);
    if delete.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] WARN A FolderDeleteById({folder_id}) declined: status={} message={}",
            status_label(&delete.status),
            delete.message
        );
    }

    if result.is_ok() && release_gate_enabled() {
        assert!(
            revoke_ok,
            "release share gate failed to revoke active share"
        );
        assert_eq!(
            delete.status,
            ResponseStatus::Ok,
            "release share gate failed to delete fixture folder: {}",
            delete.message
        );
    }

    // Re-raise any captured panic from the inner closure so the test fails.
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
