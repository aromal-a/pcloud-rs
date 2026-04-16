#![allow(clippy::pedantic)]
//! Integration test: upload session lifecycle through the daemon IPC
//! dispatch (`Request::UploadCreate`, `UploadPause`, `UploadResume`,
//! `UploadCancel`, `UploadList`).
//!
//! Exercises the full path from IPC request through `RuntimeShell` to the
//! in-memory `SessionRegistry`, including:
//!
//! - create with each `ConflictMode` variant,
//! - `ConflictMode::Rename` deduplication (`"name (2).ext"` logic),
//! - pause / resume / cancel transitions,
//! - terminal-state rejection (Conflict response),
//! - list returns all sessions.
//!
//! Does NOT require network or an authenticated session — the registry is
//! purely in-memory.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::bootstrap_with_config;
use pcloud_ipc::{Request, ResponseStatus, UploadConflictMode};

fn unique_root(tag: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "pcloud-daemon-upload-sess-{tag}-{}-{nonce}",
        std::process::id()
    ))
}

fn bootstrap_runtime() -> pcloud_daemon::RuntimeShell {
    let root = unique_root("sess");
    let config = ConfigProfile::secure_defaults(root, Environment::Test);
    bootstrap_with_config(config).expect("test bootstrap should succeed")
}

/// Helper: extract `session_id` from a successful UploadCreate response.
fn extract_session_id(msg: &str) -> u64 {
    let v: serde_json::Value = serde_json::from_str(msg).expect("valid JSON");
    v["session_id"].as_u64().expect("session_id present")
}

// ── Create ──────────────────────────────────────────────────────────

#[test]
fn upload_create_returns_session_id_and_ok() {
    let mut rt = bootstrap_runtime();
    let resp = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/test.bin"),
        remote_name: "test.bin".to_owned(),
        parent_folder_id: Some(0),
        total_bytes: 1024,
        conflict_mode: None,
    });
    assert_eq!(resp.status, ResponseStatus::Ok, "{}", resp.message);
    let id = extract_session_id(&resp.message);
    assert!(id > 0, "session id should be positive");
}

#[test]
fn upload_create_rejects_empty_remote_name() {
    let mut rt = bootstrap_runtime();
    let resp = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/test.bin"),
        remote_name: "  ".to_owned(),
        parent_folder_id: Some(0),
        total_bytes: 1024,
        conflict_mode: None,
    });
    assert_eq!(resp.status, ResponseStatus::InvalidRequest);
}

#[test]
fn upload_create_with_each_conflict_mode() {
    let mut rt = bootstrap_runtime();
    for mode in [
        UploadConflictMode::Error,
        UploadConflictMode::Overwrite,
        UploadConflictMode::Skip,
        UploadConflictMode::Rename,
    ] {
        let resp = rt.handle_request(Request::UploadCreate {
            local_path: PathBuf::from("/tmp/cm.bin"),
            remote_name: format!("cm-{mode:?}.bin"),
            parent_folder_id: Some(0),
            total_bytes: 512,
            conflict_mode: Some(mode),
        });
        assert_eq!(
            resp.status,
            ResponseStatus::Ok,
            "mode {:?} should succeed: {}",
            mode,
            resp.message
        );
    }
}

// ── Rename deduplication ────────────────────────────────────────────

#[test]
fn upload_create_rename_deduplicates_remote_name() {
    let mut rt = bootstrap_runtime();

    // First upload: "report.pdf" should keep its name.
    let resp1 = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/a.bin"),
        remote_name: "report.pdf".to_owned(),
        parent_folder_id: Some(42),
        total_bytes: 100,
        conflict_mode: Some(UploadConflictMode::Rename),
    });
    assert_eq!(resp1.status, ResponseStatus::Ok);
    let v1: serde_json::Value = serde_json::from_str(&resp1.message).unwrap();
    assert_eq!(v1["remote_name"].as_str().unwrap(), "report.pdf");

    // Second upload with the same name + same parent → "report (2).pdf".
    let resp2 = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/b.bin"),
        remote_name: "report.pdf".to_owned(),
        parent_folder_id: Some(42),
        total_bytes: 200,
        conflict_mode: Some(UploadConflictMode::Rename),
    });
    assert_eq!(resp2.status, ResponseStatus::Ok);
    let v2: serde_json::Value = serde_json::from_str(&resp2.message).unwrap();
    assert_eq!(v2["remote_name"].as_str().unwrap(), "report (2).pdf");

    // Third: "report (3).pdf".
    let resp3 = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/c.bin"),
        remote_name: "report.pdf".to_owned(),
        parent_folder_id: Some(42),
        total_bytes: 300,
        conflict_mode: Some(UploadConflictMode::Rename),
    });
    assert_eq!(resp3.status, ResponseStatus::Ok);
    let v3: serde_json::Value = serde_json::from_str(&resp3.message).unwrap();
    assert_eq!(v3["remote_name"].as_str().unwrap(), "report (3).pdf");
}

// ── Pause / Resume / Cancel ─────────────────────────────────────────

#[test]
fn pause_resume_cycle() {
    let mut rt = bootstrap_runtime();
    let resp = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/pr.bin"),
        remote_name: "pr.bin".to_owned(),
        parent_folder_id: None,
        total_bytes: 2048,
        conflict_mode: None,
    });
    let id = extract_session_id(&resp.message);

    // Pause a Pending session.
    let pause_resp = rt.handle_request(Request::UploadPause { session_id: id });
    assert_eq!(
        pause_resp.status,
        ResponseStatus::Ok,
        "{}",
        pause_resp.message
    );

    // Resume from Paused.
    let resume_resp = rt.handle_request(Request::UploadResume { session_id: id });
    assert_eq!(
        resume_resp.status,
        ResponseStatus::Ok,
        "{}",
        resume_resp.message
    );
}

#[test]
fn cancel_from_pending() {
    let mut rt = bootstrap_runtime();
    let resp = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/cn.bin"),
        remote_name: "cn.bin".to_owned(),
        parent_folder_id: None,
        total_bytes: 512,
        conflict_mode: None,
    });
    let id = extract_session_id(&resp.message);

    let cancel_resp = rt.handle_request(Request::UploadCancel { session_id: id });
    assert_eq!(
        cancel_resp.status,
        ResponseStatus::Ok,
        "{}",
        cancel_resp.message
    );
}

#[test]
fn cancel_is_idempotent() {
    let mut rt = bootstrap_runtime();
    let resp = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/ci.bin"),
        remote_name: "ci.bin".to_owned(),
        parent_folder_id: None,
        total_bytes: 256,
        conflict_mode: None,
    });
    let id = extract_session_id(&resp.message);

    let r1 = rt.handle_request(Request::UploadCancel { session_id: id });
    assert_eq!(r1.status, ResponseStatus::Ok);

    // Second cancel is idempotent.
    let r2 = rt.handle_request(Request::UploadCancel { session_id: id });
    assert_eq!(r2.status, ResponseStatus::Ok);
}

#[test]
fn resume_rejects_non_paused_session() {
    let mut rt = bootstrap_runtime();
    let resp = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/rej.bin"),
        remote_name: "rej.bin".to_owned(),
        parent_folder_id: None,
        total_bytes: 128,
        conflict_mode: None,
    });
    let id = extract_session_id(&resp.message);

    // Pending → Resume should be rejected.
    let resume_resp = rt.handle_request(Request::UploadResume { session_id: id });
    assert_eq!(resume_resp.status, ResponseStatus::Conflict);
}

#[test]
fn terminal_state_rejects_pause() {
    let mut rt = bootstrap_runtime();
    let resp = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/term.bin"),
        remote_name: "term.bin".to_owned(),
        parent_folder_id: None,
        total_bytes: 64,
        conflict_mode: None,
    });
    let id = extract_session_id(&resp.message);

    // Cancel → terminal.
    let _ = rt.handle_request(Request::UploadCancel { session_id: id });

    // Pause on terminal → Conflict.
    let pause_resp = rt.handle_request(Request::UploadPause { session_id: id });
    assert_eq!(pause_resp.status, ResponseStatus::Conflict);
}

// ── Not found ───────────────────────────────────────────────────────

#[test]
fn pause_unknown_session_returns_invalid_request() {
    let mut rt = bootstrap_runtime();
    let resp = rt.handle_request(Request::UploadPause {
        session_id: 999_999,
    });
    assert_eq!(resp.status, ResponseStatus::InvalidRequest);
}

#[test]
fn resume_unknown_session_returns_invalid_request() {
    let mut rt = bootstrap_runtime();
    let resp = rt.handle_request(Request::UploadResume {
        session_id: 999_999,
    });
    assert_eq!(resp.status, ResponseStatus::InvalidRequest);
}

#[test]
fn cancel_unknown_session_returns_invalid_request() {
    let mut rt = bootstrap_runtime();
    let resp = rt.handle_request(Request::UploadCancel {
        session_id: 999_999,
    });
    assert_eq!(resp.status, ResponseStatus::InvalidRequest);
}

// ── List ────────────────────────────────────────────────────────────

#[test]
fn upload_list_returns_all_sessions() {
    let mut rt = bootstrap_runtime();

    // Start with zero sessions.
    let list0 = rt.handle_request(Request::UploadList);
    assert_eq!(list0.status, ResponseStatus::Ok);
    let arr0: Vec<serde_json::Value> = serde_json::from_str(&list0.message).unwrap();
    assert!(arr0.is_empty(), "fresh registry should be empty");

    // Create two sessions.
    let _ = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/l1.bin"),
        remote_name: "l1.bin".to_owned(),
        parent_folder_id: None,
        total_bytes: 100,
        conflict_mode: None,
    });
    let _ = rt.handle_request(Request::UploadCreate {
        local_path: PathBuf::from("/tmp/l2.bin"),
        remote_name: "l2.bin".to_owned(),
        parent_folder_id: None,
        total_bytes: 200,
        conflict_mode: None,
    });

    let list2 = rt.handle_request(Request::UploadList);
    assert_eq!(list2.status, ResponseStatus::Ok);
    let arr2: Vec<serde_json::Value> = serde_json::from_str(&list2.message).unwrap();
    assert_eq!(arr2.len(), 2, "should list both sessions");
}
