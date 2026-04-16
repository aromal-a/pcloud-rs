#![warn(unsafe_op_in_unsafe_fn)]
// Daemon crate requires targeted unsafe for signal handlers and
// FUSE mount-runtime helpers.
//! # pcloud-daemon
//!
//! Composition root for the Rust pcloud-rs service: wires configuration,
//! store, auth vault, protocol clients, runtime shell, per-subsystem
//! backends, and the local IPC server; exposes bootstrap helpers and the
//! `RuntimeShell` that the SDK and CLI drive.
//!
//! **Architecture:** see `docs/book/src/architecture/crate-map.md` for the
//! overall crate graph. This crate sits at the top of the daemon runtime
//! and depends on `pcloud-backends`, `pcloud-ipc`, `pcloud-proto`,
//! `pcloud-fs`, `pcloud-store`, `pcloud-config`, and `pcloud-secret`.
//!
//! **Stability:** T1 internal — public API is not semver-stable across
//! workspace revisions; external consumers should go through `pcloud-sdk`.
//!
//! **MSRV:** Rust 1.82 (workspace-pinned; edition 2024).
//!
//! **Features:**
//! - `metrics` (off): enables Prometheus exporter via
//!   `pcloud-observability/prometheus-exporter` and populates the
//!   metrics snapshot inside `Method::Health`.
//! - `json-logs` (off): forwards structured JSON logging to
//!   `pcloud-observability/json-logs`.
//!
//! **Platform:** portable (Linux primary; FUSE-bound runtime on Linux).
//!
//! Not yet at full C parity — see `C_FEATURE_PARITY_MATRIX.csv` and open
//! tracker items `bd-1du.4`, `bd-1du.10`.

#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// PLAN_A_PLUS P6.1: backend implementations and their local helper
// modules live in `pcloud-backends`. They are re-exported here at their
// original paths so every existing `crate::<backend>::…` reference
// inside this crate and every external `pcloud_daemon::<backend>::…`
// reference in downstream crates keeps resolving unchanged.
// **PLATFORM:** all
// **GATING:** none (portable).

pub use pcloud_backends::account_backend;
pub use pcloud_backends::auth_backend;
pub mod audit_verifier_service;
pub mod auth_vault;
pub mod ha_lease;
pub mod vault;
pub use pcloud_backends::backup_backend;
pub mod bootstrap;
pub mod config_reload;
pub use pcloud_backends::crypto_backend;
pub mod dispatch;
pub use pcloud_backends::folder_backend;
pub use pcloud_backends::ignore_patterns;
#[cfg(feature = "metrics")]
pub mod metrics_server;
pub use pcloud_backends::mount_discovery;
pub mod mount_runtime;
pub use pcloud_backends::notifications_backend;
pub use pcloud_backends::path_resolver;
pub use pcloud_backends::public_link_backend;
pub mod rate_limit;
// P6.1 follow-up: `session_lifecycle` and `refresh_loop` were lifted
// into a dedicated `pcloud-session` crate. Re-export at their historical
// paths so every `pcloud_daemon::{session_lifecycle,refresh_loop}::…`
// reference in downstream crates keeps resolving unchanged. The
// `auth_vault` shim stays in-tree because it is tightly coupled to the
// daemon-owned `vault::file::*` subtree.
pub use pcloud_session::refresh_loop;
pub mod runtime;
pub mod serve;
pub mod session_refresh;
pub mod sync_loop;
pub mod sync_loop_runtime;
pub use pcloud_backends::shares_backend;
pub use pcloud_session::session_lifecycle;
pub mod signals;
pub use pcloud_backends::sync_backend;
pub use pcloud_backends::sync_suggest;
pub use pcloud_backends::transfer_backend;
pub mod transfer_bridge;
pub mod transport_factory;
pub use pcloud_backends::upload_journal;
pub use pcloud_backends::upload_state;

pub use bootstrap::{BootstrapError, bootstrap_shell, bootstrap_with_config};
pub use dispatch::dispatch;
pub use runtime::RuntimeShell;
#[cfg(feature = "metrics")]
pub use runtime::install_panic_metrics_hook;
pub use serve::{serve_until_shutdown, serve_until_shutdown_with_flag, serve_with_shutdown};

/// Canonical pidfile location for a given managed `state_dir`.
///
/// Used by both `pcloudd` (to write the pidfile on `serve`) and by
/// `pcloudc drain` (to look up the running daemon's pid). Kept public
/// so operator-facing tooling can report the exact on-disk path.
#[must_use]
pub fn daemon_pid_path(state_dir: &std::path::Path) -> std::path::PathBuf {
    state_dir.join("daemon.pid")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    use pcloud_auth::SessionState;
    use pcloud_config::{ConfigProfile, Environment};
    use pcloud_ipc::{IpcClient, IpcServer, Request, ResponseStatus};
    use pcloud_model::ids::RemoteFolderId;
    use pcloud_model::public_links::PublicLinkUploadPolicy;
    use pcloud_model::sync::SyncState;

    use crate::{bootstrap_with_config, dispatch};

    fn bootstrap_test_shell() -> crate::RuntimeShell {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("pcloud-daemon-test-{}-{nonce}", std::process::id()));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);
        bootstrap_with_config(config).expect("runtime bootstrap should succeed")
    }

    #[test]
    fn typed_ipc_roundtrip_reaches_runtime_dispatch() {
        let mut runtime = bootstrap_test_shell();
        let server = IpcServer::new(1000);
        let client = IpcClient;

        let response = client
            .roundtrip(
                &Request::Plain {
                    method: pcloud_ipc::Method::GetStatus,
                },
                |request_bytes| {
                    let request = server.decode_request(request_bytes)?;
                    let response = dispatch(&mut runtime, request);
                    server.encode_status(response.status, response.message)
                },
            )
            .expect("roundtrip should decode");

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("auth="));
    }

    #[test]
    fn password_submission_updates_auth_state() {
        let mut runtime = bootstrap_test_shell();
        let server = IpcServer::new(1000);
        let client = IpcClient;

        let begin_response = client
            .roundtrip(
                &Request::Plain {
                    method: pcloud_ipc::Method::LoginBegin,
                },
                |request_bytes| {
                    let request = server.decode_request(request_bytes)?;
                    let response = dispatch(&mut runtime, request);
                    server.encode_status(response.status, response.message)
                },
            )
            .expect("login begin should roundtrip");
        assert_eq!(begin_response.status, ResponseStatus::Ok);

        let password_response = client
            .roundtrip(
                &Request::PasswordSubmission {
                    username: "alice@example.com".to_owned(),
                    value: "correct-horse".to_owned(),
                },
                |request_bytes| {
                    let request = server.decode_request(request_bytes)?;
                    let response = dispatch(&mut runtime, request);
                    server.encode_status(response.status, response.message)
                },
            )
            .expect("password submission should roundtrip");

        assert_eq!(password_response.status, ResponseStatus::Ok);
        assert!(
            password_response
                .message
                .contains("TwoFactorChallengeIssued")
        );
        assert_eq!(
            runtime.auth.snapshot().state,
            SessionState::TwoFactorRequired
        );
    }

    #[test]
    fn auth_failure_response_and_status_preserve_backend_message() {
        let mut runtime = bootstrap_test_shell();

        let response = dispatch(
            &mut runtime,
            Request::PasswordSubmission {
                username: "mallory@example.com".to_owned(),
                value: "wrong-password".to_owned(),
            },
        );

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("invalid credentials"));
        assert_eq!(runtime.auth.snapshot().state, SessionState::AuthFailed);
        assert_eq!(
            runtime.auth.snapshot().last_auth_error.as_deref(),
            Some("invalid credentials")
        );

        let status = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::GetStatus,
            },
        );

        assert_eq!(status.status, ResponseStatus::Ok);
        assert!(
            status
                .message
                .contains("last_auth_error=Some(\"invalid credentials\")")
        );
    }

    #[test]
    fn pause_resume_and_shutdown_update_runtime_state() {
        let mut runtime = bootstrap_test_shell();

        let pause = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::PauseSync,
            },
        );
        assert_eq!(pause.status, ResponseStatus::Ok);
        assert_eq!(runtime.engine.sync_state, SyncState::Paused);

        let resume = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::ResumeSync,
            },
        );
        assert_eq!(resume.status, ResponseStatus::Ok);
        assert_eq!(runtime.engine.sync_state, SyncState::Steady);

        let shutdown = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::Shutdown,
            },
        );
        assert_eq!(shutdown.status, ResponseStatus::Ok);
        assert!(runtime.control.shutdown_requested);
        assert_eq!(runtime.store.repositories.audit.retained_event_count, 3);
        assert!(runtime.observability.summary().contains("emitted_events=3"));
    }

    #[test]
    fn pending_request_reports_engine_work_counts() {
        let mut runtime = bootstrap_test_shell();
        runtime
            .engine
            .ingest_candidates(&[pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(1),
                source: pcloud_model::sync::ChangeSource::Remote,
                path: "docs/report.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: Some(pcloud_model::ids::RemoteFileId::new(7)),
                remote_folder_id: None,
            }]);
        runtime.engine.advance_transfer_cycle();

        let response = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::GetPending,
            },
        );

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("pending: total="));
        assert!(response.message.contains("active_downloads=1"));
    }

    #[test]
    fn pause_request_surfaces_audit_persistence_failure() {
        let mut runtime = bootstrap_test_shell();
        let broken_path = std::env::temp_dir().join(format!(
            "pcloud-audit-dir-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&broken_path).expect("broken db dir should exist");
        runtime.store.db_path = broken_path;

        let response = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::PauseSync,
            },
        );

        assert_eq!(response.status, ResponseStatus::InternalError);
        assert!(response.message.contains("audit persistence failed"));
        assert_eq!(runtime.engine.sync_state, SyncState::Paused);
    }

    #[test]
    fn sync_root_add_list_remove_roundtrip_persists_state() {
        let mut runtime = bootstrap_test_shell();
        let local_sync = std::env::temp_dir().join(format!(
            "pcloud-sync-root-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&local_sync).expect("local sync dir should exist");

        let login = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(login.status, ResponseStatus::Ok);

        let add = dispatch(
            &mut runtime,
            Request::SyncRootAdd {
                local_path: local_sync.display().to_string(),
                remote_path: "/remote-sync".to_owned(),
                sync_type: None,
            },
        );
        assert_eq!(add.status, ResponseStatus::Ok);
        // Response payload is now structured JSON (ADR-0017). Assert
        // against the JSON field rather than the old human string.
        assert!(
            add.message.contains("\"remote_folder_id\":17"),
            "expected structured JSON remote_folder_id=17, got: {}",
            add.message
        );
        assert!(
            add.message.contains("\"sync_type\":\"Full\""),
            "expected default sync_type Full, got: {}",
            add.message
        );
        assert_eq!(
            runtime
                .store
                .repositories
                .sync_graph
                .tracked_sync_roots
                .len(),
            1
        );

        let list = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::GetSyncRoots,
            },
        );
        assert_eq!(list.status, ResponseStatus::Ok);
        assert!(list.message.contains("count=1"));
        assert!(list.message.contains(&local_sync.display().to_string()));

        let remove = dispatch(&mut runtime, Request::SyncRootRemove { sync_id: 1 });
        assert_eq!(remove.status, ResponseStatus::Ok);
        assert!(
            runtime
                .store
                .repositories
                .sync_graph
                .tracked_sync_roots
                .is_empty()
        );
    }

    #[test]
    fn sync_root_remove_evicts_runtime_engine_work() {
        let mut runtime = bootstrap_test_shell();
        let local_sync = std::env::temp_dir().join(format!(
            "pcloud-sync-root-evict-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&local_sync).expect("local sync dir should exist");

        let login = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(login.status, ResponseStatus::Ok);

        let add = dispatch(
            &mut runtime,
            Request::SyncRootAdd {
                local_path: local_sync.display().to_string(),
                remote_path: "/remote-sync".to_owned(),
                sync_type: None,
            },
        );
        assert_eq!(add.status, ResponseStatus::Ok);

        runtime.engine.ingest_candidates(&[
            pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(1),
                source: pcloud_model::sync::ChangeSource::Remote,
                path: "remote-sync/report.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: Some(pcloud_model::ids::RemoteFileId::new(99)),
                remote_folder_id: None,
            },
            pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(2),
                source: pcloud_model::sync::ChangeSource::Local,
                path: "other-sync/note.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: None,
                remote_folder_id: None,
            },
        ]);
        runtime.engine.advance_transfer_cycle();

        let remove = dispatch(&mut runtime, Request::SyncRootRemove { sync_id: 1 });
        assert_eq!(remove.status, ResponseStatus::Ok);
        assert!(
            runtime
                .engine
                .scheduler
                .queued_operations
                .iter()
                .all(|operation| operation.sync_id().get() != 1)
        );
        assert!(
            runtime
                .engine
                .downloads
                .active_downloads
                .iter()
                .all(|task| task.operation.sync_id().get() != 1)
        );
    }

    #[test]
    fn sync_root_add_requires_authenticated_session() {
        let mut runtime = bootstrap_test_shell();
        let local_sync = std::env::temp_dir().join(format!(
            "pcloud-sync-root-auth-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&local_sync).expect("local sync dir should exist");

        let add = dispatch(
            &mut runtime,
            Request::SyncRootAdd {
                local_path: local_sync.display().to_string(),
                remote_path: "/remote-sync".to_owned(),
                sync_type: None,
            },
        );

        assert_eq!(add.status, ResponseStatus::Conflict);
        assert!(add.message.contains("authenticated session"));
    }

    #[test]
    fn sync_root_add_rejects_missing_remote_folder() {
        let mut runtime = bootstrap_test_shell();
        let local_sync = std::env::temp_dir().join(format!(
            "pcloud-sync-root-remote-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&local_sync).expect("local sync dir should exist");
        let login = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(login.status, ResponseStatus::Ok);

        let add = dispatch(
            &mut runtime,
            Request::SyncRootAdd {
                local_path: local_sync.display().to_string(),
                remote_path: "/missing-remote".to_owned(),
                sync_type: None,
            },
        );

        assert_eq!(add.status, ResponseStatus::Conflict);
        assert!(add.message.contains("remote sync root validation failed"));
        assert!(
            runtime
                .store
                .repositories
                .sync_graph
                .tracked_sync_roots
                .is_empty()
        );
    }

    #[test]
    fn sync_root_add_rejects_duplicate_and_nested_local_paths() {
        let mut runtime = bootstrap_test_shell();
        let base = std::env::temp_dir().join(format!(
            "pcloud-sync-root-conflict-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        let nested = base.join("nested");
        fs::create_dir_all(&nested).expect("nested sync dir should exist");
        let login = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(login.status, ResponseStatus::Ok);

        let first_add = dispatch(
            &mut runtime,
            Request::SyncRootAdd {
                local_path: base.display().to_string(),
                remote_path: "/remote-sync".to_owned(),
                sync_type: None,
            },
        );
        assert_eq!(first_add.status, ResponseStatus::Ok);

        let duplicate = dispatch(
            &mut runtime,
            Request::SyncRootAdd {
                local_path: base.display().to_string(),
                remote_path: "/remote-sync".to_owned(),
                sync_type: None,
            },
        );
        assert_eq!(duplicate.status, ResponseStatus::Conflict);
        assert!(duplicate.message.contains("already tracked"));

        let nested_add = dispatch(
            &mut runtime,
            Request::SyncRootAdd {
                local_path: nested.display().to_string(),
                remote_path: "/remote-sync".to_owned(),
                sync_type: None,
            },
        );
        assert_eq!(nested_add.status, ResponseStatus::Conflict);
        assert!(nested_add.message.contains("inside an already tracked"));
    }

    #[test]
    fn create_change_list_show_and_delete_public_links_use_public_link_runtime() {
        let mut runtime = bootstrap_test_shell();

        let login = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(login.status, ResponseStatus::Ok);

        let create_file = dispatch(
            &mut runtime,
            Request::CreateFilePublicLink {
                path: "/Docs/report.txt".to_owned(),
            },
        );
        assert_eq!(create_file.status, ResponseStatus::Ok);
        // The daemon emits a JSON payload (documented in the manpage
        // Recipe 3 as `.message | fromjson | .code`). Assertions now
        // parse the JSON so they stay robust if field order changes.
        let json_file: serde_json::Value = serde_json::from_str(&create_file.message)
            .expect("create-file-link message must be JSON");
        assert_eq!(json_file["id"], 71);
        assert_eq!(json_file["is_folder"], false);
        assert!(json_file["link"].is_string());

        let create_folder = dispatch(
            &mut runtime,
            Request::CreateFolderPublicLink {
                path: "/Docs".to_owned(),
            },
        );
        assert_eq!(create_folder.status, ResponseStatus::Ok);
        let json_folder: serde_json::Value = serde_json::from_str(&create_folder.message)
            .expect("create-folder-link message must be JSON");
        assert_eq!(json_folder["id"], 81);
        assert_eq!(json_folder["is_folder"], true);

        let change_expire = dispatch(
            &mut runtime,
            Request::ChangePublicLinkExpire {
                link_id: 7,
                expire: Some(1_700_000_000),
            },
        );
        assert_eq!(change_expire.status, ResponseStatus::Ok);
        assert!(
            change_expire
                .message
                .contains("public link expire updated: id=7, expire=1700000000")
        );

        let clear_expire = dispatch(
            &mut runtime,
            Request::ChangePublicLinkExpire {
                link_id: 7,
                expire: None,
            },
        );
        assert_eq!(clear_expire.status, ResponseStatus::Ok);
        assert!(
            clear_expire
                .message
                .contains("public link expire cleared: id=7")
        );

        let change_password = dispatch(
            &mut runtime,
            Request::ChangePublicLinkPassword {
                link_id: 7,
                password: Some("new-secret".to_owned()),
            },
        );
        assert_eq!(change_password.status, ResponseStatus::Ok);
        assert!(
            change_password
                .message
                .contains("public link password updated: id=7")
        );
        assert!(!change_password.message.contains("new-secret"));

        let clear_password = dispatch(
            &mut runtime,
            Request::ChangePublicLinkPassword {
                link_id: 7,
                password: None,
            },
        );
        assert_eq!(clear_password.status, ResponseStatus::Ok);
        assert!(
            clear_password
                .message
                .contains("public link password cleared: id=7")
        );

        let enable_upload = dispatch(
            &mut runtime,
            Request::ChangePublicLinkUpload {
                link_id: 7,
                policy: PublicLinkUploadPolicy::Everyone,
            },
        );
        assert_eq!(enable_upload.status, ResponseStatus::Ok);
        assert!(
            enable_upload
                .message
                .contains("public link upload policy updated: id=7, policy=Everyone")
        );

        let disable_upload = dispatch(
            &mut runtime,
            Request::ChangePublicLinkUpload {
                link_id: 7,
                policy: PublicLinkUploadPolicy::Disabled,
            },
        );
        assert_eq!(disable_upload.status, ResponseStatus::Ok);
        assert!(
            disable_upload
                .message
                .contains("public link upload policy updated: id=7, policy=Disabled")
        );

        let list = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::ListPublicLinks,
            },
        );
        assert_eq!(list.status, ResponseStatus::Ok);
        let list_json: serde_json::Value =
            serde_json::from_str(&list.message).expect("list-links message must be JSON");
        assert_eq!(list_json["count"], 2);
        let links = list_json["links"]
            .as_array()
            .expect("links must be a JSON array");
        assert_eq!(links.len(), 2);
        assert!(
            links.iter().any(|l| l["code"] == "alpha123"),
            "expected alpha123 in {links:?}"
        );

        let show = dispatch(
            &mut runtime,
            Request::ShowPublicLink {
                code: "alpha123".to_owned(),
            },
        );
        assert_eq!(show.status, ResponseStatus::Ok);
        assert!(
            show.message
                .contains("public link contents: code=\"alpha123\"")
        );
        assert!(show.message.contains("alpha123-docs"));

        let delete = dispatch(&mut runtime, Request::DeletePublicLink { link_id: 7 });
        assert_eq!(delete.status, ResponseStatus::Ok);
        assert!(delete.message.contains("public link deleted: id=7"));

        let missing = dispatch(&mut runtime, Request::DeletePublicLink { link_id: 404 });
        assert_eq!(missing.status, ResponseStatus::Conflict);
        assert!(missing.message.contains("public link not found"));

        let invalid_change = dispatch(
            &mut runtime,
            Request::ChangePublicLinkExpire {
                link_id: 404,
                expire: Some(123),
            },
        );
        assert_eq!(invalid_change.status, ResponseStatus::Conflict);
        assert!(invalid_change.message.contains("invalid link"));

        let invalid_password = dispatch(
            &mut runtime,
            Request::ChangePublicLinkPassword {
                link_id: 405,
                password: Some("bad".to_owned()),
            },
        );
        assert_eq!(invalid_password.status, ResponseStatus::Conflict);
        assert!(invalid_password.message.contains("invalid password"));

        let invalid_upload = dispatch(
            &mut runtime,
            Request::ChangePublicLinkUpload {
                link_id: 406,
                policy: PublicLinkUploadPolicy::ChosenUsers,
            },
        );
        assert_eq!(invalid_upload.status, ResponseStatus::Conflict);
        assert!(invalid_upload.message.contains("invalid upload policy"));
    }

    #[test]
    fn run_localscan_signals_engine_wake_counter() {
        let mut runtime = bootstrap_test_shell();
        let before = runtime.engine.localscan_wakes;
        let resp = dispatch(&mut runtime, Request::RunLocalScan);
        assert_eq!(resp.status, ResponseStatus::Ok);
        assert!(resp.message.contains("local scan wake signalled"));
        assert_eq!(runtime.engine.localscan_wakes, before + 1);
        // Second wake bumps the counter again.
        let _ = dispatch(&mut runtime, Request::RunLocalScan);
        assert_eq!(runtime.engine.localscan_wakes, before + 2);
    }

    #[test]
    fn send_publink_dispatches_through_public_link_runtime() {
        let mut runtime = bootstrap_test_shell();
        let unauthenticated = dispatch(
            &mut runtime,
            Request::SendPublink {
                code: "alpha123".to_owned(),
                mails: "alice@example.com".to_owned(),
                message: "hi".to_owned(),
            },
        );
        assert_eq!(unauthenticated.status, ResponseStatus::Conflict);

        let login = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(login.status, ResponseStatus::Ok);

        let empty_code = dispatch(
            &mut runtime,
            Request::SendPublink {
                code: "   ".to_owned(),
                mails: "alice@example.com".to_owned(),
                message: "hi".to_owned(),
            },
        );
        assert_eq!(empty_code.status, ResponseStatus::InvalidRequest);

        let empty_mails = dispatch(
            &mut runtime,
            Request::SendPublink {
                code: "alpha123".to_owned(),
                mails: "   ".to_owned(),
                message: "hi".to_owned(),
            },
        );
        assert_eq!(empty_mails.status, ResponseStatus::InvalidRequest);

        let ok = dispatch(
            &mut runtime,
            Request::SendPublink {
                code: "alpha123".to_owned(),
                mails: "alice@example.com,bob@example.com".to_owned(),
                message: "Here is the link".to_owned(),
            },
        );
        assert_eq!(ok.status, ResponseStatus::Ok);
        assert!(ok.message.contains("public link sent"));
        assert!(ok.message.contains("alpha123"));
        // Audit/response message must report recipient count but never
        // the raw email addresses (PII-stripping invariant).
        assert!(ok.message.contains("recipients=2"));
        assert!(!ok.message.contains("alice@example.com"));
        assert!(!ok.message.contains("bob@example.com"));

        let bad_email = dispatch(
            &mut runtime,
            Request::SendPublink {
                code: "alpha123".to_owned(),
                mails: "not-an-email".to_owned(),
                message: "hi".to_owned(),
            },
        );
        assert_eq!(bad_email.status, ResponseStatus::Conflict);
        assert!(bad_email.message.contains("invalid email"));
    }

    #[test]
    fn create_list_and_delete_upload_links_use_public_link_runtime() {
        let mut runtime = bootstrap_test_shell();

        let login = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(login.status, ResponseStatus::Ok);

        let create = dispatch(
            &mut runtime,
            Request::CreateUploadLink {
                path: "/incoming".to_owned(),
                comment: "Drop files here".to_owned(),
                expire: Some(123),
                maxspace: Some(2048),
                maxfiles: Some(5),
            },
        );
        assert_eq!(create.status, ResponseStatus::Ok);
        // JSON payload (Recipe 3 / 9 in the manpage).
        let json_create: serde_json::Value =
            serde_json::from_str(&create.message).expect("create-upload-link message must be JSON");
        assert_eq!(json_create["id"], 171);
        assert_eq!(json_create["is_folder"], true);
        assert!(json_create["link"].is_string());

        let list = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::ListUploadLinks,
            },
        );
        assert_eq!(list.status, ResponseStatus::Ok);
        assert!(list.message.contains("upload links: count=1"));
        assert!(list.message.contains("Drop files here"));

        let delete = dispatch(
            &mut runtime,
            Request::DeleteUploadLink { upload_link_id: 17 },
        );
        assert_eq!(delete.status, ResponseStatus::Ok);
        assert!(delete.message.contains("upload link deleted: id=17"));

        let delete_missing = dispatch(
            &mut runtime,
            Request::DeleteUploadLink {
                upload_link_id: 404,
            },
        );
        assert_eq!(delete_missing.status, ResponseStatus::Conflict);
        assert!(delete_missing.message.contains("upload link not found"));
    }

    #[test]
    fn create_tree_public_link_uses_public_link_runtime() {
        let mut runtime = bootstrap_test_shell();

        let login = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(login.status, ResponseStatus::Ok);

        let create = dispatch(
            &mut runtime,
            Request::CreateTreePublicLink {
                name: "Quarterly Docs".to_owned(),
                root_folder_id: Some(9),
                folder_ids_csv: Some("9,10".to_owned()),
                file_ids_csv: Some("11,12".to_owned()),
                expire: Some(123),
                maxdownloads: Some(7),
                maxtraffic: Some(2048),
            },
        );
        assert_eq!(create.status, ResponseStatus::Ok);
        assert!(
            create
                .message
                .contains("tree public link created: id=271, name=\"Quarterly Docs\"")
        );

        let invalid = dispatch(
            &mut runtime,
            Request::CreateTreePublicLink {
                name: "Quarterly Docs".to_owned(),
                root_folder_id: None,
                folder_ids_csv: None,
                file_ids_csv: None,
                expire: None,
                maxdownloads: None,
                maxtraffic: None,
            },
        );
        assert_eq!(invalid.status, ResponseStatus::InvalidRequest);
        assert!(
            invalid
                .message
                .contains("tree link requires at least one target id")
        );
    }

    #[test]
    fn public_link_access_helpers_use_public_link_runtime() {
        let mut runtime = bootstrap_test_shell();

        let login = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(login.status, ResponseStatus::Ok);

        let list = dispatch(&mut runtime, Request::ListPublicLinkAccess { link_id: 7 });
        assert_eq!(list.status, ResponseStatus::Ok);
        assert!(
            list.message
                .contains("public link access: link_id=7, count=2")
        );
        assert!(list.message.contains("alice@example.com"));

        let add = dispatch(
            &mut runtime,
            Request::AddPublicLinkAccess {
                link_id: 7,
                email: "alice@example.com".to_owned(),
            },
        );
        assert_eq!(add.status, ResponseStatus::Ok);
        assert!(add.message.contains("public link access granted"));

        let invalid_email = dispatch(
            &mut runtime,
            Request::AddPublicLinkAccess {
                link_id: 7,
                email: "not-an-email".to_owned(),
            },
        );
        assert_eq!(invalid_email.status, ResponseStatus::Conflict);
        assert!(invalid_email.message.contains("invalid email"));

        let remove = dispatch(
            &mut runtime,
            Request::RemovePublicLinkAccess {
                link_id: 7,
                receiver_id: 33,
            },
        );
        assert_eq!(remove.status, ResponseStatus::Ok);
        assert!(remove.message.contains("public link access removed"));

        let missing = dispatch(
            &mut runtime,
            Request::RemovePublicLinkAccess {
                link_id: 7,
                receiver_id: 404,
            },
        );
        assert_eq!(missing.status, ResponseStatus::Conflict);
        assert!(missing.message.contains("receiver not found"));
    }

    #[test]
    fn bookmark_helpers_use_public_link_runtime() {
        let mut runtime = bootstrap_test_shell();

        let login = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(login.status, ResponseStatus::Ok);

        let list = dispatch(&mut runtime, Request::ListBookmarks);
        assert_eq!(list.status, ResponseStatus::Ok);
        assert!(list.message.contains("bookmarks: count=2"));
        assert!(list.message.contains("alpha123"));

        let change = dispatch(
            &mut runtime,
            Request::ChangeBookmark {
                code: "alpha123".to_owned(),
                location_id: 8,
                name: "Renamed Pin".to_owned(),
                description: "Updated".to_owned(),
            },
        );
        assert_eq!(change.status, ResponseStatus::Ok);
        assert!(change.message.contains("bookmark changed"));

        let missing = dispatch(
            &mut runtime,
            Request::RemoveBookmark {
                code: "alpha123".to_owned(),
                location_id: 404,
            },
        );
        assert_eq!(missing.status, ResponseStatus::Conflict);
        assert!(missing.message.contains("bookmark not found"));

        let remove = dispatch(
            &mut runtime,
            Request::RemoveBookmark {
                code: "alpha123".to_owned(),
                location_id: 8,
            },
        );
        assert_eq!(remove.status, ResponseStatus::Ok);
        assert!(remove.message.contains("bookmark removed"));
    }

    #[test]
    fn lock_crypto_request_is_idempotent_and_preserves_state() {
        // Before setup the shell is NotSetup; `lock()` must remain a no-op there
        // (lock_crypto never weakens the state machine). After setup+start we
        // are Unlocked; lock_crypto must transition back to Locked.
        let mut runtime = bootstrap_test_shell();

        // Idempotent pre-setup: NotSetup stays NotSetup.
        let response = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::LockCrypto,
            },
        );
        assert_eq!(response.status, ResponseStatus::Ok);
        assert_eq!(
            runtime.crypto.unlock_state,
            pcloud_crypto::state::UnlockState::NotSetup
        );

        // Setup + start brings us to Unlocked.
        let setup = dispatch(
            &mut runtime,
            Request::CryptoSetup {
                password: "topsecret".to_owned(),
                hint: None,
            },
        );
        assert_eq!(setup.status, ResponseStatus::Ok);
        let start = dispatch(
            &mut runtime,
            Request::CryptoUnlock {
                password: "topsecret".to_owned(),
            },
        );
        assert_eq!(start.status, ResponseStatus::Ok);
        assert_eq!(
            runtime.crypto.unlock_state,
            pcloud_crypto::state::UnlockState::Unlocked
        );

        // Locking from Unlocked transitions to Locked.
        let lock = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::LockCrypto,
            },
        );
        assert_eq!(lock.status, ResponseStatus::Ok);
        assert_eq!(
            runtime.crypto.unlock_state,
            pcloud_crypto::state::UnlockState::Locked
        );

        // Calling lock again is idempotent.
        let lock2 = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::LockCrypto,
            },
        );
        assert_eq!(lock2.status, ResponseStatus::Ok);
        assert_eq!(
            runtime.crypto.unlock_state,
            pcloud_crypto::state::UnlockState::Locked
        );
    }

    #[test]
    fn serve_loop_exits_after_shutdown_request() {
        let mut runtime = bootstrap_test_shell();
        let socket_path = runtime.config.paths.ipc_socket_path();
        let server = IpcServer::new(pcloud_ipc::current_effective_uid());
        let bound = server.bind(&socket_path).expect("socket should bind");

        // Rendezvous: ensure the serve thread has entered the loop before
        // the client issues the shutdown request. The listening socket is
        // already bound (backlog absorbs queued connects), so the barrier
        // is purely about removing scheduler-dependent timing that a
        // `thread::sleep` would otherwise leave up to the OS.
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let thread_barrier = std::sync::Arc::clone(&barrier);

        let handle = std::thread::spawn(move || {
            thread_barrier.wait();
            crate::serve_until_shutdown(&bound, &mut runtime)
                .expect("serve loop should exit cleanly");
            runtime.control.shutdown_requested
        });

        barrier.wait();

        let client = IpcClient;
        let response = client
            .send(
                &socket_path,
                &Request::Plain {
                    method: pcloud_ipc::Method::Shutdown,
                },
            )
            .expect("shutdown request should succeed");

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(handle.join().expect("serve loop thread should join"));
    }

    #[test]
    fn crypto_setup_start_mkdir_cycle_is_active() {
        // Agent B enabled the real crypto path. This test exercises the full
        // setup -> start -> mkdir -> lock cycle through the IPC request
        // dispatcher and verifies the active state machine. Empty passwords
        // and locked mkdir attempts are still rejected defensively.
        let mut runtime = bootstrap_test_shell();

        // Empty password is rejected early on setup.
        let empty_setup = dispatch(
            &mut runtime,
            Request::CryptoSetup {
                password: String::new(),
                hint: None,
            },
        );
        assert_eq!(empty_setup.status, ResponseStatus::InvalidRequest);

        // Empty password is rejected early on unlock/start too.
        let empty_unlock = dispatch(
            &mut runtime,
            Request::CryptoUnlock {
                password: String::new(),
            },
        );
        assert_eq!(empty_unlock.status, ResponseStatus::InvalidRequest);

        // mkdir while locked fails with Unauthorized.
        let mkdir_locked = dispatch(
            &mut runtime,
            Request::CryptoMkdir {
                name: "protected".to_owned(),
                parent_folder_id: None,
                local_folder_id: None,
            },
        );
        assert_eq!(mkdir_locked.status, ResponseStatus::Unauthorized);

        // Real setup succeeds.
        let setup = dispatch(
            &mut runtime,
            Request::CryptoSetup {
                password: "topsecret".to_owned(),
                hint: Some("hint".to_owned()),
            },
        );
        assert_eq!(setup.status, ResponseStatus::Ok);

        // start (via CryptoUnlock when already setup) succeeds.
        let start = dispatch(
            &mut runtime,
            Request::CryptoUnlock {
                password: "topsecret".to_owned(),
            },
        );
        assert_eq!(start.status, ResponseStatus::Ok);
        assert_eq!(
            runtime.crypto.unlock_state,
            pcloud_crypto::state::UnlockState::Unlocked
        );

        // mkdir now succeeds.
        let mkdir = dispatch(
            &mut runtime,
            Request::CryptoMkdir {
                name: "protected".to_owned(),
                parent_folder_id: None,
                local_folder_id: None,
            },
        );
        assert_eq!(mkdir.status, ResponseStatus::Ok);

        // Wrong password after lock is rejected with Unauthorized.
        let lock = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::LockCrypto,
            },
        );
        assert_eq!(lock.status, ResponseStatus::Ok);
        let wrong = dispatch(
            &mut runtime,
            Request::CryptoUnlock {
                password: "wrong".to_owned(),
            },
        );
        assert_eq!(wrong.status, ResponseStatus::Unauthorized);
    }

    #[test]
    fn submit_two_factor_code_completes_protocol_auth_flow() {
        let mut runtime = bootstrap_test_shell();

        let _ = dispatch(
            &mut runtime,
            Request::PasswordSubmission {
                username: "alice@example.com".to_owned(),
                value: "correct-horse".to_owned(),
            },
        );
        let response = dispatch(
            &mut runtime,
            Request::TwoFactorCodeSubmission {
                value: "654321".to_owned(),
                trust_device: false,
                recovery_code: false,
            },
        );

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("LoginSucceeded"));
        assert_eq!(runtime.auth.snapshot().state, SessionState::Authenticated);
        assert_eq!(
            runtime
                .auth
                .snapshot()
                .authenticated_user
                .map(|id| id.get()),
            Some(42)
        );
        assert_eq!(
            runtime.auth.snapshot().email.as_deref(),
            Some("alice@example.com")
        );
        assert!(runtime.auth.snapshot().auth_token.is_some());
    }

    #[test]
    fn send_two_factor_sms_requests_delivery_for_pending_challenge() {
        let mut runtime = bootstrap_test_shell();

        let _ = dispatch(
            &mut runtime,
            Request::PasswordSubmission {
                username: "alice@example.com".to_owned(),
                value: "correct-horse".to_owned(),
            },
        );
        let response = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::SendTwoFactorSms,
            },
        );

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("tfa sms requested"));
        assert!(response.message.contains("+49"));
    }

    #[test]
    fn send_two_factor_notification_requests_delivery_for_pending_challenge() {
        let mut runtime = bootstrap_test_shell();

        let _ = dispatch(
            &mut runtime,
            Request::PasswordSubmission {
                username: "alice@example.com".to_owned(),
                value: "correct-horse".to_owned(),
            },
        );
        let response = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::SendTwoFactorNotification,
            },
        );

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("tfa notification requested"));
        assert!(response.message.contains("Pixel"));
    }

    #[test]
    fn userinfo_request_uses_authenticated_session_token() {
        let mut runtime = bootstrap_test_shell();

        let _ = dispatch(
            &mut runtime,
            Request::PasswordSubmission {
                username: "alice@example.com".to_owned(),
                value: "correct-horse".to_owned(),
            },
        );
        let _ = dispatch(
            &mut runtime,
            Request::TwoFactorCodeSubmission {
                value: "654321".to_owned(),
                trust_device: false,
                recovery_code: false,
            },
        );
        let response = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::GetUserInfo,
            },
        );

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("alice@example.com"));
    }

    #[test]
    fn recovery_code_submission_completes_protocol_auth_flow() {
        let mut runtime = bootstrap_test_shell();

        let _ = dispatch(
            &mut runtime,
            Request::PasswordSubmission {
                username: "alice@example.com".to_owned(),
                value: "correct-horse".to_owned(),
            },
        );
        let response = dispatch(
            &mut runtime,
            Request::TwoFactorCodeSubmission {
                value: "654321".to_owned(),
                trust_device: false,
                recovery_code: true,
            },
        );

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("LoginSucceeded"));
        assert_eq!(runtime.auth.snapshot().state, SessionState::Authenticated);
    }

    #[test]
    fn auth_token_submission_authenticates_via_userinfo() {
        let mut runtime = bootstrap_test_shell();

        let response = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(response.message.contains("LoginSucceeded"));
        assert_eq!(runtime.auth.snapshot().state, SessionState::Authenticated);
        assert_eq!(
            runtime
                .auth
                .snapshot()
                .authenticated_user
                .map(|id| id.get()),
            Some(42)
        );
        assert_eq!(
            runtime.auth.snapshot().email.as_deref(),
            Some("alice@example.com")
        );
        assert!(
            runtime
                .store
                .repositories
                .accounts
                .primary_account
                .is_some()
        );
    }

    #[test]
    fn runtime_polls_remote_diff_through_sync_api_and_populates_engine_queue() {
        let mut runtime = bootstrap_test_shell();

        let auth = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(auth.status, ResponseStatus::Ok);

        let planned = runtime
            .poll_remote_diff_once(0, 128)
            .expect("remote diff should succeed");

        assert_eq!(planned, 1);
        assert_eq!(runtime.engine.scheduler.queued_operations.len(), 1);
        assert!(runtime.engine.summary().contains("queued=1"));
    }

    #[test]
    fn runtime_prepares_download_and_upload_metadata_through_transfer_runtime() {
        let mut runtime = bootstrap_test_shell();

        let auth = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(auth.status, ResponseStatus::Ok);

        let _ = runtime.engine.ingest_candidates(&[
            pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(1),
                source: pcloud_model::sync::ChangeSource::Remote,
                path: "report.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: Some(pcloud_model::ids::RemoteFileId::new(9)),
                remote_folder_id: None,
            },
            pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(1),
                source: pcloud_model::sync::ChangeSource::Local,
                path: "upload.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: None,
                remote_folder_id: None,
            },
        ]);
        runtime
            .filesystem
            .seed_staged_file("upload.txt", b"local-upload-payload".to_vec());
        let _ = runtime.engine.advance_transfer_cycle();

        let prepared_downloads = runtime
            .prepare_active_downloads_once()
            .expect("download link fetch should succeed");
        let prepared_uploads = runtime
            .prepare_active_uploads_once()
            .expect("upload_create should succeed");

        assert_eq!(prepared_downloads, 1);
        assert_eq!(prepared_uploads, 1);
    }

    #[test]
    fn runtime_executes_download_and_upload_tasks_and_updates_lifecycle() {
        let mut runtime = bootstrap_test_shell();

        let auth = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(auth.status, ResponseStatus::Ok);

        let _ = runtime.engine.ingest_candidates(&[
            pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(1),
                source: pcloud_model::sync::ChangeSource::Remote,
                path: "report.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: Some(pcloud_model::ids::RemoteFileId::new(9)),
                remote_folder_id: None,
            },
            pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(1),
                source: pcloud_model::sync::ChangeSource::Local,
                path: "upload.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: None,
                remote_folder_id: None,
            },
        ]);
        runtime
            .filesystem
            .seed_staged_file("upload.txt", b"local-upload-payload".to_vec());
        let _ = runtime.engine.advance_transfer_cycle();

        let completed_downloads = runtime
            .execute_active_downloads_once()
            .expect("download execution should succeed");
        let completed_uploads = runtime
            .execute_active_uploads_once()
            .expect("upload execution should succeed");

        assert_eq!(completed_downloads, 1);
        assert_eq!(completed_uploads, 1);
        assert_eq!(runtime.engine.downloads.completed_count(), 1);
        assert_eq!(runtime.engine.uploads.completed_count(), 1);
        assert_eq!(
            runtime
                .filesystem
                .read_staged_path("report.txt", 0, usize::MAX)
                .expect("downloaded bytes should be staged")
                .bytes,
            b"downloaded:/get/abc/report.txt"
        );
        assert_eq!(
            runtime
                .cache
                .staging
                .get("report.txt")
                .expect("downloaded bytes should be present in cache staging"),
            &b"downloaded:/get/abc/report.txt"[..]
        );
        assert_eq!(
            &**runtime
                .cache
                .pages
                .get("upload:77:upload.txt")
                .expect("uploaded payload should be cached"),
            b"local-upload-payload"
        );
        assert!(runtime.engine.summary().contains("completed_uploads=1"));
        assert!(runtime.engine.summary().contains("completed_downloads=1"));
    }

    #[test]
    fn runtime_marks_failed_transfer_tasks_when_execution_fails() {
        let mut runtime = bootstrap_test_shell();

        let auth = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(auth.status, ResponseStatus::Ok);

        let _ = runtime.engine.ingest_candidates(&[
            pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(1),
                source: pcloud_model::sync::ChangeSource::Remote,
                path: "bad-download.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: Some(pcloud_model::ids::RemoteFileId::new(999)),
                remote_folder_id: None,
            },
            pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(1),
                source: pcloud_model::sync::ChangeSource::Local,
                path: "fail-upload.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: None,
                remote_folder_id: None,
            },
        ]);
        runtime
            .filesystem
            .seed_staged_file("fail-upload.txt", b"payload".to_vec());
        let _ = runtime.engine.advance_transfer_cycle();

        let completed_downloads = runtime
            .execute_active_downloads_once()
            .expect("download execution should not abort runtime");
        let completed_uploads = runtime
            .execute_active_uploads_once()
            .expect("upload execution should not abort runtime");

        assert_eq!(completed_downloads, 0);
        assert_eq!(completed_uploads, 0);
        assert_eq!(runtime.engine.downloads.failed_count(), 1);
        assert_eq!(runtime.engine.uploads.failed_count(), 1);
        assert!(
            runtime.engine.downloads.failed[0]
                .last_error
                .as_deref()
                .unwrap_or("")
                .contains("recovery=RetryLater")
        );
        assert!(
            runtime.engine.uploads.failed[0]
                .last_error
                .as_deref()
                .unwrap_or("")
                .contains("recovery=RetryLater")
        );
    }

    #[test]
    fn upload_execution_fails_when_local_payload_is_missing() {
        let mut runtime = bootstrap_test_shell();

        let auth = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(auth.status, ResponseStatus::Ok);

        let _ = runtime
            .engine
            .ingest_candidates(&[pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(1),
                source: pcloud_model::sync::ChangeSource::Local,
                path: "missing-upload.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: None,
                remote_folder_id: None,
            }]);
        let _ = runtime.engine.advance_transfer_cycle();

        let completed_uploads = runtime
            .execute_active_uploads_once()
            .expect("upload execution should not abort runtime");

        assert_eq!(completed_uploads, 0);
        assert_eq!(runtime.engine.uploads.failed_count(), 1);
        assert!(
            runtime.engine.uploads.failed[0]
                .last_error
                .as_deref()
                .unwrap_or("")
                .contains("missing staged upload payload")
        );
        assert!(
            runtime.engine.uploads.failed[0]
                .last_error
                .as_deref()
                .unwrap_or("")
                .contains("recovery=Terminal")
        );
    }

    #[test]
    fn nested_upload_uses_remote_parent_folder_and_basename() {
        let mut runtime = bootstrap_test_shell();

        let auth = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(auth.status, ResponseStatus::Ok);

        let batch = runtime
            .engine
            .ingest_candidates(&[pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(1),
                source: pcloud_model::sync::ChangeSource::Local,
                path: "docs/nested/report.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: None,
                remote_folder_id: Some(RemoteFolderId::new(44)),
            }]);
        assert_eq!(
            batch,
            [pcloud_model::sync::PlannedOperation::UploadFile {
                sync_id: pcloud_model::ids::SyncId::new(1),
                path: "docs/nested/report.txt".to_owned(),
                remote_parent_folder_id: Some(RemoteFolderId::new(44)),
                remote_name: "report.txt".to_owned(),
            }]
        );

        runtime
            .filesystem
            .seed_staged_file("docs/nested/report.txt", b"nested-payload".to_vec());
        let _ = runtime.engine.advance_transfer_cycle();

        let completed = runtime
            .execute_active_uploads_once()
            .expect("nested upload execution should succeed");

        assert_eq!(completed, 1);
        assert_eq!(runtime.engine.uploads.completed_count(), 1);
        assert!(
            runtime
                .cache
                .pages
                .get("upload:77:docs/nested/report.txt")
                .is_some()
        );
    }

    #[test]
    fn upload_execution_reads_full_payload_across_prefetch_windows() {
        let mut runtime = bootstrap_test_shell();
        runtime.filesystem.reads.prefetch_window_bytes = 4;

        let auth = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(auth.status, ResponseStatus::Ok);

        let _ = runtime
            .engine
            .ingest_candidates(&[pcloud_model::sync::SyncCandidate {
                sync_id: pcloud_model::ids::SyncId::new(1),
                source: pcloud_model::sync::ChangeSource::Local,
                path: "large-upload.txt".to_owned(),
                entry_kind: pcloud_model::sync::EntryKind::File,
                change_kind: pcloud_model::sync::ChangeKind::Upsert,
                remote_file_id: None,
                remote_folder_id: None,
            }]);
        runtime
            .filesystem
            .seed_staged_file("large-upload.txt", b"abcdefghij".to_vec());
        let _ = runtime.engine.advance_transfer_cycle();

        let completed = runtime
            .execute_active_uploads_once()
            .expect("upload execution should succeed");

        assert_eq!(completed, 1);
        assert_eq!(runtime.engine.uploads.completed_count(), 1);
        assert_eq!(
            &**runtime
                .cache
                .pages
                .get("upload:77:large-upload.txt")
                .expect("uploaded payload should be cached"),
            b"abcdefghij"
        );
    }

    #[test]
    fn logout_clears_session_and_persisted_account_state() {
        let mut runtime = bootstrap_test_shell();

        let _ = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        let response = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::Logout,
            },
        );

        assert_eq!(response.status, ResponseStatus::Ok);
        assert_eq!(runtime.auth.snapshot().state, SessionState::LoggedOut);
        assert!(runtime.auth.snapshot().auth_token.is_none());
        assert!(
            runtime
                .store
                .repositories
                .accounts
                .primary_account
                .is_none()
        );
        assert!(!runtime.config.paths.auth_token_vault_path().exists());
    }

    #[test]
    fn auth_persistence_failure_does_not_project_authenticated_account_into_store() {
        let mut runtime = bootstrap_test_shell();
        let blocking_path = runtime.config.paths.config_dir.join("not-a-directory");
        fs::write(&blocking_path, b"block").expect("blocking file should be created");
        runtime.config.paths.config_dir = blocking_path;

        let response = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );

        assert_eq!(response.status, ResponseStatus::InternalError);
        assert!(response.message.contains("failed to persist auth state"));
        assert!(
            runtime
                .store
                .repositories
                .accounts
                .primary_account
                .is_none()
        );
    }

    #[test]
    fn auth_and_userinfo_emit_audit_events() {
        let mut runtime = bootstrap_test_shell();

        let auth = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(auth.status, ResponseStatus::Ok);

        let userinfo = dispatch(
            &mut runtime,
            Request::Plain {
                method: pcloud_ipc::Method::GetUserInfo,
            },
        );
        assert_eq!(userinfo.status, ResponseStatus::Ok);
        assert_eq!(runtime.store.repositories.audit.retained_event_count, 2);
        assert!(runtime.observability.summary().contains("emitted_events=2"));
        assert_eq!(
            runtime.observability.live_health.last_event.as_deref(),
            Some("auth.userinfo")
        );
    }

    #[test]
    fn auth_tokens_are_not_persisted_by_secure_default() {
        let mut runtime = bootstrap_test_shell();
        assert!(!runtime.config.features.durable_auth_tokens_enabled);

        let response = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );

        assert_eq!(response.status, ResponseStatus::Ok);
        assert!(!runtime.config.paths.auth_token_vault_path().exists());
        assert!(
            runtime
                .store
                .repositories
                .accounts
                .primary_account
                .is_some()
        );
        assert_eq!(
            runtime
                .store
                .repositories
                .preferences
                .durable_auth_tokens_enabled,
            None
        );
    }

    #[test]
    fn authsave_toggle_persists_preference_and_syncs_vault() {
        let mut runtime = bootstrap_test_shell();
        let vault_path = runtime.config.paths.auth_token_vault_path();

        let enable = dispatch(&mut runtime, Request::AuthPersistence { enabled: true });
        assert_eq!(enable.status, ResponseStatus::Ok);
        assert!(runtime.config.features.durable_auth_tokens_enabled);
        assert_eq!(
            runtime
                .store
                .repositories
                .preferences
                .durable_auth_tokens_enabled,
            Some(true)
        );
        assert!(!vault_path.exists());

        let auth = dispatch(
            &mut runtime,
            Request::AuthTokenSubmission {
                value: "digest-auth-token".to_owned(),
            },
        );
        assert_eq!(auth.status, ResponseStatus::Ok);
        assert!(vault_path.exists());

        let disable = dispatch(&mut runtime, Request::AuthPersistence { enabled: false });
        assert_eq!(disable.status, ResponseStatus::Ok);
        assert!(!runtime.config.features.durable_auth_tokens_enabled);
        assert_eq!(
            runtime
                .store
                .repositories
                .preferences
                .durable_auth_tokens_enabled,
            Some(false)
        );
        assert!(!vault_path.exists());
    }

    #[test]
    fn bootstrap_restores_session_from_auth_vault() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pcloud-daemon-restore-{}-{nonce}",
            std::process::id()
        ));
        let mut config = ConfigProfile::secure_defaults(root, Environment::Development);
        config.features.durable_auth_tokens_enabled = true;
        fs::create_dir_all(&config.paths.config_dir).expect("config dir should exist");
        fs::write(config.paths.auth_token_vault_path(), "digest-auth-token\n")
            .expect("vault file should be written");
        fs::set_permissions(
            config.paths.auth_token_vault_path(),
            fs::Permissions::from_mode(0o600),
        )
        .expect("vault permissions should be tightened");

        let runtime = bootstrap_with_config(config).expect("runtime bootstrap should succeed");

        assert_eq!(runtime.auth.snapshot().state, SessionState::Authenticated);
        assert_eq!(
            runtime
                .auth
                .snapshot()
                .authenticated_user
                .map(|id| id.get()),
            Some(42)
        );
        assert_eq!(
            runtime.auth.snapshot().email.as_deref(),
            Some("alice@example.com")
        );
    }

    #[test]
    fn bootstrap_authenticates_from_token_file() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pcloud-daemon-token-file-{}-{nonce}",
            std::process::id()
        ));
        let config = ConfigProfile::secure_defaults(root.clone(), Environment::Development);
        let (mut store, integrity) =
            pcloud_store::bootstrap_profile(&config.paths.state_dir.join("store.sqlite3"))
                .expect("store bootstrap should succeed");
        let mut auth = pcloud_auth::SessionManager::new();
        let auth_runtime = crate::auth_backend::AuthRuntime::from_config(&config);

        let used = crate::bootstrap::apply_bootstrap_credentials(
            &config,
            &mut store,
            &auth_runtime,
            &mut auth,
            crate::bootstrap::BootstrapCredentials {
                token: Some(pcloud_secret::secret_string::SecretString::new(
                    "digest-auth-token",
                )),
                ..Default::default()
            },
        )
        .expect("credential bootstrap should succeed");

        assert!(used);
        assert_eq!(auth.snapshot().state, SessionState::Authenticated);
        assert_eq!(
            auth.snapshot().authenticated_user.map(|id| id.get()),
            Some(42)
        );
        assert_eq!(auth.snapshot().email.as_deref(), Some("alice@example.com"));
        assert!(store.repositories.accounts.primary_account.is_some());
        let _ = integrity;
        let _ = auth_runtime;
        let _ = config;
    }

    #[test]
    fn bootstrap_authenticates_from_username_password_and_tfa_files() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pcloud-daemon-password-files-{}-{nonce}",
            std::process::id()
        ));
        let config = ConfigProfile::secure_defaults(root.clone(), Environment::Development);
        let (mut store, _) =
            pcloud_store::bootstrap_profile(&config.paths.state_dir.join("store.sqlite3"))
                .expect("store bootstrap should succeed");
        let mut auth = pcloud_auth::SessionManager::new();
        let auth_runtime = crate::auth_backend::AuthRuntime::from_config(&config);

        let used = crate::bootstrap::apply_bootstrap_credentials(
            &config,
            &mut store,
            &auth_runtime,
            &mut auth,
            crate::bootstrap::BootstrapCredentials {
                username: Some("alice@example.com".to_owned()),
                password: Some(pcloud_secret::secret_string::SecretString::new(
                    "correct-horse",
                )),
                two_factor_code: Some(pcloud_secret::secret_string::SecretString::new("654321")),
                ..Default::default()
            },
        )
        .expect("credential bootstrap should succeed");

        assert!(used);
        assert_eq!(auth.snapshot().state, SessionState::Authenticated);
        assert_eq!(
            auth.snapshot().authenticated_user.map(|id| id.get()),
            Some(42)
        );
        assert_eq!(auth.snapshot().email.as_deref(), Some("alice@example.com"));
        assert!(store.repositories.accounts.primary_account.is_some());
    }

    #[test]
    fn bootstrap_uses_persisted_authsave_preference() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pcloud-daemon-authsave-pref-{}-{nonce}",
            std::process::id()
        ));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);

        let store_path = config.paths.state_dir.join("store.sqlite3");
        let (mut seeded_store, _) =
            pcloud_store::bootstrap_profile(&store_path).expect("store bootstrap should succeed");
        seeded_store
            .repositories
            .preferences
            .durable_auth_tokens_enabled = Some(true);
        pcloud_store::persist_profile(&seeded_store).expect("seeded store should persist");

        let runtime = bootstrap_with_config(config).expect("runtime bootstrap should succeed");

        assert!(runtime.config.features.durable_auth_tokens_enabled);
        assert_eq!(
            runtime
                .store
                .repositories
                .preferences
                .durable_auth_tokens_enabled,
            Some(true)
        );
    }

    #[test]
    fn bootstrap_restores_persisted_api_server_preference() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pcloud-daemon-api-server-pref-{}-{nonce}",
            std::process::id()
        ));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);

        let store_path = config.paths.state_dir.join("store.sqlite3");
        let (mut seeded_store, _) =
            pcloud_store::bootstrap_profile(&store_path).expect("store bootstrap should succeed");
        seeded_store.repositories.preferences.api_server_binapi =
            Some("bineapi-eu.pcloud.com:8443".to_owned());
        seeded_store.repositories.preferences.api_server_location_id = Some(2);
        pcloud_store::persist_profile(&seeded_store).expect("seeded store should persist");

        let runtime = bootstrap_with_config(config).expect("runtime bootstrap should succeed");

        assert_eq!(runtime.config.api.host, "bineapi-eu.pcloud.com");
        assert_eq!(runtime.config.api.server_name, "bineapi-eu.pcloud.com");
        assert_eq!(runtime.config.api.port, 8443);
        assert_eq!(
            runtime
                .store
                .repositories
                .preferences
                .api_server_binapi
                .as_deref(),
            Some("bineapi-eu.pcloud.com:8443")
        );
        assert_eq!(
            runtime
                .store
                .repositories
                .preferences
                .api_server_location_id,
            Some(2)
        );
    }

    #[test]
    fn bootstrap_clears_stale_account_when_vault_token_is_invalid() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pcloud-daemon-invalid-vault-{}-{nonce}",
            std::process::id()
        ));
        let mut config = ConfigProfile::secure_defaults(root, Environment::Test);
        config.features.durable_auth_tokens_enabled = true;
        config.api.mode = pcloud_config::api::ApiMode::Plaintext;
        config.api.host = "127.0.0.1".to_owned();
        config.api.port = 9;
        config.api.server_name = "127.0.0.1".to_owned();
        config.api.connect_timeout_ms = 100;
        config.api.read_timeout_ms = 100;

        let (mut seeded_store, _) =
            pcloud_store::bootstrap_profile(&config.paths.state_dir.join("store.sqlite3"))
                .expect("store bootstrap should succeed");
        seeded_store.repositories.accounts.primary_account =
            Some(pcloud_store::repositories::account::AccountRecord {
                user_id: pcloud_model::ids::UserId::new(7),
                email: "stale@example.com".to_owned(),
                auth_token_present: true,
            });
        pcloud_store::persist_profile(&seeded_store).expect("seeded store should persist");

        fs::create_dir_all(&config.paths.config_dir).expect("config dir should exist");
        fs::write(config.paths.auth_token_vault_path(), "invalid-token\n")
            .expect("vault file should be written");
        fs::set_permissions(
            config.paths.auth_token_vault_path(),
            fs::Permissions::from_mode(0o600),
        )
        .expect("vault permissions should be tightened");

        let runtime =
            bootstrap_with_config(config.clone()).expect("runtime bootstrap should succeed");

        assert_eq!(runtime.auth.snapshot().state, SessionState::AuthFailed);
        assert!(
            runtime
                .store
                .repositories
                .accounts
                .primary_account
                .is_none()
        );
        assert!(!config.paths.auth_token_vault_path().exists());
    }

    #[test]
    fn bootstrap_rejects_insecure_auth_vault_permissions() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pcloud-daemon-insecure-vault-{}-{nonce}",
            std::process::id()
        ));
        let mut config = ConfigProfile::secure_defaults(root, Environment::Development);
        config.features.durable_auth_tokens_enabled = true;
        fs::create_dir_all(&config.paths.config_dir).expect("config dir should exist");
        fs::write(config.paths.auth_token_vault_path(), "digest-auth-token\n")
            .expect("vault file should be written");
        fs::set_permissions(
            config.paths.auth_token_vault_path(),
            fs::Permissions::from_mode(0o644),
        )
        .expect("vault permissions should be relaxed");

        let err = bootstrap_with_config(config).expect_err("insecure vault should be rejected");

        assert!(
            err.to_string()
                .contains("vault file must not grant group or other access")
        );
    }

    #[test]
    fn production_config_rejects_api_downgrade() {
        let mut config = ConfigProfile::secure_defaults(
            std::env::temp_dir().join("pcloud-production-downgrade-test"),
            Environment::Production,
        );
        config.api.mode = pcloud_config::api::ApiMode::Plaintext;

        let err =
            bootstrap_with_config(config).expect_err("plaintext production api should be rejected");
        assert!(
            err.to_string()
                .contains("production environment requires tls api mode")
        );
    }

    // --------------------------------------------------------------------
    // Folder-metadata helpers (rows 77/78/84 in the parity matrix):
    //   - Request::GetFolderIdByPath   -> psync_get_fsfolderid_by_path
    //   - Request::GetFolderFlags      -> psync_get_fsfolderflags_by_id
    //   - Request::GetFolderOwnerId    -> psync_get_folder_ownerid
    // These tests cover the no-auth and invalid-path gates at the
    // dispatch layer. Full listfolder-backed resolution is covered by
    // the unit tests in `path_resolver::tests` (mock transport).
    // --------------------------------------------------------------------

    #[test]
    fn get_folder_id_by_path_rejects_missing_auth() {
        let mut runtime = bootstrap_test_shell();
        let resp = dispatch(
            &mut runtime,
            Request::GetFolderIdByPath {
                path: "/Docs".to_owned(),
            },
        );
        assert_eq!(resp.status, ResponseStatus::Unauthorized);
        assert!(resp.message.contains("authenticated"));
    }

    #[test]
    fn get_folder_id_by_path_rejects_empty_path() {
        let mut runtime = bootstrap_test_shell();
        let resp = dispatch(
            &mut runtime,
            Request::GetFolderIdByPath {
                path: "   ".to_owned(),
            },
        );
        assert_eq!(resp.status, ResponseStatus::InvalidRequest);
    }

    #[test]
    fn get_folder_flags_rejects_missing_auth() {
        let mut runtime = bootstrap_test_shell();
        let resp = dispatch(
            &mut runtime,
            Request::GetFolderFlags {
                path: "/Docs".to_owned(),
            },
        );
        assert_eq!(resp.status, ResponseStatus::Unauthorized);
    }

    #[test]
    fn get_folder_owner_id_rejects_missing_auth() {
        let mut runtime = bootstrap_test_shell();
        let resp = dispatch(
            &mut runtime,
            Request::GetFolderOwnerId {
                path: "/Docs".to_owned(),
            },
        );
        assert_eq!(resp.status, ResponseStatus::Unauthorized);
    }

    // --------------------------------------------------------------------
    // filesystem_status (row 86) — does not need any transport or auth.
    // --------------------------------------------------------------------

    #[test]
    fn filesystem_status_rejects_empty_path() {
        let mut runtime = bootstrap_test_shell();
        let resp = dispatch(
            &mut runtime,
            Request::FilesystemStatus {
                path: String::new(),
            },
        );
        assert_eq!(resp.status, ResponseStatus::InvalidRequest);
    }

    #[test]
    fn filesystem_status_outside_any_sync_root_returns_invsync() {
        let mut runtime = bootstrap_test_shell();
        let resp = dispatch(
            &mut runtime,
            Request::FilesystemStatus {
                path: "/no/such/path".to_owned(),
            },
        );
        assert_eq!(resp.status, ResponseStatus::Ok);
        assert_eq!(resp.message, "INVSYNC");
    }

    #[test]
    fn filesystem_status_within_idle_sync_root_returns_insync() {
        use pcloud_model::ids::SyncId;
        use pcloud_store::repositories::sync_graph::SyncRootRecord;

        let mut runtime = bootstrap_test_shell();
        runtime
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .push(SyncRootRecord {
                sync_id: SyncId::new(7),
                local_path: "/mnt/pcloud".to_owned(),
                remote_path: "/".to_owned(),
                paused: false,
                sync_type: pcloud_model::sync::SyncType::Full,
            });

        let resp = dispatch(
            &mut runtime,
            Request::FilesystemStatus {
                path: "/mnt/pcloud/docs/report.txt".to_owned(),
            },
        );
        assert_eq!(resp.status, ResponseStatus::Ok);
        assert_eq!(resp.message, "INSYNC");
    }

    #[test]
    fn filesystem_status_in_paused_sync_root_returns_nosync() {
        use pcloud_model::ids::SyncId;
        use pcloud_store::repositories::sync_graph::SyncRootRecord;

        let mut runtime = bootstrap_test_shell();
        runtime
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .push(SyncRootRecord {
                sync_id: SyncId::new(8),
                local_path: "/mnt/paused".to_owned(),
                remote_path: "/".to_owned(),
                paused: true,
                sync_type: pcloud_model::sync::SyncType::Full,
            });

        let resp = dispatch(
            &mut runtime,
            Request::FilesystemStatus {
                path: "/mnt/paused/anything".to_owned(),
            },
        );
        assert_eq!(resp.status, ResponseStatus::Ok);
        assert_eq!(resp.message, "NOSYNC");
    }

    // --------------------------------------------------------------------
    // path_resolver unit tests (mock transport, rows 77/78/84 resolver
    // behavior). The MockTransport mirrors the one in path_resolver.rs
    // but exercises the three public helpers (id/flags/owner) end-to-end.
    // --------------------------------------------------------------------

    #[test]
    fn path_resolver_public_helpers_use_mocked_listfolder() {
        use pcloud_proto::auth_api::{ApiServerHintConsumer, ProtocolTransport};
        use pcloud_proto::response::Value;
        use pcloud_secret::secret_string::SecretString;
        use std::io;
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct MockTransport {
            responses: Arc<Mutex<Vec<Value>>>,
        }

        impl ProtocolTransport for MockTransport {
            type Error = io::Error;
            fn execute(
                &self,
                _request: &pcloud_proto::EncodedRequest,
            ) -> Result<Value, Self::Error> {
                self.responses
                    .lock()
                    .expect("mock responses lock")
                    .pop()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no mock"))
            }
        }

        impl ApiServerHintConsumer for MockTransport {
            fn apply_api_server_hint(&self, _: &str) {}
        }

        // Build a `metadata` hash with the can* permission bools the
        // folder_api extractor consumes (canread/canmodify/...), plus
        // the `encrypted`, `isshared`, `userid` facets.
        #[allow(clippy::too_many_arguments)]
        fn listing_with(
            folder_id: u64,
            can_read: bool,
            can_modify: bool,
            can_create: bool,
            can_delete: bool,
            can_manage: bool,
            encrypted: bool,
            is_shared: bool,
            owner_user_id: u64,
            entries: Vec<Value>,
        ) -> Value {
            Value::Hash(vec![(
                "metadata".to_owned(),
                Value::Hash(vec![
                    ("folderid".to_owned(), Value::Number(folder_id)),
                    ("name".to_owned(), Value::String("Docs".to_owned())),
                    ("canread".to_owned(), Value::Bool(can_read)),
                    ("canmodify".to_owned(), Value::Bool(can_modify)),
                    ("cancreate".to_owned(), Value::Bool(can_create)),
                    ("candelete".to_owned(), Value::Bool(can_delete)),
                    ("canmanage".to_owned(), Value::Bool(can_manage)),
                    ("encrypted".to_owned(), Value::Bool(encrypted)),
                    ("isshared".to_owned(), Value::Bool(is_shared)),
                    ("userid".to_owned(), Value::Number(owner_user_id)),
                    ("contents".to_owned(), Value::Array(entries)),
                ]),
            )])
        }

        // Response queue is popped in LIFO order — push what's consumed
        // last first. Each helper issues exactly one `listfolder` call
        // against the absolute path, so we push one response per helper.
        let responses = Arc::new(Mutex::new(vec![
            // owner_id call (all caps on)
            listing_with(11, true, true, true, true, true, false, false, 777, vec![]),
            // flags call — read only, encrypted, shared
            listing_with(11, true, false, false, false, false, true, true, 0, vec![]),
            // id-by-path call — parent listing for `/Docs`
            Value::Hash(vec![(
                "metadata".to_owned(),
                Value::Hash(vec![
                    ("folderid".to_owned(), Value::Number(0)),
                    ("name".to_owned(), Value::String("/".to_owned())),
                    (
                        "contents".to_owned(),
                        Value::Array(vec![Value::Hash(vec![
                            ("name".to_owned(), Value::String("Docs".to_owned())),
                            ("isfolder".to_owned(), Value::Bool(true)),
                            ("folderid".to_owned(), Value::Number(11)),
                        ])]),
                    ),
                ]),
            )]),
        ]));
        let transport = MockTransport {
            responses: responses.clone(),
        };
        let resolver = crate::path_resolver::RemotePathResolver::new(
            transport,
            SecretString::new("tok".to_owned()),
        );

        let id = resolver
            .get_folder_id_by_path("/Docs")
            .expect("id resolves");
        assert_eq!(id.get(), 11);

        let flags = resolver.get_folder_flags("/Docs").expect("flags resolve");
        // READ=1; with no write caps, readonly=true. See `perm_bits` in
        // `pcloud_proto::folder_api` for the C PSYNC_PERM_* layout.
        assert_eq!(
            flags.permissions,
            Some(pcloud_proto::folder_api::perm_bits::READ)
        );
        assert!(flags.encrypted);
        assert!(flags.shared);
        assert!(flags.readonly);

        let owner = resolver
            .get_folder_owner_id("/Docs")
            .expect("owner resolves");
        assert_eq!(owner.get(), 777);
    }
}
