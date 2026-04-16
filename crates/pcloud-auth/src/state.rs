//! State types tracked by [`crate::manager::SessionManager`].
//!
//! [`SessionState`] is the coarse-grained auth-flow state. The full
//! runtime snapshot lives in [`SessionSnapshot`], which may hold a
//! [`SecretString`] auth token and a [`PendingChallenge`] for the 2FA
//! flow. Both container types implement [`Clone`] by hand so every
//! secret duplication goes through the audit-visible
//! [`SecretString::clone_secret`] path (ADR 0007).

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_model::ids::UserId;
use pcloud_secret::secret_string::SecretString;
use serde::{Deserialize, Serialize};

/// Coarse auth-flow state tracked by [`crate::manager::SessionManager`].
///
/// Serialization does **not** carry secret material; only the tag name
/// is emitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    /// No session, no credentials in flight.
    LoggedOut,
    /// `BeginLogin` accepted; waiting for credentials.
    AwaitingCredentials,
    /// A password login is in flight against the server.
    AuthenticatingWithPassword,
    /// A token login (existing session revalidation) is in flight.
    AuthenticatingWithToken,
    /// Server demanded 2FA; a [`PendingChallenge`] is attached to the
    /// snapshot.
    TwoFactorRequired,
    /// Authenticated. An auth token is held in memory.
    Authenticated,
    /// Server hard-rejected the login.
    AuthFailed,
}

/// 2FA challenge that must be answered before authentication completes.
///
/// Holds the server-issued challenge `token` as a [`SecretString`] so it
/// is redacted from logs and zeroized on drop. This type cannot derive
/// [`Clone`] because [`SecretString`] deliberately does not; the
/// hand-written impl routes through [`SecretString::clone_secret`] (ADR
/// 0007).
#[derive(Debug, PartialEq, Eq)]
pub struct PendingChallenge {
    /// Server-issued challenge token. Zeroized on drop.
    pub token: SecretString,
    /// Whether the submission should ask the server to trust this
    /// device and skip future 2FA prompts.
    pub trust_device: bool,
}

impl Clone for PendingChallenge {
    // Audit-visible: `SecretString` cannot derive `Clone`, so the manual impl
    // delegates duplication to the named `clone_secret` method (audit M3).
    fn clone(&self) -> Self {
        Self {
            token: self.token.clone_secret(),
            trust_device: self.trust_device,
        }
    }
}

/// Full snapshot of the auth state machine.
///
/// The `auth_token` field holds a live [`SecretString`]; reading it is
/// an audit-visible operation. The hand-written [`Clone`] impl
/// duplicates every secret via [`SecretString::clone_secret`] (ADR
/// 0007).
#[derive(Debug, PartialEq, Eq)]
pub struct SessionSnapshot {
    /// Current coarse state.
    pub state: SessionState,
    /// Authenticated user identifier, if known.
    pub authenticated_user: Option<UserId>,
    /// Live auth token. Zeroized on drop.
    pub auth_token: Option<SecretString>,
    /// Authenticated user email, if known.
    pub email: Option<String>,
    /// Pending 2FA challenge, if any.
    pub pending_challenge: Option<PendingChallenge>,
    /// Last server-supplied failure message, if any. Secret-free.
    pub last_auth_error: Option<String>,
}

impl Clone for SessionSnapshot {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            authenticated_user: self.authenticated_user,
            auth_token: self.auth_token.as_ref().map(SecretString::clone_secret),
            email: self.email.clone(),
            pending_challenge: self.pending_challenge.clone(),
            last_auth_error: self.last_auth_error.clone(),
        }
    }
}
