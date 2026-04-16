//! Commands that drive the [`crate::manager::SessionManager`] state
//! machine. Every command that carries a credential stores it in a
//! [`SecretString`] so the buffer is zeroized on drop (see ADR 0007).

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_model::ids::UserId;
use pcloud_secret::secret_string::SecretString;

/// Inputs to [`crate::manager::SessionManager::apply`].
///
/// Variants that carry a credential (`password`, `token`, `code`,
/// `auth_token`) wrap the value in [`SecretString`] so it is redacted in
/// `Debug` output and zeroized on drop. See ADR 0007 for the wider
/// rationale behind the manual [`Clone`] impl.
#[derive(Debug, PartialEq, Eq)]
pub enum AuthCommand {
    /// Move to [`crate::state::SessionState::AwaitingCredentials`].
    BeginLogin,
    /// Begin a password-based login flow.
    LoginWithPassword {
        /// Account identifier (email or login).
        username: String,
        /// User-supplied password. Zeroized on drop.
        password: SecretString,
    },
    /// Begin a token-based login flow (existing session revalidation).
    LoginWithToken {
        /// Auth token presented by the caller. Zeroized on drop.
        token: SecretString,
    },
    /// Submit a two-factor code against a pending challenge.
    SubmitTwoFactorCode {
        /// The 2FA code (TOTP, SMS, push response, or recovery code).
        /// Zeroized on drop.
        code: SecretString,
        /// If `true`, ask the server to remember the device and skip
        /// future 2FA challenges.
        trust_device: bool,
    },
    /// Mark the session as authenticated after a successful server
    /// response. Installs the fresh auth token.
    MarkAuthenticated {
        /// Numeric user id returned by the server, if known.
        user_id: Option<UserId>,
        /// The fresh auth token. Zeroized on drop.
        auth_token: SecretString,
    },
    /// Mark the session as permanently failed. Clears credentials.
    MarkAuthenticationFailed {
        /// Optional server-supplied error message.
        message: Option<String>,
    },
    /// Soft failure of a 2FA code submission. Unlike
    /// [`AuthCommand::MarkAuthenticationFailed`], this KEEPS `pending_challenge`
    /// populated so the caller can retype the code against the same
    /// challenge token without a fresh password round-trip. pCloud
    /// allows several attempts on a single challenge before issuing a
    /// new one; we only transition to the hard-failed state once the
    /// server returns an expired-token error.
    MarkTwoFactorCodeInvalid {
        /// Optional server-supplied reason for the soft failure.
        message: Option<String>,
    },
    /// Explicit logout: clears credentials, zeroizes the auth token,
    /// moves to [`crate::state::SessionState::LoggedOut`].
    Logout,
}

impl Clone for AuthCommand {
    // `SecretString` intentionally does not derive `Clone`; each duplication
    // goes through the audit-visible `clone_secret` method (audit M3).
    fn clone(&self) -> Self {
        match self {
            Self::BeginLogin => Self::BeginLogin,
            Self::LoginWithPassword { username, password } => Self::LoginWithPassword {
                username: username.clone(),
                password: password.clone_secret(),
            },
            Self::LoginWithToken { token } => Self::LoginWithToken {
                token: token.clone_secret(),
            },
            Self::SubmitTwoFactorCode { code, trust_device } => Self::SubmitTwoFactorCode {
                code: code.clone_secret(),
                trust_device: *trust_device,
            },
            Self::MarkAuthenticated {
                user_id,
                auth_token,
            } => Self::MarkAuthenticated {
                user_id: *user_id,
                auth_token: auth_token.clone_secret(),
            },
            Self::MarkAuthenticationFailed { message } => Self::MarkAuthenticationFailed {
                message: message.clone(),
            },
            Self::MarkTwoFactorCodeInvalid { message } => Self::MarkTwoFactorCodeInvalid {
                message: message.clone(),
            },
            Self::Logout => Self::Logout,
        }
    }
}
