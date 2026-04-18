//! Shared base64 encode/decode helpers for `pcloud-crypto`.
//!
//! These wrappers consolidate the hand-rolled base64 implementations that
//! previously existed in both `password_scorer.rs` and `share_temppass.rs`.
//! All callers within `pcloud-crypto` should use these instead of rolling
//! their own — see LOW-3.Q in the crypto audit.

// **PLATFORM:** all
// **GATING:** none (portable).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

/// Standard RFC 4648 base64 encode (alphabet `+/`, `=` padding).
///
/// Produces the same character set as the legacy C `putil_base64_encode`
/// output used by `psymkey_derive`.
pub fn base64_encode(data: &[u8]) -> String {
    B64.encode(data)
}

/// Standard RFC 4648 base64 decode.
///
/// # Errors
///
/// Returns a [`base64::DecodeError`] if the input contains characters
/// outside the standard alphabet or has invalid padding.
pub fn base64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    B64.decode(s)
}
