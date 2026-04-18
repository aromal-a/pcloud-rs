#![no_main]
use libfuzzer_sys::fuzz_target;

use pcloud_crypto::content::open_sector;
use pcloud_secret::secret_bytes::SecretBytes;

fuzz_target!(|data: &[u8]| {
    if data.len() < 32 {
        return;
    }
    // Split data into a fake 32-byte key and the remainder as ciphertext frame.
    // open_sector takes: file_key: &SecretBytes, expected_index: u32, frame: &[u8]
    let (key_bytes, frame) = data.split_at(32);
    let file_key = SecretBytes::new(key_bytes.to_vec());

    // Try several candidate sector indices derived from the frame header so
    // the fuzzer can explore both the mismatch and the AEAD path.
    let index_from_frame: u32 = if frame.len() >= 4 {
        u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]])
    } else {
        0
    };

    // Must NEVER panic — only return Ok or Err.
    let _ = open_sector(&file_key, index_from_frame, frame);
    let _ = open_sector(&file_key, 0, frame);
    let _ = open_sector(&file_key, u32::MAX, frame);
});
