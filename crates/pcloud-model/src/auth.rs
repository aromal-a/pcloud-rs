// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Client-visible authentication state machine.
///
/// Mirrors the states the daemon's `auth_backend` transitions through
/// while negotiating with the pCloud API (password, token, TFA code,
/// TFA device notification, recovery code). Surfaced verbatim to the
/// SDK and CLI so UIs can render progress without peeking at internal
/// daemon structs.
///
/// # Example
///
/// ```
/// use pcloud_model::auth::AuthState;
///
/// let s = AuthState::Authenticated;
/// let j = serde_json::to_string(&s).unwrap();
/// let back: AuthState = serde_json::from_str(&j).unwrap();
/// assert_eq!(s, back);
///
/// // UIs branch on the client-visible state machine:
/// fn should_prompt_for_tfa(s: &AuthState) -> bool {
///     matches!(s, AuthState::TwoFactorRequired)
/// }
/// assert!(should_prompt_for_tfa(&AuthState::TwoFactorRequired));
/// assert!(!should_prompt_for_tfa(&AuthState::Authenticated));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthState {
    /// No credentials are held; the session is anonymous.
    LoggedOut,
    /// The daemon is waiting for the operator to supply credentials.
    AwaitingCredentials,
    /// A password authentication round-trip is in flight.
    AuthenticatingWithPassword,
    /// A digest (challenge/response) authentication is in flight.
    AuthenticatingWithDigest,
    /// The server requested a second factor; the daemon is waiting for
    /// a TFA code, recovery code, SMS, or device notification.
    TwoFactorRequired,
    /// A token (previously persisted or refreshed) is being validated.
    AuthenticatingWithToken,
    /// The session is fully authenticated.
    Authenticated,
    /// The last authentication attempt failed; the daemon is idle and
    /// waiting for the client to retry.
    AuthFailed,
    /// A previously-authenticated session entered a degraded state
    /// (token near expiry, server returned a soft error). Callers can
    /// usually continue but should schedule a refresh.
    Degraded,
}
