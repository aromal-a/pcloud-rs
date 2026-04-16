//! `SecretBytes` — an audit-hardened wrapper around a heap-allocated binary
//! secret such as a derived crypto key or MAC tag.
//!
//! Hardening properties mirror [`crate::secret_string::SecretString`]:
//! - `#[derive(ZeroizeOnDrop)]` guarantees the buffer is scrubbed on `Drop`.
//! - `Clone` is deliberately not derived; use [`crate::secret_bytes::SecretBytes::clone_secret`].
//! - `PartialEq` is constant-time via [`subtle::ConstantTimeEq`].
//! - No `Serialize`/`Deserialize` impl is provided.
//! - `Debug` is redacted.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::fmt;

use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{ExposeSecret, SecretMaterial};

/// Zeroize-on-drop, redacted-`Debug` wrapper around a binary secret.
#[derive(ZeroizeOnDrop)]
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    /// Wrap a binary secret (key material, MAC tag, derived bytes). The
    /// buffer is scrubbed when the `SecretBytes` is dropped.
    ///
    /// ```
    /// use pcloud_secret::{SecretMaterial, secret_bytes::SecretBytes};
    /// let k = SecretBytes::new(vec![0xab; 32]);
    /// assert_eq!(k.expose_len(), 32);
    /// ```
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Returns `true` when the underlying buffer has zero length.
    ///
    /// ```
    /// use pcloud_secret::secret_bytes::SecretBytes;
    /// assert!(SecretBytes::new(Vec::new()).is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Audit-visible duplication. See [`crate::secret_string::SecretString::clone_secret`].
    ///
    /// ```
    /// use pcloud_secret::{ExposeSecret, secret_bytes::SecretBytes};
    /// let k = SecretBytes::new(vec![1, 2, 3]);
    /// let k2 = k.clone_secret();
    /// assert_eq!(k.expose_secret(), k2.expose_secret());
    /// ```
    #[must_use]
    pub fn clone_secret(&self) -> Self {
        Self(self.0.clone())
    }
}

impl SecretMaterial for SecretBytes {
    fn expose_len(&self) -> usize {
        self.0.len()
    }
}

impl ExposeSecret<[u8]> for SecretBytes {
    fn expose_secret(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBytes(<redacted>)")
    }
}

impl PartialEq for SecretBytes {
    /// Constant-time equality. Protects MAC-tag and derived-key comparisons
    /// from byte-at-a-time timing oracles.
    ///
    /// ```
    /// use pcloud_secret::secret_bytes::SecretBytes;
    /// assert_eq!(SecretBytes::new(vec![1, 2]), SecretBytes::new(vec![1, 2]));
    /// assert_ne!(SecretBytes::new(vec![1, 2]), SecretBytes::new(vec![1, 3]));
    /// ```
    fn eq(&self, other: &Self) -> bool {
        self.0.as_slice().ct_eq(other.0.as_slice()).into()
    }
}

impl Eq for SecretBytes {}

impl Zeroize for SecretBytes {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

// NOTE: `Serialize`/`Deserialize` are intentionally NOT implemented.
