//! `SecretString` — an audit-hardened wrapper around a heap-allocated UTF-8
//! secret such as an auth token, password, or 2FA code.
//!
//! Hardening properties (audit finding M3):
//! - `#[derive(ZeroizeOnDrop)]` guarantees the backing buffer is scrubbed on
//!   `Drop` without a hand-written `impl Drop`, so the zeroize contract cannot
//!   silently regress.
//! - `Clone` is deliberately **not** derived. Cloning a secret is an audit
//!   event because each clone doubles the window of in-memory exposure;
//!   callers must invoke the explicit, audit-visible `SecretString::clone_secret`
//!   so code review can surface every duplication.
//! - Equality is evaluated in constant time via [`subtle::ConstantTimeEq`] —
//!   the default `PartialEq` would leak token length / prefix through timing
//!   side channels when comparing auth tokens.
//! - `Serialize`/`Deserialize` are intentionally NOT implemented. Serde-derived
//!   containers cannot accidentally leak a secret to disk, logs, or the wire.
//!   A compile-fail test (`tests/compile_fail_serialize.rs`) enforces this.
//! - `Debug` renders `SecretString(<redacted>)` — the underlying bytes never
//!   reach a formatter.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::fmt;

use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{ExposeSecret, SecretMaterial};

/// Secret-bearing UTF-8 wrapper. See module docs for hardening guarantees.
///
/// Deliberately does not derive `Clone`; call [`SecretString::clone_secret`]
/// for an audit-visible duplication.
#[derive(ZeroizeOnDrop)]
pub struct SecretString(String);

impl SecretString {
    /// Wrap a UTF-8 secret (password, auth token, 2FA code, ...). The value
    /// is zeroized when the `SecretString` is dropped.
    ///
    /// ```
    /// use pcloud_secret::secret_string::SecretString;
    /// let s = SecretString::new("hunter2");
    /// assert!(!s.is_empty());
    /// ```
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns `true` when the underlying string has zero length. Safe to
    /// log — reveals only emptiness, not content.
    ///
    /// ```
    /// use pcloud_secret::secret_string::SecretString;
    /// assert!(SecretString::new("").is_empty());
    /// assert!(!SecretString::new("x").is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Audit-visible duplication of the secret.
    ///
    /// Replaces the removed `#[derive(Clone)]`. Each invocation doubles the
    /// in-memory exposure window, so every call site is intentionally
    /// conspicuous in code review.
    ///
    /// ```
    /// use pcloud_secret::{ExposeSecret, secret_string::SecretString};
    /// let a = SecretString::new("t");
    /// let b = a.clone_secret();
    /// assert_eq!(a.expose_secret(), b.expose_secret());
    /// ```
    #[must_use]
    pub fn clone_secret(&self) -> Self {
        Self(self.0.clone())
    }
}

impl SecretMaterial for SecretString {
    fn expose_len(&self) -> usize {
        self.0.len()
    }
}

impl ExposeSecret<str> for SecretString {
    fn expose_secret(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl PartialEq for SecretString {
    /// Constant-time equality. Protects auth-token and password comparisons
    /// from byte-at-a-time timing oracles.
    ///
    /// ```
    /// use pcloud_secret::secret_string::SecretString;
    /// assert_eq!(SecretString::new("abc"), SecretString::new("abc"));
    /// assert_ne!(SecretString::new("abc"), SecretString::new("abd"));
    /// ```
    fn eq(&self, other: &Self) -> bool {
        self.0.as_bytes().ct_eq(other.0.as_bytes()).into()
    }
}

impl Eq for SecretString {}

// Explicitly ensure `Zeroize` is reachable via the wrapper (belt-and-braces
// against a future refactor that swaps the inner type for something that
// does not implement `Zeroize`).
impl Zeroize for SecretString {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

// NOTE: `Serialize`/`Deserialize` are intentionally NOT implemented.
// See `tests/compile_fail_serialize.rs`.
