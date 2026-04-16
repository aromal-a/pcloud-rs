//! Secret-bearing wire-type wrapper.
//!
//! `RedactedString` is a transparent `String` newtype for IPC request
//! fields that carry transit-only secrets (account passwords, crypto
//! passphrases, OTP / recovery codes, public-link passwords, auth
//! tokens). It serialises identically to `String` so the wire format
//! is unchanged, but its `Debug` impl prints `<redacted N bytes>`
//! instead of the plaintext — closing the last remaining leak vector
//! from an accidental `tracing::debug!("{req:?}")` in a request
//! handler.
//!
//! This is the redactor the `Request` audit H1 note asks for:
//!
//! > "If this invariant regresses (e.g. `Request` values start being
//! > stored on long-lived state or logged via `Debug`), these fields
//! > must be converted to `SecretString` and a serde-skip or redacted-
//! > serialize wrapper added."
//!
//! `SecretString` (from `pcloud-secret`) cannot be used directly on
//! the wire because it deliberately does not expose serde impls
//! (audit finding M3, preventing accidental cross-boundary
//! serialisation). `RedactedString` bridges that gap: serde on the
//! IPC boundary, redacted in diagnostics, still immediately
//! destructured into a `SecretString` on the daemon side.

use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

/// A `String` that round-trips identically on the IPC wire but whose
/// `Debug` representation is redacted. See the module-level docs.
///
/// Construct via `RedactedString::from(...)` or `.into()` from any
/// `Into<String>`. Consume via `AsRef<str>`, `Deref<Target = str>`,
/// or `.into_string()` when an owned `String` is needed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct RedactedString(String);

impl RedactedString {
    /// Construct from any owned / borrowed string-like value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Unwrap to the inner `String`. Use on the daemon side when
    /// handing off to `SecretString::from(...)`.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }

    /// Borrow the inner secret as `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Byte length of the wrapped secret. Safe to log — it reveals
    /// nothing about the content.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` when the wrapped secret is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted {} bytes>", self.0.len())
    }
}

impl From<String> for RedactedString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for RedactedString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<RedactedString> for String {
    fn from(value: RedactedString) -> String {
        value.0
    }
}

impl AsRef<str> for RedactedString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for RedactedString {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_plaintext() {
        let s = RedactedString::new("hunter2");
        let rendered = format!("{s:?}");
        assert!(
            !rendered.contains("hunter2"),
            "Debug output must not include the plaintext secret; got {rendered:?}"
        );
        assert!(
            rendered.contains("redacted"),
            "Debug output should mark the field as redacted; got {rendered:?}"
        );
        assert!(
            rendered.contains("7"),
            "Debug output should reveal the byte length (7 for \"hunter2\"); got {rendered:?}"
        );
    }

    #[test]
    fn serde_roundtrip_is_plain_string() {
        let s = RedactedString::new("hunter2");
        let wire = serde_json::to_string(&s).unwrap();
        assert_eq!(wire, "\"hunter2\"");
        let back: RedactedString = serde_json::from_str(&wire).unwrap();
        assert_eq!(back.as_str(), "hunter2");
    }

    #[test]
    fn empty_and_len_are_const_time_safe_markers() {
        let empty = RedactedString::default();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        let s = RedactedString::from("abc");
        assert!(!s.is_empty());
        assert_eq!(s.len(), 3);
    }
}
