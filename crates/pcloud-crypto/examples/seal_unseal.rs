#![allow(clippy::pedantic)]
//! Derives a per-file key from a master key, seals a sector with AES-256-GCM,
//! unseals it, and verifies that tampered ciphertext is rejected. Mirrors the
//! sector-oriented content crypto used by the real pCloud crypto-folder
//! code path.
//!
//! Run with: `cargo run -p pcloud-crypto --example seal_unseal`

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_crypto::content::{SECTOR_SIZE_BYTES, derive_file_key, open_sector, seal_sector};
use pcloud_secret::secret_bytes::SecretBytes;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Demo-only master key. A real master key is produced by Argon2 over the
    // user's crypto password and is wrapped in a `SecretBytes` that
    // zeroizes on Drop.
    let master = SecretBytes::new(vec![0x42u8; 32]);

    // Per-file key = HMAC-SHA256(master, label || file_seed).
    let file_seed = b"example-file-0001";
    let file_key = derive_file_key(&master, file_seed);

    let plaintext = b"AES-GCM sector payload for pcloud-crypto example".to_vec();
    let sector_index = 0u32;

    let frame = seal_sector(&file_key, sector_index, &plaintext, SECTOR_SIZE_BYTES)?;
    println!(
        "sealed: {} bytes plaintext -> {} bytes frame (index + nonce + ct+tag)",
        plaintext.len(),
        frame.len()
    );

    // Happy-path unseal.
    let round = open_sector(&file_key, sector_index, &frame)?;
    assert_eq!(round, plaintext);
    println!("unseal: round-trip ok ({} bytes)", round.len());

    // Tamper detection: flip one byte deep inside the ciphertext and confirm
    // the AEAD rejects the frame.
    let mut tampered = frame.clone();
    let target = tampered.len() - 5;
    tampered[target] ^= 0x01;
    match open_sector(&file_key, sector_index, &tampered) {
        Ok(_) => panic!("tampered frame must not authenticate"),
        Err(err) => println!("tamper:  correctly rejected ({err})"),
    }

    Ok(())
}
