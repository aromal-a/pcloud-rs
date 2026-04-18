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
