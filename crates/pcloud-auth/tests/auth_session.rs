#![allow(clippy::pedantic)]
//! Integration tests for the `pcloud-auth` session state machine.
//!
//! These tests exercise the public API surface of [`pcloud_auth`] without
//! any network I/O. All transitions drive the I/O-free
//! [`SessionManager::apply`] entry point.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_auth::{AuthCommand, AuthEvent, SessionManager, SessionManagerError, SessionState};
use pcloud_secret::secret_string::SecretString;

// ── helpers ──────────────────────────────────────────────────────────────────

fn make_secret(s: &str) -> SecretString {
    SecretString::new(s)
}

// ── initial state ─────────────────────────────────────────────────────────────

#[test]
fn initial_state_is_logged_out() {
    let manager = SessionManager::new();
    assert_eq!(manager.snapshot().state, SessionState::LoggedOut);
    assert!(manager.snapshot().auth_token.is_none());
    assert!(manager.snapshot().authenticated_user.is_none());
    assert!(manager.snapshot().pending_challenge.is_none());
    assert!(manager.snapshot().last_auth_error.is_none());
}

#[test]
fn default_matches_new() {
    let a = SessionManager::new();
    let b = SessionManager::default();
    assert_eq!(a.snapshot().state, b.snapshot().state);
}

// ── BeginLogin ────────────────────────────────────────────────────────────────

#[test]
fn begin_login_transitions_to_awaiting_credentials() {
    let mut manager = SessionManager::new();
    let event = manager
        .apply(AuthCommand::BeginLogin)
        .expect("BeginLogin should succeed");
    assert_eq!(event, AuthEvent::LoginStarted);
    assert_eq!(manager.snapshot().state, SessionState::AwaitingCredentials);
}

// ── LoginWithPassword ─────────────────────────────────────────────────────────

#[test]
fn login_with_password_transitions_to_authenticating() {
    let mut manager = SessionManager::new();
    let event = manager
        .apply(AuthCommand::LoginWithPassword {
            username: "alice@example.com".to_owned(),
            password: make_secret("hunter2"),
        })
        .expect("LoginWithPassword should succeed");
    assert_eq!(event, AuthEvent::LoginStarted);
    assert_eq!(
        manager.snapshot().state,
        SessionState::AuthenticatingWithPassword
    );
}

// ── LoginWithToken ────────────────────────────────────────────────────────────

#[test]
fn login_with_token_transitions_to_authenticating_with_token() {
    let mut manager = SessionManager::new();
    let event = manager
        .apply(AuthCommand::LoginWithToken {
            token: make_secret("tok-abc"),
        })
        .expect("LoginWithToken should succeed");
    assert_eq!(event, AuthEvent::LoginStarted);
    assert_eq!(
        manager.snapshot().state,
        SessionState::AuthenticatingWithToken
    );
}

// ── MarkAuthenticated ─────────────────────────────────────────────────────────

#[test]
fn mark_authenticated_from_authenticating_with_password_succeeds() {
    let mut manager = SessionManager::new();
    manager
        .apply(AuthCommand::LoginWithPassword {
            username: "alice@example.com".to_owned(),
            password: make_secret("hunter2"),
        })
        .expect("start login");

    let event = manager
        .apply(AuthCommand::MarkAuthenticated {
            user_id: None,
            auth_token: make_secret("live-tok"),
        })
        .expect("MarkAuthenticated should succeed");
    assert!(matches!(event, AuthEvent::LoginSucceeded { .. }));
    assert_eq!(manager.snapshot().state, SessionState::Authenticated);
    assert!(manager.snapshot().auth_token.is_some());
}

#[test]
fn mark_authenticated_from_logged_out_returns_error() {
    let mut manager = SessionManager::new();
    let err = manager
        .apply(AuthCommand::MarkAuthenticated {
            user_id: None,
            auth_token: make_secret("tok"),
        })
        .expect_err("transition from LoggedOut should be rejected");
    assert_eq!(err, SessionManagerError::InvalidAuthenticatedTransition);
    // Snapshot must be unchanged.
    assert_eq!(manager.snapshot().state, SessionState::LoggedOut);
}

// ── TwoFactor flow ────────────────────────────────────────────────────────────

#[test]
fn two_factor_required_after_challenge_issued() {
    let mut manager = SessionManager::new();
    manager
        .apply(AuthCommand::LoginWithPassword {
            username: "bob@example.com".to_owned(),
            password: make_secret("s3cr3t"),
        })
        .expect("start login");

    let event = manager.issue_two_factor_challenge(make_secret("challenge-token"), false);
    assert_eq!(event, AuthEvent::TwoFactorChallengeIssued);
    assert_eq!(manager.snapshot().state, SessionState::TwoFactorRequired);
    assert!(manager.snapshot().pending_challenge.is_some());
}

#[test]
fn submit_tfa_code_when_challenge_present_transitions_to_authenticating() {
    let mut manager = SessionManager::new();
    manager
        .apply(AuthCommand::LoginWithPassword {
            username: "bob@example.com".to_owned(),
            password: make_secret("s3cr3t"),
        })
        .expect("start login");
    manager.issue_two_factor_challenge(make_secret("challenge-token"), false);

    let event = manager
        .apply(AuthCommand::SubmitTwoFactorCode {
            code: make_secret("123456"),
            trust_device: false,
        })
        .expect("SubmitTwoFactorCode should succeed");
    assert_eq!(event, AuthEvent::LoginStarted);
    assert_eq!(
        manager.snapshot().state,
        SessionState::AuthenticatingWithPassword
    );
}

#[test]
fn submit_tfa_code_without_pending_challenge_returns_error() {
    let mut manager = SessionManager::new();
    // No challenge installed — the state machine is still LoggedOut.
    let err = manager
        .apply(AuthCommand::SubmitTwoFactorCode {
            code: make_secret("000000"),
            trust_device: false,
        })
        .expect_err("SubmitTwoFactorCode without challenge should fail");
    assert_eq!(err, SessionManagerError::NoPendingChallenge);
    // State machine must be unchanged.
    assert_eq!(manager.snapshot().state, SessionState::LoggedOut);
}

#[test]
fn tfa_code_can_complete_to_authenticated() {
    let mut manager = SessionManager::new();
    manager
        .apply(AuthCommand::LoginWithPassword {
            username: "carol@example.com".to_owned(),
            password: make_secret("pass"),
        })
        .expect("start login");
    manager.issue_two_factor_challenge(make_secret("srv-tok"), false);
    manager
        .apply(AuthCommand::SubmitTwoFactorCode {
            code: make_secret("654321"),
            trust_device: true,
        })
        .expect("SubmitTwoFactorCode should succeed");

    // Simulate the server responding OK after we submitted the code.
    let event = manager
        .apply(AuthCommand::MarkAuthenticated {
            user_id: None,
            auth_token: make_secret("auth-tok"),
        })
        .expect("MarkAuthenticated should succeed");
    assert!(matches!(event, AuthEvent::LoginSucceeded { .. }));
    assert_eq!(manager.snapshot().state, SessionState::Authenticated);
}

#[test]
fn snapshot_clone_duplicates_secret_fields_without_changing_state() {
    use pcloud_auth::{PendingChallenge, SessionSnapshot};
    use pcloud_model::ids::UserId;
    use pcloud_secret::ExposeSecret;

    let snapshot = SessionSnapshot {
        state: SessionState::TwoFactorRequired,
        authenticated_user: Some(UserId::new(19)),
        auth_token: Some(make_secret("auth-token")),
        email: Some("clone@example.test".to_owned()),
        pending_challenge: Some(PendingChallenge {
            token: make_secret("challenge-token"),
            trust_device: true,
        }),
        last_auth_error: Some("retry".to_owned()),
    };
    let cloned = snapshot.clone();
    assert_eq!(cloned, snapshot);
    assert_eq!(
        cloned.auth_token.as_ref().unwrap().expose_secret(),
        "auth-token"
    );
    assert_eq!(
        cloned
            .pending_challenge
            .as_ref()
            .unwrap()
            .token
            .expose_secret(),
        "challenge-token"
    );
}

// ── Logout ────────────────────────────────────────────────────────────────────

#[test]
fn logout_from_authenticated_transitions_to_logged_out_and_clears_token() {
    let mut manager = SessionManager::new();
    manager
        .apply(AuthCommand::LoginWithPassword {
            username: "dave@example.com".to_owned(),
            password: make_secret("pass"),
        })
        .expect("start login");
    manager
        .apply(AuthCommand::MarkAuthenticated {
            user_id: None,
            auth_token: make_secret("tok"),
        })
        .expect("mark authenticated");

    let event = manager
        .apply(AuthCommand::Logout)
        .expect("Logout should succeed");
    assert_eq!(event, AuthEvent::LoggedOut);
    assert_eq!(manager.snapshot().state, SessionState::LoggedOut);
    assert!(manager.snapshot().auth_token.is_none());
    assert!(manager.snapshot().pending_challenge.is_none());
}

// ── revoke ────────────────────────────────────────────────────────────────────

#[test]
fn revoke_zeroizes_token_and_transitions_to_logged_out() {
    let mut manager = SessionManager::new();
    manager
        .apply(AuthCommand::LoginWithToken {
            token: make_secret("tok-revoke"),
        })
        .expect("start");
    manager
        .apply(AuthCommand::MarkAuthenticated {
            user_id: None,
            auth_token: make_secret("live"),
        })
        .expect("authenticate");

    let event = manager.revoke();
    assert_eq!(event, AuthEvent::LoggedOut);
    assert_eq!(manager.snapshot().state, SessionState::LoggedOut);
    assert!(manager.snapshot().auth_token.is_none());
}

// ── MarkAuthenticationFailed ──────────────────────────────────────────────────

#[test]
fn mark_auth_failed_transitions_to_auth_failed_and_records_error() {
    let mut manager = SessionManager::new();
    manager
        .apply(AuthCommand::LoginWithPassword {
            username: "eve@example.com".to_owned(),
            password: make_secret("bad"),
        })
        .expect("start login");

    let event = manager
        .apply(AuthCommand::MarkAuthenticationFailed {
            message: Some("invalid credentials".to_owned()),
        })
        .expect("MarkAuthenticationFailed should succeed");
    assert!(matches!(event, AuthEvent::LoginFailed { .. }));
    assert_eq!(manager.snapshot().state, SessionState::AuthFailed);
    assert_eq!(
        manager.snapshot().last_auth_error.as_deref(),
        Some("invalid credentials")
    );
    assert!(manager.snapshot().auth_token.is_none());
}

// ── replace_auth_token ────────────────────────────────────────────────────────

#[test]
fn replace_auth_token_requires_authenticated_state() {
    let mut manager = SessionManager::new();
    let err = manager
        .replace_auth_token(make_secret("tok"))
        .expect_err("replace from LoggedOut should fail");
    assert_eq!(err, SessionManagerError::InvalidAuthenticatedTransition);
}
