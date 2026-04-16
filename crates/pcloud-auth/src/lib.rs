#![deny(missing_docs)]
#![forbid(unsafe_code)]
#![allow(clippy::pedantic)]
//! # pcloud-auth
//!
//! Authentication state machine and two-factor orchestration for the
//! Rust pcloud-rs path. Owns the session manager, TFA flows, typed auth
//! events, and session-lifecycle tracking (proactive refresh and idle
//! logout). Every in-memory credential flows through
//! [`pcloud_secret::secret_string::SecretString`] — this crate never
//! derives `Clone` over a secret, never logs credential bytes, and
//! never persists raw credentials.
//!
//! ## Cross-references
//!
//! * `ARCHITECTURE.md` section "Auth lifecycle".
//! * `SECURITY-MODEL.md` section "Secrets".
//! * ADR 0007 ("Secret handling and audit-visible duplication") — the
//!   rationale for the [`pcloud_secret::secret_string::SecretString::clone_secret`]
//!   pattern used throughout this crate.
//!
//! ## State machine
//!
//! The auth flow follows the progression
//! `Anonymous → Challenge → TwoFactor → Authenticated → Refresh → Expired`.
//! The coarse states live in [`state::SessionState`]; each transition
//! is driven by an [`commands::AuthCommand`] fed to
//! [`manager::SessionManager::apply`] and emits a non-secret
//! [`events::AuthEvent`].
//!
//! ```text
//!   LoggedOut                             ◄── "Anonymous"
//!      │  BeginLogin
//!      ▼
//!   AwaitingCredentials                   ◄── "Challenge" (collecting creds)
//!      │  LoginWithPassword / LoginWithToken
//!      ▼
//!   AuthenticatingWith{Password,Token}
//!      ├── server: ok         ──► Authenticated
//!      ├── server: TFA needed ──► TwoFactorRequired
//!      └── server: hard fail  ──► AuthFailed
//!
//!   TwoFactorRequired                     ◄── "TwoFactor" (challenge held)
//!      │  SubmitTwoFactorCode (TOTP | SMS | push | recovery)
//!      │  send_two_factor_sms             (re-deliver code over SMS)
//!      │  send_two_factor_notification    (re-deliver push to device)
//!      ├── ok         ──► Authenticated
//!      ├── soft fail  ──► TwoFactorRequired (PendingChallenge preserved;
//!      │                                    caller may retype)
//!      └── hard fail  ──► AuthFailed
//!
//!   Authenticated                         ◄── "Authenticated"
//!      ├── refresh_token (pCloud-native, see ADR 0007) ──► Authenticated
//!      │                                                   "Refresh"
//!      ├── token_refresh_expired                        ──► LoggedOut
//!      │                                                   "Expired"
//!      ├── handle_auth_expired (401 from downstream)    ──► Refresh or
//!      │                                                   LoggedOut
//!      └── Logout / revoke                              ──► LoggedOut
//! ```
//!
//! ### Transition notes
//!
//! * **Anonymous → Challenge**: `BeginLogin` or direct
//!   `LoginWithPassword` / `LoginWithToken`. No network I/O yet.
//! * **Challenge → TwoFactor**: server returns
//!   [`pcloud_proto::auth_api::PasswordLoginOutcome::TwoFactorRequired`].
//!   The challenge token is parked in
//!   [`state::PendingChallenge::token`] as a
//!   [`pcloud_secret::secret_string::SecretString`].
//! * **TwoFactor → Authenticated**: any of the four 2FA delivery paths
//!   (TOTP, SMS, push, recovery) — see the `# Flow` sections on
//!   [`orchestrator::ProtocolAuthFlow::submit_two_factor_code`],
//!   [`orchestrator::ProtocolAuthFlow::send_two_factor_sms`], and
//!   [`orchestrator::ProtocolAuthFlow::send_two_factor_notification`].
//! * **Authenticated → Refresh**: proactive via
//!   [`refresh::RefreshCoordinator`] at 80 % of lifetime; reactive via
//!   [`refresh::RefreshCoordinator::handle_auth_expired`] on 401.
//! * **Refresh → Expired**: server classifies the current token as
//!   unrecoverably expired (`AuthRefreshError::AuthExpired`);
//!   [`manager::SessionManager::revoke`] zeroizes the in-memory token.
//!
//! ## Security invariants
//!
//! * Secrets in [`state::SessionSnapshot`] / [`state::PendingChallenge`] /
//!   [`commands::AuthCommand`] are stored as
//!   [`pcloud_secret::secret_string::SecretString`] (zeroized on drop,
//!   redacted `Debug`, constant-time `PartialEq`).
//! * `Clone` on container types is hand-written and delegates to
//!   [`pcloud_secret::secret_string::SecretString::clone_secret`] so every
//!   secret duplication is grep-able in code review (ADR 0007).
//! * [`manager::SessionManager::revoke`] zeroizes the in-memory auth
//!   token by dropping its
//!   [`pcloud_secret::secret_string::SecretString`] (its `ZeroizeOnDrop`
//!   impl scrubs the heap buffer before deallocation).
//! * [`orchestrator::ProtocolAuthFlow::refresh_token`] never surfaces raw
//!   token bytes in the error or event payloads it emits.
//! * **ADR 0007**: the user's password is **never** persisted to disk;
//!   only the auth token may be persisted, and only behind an explicit
//!   opt-in guarded by the vault at
//!   `pcloud-daemon::auth_vault`. This crate owns the in-memory half of
//!   that contract — no code path here writes a credential to durable
//!   storage.

// **PLATFORM:** all
// **GATING:** none (portable).

pub mod commands;
pub mod events;
pub mod lifecycle;
pub mod manager;
pub mod orchestrator;
pub mod refresh;
pub mod state;

/// Crate identifier used by audit and telemetry records.
pub const CRATE_NAME: &str = "pcloud-auth";

/// Compile-time descriptor for the auth subsystem.
///
/// Used by the daemon's plugin-registry scaffolding to announce the
/// presence of the `pcloud-auth` stack. It carries no runtime state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSubsystem {
    /// Stable, human-readable subsystem name ("auth" in releases).
    pub name: &'static str,
}

pub use commands::AuthCommand;
pub use events::AuthEvent;
pub use lifecycle::{
    Clock, LifecycleError, RefreshGuard, RefreshPolicy, RefreshTicket, SessionLifecycle,
    SystemClock, TestClock, revoke_token,
};
pub use manager::{SessionManager, SessionManagerError};
pub use orchestrator::{AuthFlowError, ProtocolAuthFlow, RefreshTokenError};
pub use refresh::{RefreshCoordinator, RefreshDecision, RefreshOutcome};
pub use state::{PendingChallenge, SessionSnapshot, SessionState};
