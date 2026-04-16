//! [`SessionManager`] — owns the auth state machine, the live auth
//! token, and optional session-lifecycle timers.
//!
//! The manager is I/O-free: it receives [`AuthCommand`]s and emits
//! [`AuthEvent`]s. All external interactions (HTTP to pCloud, vault
//! persistence, IPC) live behind [`crate::orchestrator::ProtocolAuthFlow`].
//! This separation keeps every state transition unit-testable without a
//! network and removes any transport-coupled code path that could leak
//! a credential.
//!
//! # State machine
//!
//! See the crate-level docs for the ASCII diagram. The authoritative
//! transition table lives in [`SessionManager::apply`]; lifecycle-aware
//! transitions (refresh, idle logout, hard expiry) live in
//! [`crate::refresh::RefreshCoordinator`].
//!
//! # Secret handling (ADR 0007)
//!
//! * The in-memory `auth_token` is a [`SecretString`] — zeroized on
//!   drop, redacted in `Debug`, and compared in constant time.
//! * [`SessionManager::revoke`] takes the `SecretString` out of the
//!   snapshot so its `Drop` runs and scrubs the heap buffer.
//! * [`SessionManager::replace_auth_token`] and
//!   [`SessionManager::record_refresh`] overwrite the `Option<SecretString>`
//!   slot; the previous value is dropped (and therefore zeroized) as
//!   part of the replacement.
//! * The user's password is never stored on this type. Password bytes
//!   reach the manager only as a transient field of
//!   [`AuthCommand::LoginWithPassword`] and are forwarded to the
//!   orchestrator without being retained.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_model::ids::UserId;
use pcloud_secret::secret_string::SecretString;
use thiserror::Error;

use crate::{
    commands::AuthCommand,
    events::AuthEvent,
    lifecycle::{RefreshPolicy, SessionLifecycle},
    state::{PendingChallenge, SessionSnapshot, SessionState},
};

/// Owner of the auth state machine and the in-memory auth token.
///
/// A `SessionManager` is driven by [`SessionManager::apply`] and
/// inspected via [`SessionManager::snapshot`]. Secrets held inside the
/// snapshot are zeroized either when the manager is dropped (via
/// [`SecretString`]'s `ZeroizeOnDrop` impl) or when
/// [`SessionManager::revoke`] explicitly moves the token out of the
/// snapshot to force an early scrub.
///
/// # State transitions
///
/// | From                                | Command                               | To                                  |
/// |-------------------------------------|---------------------------------------|-------------------------------------|
/// | `LoggedOut`                         | `BeginLogin`                          | `AwaitingCredentials`               |
/// | any                                 | `LoginWithPassword`                   | `AuthenticatingWithPassword`        |
/// | any                                 | `LoginWithToken`                      | `AuthenticatingWithToken`           |
/// | `TwoFactorRequired`                 | `SubmitTwoFactorCode`                 | `AuthenticatingWithPassword` (retry)|
/// | `AuthenticatingWith{Password,Token}`| `MarkAuthenticated`                   | `Authenticated`                     |
/// | `TwoFactorRequired`                 | `MarkAuthenticated`                   | `Authenticated`                     |
/// | any                                 | `MarkAuthenticationFailed`            | `AuthFailed` (creds cleared)        |
/// | `TwoFactorRequired`                 | `MarkTwoFactorCodeInvalid`            | `TwoFactorRequired` (challenge kept)|
/// | any                                 | `Logout`                              | `LoggedOut` (token zeroized)        |
///
/// Invalid transitions return
/// [`SessionManagerError::InvalidAuthenticatedTransition`] and leave the
/// snapshot unchanged.
#[derive(Debug)]
pub struct SessionManager {
    snapshot: SessionSnapshot,
    lifecycle: Option<SessionLifecycle>,
}

/// Errors produced by the [`SessionManager`] state machine.
///
/// All variants are **programmer errors** (bad transitions) rather than
/// user-facing authentication failures — a real 2FA rejection or
/// expired-token response surfaces as an [`AuthEvent`] instead. Treat
/// these as bugs to fix in the calling orchestration code.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SessionManagerError {
    /// A 2FA code was submitted while no [`PendingChallenge`] was
    /// attached to the snapshot.
    ///
    /// * **Cause**: `SubmitTwoFactorCode` applied outside
    ///   [`SessionState::TwoFactorRequired`], or after the challenge
    ///   was cleared by a prior `Logout` / `MarkAuthenticationFailed`.
    /// * **Recoverability**: not recoverable in-place. The caller must
    ///   restart the login flow from
    ///   [`AuthCommand::LoginWithPassword`] / `LoginWithToken` so the
    ///   server issues a fresh challenge.
    /// * **Retry guidance**: do not retry with the same command; the
    ///   server-side challenge token is no longer addressable.
    #[error("two-factor code was submitted while no challenge is pending")]
    NoPendingChallenge,
    /// A state transition was requested from a state that does not
    /// allow it (e.g. replacing the auth token while `LoggedOut`, or
    /// marking the session authenticated without a login in flight).
    ///
    /// * **Cause**: orchestration bug — the caller invoked
    ///   [`SessionManager::replace_auth_token`],
    ///   [`SessionManager::update_userinfo`],
    ///   [`SessionManager::record_refresh`], or applied
    ///   [`AuthCommand::MarkAuthenticated`] from a non-authenticating
    ///   state.
    /// * **Recoverability**: the snapshot is left unchanged; there is
    ///   no state corruption. The caller should inspect its own control
    ///   flow — this should never be reached at runtime in correct
    ///   code.
    /// * **Retry guidance**: do not retry; fix the call site.
    #[error("authenticated transition requires an active login flow")]
    InvalidAuthenticatedTransition,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    /// Create a fresh manager in [`SessionState::LoggedOut`] with no
    /// attached lifecycle.
    #[must_use]
    pub fn new() -> Self {
        Self {
            snapshot: SessionSnapshot {
                state: SessionState::LoggedOut,
                authenticated_user: None,
                auth_token: None,
                email: None,
                pending_challenge: None,
                last_auth_error: None,
            },
            lifecycle: None,
        }
    }

    /// Borrow the current [`SessionSnapshot`]. Reading the held
    /// [`SecretString`] auth token requires an explicit `expose_secret`
    /// call — the snapshot itself is not a credential leak.
    #[must_use]
    pub fn snapshot(&self) -> &SessionSnapshot {
        &self.snapshot
    }

    /// Inspect lifecycle fields (`expires_at`, `last_used_at`, ...).
    /// Returns `None` if no authenticated session is attached.
    #[must_use]
    pub fn lifecycle(&self) -> Option<&SessionLifecycle> {
        self.lifecycle.as_ref()
    }

    /// Attach (or replace) lifecycle timing after a successful auth.
    /// Caller is responsible for providing `now_secs` from their clock.
    pub fn attach_lifecycle(
        &mut self,
        now_secs: u64,
        policy: &RefreshPolicy,
        credentials_retained: bool,
    ) {
        self.lifecycle = Some(SessionLifecycle::new(
            now_secs,
            policy,
            credentials_retained,
        ));
    }

    /// Update `last_used_at` to mark activity. No-op when not authenticated.
    pub fn mark_used(&mut self, now_secs: u64) {
        if let Some(lc) = self.lifecycle.as_mut() {
            lc.mark_used(now_secs);
        }
    }

    /// Record a successful refresh (reset timers, replace token).
    pub fn record_refresh(
        &mut self,
        now_secs: u64,
        policy: &RefreshPolicy,
        new_token: SecretString,
    ) -> Result<(), SessionManagerError> {
        if !matches!(self.snapshot.state, SessionState::Authenticated) {
            return Err(SessionManagerError::InvalidAuthenticatedTransition);
        }
        self.snapshot.auth_token = Some(new_token);
        if let Some(lc) = self.lifecycle.as_mut() {
            lc.record_refresh(now_secs, policy);
        } else {
            self.lifecycle = Some(SessionLifecycle::new(now_secs, policy, false));
        }
        Ok(())
    }

    /// Explicit revocation: transitions to `LoggedOut` and zeroizes the
    /// in-memory auth token via `SecretString::Drop`.
    pub fn revoke(&mut self) -> AuthEvent {
        self.snapshot.state = SessionState::LoggedOut;
        self.snapshot.authenticated_user = None;
        // Move the token out so its Drop runs and zeroizes memory.
        let _ = self.snapshot.auth_token.take();
        self.snapshot.email = None;
        self.snapshot.pending_challenge = None;
        self.snapshot.last_auth_error = None;
        self.lifecycle = None;
        AuthEvent::LoggedOut
    }

    /// Fold a freshly fetched `userinfo` response into the
    /// authenticated snapshot. Only valid while
    /// [`SessionState::Authenticated`].
    pub fn update_userinfo(
        &mut self,
        user_id: Option<UserId>,
        email: Option<String>,
    ) -> Result<(), SessionManagerError> {
        match self.snapshot.state {
            SessionState::Authenticated => {
                if user_id.is_some() {
                    self.snapshot.authenticated_user = user_id;
                }
                self.snapshot.email = email;
                Ok(())
            }
            _ => Err(SessionManagerError::InvalidAuthenticatedTransition),
        }
    }

    /// Replace the in-memory auth token. The previously held
    /// [`SecretString`] is dropped and zeroized in the process.
    /// Only valid while [`SessionState::Authenticated`].
    pub fn replace_auth_token(
        &mut self,
        auth_token: SecretString,
    ) -> Result<(), SessionManagerError> {
        match self.snapshot.state {
            SessionState::Authenticated => {
                self.snapshot.auth_token = Some(auth_token);
                Ok(())
            }
            _ => Err(SessionManagerError::InvalidAuthenticatedTransition),
        }
    }

    /// Apply an [`AuthCommand`] and return the resulting [`AuthEvent`].
    ///
    /// This is the only way to drive the state machine forward. Invalid
    /// transitions (e.g. `MarkAuthenticated` while
    /// [`SessionState::LoggedOut`]) return
    /// [`SessionManagerError::InvalidAuthenticatedTransition`] and
    /// leave the snapshot unchanged except for the documented side
    /// effects of each command.
    pub fn apply(&mut self, command: AuthCommand) -> Result<AuthEvent, SessionManagerError> {
        match command {
            AuthCommand::BeginLogin => {
                self.snapshot.state = SessionState::AwaitingCredentials;
                self.snapshot.last_auth_error = None;
                Ok(AuthEvent::LoginStarted)
            }
            AuthCommand::LoginWithPassword { .. } => {
                self.snapshot.state = SessionState::AuthenticatingWithPassword;
                self.snapshot.pending_challenge = None;
                self.snapshot.last_auth_error = None;
                Ok(AuthEvent::LoginStarted)
            }
            AuthCommand::LoginWithToken { .. } => {
                self.snapshot.state = SessionState::AuthenticatingWithToken;
                self.snapshot.pending_challenge = None;
                self.snapshot.last_auth_error = None;
                Ok(AuthEvent::LoginStarted)
            }
            AuthCommand::SubmitTwoFactorCode {
                code: _code,
                trust_device,
            } => {
                if self.snapshot.pending_challenge.is_none() {
                    return Err(SessionManagerError::NoPendingChallenge);
                }

                self.snapshot.state = SessionState::AuthenticatingWithPassword;
                if let Some(pending_challenge) = self.snapshot.pending_challenge.as_mut() {
                    pending_challenge.trust_device = trust_device;
                }
                Ok(AuthEvent::LoginStarted)
            }
            AuthCommand::MarkAuthenticated {
                user_id,
                auth_token,
            } => match self.snapshot.state {
                SessionState::AuthenticatingWithPassword
                | SessionState::AuthenticatingWithToken
                | SessionState::TwoFactorRequired => {
                    self.snapshot.state = SessionState::Authenticated;
                    self.snapshot.authenticated_user = user_id;
                    self.snapshot.auth_token = Some(auth_token);
                    self.snapshot.pending_challenge = None;
                    self.snapshot.last_auth_error = None;
                    Ok(AuthEvent::LoginSucceeded {
                        user_id: self.snapshot.authenticated_user,
                    })
                }
                _ => Err(SessionManagerError::InvalidAuthenticatedTransition),
            },
            AuthCommand::MarkAuthenticationFailed { message } => {
                self.snapshot.state = SessionState::AuthFailed;
                self.snapshot.authenticated_user = None;
                self.snapshot.auth_token = None;
                self.snapshot.email = None;
                self.snapshot.pending_challenge = None;
                self.snapshot.last_auth_error = message.clone();
                self.lifecycle = None;
                Ok(AuthEvent::LoginFailed { message })
            }
            AuthCommand::MarkTwoFactorCodeInvalid { message } => {
                // Keep `pending_challenge` so the caller can retype the
                // code. Only record the last error and surface a
                // LoginFailed event for audit. State stays in
                // `TwoFactorRequired` implicitly since the challenge is
                // preserved — no explicit transition needed.
                self.snapshot.last_auth_error = message.clone();
                Ok(AuthEvent::LoginFailed { message })
            }
            AuthCommand::Logout => {
                self.snapshot.state = SessionState::LoggedOut;
                self.snapshot.authenticated_user = None;
                self.snapshot.auth_token = None;
                self.snapshot.email = None;
                self.snapshot.pending_challenge = None;
                self.snapshot.last_auth_error = None;
                self.lifecycle = None;
                Ok(AuthEvent::LoggedOut)
            }
        }
    }

    /// Transition to [`SessionState::TwoFactorRequired`] and install a
    /// [`PendingChallenge`]. Returns
    /// [`AuthEvent::TwoFactorChallengeIssued`].
    pub fn issue_two_factor_challenge(
        &mut self,
        token: SecretString,
        trust_device: bool,
    ) -> AuthEvent {
        self.snapshot.state = SessionState::TwoFactorRequired;
        self.snapshot.pending_challenge = Some(PendingChallenge {
            token,
            trust_device,
        });
        self.snapshot.last_auth_error = None;
        AuthEvent::TwoFactorChallengeIssued
    }
}

#[cfg(test)]
mod tests {
    use pcloud_secret::{ExposeSecret, secret_string::SecretString};

    use crate::{AuthCommand, SessionManager, SessionState};

    #[test]
    fn submit_two_factor_code_preserves_server_challenge_token() {
        let mut manager = SessionManager::new();
        manager.issue_two_factor_challenge(SecretString::new("server-token"), false);

        let event = manager
            .apply(AuthCommand::SubmitTwoFactorCode {
                code: SecretString::new("654321"),
                trust_device: true,
            })
            .expect("submission should succeed");

        assert_eq!(event, crate::AuthEvent::LoginStarted);
        assert_eq!(
            manager.snapshot().state,
            SessionState::AuthenticatingWithPassword
        );
        let challenge = manager
            .snapshot()
            .pending_challenge
            .as_ref()
            .expect("challenge should remain pending");
        assert_eq!(challenge.token.expose_secret(), "server-token");
        assert!(challenge.trust_device);
    }

    #[test]
    fn auth_failure_preserves_last_backend_error() {
        let mut manager = SessionManager::new();

        let event = manager
            .apply(AuthCommand::MarkAuthenticationFailed {
                message: Some("invalid credentials".to_owned()),
            })
            .expect("failure transition should succeed");

        assert_eq!(
            event,
            crate::AuthEvent::LoginFailed {
                message: Some("invalid credentials".to_owned())
            }
        );
        assert_eq!(manager.snapshot().state, SessionState::AuthFailed);
        assert_eq!(
            manager.snapshot().last_auth_error.as_deref(),
            Some("invalid credentials")
        );
    }

    #[test]
    fn replace_auth_token_updates_authenticated_session() {
        let mut manager = SessionManager::new();
        manager
            .apply(AuthCommand::LoginWithToken {
                token: SecretString::new("old-token"),
            })
            .expect("token login should start");
        manager
            .apply(AuthCommand::MarkAuthenticated {
                user_id: None,
                auth_token: SecretString::new("old-token"),
            })
            .expect("mark authenticated should succeed");

        manager
            .replace_auth_token(SecretString::new("new-token"))
            .expect("auth token replacement should succeed");

        assert_eq!(
            manager
                .snapshot()
                .auth_token
                .as_ref()
                .expect("auth token should exist")
                .expose_secret(),
            "new-token"
        );
    }
}
