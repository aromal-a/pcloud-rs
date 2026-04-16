// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Client-visible crypto subsystem state.
///
/// Mirrors the `pcloud-crypto` runtime state exposed to the SDK/CLI so
/// UIs can prompt for a password exactly when needed.
///
/// # Example
///
/// ```
/// use pcloud_model::crypto::CryptoState;
///
/// fn requires_password(s: &CryptoState) -> bool {
///     matches!(s, CryptoState::Locked | CryptoState::Expired)
/// }
/// assert!(requires_password(&CryptoState::Locked));
/// assert!(requires_password(&CryptoState::Expired));
/// assert!(!requires_password(&CryptoState::Unlocked));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CryptoState {
    /// Crypto is not enabled on this account.
    Disabled,
    /// Crypto is enabled but initial setup has not completed (keys not
    /// yet generated / activated).
    SetupRequired,
    /// Keys are present but locked — a crypto password is required
    /// before encrypted folders can be read or written.
    Locked,
    /// The daemon is currently validating the supplied password.
    Unlocking,
    /// Keys are unlocked and usable.
    Unlocked,
    /// The unlocked key material has expired; the user must re-enter
    /// the crypto password.
    Expired,
    /// An error occurred that leaves the crypto subsystem unusable
    /// until it is reset or the account is re-authenticated.
    Error,
}
