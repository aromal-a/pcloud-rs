//! Runtime policy for the crypto subsystem.
//!
//! These flags govern behaviour that is safety-relevant on the Rust path.
//! `persist_master_key = false` is a hard default that the active runtime
//! enforces; an attempt to flip it to `true` from config must be explicitly
//! rejected by the daemon so the Rust path cannot accidentally be configured
//! to persist plaintext key material like some C code paths historically
//! tolerated.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Safety policy governing the crypto runtime.
///
/// Non-secret. The invariant `persist_master_key == false` is enforced by
/// [`CryptoPolicy::is_safe`] and by the daemon bootstrap path; flipping it
/// would violate ADR-0007 and the project's zeroize-discipline contract.
///
/// # Security
/// This struct is the policy gate consulted by every sensitive entry point
/// in [`crate::CryptoShell`] (`setup`, `start`, `change_password_*`). A
/// policy where `persist_master_key == true` is treated as misconfiguration
/// and the operation is refused with [`crate::CryptoError::UnsafePolicy`]
/// *before* any Argon2id derivation runs. Per ADR-0007 the password is
/// never persisted, and by extension no derived master key is either; this
/// policy is the programmatic statement of that invariant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CryptoPolicy {
    /// If set, lock crypto on platform-suspend events.
    ///
    /// # Security
    /// Mitigates key exposure across a laptop-lid / sleep cycle where the
    /// process image may be written to disk by the host OS. The runtime
    /// drops the resident `SecretBytes` so the Argon2id output is zeroized
    /// before suspend completes.
    pub lock_on_suspend: bool,
    /// Must remain `false`. The Rust path never persists master key material.
    ///
    /// # Security
    /// Per ADR-0007 (password never persisted). Setting this to `true`
    /// is refused by [`CryptoPolicy::is_safe`] and by the daemon
    /// bootstrap; see also [`crate::CryptoError::UnsafePolicy`]. The
    /// legacy C client historically tolerated on-disk key caches under
    /// some build flags; the Rust path intentionally does not carry that
    /// behaviour forward.
    pub persist_master_key: bool,
    /// Auto-lock after this many seconds of inactivity. 0 disables auto-lock.
    ///
    /// # Security
    /// Reduces the window during which the resident Argon2id master key
    /// is available to an attacker who later compromises the running
    /// process. The daemon enforces the timer and drops the
    /// `SecretBytes` (zeroize on drop) when it fires.
    pub auto_lock_idle_secs: u64,
}

impl Default for CryptoPolicy {
    fn default() -> Self {
        Self {
            lock_on_suspend: true,
            persist_master_key: false,
            auto_lock_idle_secs: 0,
        }
    }
}

impl CryptoPolicy {
    /// Returns true if the policy is safe for the active Rust runtime.
    ///
    /// # Security
    /// The safety predicate is intentionally conservative: any future
    /// policy bit that would weaken zeroize-on-drop or introduce
    /// plaintext key persistence must AND into this result. Callers
    /// evaluate it before performing any key derivation so that an
    /// unsafe configuration cannot even enter the Argon2id hot path.
    #[must_use]
    pub fn is_safe(&self) -> bool {
        !self.persist_master_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_safe() {
        assert!(CryptoPolicy::default().is_safe());
    }

    #[test]
    fn persistence_is_unsafe() {
        let p = CryptoPolicy {
            persist_master_key: true,
            ..CryptoPolicy::default()
        };
        assert!(!p.is_safe());
    }
}
