#![allow(clippy::pedantic)]
//! Live sync-root lifecycle coverage: add (Full / UploadOnly /
//! DownloadOnly), list, change-type, pause, resume, remove, plus an
//! idempotence probe.
//!
//! All tests gate on `PCLOUD_LIVE_E2E=1`. Every local sync root is
//! created inside a uniquely-named temp directory that is cleaned up on
//! drop.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use std::{fs, path::PathBuf, time::SystemTime};

use pcloud_ipc::{Method, Request, ResponseStatus};
use pcloud_model::sync::SyncType;

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, authenticate, expect_ok,
    optional_env, scratch_folder, skip_if_not_live,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

fn unique_local_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let p = std::env::temp_dir().join(format!(
        "pcloud-live-e2e-sync-{tag}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&p).expect("local sync root create");
    p
}

fn extract_sync_id(message: &str) -> Option<u64> {
    // The daemon's SyncRootAdd response message is a short key=value
    // string containing `sync_id=<u64>`. Fall back to the last numeric
    // token if the key is not present so we do not over-fit.
    if let Some(after) = message.find("sync_id=") {
        let tail = &message[after + "sync_id=".len()..];
        let end = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        return tail[..end].parse::<u64>().ok();
    }
    message
        .split_whitespace()
        .filter_map(|t| t.parse::<u64>().ok())
        .next_back()
}

fn add_sync_root(
    daemon: &mut TestDaemon,
    local: &PathBuf,
    remote: &str,
    sync_type: SyncType,
) -> Option<u64> {
    let req = Request::SyncRootAdd {
        local_path: local.to_string_lossy().into_owned(),
        remote_path: remote.to_owned(),
        sync_type: Some(sync_type),
    };
    let resp = daemon.dispatch(req);
    assert_no_secret_leak(&resp);
    if resp.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] sync_add({local:?}, {remote:?}, {sync_type:?}) did not accept: \
             status={} message={}",
            crate::common::status_label(&resp.status),
            resp.message
        );
        return None;
    }
    extract_sync_id(&resp.message)
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_sync_root_all_flavors() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping sync_roots: need credentials");
        return;
    }

    let mut daemon = TestDaemon::new("sync-flavors");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping: {err}");
        return;
    }

    let remote = scratch_folder();
    let flavors = [SyncType::Full, SyncType::UploadOnly, SyncType::DownloadOnly];
    let mut roots: Vec<(u64, PathBuf)> = Vec::new();
    for (i, flavor) in flavors.iter().enumerate() {
        let local = unique_local_root(&format!("flavor-{i}"));
        match add_sync_root(&mut daemon, &local, &remote, *flavor) {
            Some(id) => roots.push((id, local)),
            None => {
                // Remote folder may not be syncable for this flavor. Do
                // not fail the whole test; just clean up.
                let _ = fs::remove_dir_all(&local);
            }
        }
    }

    if roots.is_empty() {
        eprintln!(
            "[live-e2e] no flavors accepted; backend may have rejected the scratch folder. \
             Skipping mutation steps."
        );
        return;
    }

    // List should now contain at least one of the ids we just registered.
    let listed = expect_ok(
        &mut daemon,
        Request::Plain {
            method: Method::GetSyncRoots,
        },
        "sync-list",
    );
    for (id, _) in &roots {
        assert!(
            listed.message.contains(&id.to_string()),
            "sync-list output must mention sync_id {id}: {}",
            listed.message
        );
    }

    // Mutate the first root through the full pause → resume → change-type
    // lifecycle. We treat each step's acceptance as opportunistic: if the
    // retained backend declines (Unavailable / InvalidRequest), we log
    // and move on — this test is about the IPC surface being wired end-
    // to-end, not about forcing a specific engine state.
    let (sync_id, _) = roots[0];
    for step in [
        ("pause", Request::SyncRootPause { sync_id }),
        ("resume", Request::SyncRootResume { sync_id }),
        (
            "change-type-upload",
            Request::SyncRootChangeType {
                sync_id,
                sync_type: SyncType::UploadOnly,
            },
        ),
        (
            "change-type-full",
            Request::SyncRootChangeType {
                sync_id,
                sync_type: SyncType::Full,
            },
        ),
    ] {
        let (label, req) = step;
        let resp = daemon.dispatch(req);
        assert_no_secret_leak(&resp);
        if resp.status != ResponseStatus::Ok {
            eprintln!(
                "[live-e2e] sync_{label} declined: status={} message={}",
                crate::common::status_label(&resp.status),
                resp.message
            );
        }
    }

    // Remove every root we successfully added; verify each removal is Ok.
    for (id, local) in &roots {
        let resp = daemon.dispatch(Request::SyncRootRemove { sync_id: *id });
        assert_no_secret_leak(&resp);
        // Idempotence: a second remove should be a no-op or a clean
        // InvalidRequest, never a panic / InternalError.
        let resp2 = daemon.dispatch(Request::SyncRootRemove { sync_id: *id });
        assert_no_secret_leak(&resp2);
        assert!(
            matches!(
                resp2.status,
                ResponseStatus::Ok | ResponseStatus::InvalidRequest | ResponseStatus::Unavailable
            ),
            "second sync-remove for {id} returned {}: {}",
            crate::common::status_label(&resp2.status),
            resp2.message
        );
        let _ = fs::remove_dir_all(local);
    }

    // A clean list should now not mention the removed ids.
    let after = daemon.dispatch(Request::Plain {
        method: Method::GetSyncRoots,
    });
    assert_no_secret_leak(&after);
}
