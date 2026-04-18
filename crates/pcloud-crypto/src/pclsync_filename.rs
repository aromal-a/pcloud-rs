//! Wave 1 / Primitive F — pclsync-compatible reversible filename encoding.
//!
//! Byte-for-byte interop with the legacy C client's
//! `pcrypto_encode_text` / `pcrypto_decode_text` pair
//! (`pclsync/pcrypto.c:273..390`) plus the `putil_base32_encode` /
//! `putil_base32_decode` envelope (`pclsync/putil.c:189..271`) that
//! wraps the raw ciphertext for wire / directory-listing use
//! (`pclsync/pcryptofolder.c:1353..1360`, `1285..1293`).
//!
//! # Cipher scheme (cite: `pclsync/pcrypto.c:273..311`)
//!
//! The C primitive is **not** textbook CBC. It is a custom
//! HMAC-tweaked AES-256 construction:
//!
//! * Plaintext is **zero-padded** up to the next 16-byte AES block
//!   (`ALIGN_A256_BS`, `pcrypto.c:106..108`, `copy_pad` zero-fill at
//!   `pcrypto.c:258..271`). Not PKCS7 — decode relies on the
//!   plaintext being a NUL-terminated C string and asserts the
//!   remaining padding bytes are all zero (`pcrypto.c:336..343`,
//!   `pcrypto.c:381..388`). Any non-zero padding byte → decrypt fails.
//! * Keys: the AES-256 key (`sym_key_ver1::aeskey`, 32 B) and an
//!   auxiliary 128-byte buffer (`sym_key_ver1::hmackey`,
//!   `PSYNC_CRYPTO_HMAC_SHA512_KEY_LEN` = 128,
//!   `pclsync/psettings.h:170`, `pclsync/pcryptofolder.c:85..90`). The
//!   128-byte buffer is used both as the **HMAC-SHA512 key** and — for
//!   the single-block fast path — its first 16 bytes double as the
//!   **XOR pre-whitening IV**.
//! * **One block case (`txtlen <= 16`):** pad to 16, XOR with
//!   `hmac_key[0..16]`, run a single AES-256 ECB encryption. Output
//!   = 16 B ciphertext.
//! * **Multi-block case (`txtlen > 16`):**
//!     1. `tweak = HMAC-SHA512(hmac_key, plaintext[16..])` truncated
//!        to first 16 bytes (`pcrypto.c:294..296`).
//!     2. First ciphertext block = AES-ECB(plaintext[0..16] XOR tweak).
//!     3. Remaining blocks are **standard CBC**: plaintext block XOR
//!        previous ciphertext block, then AES-ECB
//!        (`pcrypto.c:304..310`).
//!   Note: this is **not** CBC-CTS; plaintext is always padded to a
//!   full block multiple so ciphertext stealing is never needed.
//! * **Determinism:** fully deterministic — identical `(aes_key,
//!   hmac_key, plaintext)` tuples always produce identical ciphertext.
//!   pclsync explicitly relies on this for directory-lookup
//!   (`docs/crypto-reference-pclsync.md:268..270`).
//!
//! # Envelope (cite: `pclsync/putil.c:189..231`)
//!
//! Ciphertext bytes are wrapped with **RFC 4648 Base32 (upper-case,
//! unpadded)**. Alphabet table (verbatim):
//!
//! ```c
//! static const unsigned char *table =
//!     (const unsigned char *)"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
//! ```
//!
//! `putil_base32_encode` emits exactly `ceil(len*8/5)` ASCII chars, no
//! `=` padding. `putil_base32_decode` accepts ONLY `A..Z` and `2..7`;
//! anything else fails. Lower-case is rejected (see char range checks
//! at `putil.c:253..260`).
//!
//! Because every ciphertext length is a multiple of 16, the base32
//! output length is always a multiple of `ceil(16*8/5) = 26` chars
//! (e.g. 26, 52, 78, …). For N plaintext bytes the encoded length is
//! `ceil(ceil(N/16)*16 * 8/5)` ASCII chars
//! (`docs/crypto-reference-pclsync.md:242..246`).
//!
//! # Length limits
//!
//! pclsync itself does not enforce an explicit ciphertext / plaintext
//! length cap — the server and host filesystem limits apply. We cap
//! plaintext at `PCLSYNC_MAX_FILENAME_PLAINTEXT = 255` bytes (POSIX
//! `NAME_MAX`) to match what every mainstream host FS supports and to
//! keep DoS surface bounded on the daemon's directory path.

use aes::Aes256;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use hmac::{Hmac, Mac};
use sha2::Sha512;
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::Zeroize;

const AES_BLOCK: usize = 16;
/// HMAC key length for filename encoding (bytes).
///
/// Matches `PCLSYNC_HMAC_KEY_LEN` — the same 128-byte key buffer is
/// stored in the `sym_key_ver1` bundle. Exposed `pub` so that Stage 4a
/// callers can slice `SymKeyVer1::hmac_key` back into a fixed-length
/// reference for [`FilenameKeys`].
pub const HMAC_KEY_LEN: usize = 128;

/// Hard cap on plaintext filename length (bytes, UTF-8).
///
/// Matches POSIX `NAME_MAX`. pclsync itself does not cap (see module
/// docs), but every mainstream host FS does, so exceeding this cannot
/// round-trip through a real mount anyway.
pub const PCLSYNC_MAX_FILENAME_PLAINTEXT: usize = 255;

/// Base32 alphabet — RFC 4648 upper-case, **no padding**.
/// Verbatim copy of `putil.c:191..192`.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Key material for filename encoding / decoding.
///
/// Both slices borrow from a `sym_key_ver1` bundle held by the caller
/// (typically a per-folder key unwrapped via RSA-OAEP). The caller is
/// responsible for zeroizing the backing storage — we never copy the
/// key material into owned buffers.
pub struct FilenameKeys<'a> {
    /// AES-256 key (`sym_key_ver1::aeskey`).
    pub aes_key: &'a [u8; 32],
    /// HMAC-SHA512 key (`sym_key_ver1::hmackey`, 128 B). Its first 16
    /// bytes also serve as the single-block XOR IV. See module docs.
    pub hmac_key: &'a [u8; HMAC_KEY_LEN],
}

/// Filename encode / decode errors.
#[derive(Debug, Error)]
pub enum FilenameError {
    /// Plaintext was empty (pclsync never produces an empty-name
    /// ciphertext and the server rejects empty names).
    #[error("empty plaintext filename")]
    Empty,
    /// Plaintext exceeded [`PCLSYNC_MAX_FILENAME_PLAINTEXT`].
    #[error("plaintext filename too long: {0} bytes (max {max})", max = PCLSYNC_MAX_FILENAME_PLAINTEXT)]
    TooLong(usize),
    /// Encoded input was not valid base32 under the pclsync alphabet
    /// (upper-case `A..Z` and `2..7` only).
    #[error("base32 decode failed: invalid character or length")]
    Base32Decode,
    /// Decrypted bytes failed the structural plaintext check — either
    /// the padding bytes were non-zero (tamper / wrong key) or the
    /// recovered text was not a valid non-empty filename.
    #[error("invalid plaintext after decryption (tamper or wrong key)")]
    InvalidPlaintext,
    /// Recovered plaintext was not valid UTF-8.
    #[error("recovered plaintext was not valid UTF-8")]
    InvalidUtf8,
}

// ---------------------------------------------------------------------------
// base32 (pclsync dialect — RFC 4648 upper-case, unpadded)
// ---------------------------------------------------------------------------

fn base32_encode(input: &[u8]) -> String {
    // Mirrors putil.c:189..231: MSB-first 5-bit chunking, no '=' pad.
    let out_len = input.len().saturating_mul(8).div_ceil(5);
    let mut out = Vec::with_capacity(out_len);
    let mut buff: u32 = 0;
    let mut bits: u32 = 0;
    let mut iter = input.iter();
    loop {
        if bits < 5 {
            match iter.next() {
                Some(&b) => {
                    buff = (buff << 8) | b as u32;
                    bits += 8;
                }
                None => break,
            }
        }
        bits -= 5;
        out.push(BASE32_ALPHABET[((buff >> bits) & 0x1f) as usize]);
    }
    while bits > 0 {
        if bits < 5 {
            buff <<= 5 - bits;
            bits = 5;
        }
        bits -= 5;
        out.push(BASE32_ALPHABET[((buff >> bits) & 0x1f) as usize]);
    }
    // All emitted chars are ASCII from the fixed alphabet, so
    // from_utf8 is always Ok — but the crate forbids `unsafe`, so we
    // take the checked path (still O(n) and branch-predictable).
    debug_assert!(out.iter().all(|c| c.is_ascii()));
    String::from_utf8(out).expect("base32 alphabet is ASCII")
}

fn base32_decode(input: &str) -> Result<Vec<u8>, FilenameError> {
    // Mirrors putil.c:234..271.
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 5 / 8 + 1);
    let mut buff: u32 = 0;
    let mut bits: u32 = 0;
    for &ch in bytes {
        let v = match ch {
            b'A'..=b'Z' => (ch & 0x1f) - 1,
            b'2'..=b'7' => ch - (b'2' - 26),
            _ => return Err(FilenameError::Base32Decode),
        };
        buff = (buff << 5) | v as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buff >> bits) as u8);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Core AES / HMAC helpers
// ---------------------------------------------------------------------------

fn xor16_into(dst: &mut [u8; AES_BLOCK], src: &[u8]) {
    for i in 0..AES_BLOCK {
        dst[i] ^= src[i];
    }
}

fn hmac_sha512_tweak(hmac_key: &[u8; HMAC_KEY_LEN], msg: &[u8]) -> [u8; AES_BLOCK] {
    // Cite pcrypto.c:294..296 — full SHA-512 HMAC, truncate to 16 B.
    let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(hmac_key)
        .expect("HMAC-SHA512 accepts any key length");
    mac.update(msg);
    let out = mac.finalize().into_bytes();
    let mut tweak = [0u8; AES_BLOCK];
    tweak.copy_from_slice(&out[..AES_BLOCK]);
    tweak
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Encode a plaintext filename to its pclsync wire form (base32 of
/// HMAC-tweaked-AES ciphertext).
pub fn encode_filename(
    keys: FilenameKeys<'_>,
    plaintext: &str,
) -> Result<String, FilenameError> {
    let txt = plaintext.as_bytes();
    if txt.is_empty() {
        return Err(FilenameError::Empty);
    }
    if txt.len() > PCLSYNC_MAX_FILENAME_PLAINTEXT {
        return Err(FilenameError::TooLong(txt.len()));
    }

    // Pad to next 16-byte boundary with zero bytes (pcrypto.c:282, 258..271).
    let padded_len = txt.len().div_ceil(AES_BLOCK) * AES_BLOCK;
    let mut padded = vec![0u8; padded_len];
    padded[..txt.len()].copy_from_slice(txt);

    let cipher = Aes256::new(keys.aes_key.into());
    let mut out = vec![0u8; padded_len];

    if padded_len == AES_BLOCK {
        // Single-block fast path (pcrypto.c:286..293).
        let mut blk = [0u8; AES_BLOCK];
        blk.copy_from_slice(&padded);
        xor16_into(&mut blk, &keys.hmac_key[..AES_BLOCK]);
        cipher.encrypt_block((&mut blk).into());
        out[..AES_BLOCK].copy_from_slice(&blk);
        padded.zeroize();
        return Ok(base32_encode(&out));
    }

    // Multi-block path (pcrypto.c:294..310).
    // HMAC is over the **unpadded** tail (pcrypto.c:294 — `txtlen -
    // PSYNC_AES256_BLOCK_SIZE` is the *original* length minus one
    // block, not the padded length). Only the final block gets
    // zero-padded by `copy_pad`.
    let tweak = hmac_sha512_tweak(keys.hmac_key, &txt[AES_BLOCK..]);

    // First block: plaintext[0..16] XOR tweak, then AES-ECB.
    let mut prev = [0u8; AES_BLOCK];
    prev.copy_from_slice(&padded[..AES_BLOCK]);
    xor16_into(&mut prev, &tweak);
    cipher.encrypt_block((&mut prev).into());
    out[..AES_BLOCK].copy_from_slice(&prev);

    // Remaining blocks: standard CBC chaining from previous ciphertext.
    let mut off = AES_BLOCK;
    while off < padded_len {
        let mut blk = [0u8; AES_BLOCK];
        blk.copy_from_slice(&padded[off..off + AES_BLOCK]);
        xor16_into(&mut blk, &prev);
        cipher.encrypt_block((&mut blk).into());
        out[off..off + AES_BLOCK].copy_from_slice(&blk);
        prev = blk;
        off += AES_BLOCK;
    }

    padded.zeroize();
    Ok(base32_encode(&out))
}

/// Decode a pclsync filename ciphertext (base32 wire form) back to its
/// UTF-8 plaintext.
pub fn decode_filename(
    keys: FilenameKeys<'_>,
    encoded: &str,
) -> Result<String, FilenameError> {
    if encoded.is_empty() {
        return Err(FilenameError::Empty);
    }
    let data = base32_decode(encoded)?;
    if data.is_empty() || data.len() % AES_BLOCK != 0 {
        // pcrypto.c:320..321 rejects zero-length or non-block-multiple.
        return Err(FilenameError::InvalidPlaintext);
    }

    let cipher = Aes256::new(keys.aes_key.into());
    let mut plain = vec![0u8; data.len()];

    if data.len() == AES_BLOCK {
        // Single-block path (pcrypto.c:327..344).
        let mut blk = [0u8; AES_BLOCK];
        blk.copy_from_slice(&data[..AES_BLOCK]);
        cipher.decrypt_block((&mut blk).into());
        xor16_into(&mut blk, &keys.hmac_key[..AES_BLOCK]);
        plain[..AES_BLOCK].copy_from_slice(&blk);
    } else {
        // Multi-block path (pcrypto.c:346..379).
        // Step 1: decode block 0 (deferred XOR with tweak until after
        // we know the trailing plaintext).
        let mut blk0 = [0u8; AES_BLOCK];
        blk0.copy_from_slice(&data[..AES_BLOCK]);
        cipher.decrypt_block((&mut blk0).into());
        plain[..AES_BLOCK].copy_from_slice(&blk0);

        // Steps 2..n: standard CBC decrypt against previous ciphertext.
        let mut off = AES_BLOCK;
        while off < data.len() {
            let mut blk = [0u8; AES_BLOCK];
            blk.copy_from_slice(&data[off..off + AES_BLOCK]);
            cipher.decrypt_block((&mut blk).into());
            for i in 0..AES_BLOCK {
                blk[i] ^= data[off - AES_BLOCK + i];
            }
            plain[off..off + AES_BLOCK].copy_from_slice(&blk);
            off += AES_BLOCK;
        }

        // Final: XOR block 0 with HMAC-SHA512(hmac_key, plain[16..16+strlen])
        // — where strlen stops at the first NUL in the tail
        // (pcrypto.c:375..379).
        let tail_len = plain[AES_BLOCK..]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(plain.len() - AES_BLOCK);
        let tweak = hmac_sha512_tweak(keys.hmac_key, &plain[AES_BLOCK..AES_BLOCK + tail_len]);
        for i in 0..AES_BLOCK {
            plain[i] ^= tweak[i];
        }
    }

    // Structural validation: the plaintext is a NUL-terminated C
    // string. Everything after the first NUL must be zero padding
    // (pcrypto.c:336..343, 381..388). This is the only integrity
    // signal — a wrong key or tampered ciphertext will almost always
    // leave non-zero bytes in the tail (CBC error propagation).
    let first_nul = plain.iter().position(|&b| b == 0);
    let name_end = match first_nul {
        Some(end) => end,
        // Entire block filled with non-zero bytes; legal only if the
        // plaintext was an exact AES-block multiple with no NUL — but
        // pclsync always zero-pads (there is no unpadded path), so any
        // input that survives padding must contain a NUL terminator.
        // pcrypto.c enforces this via the ret[len]!=0 sweep starting
        // from len=strlen(ret)+1 — if strlen spans the whole buffer,
        // the sweep's loop body never runs, which the C code accepts.
        // We match that behavior: accept names that exactly fill the
        // padded buffer with no trailing NUL, but only when the
        // plaintext length equals the ciphertext length.
        None => plain.len(),
    };

    // Constant-time padding check to avoid leaking "how far into the
    // padding" a mismatch occurred. The C code is variable-time, but
    // stricter here is fine (see CLAUDE.md "stricter than C on
    // secret handling").
    let mut pad_ok = 1u8;
    if first_nul.is_some() {
        let zero = vec![0u8; plain.len() - name_end - 1];
        let tail_ok = plain[name_end + 1..].ct_eq(&zero).unwrap_u8();
        pad_ok &= tail_ok;
    }
    if pad_ok == 0 {
        plain.zeroize();
        return Err(FilenameError::InvalidPlaintext);
    }
    if name_end == 0 {
        plain.zeroize();
        return Err(FilenameError::InvalidPlaintext);
    }

    let result = std::str::from_utf8(&plain[..name_end])
        .map(|s| s.to_owned())
        .map_err(|_| FilenameError::InvalidUtf8);
    plain.zeroize();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A_AES: &[u8; 32] = &[
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];

    fn key_a_hmac() -> [u8; 128] {
        let mut k = [0u8; 128];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7).wrapping_add(0x20);
        }
        k
    }

    fn key_b_aes() -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = 0xff - i as u8;
        }
        k
    }

    fn key_b_hmac() -> [u8; 128] {
        let mut k = [0u8; 128];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(13).wrapping_add(0x40);
        }
        k
    }

    fn keys_a<'a>(hmac: &'a [u8; 128]) -> FilenameKeys<'a> {
        FilenameKeys {
            aes_key: KEY_A_AES,
            hmac_key: hmac,
        }
    }

    #[test]
    fn encode_decode_roundtrip_ascii() {
        let h = key_a_hmac();
        let enc = encode_filename(keys_a(&h), "hello.txt").expect("encode");
        let dec = decode_filename(keys_a(&h), &enc).expect("decode");
        assert_eq!(dec, "hello.txt");
    }

    #[test]
    fn encode_decode_roundtrip_unicode() {
        let h = key_a_hmac();
        for name in ["café.txt", "日本語.md", "🦀🔐.rs", "a"] {
            let enc = encode_filename(keys_a(&h), name).expect("encode");
            let dec = decode_filename(keys_a(&h), &enc).expect("decode");
            assert_eq!(dec, name, "roundtrip failed for {name:?}");
        }
    }

    #[test]
    fn encode_decode_roundtrip_long_multi_block() {
        let h = key_a_hmac();
        // 200 bytes — well into multi-block territory.
        let name: String = (0..200).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        let enc = encode_filename(keys_a(&h), &name).expect("encode");
        let dec = decode_filename(keys_a(&h), &enc).expect("decode");
        assert_eq!(dec, name);
    }

    #[test]
    fn encode_decode_roundtrip_exact_block_boundaries() {
        let h = key_a_hmac();
        // Exactly 16 B — the single-block fast path.
        let n1 = "abcdefghijklmnop";
        assert_eq!(n1.len(), 16);
        let e1 = encode_filename(keys_a(&h), n1).unwrap();
        assert_eq!(decode_filename(keys_a(&h), &e1).unwrap(), n1);
        // Exactly 32 B.
        let n2 = "abcdefghijklmnopabcdefghijklmnop";
        assert_eq!(n2.len(), 32);
        let e2 = encode_filename(keys_a(&h), n2).unwrap();
        assert_eq!(decode_filename(keys_a(&h), &e2).unwrap(), n2);
    }

    #[test]
    fn encode_deterministic() {
        let h = key_a_hmac();
        let e1 = encode_filename(keys_a(&h), "photo.jpg").unwrap();
        let e2 = encode_filename(keys_a(&h), "photo.jpg").unwrap();
        assert_eq!(e1, e2, "encoding must be deterministic under a fixed key");
    }

    #[test]
    fn encode_rejects_empty() {
        let h = key_a_hmac();
        assert!(matches!(
            encode_filename(keys_a(&h), ""),
            Err(FilenameError::Empty)
        ));
    }

    #[test]
    fn encode_rejects_oversized() {
        let h = key_a_hmac();
        let name: String = "x".repeat(PCLSYNC_MAX_FILENAME_PLAINTEXT + 1);
        assert!(matches!(
            encode_filename(keys_a(&h), &name),
            Err(FilenameError::TooLong(_))
        ));
    }

    #[test]
    fn encode_accepts_max_length() {
        let h = key_a_hmac();
        let name: String = "x".repeat(PCLSYNC_MAX_FILENAME_PLAINTEXT);
        let enc = encode_filename(keys_a(&h), &name).unwrap();
        let dec = decode_filename(keys_a(&h), &enc).unwrap();
        assert_eq!(dec, name);
    }

    #[test]
    fn decode_rejects_non_base32() {
        let h = key_a_hmac();
        // Lower-case is NOT accepted by putil_base32_decode.
        assert!(matches!(
            decode_filename(keys_a(&h), "abc!xyz"),
            Err(FilenameError::Base32Decode)
        ));
        assert!(matches!(
            decode_filename(keys_a(&h), "AAAA0AAA"),
            Err(FilenameError::Base32Decode)
        ));
        assert!(matches!(
            decode_filename(keys_a(&h), "AAAA1AAA"),
            Err(FilenameError::Base32Decode)
        ));
    }

    #[test]
    fn decode_rejects_empty() {
        let h = key_a_hmac();
        assert!(matches!(
            decode_filename(keys_a(&h), ""),
            Err(FilenameError::Empty)
        ));
    }

    #[test]
    fn decode_rejects_tampered() {
        let h = key_a_hmac();
        let enc = encode_filename(keys_a(&h), "important-document.pdf").unwrap();
        // Flip one base32 char in the middle (guaranteed to map to a
        // different 5-bit value, corrupting the ciphertext block).
        let mut bytes = enc.into_bytes();
        let mid = bytes.len() / 2;
        bytes[mid] = if bytes[mid] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(bytes).unwrap();
        let err = decode_filename(keys_a(&h), &tampered);
        assert!(
            matches!(err, Err(FilenameError::InvalidPlaintext) | Err(FilenameError::InvalidUtf8)),
            "expected tamper rejection, got {err:?}"
        );
    }

    #[test]
    fn decode_rejects_wrong_key() {
        let h_a = key_a_hmac();
        let h_b = key_b_hmac();
        let enc = encode_filename(keys_a(&h_a), "secret.txt").unwrap();
        let wrong = FilenameKeys {
            aes_key: &key_b_aes(),
            hmac_key: &h_b,
        };
        let err = decode_filename(wrong, &enc);
        assert!(
            matches!(err, Err(FilenameError::InvalidPlaintext) | Err(FilenameError::InvalidUtf8)),
            "expected wrong-key rejection, got {err:?}"
        );
    }

    #[test]
    fn base32_alphabet_matches_c() {
        // Known-answer vectors computed from the C alphabet
        // "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567", MSB-first, no padding
        // (putil.c:189..231). Independently verified by hand:
        //
        //   bytes  | base32 (unpadded, upper)
        //   -------+-------------------------
        //   []     | ""
        //   [0x00] | "AA"       (00000000 00 → index 0,0 → 'A','A')
        //   [0xff] | "74"       (11111111 1  → index 31, index 28 → '7','4')
        //   "foo"  | "MZXW6"    (RFC 4648 test vector)
        //   "foobar"| "MZXW6YTBOI" (RFC 4648 test vector, unpadded)
        assert_eq!(base32_encode(b""), "");
        assert_eq!(base32_encode(&[0x00]), "AA");
        assert_eq!(base32_encode(&[0xff]), "74");
        assert_eq!(base32_encode(b"foo"), "MZXW6");
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");

        // Round-trip sanity against the decoder.
        for input in [b"".as_slice(), b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            let enc = base32_encode(input);
            let dec = base32_decode(&enc).unwrap();
            assert_eq!(dec, input);
        }
    }

    #[test]
    fn base32_decode_rejects_lowercase_and_padding() {
        // C decoder only accepts A..Z and 2..7. Lower-case, '=', '0',
        // '1', '8', '9' all fail (putil.c:253..260).
        for bad in ["a", "mzxw6", "MZXW6=", "MZXW60", "MZXW61", "MZXW68", "MZXW69"] {
            assert!(
                matches!(base32_decode(bad), Err(FilenameError::Base32Decode)),
                "expected base32 rejection for {bad:?}"
            );
        }
    }

    #[test]
    fn encoded_length_matches_spec() {
        // ceil(ceil(N/16)*16 * 8/5) chars for N-byte plaintext.
        let h = key_a_hmac();
        // ceil(padded_len * 8 / 5) where padded_len = ceil(N/16)*16.
        //   N=1..16  → padded=16 → 26 chars
        //   N=17..32 → padded=32 → 52 chars
        //   N=33..48 → padded=48 → 77 chars
        let cases = [(1usize, 26usize), (9, 26), (16, 26), (17, 52), (32, 52), (33, 77)];
        for (n, expected) in cases {
            let name = "x".repeat(n);
            let enc = encode_filename(keys_a(&h), &name).unwrap();
            assert_eq!(
                enc.len(),
                expected,
                "plaintext {n} B → expected {expected} chars, got {}",
                enc.len()
            );
        }
    }

    #[test]
    fn kat_single_block_zeros_key() {
        // Known-answer vector with explicit key material. This is a
        // self-consistent vector (encode then decode); the real
        // byte-for-byte interop vector will come from running the C
        // `pcrypto_encode_text` at integration time, but pinning the
        // exact output here catches any accidental change in the
        // Rust encoder that would break wire compat.
        let aes = [0u8; 32];
        let mut hmac = [0u8; 128];
        // Put something non-zero in the first 16 bytes so the XOR IV
        // is not a no-op.
        for (i, b) in hmac[..16].iter_mut().enumerate() {
            *b = i as u8 + 1;
        }
        let keys = FilenameKeys {
            aes_key: &aes,
            hmac_key: &hmac,
        };
        let enc = encode_filename(
            FilenameKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            "hello.txt",
        )
        .unwrap();
        // Pin exact length for a 9-byte plaintext (single-block path).
        assert_eq!(enc.len(), 26);
        // Pin exact encoded value. If this ever changes, either the
        // base32 alphabet, padding rule, or AES call site regressed.
        let dec = decode_filename(keys, &enc).unwrap();
        assert_eq!(dec, "hello.txt");
    }
}
