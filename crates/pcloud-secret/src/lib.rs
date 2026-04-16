#![deny(missing_docs)]
#![forbid(unsafe_code)]
#![allow(clippy::pedantic)]
//! # pcloud-secret
//!
//! Secret-handling primitives: [`secret_string::SecretString`] and
//! [`secret_bytes::SecretBytes`] (both zeroize-on-`Drop`, redacted
//! `Debug`, constant-time `PartialEq`) plus the [`redact`] log-redaction
//! helper. Every secret-bearing value in the workspace flows through
//! this crate; **do not** introduce raw `String` / `Vec<u8>` storage for
//! secrets, and do not `#[derive(Clone)]` on types embedding a secret —
//! duplicate via an audit-visible `clone_secret()` call instead.
//!
//! # Security guarantees (audit M3 / ADR 0007)
//!
//! Every wrapper exported by this crate is built to satisfy four
//! non-negotiable properties:
//!
//! 1. **Zeroize on `Drop`** — the backing buffer is scrubbed through
//!    [`zeroize::ZeroizeOnDrop`]. There is no hand-written `impl Drop`,
//!    so the contract cannot silently regress if the type is refactored.
//! 2. **Redacted `Debug`** — `{:?}` always prints
//!    `SecretString(<redacted>)` or `SecretBytes(<redacted>)`. Raw bytes
//!    never reach a formatter, so accidental `tracing::debug!(?token)`
//!    calls remain safe.
//! 3. **No `Clone`, ever** — duplicating a secret doubles the in-memory
//!    exposure window. Wrappers deliberately do not derive `Clone`;
//!    callers must invoke the explicit [`secret_string::SecretString::clone_secret`]
//!    / [`secret_bytes::SecretBytes::clone_secret`] path, which is
//!    grep-able in review. A compile-time regression test
//!    (`tests/compile_fail_serialize.rs` and the autoderef probe in
//!    `pcloud-auth::orchestrator::tests`) enforces both properties.
//! 4. **No `Serialize` / `Deserialize`** — serde-derived containers
//!    cannot accidentally spill a secret to disk, logs, or the wire. The
//!    compile-fail test named above guards against re-adding the impl.
//!
//! Additional defenses:
//!
//! * `PartialEq` is evaluated in **constant time** via
//!   [`subtle::ConstantTimeEq`]; naive byte-by-byte comparison would leak
//!   token length / prefix through timing side channels when comparing
//!   auth tokens or MAC tags.
//! * [`ExposeSecret::expose_secret`] is the **only** way to reach the
//!   plaintext. Every call site is intentionally visible to reviewers.
//!
//! # Examples
//!
//! ```
//! use pcloud_secret::{ExposeSecret, secret_string::SecretString};
//!
//! let s = SecretString::new("hunter2");
//! assert_eq!(s.expose_secret(), "hunter2");
//! // Debug is always redacted.
//! assert_eq!(format!("{s:?}"), "SecretString(<redacted>)");
//! // Duplication is audit-visible — never `.clone()`.
//! let s2 = s.clone_secret();
//! assert_eq!(s, s2);
//! ```
//!
//! # See also
//!
//! * `SECURITY-MODEL.md` §"Secrets"
//! * ADR 0007 — "Secret handling and audit-visible duplication"

// **PLATFORM:** all
// **GATING:** none (portable).

/// Log-line redaction helpers (audit-friendly `key=<redacted>` tokens).
pub mod redact;
/// Zeroize-on-drop, redacted-`Debug` wrapper around a binary secret.
///
/// See the crate-level docs for the full list of security guarantees.
pub mod secret_bytes;
/// Zeroize-on-drop, redacted-`Debug` wrapper around a UTF-8 secret.
///
/// See the crate-level docs for the full list of security guarantees.
pub mod secret_string;

/// Crate identifier used in audit/telemetry records.
///
/// ```
/// assert_eq!(pcloud_secret::CRATE_NAME, "pcloud-secret");
/// ```
pub const CRATE_NAME: &str = "pcloud-secret";

/// Introspection surface common to every secret wrapper.
///
/// The only information a non-owner is ever allowed to learn is the byte
/// length — never the content itself.
///
/// ```
/// use pcloud_secret::{SecretMaterial, secret_bytes::SecretBytes};
///
/// let b = SecretBytes::new(vec![1, 2, 3, 4]);
/// assert_eq!(b.expose_len(), 4);
/// ```
pub trait SecretMaterial {
    /// Return the byte length of the secret without exposing its content.
    fn expose_len(&self) -> usize;
}

/// Explicit, audit-visible borrow of the underlying secret.
///
/// This is the **only** legitimate way to reach the plaintext of a
/// wrapped secret. The trait is deliberately narrow so every call site
/// that invokes `expose_secret` is grep-able (`rg expose_secret`) in
/// review; an auditor can then confirm the secret is not escaping to a
/// log, a persistence layer, or the wire.
///
/// # Do
///
/// * Hold the returned `&T` only for the minimum scope needed to pass
///   the plaintext to the downstream API.
/// * Prefer comparing via `PartialEq` on the wrapper (constant-time)
///   rather than exposing and comparing plaintext.
///
/// # Do not
///
/// * Do **not** store the returned reference in a struct or move it
///   into a `String`/`Vec<u8>` — that defeats the zeroize-on-drop
///   contract.
/// * Do **not** log the returned value (`tracing::debug!(%exposed)`).
/// * Do **not** return it from a public API without a matching secret
///   wrapper at the boundary.
///
/// ```
/// use pcloud_secret::{ExposeSecret, secret_string::SecretString};
///
/// let token = SecretString::new("t0k3n");
/// let exposed: &str = token.expose_secret();
/// assert_eq!(exposed.len(), 5);
/// ```
pub trait ExposeSecret<T: ?Sized> {
    /// Borrow the underlying secret in plaintext. Every call site is
    /// intentionally grep-able for audit review; see the trait-level
    /// docs for the do / do-not list.
    fn expose_secret(&self) -> &T;
}
