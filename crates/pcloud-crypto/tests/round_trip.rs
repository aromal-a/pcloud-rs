//! Self round-trip tests for pcloud-crypto. These are NOT known-answer tests
//! against the C client. KATs against C-client vectors are tracked under
//! bd-1du.10.
//!
//! IMPORTANT: The Rust pcloud-crypto implementation uses AES-256-GCM
//! with HMAC-SHA256 key derivation. This format may differ from the
//! legacy C client (pcloudcom/pcloud-rs pcryptofolder.c).
//!
//! TODO(bd-1du.10): Before claiming cross-client file access, obtain a
//! sample ciphertext from the C client and add a decrypt KAT here.
//! Until that test passes, files encrypted by the C client should be
//! treated as potentially incompatible.
//!
//! Track in: bd-1du.10

use pcloud_crypto::CryptoShell;
use pcloud_secret::secret_bytes::SecretBytes;
use pcloud_secret::secret_string::SecretString;

#[cfg(test)]
mod kat {
    use super::*;

    /// Documents the Rust AES-256-GCM sector wire contract. Asserting here
    /// — rather than silently noting — locks the format. Any deviation
    /// from the stated primitives or AAD endianness will fail a companion
    /// KAT (`kat_c_client_vector`, `hand_computed_aad_roundtrip`) below.
    ///
    /// Rust implementation uses:
    /// - Cipher: AES-256-GCM
    /// - Key derivation: HMAC-SHA256(master_key, "pcloud-crypto/file-key/v1" || file_seed)
    /// - Nonce: 96-bit from OsRng (random per sector)
    /// - **AAD: sector index (4-byte big-endian)** — see `content.rs::seal_sector`
    ///   using `sector_index.to_be_bytes()`. Earlier revisions of this test
    ///   incorrectly documented "little-endian"; fixed under audit 04 H-1.
    /// - Master key derivation: Argon2id (m=19456, t=2, p=1), 16-byte salt, 32-byte output
    ///
    /// C implementation (pcryptofolder.c) has not been captured as a
    /// cross-client KAT (tracked under bd-1du.10). Cross-client file
    /// access remains `Partial` in the parity matrix until a C-generated
    /// fixture is obtained.
    #[test]
    fn algorithm_parameters_documented() {
        // Concrete structural invariant: the Rust frame layout must stay
        // [BE u32 idx][12-byte nonce][ct || 16-byte tag] = 32 bytes overhead.
        assert_eq!(pcloud_crypto::content::SECTOR_OVERHEAD, 4 + 12 + 16);
    }

    /// Round-trip test: encrypt then decrypt produces the original plaintext.
    ///
    /// This proves the Rust implementation is internally consistent, but does
    /// NOT prove cross-client compatibility with the C pcloud client.
    #[test]
    fn sector_round_trip() {
        let mut shell = CryptoShell::default();
        shell
            .setup(SecretString::new("test-master-password-kat"), None)
            .expect("setup must succeed");
        shell
            .start(SecretString::new("test-master-password-kat"))
            .expect("start must succeed");

        // A deterministic file seed (32 bytes), simulating a per-file seed
        // as would be generated for a real encrypted file.
        let file_seed = [0x42u8; 32];
        let sector_index: u32 = 0;
        let plaintext = b"known-answer-test-plaintext-payload-12345678";

        let sealed = shell
            .seal_sector(&file_seed, sector_index, plaintext)
            .expect("seal_sector must succeed");

        // Sanity: ciphertext must not be identical to plaintext.
        assert_ne!(
            &sealed[..plaintext.len().min(sealed.len())],
            plaintext.as_ref()
        );

        let recovered = shell
            .open_sector(&file_seed, sector_index, &sealed)
            .expect("open_sector must succeed");

        assert_eq!(
            recovered, plaintext,
            "round-trip decryption must recover the original plaintext"
        );
    }

    /// Cross-sector isolation: sector 0 ciphertext cannot be decrypted as sector 1.
    ///
    /// Proves the sector index AAD binding is enforced.
    #[test]
    fn sector_index_aad_binding() {
        let mut shell = CryptoShell::default();
        shell
            .setup(SecretString::new("test-aad-binding"), None)
            .expect("setup");
        shell
            .start(SecretString::new("test-aad-binding"))
            .expect("start");

        let file_seed = [0xBEu8; 32];
        let plaintext = b"sector-aad-binding-test";

        let sealed = shell
            .seal_sector(&file_seed, 0, plaintext)
            .expect("seal sector 0");

        // Attempting to open sector 0 ciphertext as sector 1 must fail.
        let result = shell.open_sector(&file_seed, 1, &sealed);
        assert!(
            result.is_err(),
            "opening sector 0 ciphertext as sector 1 must fail (AAD mismatch)"
        );
    }

    /// HIGH-3.I: Password rotation invalidates all existing sector ciphertext.
    ///
    /// Proves that rotating the master key (via `change_password_unlocked`)
    /// makes ciphertext produced under the old key unreadable. This is a
    /// fundamental design constraint of the direct master-key-derivation
    /// approach: per-file keys are `HMAC-SHA256(master_key, ...)`, so any
    /// change to `master_key` invalidates all derived per-file keys.
    ///
    /// Callers MUST complete a full re-encryption pass after rotation.
    /// See: bd-1du.10 for KEK-indirection architecture tracking.
    #[test]
    fn password_rotation_invalidates_ciphertext() {
        let mut shell = CryptoShell::default();
        shell
            .setup(SecretString::new("password-a"), None)
            .expect("setup must succeed");
        shell
            .start(SecretString::new("password-a"))
            .expect("start with password-a");

        let file_seed = [0xABu8; 32];
        let plaintext = b"sensitive-data-encrypted-under-key-a";

        // Encrypt under key A.
        let sealed_under_a = shell
            .seal_sector(&file_seed, 0, plaintext)
            .expect("seal under key A must succeed");

        // Rotate to password B — this derives a new master key.
        shell
            .change_password_unlocked(SecretString::new("password-b"), 0)
            .expect("password rotation must succeed");

        // Now the shell holds key B. Attempting to open the ciphertext that
        // was produced under key A must fail (AEAD tag mismatch).
        let result = shell.open_sector(&file_seed, 0, &sealed_under_a);
        assert!(
            result.is_err(),
            "ciphertext sealed under key A must be unreadable after rotation to key B"
        );

        // Sanity-check: re-encrypting the same plaintext under key B produces
        // different ciphertext.
        let sealed_under_b = shell
            .seal_sector(&file_seed, 0, plaintext)
            .expect("seal under key B must succeed");
        assert_ne!(
            sealed_under_a, sealed_under_b,
            "ciphertext under key A and key B must differ"
        );

        // And the new ciphertext is readable under key B.
        let recovered = shell
            .open_sector(&file_seed, 0, &sealed_under_b)
            .expect("ciphertext sealed under key B must be readable");
        assert_eq!(recovered, plaintext);
    }

    /// Self-consistency KAT: encrypts a known plaintext and verifies structural
    /// properties of the ciphertext are stable across builds.
    ///
    /// This is NOT a cross-client test (C-client binary unavailable in CI) —
    /// it detects algorithm regressions by verifying the output length invariant
    /// and confirming encryption succeeds on known-good input.
    ///
    /// Update the expected structural properties only after verifying the new
    /// ciphertext is correct against the C client (bd-1du.10).
    #[test]
    fn kat_known_sector_vector() {
        let mut shell = CryptoShell::default();
        shell
            .setup(SecretString::new("kat-master-password"), None)
            .expect("setup must succeed");
        shell
            .start(SecretString::new("kat-master-password"))
            .expect("start must succeed");

        let file_seed = [0x42u8; 32];
        let sector_index: u32 = 0;
        let plaintext = [0xABu8; 32];

        let result = shell.seal_sector(&file_seed, sector_index, &plaintext);
        assert!(
            result.is_ok(),
            "sector encryption must not fail on valid input"
        );
        let ct = result.unwrap();
        // Wire layout: [4-byte sector index][12-byte nonce][ciphertext + 16-byte GCM tag]
        // So total overhead is 4 + 12 + 16 = 32 bytes beyond plaintext length.
        let expected_len = plaintext.len() + pcloud_crypto::content::SECTOR_OVERHEAD;
        assert_eq!(
            ct.len(),
            expected_len,
            "ciphertext length must equal plaintext length + sector overhead ({})",
            pcloud_crypto::content::SECTOR_OVERHEAD
        );
        // Ciphertext must differ from plaintext (sanity check encryption occurred).
        assert_ne!(&ct[..plaintext.len().min(ct.len())], plaintext.as_ref());

        // Round-trip must recover original plaintext.
        let recovered = shell
            .open_sector(&file_seed, sector_index, &ct)
            .expect("open_sector must succeed");
        assert_eq!(recovered, plaintext.as_ref());
    }

    /// Committed-fixture KAT (see `tests/fixtures/c_client_kat/README.md`).
    ///
    /// Reads a pre-generated sector frame and decrypts it with
    /// `pcloud_crypto::content::open_sector`. Any change to AAD width /
    /// endianness, per-file key derivation label, or frame layout will
    /// fail this test with `AuthFailed` or a length mismatch.
    ///
    /// NOTE: this fixture is **not** a cross-client vector against the
    /// legacy C client (pclsync/pcryptofolder.c). It is generated from
    /// the same spec using the Python `cryptography` library so the wire
    /// format is locked against regression. Cross-client KAT is tracked
    /// under bd-1du.10.
    #[test]
    fn kat_c_client_vector() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        use std::path::Path;

        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("c_client_kat");

        let master_hex =
            std::fs::read_to_string(base.join("master_key.hex")).expect("read master_key.hex");
        let master = hex_decode(master_hex.trim());
        assert_eq!(master.len(), 32, "master key must be 32 bytes");

        let seed_hex =
            std::fs::read_to_string(base.join("file_seed.hex")).expect("read file_seed.hex");
        let file_seed = hex_decode(seed_hex.trim());
        assert_eq!(file_seed.len(), 32, "file seed must be 32 bytes");

        let sector = std::fs::read(base.join("sector.bin")).expect("read sector.bin");
        let expected = std::fs::read(base.join("expected_plaintext.bin"))
            .expect("read expected_plaintext.bin");

        // Derive per-file key exactly as pcloud_crypto::content::derive_file_key does.
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&master).expect("hmac new from master");
        mac.update(b"pcloud-crypto/file-key/v1");
        mac.update(&file_seed);
        let file_key_bytes = mac.finalize().into_bytes();
        let file_key = SecretBytes::new(file_key_bytes.to_vec());

        let recovered = pcloud_crypto::content::open_sector(&file_key, 0, &sector)
            .expect("open_sector must succeed on committed KAT fixture");
        assert_eq!(
            recovered, expected,
            "KAT plaintext mismatch — sector wire format has drifted"
        );
    }

    /// Hand-computed AAD roundtrip: proves the AAD bound into the AEAD is
    /// the **big-endian** 4-byte sector index, matching the code in
    /// `content.rs::seal_sector` (H-1).
    ///
    /// Encrypts a plaintext via AES-256-GCM directly with a known key and
    /// a hand-computed BE AAD, then verifies that
    /// `pcloud_crypto::content::open_sector` accepts the resulting frame.
    /// If someone flips the AAD encoding to little-endian in the Rust
    /// code, `open_sector` will reject this frame with `AuthFailed`.
    #[test]
    fn hand_computed_aad_roundtrip() {
        use aes_gcm::{
            Aes256Gcm, KeyInit, Nonce,
            aead::{Aead, Payload},
        };

        let file_key_bytes = [0x11u8; 32];
        let file_key = SecretBytes::new(file_key_bytes.to_vec());
        let sector_index: u32 = 0x01020304;
        let nonce_bytes = [0x77u8; 12];
        let plaintext = b"aad-endianness-roundtrip-vector";

        // AAD must be BE to match Rust seal_sector.
        let aad_be = sector_index.to_be_bytes();

        let cipher = <Aes256Gcm as KeyInit>::new_from_slice(&file_key_bytes).unwrap();
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext.as_ref(),
                    aad: &aad_be,
                },
            )
            .expect("encrypt");

        // Reconstruct the frame layout used by seal_sector:
        // [BE u32 idx][12-byte nonce][ct || 16-byte tag].
        let mut frame = Vec::with_capacity(4 + 12 + ct.len());
        frame.extend_from_slice(&aad_be);
        frame.extend_from_slice(&nonce_bytes);
        frame.extend_from_slice(&ct);

        let pt = pcloud_crypto::content::open_sector(&file_key, sector_index, &frame)
            .expect("open_sector must accept BE-AAD frame");
        assert_eq!(pt, plaintext);

        // Sanity: if we had built the AAD as little-endian, open_sector
        // with the true index would reject. Build an LE-AAD frame and
        // prove it fails.
        let aad_le = sector_index.to_le_bytes();
        let ct_le = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext.as_ref(),
                    aad: &aad_le,
                },
            )
            .expect("encrypt-le");
        let mut frame_le = Vec::with_capacity(4 + 12 + ct_le.len());
        // Keep the header index BE so `open_sector`'s pre-AEAD index
        // check passes; the AEAD itself will reject due to AAD mismatch.
        frame_le.extend_from_slice(&aad_be);
        frame_le.extend_from_slice(&nonce_bytes);
        frame_le.extend_from_slice(&ct_le);
        assert!(
            pcloud_crypto::content::open_sector(&file_key, sector_index, &frame_le).is_err(),
            "LE-AAD frame must be rejected by BE-expecting open_sector"
        );
    }

    fn hex_decode(s: &str) -> Vec<u8> {
        assert!(
            s.len() % 2 == 0,
            "hex string must have even length"
        );
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    /// Cross-file isolation: file A's ciphertext cannot be decrypted with file B's seed.
    ///
    /// Proves per-file key derivation isolates files from each other.
    #[test]
    fn cross_file_seed_isolation() {
        let mut shell = CryptoShell::default();
        shell
            .setup(SecretString::new("test-cross-file"), None)
            .expect("setup");
        shell
            .start(SecretString::new("test-cross-file"))
            .expect("start");

        let seed_a = [0x01u8; 32];
        let seed_b = [0x02u8; 32];
        let plaintext = b"cross-file-isolation-test";

        let sealed = shell
            .seal_sector(&seed_a, 0, plaintext)
            .expect("seal with seed_a");

        // Opening with a different seed must fail.
        let result = shell.open_sector(&seed_b, 0, &sealed);
        assert!(
            result.is_err(),
            "opening with a different file seed must fail (key isolation)"
        );
    }
}
