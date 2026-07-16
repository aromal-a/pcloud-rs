#![allow(clippy::pedantic)]
//! Shared gating / bootstrap helpers for the live-E2E harness.
//!
//! All per-feature test binaries include this module via `mod common;`.
//!
//! Security invariants (must not regress):
//!
//! * No credential values are ever hardcoded. Every secret is read from the
//!   environment, and only when `PCLOUD_LIVE_E2E=1`.
//! * No secret is logged. We intentionally use `len()` summaries when we need
//!   to prove a secret was read.
//! * Each test scopes its daemon to a unique temp directory and deletes that
//!   directory on completion so persisted tokens/vaults never outlive the
//!   process.

#![allow(dead_code)]

// **PLATFORM:** all
// **GATING:** none (portable).

use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use pcloud_auth::SessionState;
use pcloud_config::{ConfigProfile, Environment, env::apply_env_overrides};
use pcloud_daemon::{RuntimeShell, bootstrap_with_config, dispatch};
use pcloud_ipc::{Method, Request, Response, ResponseStatus};

/// Environment-variable master gate.
pub const GATE_ENV: &str = "PCLOUD_LIVE_E2E";
/// Release-only hard-gate marker. When enabled, selected qualification tests
/// must fail instead of converting unavailable credentials/backend behavior
/// into an advisory skip.
pub const RELEASE_GATE_ENV: &str = "PCLOUD_RELEASE_GATE";

/// Required credential envs for the login phase.
pub const ENV_USER: &str = "PCLOUD_TEST_USER";
pub const ENV_PASSWORD: &str = "PCLOUD_TEST_PASSWORD";
pub const ENV_TOKEN: &str = "PCLOUD_TEST_TOKEN";

/// Optional TFA envs. At most one of these is required if the account has TFA.
pub const ENV_TFA_CODE: &str = "PCLOUD_TEST_TFA_CODE";
pub const ENV_RECOVERY_CODE: &str = "PCLOUD_TEST_RECOVERY_CODE";

/// Optional remote scratch folder ("/" by default). Tests that mutate the
/// remote account (upload/link/crypto/backup) will create objects under this
/// path and delete them afterwards.
pub const ENV_SCRATCH_FOLDER: &str = "PCLOUD_TEST_SCRATCH";

/// Optional crypto password for accounts that have crypto enabled. Only the
/// crypto tests read it.
pub const ENV_CRYPTO_PASSWORD: &str = "PCLOUD_TEST_CRYPTO_PASSWORD";

/// Secondary opt-in gate for tests that mutate the soak account in
/// observable ways: triggering an email send (`VerifyEmail`), rotating
/// the account password (`AccountChangePassword`), or any other action
/// the operator must explicitly accept the side-effect for.
///
/// `PCLOUD_LIVE_E2E=1` is the master gate. A destructive test
/// additionally requires `PCLOUD_LIVE_E2E_DESTRUCTIVE=1`. Tests that
/// only read, or that target IETF-reserved black-hole identifiers
/// (e.g. `@example.invalid`), do **not** need this gate.
pub const DESTRUCTIVE_GATE_ENV: &str = "PCLOUD_LIVE_E2E_DESTRUCTIVE";

/// Returns `true` when the master gate is explicitly enabled.
pub fn gate_enabled() -> bool {
    matches!(
        env::var(GATE_ENV).ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Return whether the caller is executing a release-blocking live gate.
#[must_use]
pub fn release_gate_enabled() -> bool {
    matches!(
        env::var(RELEASE_GATE_ENV).ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Returns `true` when both the master gate and the destructive gate
/// are explicitly enabled. Test bodies that mutate the soak account
/// (trigger emails, rotate passwords, etc.) call
/// [`skip_if_not_destructive`] to gate-skip when this returns `false`.
#[must_use]
pub fn destructive_gate_enabled() -> bool {
    gate_enabled()
        && matches!(
            env::var(DESTRUCTIVE_GATE_ENV).ok().as_deref(),
            Some("1") | Some("true") | Some("yes")
        )
}

/// Skip helper for destructive tests. Returns `true` and prints a
/// reason when either the master gate or the destructive gate is
/// missing. Test bodies should `return;` immediately when this returns
/// `true`.
#[must_use]
pub fn skip_if_not_destructive(required: &[&str]) -> bool {
    if skip_if_not_live(required) {
        return true;
    }
    if !destructive_gate_enabled() {
        eprintln!(
            "[live-e2e] skipping destructive test: {}=1 is not set. \
             This test mutates the soak account (email send / password rotation) — \
             enable only when an operator has agreed to the side effect.",
            DESTRUCTIVE_GATE_ENV
        );
        return true;
    }
    false
}

/// Reads an env var, treating empty strings as absent.
pub fn optional_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

/// Emits a structured "skipping" message and returns `true` when the live
/// gate or required credentials are missing. Test bodies should `return;`
/// immediately when this returns `true`.
#[must_use]
pub fn skip_if_not_live(required: &[&str]) -> bool {
    if !gate_enabled() {
        eprintln!(
            "[live-e2e] skipping test: {}=1 is not set. See crates/pcloud-live-e2e/README.md.",
            GATE_ENV
        );
        return true;
    }
    for key in required {
        if optional_env(key).is_none() {
            eprintln!("[live-e2e] skipping test: required env {key} is unset or empty");
            return true;
        }
    }
    false
}

static DAEMON_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_live_root(tag: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let seq = DAEMON_SEQ.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!(
        "pcloud-live-e2e-{tag}-{}-{nonce}-{seq}",
        std::process::id()
    ))
}

/// Build a fresh, production-profile daemon config under a unique temp root.
pub fn fresh_config(tag: &str) -> ConfigProfile {
    let root = unique_live_root(tag);
    apply_env_overrides(ConfigProfile::secure_defaults(
        root,
        Environment::Production,
    ))
    .expect("live config should parse env overrides")
}

/// Recursively removes the daemon root after a test completes.
pub fn cleanup_root(config: &ConfigProfile) {
    if let Some(parent) = config.paths.config_dir.parent() {
        let _ = fs::remove_dir_all(parent);
    }
}

/// A fully-constructed, fresh daemon runtime scoped to its own temp root.
pub struct TestDaemon {
    pub runtime: RuntimeShell,
    pub config: ConfigProfile,
}

impl TestDaemon {
    pub fn new(tag: &str) -> Self {
        let config = fresh_config(tag);
        let runtime = bootstrap_with_config(config.clone())
            .expect("daemon bootstrap under unique temp root should succeed");
        Self { runtime, config }
    }

    pub fn dispatch(&mut self, request: Request) -> Response {
        dispatch(&mut self.runtime, request)
    }

    pub fn session_state(&self) -> SessionState {
        self.runtime.auth.snapshot().state.clone()
    }

    pub fn is_authenticated(&self) -> bool {
        self.runtime.auth.snapshot().auth_token.is_some()
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        cleanup_root(&self.config);
    }
}

/// Authenticate the daemon against the real pCloud backend using whichever
/// credential bundle is available. Prefers `PCLOUD_TEST_TOKEN` when set;
/// otherwise falls back to username/password (+ optional TFA).
///
/// Returns `Ok(())` on `SessionState::Authenticated`.
pub fn authenticate(daemon: &mut TestDaemon) -> Result<(), String> {
    if let Some(token) = optional_env(ENV_TOKEN) {
        let resp = daemon.dispatch(Request::AuthTokenSubmission {
            value: token.into(),
        });
        if resp.status != ResponseStatus::Ok {
            return Err(format!(
                "token auth failed: {} ({})",
                resp.message,
                status_label(&resp.status)
            ));
        }
        if daemon.session_state() == SessionState::Authenticated {
            return Ok(());
        }
        return Err(format!(
            "token auth did not authenticate: state={:?}",
            daemon.session_state()
        ));
    }

    let user = optional_env(ENV_USER).ok_or_else(|| format!("missing {ENV_USER}"))?;
    let password = optional_env(ENV_PASSWORD).ok_or_else(|| format!("missing {ENV_PASSWORD}"))?;

    let resp = daemon.dispatch(Request::PasswordSubmission {
        username: user,
        value: password.into(),
    });
    if resp.status != ResponseStatus::Ok {
        return Err(format!(
            "password auth failed: {} ({})",
            resp.message,
            status_label(&resp.status)
        ));
    }

    match daemon.session_state() {
        SessionState::Authenticated => Ok(()),
        SessionState::TwoFactorRequired => {
            let tfa = optional_env(ENV_TFA_CODE);
            let rec = optional_env(ENV_RECOVERY_CODE);
            let Some(code) = tfa.clone().or_else(|| rec.clone()) else {
                return Err(format!(
                    "account requires TFA but neither {ENV_TFA_CODE} nor {ENV_RECOVERY_CODE} is set"
                ));
            };
            let resp = daemon.dispatch(Request::TwoFactorCodeSubmission {
                value: code,
                trust_device: false,
                recovery_code: rec.is_some(),
            });
            if resp.status != ResponseStatus::Ok {
                return Err(format!("TFA submission failed: {}", resp.message));
            }
            if daemon.session_state() != SessionState::Authenticated {
                return Err(format!(
                    "TFA completed but session is {:?}",
                    daemon.session_state()
                ));
            }
            Ok(())
        }
        other => Err(format!(
            "unexpected session state after password auth: {other:?}"
        )),
    }
}

/// Build an authenticated daemon or explicitly skip-with-message when the
/// harness is unable to authenticate against the backend.
///
/// Returns `None` when the test should short-circuit.
#[must_use]
pub fn authed_daemon(tag: &str) -> Option<TestDaemon> {
    let mut daemon = TestDaemon::new(tag);
    match authenticate(&mut daemon) {
        Ok(()) => Some(daemon),
        Err(err) => {
            eprintln!("[live-e2e] skipping test '{tag}': {err}");
            None
        }
    }
}

pub fn status_label(status: &ResponseStatus) -> &'static str {
    match status {
        ResponseStatus::Ok => "Ok",
        ResponseStatus::InvalidRequest => "InvalidRequest",
        ResponseStatus::Unauthorized => "Unauthorized",
        ResponseStatus::Conflict => "Conflict",
        ResponseStatus::Unavailable => "Unavailable",
        ResponseStatus::InternalError => "InternalError",
        ResponseStatus::PolicyViolation { .. } => "PolicyViolation",
        _ => "Unknown",
    }
}

/// Assert the response message never contains anything that looks like a raw
/// secret value we fed in. We intentionally scan for the live password and
/// token strings so a regression in error-formatting is caught here rather
/// than silently exposed.
pub fn assert_no_secret_leak(response: &Response) {
    for (env_var, label) in [
        (ENV_PASSWORD, "password"),
        (ENV_TOKEN, "auth-token"),
        (ENV_CRYPTO_PASSWORD, "crypto-password"),
        (ENV_TFA_CODE, "tfa-code"),
        (ENV_RECOVERY_CODE, "recovery-code"),
    ] {
        if let Some(secret) = optional_env(env_var) {
            assert!(
                !response.message.contains(&secret),
                "response message leaked {label} value (from ${env_var})"
            );
        }
    }
}

/// Convenience: dispatch + secret-leak check + status assertion.
pub fn expect_ok(daemon: &mut TestDaemon, request: Request, what: &str) -> Response {
    let resp = daemon.dispatch(request);
    assert_no_secret_leak(&resp);
    assert_eq!(
        resp.status,
        ResponseStatus::Ok,
        "{what} failed: status={} message={}",
        status_label(&resp.status),
        resp.message
    );
    resp
}

pub fn scratch_folder() -> String {
    optional_env(ENV_SCRATCH_FOLDER).unwrap_or_else(|| "/".to_owned())
}

/// Best-effort: compute the sidecar path for a snapshot archive. The
/// daemon's snapshot layer writes a sidecar next to the archive whose
/// extension is the archive's filename plus `.sha3`. Used by test
/// cleanup only; never load-bearing.
pub fn sidecar_path(archive: &std::path::Path) -> std::path::PathBuf {
    let mut s: std::ffi::OsString = archive.as_os_str().to_owned();
    s.push(".sha3");
    std::path::PathBuf::from(s)
}

/// Utility for test bodies that need a `userinfo` sanity probe after auth.
pub fn probe_userinfo(daemon: &mut TestDaemon) {
    let resp = daemon.dispatch(Request::Plain {
        method: Method::GetUserInfo,
    });
    assert_no_secret_leak(&resp);
    assert_eq!(
        resp.status,
        ResponseStatus::Ok,
        "post-auth userinfo failed: {}",
        resp.message
    );
}

// ── AccountChangePassword marker-file recovery (CLAUDEREV deferred-set
// D2.1, fire 46) ────────────────────────────────────────────────────────
//
// The destructive `AccountChangePassword` round-trip rotates the soak
// account's password from `original` → `temp` → `original`. A flake
// between the two rotations would leave the account password stuck at
// `temp` with no way for a fresh test process to recover. Solution: a
// marker file under $TMPDIR persists `(original, temp, phase)` so the
// next invocation can detect a half-rotated state and roll forward.
//
// Privacy: the marker contains both passwords in plaintext at mode
// 0600 under TMPDIR. That's the same disclosure surface as
// `PCLOUD_TEST_PASSWORD` in the env, so net new exposure is zero.
//
// The file is keyed on a non-cryptographic hash of the user email so
// concurrent CI runs against different accounts get separate markers.

/// Persisted state for the `AccountChangePassword` round-trip recovery.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AcpRotationMarker {
    /// Original (env-supplied) password.
    pub original: String,
    /// Temporary password the test rotated to.
    pub temp: String,
    /// Pipeline phase the marker was written at.
    pub phase: AcpPhase,
}

/// Pipeline phases for the AccountChangePassword round-trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AcpPhase {
    /// `original → temp` rotation has been dispatched and acknowledged.
    /// On recovery we know `temp` is the live password and need to
    /// rotate back to `original`.
    RotatedToTemp,
}

/// Compute the marker-file path for `user_email`. Uses a fast non-
/// cryptographic hash so concurrent runs against different accounts
/// produce different paths; the marker file content is the secret
/// envelope, not the path.
pub fn acp_marker_path(user_email: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    user_email.hash(&mut hasher);
    let h = hasher.finish();
    env::temp_dir().join(format!("pcloud-rs-acp-marker-{h:016x}"))
}

/// Read the marker file if present and parsable.
pub fn read_acp_marker(path: &Path) -> Option<AcpRotationMarker> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Persist the marker file at mode 0600 (Unix). Returns the resulting
/// path on success.
pub fn write_acp_marker(path: &Path, marker: &AcpRotationMarker) -> Result<(), String> {
    let bytes = serde_json::to_vec(marker).map_err(|e| format!("serialize marker: {e}"))?;
    fs::write(path, bytes).map_err(|e| format!("write marker {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod marker {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Best-effort delete. Errors are intentionally ignored — the next
/// run will recover from a stale marker via [`read_acp_marker`].
pub fn delete_acp_marker(path: &Path) {
    let _ = std::fs::remove_file(path);
}
