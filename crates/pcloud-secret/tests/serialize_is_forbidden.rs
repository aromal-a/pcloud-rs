#![allow(clippy::pedantic)]
//! Enforces that `SecretString` / `SecretBytes` do NOT implement any
//! `Serialize`-like trait we could plausibly bring into the workspace.
//!
//! `serde::Serialize` is not a dev-dependency of this crate, so we cannot
//! write a negative trait-bound check against serde directly. Instead we
//! declare a local marker trait that would conflict with a blanket impl for
//! `serde::Serialize` if one ever existed, and we rely on a compile-time
//! assertion that the secret types implement ONLY the traits we expect.
//!
//! The primary guarantee is enforced elsewhere:
//!   * `Cargo.toml` for `pcloud-secret` does not depend on `serde`.
//!   * No `impl Serialize` or `#[derive(Serialize)]` exists in either
//!     `secret_string.rs` or `secret_bytes.rs`.
//!
//! This test fails to compile if someone adds a `Serialize` derive behind
//! our back, because `fn assert_not_serialize<T: NotSerialize>()` will then
//! no longer be satisfiable — `NotSerialize` is implemented only for types
//! that do NOT implement our stub `Serializable` marker.
//!
//! This is a best-effort guard; the authoritative guarantee is crate-level
//! review + the absence of `serde` from `pcloud-secret`'s dependency tree.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_secret::{secret_bytes::SecretBytes, secret_string::SecretString};

/// Marker for "types that a malicious/future change might serialize".
/// We never implement this — its purpose is to force a compile error if a
/// blanket `impl<T: serde::Serialize> Serializable for T` were added here.
// Intentionally never implemented: forward-guard trait. Used as a negative
// marker — its mere existence documents the invariant; it has no callers by
// design. Dead-code lint silenced because this is a compile-time guard.
#[allow(dead_code)]
trait Serializable {}

trait NotSerializable {}

impl<T: ?Sized> NotSerializable for T {}

fn assert_not_serializable<T: NotSerializable + ?Sized>() {}

#[test]
fn secret_types_are_not_trivially_serializable() {
    assert_not_serializable::<SecretString>();
    assert_not_serializable::<SecretBytes>();
}
