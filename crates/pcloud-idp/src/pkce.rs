//! RFC 7636 PKCE helpers (S256 only).
//!
//! The OIDC broker uses PKCE to protect the public-client authorization-code
//! flow from code-interception attacks. Only `S256` is supported; the broker
//! refuses to emit a `plain` challenge because it offers no protection over
//! a pure authorization-code exchange.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::IdpError;

/// Length of the raw random material used for the PKCE code verifier. 32 bytes
/// encodes to 43 unpadded base64url characters, well inside the 43–128 range
/// RFC 7636 §4.1 permits.
pub(crate) const VERIFIER_BYTES: usize = 32;

/// Length of the random CSRF `state` parameter. 16 bytes → 22 unpadded
/// base64url characters, which comfortably exceeds the 128-bit CSRF minimum.
pub(crate) const STATE_BYTES: usize = 16;

/// Length of the OIDC `nonce`. 16 bytes → 128 bits of entropy to bind the
/// request to the returned ID token's `nonce` claim.
pub(crate) const NONCE_BYTES: usize = 16;

/// Fill `buf` with cryptographically strong random bytes via [`getrandom`].
pub(crate) fn random_bytes(buf: &mut [u8]) -> Result<(), IdpError> {
    getrandom::getrandom(buf).map_err(|e| IdpError::Other(format!("rng unavailable: {e}")))
}

/// Generate a fresh URL-safe token of `n` random bytes, base64url-encoded
/// without padding. Used for `code_verifier`, `state`, and `nonce`.
pub(crate) fn random_token(n: usize) -> Result<String, IdpError> {
    let mut buf = vec![0u8; n];
    random_bytes(&mut buf)?;
    Ok(URL_SAFE_NO_PAD.encode(&buf))
}

/// Compute the S256 PKCE challenge: `base64url(sha256(verifier))`.
///
/// ```ignore
/// let c = pcloud_idp::pkce::s256_challenge("abc");
/// assert_eq!(c.len(), 43);
/// ```
#[must_use]
pub fn s256_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let digest = hasher.finalize();
    URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 Appendix B test vector:
    /// verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
    /// challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(s256_challenge(verifier), expected);
    }

    #[test]
    fn random_token_is_unpadded_urlsafe() {
        let t = random_token(32).expect("rng");
        assert!(!t.contains('='));
        assert!(!t.contains('+'));
        assert!(!t.contains('/'));
        // 32 bytes → ceil(32*4/3) = 43 chars without padding.
        assert_eq!(t.len(), 43);
    }
}
