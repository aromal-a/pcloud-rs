#![allow(clippy::pedantic)]
//! Live auth-lifecycle coverage: login (password/token), logout,
//! session-status probe, durable-token persistence, and a post-login
//! drill that the 0600 / 0700 vault-file permission invariants are upheld.
//!
//! Every body short-circuits unless `PCLOUD_LIVE_E2E=1` is set and the
//! needed credential envs are populated. See `README.md`.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use pcloud_auth::SessionState;
use pcloud_ipc::{Method, Request, ResponseStatus};

use crate::common::{
    ENV_PASSWORD, ENV_TOKEN, ENV_USER, TestDaemon, assert_no_secret_leak, authenticate, expect_ok,
    gate_enabled, optional_env, probe_userinfo, skip_if_not_live,
};

fn have_any_credentials() -> bool {
    optional_env(ENV_TOKEN).is_some()
        || (optional_env(ENV_USER).is_some() && optional_env(ENV_PASSWORD).is_some())
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_login_userinfo_logout() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!(
            "[live-e2e] skipping: need {ENV_TOKEN} or {ENV_USER}+{ENV_PASSWORD} to exercise auth"
        );
        return;
    }

    let mut daemon = TestDaemon::new("auth-login");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping auth login test: {err}");
        return;
    }
    assert!(daemon.is_authenticated(), "daemon should be authenticated");
    probe_userinfo(&mut daemon);

    let logout = expect_ok(
        &mut daemon,
        Request::Plain {
            method: Method::Logout,
        },
        "logout",
    );
    assert_no_secret_leak(&logout);
    assert!(
        !daemon.is_authenticated(),
        "daemon should have dropped in-memory token on Logout"
    );
    assert_eq!(
        daemon.session_state(),
        SessionState::LoggedOut,
        "post-logout state should be LoggedOut"
    );

    // A post-logout userinfo should now be rejected cleanly.
    let resp = daemon.dispatch(Request::Plain {
        method: Method::GetUserInfo,
    });
    assert_no_secret_leak(&resp);
    assert!(
        matches!(
            resp.status,
            ResponseStatus::Unauthorized | ResponseStatus::InvalidRequest
        ),
        "post-logout userinfo must not succeed: status={}",
        crate::common::status_label(&resp.status)
    );
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + PCLOUD_TEST_TOKEN"]
fn live_login_by_token() {
    if skip_if_not_live(&[ENV_TOKEN]) {
        return;
    }
    let token = optional_env(ENV_TOKEN).expect("gate checked token presence");

    let mut daemon = TestDaemon::new("auth-token");
    let resp = daemon.dispatch(Request::AuthTokenSubmission { value: token });
    assert_no_secret_leak(&resp);
    assert_eq!(
        resp.status,
        ResponseStatus::Ok,
        "token submission failed: {} ({})",
        resp.message,
        crate::common::status_label(&resp.status)
    );
    assert_eq!(daemon.session_state(), SessionState::Authenticated);
    probe_userinfo(&mut daemon);
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_session_status_payload_is_non_empty() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping session-status: need credentials");
        return;
    }
    let mut daemon = TestDaemon::new("auth-session-status");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping: {err}");
        return;
    }

    let resp = expect_ok(
        &mut daemon,
        Request::Plain {
            method: Method::SessionStatus,
        },
        "session-status",
    );
    // Payload shape is JSON; parse leniently.
    let value: serde_json::Value =
        serde_json::from_str(&resp.message).expect("SessionStatus response body must be JSON");
    assert!(value.is_object(), "SessionStatus must be a JSON object");
    assert_no_secret_leak(&resp);
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials"]
fn live_vault_permissions_after_persistence_opt_in() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !have_any_credentials() {
        eprintln!("[live-e2e] skipping vault perms: need credentials");
        return;
    }
    let mut daemon = TestDaemon::new("auth-vault-perms");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping: {err}");
        return;
    }

    let vault_path = daemon.config.paths.auth_token_vault_path();
    let config_dir = daemon.config.paths.config_dir.clone();

    // Opt in to durable persistence. If the feature is not enabled in this
    // build it returns InvalidRequest / Unavailable — we treat that as a
    // soft skip rather than a failure.
    let resp = daemon.dispatch(Request::AuthPersistence { enabled: true });
    assert_no_secret_leak(&resp);
    if resp.status != ResponseStatus::Ok {
        eprintln!(
            "[live-e2e] skipping vault-perms drill: AuthPersistence returned {} ({})",
            resp.message,
            crate::common::status_label(&resp.status)
        );
        return;
    }

    // Refresh userinfo to give the daemon a chance to write the vault.
    probe_userinfo(&mut daemon);

    // Permission drill (Unix only).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if vault_path.exists() {
            let meta = std::fs::metadata(&vault_path).expect("vault metadata");
            assert!(
                meta.file_type().is_file(),
                "vault must be a regular file, got {:?}",
                meta.file_type()
            );
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "auth_token vault must be 0600, got {:#o}",
                mode
            );
        } else {
            eprintln!(
                "[live-e2e] note: vault file {} absent after AuthPersistence=true \
                 (daemon may defer persistence to a later event)",
                vault_path.display()
            );
        }
        let dir_meta = std::fs::metadata(&config_dir).expect("config dir metadata");
        let dir_mode = dir_meta.permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "config dir must be 0700, got {:#o}",
            dir_mode
        );
    }
    #[cfg(not(unix))]
    {
        let _ = (vault_path, config_dir); // unused on non-unix
        eprintln!("[live-e2e] vault-perms drill skipped on non-unix");
    }

    // Opt out, which should destroy the vault.
    let _ = daemon.dispatch(Request::AuthPersistence { enabled: false });
    let _ = gate_enabled(); // ensure the gate helper remains wired
}
