//! Lifecycle state machine for the crypto subsystem.
//!
//! # Security
//!
//! The [`crate::state::UnlockState`] enum is the single gate between
//! "plaintext key material is resident in process memory" and "no plaintext
//! key material exists". All sector/metadata operations consult this gate
//! before attempting any key access. Because the enum is `Copy` it carries
//! no secrets itself, but downstream code treats any state other than
//! [`crate::state::UnlockState::Unlocked`] as a hard refusal to decrypt.
//!
//! Per ADR-0007 the transition graph never persists a state that would imply
//! a resident master key; only setup fingerprints are durable.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Lifecycle of the crypto subsystem.
///
/// Mirrors the C client's `psync_crypto_isstarted()` / `psync_crypto_issetup()`
/// split, but keeps the Rust path stricter — the only way to leave
/// [`UnlockState::Locked`] is through [`crate::CryptoShell::start`] with the
/// correct password plus a previously performed [`crate::CryptoShell::setup`].
///
/// # Security
/// This enum is the plaintext-key-residency gate. Values other than
/// [`UnlockState::Unlocked`] imply that no `SecretBytes` containing the
/// master key is resident; the [`crate::CryptoShell`] is required to drop
/// and zeroize key material before transitioning to any non-`Unlocked`
/// variant. Per ADR-0007 the password itself never influences this value
/// beyond gating the Argon2id fingerprint check in
/// [`crate::CryptoShell::start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnlockState {
    /// Crypto has never been set up on this account / profile.
    NotSetup,
    /// Crypto is set up but currently locked. No plaintext key material is
    /// resident in memory.
    Locked,
    /// A `start` operation is in progress. This window is kept as narrow as
    /// possible and is intentionally never observed over IPC.
    Unlocking,
    /// Crypto is active. Content/metadata operations are permitted.
    Unlocked,
}

impl UnlockState {
    /// Returns `true` iff crypto is currently unlocked (equivalent to the C
    /// `psync_crypto_isstarted()` predicate).
    ///
    /// # Security
    /// Callers use this as a precondition for releasing plaintext. The
    /// predicate is a simple `match` with no secret-dependent branch — it
    /// is constant-time with respect to key material (which is not held
    /// by this type at all). Returning `true` does **not** imply the
    /// caller currently holds the key; the [`crate::keys::KeyManager`]
    /// `active_key_material` slot must also be `Some`.
    #[must_use]
    pub fn is_started(self) -> bool {
        matches!(self, UnlockState::Unlocked)
    }

    /// Returns `true` iff crypto has been set up on this profile (mirrors the
    /// C `psync_crypto_issetup()` predicate).
    ///
    /// # Security
    /// "Set up" means a non-secret [`crate::keys::SetupFingerprint`] has
    /// been recorded; it reveals no key bits. Per ADR-0007 the password
    /// is not persisted, so this predicate cannot be used as an oracle
    /// against the password itself.
    #[must_use]
    pub fn is_setup(self) -> bool {
        !matches!(self, UnlockState::NotSetup)
    }
}
