# Section 3 — Crypto Subsystem Audit

**Scope:** `crates/pcloud-crypto/`, `crates/pcloud-kms/`, `docs/enterprise/crypto-compat.md`
**Mode:** Read-only enterprise readiness audit
**Date:** 2026-04-17

---

## Executive Summary

The Rust crypto path is internally sound on first-principles AEAD, key handling, and zeroization discipline — materially stricter than the legacy C client. However, **six of the audit-request items that were supposedly added are NOT present in the code** (brute-force lockout, `sectors_sealed` AtomicU64, `consecutive_failures`, `MAX_ENCRYPTED_FILENAME_BYTES`, NFC normalization, `PBKDF2_ITERS_LEGACY`, `SamePassphrase`). Additionally, the newly-created `crypto_util.rs` is **orphaned** (not declared as a module and `base64` is not in the crate's `Cargo.toml`), so the claimed base64 consolidation has not actually landed — `share_temppass.rs` and `password_scorer.rs` still use their own hand-rolled base64. There are **no C-client KATs**, which remains a CRITICAL parity gap.

**Severity counts:** 1 CRITICAL, 4 HIGH, 6 MEDIUM, 3 LOW.

---

## CRITICAL Findings

### CRITICAL-C.1 — No C-client KAT exists (cross-client compatibility is unverified)
**File:** `crates/pcloud-crypto/tests/round_trip.rs:23-40`, `docs/enterprise/crypto-compat.md:19-20`

The file formerly named `kat_compatibility.rs` has been renamed to `round_trip.rs` with an explicit header that states: *"These are NOT known-answer tests against the C client"* (line 2). The placeholder test `algorithm_parameters_documented` (line 26) contains only comments; the C implementation is documented as *"TBD: obtain from upstream source analysis before claiming parity"* (line 35). The compat document (`crypto-compat.md:19-20`) reiterates that the format is NOT byte-compatible and the primitives used by the C client are "TBD".

**Impact:** Any claim of "pCloud Crypto folder compatibility" is unproven. Users with existing C-client-encrypted folders cannot access their data, and cross-client migration is impossible to validate. This is called out in the doc but remains unresolved.

**Recommendation:** Obtain a C-client ciphertext sample + known password + known plaintext and add an actual KAT decrypt test. Until then, do NOT claim cross-client parity in any docs, CLI help, or release notes.

---

## HIGH Findings

### HIGH-H.1 — Brute-force lockout / `consecutive_failures` field NOT implemented
**File:** `crates/pcloud-crypto/src/lib.rs:713-738` (`start`), full search of crate

`crypto-compat.md:58-59` claims: *"Brute-force lockout: after 5 consecutive wrong-password attempts the crypto layer refuses further unlock attempts until `reset()` is called."*

Grep for `consecutive_failures`, `lockout`, `5 consecutive`, `attempt` across the full crate returns no matching implementation. The `start` function (`lib.rs:713-738`) has no failure counter, no lockout logic, and no persisted counter. A caller can attempt unlimited `start()` calls with different passwords at Argon2id cost only. The documentation materially misrepresents the implementation.

**Impact:** Offline/online password-guessing attacks against a recovered profile are bounded only by Argon2id cost. Enterprise deployments that rely on lockout for defense-in-depth do not get it.

**Recommendation:** Either (a) implement `consecutive_failures: AtomicU32` on `KeyManager`, incremented on `WrongPassword` in `start()`, with a hard refusal when the counter reaches 5 (and documented reset semantics); or (b) remove the false claim from `crypto-compat.md`.

### HIGH-H.2 — `sectors_sealed: AtomicU64` counter NOT present on CryptoShell
**File:** `crates/pcloud-crypto/src/lib.rs:318-351` (`CryptoShell` struct), `content.rs:177-207` (`seal_sector`)

Grep across the crate for `sectors_sealed`, `AtomicU64`, or any atomic counter returns no hits. The `CryptoShell` struct has no counter field. `seal_sector` does not increment any counter. Without a counter there is also no rekey gate against the `2^32-sector` budget mentioned in `lib.rs:1097-1101`.

**Impact:** No runtime visibility into AEAD nonce-budget exhaustion; no data-driven rekey trigger. For AES-256-GCM with a 96-bit random nonce per-file, the safe budget is bounded, and without a counter the daemon cannot proactively rotate keys.

**Recommendation:** Add `sectors_sealed: AtomicU64`, increment inside `seal_sector` (both `CryptoShell::seal_sector` and `content::seal_sector`), and wire a warning threshold.

### HIGH-H.3 — NFC/Unicode normalization NOT applied before filename HMAC
**File:** `crates/pcloud-crypto/src/metadata.rs:90-108`

`crypto-compat.md:12` states: *"Filename encryption: deterministic `HMAC-SHA256(master_key, "pcloud-crypto/filename/v1" || NFC(name))`"*. The actual code (`metadata.rs:100`) passes `plaintext.as_bytes()` directly to the HMAC engine with no normalization. Grep for `unicode`, `normalize`, `NFC`, `NFKC`, or `unicode-normalization` across the crate and `Cargo.toml` returns zero hits.

**Impact:** Two clients that canonicalize filenames differently (e.g., macOS NFD vs Linux NFC) will produce different HMAC tags for the same visual name. Cross-platform folder sharing silently breaks. Also a documentation accuracy regression (the compat doc lies about the primitive).

**Recommendation:** Add the `unicode-normalization` crate to `Cargo.toml`, call `name.nfc().collect::<String>()` before `mac.update`, and add a test vector exercising both NFC and NFD forms.

### HIGH-H.4 — `crypto_util.rs` base64 consolidation is orphaned (not wired in)
**File:** `crates/pcloud-crypto/src/crypto_util.rs:1-31`, `lib.rs:51-90`, `Cargo.toml:9-21`

`crypto_util.rs` exists and uses `base64::Engine`, but:
- `lib.rs` has **no** `pub mod crypto_util;` declaration (searched lines 51-90 where sibling modules are declared).
- `Cargo.toml` has **no** `base64` dependency (lines 10-20 list `aes-gcm`, `argon2`, `getrandom`, `hmac`, `pcloud-kms`, `pcloud-secret`, `serde`, `sha2`, `subtle`, `thiserror`, `zeroize` — no `base64`).
- `share_temppass.rs:408-491` still ships its own hand-rolled `b64_encode` / `b64_decode`.
- `password_scorer.rs:577-607` still ships its own hand-rolled `base64_encode`.

So `crypto_util.rs` would not even compile if included (missing dep), and none of its functions are reachable. The claimed consolidation has not occurred.

**Impact:** Two ad-hoc base64 implementations are still live in the crypto crate. Each hand-rolled implementation is a potential parsing / timing surface. The "base64 consolidation" line-item of the audit is a false positive.

**Recommendation:** Either wire `crypto_util` in properly (add `base64` to `Cargo.toml`, add `mod crypto_util;` in `lib.rs`, replace both ad-hoc implementations, and delete the dead copies) or delete `crypto_util.rs` and document that consolidation was abandoned.

---

## MEDIUM Findings

### MEDIUM-M.1 — `MAX_ENCRYPTED_FILENAME_BYTES` not defined/enforced
**File:** `crates/pcloud-crypto/src/metadata.rs:90-108`

Grep for `MAX_ENCRYPTED_FILENAME_BYTES` or any plaintext-length cap returns no hits. `encrypt_filename` only rejects empty names and names containing `/` (line 94). A caller may pass a multi-megabyte "name" string; HMAC-SHA256 will accept it, but server-side constraints and wire framing then become the only guardrails.

**Recommendation:** Add a constant (e.g. 1024 bytes plaintext, or match server limits), reject over-length inputs with a distinct `MetadataCryptoError::NameTooLong`, and add a test vector.

### MEDIUM-M.2 — `PBKDF2_ITERS_LEGACY` constant not introduced
**File:** `crates/pcloud-crypto/src/password_scorer.rs:536`

Only `PBKDF2_ITERS: u32 = 5000` (line 536) exists. Grep for `PBKDF2_ITERS_LEGACY` returns zero hits. The audit request asked to verify the constant was renamed with a doc comment explaining its legacy status. It has not been.

**Impact:** Cosmetic + documentation: 5000 PBKDF2 iterations is very weak by 2026 standards but is mandated by server-side contract. Future readers have no inline signal that this is intentionally legacy.

**Recommendation:** Rename to `PBKDF2_ITERS_LEGACY` with a doc comment that says: *"5000 iters is server-mandated for the account API-password contract and is NOT the crypto-folder KDF (which uses Argon2id). Do not lower further without coordination."*

### MEDIUM-M.3 — `SamePassphrase` error not returned by temppass path
**File:** `crates/pcloud-crypto/src/share_temppass.rs:78-97` (`TemppassError` enum), `lib.rs:155-159`

`CryptoError::PasswordUnchanged` exists (`lib.rs:158`) and is returned by `change_password` (`lib.rs:942`) after a constant-time byte compare. There is **no** `SamePassphrase` variant (neither `CryptoError` nor `TemppassError`). The `derive_temppass_wire` function (`share_temppass.rs:288-341`) does not check whether the temppass equals the user's current master password — a weak operator choice would be silently accepted.

**Recommendation:** Decide whether this is intentional. If the audit item meant "verify `PasswordUnchanged` covers the rotate case" — it does. If it meant "verify temppass ≠ master password" — that check is missing; add it with a constant-time compare.

### MEDIUM-M.4 — `Zeroizing<Vec<u8>>` intermediate in `unwrap_active_dek` NOT present
**File:** `crates/pcloud-crypto/src/lib.rs:559-581`

`unwrap_active_dek` unwraps the DEK then does `dek.expose().to_vec()` on `lib.rs:1163` (inside `derive_sector_file_key`) which materializes a raw `Vec<u8>` not wrapped in `Zeroizing`. It is immediately moved into `SecretBytes::new(...)` which does zeroize on drop, so leakage is bounded to the duration of the `to_vec()` + constructor call. Still, grep for `Zeroizing` across the crate returns zero hits.

**Impact:** A brief window (microseconds) of unzeroized DEK copy in heap. Minor in practice.

**Recommendation:** Either wrap the intermediate as `zeroize::Zeroizing::new(dek.expose().to_vec())` or document that `SecretBytes::new` immediately takes ownership and the intermediate is bounded.

### MEDIUM-M.5 — Password rotation does NOT re-encrypt existing sectors; contract is "ciphertext invalidated"
**File:** `crates/pcloud-crypto/src/lib.rs:837-896` (`change_password_unlocked`), `tests/round_trip.rs:121-166`

`change_password_unlocked` rotates salt + master-key + fingerprint but does **not** re-encrypt any sector ciphertext. The round-trip test (`round_trip.rs:121-166`) asserts the opposite — that ciphertext sealed under key A becomes **unreadable** after rotation. This means password rotation is effectively a nuke: all pre-rotation content becomes garbage unless the daemon separately orchestrates a re-encryption pass.

Per-file keys are derived as `HMAC-SHA256(master, label || file_seed)`. There is **no KEK indirection** (master → KEK → per-file) that would survive master-key rotation. This is explicitly acknowledged in the test comment: *"Callers MUST complete a full re-encryption pass after rotation. See: bd-1du.10 for KEK-indirection architecture tracking."*

**Impact:** A user rotating their crypto password via the official path WILL lose access to all existing encrypted content unless a re-encryption pass is performed. The daemon path that orchestrates that pass is not visible in this audit's scope.

**Recommendation:** Architect KEK indirection: wrap a stable account-level DEK under the master-key-derived KEK, so rotating the master key only re-wraps one blob. Tracked under `bd-1du.10`; should be the top priority before any production enterprise claim.

### MEDIUM-M.6 — Temppass signature is HMAC-SHA256, NOT RSA (per-audit item 13)
**File:** `crates/pcloud-crypto/src/share_temppass.rs:209-233`, `42-45`

The `sign` method (line 213) uses `HMAC-SHA256` under the active master key, not RSA under a user private key. This is explicitly documented (`share_temppass.rs:42-45`): *"When RSA keypair mirroring lands under bd-1du.5, `TemppassBlob::sign` is the single place to swap to `prsa_sign_sha256_hash`."*

**Impact:** The signature proves *"blob came from a session with the current master key"* but not *"blob came from this user identity"*. The C client's RSA signature provides the stronger identity binding. A recipient who receives a temppass wire from an attacker who stole the master key cannot distinguish from a legitimate share.

**Status:** Documented trade-off tracked under `bd-1du.5`. Acceptable for MVP but MUST NOT be described as "parity with C temppass" in release material.

---

## LOW Findings

### LOW-L.1 — AAD endianness (big-endian) correctly documented
**File:** `crates/pcloud-crypto/src/content.rs:142-143, 191`; `docs/enterprise/crypto-compat.md:14-16`; `tests/round_trip.rs:32`

`content.rs:191` uses `sector_index.to_be_bytes()` (big-endian). The enterprise doc at `crypto-compat.md:14-16` correctly documents this with a specific note: *"an earlier version of this document incorrectly stated little-endian; the code is authoritative."* However, `tests/round_trip.rs:32` still says *"AAD: sector index (4-byte little-endian)"* — a stale comment.

**Recommendation:** Fix the stale comment in `round_trip.rs:32` to say big-endian. This is purely a docstring correction.

### LOW-L.2 — AES-256-GCM primitive choices are correct
**File:** `crates/pcloud-crypto/src/content.rs:177-207`, `249-270`

Verified:
- 12-byte random nonce from `getrandom` per sector (`content.rs:188-190`).
- 16-byte GCM tag via `Aes256Gcm::encrypt` (crate default).
- AAD = 4-byte big-endian sector index (`content.rs:191, 203`).
- Embedded index checked **before** AEAD call (`content.rs:258-262`) so the error taxonomy does not leak padding-oracle-style distinctions.
- Error variants opaque (`ContentCryptoError::InvalidFrame`, `AuthFailed`, `SectorIndexMismatch` — no byte-position info).

No finding; confirming correctness.

### LOW-L.3 — Constant-time comparisons using `subtle::ConstantTimeEq`
**File:** `crates/pcloud-crypto/src/keys.rs:199-206` (`matches_setup`), `lib.rs:934-944` (password-unchanged check), `share_temppass.rs:222-232` (blob signature verify)

All three call sites correctly use `ct_eq`. `matches_setup` checks the fingerprint constant-time. The password-unchanged check runs BEFORE Argon2id derivation so timing cannot leak which byte differs. `TemppassBlob::verify` also uses `ct_eq`. No finding.

### LOW-L.4 — `PlaintextDek` ZeroizeOnDrop correctly derived
**File:** `crates/pcloud-kms/src/lib.rs:120-122`

`PlaintextDek` has `#[derive(Zeroize)]` with `#[zeroize(drop)]` (lines 120-121), which is the semantic equivalent of `ZeroizeOnDrop` (the `drop` macro form is pre-v1.5 but still emits a `Drop` impl that zeroizes). Debug redacts bytes (`lib.rs:142-149`). No finding.

Note: modern `zeroize` prefers `#[derive(Zeroize, ZeroizeOnDrop)]` over `#[zeroize(drop)]`. Cosmetic modernization opportunity.

---

## Key Schedule / Derivation Chain Summary

Observed per-file derivation depth:
1. password → **Argon2id** (m=19456, t=2, p=1, 16-byte salt, 32-byte output) → `master_key`  (`keys.rs:154-160`)
2. `master_key` → **HMAC-SHA256**(`"pcloud-crypto/file-key/v1" || file_seed`) → `file_key`  (`content.rs:126-134`)
3. `(file_key, random_nonce, big-endian sector_index)` → AES-256-GCM ciphertext  (`content.rs:177-207`)

KMS mode: `master_key` is replaced by `KMS-unwrapped DEK` at step 2; other steps identical (`lib.rs:1144-1170`).

**Per-folder binding:** NONE. `file_seed` is caller-supplied and not bound to a folder id. A file seed reused in a different folder will produce the same `file_key`. This is called out as out-of-scope in `lib.rs:1096-1101` and is a higher-layer concern. Consider adding `folder_id` as an additional HMAC update in `derive_file_key` — would cost nothing and close the cross-folder replay surface.

---

## Recommended Action List (prioritized)

1. **CRITICAL:** Obtain C-client ciphertext sample and add a real decrypt KAT. Block any "parity" release claim on this.
2. **HIGH:** Implement `consecutive_failures` lockout OR remove the false claim from `crypto-compat.md`.
3. **HIGH:** Add `sectors_sealed: AtomicU64` + increment + rekey-threshold warning.
4. **HIGH:** Add NFC normalization to `encrypt_filename` and a cross-normalization test, OR correct `crypto-compat.md` to stop claiming `NFC()`.
5. **HIGH:** Wire `crypto_util.rs` in properly (add `base64` dep, `mod crypto_util`, delete hand-rolled base64 in `share_temppass.rs` and `password_scorer.rs`), OR delete the orphaned file.
6. **MEDIUM:** Architect KEK indirection so password rotation does not invalidate existing sectors.
7. **MEDIUM:** Introduce `MAX_ENCRYPTED_FILENAME_BYTES` with a dedicated error variant.
8. **MEDIUM:** Rename `PBKDF2_ITERS` → `PBKDF2_ITERS_LEGACY` + doc comment.
9. **MEDIUM:** Decide whether temppass must differ from master password; if so, add a constant-time check.
10. **LOW:** Fix stale little-endian comment in `tests/round_trip.rs:32`.
11. **LOW:** Modernize `#[zeroize(drop)]` → `#[derive(Zeroize, ZeroizeOnDrop)]`.

---

## Positive Observations

- `SecretBytes` / `SecretString` zeroize on drop and redact Debug. Enforced consistently (`lib.rs:981-989`, `content.rs:126-134`, `keys.rs:154-160`).
- `CryptoError::UnsafePolicy` gate fires before any key material is derived on every mutating path (setup, start, change_password*, enable_kms_mode).
- `unsafe_code` is forbidden at the crate level (`lib.rs:1`).
- Argon2id choice (memory-hard, m=19456) is strong for interactive unlock.
- KMS provider trait + process-local TTL cache + owner-level eviction on `stop()` are solid (`lib.rs:753-773`, `kms:244-294`).
- AWS/Vault/PKCS#11 providers are feature-gated with explicit `NotImplemented` stubs — misconfigured deployments fail loudly (`kms:304-335, 711-735`).
- Tests cover sector round-trip, AAD binding, cross-file seed isolation, password-rotation invalidation, tamper rejection, wrong-key rejection, and temppass round-trip (`round_trip.rs`, `share_temppass.rs` tests, `content.rs` tests).

End of report.
