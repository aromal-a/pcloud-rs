#![forbid(unsafe_code)]
//! # pclsync-compatible password-to-KEK derivation (Wave 1, Primitive A)
//!
//! Implements the exact KDF contract used by the legacy pclsync C client to
//! derive the AES-256-CTR key + IV that wraps the user's RSA-4096 private key
//! on the server (`priv_key_ver1`). Every parameter here is part of the pCloud
//! wire contract and is therefore **non-negotiable** — changing any of them
//! silently yields a KEK that cannot unwrap a server-stored private key.
//!
//! ## Wire contract (cite: `pclsync/psettings.h:168`, `pclsync/pcryptofolder.c:381..385`)
//!
//! | Parameter      | Value                          |
//! |----------------|--------------------------------|
//! | KDF            | PBKDF2-HMAC-SHA512             |
//! | Iterations     | 20 000                         |
//! | Salt length    | 64 bytes                       |
//! | Output length  | 48 bytes (32 = AES key, 16 = IV) |
//!
//! The C reference (`psymkey_generate` called from
//! `pcryptofolder.c:383..385`) invokes
//! `pssl_derive_key_sha512(password, salt, 64, 20000, buf, 48)` and then
//! splits `buf` as `buf[0..32] = AES-256 key` and `buf[32..48] = IV` used
//! for the CTR cipher over the RSA private blob.
//!
//! See also `docs/crypto-reference-pclsync.md §1.1` and the KAT seed in §8.
//!
//! ## Security posture
//! - Password is consumed through [`pcloud_secret::SecretString`] — the
//!   caller never hands us a raw `String`.
//! - The PBKDF2 output buffer lives inside a [`Dk48`] newtype that derives
//!   [`ZeroizeOnDrop`] so the full 48-byte intermediate (which is exactly the
//!   KEK material) is scrubbed when the function returns.
//! - The public [`UnlockedKek`] likewise derives [`ZeroizeOnDrop`] so the
//!   eventual consumer cannot accidentally leak the KEK into a heap leak or
//!   a core dump longer than its stack frame.
//! - `#![forbid(unsafe_code)]` is enforced at module scope.

// **PLATFORM:** all
// **GATING:** feature = "pclsync-v2" (Wave 1 scaffold; not yet on the active
// unlock path).

use hmac::Hmac;
use pbkdf2::pbkdf2;
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use sha2::Sha512;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// PBKDF2 iteration count. Matches
/// `PSYNC_CRYPTO_PASS_TO_KEY_ITERATIONS` in `pclsync/psettings.h:168`.
pub const PCLSYNC_PBKDF2_ITERATIONS: u32 = 20_000;

/// Salt length in bytes. Matches `PSYNC_CRYPTO_PBKDF2_SALT_LEN`
/// (`pclsync/psettings.h:169`).
pub const PCLSYNC_PBKDF2_SALT_LEN: usize = 64;

/// AES-256 key length. Matches `PSYNC_AES256_KEY_SIZE`.
pub const PCLSYNC_AES_KEY_LEN: usize = 32;

/// AES-CTR IV / block length. Matches `PSYNC_AES256_BLOCK_SIZE`.
pub const PCLSYNC_AES_IV_LEN: usize = 16;

/// Total PBKDF2 output in bytes (`KEY_LEN + IV_LEN`). The C client derives
/// exactly this many bytes in one PBKDF2 call and then splits the buffer.
const PCLSYNC_DK_LEN: usize = PCLSYNC_AES_KEY_LEN + PCLSYNC_AES_IV_LEN; // 48

/// Unlocked key-encryption-key material derived from a user password.
///
/// Holds the AES-256 key and the 16-byte CTR IV the pclsync C client uses
/// to unwrap a `priv_key_ver1` RSA private blob. Zeroized on drop.
#[derive(ZeroizeOnDrop)]
pub struct UnlockedKek {
    /// AES-256 key (first 32 bytes of the PBKDF2 output).
    pub key: [u8; PCLSYNC_AES_KEY_LEN],
    /// CTR IV (last 16 bytes of the PBKDF2 output).
    pub iv: [u8; PCLSYNC_AES_IV_LEN],
}

/// Fixed-size zeroizing wrapper around the raw 48-byte PBKDF2 output.
///
/// Kept private to this module — callers interact with [`UnlockedKek`] only.
#[derive(ZeroizeOnDrop)]
struct Dk48([u8; PCLSYNC_DK_LEN]);

/// Derive the pclsync KEK (AES-256 key + IV) from `password` and `salt`.
///
/// Implements `PBKDF2-HMAC-SHA512(password, salt, 20000, 48)` and splits the
/// 48-byte output into `(key || iv)` matching the C client's
/// `psymkey_generate` call at `pclsync/pcryptofolder.c:383..385`.
///
/// # Panics
/// Never. `pbkdf2::pbkdf2` with a fixed output length on a valid HMAC primitive
/// cannot fail.
#[must_use]
pub fn derive_kek(password: &SecretString, salt: &[u8; PCLSYNC_PBKDF2_SALT_LEN]) -> UnlockedKek {
    let mut dk = Dk48([0u8; PCLSYNC_DK_LEN]);

    // PBKDF2-HMAC-SHA512. The `pbkdf2` crate's non-generic free function uses
    // `Hmac<D>` internally and writes exactly `dk.0.len()` bytes.
    pbkdf2::<Hmac<Sha512>>(
        password.expose_secret().as_bytes(),
        salt,
        PCLSYNC_PBKDF2_ITERATIONS,
        &mut dk.0,
    )
    .expect("pbkdf2::<Hmac<Sha512>> is infallible for a fixed non-empty output length");

    let mut key = [0u8; PCLSYNC_AES_KEY_LEN];
    let mut iv = [0u8; PCLSYNC_AES_IV_LEN];
    key.copy_from_slice(&dk.0[..PCLSYNC_AES_KEY_LEN]);
    iv.copy_from_slice(&dk.0[PCLSYNC_AES_KEY_LEN..PCLSYNC_DK_LEN]);

    // `dk` is ZeroizeOnDrop; explicitly zeroize now to shrink the window.
    dk.0.zeroize();

    UnlockedKek { key, iv }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::hex;

    fn sec(s: &str) -> SecretString {
        SecretString::new(s.to_string())
    }

    #[test]
    fn derive_kek_deterministic_for_same_input() {
        let salt = [0x11u8; PCLSYNC_PBKDF2_SALT_LEN];
        let a = derive_kek(&sec("correct horse battery staple"), &salt);
        let b = derive_kek(&sec("correct horse battery staple"), &salt);
        assert_eq!(a.key, b.key, "KEK key must be deterministic");
        assert_eq!(a.iv, b.iv, "KEK iv must be deterministic");
    }

    #[test]
    fn derive_kek_differs_when_salt_differs() {
        let pw = sec("correct horse battery staple");
        let salt_a = [0x11u8; PCLSYNC_PBKDF2_SALT_LEN];
        let mut salt_b = [0x11u8; PCLSYNC_PBKDF2_SALT_LEN];
        salt_b[0] ^= 0x01; // flip one bit
        let a = derive_kek(&pw, &salt_a);
        let b = derive_kek(&pw, &salt_b);
        assert_ne!(a.key, b.key, "salt change must perturb key");
        assert_ne!(a.iv, b.iv, "salt change must perturb iv");
    }

    #[test]
    fn derive_kek_differs_when_password_differs() {
        let salt = [0x22u8; PCLSYNC_PBKDF2_SALT_LEN];
        let a = derive_kek(&sec("password-a"), &salt);
        let b = derive_kek(&sec("password-b"), &salt);
        assert_ne!(a.key, b.key, "password change must perturb key");
        assert_ne!(a.iv, b.iv, "password change must perturb iv");
    }

    #[test]
    fn derive_kek_48_bytes_total_output() {
        // Guards against a future refactor that accidentally doubles or
        // truncates the PBKDF2 output. KEY_LEN + IV_LEN must be exactly 48.
        assert_eq!(PCLSYNC_AES_KEY_LEN, 32);
        assert_eq!(PCLSYNC_AES_IV_LEN, 16);
        assert_eq!(PCLSYNC_AES_KEY_LEN + PCLSYNC_AES_IV_LEN, 48);

        let salt = [0u8; PCLSYNC_PBKDF2_SALT_LEN];
        let k = derive_kek(&sec("x"), &salt);
        // Array-type invariants (size known at compile time, but assert anyway
        // so a future change to the struct layout breaks the test explicitly).
        assert_eq!(k.key.len(), 32);
        assert_eq!(k.iv.len(), 16);
        assert_eq!(k.key.len() + k.iv.len(), 48);
    }

    /// Known-answer test locked to the spec's §8 seed #1:
    /// `PBKDF2-HMAC-SHA512(password="test", salt=64×0x00, iters=20000, dkLen=48)`.
    ///
    /// Expected bytes were computed with Python's stdlib `hashlib.pbkdf2_hmac`
    /// (independent implementation from the RustCrypto `pbkdf2` crate used by
    /// the code under test):
    ///
    /// ```python
    /// import hashlib
    /// hashlib.pbkdf2_hmac('sha512', b'test', b'\x00'*64, 20000, 48).hex()
    /// # -> 700379079da995a71a91d1ee64c990a170e4004407f62a5f5a91532729b34351
    /// #    712374698a714cdf9d2292cf949ab6ee
    /// ```
    #[test]
    fn kat_test_vector_from_spec() {
        const EXPECTED: [u8; 48] = hex!(
            "700379079da995a71a91d1ee64c990a1"
            "70e4004407f62a5f5a91532729b34351"
            "712374698a714cdf9d2292cf949ab6ee"
        );

        let salt = [0u8; PCLSYNC_PBKDF2_SALT_LEN];
        let kek = derive_kek(&sec("test"), &salt);

        let mut got = [0u8; 48];
        got[..32].copy_from_slice(&kek.key);
        got[32..].copy_from_slice(&kek.iv);

        assert_eq!(
            got, EXPECTED,
            "pclsync KAT mismatch — KDF output does not match the spec vector"
        );
        assert_eq!(&EXPECTED[..32], &kek.key);
        assert_eq!(&EXPECTED[32..], &kek.iv);
    }
}
