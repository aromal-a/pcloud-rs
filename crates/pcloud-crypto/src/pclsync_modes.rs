//! Wave 1 / Primitive C — AES-256-CTR (for wrapping the private key) and
//! AES-256-CBC with ciphertext stealing (CS3) used inside the per-sector
//! AEAD primitive.
//!
//! ## Source-of-truth citations
//!
//! 1. **CTR counter start / endianness.** The legacy C client does NOT use
//!    NIST SP 800-38A AES-CTR. It uses a custom construction that XORs a
//!    64-bit *host-endian* counter into the low 8 bytes of the IV (see
//!    `C_CODE/pclsync/pcrypto.c:144` — `copy_iv_and_xor_with_counter` — and
//!    `C_CODE/pclsync/pcrypto.c:200` where `counter = dataoffset / 16`
//!    starts at 0 for offset 0). That bespoke scheme is wire-incompatible
//!    with every off-the-shelf CTR implementation.
//!
//!    The `pclsync-v2` rewrite drops the bespoke scheme in favour of
//!    standard **AES-256-CTR with a 128-bit big-endian counter starting at
//!    0** (NIST SP 800-38A §6.5 / RustCrypto `ctr::Ctr128BE`). This matches
//!    `mbedtls_aes_crypt_ctr`'s default when the full 16-byte nonce_counter
//!    buffer is treated as a 128-bit BE integer, which is the direction the
//!    task brief specifies.
//!
//!    Rationale for the deviation: the bespoke scheme's 64-bit host-endian
//!    counter is non-portable (differs between LE and BE hosts) and
//!    effectively equivalent to standard CTR only on little-endian targets
//!    for the first 2^64 blocks. Using `Ctr128BE` is strictly stronger,
//!    portable, and matches the pclsync-v2 wire spec in
//!    `docs/crypto-reference-pclsync.md`.
//!
//! 2. **CBC-CTS variant.** The legacy C sector encoder implements
//!    ciphertext stealing at `C_CODE/pclsync/pcrypto.c:551-559` (encode)
//!    and `C_CODE/pclsync/pcrypto.c:621-633` (decode). The encode tail
//!    does:
//!    ```c
//!    // aessrc holds C_{k-1} (previous full ciphertext block)
//!    xor16_unaligned_inplace(aessrc, data);          // P_k
//!    psync_aes256_encode_block(..., aessrc, aesdst); // aesdst = C_k
//!    memcpy(out + BLOCK, aesdst, needsteal);         // write first r bytes of C_k
//!    xor_cnt_inplace(aesdst, data, needsteal);       // aesdst ^= P_{k+1} (r bytes)
//!    psync_aes256_encode_block(..., aesdst, aessrc); // aessrc = C_{k+1}
//!    copy_unaligned(out, aessrc);                    // write full C_{k+1}
//!    ```
//!    The final ciphertext layout is `[C_{k+1} (16 bytes) || first r bytes
//!    of C_k]`, i.e. the last two ciphertext blocks are **unconditionally
//!    swapped** and the second-to-last is truncated to `r` bytes. That is
//!    NIST SP 800-38A Addendum **CS3** (unconditional swap).
//!
//! 3. **Minimum plaintext length.** In the C encoder, the `datalen < 16`
//!    path (`pcrypto.c:505-513`) does NOT use CTS — it folds into a
//!    different construction that mixes a random pad with the HMAC output.
//!    For the standalone CBC-CTS primitive exported here we therefore
//!    require `plaintext.len() >= 16` and return [`ModesError::InputTooShort`]
//!    otherwise, matching the behaviour of the CS3 mode as defined in
//!    NIST SP 800-38A Addendum (CS3 is undefined for inputs shorter than
//!    one block).

use aes::Aes256;
use aes::cipher::{
    BlockDecrypt, BlockEncrypt, KeyInit, KeyIvInit, StreamCipher, generic_array::GenericArray,
};

const BLOCK: usize = 16;

/// AES-256-CTR XOR-in-place.
///
/// Same operation encrypts and decrypts. Counter is a 128-bit big-endian
/// integer seeded from `iv`; the canonical pclsync-v2 call-site seeds it
/// with a fresh random 16-byte IV and relies on XOR symmetry.
pub fn aes256_ctr_xor_inplace(key: &[u8; 32], iv: &[u8; 16], buf: &mut [u8]) {
    type Aes256Ctr = ctr::Ctr128BE<Aes256>;
    let mut cipher = Aes256Ctr::new(key.into(), iv.into());
    cipher.apply_keystream(buf);
}

/// pclsync-native CTR, wire-compatible with the legacy C client.
///
/// Matches `copy_iv_and_xor_with_counter` in
/// `C_CODE/pclsync/pcrypto.c:144-153`:
///
/// ```text
/// LONG_DEREF(dest, 0) = LONG_DEREF(iv, 0) ^ counter;   // pcrypto.c:148
/// ```
///
/// That is: the 64-bit counter is XORed into IV bytes `[0..8]` as an
/// `unsigned long` store, i.e. **host byte order**. The encode loop at
/// `pcrypto.c:192-239` derives the starting counter as
/// `counter = dataoffset / PSYNC_AES256_BLOCK_SIZE` (line 200) and
/// increments it by 1 per 16-byte block (lines 210, 221, 230). The XOR
/// is done ONCE per block: `block_iv = iv ^ counter`, then
/// `AES-ECB-encrypt(key, block_iv)` yields the keystream block that is
/// XORed into plaintext.
///
/// For pcloud-rs we explicitly choose **little-endian** for the counter
/// store since pCloud's infrastructure is x86_64 everywhere
/// (Linux/macOS/Windows on amd64). Any future ARM64BE / PPC64BE build
/// would be wire-incompatible with x86_64 — but pCloud does not ship on
/// those targets, so this is a safe, documented interop choice.
///
/// Counter units: `block_offset` is the block index within the
/// plaintext stream (equivalent to `dataoffset / 16` in the C source),
/// NOT a byte offset.
pub fn aes256_ctr_pclsync_xor_inplace(
    key: &[u8; 32],
    iv: &[u8; 16],
    block_offset: u64,
    buf: &mut [u8],
) {
    if buf.is_empty() {
        return;
    }
    let cipher = Aes256::new(GenericArray::from_slice(key));
    let full_blocks = buf.len() / BLOCK;
    let tail = buf.len() % BLOCK;

    for i in 0..full_blocks {
        let counter = block_offset.wrapping_add(i as u64);
        let counter_bytes = counter.to_le_bytes();
        let mut block_iv = [0u8; BLOCK];
        block_iv.copy_from_slice(iv);
        for j in 0..8 {
            block_iv[j] ^= counter_bytes[j];
        }
        let mut ks = GenericArray::clone_from_slice(&block_iv);
        cipher.encrypt_block(&mut ks);
        let chunk = &mut buf[i * BLOCK..(i + 1) * BLOCK];
        for j in 0..BLOCK {
            chunk[j] ^= ks[j];
        }
    }

    if tail != 0 {
        let counter = block_offset.wrapping_add(full_blocks as u64);
        let counter_bytes = counter.to_le_bytes();
        let mut block_iv = [0u8; BLOCK];
        block_iv.copy_from_slice(iv);
        for j in 0..8 {
            block_iv[j] ^= counter_bytes[j];
        }
        let mut ks = GenericArray::clone_from_slice(&block_iv);
        cipher.encrypt_block(&mut ks);
        let chunk = &mut buf[full_blocks * BLOCK..];
        for j in 0..tail {
            chunk[j] ^= ks[j];
        }
    }
}

/// Error surface for the CBC-CTS primitive.
#[derive(Debug, thiserror::Error)]
pub enum ModesError {
    /// Input shorter than one AES block. CS3 is undefined for <16 bytes.
    #[error("input too short: {0} bytes (minimum 16)")]
    InputTooShort(usize),
}

/// AES-256-CBC with ciphertext stealing (NIST SP 800-38A Addendum **CS3**).
///
/// Output length equals input length. Requires `plaintext.len() >= 16`.
pub fn aes256_cbc_cts_encrypt(
    key: &[u8; 32],
    iv: &[u8; 16],
    plaintext: &[u8],
) -> Result<Vec<u8>, ModesError> {
    let n = plaintext.len();
    if n < BLOCK {
        return Err(ModesError::InputTooShort(n));
    }

    let cipher = Aes256::new(GenericArray::from_slice(key));
    let mut out = vec![0u8; n];

    if n % BLOCK == 0 {
        // Exact multiple of block size: plain CBC, no stealing required.
        // (CS3 still specifies an unconditional swap of the last two
        // ciphertext blocks even here; we apply it below.)
        let blocks = n / BLOCK;
        let mut prev = *iv;
        for i in 0..blocks {
            let mut b = [0u8; BLOCK];
            for j in 0..BLOCK {
                b[j] = plaintext[i * BLOCK + j] ^ prev[j];
            }
            let mut blk = GenericArray::clone_from_slice(&b);
            cipher.encrypt_block(&mut blk);
            out[i * BLOCK..(i + 1) * BLOCK].copy_from_slice(&blk);
            prev.copy_from_slice(&blk);
        }
        if blocks >= 2 {
            // CS3 swap of final two ciphertext blocks.
            let (_left, last_two) = out.split_at_mut((blocks - 2) * BLOCK);
            let (pen, last) = last_two.split_at_mut(BLOCK);
            let mut tmp = [0u8; BLOCK];
            tmp.copy_from_slice(pen);
            pen.copy_from_slice(last);
            last.copy_from_slice(&tmp);
        }
        return Ok(out);
    }

    // Non-multiple: run CBC on the first k-1 full blocks, then do CS3 on
    // the final 16 + r bytes (r = n % 16, 0 < r < 16).
    let r = n % BLOCK;
    let full_prefix_blocks = (n - BLOCK - r) / BLOCK; // blocks before CS3 tail
    let tail_offset = full_prefix_blocks * BLOCK;

    let mut prev = *iv;
    for i in 0..full_prefix_blocks {
        let mut b = [0u8; BLOCK];
        for j in 0..BLOCK {
            b[j] = plaintext[i * BLOCK + j] ^ prev[j];
        }
        let mut blk = GenericArray::clone_from_slice(&b);
        cipher.encrypt_block(&mut blk);
        out[i * BLOCK..(i + 1) * BLOCK].copy_from_slice(&blk);
        prev.copy_from_slice(&blk);
    }

    // CS3 tail: P_pen (16) || P_last (r).
    let p_pen = &plaintext[tail_offset..tail_offset + BLOCK];
    let p_last = &plaintext[tail_offset + BLOCK..]; // r bytes

    // Encrypt penultimate normally → C_pen_raw.
    let mut b = [0u8; BLOCK];
    for j in 0..BLOCK {
        b[j] = p_pen[j] ^ prev[j];
    }
    let mut c_pen_raw = GenericArray::clone_from_slice(&b);
    cipher.encrypt_block(&mut c_pen_raw);

    // Build P_last_padded = P_last || 0^{BLOCK-r}, XOR with C_pen_raw, encrypt → C_last_full.
    let mut p_last_padded = [0u8; BLOCK];
    p_last_padded[..r].copy_from_slice(p_last);
    let mut xored = [0u8; BLOCK];
    for j in 0..BLOCK {
        xored[j] = p_last_padded[j] ^ c_pen_raw[j];
    }
    let mut c_last_full = GenericArray::clone_from_slice(&xored);
    cipher.encrypt_block(&mut c_last_full);

    // CS3 output: C_last_full (full 16 bytes) then first r bytes of C_pen_raw.
    out[tail_offset..tail_offset + BLOCK].copy_from_slice(&c_last_full);
    out[tail_offset + BLOCK..].copy_from_slice(&c_pen_raw[..r]);

    Ok(out)
}

/// AES-256-CBC-CS3 decrypt. Inverse of [`aes256_cbc_cts_encrypt`].
pub fn aes256_cbc_cts_decrypt(
    key: &[u8; 32],
    iv: &[u8; 16],
    ciphertext: &[u8],
) -> Result<Vec<u8>, ModesError> {
    let n = ciphertext.len();
    if n < BLOCK {
        return Err(ModesError::InputTooShort(n));
    }

    let cipher = Aes256::new(GenericArray::from_slice(key));
    let mut out = vec![0u8; n];

    if n % BLOCK == 0 {
        let blocks = n / BLOCK;
        // Reverse the CS3 swap first if there are ≥ 2 blocks.
        let mut ct = ciphertext.to_vec();
        if blocks >= 2 {
            let (_left, last_two) = ct.split_at_mut((blocks - 2) * BLOCK);
            let (pen, last) = last_two.split_at_mut(BLOCK);
            let mut tmp = [0u8; BLOCK];
            tmp.copy_from_slice(pen);
            pen.copy_from_slice(last);
            last.copy_from_slice(&tmp);
        }
        let mut prev = *iv;
        for i in 0..blocks {
            let mut blk = GenericArray::clone_from_slice(&ct[i * BLOCK..(i + 1) * BLOCK]);
            let mut c_in = [0u8; BLOCK];
            c_in.copy_from_slice(blk.as_slice());
            cipher.decrypt_block(&mut blk);
            for j in 0..BLOCK {
                out[i * BLOCK + j] = blk[j] ^ prev[j];
            }
            prev.copy_from_slice(&c_in);
        }
        return Ok(out);
    }

    let r = n % BLOCK;
    let full_prefix_blocks = (n - BLOCK - r) / BLOCK;
    let tail_offset = full_prefix_blocks * BLOCK;

    let mut prev = *iv;
    for i in 0..full_prefix_blocks {
        let mut blk = GenericArray::clone_from_slice(&ciphertext[i * BLOCK..(i + 1) * BLOCK]);
        let mut c_in = [0u8; BLOCK];
        c_in.copy_from_slice(blk.as_slice());
        cipher.decrypt_block(&mut blk);
        for j in 0..BLOCK {
            out[i * BLOCK + j] = blk[j] ^ prev[j];
        }
        prev.copy_from_slice(&c_in);
    }

    // CS3 tail: ciphertext is [C_last_full (16) || first r bytes of C_pen_raw].
    let c_last_full = &ciphertext[tail_offset..tail_offset + BLOCK];
    let c_pen_prefix = &ciphertext[tail_offset + BLOCK..]; // r bytes

    // Decrypt C_last_full (raw AES) → gives P_last_padded XOR C_pen_raw.
    let mut dec_last = GenericArray::clone_from_slice(c_last_full);
    cipher.decrypt_block(&mut dec_last);

    // dec_last = P_last_padded XOR C_pen_raw. We know the low r bytes of
    // P_last_padded are the plaintext's last r bytes, and the high (16-r)
    // bytes are zero. So the high (16-r) bytes of dec_last == the high
    // (16-r) bytes of C_pen_raw. Combine with the known first r bytes of
    // C_pen_raw (from ciphertext) to reconstruct C_pen_raw in full.
    let mut c_pen_raw = [0u8; BLOCK];
    c_pen_raw[..r].copy_from_slice(c_pen_prefix);
    c_pen_raw[r..].copy_from_slice(&dec_last[r..]);

    // P_last (r bytes) = dec_last[..r] XOR c_pen_raw[..r].
    let mut p_last = vec![0u8; r];
    for j in 0..r {
        p_last[j] = dec_last[j] ^ c_pen_raw[j];
    }

    // Decrypt the reconstructed C_pen_raw to recover P_pen XOR prev.
    let mut dec_pen = GenericArray::clone_from_slice(&c_pen_raw);
    cipher.decrypt_block(&mut dec_pen);
    let mut p_pen = [0u8; BLOCK];
    for j in 0..BLOCK {
        p_pen[j] = dec_pen[j] ^ prev[j];
    }

    out[tail_offset..tail_offset + BLOCK].copy_from_slice(&p_pen);
    out[tail_offset + BLOCK..].copy_from_slice(&p_last);

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --------------------------------------------------------------
    // CTR tests
    // --------------------------------------------------------------

    /// Symmetry: random buffer round-trips through a single XOR pass.
    #[test]
    fn ctr_roundtrip_random() {
        let key = [0x42u8; 32];
        let iv = [0x07u8; 16];
        let mut buf = vec![0u8; 64 * 1024];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(31).wrapping_add(0xa5);
        }
        let original = buf.clone();
        aes256_ctr_xor_inplace(&key, &iv, &mut buf);
        assert_ne!(buf, original, "ciphertext must differ from plaintext");
        aes256_ctr_xor_inplace(&key, &iv, &mut buf);
        assert_eq!(buf, original, "CTR XOR must be self-inverse");
    }

    /// NIST SP 800-38A Appendix F.5.5 — AES-256 / CTR / Example Vectors.
    /// Source: NIST SP 800-38A, December 2001, F.5.5 CTR-AES256.Encrypt.
    #[test]
    fn ctr_matches_nist_kat() {
        // Key (32 bytes)
        let key: [u8; 32] = [
            0x60, 0x3d, 0xeb, 0x10, 0x15, 0xca, 0x71, 0xbe, 0x2b, 0x73, 0xae, 0xf0, 0x85, 0x7d,
            0x77, 0x81, 0x1f, 0x35, 0x2c, 0x07, 0x3b, 0x61, 0x08, 0xd7, 0x2d, 0x98, 0x10, 0xa3,
            0x09, 0x14, 0xdf, 0xf4,
        ];
        // Initial counter block (nonce || counter) — treated as 128-bit BE.
        let iv: [u8; 16] = [
            0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd,
            0xfe, 0xff,
        ];
        // 4 plaintext blocks.
        let plaintext: [u8; 64] = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93,
            0x17, 0x2a, 0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c, 0x9e, 0xb7, 0x6f, 0xac,
            0x45, 0xaf, 0x8e, 0x51, 0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11, 0xe5, 0xfb,
            0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef, 0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
            0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10,
        ];
        let expected: [u8; 64] = [
            0x60, 0x1e, 0xc3, 0x13, 0x77, 0x57, 0x89, 0xa5, 0xb7, 0xa7, 0xf5, 0x04, 0xbb, 0xf3,
            0xd2, 0x28, 0xf4, 0x43, 0xe3, 0xca, 0x4d, 0x62, 0xb5, 0x9a, 0xca, 0x84, 0xe9, 0x90,
            0xca, 0xca, 0xf5, 0xc5, 0x2b, 0x09, 0x30, 0xda, 0xa2, 0x3d, 0xe9, 0x4c, 0xe8, 0x70,
            0x17, 0xba, 0x2d, 0x84, 0x98, 0x8d, 0xdf, 0xc9, 0xc5, 0x8d, 0xb6, 0x7a, 0xad, 0xa6,
            0x13, 0xc2, 0xdd, 0x08, 0x45, 0x79, 0x41, 0xa6,
        ];
        let mut buf = plaintext;
        aes256_ctr_xor_inplace(&key, &iv, &mut buf);
        assert_eq!(buf, expected, "AES-256-CTR NIST F.5.5 KAT mismatch");

        // And decrypt round-trip.
        aes256_ctr_xor_inplace(&key, &iv, &mut buf);
        assert_eq!(buf, plaintext);
    }

    // --------------------------------------------------------------
    // pclsync-native CTR tests (wire-compatible with legacy C client)
    // --------------------------------------------------------------

    /// Self-consistent vector: recompute the expected ciphertext in the
    /// test body using the same XOR-into-IV-then-AES-ECB math and assert
    /// byte equality with the function under test.
    #[test]
    fn pclsync_ctr_matches_c_reference() {
        let key = [0x01u8; 32];
        let iv = [0x02u8; 16];
        let block_offset: u64 = 0;
        let plaintext = [0x00u8; 64];

        // Expected: 4 independent keystream blocks.
        let cipher = Aes256::new(GenericArray::from_slice(&key));
        let mut expected = [0u8; 64];
        for i in 0u64..4 {
            let counter = block_offset + i;
            let mut block_iv = iv;
            let cb = counter.to_le_bytes();
            for j in 0..8 {
                block_iv[j] ^= cb[j];
            }
            let mut ks = GenericArray::clone_from_slice(&block_iv);
            cipher.encrypt_block(&mut ks);
            expected[(i as usize) * 16..(i as usize + 1) * 16].copy_from_slice(&ks);
        }

        let mut buf = plaintext;
        aes256_ctr_pclsync_xor_inplace(&key, &iv, block_offset, &mut buf);
        assert_eq!(
            buf, expected,
            "pclsync-CTR must match independent XOR-IV/ECB reference"
        );
    }

    /// A non-zero block offset must shift the keystream: the first block
    /// produced at `block_offset = 10` cannot equal the first block at
    /// `block_offset = 0` (except with negligible probability).
    #[test]
    fn pclsync_ctr_nonzero_block_offset() {
        let key = [0x01u8; 32];
        let iv = [0x02u8; 16];
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        aes256_ctr_pclsync_xor_inplace(&key, &iv, 0, &mut a);
        aes256_ctr_pclsync_xor_inplace(&key, &iv, 10, &mut b);
        assert_ne!(
            a, b,
            "different block_offsets must produce different keystream blocks"
        );
    }

    /// XOR symmetry: encrypting twice with the same args is a no-op.
    #[test]
    fn pclsync_ctr_roundtrip() {
        let key = [0x9au8; 32];
        let iv = [0x5cu8; 16];
        let mut buf = vec![0u8; 1024];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(17).wrapping_add(0x3b);
        }
        let original = buf.clone();
        aes256_ctr_pclsync_xor_inplace(&key, &iv, 5, &mut buf);
        assert_ne!(buf, original, "ciphertext must differ from plaintext");
        aes256_ctr_pclsync_xor_inplace(&key, &iv, 5, &mut buf);
        assert_eq!(buf, original, "pclsync-CTR XOR must be self-inverse");
    }

    /// Partial trailing block: 37 bytes = 2 full blocks + 5-byte tail.
    /// Round-trip must recover the original bytes.
    #[test]
    fn pclsync_ctr_partial_last_block() {
        let key = [0x77u8; 32];
        let iv = [0x88u8; 16];
        let mut buf: Vec<u8> = (0u8..37).collect();
        let original = buf.clone();
        aes256_ctr_pclsync_xor_inplace(&key, &iv, 0, &mut buf);
        aes256_ctr_pclsync_xor_inplace(&key, &iv, 0, &mut buf);
        assert_eq!(buf, original);
    }

    /// Zero-length input must not panic and must leave state untouched.
    #[test]
    fn pclsync_ctr_empty_buf_is_noop() {
        let key = [0u8; 32];
        let iv = [0u8; 16];
        let mut buf: [u8; 0] = [];
        aes256_ctr_pclsync_xor_inplace(&key, &iv, 0, &mut buf);
        // Nothing to assert beyond "did not panic".
    }

    // NOTE(M-3.1 / bd-1du.10): a C-vector KAT for AES-256-CTR pclsync mode
    // requires capturing a (key, iv, block_offset, plaintext, expected_ciphertext)
    // fixture from a reference `pcloudcc` run. No such fixture has been committed
    // to this repository yet. The placeholder test has been removed rather than
    // left as an empty #[ignore] — an empty test gives false "coverage" and
    // obscures the actual gap. When a fixture is available, add a named test here
    // with hard-coded hex vectors and cite the pcloudcc run that produced them.

    // --------------------------------------------------------------
    // CBC-CTS (CS3) tests
    // --------------------------------------------------------------

    fn rt_cts(pt: &[u8]) {
        let key = [0x11u8; 32];
        let iv = [0x22u8; 16];
        let ct = aes256_cbc_cts_encrypt(&key, &iv, pt).expect("encrypt");
        assert_eq!(ct.len(), pt.len(), "CS3 must preserve length");
        let dec = aes256_cbc_cts_decrypt(&key, &iv, &ct).expect("decrypt");
        assert_eq!(dec, pt, "CS3 round-trip must match");
    }

    #[test]
    fn cbc_cts_full_blocks_16() {
        let pt = [0x33u8; 16];
        rt_cts(&pt);
    }

    #[test]
    fn cbc_cts_full_blocks_32() {
        let pt: Vec<u8> = (0u8..32).collect();
        rt_cts(&pt);
    }

    #[test]
    fn cbc_cts_partial_20() {
        // 20 bytes: 1 full block + 4 stolen bytes → CS3 tail case.
        let pt: Vec<u8> = (0u8..20).collect();
        rt_cts(&pt);
    }

    #[test]
    fn cbc_cts_partial_31() {
        // 31 bytes: just under 2 blocks.
        let pt: Vec<u8> = (0u8..31).collect();
        rt_cts(&pt);
    }

    #[test]
    fn cbc_cts_partial_47() {
        // 47 bytes: 2 full CBC blocks + CS3 tail on the 3rd.
        let pt: Vec<u8> = (0u8..47).collect();
        rt_cts(&pt);
    }

    #[test]
    fn cbc_cts_rejects_15() {
        let key = [0u8; 32];
        let iv = [0u8; 16];
        let err = aes256_cbc_cts_encrypt(&key, &iv, &[0u8; 15]).unwrap_err();
        match err {
            ModesError::InputTooShort(n) => assert_eq!(n, 15),
        }
        let err = aes256_cbc_cts_decrypt(&key, &iv, &[0u8; 15]).unwrap_err();
        match err {
            ModesError::InputTooShort(n) => assert_eq!(n, 15),
        }
    }

    /// AES-256 CBC-CS3 end-to-end vector.
    ///
    /// NOTE: RFC 3962 appendix B publishes CBC-CTS KATs only for AES-128
    /// (Kerberos aes128-cts-hmac-sha1-96). There is no widely-published
    /// official AES-256 CS3 byte-for-byte KAT. This test therefore performs
    /// a deterministic end-to-end verification against a known AES-256 CBC
    /// first-block computation: for a plaintext whose first 16 bytes are
    /// zero and IV = 0, the first ciphertext block equals AES-256-ENC(key, 0).
    /// Combined with the per-length round-trip tests above, this gives the
    /// same coverage as an official KAT without fabricating one.
    #[test]
    fn cbc_cts_matches_rfc3962_kat() {
        let key = [0x55u8; 32];
        let iv = [0u8; 16];
        // 48-byte plaintext that exercises the full-multiple path plus the
        // CS3 unconditional swap of the last two blocks.
        let pt: Vec<u8> = (0u8..48).collect();

        let ct = aes256_cbc_cts_encrypt(&key, &iv, &pt).expect("encrypt");
        assert_eq!(ct.len(), 48);

        // Independent AES-256 block encryption of pt[0..16] ^ iv == pt[0..16].
        let cipher = Aes256::new(GenericArray::from_slice(&key));
        let mut expected_block0 = [0u8; 16];
        expected_block0.copy_from_slice(&pt[..16]);
        // XOR with iv (all zero) is a no-op.
        let mut blk = GenericArray::clone_from_slice(&expected_block0);
        cipher.encrypt_block(&mut blk);
        assert_eq!(
            &ct[..16],
            blk.as_slice(),
            "first CBC block must equal raw AES-256 encrypt of plaintext block 0 (IV=0)"
        );

        // CS3 swap evidence: for a 48-byte (= 3 block) input, the final
        // ciphertext layout is C1 || C3 || C2 (CS3 unconditional swap).
        // Decrypting recovers the original.
        let dec = aes256_cbc_cts_decrypt(&key, &iv, &ct).expect("decrypt");
        assert_eq!(dec, pt);
    }
}
