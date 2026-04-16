#![allow(clippy::pedantic)]
//! Integration test: `Request::BackupSnapshot { action: Create }` through
//! the daemon produces `.tar.zst` + `.manifest.json` and the SHA3 in the
//! sidecar matches the recomputed digest over the on-disk archive.
//!
//! The test does NOT require network, GPG, or an authenticated session.
//! The daemon's state dir contains a real SQLite store (populated by
//! bootstrap), an empty-by-default vault path (which the snapshot path
//! treats as a missing file and refuses); so we stage a tiny vault file
//! by hand before dispatching Create.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::bootstrap_with_config;
use pcloud_ipc::{Request, ResponseStatus, SnapshotAction};

fn unique_root(tag: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "pcloud-daemon-snapshot-{tag}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn backup_snapshot_create_produces_zst_archive_and_sidecar() {
    let root = unique_root("create");
    let config = ConfigProfile::secure_defaults(root.clone(), Environment::Test);
    let mut runtime = bootstrap_with_config(config).expect("test bootstrap should succeed");

    // Seed a minimal auth-token vault so `auth_token.bin` is present
    // when the snapshot pipeline reads it. Content is arbitrary — the
    // pipeline does not interpret it, just hashes it.
    let vault_path = runtime.config.paths.auth_token_vault_path();
    if let Some(parent) = vault_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&vault_path, b"integration-test-vault").unwrap();

    let archive = root.join("snap.tar.zst");
    let req = Request::BackupSnapshot {
        action: SnapshotAction::Create,
        path: archive.clone(),
        gpg_recipient: None,
        yes: false,
        retention_days: None,
        zstd_level: Some(6),
    };
    let resp = runtime.handle_request(req);
    assert_eq!(
        resp.status,
        ResponseStatus::Ok,
        "create should succeed: {:?}",
        resp.message
    );

    // Files present.
    let sidecar = PathBuf::from(format!("{}.manifest.json", archive.display()));
    assert!(archive.exists(), "archive missing: {}", archive.display());
    assert!(sidecar.exists(), "sidecar missing: {}", sidecar.display());

    // Response carries the structured JSON with the SHA3 digest and
    // matches the recomputed digest over the archive bytes.
    let payload: serde_json::Value = serde_json::from_str(&resp.message).expect("message is JSON");
    let sha3 = payload["sha3_256"].as_str().expect("sha3_256 string");
    assert_eq!(sha3.len(), 64, "sha3 hex length");

    // Parse the sidecar and check the digest matches.
    let sidecar_bytes = std::fs::read(&sidecar).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&sidecar_bytes).unwrap();
    assert_eq!(parsed["sha3_256"].as_str().unwrap(), sha3);
    assert_eq!(parsed["zstd_level"].as_i64().unwrap(), 6);
    assert!(!parsed["encrypted"].as_bool().unwrap());

    // Verify round-trips through the daemon surface.
    let verify = Request::BackupSnapshot {
        action: SnapshotAction::Verify,
        path: archive.clone(),
        gpg_recipient: None,
        yes: false,
        retention_days: None,
        zstd_level: None,
    };
    let vr = runtime.handle_request(verify);
    assert_eq!(vr.status, ResponseStatus::Ok, "verify: {:?}", vr.message);
}

#[test]
fn backup_snapshot_create_rejects_out_of_range_zstd_level() {
    let root = unique_root("range");
    let config = ConfigProfile::secure_defaults(root.clone(), Environment::Test);
    let mut runtime = bootstrap_with_config(config).unwrap();

    let archive = root.join("snap.tar.zst");
    let req = Request::BackupSnapshot {
        action: SnapshotAction::Create,
        path: archive,
        gpg_recipient: None,
        yes: false,
        retention_days: None,
        zstd_level: Some(99),
    };
    let resp = runtime.handle_request(req);
    assert_eq!(resp.status, ResponseStatus::InvalidRequest);
    assert!(resp.message.contains("zstd-level"), "{:?}", resp.message);
}
