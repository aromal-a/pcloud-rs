//! Secret-bearing wire-type wrapper for proto method structs.
//!
//! `RedactedProtoString` is a transparent `String` newtype for request struct
//! fields that carry transit-only secrets (auth tokens, passwords, TFA codes,
//! digest tokens, OTP / recovery codes, crypto passphrases). It serialises
//! identically to `String` so the wire format is unchanged, but its `Debug`
//! impl prints `<redacted N bytes>` instead of the plaintext — closing the
//! H1 audit finding where an accidental `tracing::debug!("{req:?}")` on any
//! request struct would leak the secret.
//!
//! ## Why not `SecretString`?
//!
//! `SecretString` (from `pcloud-secret`) deliberately does not expose serde
//! impls to prevent accidental cross-boundary serialisation. `RedactedProtoString`
//! bridges that gap: serde-transparent on the proto wire, redacted in
//! diagnostics. Callers that need the raw `&str` value for encoding call
//! `.expose_secret()`.
//!
//! ## Why not the IPC `RedactedString`?
//!
//! `pcloud-ipc::RedactedString` lives in a separate crate and is not a
//! dependency of `pcloud-proto`. The two types are structurally identical
//! and keep the crate dependency graph clean.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// A `String` that round-trips identically on the proto wire but whose
/// `Debug` representation is redacted. See the module-level docs.
///
/// Construct via `RedactedProtoString::new(...)`, `From<String>`, or
/// `From<&str>`. Consume the inner secret via `.expose_secret()` (returns
/// `&str`) or `.into_string()` (consuming). `AsRef<str>` and `Deref<Target =
/// str>` are also available for ergonomic use in `BinaryParam::string(...)` calls.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct RedactedProtoString(String);

impl RedactedProtoString {
    /// Construct from any owned / borrowed string-like value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Expose the inner secret as `&str`.
    ///
    /// The name mirrors `SecretString::expose_secret()` so call-sites are
    /// easy to audit: any use of `.expose_secret()` is a deliberate,
    /// single-use read of a sensitive value.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Unwrap to the inner `String`. Prefer `.expose_secret()` where a
    /// `&str` suffices; use this only when an owned `String` is required
    /// (e.g., when handing off to `SecretString::from(...)`).
    ///
    /// **Note:** the caller takes ownership of the raw string and is
    /// responsible for zeroizing it when done; wrapping the result in
    /// [`zeroize::Zeroizing`] is recommended.
    #[must_use]
    pub fn into_string(mut self) -> String {
        std::mem::take(&mut self.0)
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

impl Drop for RedactedProtoString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for RedactedProtoString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted {} bytes>", self.0.len())
    }
}

impl fmt::Display for RedactedProtoString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Display also redacts — callers that need the value must call
        // `.expose_secret()` explicitly.
        write!(f, "<redacted>")
    }
}

impl From<String> for RedactedProtoString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for RedactedProtoString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<RedactedProtoString> for String {
    fn from(mut value: RedactedProtoString) -> String {
        std::mem::take(&mut value.0)
    }
}

impl AsRef<str> for RedactedProtoString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for RedactedProtoString {
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
        let s = RedactedProtoString::new("hunter2");
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
            rendered.contains('7'),
            "Debug output should reveal the byte length (7 for \"hunter2\"); got {rendered:?}"
        );
    }

    #[test]
    fn display_redacts_plaintext() {
        let s = RedactedProtoString::new("hunter2");
        let rendered = format!("{s}");
        assert!(
            !rendered.contains("hunter2"),
            "Display output must not include the plaintext secret; got {rendered:?}"
        );
    }

    #[test]
    fn expose_secret_returns_inner_value() {
        let s = RedactedProtoString::new("s3cr3t");
        assert_eq!(s.expose_secret(), "s3cr3t");
    }

    #[test]
    fn serde_roundtrip_is_plain_string() {
        let s = RedactedProtoString::new("hunter2");
        let wire = serde_json::to_string(&s).unwrap();
        assert_eq!(wire, "\"hunter2\"");
        let back: RedactedProtoString = serde_json::from_str(&wire).unwrap();
        assert_eq!(back.expose_secret(), "hunter2");
    }

    #[test]
    fn empty_and_len_are_safe_markers() {
        let empty = RedactedProtoString::default();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        let s = RedactedProtoString::from("abc");
        assert!(!s.is_empty());
        assert_eq!(s.len(), 3);
    }
}
