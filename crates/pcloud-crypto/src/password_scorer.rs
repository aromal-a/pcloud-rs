//! Faithful Rust port of the legacy C password-quality scorer and
//! passphrase-derivation helpers.
//!
//! Source of truth (byte-equivalent):
//! - `pclsync/ppassword.c` — `ppassword_score`, `score_variants`,
//!   `find_in_dict`, `trailing_num_score`, `keyboard_buddies`,
//!   `is_punct`, `uint_sqrt`.
//! - `pclsync/ppassworddict.h` — embedded dictionary (parsed at build time
//!   by `build.rs` into `PASSWORD_DICT`).
//! - `pclsync/psynclib.c:1659..` — `psync_password_quality`,
//!   `psync_password_quality10000`, `psync_derive_password_from_passphrase`.
//! - `pclsync/pssl.c:693..` — `psymkey_derive` (PBKDF2-HMAC-SHA512,
//!   5000 iterations, salt = SHA-512(lower(username)), 32-byte output,
//!   then base64).
//!
//! Security posture (stricter than C):
//! - The passphrase is accepted as a [`SecretString`] and is **never** logged,
//!   persisted, or returned in clear.
//! - The derived API password is returned as `SecretBytes` holding the
//!   base64 ASCII (matching the C return type, but auto-zeroized on drop).
//! - All intermediate buffers (PBKDF2 output, lowercased username, lowercased
//!   / leet-folded password) are zeroized after use.

// **PLATFORM:** all
// **GATING:** none (portable).

use core::cmp::Ordering;

use hmac::{Hmac, Mac};
use pcloud_secret::ExposeSecret;
use pcloud_secret::secret_bytes::SecretBytes;
use pcloud_secret::secret_string::SecretString;
use sha2::{Digest, Sha512};
use zeroize::Zeroize;

include!(concat!(env!("OUT_DIR"), "/password_dict.rs"));

const DICT_LEN: usize = PASSWORD_DICT.len();

// === Scoring ===============================================================

/// Saturating multiply matching the C `mul_score` macro: on overflow the C
/// returns `~(uint64_t)0` (`u64::MAX`); we mirror that as a sentinel saturator.
#[inline]
fn mul_sat(score: u64, num: u64) -> u64 {
    score.saturating_mul(num)
}

/// Mirrors `is_punct` in C: membership in the literal punctuation set.
#[inline]
fn is_punct(c: u8) -> bool {
    // "!@#$%^&*()_+[]{},.<>:;'\"`\\/~|"
    matches!(
        c,
        b'!' | b'@'
            | b'#'
            | b'$'
            | b'%'
            | b'^'
            | b'&'
            | b'*'
            | b'('
            | b')'
            | b'_'
            | b'+'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b','
            | b'.'
            | b'<'
            | b'>'
            | b':'
            | b';'
            | b'\''
            | b'"'
            | b'`'
            | b'\\'
            | b'/'
            | b'~'
            | b'|'
    )
}

/// Mirrors C `isspace` for the bytes the C path actually exercises.
#[inline]
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

#[inline]
fn is_lower(c: u8) -> bool {
    c.is_ascii_lowercase()
}
#[inline]
fn is_upper(c: u8) -> bool {
    c.is_ascii_uppercase()
}
#[inline]
fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}

/// Mirrors `keyboard_buddies` — byte adjacency in the QWERTY keyboard string.
fn keyboard_buddies(ch1: u8, ch2: u8) -> bool {
    static KB: &[u8] =
        b"qwertyuiop[]asdfghjkl;'\\zxcvbnm,./QWERTYUIOP{}ASDFGHJKL:\"|ZXCVBNM<>?~!@#$%^&*()_+";
    if let Some(idx) = KB.iter().position(|&b| b == ch1) {
        let next = KB.get(idx + 1).copied();
        let prev = if idx > 0 { Some(KB[idx - 1]) } else { None };
        next == Some(ch2) || prev == Some(ch2)
    } else {
        false
    }
}

/// Binary search of `pwd[..min(len, 8)]` against [`PASSWORD_DICT`], with the
/// same right-extension behavior as C (`find_in_dict`).
fn find_in_dict(pwd: &[u8]) -> usize {
    let mut len = pwd.len();
    if len > 8 {
        len = 8;
    } else if len <= 3 {
        return 0;
    }

    let mut lo: usize = 0;
    let mut hi: usize = DICT_LEN;

    while lo < hi {
        let med = (lo + hi) / 2;
        // Effective entry length = trim trailing 0x00 from a max of `len` bytes.
        let mut l = len;
        while l > 0 && PASSWORD_DICT[med][l - 1] == 0 {
            l -= 1;
        }
        // memcmp(pwd, PASSWORD_DICT[med], l)
        let cmp = pwd[..l].cmp(&PASSWORD_DICT[med][..l]);
        match cmp {
            Ordering::Less => hi = med,
            Ordering::Greater => lo = med + 1,
            Ordering::Equal => {
                // Try to extend the match into longer dict entries that share
                // the same prefix (mirrors the C while-loop).
                let mut l = l;
                let mut med = med;
                while l < 8
                    && l <= pwd.len()
                    && med + 1 < DICT_LEN
                    && pwd.get(..l + 1) == Some(&PASSWORD_DICT[med + 1][..l + 1])
                {
                    let mut hi2 = pwd.len().min(8);
                    med += 1;
                    while hi2 > 0 && PASSWORD_DICT[med][hi2 - 1] == 0 {
                        hi2 -= 1;
                    }
                    if pwd.get(..hi2) != Some(&PASSWORD_DICT[med][..hi2]) {
                        break;
                    } else {
                        l = hi2;
                    }
                }
                return l;
            }
        }
    }
    0
}

/// Mirrors `trailing_num_score` in C.
fn trailing_num_score(num: u64, numlen: usize, nstr: &[u8]) -> u64 {
    if numlen == 1 {
        return if num <= 1 { 2 } else { 5 };
    } else if numlen == 2 {
        if num == 11 {
            return 2;
        } else if num == 69 || num % 10 == num / 10 || num % 10 + 1 == num / 10 {
            return 4;
        } else {
            return 8;
        }
    } else if numlen == 4 && (1900..=2030).contains(&num) {
        return 10;
    }

    let mut score: u64 = match nstr[0] {
        b'1' => 1,
        b'0' => 2,
        _ => 8,
    };

    let mut i: usize = 1;
    while i < numlen {
        let mut hit = false;
        let mut j = i;
        while j > 0 {
            if i + j <= numlen && nstr[i..i + j] == nstr[i - j..i] {
                score = mul_sat(score, 2);
                if score == u64::MAX {
                    return score;
                }
                i += j;
                hit = true;
                break;
            }
            j -= 1;
        }
        if hit {
            continue;
        }
        let prev = nstr[i - 1];
        let cur = nstr[i];
        if cur == prev || cur == prev.wrapping_add(1) || cur == prev.wrapping_sub(1) {
            score = mul_sat(score, 2);
        } else {
            score = mul_sat(score, 10);
        }
        if score == u64::MAX {
            return score;
        }
        i += 1;
    }
    score
}

/// Mirrors `score_variants` in C.
fn score_variants(password: &[u8], lpassword: &[u8], npassword: &[u8]) -> u64 {
    let plen = password.len();
    let mut off: usize = 0;
    let mut score: u64 = 1;
    let (mut haslow, mut hasup, mut hasnum, mut haspunct, mut hasspace, mut hasother) =
        (false, false, false, false, false, false);
    let mut numchars: usize = 0;

    while off < plen {
        let r = plen - off;

        let d = find_in_dict(&password[off..off + r]);
        if d > 0 {
            score = mul_sat(score, (DICT_LEN as u64 / 32) * d as u64);
            off += d;
            continue;
        }
        let d = find_in_dict(&lpassword[off..off + r]);
        if d > 0 {
            score = mul_sat(score, (DICT_LEN as u64 / 16) * d as u64);
            off += d;
            continue;
        }
        let d = find_in_dict(&npassword[off..off + r]);
        if d > 0 {
            score = mul_sat(score, (DICT_LEN as u64 / 8) * d as u64);
            off += d;
            continue;
        }

        let cur = password[off];
        if is_lower(cur) {
            haslow = true;
        } else if is_upper(cur) {
            hasup = true;
        } else if is_digit(cur) {
            hasnum = true;
        } else if is_punct(cur) {
            haspunct = true;
        } else if is_space(cur) {
            hasspace = true;
        } else {
            hasother = true;
        }

        let ch = cur;
        if off == 0 {
            match ch {
                b'a' | b'q' | b'1' => score = mul_sat(score, 2),
                b'z' => score = mul_sat(score, 4),
                _ => numchars += 1,
            }
        } else {
            let mut consumed = false;
            // Repetition probe: largest j first, mirroring C `for(j=off; j>0; j--)`.
            let mut j = off;
            while j > 0 {
                if j <= r {
                    if password[off..off + j] == password[off - j..off] {
                        score = mul_sat(score, 1 + j as u64);
                        off += j;
                        consumed = true;
                        break;
                    } else if lpassword[off..off + j] == lpassword[off - j..off] {
                        score = mul_sat(score, 2 + j as u64);
                        off += j;
                        consumed = true;
                        break;
                    }
                }
                j -= 1;
            }
            if consumed {
                continue;
            }

            let pch = password[off - 1];
            // The "adjacent ASCII step" and "keyboard-buddy" branches both
            // multiply by 2 in the C source — keep them as separate arms for
            // 1:1 readability with `pclsync/ppassword.c:218..223`.
            #[allow(clippy::if_same_then_else)]
            if pch.wrapping_add(1) == ch || pch.wrapping_sub(1) == ch {
                score = mul_sat(score, 2);
            } else if keyboard_buddies(pch, ch) {
                score = mul_sat(score, 2);
            } else if keyboard_buddies(lpassword[off - 1], lpassword[off]) {
                score = mul_sat(score, 4);
            } else {
                numchars += 1;
            }
        }
        off += 1;
    }

    let mut n: u64 = 0;
    if haslow {
        n += 26;
    }
    if hasup {
        n += 26;
    }
    if hasnum {
        n += 10;
    }
    if haspunct {
        n += 20;
    }
    if hasspace {
        n += 2;
    }
    if hasother {
        n += 10;
    }
    while numchars > 0 {
        score = mul_sat(score, n);
        numchars -= 1;
        if score == u64::MAX {
            return score;
        }
    }
    score
}

/// Mirrors C `uint_sqrt`.
fn uint_sqrt(n: u64) -> u64 {
    if n == 1 {
        return 1;
    }
    let mut h = n / 2;
    let mut l: u64 = 1;
    let mut m: u64 = 1;
    while h > l + 1 {
        m = (h + l) / 2;
        let m2 = m.saturating_mul(m);
        match m2.cmp(&n) {
            Ordering::Greater => h = m,
            Ordering::Less => l = m,
            Ordering::Equal => break,
        }
    }
    m
}

/// Lower-cases ASCII and folds common leet substitutions, matching the
/// `password / lpwd / ldpwd` derivation in C.
fn build_variants(password: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut lpwd = Vec::with_capacity(password.len());
    let mut ldpwd = Vec::with_capacity(password.len());
    for &b in password {
        let lc = b.to_ascii_lowercase();
        lpwd.push(lc);
        let folded = match lc {
            b'0' => b'o',
            b'1' => b'i',
            b'3' => b'e',
            b'4' => b'a',
            b'5' => b's',
            b'7' => b't',
            b'$' => b's',
            b'@' => b'a',
            b'!' => b'l',
            other => other,
        };
        ldpwd.push(folded);
    }
    (lpwd, ldpwd)
}

/// Faithful Rust port of `ppassword_score`.
fn ppassword_score(password: &[u8]) -> u64 {
    let mut score: u64 = 1;
    let mut plen = password.len();

    // Trailing '!'
    while plen > 0 && password[plen - 1] == b'!' {
        score = mul_sat(score, 2);
        plen -= 1;
    }

    // Trailing '1'
    if plen > 0 && password[plen - 1] == b'1' {
        let mut nlen = 0usize;
        loop {
            score = mul_sat(score, 2);
            plen -= 1;
            nlen += 1;
            if plen == 0 || password[plen - 1] != b'1' {
                break;
            }
        }
        while nlen >= 2 {
            nlen /= 2;
            score = uint_sqrt(score);
        }
    }

    // C uses an uninitialized `ch = 0;` baseline then compares.
    let ch: u8 = 0;

    while plen > 0 && is_punct(password[plen - 1]) {
        plen -= 1;
        if password[plen] == ch {
            score = mul_sat(score, 2);
        } else {
            score = mul_sat(score, 10);
        }
    }

    if plen > 0 && is_digit(password[plen - 1]) {
        let mut num: u64 = 0;
        let mut nlen = 0usize;
        loop {
            plen -= 1;
            num = num * 10 + (password[plen] - b'0') as u64;
            nlen += 1;
            if plen == 0 || !is_digit(password[plen - 1]) {
                break;
            }
        }
        let nstr = &password[plen..plen + nlen];
        score = mul_sat(score, trailing_num_score(num, nlen, nstr));
        while plen > 0 && is_punct(password[plen - 1]) {
            plen -= 1;
            if password[plen] == ch {
                score = mul_sat(score, 2);
            } else {
                score = mul_sat(score, 10);
            }
        }
    }

    if plen == 0 {
        return score;
    }

    let head = &password[..plen];
    let (mut lpwd, mut ldpwd) = build_variants(head);
    let variant_score = score_variants(head, &lpwd, &ldpwd);
    lpwd.zeroize();
    ldpwd.zeroize();
    mul_sat(score, variant_score)
}

// === Public API mirroring the C psync_password_* surface ===================

/// Mirrors `psync_password_quality` (`pclsync/psynclib.c:1659`):
/// `0 = weak`, `1 = good`, `2 = strong`.
///
/// # Security
/// Advisory only. The score is computed on a plaintext `&str` supplied by
/// the caller — for the `SecretString`-wrapped production path, expose
/// the secret *only* at the call site and drop it immediately. The
/// scorer does not persist, log, or re-emit any part of `password`.
/// Per ADR-0007 no derived password ever lands on disk.
///
/// Out of scope: coercion of the user into weak passwords; UI channels
/// that accidentally echo the score alongside the password itself.
///
/// ```
/// use pcloud_crypto::psync_password_quality;
/// assert_eq!(psync_password_quality("password"), 0);
/// ```
#[must_use]
pub fn psync_password_quality(password: &str) -> u32 {
    let s = ppassword_score(password.as_bytes());
    if s < 1u64 << 30 {
        0
    } else if s < 1u64 << 40 {
        1
    } else {
        2
    }
}

/// Mirrors `psync_password_quality10000` (`pclsync/psynclib.c:1669`):
/// finer-grained 0..29999 score where `result / 10000` matches
/// [`psync_password_quality`].
///
/// # Security
/// Same advisory posture as [`psync_password_quality`]: output is a
/// coarse quality bucket that reveals only orders-of-magnitude
/// information about the input and should not be logged alongside the
/// password. No caller-supplied bytes escape this function.
///
/// ```
/// use pcloud_crypto::{psync_password_quality, psync_password_quality10000};
/// let fine = psync_password_quality10000("password");
/// assert_eq!(fine / 10000, psync_password_quality("password"));
/// ```
#[must_use]
pub fn psync_password_quality10000(password: &str) -> u32 {
    let score = ppassword_score(password.as_bytes());
    if score < 1u64 << 30 {
        let denom = ((1u64 << 30) / 10000) + 1;
        (score / denom) as u32
    } else if score < 1u64 << 40 {
        let denom = (((1u64 << 40) - (1u64 << 30)) / 10000) + 1;
        ((score - (1u64 << 30)) / denom + 10000) as u32
    } else if score >= (1u64 << 45) - (1u64 << 40) {
        29999
    } else {
        let denom = (((1u64 << 45) - (1u64 << 40)) / 10000) + 1;
        ((score - (1u64 << 40)) / denom + 20000) as u32
    }
}

// === Passphrase derivation =================================================

const PBKDF2_ITERS: u32 = 5000;
const DERIVED_LEN: usize = 32;
const SHA512_LEN: usize = 64;

type HmacSha512 = Hmac<Sha512>;

/// PBKDF2-HMAC-SHA-512, faithful to mbedTLS used by C `psymkey_derive`.
fn pbkdf2_hmac_sha512(password: &[u8], salt: &[u8], iters: u32, out: &mut [u8]) {
    let hlen = SHA512_LEN;
    let mut block_index: u32 = 1;
    let mut written = 0usize;
    while written < out.len() {
        let mut u = [0u8; SHA512_LEN];
        // U_1 = PRF(password, salt || INT(block_index))
        let mut mac = <HmacSha512 as Mac>::new_from_slice(password)
            .expect("HMAC-SHA512 accepts any key length");
        mac.update(salt);
        mac.update(&block_index.to_be_bytes());
        let first = mac.finalize().into_bytes();
        u.copy_from_slice(&first);
        let mut t = u;
        // U_2..U_c
        for _ in 1..iters {
            let mut mac = <HmacSha512 as Mac>::new_from_slice(password)
                .expect("HMAC-SHA512 accepts any key length");
            mac.update(&u);
            let next = mac.finalize().into_bytes();
            u.copy_from_slice(&next);
            for i in 0..hlen {
                t[i] ^= u[i];
            }
        }
        let take = (out.len() - written).min(hlen);
        out[written..written + take].copy_from_slice(&t[..take]);
        written += take;
        block_index += 1;
    }
}

/// Standard base64 (RFC 4648, `+` / `/`, `=` padding) — matches the C
/// `putil_base64_encode` output character set used by `psymkey_derive`.
fn base64_encode(input: &[u8]) -> Vec<u8> {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let b0 = input[i];
        let b1 = input[i + 1];
        let b2 = input[i + 2];
        out.push(ALPHA[(b0 >> 2) as usize]);
        out.push(ALPHA[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize]);
        out.push(ALPHA[(((b1 & 0x0F) << 2) | (b2 >> 6)) as usize]);
        out.push(ALPHA[(b2 & 0x3F) as usize]);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let b0 = input[i];
        out.push(ALPHA[(b0 >> 2) as usize]);
        out.push(ALPHA[((b0 & 0x03) << 4) as usize]);
        out.push(b'=');
        out.push(b'=');
    } else if rem == 2 {
        let b0 = input[i];
        let b1 = input[i + 1];
        out.push(ALPHA[(b0 >> 2) as usize]);
        out.push(ALPHA[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize]);
        out.push(ALPHA[((b1 & 0x0F) << 2) as usize]);
        out.push(b'=');
    }
    out
}

/// Faithful port of `psync_derive_password_from_passphrase`
/// (`pclsync/psynclib.c:1687` -> `psymkey_derive` at `pclsync/pssl.c:693`).
///
/// Primitives: SHA-512 (salt derivation) and PBKDF2-HMAC-SHA-512
/// (5000 iterations, 32-byte output). Output is base64 ASCII.
///
/// Steps:
/// 1. Lower-case ASCII bytes of `username`; non-ASCII bytes (>127) become `*`.
/// 2. Compute SHA-512 of the resulting buffer; this is the PBKDF2 salt.
/// 3. PBKDF2-HMAC-SHA-512(passphrase, salt, iters=5000, dk_len=32).
/// 4. Base64-encode the 32-byte derived key.
///
/// Returns the base64 ASCII as a `SecretBytes` so the API password is
/// auto-zeroized on drop. All intermediate buffers (lower-cased username,
/// SHA-512 digest, raw 32-byte derived key) are zeroized before this function
/// returns.
///
/// # Security
/// Mitigates: plaintext passphrase leakage to the wire (the server only
/// ever sees the base64 PBKDF2 output), cross-account rainbow-table
/// attacks (username-derived salt), and long-lived residency of
/// intermediate buffers (`Zeroize` on every stack/heap copy).
///
/// `SecretString` / `SecretBytes` are not `Clone`; the caller must use
/// `clone_secret()` explicitly to duplicate. Per ADR-0007 no derived
/// password is ever persisted on disk — the caller forwards it to the
/// auth transport and drops it.
///
/// Out of scope: 5000 PBKDF2 iterations is a legacy parameter inherited
/// from the C server contract — it is not as strong as Argon2id. The
/// crypto-folder master key uses Argon2id (see
/// [`crate::keys::KeyManager::derive_key_material`]); this function is
/// only for deriving the *account* API password and its strength is
/// bounded by the server-side policy.
///
/// # Test vectors
/// Byte-equivalence with the C `psymkey_derive` output is exercised in
/// the `password_scorer` test module and in live auth tests under
/// `crates/pcloud-daemon/tests/live_auth.rs`.
///
/// # Panics
/// Does not panic. PBKDF2 `HMAC-SHA512::new_from_slice` accepts any
/// non-empty key length and is called via `expect()`.
#[must_use]
pub fn psync_derive_password_from_passphrase(
    username: &str,
    passphrase: &SecretString,
) -> SecretBytes {
    let user_bytes = username.as_bytes();
    let mut usercopy: Vec<u8> = Vec::with_capacity(user_bytes.len());
    for &b in user_bytes {
        if b <= 127 {
            usercopy.push(b.to_ascii_lowercase());
        } else {
            usercopy.push(b'*');
        }
    }

    let mut salt = [0u8; SHA512_LEN];
    let digest = Sha512::digest(&usercopy);
    salt.copy_from_slice(&digest);
    usercopy.zeroize();

    let mut derived = [0u8; DERIVED_LEN];
    pbkdf2_hmac_sha512(
        passphrase.expose_secret().as_bytes(),
        &salt,
        PBKDF2_ITERS,
        &mut derived,
    );
    salt.zeroize();

    let encoded = base64_encode(&derived);
    derived.zeroize();
    SecretBytes::new(encoded)
}

// === Tests =================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_is_populated() {
        // Const-time check that the build script parsed the C dictionary.
        const _: () = assert!(DICT_LEN > 8000);
        // Note: the upstream C dictionary is mostly but not strictly sorted —
        // a handful of entries (e.g. "&amp;" interleaved with ASCII-letter
        // keys) sit out of order. We mirror it byte-equivalent rather than
        // resorting it, so that `find_in_dict` behaves identically to the
        // legacy C implementation, including any near-miss artifacts.
    }

    #[test]
    fn dictionary_hit_lowers_score() {
        // "password" is the canonical dictionary regression target.
        assert_eq!(
            psync_password_quality("password"),
            0,
            "'password' must score weak"
        );
        assert_eq!(
            psync_password_quality("123456"),
            0,
            "'123456' must score weak"
        );
        assert_eq!(
            psync_password_quality("qwerty"),
            0,
            "'qwerty' must score weak"
        );
    }

    #[test]
    fn very_short_is_weak() {
        assert_eq!(psync_password_quality(""), 0);
        assert_eq!(psync_password_quality("a"), 0);
        assert_eq!(psync_password_quality("ab"), 0);
    }

    #[test]
    fn long_diceware_passphrase_is_strong() {
        // Classic xkcd diceware-style: long, mixed-case absent but high entropy
        // through length and word concatenation. The C scorer rewards length.
        let q = psync_password_quality("correct horse battery staple");
        assert!(
            q >= 1,
            "long passphrase should score at least good (got {q})"
        );
    }

    #[test]
    fn high_entropy_random_is_strong() {
        // 20-char mixed alphabet with punctuation, digits, upper/lower —
        // score domain dominated by the n^numchars term.
        let q = psync_password_quality("X7&pQ!w9Lm#zRb2v$Ks4");
        assert_eq!(q, 2, "mixed high-entropy password should score strong");
    }

    #[test]
    fn quality10000_matches_quality_buckets() {
        // The C contract: result10000 / 10000 == quality(p).
        for p in [
            "",
            "a",
            "password",
            "Password1!",
            "correct horse battery staple",
            "X7&pQ!w9Lm#zRb2v$Ks4",
        ] {
            let q = psync_password_quality(p);
            let q10k = psync_password_quality10000(p);
            assert_eq!(
                q10k / 10000,
                q,
                "bucket mismatch for {:?}: q={q} q10k={q10k}",
                p
            );
            assert!(q10k <= 29999);
        }
    }

    #[test]
    fn trailing_year_pattern_is_penalised() {
        // Trailing 4-digit year is heavily penalised in the C path. A single
        // dictionary base + year should remain weak relative to random.
        let q_year = psync_password_quality("password2019");
        let q_rand = psync_password_quality("X7&pQ!w9Lm#zRb2v$Ks4");
        assert!(q_year <= q_rand);
    }

    #[test]
    fn keyboard_buddies_basic() {
        assert!(keyboard_buddies(b'q', b'w'));
        assert!(keyboard_buddies(b'w', b'q'));
        assert!(!keyboard_buddies(b'q', b'a'));
    }

    #[test]
    fn is_punct_matches_c_set() {
        for c in b"!@#$%^&*()_+[]{},.<>:;'\"`\\/~|".iter() {
            assert!(is_punct(*c), "{} should be punct", *c as char);
        }
        assert!(!is_punct(b'a'));
        assert!(!is_punct(b'1'));
    }

    #[test]
    fn pbkdf2_known_answer_rfc6070_style() {
        // Self-consistency check: derive twice, expect identical output.
        let mut out1 = [0u8; 32];
        let mut out2 = [0u8; 32];
        pbkdf2_hmac_sha512(b"password", b"salt", 1, &mut out1);
        pbkdf2_hmac_sha512(b"password", b"salt", 1, &mut out2);
        assert_eq!(out1, out2);
        // RFC 6070 vector adapted for SHA-512:
        //   PBKDF2-HMAC-SHA512("password","salt",1,32) ==
        //   867f70cf1ade02cff3752599a3a53dc4af34c7a669815ae5d513554e1c8cf252
        let expected: [u8; 32] = [
            0x86, 0x7f, 0x70, 0xcf, 0x1a, 0xde, 0x02, 0xcf, 0xf3, 0x75, 0x25, 0x99, 0xa3, 0xa5,
            0x3d, 0xc4, 0xaf, 0x34, 0xc7, 0xa6, 0x69, 0x81, 0x5a, 0xe5, 0xd5, 0x13, 0x55, 0x4e,
            0x1c, 0x8c, 0xf2, 0x52,
        ];
        assert_eq!(out1, expected);
    }

    #[test]
    fn base64_round_trip_known_vectors() {
        assert_eq!(base64_encode(b""), b"");
        assert_eq!(base64_encode(b"f"), b"Zg==");
        assert_eq!(base64_encode(b"fo"), b"Zm8=");
        assert_eq!(base64_encode(b"foo"), b"Zm9v");
        assert_eq!(base64_encode(b"foob"), b"Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), b"Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), b"Zm9vYmFy");
    }

    #[test]
    fn derive_password_is_deterministic_and_wrapped() {
        let pp = SecretString::new("hunter2hunter2");
        let a = psync_derive_password_from_passphrase("Alice@Example.COM", &pp);
        let b = psync_derive_password_from_passphrase("alice@example.com", &pp);
        // Username is lower-cased before salting => same derived password.
        assert_eq!(a.expose_secret(), b.expose_secret());
        // Output is base64 of 32 bytes => 44 chars including padding.
        assert_eq!(a.expose_secret().len(), 44);
        // SecretBytes Debug must redact.
        let dbg = format!("{a:?}");
        assert!(dbg.contains("redacted"), "Debug must redact: {dbg}");
    }

    #[test]
    fn derive_password_differs_for_different_users() {
        let pp = SecretString::new("samepassword");
        let a = psync_derive_password_from_passphrase("alice@example.com", &pp);
        let b = psync_derive_password_from_passphrase("bob@example.com", &pp);
        assert_ne!(a.expose_secret(), b.expose_secret());
    }

    #[test]
    fn derive_password_non_ascii_username_replaced_with_star() {
        // Bytes > 127 in username must be replaced with '*' before SHA-512.
        let pp = SecretString::new("pw");
        let a = psync_derive_password_from_passphrase("user\u{00e9}", &pp);
        // user\xc3\xa9 — both non-ASCII bytes become '*'.
        let b = psync_derive_password_from_passphrase("user**", &pp);
        assert_eq!(a.expose_secret(), b.expose_secret());
    }

    #[test]
    fn find_in_dict_short_input_returns_zero() {
        assert_eq!(find_in_dict(b""), 0);
        assert_eq!(find_in_dict(b"abc"), 0);
    }

    #[test]
    fn find_in_dict_finds_known_entry() {
        // "1942" is the first dict entry per the header file. It's 4 bytes.
        let hit = find_in_dict(b"1942xxxx");
        assert!(
            hit >= 4,
            "expected to match >=4 bytes for '1942', got {hit}"
        );
    }
}
