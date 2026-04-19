//! Live known-answer test: prove the pclsync-v2 primitives in this crate
//! byte-decode ciphertext produced by pCloud's official server / clients.
//!
//! Closes bead `pcloud-rs-s1p.13` (audit-04, final parity-proof wave).
//!
//! # Flow (matches the Wave 1 primitives)
//!
//! 1. Read fixtures under `tests/fixtures/pclsync_v2/` (committed).
//! 2. Parse `kat-priv-key-ver1.blob`:
//!    `[u32 type LE][u32 flags LE][u8;64 salt][AES-256-CTR(priv DER)]`
//!    — see `C_CODE/pclsync/pcryptofolder.c:72-77`.
//! 3. Derive KEK = PBKDF2-HMAC-SHA512(password, salt, 20000, 48) →
//!    (AES-256 key, 16-byte IV).
//! 4. Unwrap priv DER in place with the pclsync-native CTR (counter = 0,
//!    cf. `pcryptofolder.c:1845, 1867`).
//! 5. Parse PKCS#1 DER → `RsaPrivateKey`.
//! 6. RSA-OAEP-SHA1 unwrap `kat-file-sym-key-wrapped.bin` → `SymKeyVer1`.
//!    The folder wrapped blob is exactly 512 bytes (RSA-4096 block size).
//!    The file wrapped blob is 504 bytes — 8 bytes short. Most plausible
//!    cause: the API carries the RSA ciphertext as a big-integer whose
//!    leading zero bytes were stripped by pCloud's custom base64 /
//!    hex path. We try recovering a 512-byte block by left-padding with
//!    zeros first; if that fails, we fall back to right-pad. The
//!    discovered shape is asserted in the test output so a later reader
//!    sees which layout actually works end-to-end.
//! 7. Open sector 0 (4096 B ciphertext + 32 B detached auth tag) via
//!    `pclsync_sector::open_sector`. `sector_id = 0` = the raw sector
//!    index; the file hash is NOT part of the sector HMAC input
//!    (cf. `pcrypto.c:500-504` where HMAC input is
//!    `plaintext || le64(sectorid) || rnd16`; the file hash is used by
//!    the hash-tree auth in `pfscrypto.c:237-248`, not here).
//! 8. Compare SHA-256 + byte-wise against the plaintext fixture.

#![cfg(feature = "pclsync-v2")]
#![forbid(unsafe_code)]

use std::path::PathBuf;

use pcloud_crypto::pclsync_compat_profile::PclsyncCompatProfile;
use pcloud_crypto::{pclsync_kdf, pclsync_modes, pclsync_rsa, pclsync_sector};
use pcloud_secret::secret_string::SecretString;
use sha2::{Digest, Sha256};

/// Minimal hex encoder (avoids taking a `hex` dep).
fn hex_encode(b: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        s.push(TABLE[(byte >> 4) as usize] as char);
        s.push(TABLE[(byte & 0x0f) as usize] as char);
    }
    s
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("pclsync_v2")
}

fn read_fixture(name: &str) -> Vec<u8> {
    let path = fixtures_dir().join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {path:?}: {e}"))
}

/// Attempt to normalize a server-returned RSA-wrapped blob into the exact
/// 512-byte RSA-4096 ciphertext block that `oaep_unwrap` expects.
///
/// pCloud's `crypto_getfilekey` has been observed to return 504-byte blobs
/// (8 bytes shy of a full 512-byte block). The most plausible explanation
/// is that the ciphertext is serialized as a big-integer and leading zero
/// bytes are stripped by the server's encoder. Left-padding with zeros
/// reconstructs the original modulus-sized block because (n*2^(8*k)) for
/// k>0 would shift the entire payload, but leading-zero stripping does
/// not alter the mathematical value — OAEP decrypt on the zero-extended
/// block yields the same plaintext iff the original was leading-zero-stripped.
///
/// Returns `(layout_description, 512-byte-block)`.
fn normalize_candidates(raw: &[u8]) -> Vec<(String, Vec<u8>)> {
    const RSA_BLOCK: usize = 512;
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    if raw.len() == RSA_BLOCK {
        out.push(("exact 512 bytes (raw RSA-4096 block)".into(), raw.to_vec()));
    }
    if raw.len() < RSA_BLOCK {
        // (a) Left-pad with zeros: standard big-integer leading-zero recovery.
        let pad = RSA_BLOCK - raw.len();
        let mut lp = vec![0u8; RSA_BLOCK];
        lp[pad..].copy_from_slice(raw);
        out.push(("left-pad zeros to 512 (big-int leading-zero recovery)".into(), lp));
        // (b) Right-pad with zeros to 512 (unlikely but cheap to try).
        let mut rp = vec![0u8; RSA_BLOCK];
        rp[..raw.len()].copy_from_slice(raw);
        out.push(("right-pad zeros to 512".into(), rp));
    }
    if raw.len() > RSA_BLOCK {
        // Oversized: 8-byte `[u32 type][u32 flags]` header prefixed.
        if raw.len() == RSA_BLOCK + 8 {
            out.push((
                "stripped 8-byte [u32 type LE][u32 flags LE] prefix".into(),
                raw[8..].to_vec(),
            ));
        }
        // Generic: strip any prefix to reach exactly 512.
        let strip = raw.len() - RSA_BLOCK;
        if strip <= 16 && strip > 0 && strip != 8 {
            out.push((
                format!("stripped {strip}-byte prefix to reach 512"),
                raw[strip..].to_vec(),
            ));
        }
    }
    // Headered-504 case: if the blob is exactly 504 and the first 8 bytes
    // look like `[u32 type LE][u32 flags LE]` followed by a 496-byte tail,
    // it's not directly OAEP-decryptable (not 512). But for the record,
    // test it with left-pad on the 496 tail too.
    if raw.len() == 504 {
        let pad = RSA_BLOCK - 496;
        let mut buf = vec![0u8; RSA_BLOCK];
        buf[pad..].copy_from_slice(&raw[8..]);
        out.push((
            "strip 8-byte header from 504-byte blob + left-pad remaining 496 to 512".into(),
            buf,
        ));
    }
    out
}

#[test]
#[ignore = "live KAT: requires $PCLOUD_KAT_PASSWORD + extracted fixtures"]
fn pclsync_compat_decrypts_official_pcloud_ciphertext() {
    // Double-gate on env var so that even under `--ignored` in a CI where
    // the password is not provisioned, the test is a clean skip rather
    // than a hard failure.
    let pw = match std::env::var("PCLOUD_KAT_PASSWORD") {
        Ok(p) if !p.is_empty() => p,
        _ => {
            eprintln!(
                "\n[pclsync-compat-kat] SKIP: PCLOUD_KAT_PASSWORD unset.\n\
                 Run with:\n  \
                 set -a; source .env; set +a\n  \
                 export PCLOUD_KAT_PASSWORD=\"$PCLOUD_PASSWORD\"\n  \
                 cargo test -p pcloud-crypto --test pclsync_compat_kat_live \\\n    \
                 -- --ignored --nocapture\n"
            );
            return;
        }
    };

    // -------------------------------------------------------------------
    // 1. Load fixtures.
    // -------------------------------------------------------------------
    let plaintext_fixture = read_fixture("kat-plaintext-v1.bin");
    assert_eq!(
        plaintext_fixture.len(),
        4096,
        "plaintext fixture must be exactly one sector"
    );
    let expected_sha = Sha256::digest(&plaintext_fixture);
    eprintln!(
        "[pclsync-compat-kat] plaintext sha256 = {}",
        hex_encode(&expected_sha)
    );

    let priv_blob = read_fixture("kat-priv-key-ver1.blob");
    let file_wrapped = read_fixture("kat-file-sym-key-wrapped.bin");
    let ciphertext_full = read_fixture("kat-ciphertext-v1.bin");
    assert_eq!(
        ciphertext_full.len(),
        4096 + 32,
        "ciphertext fixture must be 4096 B ct + 32 B detached auth tag"
    );

    // -------------------------------------------------------------------
    // 2. Parse priv_key_ver1 header: [type u32 LE][flags u32 LE][salt 64][ct DER].
    //    Cited: C_CODE/pclsync/pcryptofolder.c:72-77.
    // -------------------------------------------------------------------
    let (_typ, _flags, salt, mut ct_der) = PclsyncCompatProfile::parse_priv_blob(&priv_blob)
        .expect("parse priv_key_ver1 blob");
    eprintln!(
        "[pclsync-compat-kat] priv blob: {} B, ct DER = {} B, salt[0..4]={:02x?}",
        priv_blob.len(),
        ct_der.len(),
        &salt[..4]
    );

    // -------------------------------------------------------------------
    // 3. Derive KEK from password + salt (PBKDF2-HMAC-SHA512, 20000 iters, 48 B out).
    //    Cited: C_CODE/pclsync/pcryptofolder.c:383-385, psettings.h:168.
    // -------------------------------------------------------------------
    let password = SecretString::new(pw);
    let kek = pclsync_kdf::derive_kek(&password, &salt);

    // -------------------------------------------------------------------
    // 4. In-place AES-256-CTR (pclsync variant, counter starts at 0).
    //    Cited: C_CODE/pclsync/pcryptofolder.c:1845, 1867, 1954.
    // -------------------------------------------------------------------
    pclsync_modes::aes256_ctr_pclsync_xor_inplace(&kek.key, &kek.iv, 0, &mut ct_der);

    // -------------------------------------------------------------------
    // 5. Parse PKCS#1 DER into an RsaPrivateKey.
    // -------------------------------------------------------------------
    let priv_key = pclsync_rsa::parse_priv_key_der(&ct_der).unwrap_or_else(|e| {
        panic!(
            "parse_priv_key_der failed ({e:?}) — wrong password, wrong KEK, \
             or priv_key_ver1 layout drifted. First 16 unwrapped bytes: {:02x?}",
            &ct_der[..16.min(ct_der.len())]
        )
    });
    eprintln!("[pclsync-compat-kat] priv key unwrapped + DER-parsed OK");

    // Cross-check: the folder wrapped blob is exact 512 bytes, which is the
    // raw RSA-4096 block. If this decrypts, the priv key is correct and any
    // failure on the file blob isolates to the file-key encoding layout.
    let folder_wrapped = read_fixture("kat-folder-sym-key-wrapped.bin");
    eprintln!(
        "[pclsync-compat-kat] folder wrapped key: {} bytes (sanity OAEP unwrap ...)",
        folder_wrapped.len()
    );
    match pclsync_rsa::oaep_unwrap(&priv_key, &folder_wrapped) {
        Ok(fsym) => eprintln!(
            "[pclsync-compat-kat]   folder sym_key_ver1 OK (type={}, flags={:#x})",
            fsym.sym_type, fsym.flags
        ),
        Err(e) => eprintln!(
            "[pclsync-compat-kat]   WARNING: folder OAEP unwrap also failed: {e:?}"
        ),
    }

    // -------------------------------------------------------------------
    // 6. RSA-OAEP-SHA1 unwrap the wrapped file sym_key_ver1.
    //
    //    The 504-byte file-wrapped blob is 8 bytes short of the RSA-4096
    //    block size (512). We try a cascade:
    //      (a) raw bytes as-is (will fail our length check at 504);
    //      (b) left-pad to 512 (leading-zero big-integer recovery);
    //      (c) 512 bytes with an 8-byte header stripped (oversized only).
    //    The first one that succeeds is what the server actually emits;
    //    we print the discovered layout so a later reader has a record.
    // -------------------------------------------------------------------
    eprintln!(
        "[pclsync-compat-kat] file wrapped key: {} bytes",
        file_wrapped.len()
    );
    eprintln!(
        "[pclsync-compat-kat]   file_wrapped[0..16]  = {:02x?}",
        &file_wrapped[..16]
    );
    eprintln!(
        "[pclsync-compat-kat]   file_wrapped[len-8..] = {:02x?}",
        &file_wrapped[file_wrapped.len() - 8..]
    );

    let sym = {
        let mut last_err: Option<pclsync_rsa::PclsyncRsaError> = None;
        let mut success: Option<(String, pclsync_rsa::SymKeyVer1)> = None;

        let mut candidates: Vec<(String, Vec<u8>)> = Vec::new();
        candidates.push(("as-is (no normalization)".into(), file_wrapped.clone()));
        candidates.extend(normalize_candidates(&file_wrapped));

        for (label, blob) in candidates {
            match pclsync_rsa::oaep_unwrap(&priv_key, &blob) {
                Ok(s) => {
                    success = Some((label, s));
                    break;
                }
                Err(e) => {
                    eprintln!("[pclsync-compat-kat]   attempt {label:?} → {e:?}");
                    last_err = Some(e);
                }
            }
        }

        match success {
            Some((layout, sym)) => {
                eprintln!(
                    "[pclsync-compat-kat] file sym_key_ver1 unwrapped OK via layout: {layout}"
                );
                sym
            }
            None => panic!(
                "RSA-OAEP unwrap of kat-file-sym-key-wrapped.bin ({} bytes) failed under all \
                 tried layouts. Last error: {:?}.\n\n\
                 Diagnostic summary:\n  \
                 * priv_key_ver1 AES-CTR unwrap + DER parse: OK (password + KDF + CTR correct).\n  \
                 * folder wrapped blob (512 B) RSA-OAEP-SHA1 unwrap: OK (priv key is correct).\n  \
                 * file wrapped blob (504 B) RSA-OAEP-SHA1 unwrap: FAIL under all normalizations.\n\n\
                 Most likely root cause: the fixture `kat-file-sym-key-wrapped.bin` is malformed \
                 because `scripts/extract-pclsync-kat.py::decode_maybe_hex_or_base64` matches the \
                 `key` field as hex when the server actually returned it in a different encoding \
                 (or the hex decode drops info the OAEP cipher needs). The folder fixture is \
                 exactly 512 bytes (one RSA-4096 block) which is what `crypto_getfolderkey` + \
                 the same decoder produced on the same run — so the priv key, KDF, and OAEP \
                 primitive are all correct. Fix the extractor (e.g. force base64-only decoding \
                 for `filekey[\"key\"]`, or inspect the raw string before decoding) and re-run \
                 `python3 scripts/extract-pclsync-kat.py`, then rerun this test.",
                file_wrapped.len(),
                last_err
            ),
        }
    };

    // Sanity: sym_type must be PSYNC_CRYPTO_SYM_AES256_1024BIT_HMAC = 0.
    assert_eq!(
        sym.sym_type, 0,
        "sym_key_ver1.type must be PSYNC_CRYPTO_SYM_AES256_1024BIT_HMAC"
    );

    // -------------------------------------------------------------------
    // 7. Split ciphertext fixture and open sector 0.
    //    sector_id == raw sector index == 0 here (cf. pfscrypto.c:248
    //    where pcrypto_decode_sec is called with `se->sectorid` directly).
    //    The HMAC input in pcrypto.c:500-504 is
    //      plaintext || le64(sectorid) || rnd16
    //    — the file hash does NOT enter the sector HMAC (it is used by
    //    the hash-tree auth instead).
    //
    //    The SectorKeys::hmac_key slice passed in is the full 128-byte
    //    sym.hmac_key. pcrypto_sec_encdec_create (pcrypto.c:460) stores
    //    the tail of the symmetric-key bundle as `iv` of length `ivlen`
    //    where `ivlen = keylen - 32` — for a 160-byte bundle the HMAC key
    //    is 128 bytes, matching SymKeyVer1::hmac_key exactly.
    // -------------------------------------------------------------------
    let (ct, tag_bytes) = ciphertext_full.split_at(4096);
    let mut tag = [0u8; 32];
    tag.copy_from_slice(tag_bytes);

    let keys = pclsync_sector::SectorKeys {
        aes_key: &sym.aes_key,
        hmac_key: &sym.hmac_key[..],
    };
    let plaintext = match pclsync_sector::open_sector(keys, 0, ct, &tag) {
        Ok(pt) => pt,
        Err(e) => {
            // Surface enough detail to diagnose which assumption broke.
            panic!(
                "open_sector failed: {e:?}\n\
                 Diagnostic snapshot:\n  \
                 sector_id tried: 0 (raw sector index)\n  \
                 aes_key[0..4]:  {:02x?}\n  \
                 hmac_key_len:   {} bytes (full SymKeyVer1::hmac_key)\n  \
                 ct[0..8]:       {:02x?}\n  \
                 tag[0..8]:      {:02x?}\n\n\
                 If the failure mode is AuthFailed, likely causes:\n  \
                 * SectorKeys::hmac_key should be sym.hmac_key[..64] (first 64 B)\n  \
                 * sector_id semantics differ (unlikely: pfscrypto.c:248 passes raw idx)\n  \
                 * file-hash actually participates in the HMAC tweak (would contradict pcrypto.c:500-504)\n\
                 If it's InputTooShort/Long, the ciphertext split is off.",
                &sym.aes_key[..4],
                sym.hmac_key.len(),
                &ct[..8],
                &tag[..8],
            )
        }
    };

    assert_eq!(plaintext.len(), 4096, "recovered plaintext must be 4096 B");

    // -------------------------------------------------------------------
    // 8. Byte-exact + SHA-256 match vs. the committed plaintext fixture.
    // -------------------------------------------------------------------
    let got_sha = Sha256::digest(plaintext.as_slice());
    assert_eq!(
        got_sha.as_slice(),
        expected_sha.as_slice(),
        "SHA-256 mismatch:\n  expected = {}\n  got      = {}",
        hex_encode(&expected_sha),
        hex_encode(&got_sha),
    );
    assert_eq!(
        plaintext.as_slice(),
        plaintext_fixture.as_slice(),
        "byte-exact plaintext mismatch (SHA match? that would be impossible here)",
    );

    eprintln!(
        "[pclsync-compat-kat] PASS: SHA-256 = {} (4096 B plaintext byte-identical)",
        hex_encode(&got_sha)
    );
}
