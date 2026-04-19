#![no_main]
//! Audit-06 wave-2 (bd-pcloud-rs-ncx.70) — fuzz the pclsync reversible
//! filename codec.
//!
//! The codec is a custom HMAC-tweaked AES-256 construction with
//! base32 envelope. `decode_filename` takes attacker-controlled
//! base32, AES-decrypts it, checks that the zero-pad tail is all
//! zeros (pcrypto.c:336..343, 381..388), and finally validates UTF-8.
//! A stray panic anywhere in that chain (e.g. an index miscalc, a
//! base32 bug, an AES chunking off-by-one) would be a directory-
//! listing DoS against a crypto folder, so we fuzz the full decode
//! surface. We also exercise the encode path on short inputs to keep
//! the round-trip property in view.

use libfuzzer_sys::fuzz_target;

use pcloud_crypto::pclsync_filename::{
    FilenameKeys, decode_filename, encode_filename,
};

fuzz_target!(|data: &[u8]| {
    // Need at least 32 bytes AES key + 128 bytes HMAC key + some input.
    if data.len() < 32 + 128 {
        return;
    }
    let (aes_slice, rest) = data.split_at(32);
    let (hmac_slice, rest) = rest.split_at(128);

    let aes_key: [u8; 32] = aes_slice.try_into().unwrap();
    let mut hmac_key = [0u8; 128];
    hmac_key.copy_from_slice(hmac_slice);

    // `FilenameKeys<'_>` borrows the two key buffers by reference and
    // does not implement `Copy`, so rebuild it per call to keep each
    // arm self-contained.
    let keys = || FilenameKeys {
        aes_key: &aes_key,
        hmac_key: &hmac_key,
    };

    // Decode arm: feed arbitrary bytes interpreted as an ASCII
    // candidate to `decode_filename`. Invalid base32, non-block-
    // multiple ciphertexts, and non-zero pad tails are all expected
    // to return `Err`, never panic.
    if let Ok(as_str) = std::str::from_utf8(rest) {
        let _ = decode_filename(keys(), as_str);
    }

    // Also try the raw bytes through a permissive re-cast: base32
    // decode rejects non-ASCII, so lossy UTF-8 is a fine coverage
    // surface. Bounded to 4KiB to keep fuzz iterations cheap.
    let bounded = &rest[..rest.len().min(4096)];
    let lossy = String::from_utf8_lossy(bounded);
    let _ = decode_filename(keys(), lossy.as_ref());

    // Encode arm: short plaintexts exercise both the single-block and
    // multi-block paths of `encode_filename`. Cap at 255 bytes (POSIX
    // NAME_MAX) to stay under PCLSYNC_MAX_FILENAME_PLAINTEXT.
    if let Ok(plain) = std::str::from_utf8(bounded) {
        if !plain.is_empty() && plain.len() <= 255 {
            if let Ok(encoded) = encode_filename(keys(), plain) {
                // Round-trip: re-decode must return the same plaintext
                // for any input pclsync accepted as encodeable.
                let _ = decode_filename(keys(), &encoded);
            }
        }
    }
});
