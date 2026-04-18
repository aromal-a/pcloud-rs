# Section 3 — Crypto (Opus Audit, 2026-04-18)

**Scope audited:** `crates/pcloud-crypto/{content,keys,metadata,state,lib,share_temppass,password_scorer,policy}.rs` + `tests/round_trip.rs`.

Overall posture is strong (AEAD + Argon2id + `SecretBytes` zeroize + constant-time compare + forbid(unsafe_code)), but several issues remain before "enterprise ready" is defensible.

---

## CRITICAL

### C-1. No C-client KAT — cross-client ciphertext compatibility unverified
- File: `crates/pcloud-crypto/tests/round_trip.rs:23-40`, `:1-14`
- The only "KAT" is a placeholder comment. `TODO(bd-1du.10)` explicitly flags that Rust uses AES-256-GCM + HMAC-SHA256 file-key derivation while the legacy C `pcryptofolder.c` scheme is "TBD". A clean-room rewrite advertised as drop-in cannot land without a decrypt-KAT against real C ciphertext — silent cross-client data-loss is a realistic outcome. Source-of-truth comment at `tests/round_trip.rs:37`: "If these differ, cross-client file access WILL fail silently."
- Remediation: obtain sample C sector + filename output, add decrypt KATs, gate release on them.

---

## HIGH

### H-1. AAD mismatch between doc and code (content.rs vs KAT doc)
- `crates/pcloud-crypto/src/content.rs:191` encodes AAD as `sector_index.to_be_bytes()` (big-endian).
- `crates/pcloud-crypto/tests/round_trip.rs:31` documents AAD as "4-byte little-endian".
- Code is self-consistent (seal and open both use BE at `content.rs:191` and `content.rs:257`), so no runtime bug, but the KAT docstring is wrong and will mislead the C-parity comparator. Fix the comment and pin the endianness in a doc-test.

### H-2. `sectors_sealed` budget is warn-only, not enforced
- `crates/pcloud-crypto/src/lib.rs:1181-1188` — on exceeding `u32::MAX` sealed sectors the shell only emits `log::warn!` and keeps sealing. With 96-bit random nonces the birthday bound (~2^32 per key) is where collision probability becomes non-negligible; past that the guarantee degrades and the code still produces frames. For an enterprise posture this must be a hard refusal (`CryptoError::Content(ContentCryptoError::...)`) or automatic key rotation, not a log line. Also note the comparison `count > u32::MAX as u64` checks *after* `fetch_add`, so one extra seal past the limit occurs.

### H-3. PBKDF2 iteration count kept at legacy 5000
- `crates/pcloud-crypto/src/password_scorer.rs:540` `PBKDF2_ITERS_LEGACY: u32 = 5000`.
- Used for `psync_derive_password_from_passphrase` (API-login password). OWASP 2023 minimum for PBKDF2-HMAC-SHA512 is 210,000. The code comment at `:641` acknowledges this is "legacy parameter inherited" but the value is used unchanged. For any new account flows the derivation should upgrade (or pivot to Argon2id to match the at-rest key path). At minimum, bump legacy to current OWASP and add a policy flag.

### H-4. No NFC / Unicode normalization on passwords or filenames
- Grep for `NFC|normalize|unicode-normalization` across the crate: zero matches.
- `content::encrypt_filename` and `KeyManager::derive_key_material` both consume raw bytes. A user that types the same passphrase on two platforms with different IME composition (macOS vs Linux) will get different Argon2 outputs and unlock will silently fail; a filename authored on NFD macOS will hash to a different tag than the NFC server-side canonical. Add NFC normalization at the boundary (before Argon2id input and before filename HMAC) and document the normalization profile in a KAT.

### H-5. Brute-force lockout is in-process only and easily bypassed
- `crates/pcloud-crypto/src/lib.rs:372-381` and `:759-765` — `consecutive_failures` is `#[serde(skip)]` and "lost on restart". An attacker that can crash/restart the daemon (or simply wait for service restart, or racing multiple `CryptoShell` instances) trivially resets the counter. There is also no time-based backoff.
- Remediation: persist the counter + a monotonic lockout-until wall clock alongside the setup fingerprint; keep the refusal on restart; add exponential backoff per attempt.

### H-6. Share-temppass "signature" is symmetric HMAC, not RSA
- `crates/pcloud-crypto/src/share_temppass.rs:41-46, 209-220` — the module explicitly ships an HMAC-SHA256 "signature" under the active master key as a substitute for the C client's `prsa_sign_sha256_hash`. This means the invitee cannot verify provenance against the sender's public key: any party holding the master key can forge the signature, and the invitee cannot distinguish sender from themselves post-share. Documented under `bd-1du.5`, but currently any share/teamshare claim is not cryptographically authenticated in the C-equivalent sense.

---

## MEDIUM

### M-1. `getrandom` failure is `.expect()` panic at `content.rs:189`, `keys.rs:89`, `lib.rs:573`, `share_temppass.rs:304-305`
- Policy is "treat as unrecoverable host fault" (documented at `content.rs:176`). Acceptable but fragile on constrained boot / container starts. Consider propagating via `CryptoError` for the daemon paths so the runtime can retry with backoff rather than crash.

### M-2. `CryptoMode::Kms.wrapped_dek` cloned on every sector op
- `crates/pcloud-crypto/src/lib.rs:600-601` — `WrappedDek(wrapped_dek.clone())` on each `derive_sector_file_key`. The `unwrap_cached` TTL helps, but the clone still pays allocation cost per-sector and the cloned plaintext DEK is materialised into a fresh `SecretBytes` at `:1221` (double-copy before `drop(dek)`). Minor residency window.

### M-3. Password-rotation does not invalidate old wrapped artefacts
- `lib.rs:867-1016` rotates salt + fingerprint but (per grep) does not re-wrap any outstanding `CryptoMode::Kms.wrapped_dek`. On password rotation the KMS-wrapped DEK remains bound to the old session context; documented behavior may be intentional but should be explicit.

### M-4. `TemppassError` → `CryptoError::WrongPassword` lumping (`share_temppass.rs:99-110`) is correct for the oracle surface, but the module-internal `TemppassError::Unwrap` and `::BadSignature` variants are still observable in tests and in `Display` (`:92`) — be sure production log layer only prints the collapsed `CryptoError`.

### M-5. Hand-rolled base64 encoder/decoder (`share_temppass.rs:408-491`)
- Duplicates `crypto_util` per lib.rs:89 comment. Hand-rolled base64 is a recurring footgun; consolidate to `base64` crate or single `crypto_util` path and delete the duplicate. Low exploit probability but non-constant-time on decode path via match arms.

---

## LOW

### L-1. Filename HMAC determinism leaks equal plaintext across folders
- `crates/pcloud-crypto/src/metadata.rs:80-84` — documented trade-off, acceptable for server lookup, but should be surfaced in end-user deployment docs.

### L-2. `cache_ttl_secs: 300` default (`keys.rs:93`) is unreferenced
- No code enforces a TTL on `active_key_material`. Field is dead policy state. Either wire an auto-stop timer or remove.

### L-3. `#![allow(clippy::pedantic)]` at `lib.rs:42`
- Broad allow on a crypto crate is contrary to enterprise posture. Replace with targeted `allow`s.

### L-4. `FILENAME_LABEL` and `file-key/v1` labels lack a profile-version epoch
- If the master-key scheme ever migrates, the version is only in the label string. A top-level `ProfileVersion` constant and serialized discriminant in the profile store would make migrations auditable.

---

## Passing controls (worth keeping)

- `#![forbid(unsafe_code)]` at `lib.rs:1`.
- `subtle::ConstantTimeEq` on fingerprint match (`keys.rs:200-206`) and on temppass signature verify (`share_temppass.rs:227`).
- `SecretBytes`/`SecretString` zeroize-on-drop and redacted `Debug` (`share_temppass.rs:165-173`).
- AAD binding of sector index, with pre-AEAD equality gate (`content.rs:259-262`) — defense-in-depth against swap.
- Distinct domain-separated HMAC labels for file-key / filename / fingerprint / temppass signature.
- Salt + nonce freshness proven by `distinct_invocations_produce_distinct_wires` (`share_temppass.rs:591-599`).
- Password-rotation rotates salt atomically (`keys.rs:147-160`).
- Refusal to persist master key via `CryptoPolicy::is_safe()` gate on `setup/start/enable_kms_mode`.
