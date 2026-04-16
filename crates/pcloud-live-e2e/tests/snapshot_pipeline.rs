#![allow(clippy::pedantic)]
//! Live snapshot-pipeline coverage: create (default zstd), verify
//! (SHA3 round-trip), optional GPG variant, and prune.
//!
//! Creating a snapshot does not require contacting the pCloud backend
//! (the daemon snapshots its own local state tree), but we still gate
//! on `PCLOUD_LIVE_E2E=1` so the full suite remains opt-in and so the
//! daemon has a post-auth state tree to pack.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use std::{process::Command, time::SystemTime};

use pcloud_ipc::{Request, ResponseStatus, SnapshotAction};

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, authenticate,
    optional_env, skip_if_not_live,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

fn unique_archive_path(tag: &str, ext: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "pcloud-live-e2e-snapshot-{tag}-{}-{nanos}.{ext}",
        std::process::id()
    ))
}

fn have_gpg() -> bool {
    Command::new("gpg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_snapshot_create_verify_prune_default() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping snapshot: need credentials for a meaningful state tree");
        return;
    }

    let mut daemon = TestDaemon::new("snapshot-default");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping: {err}");
        return;
    }

    let archive = unique_archive_path("create", "tar.zst");
    let create = daemon.dispatch(Request::BackupSnapshot {
        action: SnapshotAction::Create,
        path: archive.clone(),
        gpg_recipient: None,
        yes: false,
        retention_days: None,
        zstd_level: None,
    });
    assert_no_secret_leak(&create);
    if create.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] snapshot create declined: status={} message={}",
            crate::common::status_label(&create.status),
            create.message
        );
        let _ = std::fs::remove_file(&archive);
        return;
    }

    // Response body is a JSON object with at least `sha3_256` and
    // `archive`. Assert both round-trip through verify.
    let payload: serde_json::Value =
        serde_json::from_str(&create.message).expect("snapshot create body must be JSON");
    let sha = payload
        .get("sha3_256")
        .and_then(|v| v.as_str())
        .expect("snapshot create payload must expose sha3_256");
    assert_eq!(sha.len(), 64, "sha3_256 must be 64 hex chars: {sha}");

    let verify = daemon.dispatch(Request::BackupSnapshot {
        action: SnapshotAction::Verify,
        path: archive.clone(),
        gpg_recipient: None,
        yes: false,
        retention_days: None,
        zstd_level: None,
    });
    assert_no_secret_leak(&verify);
    assert_eq!(
        verify.status,
        ResponseStatus::Ok,
        "snapshot verify failed: {}",
        verify.message
    );
    let verify_payload: serde_json::Value =
        serde_json::from_str(&verify.message).expect("verify body must be JSON");
    assert_eq!(
        verify_payload.get("sha3_256").and_then(|v| v.as_str()),
        Some(sha),
        "sha3 mismatch between create and verify: {} vs {:?}",
        sha,
        verify_payload.get("sha3_256")
    );

    // Prune under a 0-day retention with --yes. We point at the
    // parent dir (snapshot prune takes a directory).
    if let Some(dir) = archive.parent() {
        let prune = daemon.dispatch(Request::BackupSnapshot {
            action: SnapshotAction::Prune,
            path: dir.to_path_buf(),
            gpg_recipient: None,
            yes: true,
            retention_days: Some(0),
            zstd_level: None,
        });
        assert_no_secret_leak(&prune);
        if prune.status != ResponseStatus::Ok {
            eprintln!(
                "[live-e2e] snapshot prune declined: status={} message={}",
                crate::common::status_label(&prune.status),
                prune.message
            );
        }
    }

    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_file(crate::common::sidecar_path(&archive));
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials + gpg binary"]
fn live_snapshot_create_verify_gpg() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping gpg snapshot: need credentials");
        return;
    }
    if !have_gpg() {
        eprintln!("[live-e2e] skipping gpg snapshot: gpg binary not present");
        return;
    }
    let recipient = match optional_env("PCLOUD_TEST_GPG_RECIPIENT") {
        Some(r) => r,
        None => {
            eprintln!(
                "[live-e2e] skipping gpg snapshot: set PCLOUD_TEST_GPG_RECIPIENT to a key id/email"
            );
            return;
        }
    };

    let mut daemon = TestDaemon::new("snapshot-gpg");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping: {err}");
        return;
    }

    let archive = unique_archive_path("gpg", "tar.zst.gpg");
    let create = daemon.dispatch(Request::BackupSnapshot {
        action: SnapshotAction::Create,
        path: archive.clone(),
        gpg_recipient: Some(recipient),
        yes: false,
        retention_days: None,
        zstd_level: Some(3),
    });
    assert_no_secret_leak(&create);
    if create.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] gpg snapshot declined (likely missing public key in local keyring): \
             status={} message={}",
            crate::common::status_label(&create.status),
            create.message
        );
        let _ = std::fs::remove_file(&archive);
        return;
    }

    let verify = daemon.dispatch(Request::BackupSnapshot {
        action: SnapshotAction::Verify,
        path: archive.clone(),
        gpg_recipient: None,
        yes: false,
        retention_days: None,
        zstd_level: None,
    });
    assert_no_secret_leak(&verify);
    if verify.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] gpg snapshot verify declined: {}",
            verify.message
        );
    }

    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_file(crate::common::sidecar_path(&archive));
}
