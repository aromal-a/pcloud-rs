//! Typed events emitted by the auth state machine. Events never carry
//! secret material — they are safe to log, audit, and forward to
//! observers.
//!
//! # Relationship to the state machine
//!
//! Every coarse transition in [`crate::state::SessionState`] emits
//! exactly one event (see the state diagram in the crate-level docs).
//! Consumers — IPC surface, audit log, TUI — may observe events freely;
//! none of them contain a [`pcloud_secret::secret_string::SecretString`]
//! or any credential-derived bytes.
//!
//! # ADR 0007
//!
//! Error payloads within events carry only server-supplied messages and
//! the curated `Display` of protocol-layer failures; both have been
//! audited in the proto crate to exclude token material.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_model::ids::UserId;

/// Auth-layer lifecycle event.
///
/// Every variant is intentionally free of secret material so audit
/// pipelines can forward events to logs, metrics, and the IPC surface
/// without risk of credential leakage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthEvent {
    /// A login command was accepted and the state machine transitioned
    /// into an authenticating state.
    LoginStarted,
    /// Login succeeded. `user_id` is present when the server returned
    /// one in the login response.
    LoginSucceeded {
        /// Authenticated user identifier when known.
        user_id: Option<UserId>,
    },
    /// Login failed with a (possibly empty) server-supplied message.
    LoginFailed {
        /// Optional server-supplied error description.
        message: Option<String>,
    },
    /// The server requested two-factor authentication. A
    /// [`crate::state::PendingChallenge`] is now held in the snapshot.
    TwoFactorChallengeIssued,
    /// Session was revoked (explicit logout or token-refresh expiry).
    LoggedOut,
    /// The session's auth token was refreshed in place. The old token
    /// remains valid server-side until the server expires it, but the
    /// session now holds the new token. Emitted by
    /// `AuthOrchestrator::refresh_token`.
    TokenRefreshed {
        /// The authenticated user identifier at the time of refresh.
        user_id: Option<UserId>,
    },
    /// A token-refresh attempt was classified as permanently expired.
    /// The session has been revoked; a fresh interactive login is
    /// required. Emitted alongside session revocation.
    TokenRefreshExpired {
        /// pCloud `result` code that classified the token as expired.
        result: u64,
    },
    /// A token-refresh attempt failed transiently (transport, server
    /// error, malformed response). The session is left untouched and
    /// the caller may retry with backoff.
    TokenRefreshTemporaryFailure {
        /// Curated (secret-free) description of the failure.
        message: String,
    },
}
