#![allow(clippy::pedantic)]
//! Live integrity-sweeper coverage: IntegrityStatus probe and
//! IntegrityRunOnce against a real sync root. We assert the payload
//! shape is legitimate and no mismatches are reported for a freshly
//! populated local tree.
//!
//! Runtime-gated on `PCLOUD_LIVE_E2E=1`.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use std::{fs, path::PathBuf, time::SystemTime};

use pcloud_ipc::{IntegrityStatusPayload, Method, Request, ResponseStatus};
use pcloud_model::sync::SyncType;

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, authenticate,
    optional_env, scratch_folder, skip_if_not_live,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

fn unique_local_root() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let p = std::env::temp_dir().join(format!(
        "pcloud-live-e2e-integ-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&p).expect("mkdir local sync root");
    // Seed with some deterministic content so the sweeper has something
    // to hash.
    for (i, name) in ["alpha.txt", "beta.bin", "gamma.log"].iter().enumerate() {
        let mut body = Vec::with_capacity(1024);
        body.extend(std::iter::repeat_n(b'a' + i as u8, 1024));
        fs::write(p.join(name), &body).expect("seed sync-root contents");
    }
    p
}

fn parse_status(message: &str) -> Option<IntegrityStatusPayload> {
    serde_json::from_str(message).ok()
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_integrity_status_and_run_once() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping integrity sweeper: need credentials");
        return;
    }

    let mut daemon = TestDaemon::new("integrity-sweeper");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping: {err}");
        return;
    }

    // Status probe before any action — must always be wired.
    let status = daemon.dispatch(Request::Plain {
        method: Method::IntegrityStatus,
    });
    assert_no_secret_leak(&status);
    assert_eq!(
        status.status,
        ResponseStatus::Ok,
        "IntegrityStatus should always be wired: {}",
        status.message
    );
    let pre = parse_status(&status.message)
        .expect("IntegrityStatus payload must be JSON IntegrityStatusPayload");

    // Register a sync root the sweeper can walk. If the backend refuses
    // the remote folder we still drill status/run-once against the
    // daemon surface (the sweeper may short-circuit on empty tree).
    let local = unique_local_root();
    let remote = scratch_folder();
    let add = daemon.dispatch(Request::SyncRootAdd {
        local_path: local.to_string_lossy().into_owned(),
        remote_path: remote.clone(),
        sync_type: Some(SyncType::UploadOnly),
    });
    assert_no_secret_leak(&add);
    // extract_sync_id parser mirrors tests/sync_roots.rs
    let sync_id = if add.status == ResponseStatus::Ok {
        add.message
            .find("sync_id=")
            .map(|i| &add.message[i + "sync_id=".len()..])
            .and_then(|t| {
                let end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
                t[..end].parse::<u64>().ok()
            })
    } else {
        None
    };

    // Trigger one sweeper cycle. The daemon may report the sweeper is
    // disabled (feature toggle): that's legitimate, and we treat it as
    // a soft skip.
    let ran = daemon.dispatch(Request::IntegrityRunOnce);
    assert_no_secret_leak(&ran);
    if ran.status == ResponseStatus::Ok {
        let payload = parse_status(&ran.message).expect("IntegrityRunOnce payload must be JSON");
        assert_eq!(
            payload.mismatches_found, 0,
            "no mismatches expected for freshly-seeded tree"
        );
        // Monotonicity: counters can't regress.
        assert!(
            payload.files_hashed >= pre.files_hashed,
            "files_hashed must be monotone non-decreasing ({} >= {})",
            payload.files_hashed,
            pre.files_hashed
        );
        assert!(
            payload.bytes_hashed >= pre.bytes_hashed,
            "bytes_hashed must be monotone non-decreasing"
        );
        assert!(
            payload.audit_drops == 0,
            "audit_drops must stay zero on the happy path, got {}",
            payload.audit_drops
        );
    } else {
        eprintln!(
            "[live-e2e] IntegrityRunOnce returned {}: {}",
            crate::common::status_label(&ran.status),
            ran.message
        );
    }

    // Cleanup — remove the sync root and the temp dir.
    if let Some(id) = sync_id {
        let _ = daemon.dispatch(Request::SyncRootRemove { sync_id: id });
    }
    let _ = fs::remove_dir_all(&local);
}
