# Section 3: Crypto Subsystem
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 3)

## Findings

### CRITICAL [3]
### HIGH [6]
### MEDIUM [7]
### LOW [3]

---

## Detailed Findings

### CRITICAL-3.A — No C-client KAT / byte-compat test vectors

`crates/pcloud-crypto/tests/kat_compatibility.rs:23-38` contains only a placeholder ("TBD: obtain from upstream"). `docs/enterprise/crypto-compat.md:1-35` explicitly warns the format is NOT byte-compatible with `pclsync/pcryptofolder.c`. Cross-client round-trip is therefore unverifiable and likely broken. **Severity: CRITICAL** — claimed parity is unsupported by evidence.

**Fix:** Obtain C-client sample ciphertexts (test vectors), add real KATs, gate any "full parity" claim on this test passing.

---

### CRITICAL-3.B — Password rotation silently orphans all ciphertext

`crates/pcloud-crypto/src/lib.rs:837-896`: `change_password_unlocked` rotates the master key + salt but does NOT re-encrypt existing sector or filename data. The per-file key is derived as `HMAC(master, seed)` (`src/content.rs:126-134`), so rotating the master invalidates every prior sector. The test at `tests/kat_compatibility.rs:119-164` confirms the break — but treats it as expected behavior with no remediation path. No KEK indirection layer exists.

**Fix:** Introduce a persistent per-file Key Encryption Key (KEK) encrypted by the master key. On password rotation, re-wrap only the master→KEK binding, leaving per-file keys and all ciphertext intact. This is the standard envelope-encryption pattern.

---

### CRITICAL-3.C — No NFC/NFD normalization on filenames

`crates/pcloud-crypto/src/metadata.rs:90-108` hashes `plaintext.as_bytes()` raw without Unicode normalization. `docs/enterprise/crypto-compat.md:12` claims "NFC(name)" is applied — this is a documentation inaccuracy. macOS HFS+ delivers filenames in NFD form; Linux ext4 delivers NFC. Two clients syncing the same file will produce different HMAC-derived encrypted names, causing a desync loop. The `.audits/01/section-03-crypto.md` already flagged this; it was not fixed.

**Fix:** Add `unicode-normalization` to `Cargo.toml` dependencies; apply `.nfc().collect::<String>()` to `plaintext` before computing the HMAC in `encrypt_filename`.

---

### HIGH-3.D — MAX_ENCRYPTED_FILENAME_BYTES not enforced at encrypt time

`crates/pcloud-crypto/src/metadata.rs:90-96` rejects only empty paths and paths containing `/`. No 255-byte limit is enforced on the plaintext or ciphertext filename length. Callers can produce names that exceed backend filesystem and API limits.

**Fix:** Add `const MAX_ENCRYPTED_FILENAME_BYTES: usize = 255;` and return `Err(CryptoError::FilenameTooLong)` when `encoded_len > MAX_ENCRYPTED_FILENAME_BYTES` before returning.

---

### HIGH-3.E — Brute-force lockout (consecutive_failures counter) absent from keys.rs

`docs/enterprise/crypto-compat.md:56-57` claims "after 5 consecutive wrong-password attempts the layer refuses further attempts". No such counter exists in `crates/pcloud-crypto/src/keys.rs` or `src/lib.rs`. The `lib.rs` `start()` function does not track failed unlock attempts. Attackers can hammer unlimited Argon2 unlock attempts.

**Fix:** Add `consecutive_failures: u32` to `KeyManager` or `CryptoShell`, increment on wrong-password error in `start()`, reset on success, and return `Err(CryptoError::TooManyFailures)` after 5 failures.

---

### HIGH-3.F — Legacy PBKDF2 iteration count = 5000 for API password derivation

`crates/pcloud-crypto/src/password_scorer.rs:536`: `PBKDF2_ITERS = 5000`. This is below 2010-era OWASP minimums (PBKDF2-HMAC-SHA256 at ≥600,000 per current guidance). The comment acknowledges it as a legacy parameter. While server-coordination is required to change this, no higher-iteration migration path exists.

**Fix:** Add a server-negotiated or config-driven parameter for the iteration count; document that 5000 is a known legacy constraint and link the tracking bead.

---

### HIGH-3.G — PBKDF2 intermediate buffers not zeroized

`crates/pcloud-crypto/src/password_scorer.rs:548-572`: Stack arrays `u: [u8; 64]` and `t: [u8; 64]` hold intermediate PRF state derived directly from the passphrase. Neither is zeroized before the function returns. On stack frames that get reused, these bytes may survive past the function scope.

**Fix:** Call `u.zeroize(); t.zeroize();` at loop exit before the function returns. Add `use zeroize::Zeroize;` import.

---

### HIGH-3.H — Share temppass uses HMAC substitute instead of RSA signing (wire incompatibility)

`crates/pcloud-crypto/src/share_temppass.rs:41-45,213-220`: The C client uses `prsa_sign_sha256_hash(crypto_privkey, …)` for temppass authentication. The Rust implementation substitutes HMAC-SHA256 under the master key. This breaks the C wire contract: the invitee cannot verify origin without the sharer's master key. The parity matrix line 124 marks this as "Implemented" — that is misleading.

**Fix:** Implement RSA-4096 keypair generation and signing mirroring the C `prsa_*` API; use RSA signatures in `share_temppass.rs`. Track under `bd-1du.5` or a child bead.

---

### HIGH-3.I — AAD endianness mismatch between code and documentation

`crates/pcloud-crypto/src/content.rs:141,191` uses `to_be_bytes()` (big-endian) for the sector offset AAD. `tests/kat_compatibility.rs:29` and `docs/enterprise/crypto-compat.md:14` both document it as "4-byte little-endian". Any external re-implementer following the docs will produce incompatible frames.

**Fix:** Align the documentation to the actual code (big-endian), or if the C client uses little-endian, switch `content.rs` to `to_le_bytes()` and add a KAT to verify.

---

### MEDIUM-3.J — Key-schedule depth is only 2 levels; no per-folder binding

The derivation chain is `master → per-file` via `HMAC(master, "pcloud-crypto/file-key/v1" || file_seed)`. There is no per-folder key layer, no per-sector rekey, and no rotation boundary. `crates/pcloud-crypto/src/lib.rs:1098-1101` explicitly defers the 2^32-sector rekey limit enforcement.

**Fix:** Document the key schedule in `keys.rs`; add per-folder binding to the file-key PRF input (`… || folder_id || file_seed`); expose a `sectors_sealed` counter so the daemon can trigger rekey before the 2^32 boundary.

---

### MEDIUM-3.K — CLAUDE.md stale: claims change_crypto_pass family is "Still missing"

`CLAUDE.md:223-227` states `change_crypto_pass`, `send_change_user_private`, and `priv_key_flags` are "Still missing". In the current codebase they appear to be partially implemented at `crates/pcloud-daemon/src/runtime.rs:2658-2842` and `crates/pcloud-daemon/src/crypto_backend.rs:234-255`. The documentation is out of sync.

**Fix:** Update `CLAUDE.md` to reflect actual implementation status; cross-check the parity matrix rows for these functions and update accordingly.

---

### MEDIUM-3.L — `unwrap_active_dek` clones plaintext DEK into unzeroizing Vec

`crates/pcloud-crypto/src/lib.rs:1158-1167`: `pt.expose().to_vec()` duplicates plaintext DEK bytes into a fresh `Vec<u8>` before wrapping in `SecretBytes`. Between the `.to_vec()` allocation and the `SecretBytes` wrap, the bytes live in an un-zeroizing allocation.

**Fix:** Use a zeroizing intermediate (e.g., `Zeroizing<Vec<u8>>`) or consume `pt` directly without cloning.

---

### MEDIUM-3.M — `PlaintextDek` derives `Zeroize` but not `ZeroizeOnDrop`

`crates/pcloud-kms/src/lib.rs:120-122` (if present): `#[derive(Zeroize)]` without `ZeroizeOnDrop`. Accidental drops — e.g., via `?` error propagation — will not scrub memory.

**Fix:** Add `ZeroizeOnDrop` to the derive: `#[derive(Zeroize, ZeroizeOnDrop)]`.

---

### MEDIUM-3.N — No server-side crypto reset handshake

`CryptoShell::reset` (`crates/pcloud-crypto/src/lib.rs:1005-1013`) only wipes local in-memory state and explicitly notes it does NOT unlink remote encrypted content. There is no server-side reset call, no fingerprint-ack wire operation. The C client's reset flow includes a remote operation.

**Fix:** Wire a `crypto_reset` API call or clearly document in the parity matrix that this is a local-only operation with a Rejected/intentional-gap rationale.

---

### MEDIUM-3.O — No sector-sealed counter for rekey budget enforcement

`crates/pcloud-crypto/src/lib.rs:1096-1101` documents the 2^32-sector limit but exposes no counter. The daemon cannot observe nonce exhaustion proximity and cannot schedule a rekey proactively.

**Fix:** Add `sectors_sealed: AtomicU64` metric on `CryptoShell` or `ContentCrypto`; expose it via the observability layer.

---

### MEDIUM-3.P — `change_password_unlocked` does not reject identical password

`crates/pcloud-crypto/src/lib.rs:859-896`: the comment acknowledges that the identical-password check is intentionally absent in `change_password_unlocked`. A programmatic caller can rotate to the same passphrase silently, producing a misleading audit trail.

**Fix:** Add a constant-time comparison between the newly derived key and the current `active_key_material` before committing; return `Err(CryptoError::SamePassphrase)` on match.

---

### LOW-3.Q — Hand-rolled base64 duplicated in two modules

`crates/pcloud-crypto/src/share_temppass.rs:408-491` duplicates base64 encode/decode logic also present in `src/password_scorer.rs:577-607`. Maintenance risk and divergence surface.

**Fix:** Consolidate into a `crypto_util` module or use the `base64` crate consistently.

---

### LOW-3.R — `wrapped_dek.clone()` on hot sector path

`crates/pcloud-crypto/src/lib.rs:568`: Every sector operation clones the wrapped DEK `Vec<u8>`. On high-throughput paths this causes repeated allocations.

**Fix:** Redesign `unwrap_cached` to borrow the wrapped DEK rather than clone.

---

### LOW-3.S — Test file name `kat_compatibility.rs` is misleading

The file contains only self-round-trip tests, not known-answer tests against external (C-client) vectors.

**Fix:** Rename to `round_trip.rs` until real KATs from the C client are added; create a separate `kat_c_compat.rs` for external vectors.

---

## Summary Matrix

| Area | Status | Severity |
|------|--------|----------|
| Nonce generation (fresh 12-byte CSPRNG per sector) | ✓ OK | — |
| AES-256-GCM sector encrypt/decrypt | ✓ Present | — |
| Key schedule depth | Shallow (2 levels) | MEDIUM |
| PBKDF2 iterations (API password) | 5000 — legacy | HIGH |
| NFC normalization on filenames | ✗ Missing | CRITICAL |
| Filename length enforcement | ✗ Missing | HIGH |
| Password rotation / KEK | Breaks all ciphertext | CRITICAL |
| Brute-force lockout | ✗ Missing | HIGH |
| C-client KAT test vectors | ✗ Missing | CRITICAL |
| Share temppass RSA signing | HMAC substitute | HIGH |
| Constant-time comparisons | ✓ Used (subtle crate) | — |
| Zeroize on Drop (key material) | Mostly OK; 2 gaps | HIGH / MEDIUM |
| change_crypto_pass family | Appears implemented (CLAUDE.md stale) | MEDIUM |
| Per-sector rekey budget | Deferred, no counter | MEDIUM |
| AAD endianness docs vs code | Mismatch | HIGH |
