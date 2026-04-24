#![allow(clippy::pedantic)]
//! Live Windows-liveness probe: daemon ↔ real pCloud API round-trip.
//!
//! Exercises the core daemon control surface from an opt-in
//! live-E2E harness: login → userinfo → listfolder → upload/download
//! round-trip → logout. No FUSE / no mount is required; this test
//! targets the pure "daemon talks to eapi.pcloud.com" path so it can
//! run on Linux, macOS, and Windows (named-pipe IPC + Tokio + TLS).
//!
//! Credentials are read ONLY from env vars. Prefers the task-specific
//! `PCLOUD_USERNAME` / `PCLOUD_PASSWORD` names, falling back to the
//! shared-harness `PCLOUD_TEST_USER` / `PCLOUD_TEST_PASSWORD` so the
//! same test binary runs under either environment without edits.
//!
//! Security invariants:
//!
//! * No credential value is ever logged. We only report lengths and
//!   the shape of the response.
//! * Each invocation scopes its daemon to a unique temp root and
//!   removes that root on completion, whether the test passes or
//!   fails.
//! * Any uploaded blob is registered on a per-file cleanup guard so
//!   the file_id and remote filename land in a trace log even when
//!   an assertion panics mid-test (IPC-level delete is still stubbed;
//!   see `bd-1du.10`).
//! * The test is `#[ignore]`d by default and short-circuits unless
//!   `PCLOUD_LIVE_E2E=1` is set.

#![forbid(unsafe_code)]

// **PLATFORM:** all (Linux / macOS / Windows).
// **GATING:** `PCLOUD_LIVE_E2E=1` + credentials.

mod common;

use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use pcloud_auth::SessionState;
use pcloud_ipc::{Method, Request, ResponseStatus};
use pcloud_sdk::EmbeddedDaemon;

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, expect_ok, gate_enabled,
    optional_env, scratch_folder, skip_if_not_live, status_label,
};

/// Accept both the task-specified env names and the shared-harness
/// names. The task brief uses `PCLOUD_USERNAME` / `PCLOUD_PASSWORD`;
/// the rest of the live-E2E crate standardises on
/// `PCLOUD_TEST_USER` / `PCLOUD_TEST_PASSWORD`. Honour whichever is
/// set, preferring the task-specific variant when both are present.
fn liveness_username() -> Option<String> {
    optional_env("PCLOUD_USERNAME").or_else(|| optional_env(ENV_USER))
}

fn liveness_password() -> Option<String> {
    optional_env("PCLOUD_PASSWORD").or_else(|| optional_env(ENV_PASSWORD))
}

/// Build a name unique enough that two concurrent test runs (or a
/// Windows and a Linux run in parallel) cannot collide. Embeds PID
/// and nanosecond wall-clock, plus a host tag.
fn unique_blob_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let host = std::env::var("COMPUTERNAME") // Windows
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok()) // most Linux shells
        .unwrap_or_else(|| "unknown".to_owned());
    // Sanitise host tag: keep only ASCII alnum + dashes.
    let host: String = host
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(24)
        .collect();
    format!(
        "pcloud-rs-tier2-windows-liveness-probe-{}-{}-{nanos}.bin",
        host,
        std::process::id()
    )
}

/// Append a record of an uploaded file_id to a local trace file so a
/// human can clean up later if the test panicked before the (stubbed)
/// deletefile IPC variant comes online. Best-effort only; failures to
/// write the trace must not mask the assertion that triggered cleanup.
fn record_uploaded_id(file_id: u64, remote_name: &str) {
    let trace_dir = env::temp_dir().join("pcloud-live-e2e-traces");
    let _ = fs::create_dir_all(&trace_dir);
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(trace_dir.join("uploaded_file_ids.log"))
    {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let _ = writeln!(f, "{} {} {} (windows_liveness)", ts, file_id, remote_name);
    }
}

/// RAII guard: records the uploaded file_id on drop so a mid-test
/// panic still leaves a breadcrumb for the human operator.
struct UploadedBlobGuard {
    file_id: Option<u64>,
    remote_name: String,
}

impl UploadedBlobGuard {
    fn new(remote_name: String) -> Self {
        Self {
            file_id: None,
            remote_name,
        }
    }

    fn set_file_id(&mut self, id: u64) {
        self.file_id = Some(id);
    }
}

impl Drop for UploadedBlobGuard {
    fn drop(&mut self) {
        if let Some(id) = self.file_id {
            record_uploaded_id(id, &self.remote_name);
        }
    }
}

/// RAII guard: remove the daemon's temp root on drop. We deliberately
/// do this separately from `TestDaemon::Drop` because the transfers
/// phase consumes the `TestDaemon` into an `EmbeddedDaemon` and we
/// still want the root deleted if the SDK phase panics.
struct TempRootGuard(PathBuf);

impl Drop for TempRootGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + PCLOUD_USERNAME/PCLOUD_PASSWORD (or PCLOUD_TEST_USER/PCLOUD_TEST_PASSWORD)"]
fn windows_liveness_login_userinfo_listfolder_roundtrip_logout() {
    if !gate_enabled() {
        let _ = skip_if_not_live(&[]);
        return;
    }
    // Accept either credential bundle. A token shortcut is honoured if
    // present so CI runners that only have a scoped token can still
    // drive this path.
    let have_user_pass = liveness_username().is_some() && liveness_password().is_some();
    let have_token = optional_env(ENV_TOKEN).is_some();
    if !(have_user_pass || have_token) {
        eprintln!(
            "[live-e2e] skipping windows_liveness: need either PCLOUD_USERNAME+PCLOUD_PASSWORD \
             (or PCLOUD_TEST_USER+PCLOUD_TEST_PASSWORD) or {ENV_TOKEN}"
        );
        return;
    }

    let overall_start = Instant::now();

    // === Phase 1: bootstrap daemon + login ==================================
    let mut daemon = TestDaemon::new("windows-liveness");

    let login_start = Instant::now();
    let login_resp = if let Some(token) = optional_env(ENV_TOKEN) {
        daemon.dispatch(Request::AuthTokenSubmission {
            value: token.into(),
        })
    } else {
        daemon.dispatch(Request::PasswordSubmission {
            username: liveness_username().expect("gate checked"),
            value: liveness_password().expect("gate checked").into(),
        })
    };
    let login_ms = login_start.elapsed().as_millis();
    assert_no_secret_leak(&login_resp);

    if login_resp.status != ResponseStatus::Ok {
        panic!(
            "live login failed: status={} message={}",
            status_label(&login_resp.status),
            login_resp.message
        );
    }
    if daemon.session_state() != SessionState::Authenticated {
        eprintln!(
            "[live-e2e] account requires TFA (state={:?}); \
             skipping windows_liveness. Set {} to a pre-provisioned token instead.",
            daemon.session_state(),
            ENV_TOKEN
        );
        return;
    }
    assert!(
        daemon.is_authenticated(),
        "auth token should be recorded after login"
    );
    eprintln!(
        "[live-e2e] windows_liveness: login OK in {} ms (first API round-trip)",
        login_ms
    );

    // === Phase 2: userinfo ==================================================
    let userinfo_start = Instant::now();
    let userinfo = expect_ok(
        &mut daemon,
        Request::Plain {
            method: Method::GetUserInfo,
        },
        "userinfo",
    );
    let userinfo_ms = userinfo_start.elapsed().as_millis();
    assert_no_secret_leak(&userinfo);
    let snapshot = daemon.runtime.auth.snapshot();
    let email = snapshot
        .email
        .clone()
        .expect("userinfo should populate session email");
    // If the operator fed in a specific username, sanity-check it
    // matches the account we're logged in as. Only applies when the
    // username looks like an email (pCloud also accepts legacy user
    // ids).
    if let Some(user) = liveness_username() {
        if user.contains('@') {
            assert_eq!(
                email.to_ascii_lowercase(),
                user.to_ascii_lowercase(),
                "userinfo email must match the credential we logged in with"
            );
        }
    }
    eprintln!(
        "[live-e2e] windows_liveness: userinfo OK in {} ms (email-length={})",
        userinfo_ms,
        email.len()
    );

    // === Phase 3: listfolder on the remote root =============================
    // listfolder is not on the IPC Request surface yet, but is
    // reachable through the SDK's folder_runtime. We hand the session
    // off to an EmbeddedDaemon that re-authenticates in-place so the
    // listing runs against the same live account.
    let root_dir = daemon
        .config
        .paths
        .config_dir
        .parent()
        .expect("managed paths share a root")
        .to_path_buf();
    drop(daemon); // Release the runtime so EmbeddedDaemon can take the root.
    // Guard ensures cleanup even if a later phase panics. TestDaemon's
    // own Drop already cleared it, but creating a new directory means
    // we re-own the cleanup responsibility.
    let _root_guard = TempRootGuard(root_dir.clone());

    let mut sdk = EmbeddedDaemon::builder(root_dir.clone())
        .build()
        .expect("SDK bootstrap under the same root should succeed");

    let reauth_start = Instant::now();
    let reauth = if let Some(token) = optional_env(ENV_TOKEN) {
        sdk.dispatch(Request::AuthTokenSubmission {
            value: token.into(),
        })
    } else {
        sdk.dispatch(Request::PasswordSubmission {
            username: liveness_username().expect("gate checked"),
            value: liveness_password().expect("gate checked").into(),
        })
    };
    assert_no_secret_leak(&reauth);
    assert_eq!(
        reauth.status,
        ResponseStatus::Ok,
        "SDK re-auth failed: {}",
        reauth.message
    );
    if !sdk.is_authenticated() {
        eprintln!(
            "[live-e2e] account requires TFA post-reauth; skipping SDK phase. \
             Provide {ENV_TOKEN} to drive this test on TFA-enforced accounts."
        );
        return;
    }
    eprintln!(
        "[live-e2e] windows_liveness: SDK re-auth OK in {} ms",
        reauth_start.elapsed().as_millis()
    );

    let listfolder_start = Instant::now();
    let entries = sdk
        .list_folder("/")
        .expect("listfolder on root should succeed against live backend");
    let listfolder_ms = listfolder_start.elapsed().as_millis();
    // An empty account would still return Ok with 0 entries, so don't
    // assert len > 0 — just that the call succeeded. Log the count for
    // the report.
    eprintln!(
        "[live-e2e] windows_liveness: listfolder / OK in {} ms ({} entries)",
        listfolder_ms,
        entries.len()
    );

    // === Phase 4: upload / download round-trip ==============================
    let scratch_path = scratch_folder();
    let remote_name = unique_blob_name();
    let payload: &[u8] = b"pcloud-rs-tier2-windows-liveness-probe";
    assert_eq!(payload.len(), 38, "fixed probe payload length");

    let mut upload_guard = UploadedBlobGuard::new(remote_name.clone());

    let upload_start = Instant::now();
    let uploaded = sdk
        .upload_data_as(&scratch_path, remote_name.clone(), payload)
        .expect("upload_data_as against live backend");
    let upload_ms = upload_start.elapsed().as_millis();
    assert_eq!(uploaded.bytes_uploaded, payload.len());
    assert_eq!(uploaded.remote_filename, remote_name);

    // `UploadSession.file_id` is populated by `upload_create` when the
    // server pre-allocates an id for an existing-path overwrite, but
    // for a brand-new filename (which is the case here by
    // construction) the id is only discoverable after `upload_save`.
    // Recover it via a listfolder probe on the scratch folder. The
    // entry we just wrote MUST be present.
    let file_id = if let Some(id) = uploaded.file_id {
        id
    } else {
        let listing = sdk
            .list_folder(&scratch_path)
            .expect("post-upload listfolder should succeed");
        let hit = listing
            .iter()
            .find(|e| !e.is_folder && e.name == remote_name)
            .unwrap_or_else(|| {
                panic!(
                    "uploaded blob {remote_name:?} not found under {scratch_path:?} \
                     after upload_save (listfolder returned {} entries)",
                    listing.len()
                )
            });
        hit.file_id.unwrap_or_else(|| {
            panic!(
                "scratch listfolder returned entry for {remote_name:?} without a file_id"
            )
        })
    };
    upload_guard.set_file_id(file_id);
    eprintln!(
        "[live-e2e] windows_liveness: upload OK in {} ms (file_id={}, {} bytes)",
        upload_ms,
        file_id,
        payload.len()
    );

    let download_start = Instant::now();
    let downloaded = sdk
        .download_file(file_id)
        .expect("download_file against live backend");
    let download_ms = download_start.elapsed().as_millis();
    assert_eq!(
        downloaded.as_slice(),
        payload,
        "round-tripped bytes must match the uploaded payload byte-for-byte"
    );
    eprintln!(
        "[live-e2e] windows_liveness: download OK in {} ms ({} bytes, byte-identical)",
        download_ms,
        downloaded.len()
    );

    // SDK-level delete_file is still stubbed under bd-1du.10; the
    // guard above writes the file_id to the trace log so an operator
    // can clean the shared scratch folder later. We intentionally do
    // not fail the test on cleanup absence — doing so would be a
    // false negative against the core liveness signal.
    //
    // Dropping `upload_guard` here — before the logout probe — ensures
    // the id is recorded regardless of what happens next.
    drop(upload_guard);

    // === Phase 5: logout ====================================================
    // The SDK's authenticated session state is distinct from the
    // daemon's; go back through a dedicated TestDaemon to exercise
    // Method::Logout + post-logout gating (the core liveness signal).
    drop(sdk);

    let mut daemon2 = TestDaemon::new("windows-liveness-logout");
    let login2_resp = if let Some(token) = optional_env(ENV_TOKEN) {
        daemon2.dispatch(Request::AuthTokenSubmission {
            value: token.into(),
        })
    } else {
        daemon2.dispatch(Request::PasswordSubmission {
            username: liveness_username().expect("gate checked"),
            value: liveness_password().expect("gate checked").into(),
        })
    };
    assert_no_secret_leak(&login2_resp);
    assert_eq!(
        login2_resp.status,
        ResponseStatus::Ok,
        "re-login for logout phase failed: {}",
        login2_resp.message
    );
    if daemon2.session_state() != SessionState::Authenticated {
        eprintln!(
            "[live-e2e] logout phase skipped: daemon2 state={:?} (TFA?)",
            daemon2.session_state()
        );
        return;
    }

    let logout_start = Instant::now();
    let logout = expect_ok(
        &mut daemon2,
        Request::Plain {
            method: Method::Logout,
        },
        "logout",
    );
    let logout_ms = logout_start.elapsed().as_millis();
    assert_no_secret_leak(&logout);
    assert!(
        !daemon2.is_authenticated(),
        "auth token must be cleared after Logout"
    );
    assert_eq!(
        daemon2.session_state(),
        SessionState::LoggedOut,
        "session state must be LoggedOut after Logout"
    );
    eprintln!(
        "[live-e2e] windows_liveness: logout OK in {} ms",
        logout_ms
    );

    eprintln!(
        "[live-e2e] windows_liveness: ALL PHASES OK in {} ms total",
        overall_start.elapsed().as_millis()
    );
}
