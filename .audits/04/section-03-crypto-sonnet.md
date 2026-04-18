# Section 3: Crypto Subsystem Audit
**Auditor:** Sonnet (independent, cross-validating with Opus)
**Date:** 2026-04-18
**Scope:** `crates/pcloud-crypto/src/` and `crates/pcloud-crypto/tests/`

---

## Summary Counts
- CRITICAL: 1
- HIGH: 2
- MEDIUM: 3
- LOW: 3

---

## CRITICAL

### CRIT-01: No Known-Answer Tests (KATs) against C-client ciphertext — cross-client compatibility unverified
**File:** `crates/pcloud-crypto/tests/round_trip.rs:1-14`

The `round_trip.rs` test file opens with an explicit disclaimer:
> "These are NOT known-answer tests against the C client. KATs against C-client vectors are tracked under bd-1du.10."

The placeholder `algorithm_parameters_documented` test has no assertions. The Rust implementation uses AES-256-GCM with HMAC-SHA256 key derivation and Argon2id for master-key derivation. The C client (`pcryptofolder.c`) uses a different primitive stack (historical evidence points to AES-CTR / RSA wrapping). Without a passing KAT against a real C-produced ciphertext, any claim of cross-client file access is unsubstantiated. If a user switches from the C client to the Rust daemon, all existing encrypted files are silently unreadable.

**Remediation:** Obtain a C-client test vector (sector ciphertext + key + sector index + expected plaintext) and add a failing-then-passing decrypt test. Gate the parity matrix row `fs,mounted pcloud filesystem` as `Partial` until this passes. Track under bd-1du.10.

---

## HIGH

### HIGH-01: Brute-force lockout resets on daemon restart — in-memory only, no persistent counter
**File:** `crates/pcloud-crypto/src/lib.rs:374-381`

`consecutive_failures` is `AtomicU32` annotated `#[serde(skip)]`. A comment confirms: "the lockout is in-process and is lost on restart." An attacker who can restart the daemon (e.g. via `systemctl restart pcloud-daemon`) gets a fresh 10-attempt window per restart. On a service that auto-restarts on crash, this reduces the lockout to ~10 guesses per crash cycle.

**Remediation:** Persist the failure counter to the profile store (SQLite or the auth vault) with a minimum cool-down timestamp. On reload, rehydrate the counter; enforce the lockout even across restarts for at least a configurable penalty window (e.g. 5 minutes of server time).

### HIGH-02: Temppass uses HMAC-SHA256 signature instead of RSA — breaks wire compatibility with C-client recipients
**File:** `crates/pcloud-crypto/src/share_temppass.rs:43-46, 211-220`

The module comment explicitly states the Rust path substitutes an HMAC-SHA256 signature for the RSA-4096 signature the C client expects (`prsa_sign_sha256_hash`). This means:
1. A Rust-side share invitation cannot be accepted by a C-side invitee, because the `sign` blob is an HMAC-SHA256 output, not an RSA signature verifiable with the invitee's public key.
2. The wire shape (`privenc` + `sign` two-blob) may differ in encoding from what the pCloud server and C client expect.

This is clearly documented and tracked under `bd-1du.5`, but it means the share-temppass feature is non-functional in a real heterogeneous environment where the invitee runs the C client.

**Remediation:** Implement RSA-4096 key-pair generation, storage (encrypted under the master key), and sign/verify before marking the shares/temppass row as `Implemented`. Until then, the parity matrix row should be `Partial`, not `Implemented`.

---

## MEDIUM

### MED-01: No NFC/NFD Unicode normalization on filenames before HMAC encoding
**File:** `crates/pcloud-crypto/src/metadata.rs:97-115`

`encrypt_filename` passes `plaintext.as_bytes()` directly into the HMAC without Unicode normalization. On macOS, HFS+ decomposes filenames to NFD; on Linux/Windows, NFC is typical. The same visual filename (e.g. `café`) produces two different HMAC hex tags under NFD vs NFC representations. This causes cross-platform lookup collisions: a file encrypted on macOS cannot be found from Linux by name without knowing which normal form was used.

**Remediation:** Apply NFC normalization (via `unicode-normalization` crate) to `plaintext` before the HMAC call, or document that callers must pre-normalize and add an assertion.

### MED-02: `change_password_unlocked` does not detect same-plaintext-password when salt rotates
**File:** `crates/pcloud-crypto/src/lib.rs:905-913`

The comment at line 907-912 explicitly documents that `change_password_unlocked` does NOT detect same-password rotation because the salt rotates. Only `change_password` (the outer wrapper) does a constant-time byte comparison before calling through. A direct call to `change_password_unlocked` with the identical password silently succeeds. This creates a footgun for callers that bypass `change_password`.

**Remediation:** Either make `change_password_unlocked` private (it currently has `pub` visibility) so callers are forced through `change_password`, or add the identical-password check inside `change_password_unlocked` by comparing `old_password` exposure before derivation.

### MED-03: Hand-rolled base64 in two separate modules with no length-fuzzing
**File:** `crates/pcloud-crypto/src/share_temppass.rs:408-491`, `crates/pcloud-crypto/src/password_scorer.rs` (uses `crypto_util` shim)

Two hand-rolled base64 implementations exist (one in `share_temppass.rs`, one consolidated into `crypto_util.rs`). Hand-rolled crypto-adjacent codecs are a maintenance hazard. The `b64_decode` in `share_temppass.rs` rejects non-multiple-of-4 input but does not fuzz-test the boundary at maximum blob sizes. No `cargo fuzz` target covers the decode path.

**Remediation:** Replace both hand-rolled implementations with the `base64` crate (already in the ecosystem with no license issue). Add a proptest or fuzz target covering the decode path with arbitrary byte inputs.

---

## LOW

### LOW-01: `sectors_sealed` nonce-exhaustion warning threshold is advisory only
**File:** `crates/pcloud-crypto/src/lib.rs:1181-1187`

When `sectors_sealed > u32::MAX` (≈4 billion sectors sealed per file-key per session), only a `log::warn!` is emitted. The operation still succeeds. A correct defense-in-depth posture would return an error and refuse to seal further sectors until the caller rotates the key, preventing the random-96-bit nonce collision window from opening silently.

**Remediation:** Return `Err(CryptoError::NonceExhaustion)` when `sectors_sealed` exceeds the threshold, forcing the daemon to rotate per-file keys.

### LOW-02: `getrandom` failure on `seal_sector` nonce generation panics instead of returning `Err`
**File:** `crates/pcloud-crypto/src/content.rs:189`

```rust
getrandom(&mut nonce_bytes).expect("OS randomness must be available");
```

This is inside `seal_sector`, a function that otherwise uses `Result`. On constrained environments (containers, early boot), CSPRNG failure should propagate as `ContentCryptoError` rather than abort the daemon process via `expect`.

**Remediation:** Map the `getrandom` error to `ContentCryptoError::InvalidFrame` (or add a new `CsrngUnavailable` variant) and return `Err`.

### LOW-03: Argon2id parameters are crate defaults — not pinned explicitly in code
**File:** `crates/pcloud-crypto/src/keys.rs:154-159`

`Argon2::default()` is used, meaning the parameters (`m=19456`, `t=2`, `p=1`) are determined by the `argon2` crate version, not this codebase. A crate upgrade could silently change the parameters and break existing profiles (wrong fingerprint on next `start`). The parameters are documented in doc comments but not enforced in code.

**Remediation:** Construct `Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::new(19456, 2, 1, Some(32)).unwrap())` explicitly so parameters are frozen in source, independent of crate default changes.
