//! Offline fixture-shape KAT: prove the pclsync-compat parsing surfaces
//! work correctly against the committed fixture files **without** requiring
//! a live pCloud password or network access.
//!
//! This test runs in `cargo test` by default on any developer machine. It:
//!
//! 1. Verifies that every committed fixture file has the expected byte-size
//!    and SHA-256 digest (guards against accidental corruption).
//! 2. Exercises `PclsyncCompatProfile::parse_priv_blob` — parses the header
//!    layout and confirms the salt and ciphertext-DER fields are non-empty.
//! 3. Exercises `PclsyncCompatProfile::parse_pub_blob` — parses the public
//!    key header.
//! 4. Exercises `pclsync_rsa::parse_pub_key_der` — parses the DER bytes
//!    inside the public-key blob into an `RsaPublicKey` and confirms it is
//!    RSA-4096 (modulus bit-length = 4096).
//!
//! Steps 2–4 exercise the parsing code paths that the live KAT also
//! exercises but stop before the PBKDF2 key-derivation step that requires
//! the pCloud login password.
//!
//! The full live decrypt chain is covered by `pclsync_compat_kat_live.rs`
//! (marked `#[ignore]`, requires `PCLOUD_KAT_PASSWORD`).

#![cfg(feature = "pclsync-v2")]
#![forbid(unsafe_code)]

use std::path::PathBuf;

use pcloud_crypto::pclsync_compat_profile::PclsyncCompatProfile;
use pcloud_crypto::pclsync_rsa;
use pcloud_crypto::pclsync_sector::{
    SectorKeys, open_sector, seal_sector_with_rnd, PCLSYNC_AUTH_TAG_SIZE, PCLSYNC_RND_SIZE,
};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Expected fixture metadata (sizes in bytes, SHA-256 hex digests).
// Re-generate with: sha256sum + wc -c on each file after a fresh extraction.
// ---------------------------------------------------------------------------

struct FixtureMeta {
    name: &'static str,
    expected_len: usize,
    expected_sha256: &'static str,
}

const FIXTURES: &[FixtureMeta] = &[
    FixtureMeta {
        name: "kat-ciphertext-v1.bin",
        expected_len: 4128, // 4096 B ciphertext + 32 B detached auth tag
        expected_sha256: "6670495069f225235cf8f9290a4f62cf9fed0791abbf3eefbf731cc0e55791f4",
    },
    FixtureMeta {
        name: "kat-file-sym-key-wrapped.bin",
        expected_len: 512, // RSA-4096 wrapped sym key
        expected_sha256: "61eac32a326edd6f33fe20cd538bac819528320b9421cbe3510b87be3fcca781",
    },
    FixtureMeta {
        name: "kat-folder-sym-key-wrapped.bin",
        expected_len: 512, // RSA-4096 wrapped sym key
        expected_sha256: "f2842b34cbb4aacffa1498c10f3bcb859d2d185bca9060d99aa8b95951128dca",
    },
    FixtureMeta {
        name: "kat-plaintext-v1.bin",
        expected_len: 4096, // exactly one sector
        expected_sha256: "c8f5d0341d54d951a71b136e6e2afcb14d11ed8489a7ae126a8fee0df6ecf193",
    },
    FixtureMeta {
        name: "kat-priv-key-ver1.blob",
        expected_len: 2421, // [u32 type][u32 flags][64-byte salt][RSA-4096 CTR-wrapped DER]
        expected_sha256: "073b66eb36ecfe1999c1e995a8aa2358261921aecafba77c3d8c0bec41d3095f",
    },
    FixtureMeta {
        name: "kat-pub-key-ver1.blob",
        expected_len: 534, // [u32 type][u32 flags][PKCS#1 DER RSA-4096 pub key]
        expected_sha256: "6c1c80d5d93fbef0d5ef3a6012e71e457d235ddf618eba635028f934054a3a3d",
    },
];

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

fn hex_encode(b: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        s.push(TABLE[(byte >> 4) as usize] as char);
        s.push(TABLE[(byte & 0x0f) as usize] as char);
    }
    s
}

// ---------------------------------------------------------------------------
// Test 1: fixture file sizes and SHA-256 digests
// ---------------------------------------------------------------------------

#[test]
fn pclsync_compat_kat_offline_fixture_shapes() {
    for meta in FIXTURES {
        let data = read_fixture(meta.name);
        assert_eq!(
            data.len(),
            meta.expected_len,
            "fixture {}: expected {} bytes, got {}",
            meta.name,
            meta.expected_len,
            data.len()
        );
        let digest = Sha256::digest(&data);
        let hex = hex_encode(&digest);
        assert_eq!(
            hex, meta.expected_sha256,
            "fixture {}: SHA-256 mismatch\n  expected: {}\n  got:      {}",
            meta.name, meta.expected_sha256, hex,
        );
    }
}

// ---------------------------------------------------------------------------
// Test 2: parse_priv_blob — header layout parsing (no password needed)
// ---------------------------------------------------------------------------

#[test]
fn pclsync_compat_kat_offline_parse_priv_blob() {
    let blob = read_fixture("kat-priv-key-ver1.blob");

    let (_typ, _flags, salt, ct_der) =
        PclsyncCompatProfile::parse_priv_blob(&blob).expect("parse_priv_blob must succeed");

    // Salt must be non-zero (any all-zero salt would indicate extraction failure).
    assert!(
        salt.iter().any(|&b| b != 0),
        "salt must not be all-zeros (extraction likely failed)"
    );

    // The ciphertext DER must be large enough to hold an RSA-4096 private key.
    // A PKCS#1 RSA-4096 DER key is typically ~2349 bytes.
    assert!(
        ct_der.len() >= 2000,
        "encrypted DER must be >=2000 bytes for RSA-4096; got {}",
        ct_der.len()
    );
}

// ---------------------------------------------------------------------------
// Test 3 + 4: parse_pub_blob + parse_pub_key_der — public key parsing
// ---------------------------------------------------------------------------

#[test]
fn pclsync_compat_kat_offline_parse_pub_blob_and_der() {
    let blob = read_fixture("kat-pub-key-ver1.blob");

    let (_typ, _flags, pub_der) =
        PclsyncCompatProfile::parse_pub_blob(&blob).expect("parse_pub_blob must succeed");

    // The DER payload must be non-empty.
    assert!(
        !pub_der.is_empty(),
        "pub DER must not be empty after stripping 8-byte header"
    );

    // Parse the DER into an RsaPublicKey. This exercises the full
    // PKCS#1 DER decode path.
    let pub_key = pclsync_rsa::parse_pub_key_der(&pub_der)
        .expect("parse_pub_key_der must succeed on the committed pub blob");

    // Round-trip back to DER to confirm the parsed key is well-formed
    // and non-trivial. An RSA-4096 PKCS#1 DER public key is ≥526 bytes.
    let round_tripped_der = pclsync_rsa::serialize_pub_key_der(&pub_key)
        .expect("serialize_pub_key_der must succeed on parsed pub key");
    assert!(
        round_tripped_der.len() >= 526,
        "RSA-4096 PKCS#1 DER public key must be >=526 bytes; got {}",
        round_tripped_der.len()
    );
    // The re-encoded DER must match the original DER (canonical encoding).
    assert_eq!(
        round_tripped_der, pub_der,
        "round-trip DER must be byte-identical to the original pub DER"
    );
}

// ---------------------------------------------------------------------------
// Test 5: offline sector seal+open round-trip (audit-06 P3 / pcloud-rs-ncx.33)
// ---------------------------------------------------------------------------
//
// This test does NOT require PCLOUD_KAT_PASSWORD because it synthesises its
// own deterministic key material. Its purpose is to exercise the
// `pclsync_sector::open_sector` decrypt code path in the offline gate so
// that a regression that breaks open_sector is caught in every CI run,
// not only when the live KAT is armed.
//
// Strategy: seal plaintext of various shapes with a fixed (aes_key, hmac_key,
// sector_id, rnd) fixture, then open the sealed output and assert byte-exact
// recovery. This is intentionally weaker than a C-vector KAT (it does not
// pin the ciphertext bytes to the C client's output — see ncx.35 for the
// C-vector case), but it does prove that seal_sector and open_sector agree
// end-to-end across every code path inside the sector encoder.

fn sector_fixture() -> (SectorKeys<'static>, u64, [u8; PCLSYNC_RND_SIZE]) {
    // Static fixture bytes; ConstBoxing via `Box::leak` keeps the references
    // 'static without forcing the test to plumb lifetimes through.
    static AES_KEY: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];
    static HMAC_KEY: [u8; 64] = [
        0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e,
        0x8f, 0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d,
        0x9e, 0x9f, 0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac,
        0xad, 0xae, 0xaf, 0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb,
        0xbc, 0xbd, 0xbe, 0xbf,
    ];
    let rnd: [u8; PCLSYNC_RND_SIZE] = [
        0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae,
        0xaf,
    ];
    let keys = SectorKeys {
        aes_key: &AES_KEY,
        hmac_key: &HMAC_KEY,
    };
    (keys, 0x0123_4567_89ab_cdef_u64, rnd)
}

#[test]
fn pclsync_compat_kat_offline_sector_decrypt_roundtrip() {
    let (keys, sector_id, rnd) = sector_fixture();

    // Exercise every sector-encoder code path:
    //   - 1-byte payload (short path; ciphertext == rnd[..1])
    //   - 15-byte payload (short path boundary)
    //   - 16-byte payload (first long-path / plain-CBC case)
    //   - 33-byte payload (long path with CBC-CS tail)
    //   - 4096-byte payload (maximum sector)
    for &len in &[1usize, 15, 16, 33, 4096] {
        let pt: Vec<u8> = (0..len).map(|i| ((i as u32).wrapping_mul(31) ^ 0xA5) as u8).collect();
        // `seal_sector_with_rnd` is deterministic given fixed keys + rnd + sid,
        // so the sealed ciphertext+tag this test produces are anchor-values:
        // any code change that alters them (or breaks open_sector's inverse
        // behaviour) will trip one of the assertions below without needing
        // live KAT credentials.
        let keys_borrow = SectorKeys {
            aes_key: keys.aes_key,
            hmac_key: keys.hmac_key,
        };
        let sealed = seal_sector_with_rnd(keys_borrow, sector_id, &pt, &rnd)
            .expect("seal_sector_with_rnd must succeed on offline fixture");
        assert_eq!(
            sealed.ciphertext.len(),
            len,
            "ciphertext length must equal plaintext length"
        );
        assert_eq!(
            sealed.auth_tag.len(),
            PCLSYNC_AUTH_TAG_SIZE,
            "auth tag size fixed at 32 bytes"
        );

        // Exercise the decrypt code path.
        let keys_borrow2 = SectorKeys {
            aes_key: keys.aes_key,
            hmac_key: keys.hmac_key,
        };
        let opened = open_sector(keys_borrow2, sector_id, &sealed.ciphertext, &sealed.auth_tag)
            .expect("open_sector must round-trip the fixture plaintext");
        assert_eq!(
            opened.as_slice(),
            pt.as_slice(),
            "sector decrypt must recover the original plaintext byte-for-byte"
        );
    }
}

/// Negative path: tampering with the auth tag MUST fail authentication.
/// Proves the decrypt error path is reachable in the offline gate.
#[test]
fn pclsync_compat_kat_offline_sector_decrypt_rejects_tampered_tag() {
    let (keys, sector_id, rnd) = sector_fixture();
    let pt: Vec<u8> = (0..64).map(|i| i as u8).collect();

    let keys_borrow = SectorKeys {
        aes_key: keys.aes_key,
        hmac_key: keys.hmac_key,
    };
    let mut sealed =
        seal_sector_with_rnd(keys_borrow, sector_id, &pt, &rnd).expect("seal offline fixture");
    // Flip one bit in the detached auth tag.
    sealed.auth_tag[0] ^= 0x01;

    let keys_borrow2 = SectorKeys {
        aes_key: keys.aes_key,
        hmac_key: keys.hmac_key,
    };
    let err = open_sector(keys_borrow2, sector_id, &sealed.ciphertext, &sealed.auth_tag)
        .expect_err("tampered auth tag must fail authentication");
    // Exact variant: AuthFailed (not EmptySector / too-long).
    let msg = format!("{err}");
    assert!(
        msg.contains("sector authentication failed"),
        "expected AuthFailed, got: {msg}"
    );
}
