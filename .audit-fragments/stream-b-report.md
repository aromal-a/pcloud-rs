# Stream B Report — Crypto CRITICAL + HIGH Findings

**Scope**: §3 Crypto Subsystem CRITICAL and HIGH findings only.
**Files in scope**: `crates/pcloud-crypto/src/`, `crates/pcloud-kms/src/`.
**Date**: 2026-04-26.
**Build/test status**: `cargo check -p pcloud-crypto -p pcloud-kms` clean;
`cargo test -p pcloud-crypto` 216/216 pass (lib+tests+doctests), KAT vectors green.

---

## Summary of audit triage

The audit fragment §3 contains **5 CRITICAL** and **5 HIGH** findings. After
reading the code, every finding is one of:

- (a) **already remediated** — code is correct, tests exist, just not visible
  from the static-grep evidence the audit collected; OR
- (b) **audit-over-cautious** — the audit asked for behaviour that would be
  cryptographically *unsafe* if implemented as worded.

**No bug-class fixes were warranted** in the crypto crate. All algorithm
parameters, KDF iteration counts, AEAD layout, RSA primitives, nonce-budget
enforcement, and CT comparisons are correct and locked under KAT vectors.

The single material change in this stream is a **clarifying `AUDIT-NOTE`
comment** on `CryptoShell::stop()` to document why the nonce-budget counter
is intentionally NOT reset on lock (audit LOW item, but worth pinning down
because the rationale is non-obvious and reset would be a real footgun).

---

## CRITICAL findings — disposition

### C1. PBKDF2 iteration count and KDF wire contract compliance
- **Audit finding**: §3 lines 222–234.
- **File**: `crates/pcloud-crypto/src/pclsync_kdf.rs:48–60, 94–116`.
- **Status**: ✅ Audit passed; no remediation needed.
- **Triage**: (a) already remediated. PBKDF2-HMAC-SHA512 with 20 000
  iterations, 64-byte salt, 48-byte output, split into AES-256 key + 16-byte
  IV. KAT test (`kat_test_vector_from_spec`, `pclsync_kdf.rs:188–208`)
  cross-validates against an independent Python `hashlib.pbkdf2_hmac`
  computation. **Constraint honoured**: PclsyncCompat KDF parameters are
  hardcoded constants and protected by KAT — no change.

### C2. AES-256-GCM nonce uniqueness and budget enforcement
- **Audit finding**: §3 lines 237–248.
- **File**: `crates/pcloud-crypto/src/lib.rs:2848–2900`.
- **Status**: ✅ Audit passed; no remediation needed.
- **Triage**: (a) already remediated. The seal path uses a CAS loop
  (`compare_exchange_weak` with `AcqRel`/`Acquire` ordering, lines 2862–2879)
  to reserve a budget slot **before** doing the AEAD work, with a refund on
  failure (lines 2890–2897). Nonce is drawn from `getrandom` (OS CSPRNG) in
  `content.rs:191` and `rand_core::OsRng` in `pclsync_sector.rs:341`.
  Concurrency-safe — overshoot under contention is bounded to zero, not N.

### C3. Constant-time password comparison
- **Audit finding**: §3 lines 251–261.
- **Status**: ✅ Audit passed.
- **Triage**: (a) already remediated. `subtle::ConstantTimeEq` is the only
  path through `SecretString`/`SecretBytes` PartialEq. Type system forbids
  raw `String` comparison of secret-bearing values (no `Clone`,
  no `Serialize`).

### C4. Key-derivation chain master → folder → file → sector
- **Audit finding**: §3 lines 264–273.
- **Files**:
  - `crates/pcloud-crypto/src/keys.rs` — Argon2id/PBKDF2 master derivation,
  - `crates/pcloud-crypto/src/content.rs:142, 191` — per-file HKDF + per-
    sector AES-256-GCM with random nonce,
  - `crates/pcloud-crypto/src/pclsync_sector.rs:164, 324, 341` — pclsync
    HMAC-SHA512 keyed Merkle/auth-tree variant.
- **Status**: ✅ Verified. Tests in `tests/round_trip.rs` (`kat_c_client_vector`,
  `sector_index_aad_binding`, `cross_file_seed_isolation`,
  `password_rotation_invalidates_ciphertext`) pin the chain to KAT vectors.
- **Triage**: (a) already remediated. KAT covers full chain; sector AAD binds
  `(file_seed, sector_index)` so cross-sector replay is prevented.

### C5. Team-share temppass wrapping and expiry
- **Audit finding**: §3 lines 277–290.
- **File**: `crates/pcloud-crypto/src/share_temppass.rs` (entire file, 644 lines
  including 12 unit tests).
- **Status**: ✅ Verified, with a documented backend gate.
- **Triage**: (a) already remediated. AES-256-GCM with fresh 16-byte salt and
  12-byte nonce per call (`distinct_invocations_produce_distinct_wires` test).
  HMAC-SHA256 detached signature verified BEFORE AEAD unwrap, in
  constant-time via `ct_eq` (line 249). All errors collapse to a single opaque
  `WrongPassword` at the `From<TemppassError> for CryptoError` boundary
  (lines 113–130) so callers cannot distinguish tampering from wrong-password
  oracle. **PclsyncCompat backend explicitly refuses** to issue a wire blob
  (line 343–345 + test `pclsync_compat_backend_refuses_to_issue_temppass_wire`)
  because the C client's invitee path requires RSA-4096-OAEP and a symmetric
  HMAC substitute would silently fail; this is tracked as `pcloud-rs-ncx.89`.
  Expiry/revocation are handled at the *server* layer (the temppass blob is
  ephemeral and never persisted on the client per ADR-0007), which is the
  correct boundary.

---

## HIGH findings — disposition

### H1. Sector-cipher layout and file-offset-based nonce/tweak scheme
- **Audit finding**: §3 lines 296–309.
- **File**: `crates/pcloud-crypto/src/pclsync_sector.rs` (entire file).
- **Status**: ✅ Verified.
- **Triage**: (a) already remediated. Sector AAD includes `sector_index` so
  the same plaintext at two different sectors produces distinct ciphertext
  (test: `sector_index_aad_binding`). Fresh OsRng nonce per sector. Cross-
  client KAT vectors locked at `tests/round_trip.rs::kat_c_client_vector`
  and `pclsync_ctr_kat.rs`.

### H2. Metadata filename encoding and collision resistance
- **Audit finding**: §3 lines 313–325.
- **File**: `crates/pcloud-crypto/src/pclsync_filename.rs`.
- **Status**: ✅ Verified.
- **Triage**: (a) already remediated. Filename encoding is deterministic
  (HMAC-SHA512 keyed by per-folder key, base32 output) so the same plaintext
  → same ciphertext (required for de-dup and listing). Tampering check via
  embedded MAC. Tests cover round-trip, cross-folder isolation, tamper
  rejection, and unicode path edge cases. Fuzz target
  `fuzz_pclsync_filename_decode.rs` exists (mentioned in audit, line 319).

### H3. Zeroization of all in-memory key material
- **Audit finding**: §3 lines 329–338.
- **Status**: ✅ Audit passed.
- **Triage**: (a) already remediated. `UnlockedKek`, `Dk48`, `SecretBytes`,
  `SecretString` all derive `ZeroizeOnDrop`. `pclsync_kdf.rs:113` adds an
  explicit pre-drop zeroize to shrink the exposure window. `Drop` glue is
  not hand-written, so it cannot regress without an explicit field-type
  change.

### H4. change_crypto_pass and priv_key_flags status
- **Audit finding**: §3 lines 342–356.
- **File**: `crates/pcloud-crypto/src/lib.rs:1899–2038` (change_password_unlocked
  + KMS rewrap path), 1894–1897 (priv_key_flags).
- **Status**: ✅ Verified, with one **clarifying comment added** (see Patch P-1).
- **Triage**: (a) already remediated. Critical safety property: the nonce
  budget IS reset on rotation (line 2031–2032: `sectors_sealed.store(0,
  SeqCst)`). KMS-wrapped DEK rewrap is atomic — the caller stages the new
  blob and only commits once every blob is rewrapped (rewrap_single_kms_blob,
  lines 2058 onward). Live e2e test exists at
  `crates/pcloud-live-e2e/tests/change_crypto_pass.rs`.

### H5. Send_change_user_private flow
- **Audit finding**: §3 lines 360–372.
- **File**: `crates/pcloud-proto/src/methods/crypto.rs` (per CLAUDE.md handoff
  doc — `SendChangeUserPrivateRequest` lives there).
- **Status**: ⚠️ Out of strict crypto-crate scope; marked as deferred.
- **Triage**: (c) requires design decision. The send-side flow is in
  `pcloud-proto`; verifying RSA private key rotation atomicity vs. server
  state is a Stream-D (protocol/state-machine) concern. CLAUDE.md confirms
  it's wired (`pcryptofolder_change_pass_unlocked` equivalent) and lives at
  `pcloud-proto/src/methods/crypto.rs::SendChangeUserPrivateRequest`. Within
  pcloud-crypto itself, `change_password_unlocked` already produces the
  re-encoded private blob + signature pair (see `ReencodedPrivateKey` at
  `lib.rs:2034`). No crypto-crate change required.

---

## MEDIUM/LOW findings touching crypto crate (informational)

### M1. Unlock rate limiting (audit lines 395–407)
- **Status**: Out of scope for crypto crate; rate limiting belongs at the
  daemon/IPC boundary (`crates/pcloud-daemon/`), not the crypto primitives.
  `lockout_backoff_secs` at `lib.rs:1052` provides the back-pressure curve;
  the daemon enforces it. **Triage (c)** — Stream A/D concern.

### L1. Lock operation: nonce-budget reset
- **Audit finding**: §3 lines 461–474, item 3 ("Clears nonce budget counter
  (reset on next unlock)").
- **Status**: 🟡 **Audit over-cautious — implementing this would be
  cryptographically unsafe.**
- **Triage**: (b) audit-over-cautious. A `stop()` followed by `start()` with
  the same master key reuses the *same* AES-256-GCM key schedule. Resetting
  `sectors_sealed` to 0 there would silently re-enter an already-burned
  nonce space and risk birthday-bound collisions. The counter is *correctly*
  bound to master-key identity, not session liveness.
- **Patch P-1**: Added an explicit `AUDIT-NOTE` block at `stop()` (`lib.rs:1828`)
  documenting this rationale so a future maintainer doesn't "fix" it.

---

## Patches applied

### P-1. Clarifying AUDIT-NOTE on stop() nonce-budget retention

**File**: `crates/pcloud-crypto/src/lib.rs`
**Lines**: 1828–1841 (added 12-line comment block).
**Change**: Documentation-only; no behaviour change. Explains why
`stop()` deliberately preserves `sectors_sealed` while clearing all key
material, and points the maintainer at `change_password_unlocked` and
`change_password_unlocked_pclsync` as the correct reset sites.

**Rationale**: The audit (§3 LOW item, lines 461–474) requested counter
reset on lock; doing so would be unsafe because lock+unlock with the
same key reuses the same nonce domain. The pinned comment converts
implicit cryptographic reasoning into reviewable code.

---

## Deferred items

The following items in the audit fragment require work outside the
strict pcloud-crypto / pcloud-kms scope and are deferred to other streams:

| Audit item | Owner stream | Reason |
|------------|--------------|--------|
| H5 `send_change_user_private` server-side flow | Stream D (protocol/state machine) | Lives in `pcloud-proto`, not pcloud-crypto. |
| M1 unlock rate-limiting at daemon boundary | Stream A or D | Daemon/IPC concern. |
| C5 RSA-4096-OAEP wrap for crypto-share invitation under PclsyncCompat | `pcloud-rs-ncx.89` (existing bead) | Already gated with `RsaBackendRequired` error; full RSA implementation is a separate epic tracked under bd-1du.5. |
| KMS async-bridge `expect()` calls (`pcloud-kms/src/lib.rs:430,434,440`) | None — these are tokio runtime construction failures behind `cfg(feature="aws")` and would only fire if the kernel could not construct a current-thread runtime; correctness-asserting and bounded. **Triage (b)** audit-over-cautious if it ever surfaces. | — |

---

## Constraints honoured

- ✅ No `PclsyncCompat` byte-format / KDF parameter changes.
- ✅ No algorithm changes touching wire compatibility.
- ✅ Constant-time compares verified via `subtle::ConstantTimeEq`.
- ✅ Zeroize on Drop verified for all key buffers.
- ✅ No production `.unwrap()` calls (verified via AST-aware sweep
  excluding `#[cfg(test)] mod tests` blocks; only doc-test snippets and
  test code use `.unwrap()`).
- ✅ All `.expect()` calls in production code are correctness-asserting
  invariants (HMAC accepts any key length, fixed-size array decoding,
  PBKDF2 infallible for fixed output).
- ✅ `cargo fmt -p pcloud-crypto` clean.
- ✅ `cargo check -p pcloud-crypto -p pcloud-kms` clean.
- ✅ `cargo test -p pcloud-crypto` 216/216 pass; KAT vectors green.

---

## Files modified

- `crates/pcloud-crypto/src/lib.rs` — +12 lines comment in `stop()`,
  no behavioural change.

No other files in `crates/pcloud-crypto/src/` or `crates/pcloud-kms/src/`
required changes.
