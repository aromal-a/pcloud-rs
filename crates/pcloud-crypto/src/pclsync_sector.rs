//! Wave 1 / Primitive D — pclsync-compatible per-sector AEAD.
//!
//! Byte-for-byte reimplementation of `pcrypto_encode_sec` /
//! `pcrypto_decode_sec` from `C_CODE/pclsync/pcrypto.c` (lines 487–642).
//!
//! # Algorithm (confirmed against C source)
//!
//! Inputs:
//!
//! - `aes_key` — 32 bytes (AES-256).
//! - `hmac_key` — arbitrary-length key material used as the HMAC-SHA-512
//!   key (the legacy `sym_key_ver1` layout uses 64 bytes; see
//!   `pcrypto_sec_encdec_create`, `pcrypto.c:460`, which stores the tail
//!   of the symmetric-key bundle as `iv` of length `ivlen`).
//! - `plaintext` — `0..=4096` bytes.
//! - `sector_id: u64` — passed by address in C
//!   (`psync_hmac_sha512_update(&ctx, &sectorid, sizeof(sectorid))`,
//!   `pcrypto.c:502`). On all supported platforms this is little-endian.
//! - `rnd` — 16 random bytes from `pssl_rand_strong` (`pcrypto.c:499`).
//!
//! ## Encoding (`pcrypto_encode_sec`, `pcrypto.c:487`)
//!
//! 1. `hmac = HMAC-SHA-512(hmac_key, plaintext || le64(sector_id) || rnd)`
//!    (`pcrypto.c:500–504`).
//! 2. Two modes based on plaintext length:
//!    - **Short (< 16 bytes)** (`pcrypto.c:505–513`):
//!      `out = rnd[0..datalen]` (ciphertext length == plaintext length,
//!      so for short data the ciphertext is just a truncated prefix of
//!      `rnd`; the bulk of the secrecy lives in the 32-byte auth tag).
//!      `tag_plain = (rnd XOR plaintext-zero-extended-to-16) || hmac[0..16]`
//!      encrypted as 2 consecutive AES-256-ECB blocks → 32-byte auth tag.
//!    - **≥ 16 bytes** (`pcrypto.c:514–559`):
//!      CBC with ciphertext stealing (pclsync variant) using
//!      `iv = hmac[0..16]`. For `datalen % 16 == 0` this is plain CBC.
//!      The auth tag is built as
//!      `tag_plain = rnd[0..8] || hmac[0..16] || rnd[8..16]`
//!      then encrypted as 2 consecutive AES-256-ECB blocks
//!      (`pcrypto.c:519–525`).
//!
//! ## Decoding (`pcrypto_decode_sec`, `pcrypto.c:562`)
//!
//! 1. AES-256-ECB-decrypt the 32-byte auth tag → recover
//!    `(rnd^pt_padded, hmac[0..16])` for short plaintexts or
//!    `(rnd[0..8], hmac[0..16], rnd[8..16])` for long plaintexts
//!    (`pcrypto.c:576–577`).
//! 2. Recover plaintext:
//!    - Short: `pt = (rnd^pt_padded) XOR rnd_truncated`
//!      (`pcrypto.c:580–584`).
//!    - Long: CBC-CS decrypt with `iv = hmac[0..16]`
//!      (`pcrypto.c:585–634`).
//! 3. Recompute `HMAC-SHA-512(hmac_key, pt || le64(sid) || rnd16)`
//!    (`pcrypto.c:635–639`).
//! 4. Constant-time compare the first 16 bytes of the recomputed digest
//!    against the recovered `hmac[0..16]` slot
//!    (`memcmp_const`, `pcrypto.c:640`).
//!
//! # Short-plaintext handling
//!
//! For `0 ≤ datalen < 16`, the C code does NOT pad with zeros and does
//! NOT reject the input. Instead, `out[0..datalen] = rnd[0..datalen]`
//! and the auth blob absorbs `rnd XOR pt` in its first block. This is
//! faithfully reproduced here (`pcrypto.c:505–513`).
//!
//! # Security
//!
//! - [`open_sector`] recomputes the HMAC tag and compares in constant
//!   time via `subtle::ConstantTimeEq`.
//! - Any mismatch returns [`SectorError::AuthFailed`]; the intermediate
//!   plaintext buffer is held in a `Zeroizing<Vec<u8>>` and is zeroed on
//!   the error return path.
//! - `tweak`, `tag_plain`, `hmac16`, `rnd_recovered`, and the HMAC
//!   digest scratch buffers are all `Zeroizing`.
//! - `forbid(unsafe_code)` is inherited from the crate root.

#![allow(clippy::module_name_repetitions)]
#![allow(clippy::needless_range_loop)] // indexed loops mirror the C source for reviewability.

use aes::Aes256;
use cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use hmac::{Hmac, Mac};
use sha2::Sha512;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

type HmacSha512 = Hmac<Sha512>;

/// Maximum pclsync sector payload (bytes). The legacy C layer
/// (`pfscrypto.c`) never calls `pcrypto_encode_sec` with more; we
/// re-enforce here.
pub const PCLSYNC_SECTOR_SIZE: usize = 4096;

/// Detached auth tag size — `psync_crypto_auth_sector_t` element
/// (`pfscrypto.h:54`).
pub const PCLSYNC_AUTH_TAG_SIZE: usize = 32;

/// Per-sector random field (`rnd` in C, 16 bytes = `PSYNC_AES256_BLOCK_SIZE`).
pub const PCLSYNC_RND_SIZE: usize = 16;

const BLOCK: usize = 16;

/// Keys for a single sector operation.
///
/// - `aes_key` must be exactly 32 bytes.
/// - `hmac_key` may be any length. The `sym_key_ver1` bundle stores
///   64 bytes (`pcrypto.c:464–465`, where `ivlen = keylen - 32`). The
///   Rust signature accepts `&[u8]` to match this variable-length
///   reality. The upstream task spec stub typed this as `&[u8; 128]`;
///   that was incorrect — see `pcrypto_sec_encdec_create`.
pub struct SectorKeys<'a> {
    /// AES-256 key.
    pub aes_key: &'a [u8; 32],
    /// HMAC-SHA-512 key (typically 64 bytes for `sym_key_ver1`).
    pub hmac_key: &'a [u8],
}

/// Output of [`seal_sector`]: ciphertext (same length as plaintext) and
/// the 32-byte detached auth tag.
///
/// The auth tag is stored **separately** from the sector payload in the
/// legacy wire format — it goes into the Merkle hash tree
/// (`psync_crypto_auth_sector_t` in `pfscrypto.h:54`), not inline with
/// the ciphertext. Primitive E will consume [`SealedSector::auth_tag`].
#[derive(Debug, Clone)]
pub struct SealedSector {
    /// Ciphertext. Same length as plaintext.
    pub ciphertext: Vec<u8>,
    /// 32-byte detached auth tag.
    pub auth_tag: [u8; PCLSYNC_AUTH_TAG_SIZE],
}

/// Errors surfaced by the sector AEAD.
#[derive(Debug, thiserror::Error)]
pub enum SectorError {
    /// Plaintext exceeds [`PCLSYNC_SECTOR_SIZE`].
    #[error("plaintext length {0} exceeds max sector size 4096")]
    PlaintextTooLong(usize),
    /// Ciphertext exceeds [`PCLSYNC_SECTOR_SIZE`].
    #[error("ciphertext length {0} exceeds max sector size 4096")]
    CiphertextTooLong(usize),
    /// Auth tag verification failed.
    #[error("sector authentication failed")]
    AuthFailed,
    /// OS randomness source failed while generating the sector nonce.
    #[error("failed to generate sector nonce: {0}")]
    Rng(String),
    /// Plaintext is empty. The sector encoder requires at least one byte of
    /// plaintext — an empty sector is a caller bug (file-system layer should
    /// never issue a zero-byte sector seal). Rejecting explicitly rather than
    /// silently producing an all-rnd ciphertext makes the contract auditable.
    #[error("sector plaintext must not be empty")]
    EmptySector,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// HMAC-SHA-512 over a sequence of byte slices. Mirrors
/// `psync_hmac_sha512_init/_update/_final` (`pcrypto.c:500–504`).
fn hmac_sha512(key: &[u8], parts: &[&[u8]]) -> Zeroizing<[u8; 64]> {
    // SAFETY: HMAC-SHA-512 accepts any non-zero key length (RFC 2104).
    // All callers pass a fixed-length HMAC key slice derived from the
    // pclsync key schedule, never zero-length.
    let mut mac = <HmacSha512 as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    for p in parts {
        mac.update(p);
    }
    let out = mac.finalize().into_bytes();
    let mut buf = Zeroizing::new([0u8; 64]);
    buf.copy_from_slice(&out);
    buf
}

/// Encrypt 32 bytes as two consecutive AES-256-ECB blocks in place.
/// Mirrors `psync_aes256_encode_2blocks_consec` (C helper).
fn ecb_encrypt_two_blocks(cipher: &Aes256, blocks: &mut [u8; 32]) {
    let (b0, b1) = blocks.split_at_mut(BLOCK);
    cipher.encrypt_block(aes::Block::from_mut_slice(b0));
    cipher.encrypt_block(aes::Block::from_mut_slice(b1));
}

/// Inverse of [`ecb_encrypt_two_blocks`].
fn ecb_decrypt_two_blocks(cipher: &Aes256, blocks: &mut [u8; 32]) {
    let (b0, b1) = blocks.split_at_mut(BLOCK);
    cipher.decrypt_block(aes::Block::from_mut_slice(b0));
    cipher.decrypt_block(aes::Block::from_mut_slice(b1));
}

fn xor_into(dst: &mut [u8], src: &[u8]) {
    debug_assert!(dst.len() >= src.len());
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d ^= *s;
    }
}

// ---------------------------------------------------------------------------
// CBC with ciphertext stealing — pclsync variant
// ---------------------------------------------------------------------------
//
// Matches the long-path branch of `pcrypto_encode_sec`
// (`pcrypto.c:514–559`) and the long-path branch of
// `pcrypto_decode_sec` (`pcrypto.c:585–634`).
//
// - `len % 16 == 0` → plain CBC.
// - `len % 16 != 0` and `len > 16` → CBC-CS: the last two ciphertext
//   blocks swap their tails so the output length stays == input length.

fn cbc_cs_encrypt(cipher: &Aes256, iv: &[u8; 16], plaintext: &[u8], out: &mut [u8]) {
    let mut len = plaintext.len();
    let needsteal = if len % BLOCK != 0 {
        let r = len % BLOCK;
        len -= r + BLOCK;
        r
    } else {
        0
    };

    // Main CBC loop (pcrypto.c:527–550).
    let mut prev = *iv;
    let mut p_off = 0usize;
    let mut o_off = 0usize;
    let mut remaining = len;
    while remaining > 0 {
        let mut block = [0u8; BLOCK];
        block.copy_from_slice(&plaintext[p_off..p_off + BLOCK]);
        xor_into(&mut block, &prev);
        cipher.encrypt_block(aes::Block::from_mut_slice(&mut block));
        out[o_off..o_off + BLOCK].copy_from_slice(&block);
        prev = block;
        p_off += BLOCK;
        o_off += BLOCK;
        remaining -= BLOCK;
    }

    if needsteal != 0 {
        // pcrypto.c:551–559.
        //   aessrc currently = last CT block (= prev).
        //   data[p_off..p_off+16]                    = penultimate PT (full block).
        //   data[p_off+16..p_off+16+needsteal]       = tail PT.
        //
        //   xor16_unaligned_inplace(aessrc, data);           aessrc ^= pen_pt
        //   psync_aes256_encode_block(aessrc, aesdst);       aesdst = E(aessrc)
        //   memcpy(out + 16, aesdst, needsteal);             tail CT = aesdst[..ns]
        //   xor_cnt_inplace(aesdst, data, needsteal);        aesdst[..ns] ^= tail_pt
        //   psync_aes256_encode_block(aesdst, aessrc);       aessrc = E(aesdst)
        //   copy_unaligned(out, aessrc);                     penultimate CT = aessrc
        let pen_pt = &plaintext[p_off..p_off + BLOCK];
        let tail_pt = &plaintext[p_off + BLOCK..p_off + BLOCK + needsteal];

        let mut aessrc = prev;
        xor_into(&mut aessrc, pen_pt);
        let mut aesdst = aessrc;
        cipher.encrypt_block(aes::Block::from_mut_slice(&mut aesdst));
        out[o_off + BLOCK..o_off + BLOCK + needsteal].copy_from_slice(&aesdst[..needsteal]);
        xor_into(&mut aesdst[..needsteal], tail_pt);
        let mut final_ct = aesdst;
        cipher.encrypt_block(aes::Block::from_mut_slice(&mut final_ct));
        out[o_off..o_off + BLOCK].copy_from_slice(&final_ct);
    }
}

fn cbc_cs_decrypt(cipher: &Aes256, iv: &[u8; 16], ciphertext: &[u8], out: &mut [u8]) {
    let mut len = ciphertext.len();
    let needsteal = if len % BLOCK != 0 {
        let r = len % BLOCK;
        len -= r + BLOCK;
        r
    } else {
        0
    };

    // Main CBC-decrypt loop (pcrypto.c:596–620). The 4-block vectorized
    // variant in C is a performance optimization only; the 1-block loop
    // at C:609–620 is semantically identical and easier to audit here.
    let mut prev = *iv;
    let mut c_off = 0usize;
    let mut o_off = 0usize;
    let mut remaining = len;
    while remaining > 0 {
        let mut block = [0u8; BLOCK];
        block.copy_from_slice(&ciphertext[c_off..c_off + BLOCK]);
        let ct_saved = block;
        cipher.decrypt_block(aes::Block::from_mut_slice(&mut block));
        xor_into(&mut block, &prev);
        out[o_off..o_off + BLOCK].copy_from_slice(&block);
        prev = ct_saved;
        c_off += BLOCK;
        o_off += BLOCK;
        remaining -= BLOCK;
    }

    if needsteal != 0 {
        // pcrypto.c:621–633.
        //   aessrc = penultimate CT block
        //   aesdst = D(aessrc)
        //   xor_cnt_inplace(aesdst, data+16, needsteal)    aesdst[..ns] ^= tail_ct
        //   out[+16..+16+ns] = aesdst[..ns]                tail plaintext
        //   aesdst[..ns]     = tail_ct                     rebuild block for 2nd decrypt
        //   aessrc = D(aesdst)
        //   aessrc ^= prev                                 prev is last full CT block
        //   out[..16] = aessrc                             penultimate plaintext
        let tail_ct = &ciphertext[c_off + BLOCK..c_off + BLOCK + needsteal];

        let mut aessrc = [0u8; BLOCK];
        aessrc.copy_from_slice(&ciphertext[c_off..c_off + BLOCK]);
        let mut aesdst = aessrc;
        cipher.decrypt_block(aes::Block::from_mut_slice(&mut aesdst));
        xor_into(&mut aesdst[..needsteal], tail_ct);
        out[o_off + BLOCK..o_off + BLOCK + needsteal].copy_from_slice(&aesdst[..needsteal]);
        aesdst[..needsteal].copy_from_slice(tail_ct);
        let mut again = aesdst;
        cipher.decrypt_block(aes::Block::from_mut_slice(&mut again));
        xor_into(&mut again, &prev);
        out[o_off..o_off + BLOCK].copy_from_slice(&again);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Encrypt one sector and produce a 32-byte detached auth tag.
///
/// The 16-byte `rnd` nonce is drawn from [`rand_core::OsRng`]. For
/// deterministic / KAT-driven flows use [`seal_sector_with_rnd`].
pub fn seal_sector(
    keys: SectorKeys<'_>,
    sector_id: u64,
    plaintext: &[u8],
) -> Result<SealedSector, SectorError> {
    if plaintext.is_empty() {
        return Err(SectorError::EmptySector);
    }
    if plaintext.len() > PCLSYNC_SECTOR_SIZE {
        return Err(SectorError::PlaintextTooLong(plaintext.len()));
    }

    let mut rnd = Zeroizing::new([0u8; PCLSYNC_RND_SIZE]);
    {
        use rand_core::RngCore;
        rand_core::OsRng
            .try_fill_bytes(rnd.as_mut())
            .map_err(|e| SectorError::Rng(e.to_string()))?;
    }

    seal_sector_with_rnd(keys, sector_id, plaintext, &rnd)
}

/// Deterministic [`seal_sector`] variant: caller supplies the 16-byte
/// `rnd` field. Used by the KAT harness and by any higher layer that
/// derives `rnd` from an external nonce source.
#[doc(hidden)]
pub fn seal_sector_with_rnd(
    keys: SectorKeys<'_>,
    sector_id: u64,
    plaintext: &[u8],
    rnd: &[u8; PCLSYNC_RND_SIZE],
) -> Result<SealedSector, SectorError> {
    if plaintext.is_empty() {
        return Err(SectorError::EmptySector);
    }
    if plaintext.len() > PCLSYNC_SECTOR_SIZE {
        return Err(SectorError::PlaintextTooLong(plaintext.len()));
    }

    let sector_id_le = sector_id.to_le_bytes();

    // pcrypto.c:500–504
    let hmac_digest = hmac_sha512(keys.hmac_key, &[plaintext, &sector_id_le, rnd.as_ref()]);

    // SAFETY: `keys.aes_key` is a fixed-length `&[u8; PCLSYNC_AES_KEY_LEN]`
    // (32 bytes), which is the required key size for AES-256.
    let aes = Aes256::new_from_slice(keys.aes_key).expect("AES-256 key is 32 bytes");
    let datalen = plaintext.len();
    let mut ciphertext = vec![0u8; datalen];
    let mut auth_tag = [0u8; PCLSYNC_AUTH_TAG_SIZE];

    if datalen < BLOCK {
        // Short path — pcrypto.c:505–513.
        //   tag_plain[0..16]  = rnd; tag_plain[0..dl] ^= data
        //   tag_plain[16..32] = hmac[0..16]
        //   out[0..dl]        = rnd[0..dl]
        //   authout           = ECB2(tag_plain)
        let mut tag_plain = Zeroizing::new([0u8; 32]);
        tag_plain[..BLOCK].copy_from_slice(rnd.as_ref());
        xor_into(&mut tag_plain[..datalen], plaintext);
        tag_plain[BLOCK..BLOCK + BLOCK].copy_from_slice(&hmac_digest[..BLOCK]);

        if datalen > 0 {
            ciphertext.copy_from_slice(&rnd[..datalen]);
        }
        let mut tag_bytes = *tag_plain;
        ecb_encrypt_two_blocks(&aes, &mut tag_bytes);
        auth_tag.copy_from_slice(&tag_bytes);
    } else {
        // Long path — pcrypto.c:514–559.
        //   tag_plain = rnd[0..8] || hmac[0..16] || rnd[8..16]  (C:519–523)
        //   authout   = ECB2(tag_plain)                         (C:524–525)
        //   CBC-CS encrypt plaintext with iv = hmac[0..16]      (C:526+)
        let mut tag_plain = Zeroizing::new([0u8; 32]);
        tag_plain[..BLOCK / 2].copy_from_slice(&rnd[..BLOCK / 2]);
        tag_plain[BLOCK / 2..BLOCK / 2 + BLOCK].copy_from_slice(&hmac_digest[..BLOCK]);
        tag_plain[BLOCK + BLOCK / 2..].copy_from_slice(&rnd[BLOCK / 2..]);

        let mut tag_bytes = *tag_plain;
        ecb_encrypt_two_blocks(&aes, &mut tag_bytes);
        auth_tag.copy_from_slice(&tag_bytes);

        let mut iv = [0u8; BLOCK];
        iv.copy_from_slice(&hmac_digest[..BLOCK]);
        cbc_cs_encrypt(&aes, &iv, plaintext, &mut ciphertext);
    }

    Ok(SealedSector {
        ciphertext,
        auth_tag,
    })
}

/// Decrypt one sector and verify the 32-byte detached auth tag.
///
/// Returns [`SectorError::AuthFailed`] on any mismatch. The plaintext
/// scratch buffer is held in a `Zeroizing<Vec<u8>>` and is zeroed on
/// the error return path.
pub fn open_sector(
    keys: SectorKeys<'_>,
    sector_id: u64,
    ciphertext: &[u8],
    auth_tag: &[u8; PCLSYNC_AUTH_TAG_SIZE],
) -> Result<Zeroizing<Vec<u8>>, SectorError> {
    // audit-06 P3 (pcloud-rs-ncx.34): explicitly reject a zero-length
    // sector. `pcrypto_decode_sec` in C (`pcrypto.c:562`) does not
    // produce nor accept an empty plaintext/ciphertext pair: the encoder
    // never emits one, so any open_sector() call with empty ciphertext
    // is a caller-contract bug. Surfacing the error explicitly (rather
    // than silently succeeding on `plaintext = []`) makes the wire
    // contract auditable and mirrors the encode-side guard above.
    if ciphertext.is_empty() {
        return Err(SectorError::EmptySector);
    }
    if ciphertext.len() > PCLSYNC_SECTOR_SIZE {
        return Err(SectorError::CiphertextTooLong(ciphertext.len()));
    }

    // SAFETY: `keys.aes_key` is a fixed-length `&[u8; PCLSYNC_AES_KEY_LEN]`
    // (32 bytes), the required AES-256 key size.
    let aes = Aes256::new_from_slice(keys.aes_key).expect("AES-256 key is 32 bytes");

    // pcrypto.c:576–577 — decrypt the auth tag in place.
    let mut tag_plain = Zeroizing::new([0u8; 32]);
    tag_plain.copy_from_slice(auth_tag);
    ecb_decrypt_two_blocks(&aes, &mut tag_plain);

    let datalen = ciphertext.len();
    let mut plaintext = Zeroizing::new(vec![0u8; datalen]);
    let mut hmac16 = Zeroizing::new([0u8; BLOCK]);
    let mut rnd_recovered = Zeroizing::new([0u8; PCLSYNC_RND_SIZE]);

    if datalen < BLOCK {
        // Short path — pcrypto.c:580–584.
        //
        // Post-decrypt tag_plain layout (inverse of encode C:506–509):
        //   tag_plain[0..16]  = rnd XOR pt_zero_extended_to_16
        //   tag_plain[16..32] = hmac_digest[0..16]
        //
        // Plaintext recovery: pt[i] = tag_plain[i] XOR ct[i]   (since ct = rnd[0..dl])
        // Rnd recovery:       rnd[0..dl]  = ct
        //                     rnd[dl..16] = tag_plain[dl..16]  (untouched by encode xor)
        let (rnd_xored_pt, hmac_digest_slot) = tag_plain.split_at(BLOCK);

        for i in 0..datalen {
            plaintext[i] = rnd_xored_pt[i] ^ ciphertext[i];
        }
        rnd_recovered[..datalen].copy_from_slice(ciphertext);
        rnd_recovered[datalen..].copy_from_slice(&rnd_xored_pt[datalen..]);
        hmac16.copy_from_slice(hmac_digest_slot);
    } else {
        // Long path — pcrypto.c:585+.
        //
        // Post-decrypt tag_plain layout (inverse of encode C:519–523):
        //   tag_plain[0..8]   = rnd[0..8]
        //   tag_plain[8..24]  = hmac_digest[0..16]   (== CBC iv)
        //   tag_plain[24..32] = rnd[8..16]
        let mut iv = [0u8; BLOCK];
        iv.copy_from_slice(&tag_plain[BLOCK / 2..BLOCK / 2 + BLOCK]);
        hmac16.copy_from_slice(&iv);

        rnd_recovered[..BLOCK / 2].copy_from_slice(&tag_plain[..BLOCK / 2]);
        rnd_recovered[BLOCK / 2..].copy_from_slice(&tag_plain[BLOCK + BLOCK / 2..]);

        cbc_cs_decrypt(&aes, &iv, ciphertext, &mut plaintext);
    }

    // pcrypto.c:635–639 — recompute HMAC over recovered plaintext + sid + rnd.
    let sector_id_le = sector_id.to_le_bytes();
    let recomputed = hmac_sha512(
        keys.hmac_key,
        &[plaintext.as_slice(), &sector_id_le, rnd_recovered.as_ref()],
    );

    // pcrypto.c:640 — constant-time compare of the first 16 bytes.
    if recomputed[..BLOCK].ct_eq(hmac16.as_ref()).unwrap_u8() != 1 {
        // `plaintext` is Zeroizing → zeroed on drop via the ? / return path.
        return Err(SectorError::AuthFailed);
    }

    // audit-06 P1 (pcloud-rs-ncx.31): return the `Zeroizing<Vec<u8>>`
    // directly so the caller's plaintext also zeroes on drop. Callers
    // that need a plain `Vec<u8>` (e.g. FUSE write-path) deliberately
    // opt out of zeroization via `(*pt).clone()` or explicit copy.
    Ok(plaintext)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_fixture() -> ([u8; 32], [u8; 64]) {
        let mut aes = [0u8; 32];
        let mut hmac = [0u8; 64];
        for (i, b) in aes.iter_mut().enumerate() {
            *b = i as u8;
        }
        for (i, b) in hmac.iter_mut().enumerate() {
            *b = 0x80u8.wrapping_add(i as u8);
        }
        (aes, hmac)
    }

    fn roundtrip(pt: &[u8], sector_id: u64) {
        let (aes, hmac) = keys_fixture();
        let sealed = seal_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            sector_id,
            pt,
        )
        .expect("seal");
        assert_eq!(sealed.ciphertext.len(), pt.len());
        let opened = open_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            sector_id,
            &sealed.ciphertext,
            &sealed.auth_tag,
        )
        .expect("open");
        assert_eq!(opened.as_slice(), pt);
    }

    #[test]
    fn seal_open_roundtrip_empty() {
        // Empty plaintext is now explicitly rejected (M-3.6 / audit-05).
        // The C code accepted it silently (producing an all-rnd ciphertext
        // that was technically undecryptable to the original plaintext for
        // 0-byte files), but the Rust path rejects it explicitly to make
        // the caller contract auditable.
        let (aes, hmac) = keys_fixture();
        let err = seal_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            0,
            &[],
        )
        .unwrap_err();
        assert!(
            matches!(err, SectorError::EmptySector),
            "expected EmptySector, got {err:?}"
        );
    }

    #[test]
    fn seal_open_roundtrip_1_byte() {
        roundtrip(&[0xAB], 1);
    }

    #[test]
    fn seal_open_roundtrip_15_bytes() {
        let pt: Vec<u8> = (0..15).collect();
        roundtrip(&pt, 42);
    }

    #[test]
    fn seal_open_roundtrip_16_bytes() {
        let pt: Vec<u8> = (0..16).collect();
        roundtrip(&pt, 42);
    }

    #[test]
    fn seal_open_roundtrip_4096_bytes() {
        let pt: Vec<u8> = (0..4096).map(|i| (i * 7 + 3) as u8).collect();
        roundtrip(&pt, 0xDEAD_BEEF);
    }

    #[test]
    fn seal_open_roundtrip_various_sizes() {
        for len in (1..=4096).step_by(500) {
            let pt: Vec<u8> = (0..len).map(|i| (i ^ 0x55) as u8).collect();
            roundtrip(&pt, len as u64);
        }
        // Extra coverage around block boundaries (where CBC-CS kicks in).
        for &len in &[1usize, 15, 16, 17, 31, 32, 33, 63, 64, 65, 4095, 4096] {
            let pt: Vec<u8> = (0..len).map(|i| i as u8).collect();
            roundtrip(&pt, 0x0123_4567_89AB_CDEF);
        }
    }

    #[test]
    fn open_rejects_tampered_ciphertext() {
        let (aes, hmac) = keys_fixture();
        let pt: Vec<u8> = (0..200).collect();
        let mut sealed = seal_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            7,
            &pt,
        )
        .unwrap();
        sealed.ciphertext[100] ^= 0x01;
        let err = open_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            7,
            &sealed.ciphertext,
            &sealed.auth_tag,
        )
        .unwrap_err();
        assert!(matches!(err, SectorError::AuthFailed));
    }

    #[test]
    fn open_rejects_tampered_auth_tag() {
        let (aes, hmac) = keys_fixture();
        let pt: Vec<u8> = (0..200).collect();
        let mut sealed = seal_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            7,
            &pt,
        )
        .unwrap();
        sealed.auth_tag[0] ^= 0x80;
        let err = open_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            7,
            &sealed.ciphertext,
            &sealed.auth_tag,
        )
        .unwrap_err();
        assert!(matches!(err, SectorError::AuthFailed));
    }

    #[test]
    fn open_rejects_wrong_sector_id() {
        let (aes, hmac) = keys_fixture();
        let pt: Vec<u8> = (0..200).collect();
        let sealed = seal_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            1,
            &pt,
        )
        .unwrap();
        let err = open_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            2,
            &sealed.ciphertext,
            &sealed.auth_tag,
        )
        .unwrap_err();
        assert!(matches!(err, SectorError::AuthFailed));
    }

    #[test]
    fn seal_is_randomized() {
        let (aes, hmac) = keys_fixture();
        let pt = vec![0x42u8; 200];
        let a = seal_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            3,
            &pt,
        )
        .unwrap();
        let b = seal_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            3,
            &pt,
        )
        .unwrap();
        // Different rnd → different ciphertext AND different auth tag.
        assert_ne!(a.ciphertext, b.ciphertext);
        assert_ne!(a.auth_tag, b.auth_tag);
    }

    #[test]
    fn deterministic_rnd_is_stable() {
        // Self-check: same rnd → same ciphertext and tag. This is the
        // hook a future C-generated KAT will bolt onto.
        let (aes, hmac) = keys_fixture();
        let rnd = [0u8; PCLSYNC_RND_SIZE];
        let pt: Vec<u8> = (0..64).collect();
        let a = seal_sector_with_rnd(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            1,
            &pt,
            &rnd,
        )
        .unwrap();
        let b = seal_sector_with_rnd(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            1,
            &pt,
            &rnd,
        )
        .unwrap();
        assert_eq!(a.ciphertext, b.ciphertext);
        assert_eq!(a.auth_tag, b.auth_tag);
    }

    // NOTE(M-3.1 / bd-1du.10): a byte-exact KAT for pcrypto_encode_sec /
    // pcrypto_decode_sec requires a fixture extracted from a standalone C
    // reference harness (link `pcrypto.c` with fixed aes_key / hmac_key /
    // rnd / sid, capture expected ciphertext + auth_tag hex). No such fixture
    // has been committed yet. The placeholder has been removed — an empty
    // #[ignore] test provides no coverage value and misleads readers about
    // what has been verified. When a fixture is available, add a named test
    // with hard-coded hex vectors, citing the C run that produced them.

    #[test]
    fn seal_rejects_empty_plaintext() {
        let (aes, hmac) = keys_fixture();
        let err = seal_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            0,
            &[],
        )
        .unwrap_err();
        assert!(
            matches!(err, SectorError::EmptySector),
            "expected EmptySector, got {err:?}"
        );
    }

    /// audit-06 P3 (pcloud-rs-ncx.34): decoder must reject zero-length
    /// ciphertext with the same explicit contract as the encoder.
    #[test]
    fn open_rejects_empty_ciphertext() {
        let (aes, hmac) = keys_fixture();
        let tag = [0u8; PCLSYNC_AUTH_TAG_SIZE];
        let err = open_sector(
            SectorKeys {
                aes_key: &aes,
                hmac_key: &hmac,
            },
            0,
            &[],
            &tag,
        )
        .unwrap_err();
        assert!(
            matches!(err, SectorError::EmptySector),
            "expected EmptySector, got {err:?}"
        );
    }
}
