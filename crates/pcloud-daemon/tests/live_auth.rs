#![allow(clippy::pedantic)]
// Audit finding L1: this test harness intentionally references live-auth env
// var NAMES in `eprintln!` skip messages (e.g. "PCLOUD_LIVE_PASSWORD is not
// set"). The env var VALUES must never be printed, even in debug output.
// Do not replace `optional_env(name)` sites with `env::var(name).unwrap()`
// style formatting — that would leak the secret through stderr.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::{
    env, fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use pcloud_auth::SessionState;
use pcloud_config::{ConfigProfile, Environment, env::apply_env_overrides};
use pcloud_daemon::{bootstrap_with_config, dispatch};
use pcloud_ipc::{Method, Request, ResponseStatus};

fn unique_live_root() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nonce = SEQ.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("pcloud-live-auth-{}-{nonce}", std::process::id()))
}

fn live_config() -> pcloud_config::ConfigProfile {
    let root = unique_live_root();
    apply_env_overrides(ConfigProfile::secure_defaults(
        root,
        Environment::Production,
    ))
    .expect("live config should parse env overrides")
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn cleanup_root(config: &ConfigProfile) {
    let root = config
        .paths
        .config_dir
        .parent()
        .expect("managed paths should share a root")
        .to_path_buf();
    let _ = fs::remove_dir_all(root);
}

fn assert_authenticated_userinfo(runtime: &mut pcloud_daemon::RuntimeShell) {
    let userinfo = dispatch(
        runtime,
        Request::Plain {
            method: Method::GetUserInfo,
        },
    );
    assert_eq!(
        userinfo.status,
        ResponseStatus::Ok,
        "userinfo failed: {}",
        userinfo.message
    );
    assert!(runtime.auth.snapshot().authenticated_user.is_some());
    assert!(runtime.auth.snapshot().email.is_some());
}

#[test]
#[ignore = "requires a real pCloud auth token via PCLOUD_LIVE_AUTH_TOKEN"]
fn live_token_auth_and_userinfo_succeed_against_production_path() {
    let Some(token) = optional_env("PCLOUD_LIVE_AUTH_TOKEN") else {
        eprintln!("skipping live auth test: PCLOUD_LIVE_AUTH_TOKEN is not set");
        return;
    };
    let config = live_config();
    let mut runtime =
        bootstrap_with_config(config.clone()).expect("runtime bootstrap should succeed");

    let auth = dispatch(
        &mut runtime,
        Request::AuthTokenSubmission {
            value: token.into(),
        },
    );
    assert_eq!(
        auth.status,
        ResponseStatus::Ok,
        "auth failed: {}",
        auth.message
    );
    assert_eq!(runtime.auth.snapshot().state, SessionState::Authenticated);
    assert_authenticated_userinfo(&mut runtime);
    cleanup_root(&config);
}

#[test]
#[ignore = "requires a real pCloud username/password via PCLOUD_LIVE_USERNAME and PCLOUD_LIVE_PASSWORD"]
fn live_password_auth_progresses_on_production_path() {
    let Some(username) = optional_env("PCLOUD_LIVE_USERNAME") else {
        eprintln!("skipping live password auth test: PCLOUD_LIVE_USERNAME is not set");
        return;
    };
    let Some(password) = optional_env("PCLOUD_LIVE_PASSWORD") else {
        eprintln!("skipping live password auth test: PCLOUD_LIVE_PASSWORD is not set");
        return;
    };

    let config = live_config();
    let mut runtime =
        bootstrap_with_config(config.clone()).expect("runtime bootstrap should succeed");

    let auth = dispatch(
        &mut runtime,
        Request::PasswordSubmission {
            username,
            value: password.into(),
        },
    );
    assert_eq!(
        auth.status,
        ResponseStatus::Ok,
        "password auth failed: {}",
        auth.message
    );
    assert!(
        matches!(
            runtime.auth.snapshot().state,
            SessionState::Authenticated | SessionState::TwoFactorRequired
        ),
        "unexpected session state after password auth: {:?}",
        runtime.auth.snapshot().state
    );

    if runtime.auth.snapshot().state == SessionState::Authenticated {
        assert_authenticated_userinfo(&mut runtime);
    }

    cleanup_root(&config);
}

#[test]
#[ignore = "requires a real pCloud TFA account via PCLOUD_LIVE_USERNAME, PCLOUD_LIVE_PASSWORD, and PCLOUD_LIVE_TFA_CODE or PCLOUD_LIVE_RECOVERY_CODE"]
fn live_password_tfa_auth_and_userinfo_succeed_against_production_path() {
    let Some(username) = optional_env("PCLOUD_LIVE_USERNAME") else {
        eprintln!("skipping live TFA auth test: PCLOUD_LIVE_USERNAME is not set");
        return;
    };
    let Some(password) = optional_env("PCLOUD_LIVE_PASSWORD") else {
        eprintln!("skipping live TFA auth test: PCLOUD_LIVE_PASSWORD is not set");
        return;
    };
    let tfa_code = optional_env("PCLOUD_LIVE_TFA_CODE");
    let recovery_code = optional_env("PCLOUD_LIVE_RECOVERY_CODE");
    let Some(code) = tfa_code.clone().or(recovery_code.clone()) else {
        eprintln!("skipping live TFA auth test: no TFA or recovery code env var is set");
        return;
    };

    let config = live_config();
    let mut runtime =
        bootstrap_with_config(config.clone()).expect("runtime bootstrap should succeed");

    let auth = dispatch(
        &mut runtime,
        Request::PasswordSubmission {
            username,
            value: password.into(),
        },
    );
    assert_eq!(
        auth.status,
        ResponseStatus::Ok,
        "password auth failed: {}",
        auth.message
    );
    if runtime.auth.snapshot().state != SessionState::TwoFactorRequired {
        eprintln!(
            "skipping live TFA completion test: account did not enter TwoFactorRequired, state={:?}",
            runtime.auth.snapshot().state
        );
        cleanup_root(&config);
        return;
    }

    let tfa = dispatch(
        &mut runtime,
        Request::TwoFactorCodeSubmission {
            value: code,
            trust_device: false,
            recovery_code: recovery_code.is_some(),
        },
    );
    assert_eq!(
        tfa.status,
        ResponseStatus::Ok,
        "TFA auth failed: {}",
        tfa.message
    );
    assert_eq!(runtime.auth.snapshot().state, SessionState::Authenticated);
    assert_authenticated_userinfo(&mut runtime);

    cleanup_root(&config);
}

#[test]
#[ignore = "opt-in live negative-path verification against the real pCloud service"]
fn live_invalid_password_is_rejected_on_production_path() {
    let config = live_config();
    let mut runtime =
        bootstrap_with_config(config.clone()).expect("runtime bootstrap should succeed");

    let auth = dispatch(
        &mut runtime,
        Request::PasswordSubmission {
            username: "nobody@example.invalid".to_owned(),
            value: "definitely-wrong-password".to_owned().into(),
        },
    );
    assert!(
        matches!(
            auth.status,
            ResponseStatus::Ok | ResponseStatus::Unavailable
        ),
        "unexpected invalid-auth status: {:?} {}",
        auth.status,
        auth.message
    );
    assert_eq!(runtime.auth.snapshot().state, SessionState::AuthFailed);

    cleanup_root(&config);
}
