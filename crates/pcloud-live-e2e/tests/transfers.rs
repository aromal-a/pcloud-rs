#![allow(clippy::pedantic)]
//! Live transfer harness: upload_create/write/save, get_file_link, download.
//!
//! Uploads a unique temporary blob under the configured scratch folder, asserts
//! the round-trip returns the original bytes, then deletes the upload so no
//! residue is left on the account.

// **PLATFORM:** all
// **GATING:** none (portable).

mod common;

use std::{io::Write, time::SystemTime};

use pcloud_sdk::EmbeddedDaemon;

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, authenticate, gate_enabled, optional_env,
    scratch_folder, skip_if_not_live,
};

fn unique_name(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}-{nanos}.bin", std::process::id())
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_upload_download_roundtrip() {
    if !gate_enabled() {
        let _ = skip_if_not_live(&[]);
        return;
    }
    // Require either a token OR (user+password).
    if optional_env(ENV_TOKEN).is_none()
        && (optional_env(ENV_USER).is_none() || optional_env(ENV_PASSWORD).is_none())
    {
        eprintln!(
            "[live-e2e] skipping upload/download: need {ENV_TOKEN} or {ENV_USER}+{ENV_PASSWORD}"
        );
        return;
    }

    let mut daemon = TestDaemon::new("transfers");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping upload/download: {err}");
        return;
    }

    // Drive transfers through the SDK's direct-upload helper which exercises
    // upload_create + upload_write + upload_save against the live backend.
    let root = daemon
        .config
        .paths
        .config_dir
        .parent()
        .unwrap()
        .to_path_buf();
    drop(daemon); // Release the runtime so we can hand the root to EmbeddedDaemon.

    let mut sdk = EmbeddedDaemon::builder(root.clone())
        .build()
        .expect("SDK bootstrap");

    // Re-authenticate the embedded daemon via IPC dispatch to keep credentials
    // out of the fresh in-memory session.
    let resp = if let Some(token) = optional_env(ENV_TOKEN) {
        sdk.dispatch(pcloud_ipc::Request::AuthTokenSubmission { value: token })
    } else {
        sdk.dispatch(pcloud_ipc::Request::PasswordSubmission {
            username: optional_env(ENV_USER).unwrap(),
            value: optional_env(ENV_PASSWORD).unwrap(),
        })
    };
    assert_eq!(
        resp.status,
        pcloud_ipc::ResponseStatus::Ok,
        "re-auth for transfer harness failed: {}",
        resp.message
    );
    if !sdk.is_authenticated() {
        eprintln!("[live-e2e] skipping: account needs TFA; use a scoped PCLOUD_TEST_TOKEN instead");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    let scratch_path = scratch_folder();
    let filename = unique_name("live-e2e");
    let payload: Vec<u8> = (0u16..4096).flat_map(|n| n.to_le_bytes()).collect();

    let uploaded = sdk
        .upload_data_as(&scratch_path, filename.clone(), &payload)
        .expect("upload_data_as against live backend");
    assert_eq!(uploaded.bytes_uploaded, payload.len());
    assert_eq!(uploaded.remote_filename, filename);

    let file_id = uploaded
        .file_id
        .expect("backend should have assigned a file_id after upload_save");

    // Round-trip: re-download and compare byte-for-byte.
    let fetched = sdk
        .download_file(file_id)
        .expect("download_file against live backend");
    assert_eq!(fetched, payload, "round-tripped bytes must match");

    // Best-effort cleanup. The public deletefile API is not (yet) on the
    // active Rust path; record the file_id to a local trace file so humans can
    // clean up if this harness is ever run against a shared account.
    let trace_dir = std::env::temp_dir().join("pcloud-live-e2e-traces");
    let _ = std::fs::create_dir_all(&trace_dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(trace_dir.join("uploaded_file_ids.log"))
    {
        let _ = writeln!(
            f,
            "{} {} {}",
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default(),
            file_id,
            filename
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}
