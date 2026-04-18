# Section 3. Crypto Subsystem

**Scope.** This dimension audits cryptographic correctness, algorithm fidelity vs the legacy pCloud C client, key schedule, nonce discipline, lifecycle, team-share temppass, KMS wiring, zeroization, constant-time comparisons, and dependency posture of the `pcloud-crypto` crate and the `pcloud-kms` crate, along with how the daemon/runtime drive them via IPC.

**Auditor:** parallel Dimension 3 specialist (non-FIPS, non-parity-accounting).

**Files audited (exhaustive list):**
- `crates/pcloud-crypto/Cargo.toml` (32 lines)
- `crates/pcloud-crypto/src/lib.rs` (1508 lines) — `CryptoShell`, lifecycle, sector wrappers, change-password, KMS routing.
- `crates/pcloud-crypto/src/content.rs` (328 lines) — AES-256-GCM sector AEAD, per-file key derivation.
- `crates/pcloud-crypto/src/keys.rs` (207 lines) — Argon2id master-key derivation, setup fingerprint.
- `crates/pcloud-crypto/src/metadata.rs` (149 lines) — deterministic filename encoding.
- `crates/pcloud-crypto/src/password_scorer.rs` (874 lines) — password scorer + PBKDF2-HMAC-SHA512 passphrase→API-password derivation.
- `crates/pcloud-crypto/src/policy.rs` (101 lines) — policy gates for master-key persistence.
- `crates/pcloud-crypto/src/share_temppass.rs` (647 lines) — crypto-folder share temppass wrap/unwrap.
- `crates/pcloud-crypto/src/state.rs` (77 lines) — lifecycle state machine.
- `crates/pcloud-crypto/tests/integration.rs` (135 lines).
- `crates/pcloud-crypto/tests/kms_routing.rs` (336 lines).
- `crates/pcloud-crypto/tests/proptest_seal.rs` (93 lines).
- `crates/pcloud-crypto/benches/aead_sector.rs` (67 lines).
- `crates/pcloud-crypto/vendored/password_dict.rs` (build-time-generated, non-secret).
- `crates/pcloud-kms/src/lib.rs` (1331 lines) — `KmsProvider` trait, `NullKms`, AWS KMS, HashiCorp Vault, PKCS#11 HSM (feature-gated), process-local plaintext-DEK cache.
- Daemon wiring (read-only context, not the primary focus of this dimension):
  `crates/pcloud-daemon/src/runtime.rs` (`unlock_crypto`, `setup_crypto`, `lock_crypto`, `crypto_reset`, `change_crypto_password`, `change_crypto_password_unlocked`, `crypto_priv_key_flags`, `send_crypto_change_user_private`, `upload_reencoded_private_key`).
  `crates/pcloud-ipc/src/lib.rs` (Request/Method variants: `CryptoUnlock`, `CryptoSetup`, `CryptoChangePassword`, `CryptoChangePasswordUnlocked`, `CryptoMkdir`, `LockCrypto`, `CryptoReset`, `GetCryptoStatus`, `GetCryptoPrivKeyFlags`, `SendCryptoChangeUserPrivate`, `GetCryptoHint`).

**Workspace crypto dependency pins (from `Cargo.toml` + `Cargo.lock`):**
- `aes-gcm = "0.10.3"` (default-features off, `aes + alloc`) — RustCrypto, actively maintained.
- `argon2 = "0.5.3"` — RustCrypto.
- `getrandom = "0.2.17"` primary (also `0.3.4` and `0.4.2` transitively via `rand`).
- `hmac = "0.12.1"` — RustCrypto.
- `sha2 = "0.10.9"` — RustCrypto.
- `subtle = "2.6.1"` — RustCrypto (constant-time primitives).
- `zeroize = "1.8.2"` with `zeroize_derive` — RustCrypto.
- `#![forbid(unsafe_code)]` at `crates/pcloud-crypto/src/lib.rs:1`; zero `unsafe` blocks in the crate (confirmed by grep).

The rest of this report is organised as per the audit prompt's 13 focus areas, then a severity-ranked findings ledger (CRITICAL/HIGH/MEDIUM/LOW), then a remediation summary.

---

## 1. Algorithm fidelity vs legacy C client

### 1.1 What CLAUDE.md claims

From `CLAUDE.md` → "Crypto parity progress":

> Implemented on the active Rust path:
> - setup/start/stop/reset,
> - lock/unlock lifecycle,
> - crypto folder creation,
> - AES-256-GCM sector encryption,
> - deterministic metadata filename encoding,
> - zeroized key handling via `SecretBytes` / `SecretString`,
> - password rotation helpers,
> - fingerprint verification and reset paths,
> - active daemon/IPC/SDK crypto control surfaces.
> - crypto-aware share/team-share temppass flow.
>
> Still missing:
> - `change_crypto_pass` family,
> - `send_change_user_private`,
> - `priv_key_flags`.

### 1.2 What the code actually implements

The Rust `pcloud-crypto` crate is **NOT** a byte-level port of the C `pclsync/pcryptofolder.c` wire format. It is a **re-implementation with the same shape** but with different primitives, different on-disk persistence, and no byte-identical interoperability guarantee. The code itself is explicit about this — see the doc block at `crates/pcloud-crypto/src/share_temppass.rs:39-46`:

> The active Rust crypto path (see `crate::keys::KeyManager`) does not yet store an RSA-4096 keypair in the form the C client expects, so the "signature" produced here is an HMAC-SHA256 tag under the active master key rather than an RSA signature under the user private key.

Concretely the Rust path differs from the upstream C client as follows:

| Surface | Legacy C (`pclsync/pcryptofolder.c`, `pcryptofolder.h`) | Rust (`pcloud-crypto`) |
|---|---|---|
| Master key | Per-user RSA-4096 private key wrapped by a master passphrase using AES-CTR + separate SHA signature; generated on enrolment and persisted server-side | 32-byte symmetric Argon2id output kept in `SecretBytes`, never persisted; no RSA keypair at all |
| Sector AEAD | AES-CTR + HMAC / SHA-based MAC (legacy composed construction) | **Single-pass AES-256-GCM** (AEAD) with 12-byte nonce from `OsRng` |
| Per-file key | Derived from the RSA-wrapped symmetric key | `HMAC-SHA256(master, "pcloud-crypto/file-key/v1" || file_seed)` — see `content.rs:127` |
| Nonce | C uses a counter-style IV seeded from file metadata | 96-bit **random** nonce from `getrandom()` — see `content.rs:188-190` |
| Filename encoding | C uses AES-CBC-encrypted filename blobs | `HMAC-SHA256(master, "pcloud-crypto/filename/v1" || plaintext)` then hex — see `metadata.rs:90-108` |
| Fingerprint / unlock gate | C derives the master key on every unlock and attempts to decrypt an RSA-wrapped test blob | Rust stores `HMAC-SHA256(derived, "pcloud-crypto/fingerprint/v1")` as a non-secret 32-byte check tag — see `keys.rs:178-185` |
| Password rotation | C reuses the user's RSA key, re-wraps it under the new password, uploads `privenc + sign` | Rust emits a version-tagged `"pcrypto/v1/" || hex(salt) || "/" || hex(fingerprint) || "/" || hex(flags_le)` blob signed with `HMAC-SHA256(current_master)` — see `lib.rs:874-896` |
| Team-share temppass | C re-wraps the RSA private key under Argon2-from-temppass, signs with `prsa_sign_sha256_hash` | Rust wraps the current 32-byte master key under `AES-256-GCM(kek = Argon2id(temppass, 16B_salt))` and signs with `HMAC-SHA256(master)` — see `share_temppass.rs:288-341` |

### 1.3 Finding

This is **MUCH STRONGER CRYPTO** than legacy C for single-device scenarios, and it is clearly documented as such. However, it is **NOT** the "active crypto on the retained C path" — it is a re-design. The CLAUDE.md phrasing "crypto is active on the retained Rust path" is truthful for the *rewrite*, but an auditor who reads the parity matrix and expects byte-level interop with the upstream pCloud server's encrypted-folder ciphertext will be mistaken.

See CRITICAL-3.A below — there are **no cross-client KAT (known-answer test) vectors** proving that a ciphertext produced by the Rust crate can be decrypted by a real upstream pCloud C client. The share temppass module flags this under bd-1du.5 at `share_temppass.rs:44-45`, but no equivalent caveat exists for *content* sectors, filenames, or the setup fingerprint.

---

## 2. Key schedule

### 2.1 Master-key derivation — `crates/pcloud-crypto/src/keys.rs:134-160`

```rust
pub fn derive_key_material_with_salt(password: &SecretString, salt: &[u8]) -> SecretBytes {
    let mut derived = vec![0u8; DERIVED_KEY_LEN];            // 32 bytes
    Argon2::default()
        .hash_password_into(password.expose_secret().as_bytes(), salt, &mut derived)
        .expect("fixed argon2 output length should be valid");
    SecretBytes::new(derived)
}
```

- Primitive: **Argon2id** via `argon2` crate defaults.
- `argon2 = "0.5.3"` `Argon2::default()` resolves to **`m = 19456` KiB (~19 MiB), `t = 2`, `p = 1`** (crate source: OWASP-recommended 2022 preset).
- Output: **32 bytes** (`DERIVED_KEY_LEN`).
- Salt: **16 bytes** per-profile, generated once on `KeyManager::default()` via `getrandom()` — `keys.rs:88-89`.
- Password wrapped in `SecretString` (zeroize on drop). Output wrapped in `SecretBytes`. Input `password` is borrowed.

### 2.2 Fingerprint — `crates/pcloud-crypto/src/keys.rs:178-185`

`HMAC-SHA256(derived_key, "pcloud-crypto/fingerprint/v1")` → 32 bytes non-secret.

### 2.3 Per-file key — `crates/pcloud-crypto/src/content.rs:126-134`

`HMAC-SHA256(master, "pcloud-crypto/file-key/v1" || file_seed)` → 32 bytes in `SecretBytes`.

### 2.4 Per-sector key — same key as per-file, **no per-sector key**

The sector layer uses a single per-file 32-byte key and distinguishes sectors **only via the 4-byte big-endian sector index bound as AAD** (`content.rs:191`). There is no sector-level subkey schedule.

### 2.5 Findings

- **MEDIUM-3.B (no separate per-sector key).** Rotating nonces is the sole protection against within-file key reuse. At 96-bit random nonces, expected collision is at ~2⁴⁸ sectors. The doc at `lib.rs:1096-1101` acknowledges this and says "sector-level rekey is expected every 2^32 sectors on the enterprise path but is not enforced here; the daemon owns the rekey schedule". **The daemon does NOT currently enforce any such rekey schedule** (confirmed by grep for `rekey` across `crates/pcloud-daemon/src/`). Remediation: either add a real sector-rekey hook at the daemon or swap to AES-GCM-SIV / XChaCha20-Poly1305 where nonce collisions are safer.
- **HIGH-3.C (Argon2id parameter divergence is UNTESTED against the C client).** The C client's key-stretching parameters come from `pclsync/pssl.c:psymkey_derive`, which is **PBKDF2-HMAC-SHA-512, 5000 iterations** (see the doc at `password_scorer.rs:536-538`). That is the *account API password* derivation — a different code path. The *master-key derivation* on the C side is in `pclsync/pcryptofolder.c` and uses the historical pCloud-defined KDF (not Argon2). The Rust side does Argon2id for the crypto-folder master key. These **do not interoperate**: a Rust client cannot read a legacy-C encrypted folder, and vice versa. Mark this as CRITICAL if the product claim is "drop-in replacement" for an existing C-enrolled user; mark as MEDIUM if the product is a greenfield migration path. CLAUDE.md currently forbids the "drop-in replacement" claim (see `CLAUDE.md` "Do not claim"), which is the right posture — this finding is then **HIGH** only in that the matrix row should explicitly say "not byte-compatible; new enrolment required".

---

## 3. Nonce generation

### 3.1 Sector AEAD — `crates/pcloud-crypto/src/content.rs:186-206`

```rust
let mut nonce_bytes = [0u8; NONCE_LEN];                 // 12 bytes
getrandom(&mut nonce_bytes).expect("OS randomness must be available");
let nonce = Nonce::from_slice(&nonce_bytes);
let aad = sector_index.to_be_bytes();
```

- **Random 96-bit nonce** from `getrandom` (OS CSPRNG). Not counter-derived. Not offset-derived.
- On OS CSPRNG failure, the function panics via `.expect(...)`. The doc at `content.rs:173-176` marks this as an "unrecoverable host fault". This is defensible on Linux/macOS where `getrandom(2)` only fails on misconfigured kernels, but there is **no fallback** on embedded Rust targets.

### 3.2 Share temppass — `crates/pcloud-crypto/src/share_temppass.rs:302-305`

```rust
let mut salt  = [0u8; TEMPPASS_SALT_LEN];   // 16 bytes
let mut nonce = [0u8; TEMPPASS_NONCE_LEN];  // 12 bytes
getrandom(&mut salt).map_err(|_| TemppassError::Malformed)?;
getrandom(&mut nonce).map_err(|_| TemppassError::Malformed)?;
```

Both salt and nonce are freshly drawn from the OS CSPRNG on every call. Property test `distinct_invocations_produce_distinct_wires` at `share_temppass.rs:591-599` asserts freshness.

### 3.3 KMS-wrapped DEK generation — `crates/pcloud-crypto/src/lib.rs:537-545`

```rust
let mut dek_bytes = vec![0u8; KMS_DEK_LEN];             // 32 bytes
getrandom::getrandom(&mut dek_bytes)
    .expect("OS randomness should be available for DEK generation");
let dek = pcloud_kms::PlaintextDek(dek_bytes);
```

DEK drawn from OS CSPRNG once at `enable_kms_mode` time; wrapped blob persisted inside `CryptoShell::mode = CryptoMode::Kms`.

### 3.4 PKCS#11 AES-GCM IV — `crates/pcloud-kms/src/lib.rs:962-964`

```rust
let mut iv = [0u8; 12];
getrandom::getrandom(&mut iv)
    .map_err(|e: getrandom::Error| KmsError::Other(e.to_string()))?;
```

12-byte IV from OS CSPRNG.

### 3.5 Findings

- **GOOD:** Every nonce/IV path uses `getrandom` (OS CSPRNG) — no `SmallRng`, no thread-local PRNG, no counter derivation. The audit prompt's CRITICAL check ("(key, nonce) reuse reachable" under a weak RNG) is **not reachable** under normal host configuration.
- **LOW-3.D (error discipline divergence).** Sector AEAD `seal_sector` **panics** on `getrandom` failure (`content.rs:189`); share temppass **returns `Malformed`** on the same failure (`share_temppass.rs:304-305`); KMS DEK **panics** (`lib.rs:540`); PKCS#11 IV **returns `Other`** (`kms/lib.rs:964`). The two panic sites are OK because `getrandom` on Linux only fails if the kernel is too old for `getrandom(2)`, but the inconsistency hurts readability and auditability. Remediation: pick one policy (prefer "propagate as error") and apply uniformly.
- **MEDIUM-3.E (random 96-bit nonce collision bound).** With random nonces, the AEAD birthday bound is ~2⁴⁸ sectors at 2⁻³² collision probability. At the 4 KiB sector size this is 2⁴⁸ × 4 KiB ≈ 1 EB of data per key — not reachable today but not future-proof. The code doc (`lib.rs:1096-1101`) acknowledges a sector-rekey schedule is needed on the enterprise path but it is not enforced. Consider AES-GCM-SIV (`aes-gcm-siv` crate) for enterprise mode, which is nonce-misuse-resistant.

---

## 4. Fingerprints & reset

### 4.1 Fingerprint check — `crates/pcloud-crypto/src/keys.rs:199-206`

```rust
pub fn matches_setup(&self, key: &SecretBytes) -> bool {
    let Some(stored) = self.setup_fingerprint.as_ref() else { return false; };
    let computed = Self::fingerprint_for(key);
    computed.0.ct_eq(&stored.0).into()
}
```

**GOOD:** constant-time comparison via `subtle::ConstantTimeEq`.

### 4.2 Wrong-password path — `crates/pcloud-crypto/src/lib.rs:727-738`

```rust
self.unlock_state = state::UnlockState::Unlocking;
let derived = self.keys.derive_key_material(&password);
if !self.keys.matches_setup(&derived) {
    drop(derived);
    self.unlock_state = state::UnlockState::Locked;
    return Err(CryptoError::WrongPassword);
}
self.keys.active_key_material = Some(derived);
```

- **GOOD:** derived material dropped (zeroized) on wrong-password.
- **GOOD:** `UnlockState` transitions back to `Locked` and never reveals partial `Unlocked` state.

### 4.3 Rate-limit / lockout

- **HIGH-3.F (no wrong-password rate-limit / lockout at the crypto layer).** Nothing in `pcloud-crypto` rate-limits brute-force unlock attempts. The daemon handler at `crates/pcloud-daemon/src/runtime.rs:2533-2564` (`unlock_crypto`) calls `self.crypto.start(secret)` directly and returns `Unauthorized` on failure. No counter, no exponential backoff, no lockout. An IPC client (owner-only, but still a local attack surface) can call `unlock_crypto` in a tight loop. At Argon2id default cost (~200 ms per attempt on a laptop CPU) this bounds practical online guessing to ~5 attempts/second, which is better than nothing but is not the "enterprise ready" posture CLAUDE.md gestures at.
- Remediation: track consecutive-failure count in `KeyManager` and require a backoff delay or transient lockout. Keep the backoff constant-time to avoid leaking whether the shell is locked vs mid-unlock.

### 4.4 Reset path — `crates/pcloud-crypto/src/lib.rs:1005-1013`

```rust
pub fn reset(&mut self) {
    self.stop();
    self.keys.setup_fingerprint = None;
    self.folders.clear();
    self.next_local_folder_id = 1;
    self.hint = None;
    self.mode = CryptoMode::Raw;
    self.unlock_state = state::UnlockState::NotSetup;
}
```

- **GOOD:** `stop()` first (drops+zeroizes active key material, evicts KMS cache). Then fingerprint is zeroed, mode reverts to Raw.
- **MEDIUM-3.G (recovery code flow is at the daemon only).** The C client exposes a recovery-code path; in Rust the recovery code is enforced at `runtime.rs:2714-2720` / `2771-2776` as an IPC-level non-empty string. There is no cryptographic binding between the recovery code and the reset operation at the `CryptoShell` level. The daemon forwards to the backend (`upload_reencoded_private_key` at `runtime.rs:2814-2842`), which has the final say. If a future refactor drops the IPC-level check, `CryptoShell::reset()` has no safeguard of its own. Consider adding a `require_recovery_proof: bool` policy bit.

---

## 5. Rotation (`change_crypto_pass` family)

CLAUDE.md marks this family as **"Still missing"**. **This is wrong as of the code I read.**

### 5.1 Actual implementation — `crates/pcloud-crypto/src/lib.rs:837-967`

Two functions are live:

- `CryptoShell::change_password_unlocked(new_password, flags) -> ReencodedPrivateKey` (`lib.rs:837-896`)
- `CryptoShell::change_password(old_password, new_password, flags) -> ReencodedPrivateKey` (`lib.rs:914-967`)

Both:

1. Verify policy (`policy.is_safe()` — rejects if `persist_master_key == true`).
2. Constant-time byte-compare old vs new passwords (`change_password` only — `lib.rs:934-944`).
3. Derive new key material under a **freshly-rotated 16-byte salt** (`lib.rs:858-862`).
4. Emit a version-tagged blob `pcrypto/v1/<salt_hex>/<fingerprint_hex>/<flags_le_hex>` + HMAC-SHA256 signature keyed by the **old** master.
5. Install the new salt + new fingerprint + new flags + new active master key.

The daemon wires this via `change_crypto_password` and `change_crypto_password_unlocked` (`runtime.rs:2701-2812`), and uploads the rekeyed blob to the backend via `crypto_runtime.change_user_private(...)` (`runtime.rs:2822-2828`).

### 5.2 Findings

- **HIGH-3.H (CLAUDE.md is out of date).** This is a documentation/parity-matrix drift, not a code defect. The Rust crate does implement `change_crypto_pass{_unlocked}` with stronger primitives than C (constant-time old-vs-new check, fresh salt on every rotation, HMAC-SHA256 signature under the old master, explicit version tag for forward-compat). The CLAUDE.md "Still missing" list must be corrected or this creates a false audit signal.
- **HIGH-3.I (no re-encryption of existing content on rotation).** Because the Rust master key is used as the **HMAC key for per-file key derivation**, rotating the master key **invalidates every existing per-file key**. The C client re-encrypts the RSA-wrapped DEK, leaving per-file AES keys unchanged. The Rust design does **not** re-encrypt any existing ciphertext on rotation — old sector frames are permanently unreadable after a rotation. No test and no doc currently warns about this. **This is a real-world data-loss trap.** Remediation: either (a) introduce a KEK-of-master-key layer so per-file keys stay stable across master rotations, or (b) document clearly and add an integration test that rotates the password and then proves old sector frames no longer decrypt, so callers understand the invariant.
- **MEDIUM-3.J (no binding of `ReencodedPrivateKey.private_key_hex` to the user identity).** The blob `pcrypto/v1/<salt>/<fp>/<flags>` does not carry a user id or account id. If a server accepts any blob signed under any master-known-to-the-session, an operator error could cross-account the rotation blob. Remediation: include a 64-bit account id (or a user-identity HMAC slot) inside the versioned blob before signing.
- **LOW-3.K (`change_password_unlocked` deliberately skips the "identical password" check).** Documented at `lib.rs:864-869` — because the salt is rotated, the new key will differ from the old even for identical passwords, so the check is moot at the derived-key layer. Callers who want "reject identical password" must use `change_password`, not `change_password_unlocked`. Defensible, but the IPC handler at `runtime.rs:2758-2812` exposes the unlocked variant directly — an IPC client can reset to the same passphrase silently. Mark as LOW since the C client had the same property.

---

## 6. `send_change_user_private` and `priv_key_flags`

CLAUDE.md marks both as missing. **Both are wrong.**

### 6.1 `priv_key_flags` — `crates/pcloud-crypto/src/lib.rs:814-817`

```rust
pub fn priv_key_flags(&self) -> u64 {
    self.keys.private_flags
}
```

Backed by `KeyManager::private_flags: u64` (`keys.rs:71-72`) with `PRIV_KEY_FLAG_TEMP_PASS = 1` (`keys.rs:84`) matching the C `PSYNC_CRYPTO_FLAG_TEMP_PASS`. Daemon IPC handler at `runtime.rs:2658-2663` (`GetCryptoPrivKeyFlags`). Tested at `lib.rs:1367-1370`.

### 6.2 `send_change_user_private` — `crates/pcloud-daemon/src/runtime.rs:2667-2698`

```rust
fn send_crypto_change_user_private(&mut self) -> Response {
    // ... auth token check ...
    match self.crypto_runtime.send_change_user_private(auth_token.expose_secret()) { ... }
}
```

Wired to IPC method `SendCryptoChangeUserPrivate`. Backed by `CryptoRuntime` in `crates/pcloud-daemon/src/crypto_backend.rs` (I did not deep-read this file in this audit because it is outside the `pcloud-crypto` / `pcloud-kms` scope of Dimension 3, but the method exists and is reachable).

### 6.3 Finding

- **HIGH-3.L (CLAUDE.md drift, repeat).** Same class as HIGH-3.H. Both features exist; the handoff doc must be corrected or Dimension 1 (parity accounting) will double-count this gap.

---

## 7. Team-share temppass (`crates/pcloud-crypto/src/share_temppass.rs`)

### 7.1 Wrap flow — `share_temppass.rs:288-341`

1. Validate shell is unlocked (borrows `master` without cloning).
2. Fresh 16-byte salt + 12-byte nonce from OS CSPRNG.
3. `kek = Argon2id(temppass, salt)` → 32 bytes in `SecretBytes`.
4. `ct = AES-256-GCM(kek, nonce, aad = "pcloud-crypto/share-temppass/aad/v1", msg = master.expose_secret())`.
5. `sig = HMAC-SHA256(master, "pcloud-crypto/share-temppass/sig/v1" || blob_encoded)`.
6. Emit both as base64.

### 7.2 Unwrap flow — `share_temppass.rs:377-403`

1. Base64-decode both blobs.
2. `TemppassBlob::verify(verifier_master, signature)` — **HMAC-SHA256 verified with `ct_eq` BEFORE any AEAD unwrap** (`share_temppass.rs:222-232`). Good.
3. Re-derive `kek` from temppass + embedded salt.
4. `AES-256-GCM-Open(kek, nonce, aad = fixed, ct)`.
5. Return recovered master as `SecretBytes`.

### 7.3 Findings

- **GOOD:** constant-time signature verification (`share_temppass.rs:227`). Tamper path collapses to single opaque `BadSignature` error so a caller cannot distinguish.
- **GOOD:** 16-byte salt + 12-byte nonce freshly drawn every call; property-tested at `share_temppass.rs:591-599`.
- **GOOD:** `Debug` impl redacts ciphertext (`share_temppass.rs:165-173`).
- **GOOD:** no Clone on `TemppassBlob`.
- **MEDIUM-3.M (HMAC signature is not a cryptographic proof of identity).** The module itself documents this at `share_temppass.rs:38-45` — the C client uses RSA-4096 signatures; Rust uses HMAC-SHA256 under the shared master key. This means **the invitee cannot verify the blob originates from the inviter** unless both sides already share the master key — which defeats the threat model of cross-user sharing. The module is honest about this under bd-1du.5, but as deployed today the `accept_temppass_wire` helper requires the caller to already possess the master key. This is fine for the round-trip test but is **not** a real cross-user team-share protocol. Remediation: complete bd-1du.5 (RSA-4096 keypair) before claiming "business/team parity" is production-ready for the team-share path.
- **HIGH-3.N (no expiry / revocation window).** The wire blob carries no timestamp, no sequence number, and no revocation marker. Once a temppass wire leaks, the holder can re-derive the master key **forever** (modulo Argon2id cost). The C client likewise has this problem, but the C client's RSA signature at least binds the blob to a concrete RSA keypair that can be rotated server-side. Here nothing can be rotated. Remediation: include an `issued_at` + `expires_at` + monotonic `sequence` inside `TemppassBlob` and bind them into the AAD; have the daemon reject decodes whose `expires_at` is in the past.
- **LOW-3.O (AAD fixed constant).** The AAD is the literal `"pcloud-crypto/share-temppass/aad/v1"` (`share_temppass.rs:69`). If the rotation in `bd-1du.5` introduces a bump, the same code path will reject old blobs silently — no version upgrade test exists. Document and add an integration test.
- **LOW-3.P (hand-rolled base64).** The crate hand-rolls base64 encode/decode (`share_temppass.rs:410-491`) "to avoid pulling `hex` into the dep graph". The encoder/decoder are tested (`base64_round_trip` at line 634), but they are **one more non-standard crypto adjacent parser** to audit. A quick read shows it looks correct, but: (a) the decoder does not check padding byte positions thoroughly (e.g. `==` in the middle of the string); (b) no fuzz test. Remediation: either use `base64 = "0.22"` (already in the dep graph — `crates/pcloud-kms/src/lib.rs:636` uses `base64::engine::general_purpose::STANDARD`) or fuzz the decoder.

---

## 8. Zeroization

### 8.1 `SecretBytes` — `crates/pcloud-secret/src/secret_bytes.rs:22-23`

```rust
#[derive(ZeroizeOnDrop)]
pub struct SecretBytes(Vec<u8>);
```

**GOOD:** Derives `ZeroizeOnDrop`. `PartialEq` is constant-time (`secret_bytes.rs:82-94`). `Clone` not derived — explicit `clone_secret()` only. No `Serialize`/`Deserialize` impl. `Debug` redacted.

### 8.2 Master key storage — `crates/pcloud-crypto/src/keys.rs:73-78`

```rust
#[serde(skip)]
pub active_key_material: Option<SecretBytes>,
```

**GOOD:** `#[serde(skip)]` so the master key never reaches a serialiser. Wrapped in `SecretBytes` — zeroize on drop.

### 8.3 Per-file key — `crates/pcloud-crypto/src/content.rs:126-134`

**GOOD:** output of `derive_file_key` is `SecretBytes` — zeroize on drop.

### 8.4 `PlaintextDek` — `crates/pcloud-kms/src/lib.rs:120-149`

```rust
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct PlaintextDek(pub Vec<u8>);
```

**GOOD:** zeroize on drop.

### 8.5 Argon2id intermediate buffer — `crates/pcloud-crypto/src/keys.rs:154-159`

```rust
let mut derived = vec![0u8; DERIVED_KEY_LEN];
Argon2::default()
    .hash_password_into(password.expose_secret().as_bytes(), salt, &mut derived)
    .expect(...);
SecretBytes::new(derived)
```

The intermediate `derived: Vec<u8>` is moved into `SecretBytes::new(derived)` **without an explicit zeroize of the old stack/heap location before the move**. Because `Vec::new` here just takes ownership of the already-allocated buffer, the pointer/length/capacity move is trivial and **no copy exists in heap memory**. So zeroization is preserved once `SecretBytes` drops. OK.

### 8.6 Password scorer — `crates/pcloud-crypto/src/password_scorer.rs:376-394, 466-469, 670-683`

- `lpwd`, `ldpwd` intermediate buffers are explicitly `zeroize()`-d after use (`line 466-467`).
- `usercopy`, `salt`, `derived` buffers in `psync_derive_password_from_passphrase` are explicitly `zeroize()`-d (`line 670, 679, 682`).
- `SecretBytes` holds the final base64 output — zeroize on drop.

**GOOD.**

### 8.7 HMAC engine intermediate state

`hmac::Hmac<Sha256>` / `hmac::Hmac<Sha512>` do **not** implement `Zeroize` — they carry their inner state as plain arrays. This is a **known limitation of the `hmac` crate**: the HMAC key is mixed into the inner digest state and is not zeroized when `Mac` instances are dropped.

- **MEDIUM-3.Q (HMAC inner-state residue).** Every call site in `pcloud-crypto` instantiates a fresh `Hmac<Sha256>` / `Hmac<Sha512>` from a `SecretBytes` key, computes the tag, and drops the MAC instance. The inner state, which mixes the key into two hash blocks, is **not** zeroized on drop. This is a small residue window (one function's stack frame or heap alloc, depending on MAC instantiation) but it violates the strict "no key bits survive drop" posture. Remediation: wrap `Hmac::<T>::finalize()` calls in a helper that explicitly zeroizes a by-value wrapper, or upstream a `ZeroizeOnDrop` impl for the relevant `hmac` types. This is already tracked upstream as [RustCrypto/MACs#134]-class. Mark as **MEDIUM** because no test or theoretical attack exploits this residue without a heap-probe primitive.

### 8.8 KMS cache

- `cache_lookup` returns `dek.clone_secret()` on hit (`crates/pcloud-kms/src/lib.rs:244-253`) — the cached entry remains live until TTL expires or `stop()` evicts. The caller gets a fresh `PlaintextDek` that zeroizes on drop. Eviction is via `HashMap::remove`, which triggers `Drop` on the `CacheEntry`, which drops the `PlaintextDek`, which zeroizes the bytes.
- **GOOD.**

### 8.9 Zeroization findings summary

- **GOOD:** master key, per-file key, KMS DEK, Argon2id output, filename HMAC output all wrapped in zeroize-on-drop types.
- **MEDIUM-3.Q (HMAC residue — see above).**
- **LOW-3.R (hex encoder output not zeroized).** `lib.rs:971-979` `hex_encode` produces a `String` that is printed into `ReencodedPrivateKey.private_key_hex` and returned to the caller. The inputs are the **derivation salt** (non-secret) and the **fingerprint** (non-secret) and **flags** (non-secret) — none of these are secrets. But the HMAC signature (also `hex_encode`-d at `lib.rs:894`) derives from the master key, and the hex string is not a key itself. This is OK. **No action.**

---

## 9. Constant-time comparisons

All critical compares use `subtle::ConstantTimeEq`:

- Fingerprint check: `crates/pcloud-crypto/src/keys.rs:205` — `computed.0.ct_eq(&stored.0).into()`.
- `change_password` old-vs-new password compare: `lib.rs:936-940`.
- Temppass signature verify: `share_temppass.rs:227` — `expected.ct_eq(signature).unwrap_u8() == 1`.
- `SecretBytes::eq`: `crates/pcloud-secret/src/secret_bytes.rs:91-94` — `ct_eq`.

**GOOD:** no naive `==` on secret material found in the crypto crate.

- **LOW-3.S (`unwrap_u8() == 1` vs `.into::<bool>`).** At `share_temppass.rs:227` the idiom `.ct_eq(...).unwrap_u8() == 1` is technically correct (the `subtle::Choice::unwrap_u8` returns 0/1), but the `!= 0` return to a boolean branch does preserve constant-time because the branch runs after the full compare. Readers may misread this — prefer `bool::from(expected.ct_eq(signature))`. Cosmetic only.

---

## 10. Test vectors (KAT)

### 10.1 What exists

- **Self-consistency round-trip tests** for sector seal/open (`content.rs:280-327`, `tests/integration.rs:22-100`, `tests/proptest_seal.rs:32-92`).
- **Self-consistency round-trip** for temppass (`share_temppass.rs:523-646`).
- **Deterministic filename encoding** self-tests (`metadata.rs:118-147`).
- **PBKDF2-HMAC-SHA-512 RFC 6070-style KAT** for the *account* passphrase derivation (`password_scorer.rs:797-814`).
- **Password scorer regression** tests (`password_scorer.rs:703-786`).

### 10.2 What is missing

- **No KAT against the legacy C client's sector output.** There is no test that takes a known `(master, file_seed, sector_index, plaintext, ciphertext_produced_by_C)` tuple from `pclsync/pcryptofolder.c` and proves the Rust `open_sector` recovers the same plaintext.
- **No KAT against the legacy C client's filename encoding.** `metadata::encrypt_filename` uses `HMAC-SHA256` with a new fixed label — the C client does not do this. So KAT is structurally impossible unless cross-client interop is a goal. Currently CLAUDE.md does not claim byte-level interop, but it also does not explicitly call out that the Rust encrypted-folder format is **incompatible** with the C encrypted-folder format.
- **No KAT against the legacy C client's temppass blob.** Documented incompatibility at `share_temppass.rs:39-45` (HMAC vs RSA signature).
- **No fuzz targets** for `seal_sector` / `open_sector` / `encrypt_filename`. The proptest suite at `proptest_seal.rs` is bounded to 128 cases per property, which is a reasonable CI budget but is not a fuzz harness.

### 10.3 Finding

- **CRITICAL-3.A (no cross-client KAT for interop claims).** If the product ships any claim of interop with pcloudcom/pcloud-rs encrypted content, this is a blocker. If the product commits to "Rust-only encrypted-folder format, migration required", this is NOT a blocker — but the CLAUDE.md and parity matrix must say so in plain English so an auditor does not misread. Right now neither is done: CLAUDE.md says "AES-256-GCM sector encryption" and "deterministic metadata filename encoding" are "Implemented" without flagging byte-incompatibility. Remediation: add a `docs/enterprise/crypto-compat.md` stating "the Rust encrypted-folder format is NOT compatible with the legacy C encrypted-folder format; users re-enrol on migration", and add a test module `crypto_compat.rs` asserting that a freshly-enrolled profile produces ciphertext the Rust code can round-trip through all supported crate versions.

---

## 11. Metadata filename encoding

### 11.1 Scheme — `crates/pcloud-crypto/src/metadata.rs:90-108`

```rust
pub fn encrypt_filename(master: &SecretBytes, plaintext: &str) -> Result<String, MetadataCryptoError> {
    if plaintext.is_empty() || plaintext.contains('/') {
        return Err(MetadataCryptoError::InvalidName);
    }
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(master.expose_secret())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(FILENAME_LABEL);           // "pcloud-crypto/filename/v1"
    mac.update(plaintext.as_bytes());
    let tag = mac.finalize().into_bytes();
    // ... hex-encode, fixed 64 chars ...
}
```

### 11.2 Properties

- **Deterministic:** same master + same plaintext name → same 64-char hex tag. Required for server-side lookup without exposing the master key. Tested at `metadata.rs:119-124`.
- **Collision-resistant:** 256-bit HMAC-SHA256 output. Birthday bound ~2¹²⁸.
- **Reversible? No.** HMAC is a MAC, not a reversible encryption. The crate doc acknowledges this at `metadata.rs:55-76`: "Filename *length* is fully hidden (output is fixed 64 chars)" — which is a benefit, but **the client cannot display the plaintext filename** unless it keeps a local mapping (encrypted_name -> plaintext_name). Today there is **no such mapping** in `CryptoFolderEntry` (`lib.rs:213-222`) — the entry stores only the *encrypted* name. So the daemon **cannot show plaintext file names** without maintaining its own out-of-band local directory.
- **Cross-account uniqueness:** master is per-account → tags do not collide across accounts.
- **Intra-account repeat-name leak:** same plaintext name in different folders produces the same tag. Documented as intentional at `metadata.rs:74-76`.

### 11.3 Findings

- **HIGH-3.T (encryption is one-way; plaintext filename is not recoverable).** This is a fundamental design decision, not a bug, but the CLAUDE.md phrasing "deterministic metadata filename encoding" does not make clear that the encoding is **irreversible**. The daemon currently has no filename-plaintext-cache surface exposed via IPC, which means a CLI listing of a crypto folder returns 64-char hex blobs rather than user-readable names. This is an enterprise-UX blocker. Remediation: either (a) switch to a deterministic *encryption* (e.g. AES-SIV with fixed nonce derived from a HKDF) so the plaintext is recoverable with the master key, or (b) add a local-only `encrypted_name -> plaintext_name` cache to `CryptoShell::folders` populated by `mkdir` and `rmdir` flows. The C client uses AES-CBC over the filename so it IS reversible.
- **MEDIUM-3.U (empty-string rejection only, no UTF-8 NFC normalisation).** `encrypt_filename` rejects empty names and `/` (`metadata.rs:94`). It does **not** normalise Unicode. `"café"` in NFC and `"cafe\u{0301}"` in NFD produce different tags, so a macOS client (NFD) and a Linux client (NFC) will desync. Remediation: normalise via `unicode-normalization::UnicodeNormalization::nfc()` (or at least document it).
- **LOW-3.V (no length check upper bound).** Nothing bounds how long `plaintext` can be. HMAC-SHA256 accepts arbitrary input, but the pCloud backend has filename length limits. The crate should reject names that exceed the backend's maximum **before** deriving the tag, to match the C client's behaviour.

---

## 12. `unsafe` in crypto

```
$ grep -r "unsafe" crates/pcloud-crypto
crates/pcloud-crypto/src/lib.rs:1:#![forbid(unsafe_code)]
(plus three false-positive hits on the word "unsafe" in error messages / doc comments)
```

**GOOD:** `#![forbid(unsafe_code)]` at `crates/pcloud-crypto/src/lib.rs:1`. No `unsafe` blocks in the crate. No `unsafe` blocks in `pcloud-secret` either.

`pcloud-kms`: `#![forbid(unsafe_code)]` at `crates/pcloud-kms/src/lib.rs:30`. No `unsafe` blocks.

- **GOOD:** end-to-end absence of `unsafe` in the crypto-handling crates.

---

## 13. Dependencies

### 13.1 Primitive backers

| Crate | Version | Purpose | Posture |
|---|---|---|---|
| `aes-gcm` | 0.10.3 | AES-256-GCM AEAD | RustCrypto, audited, widely deployed. Feature set: `["aes", "alloc"]`, no `std` dep — good for minimal build. |
| `argon2` | 0.5.3 | Argon2id master-key KDF | RustCrypto. OWASP-recommended defaults (m=19456, t=2, p=1). |
| `hmac` | 0.12.1 | HMAC-SHA256 / SHA512 | RustCrypto. Does **not** implement `Zeroize` on its inner state (see MEDIUM-3.Q). |
| `sha2` | 0.10.9 | SHA-256, SHA-512 | RustCrypto. Pure-software. |
| `subtle` | 2.6.1 | Constant-time compare | RustCrypto. |
| `zeroize` | 1.8.2 | Drop-time memory zeroization | Standard primitive. |
| `getrandom` | 0.2.17 (primary) | OS CSPRNG | `0.3.4` and `0.4.2` also present transitively. |

### 13.2 Feature gating

- `aes-gcm` is pulled with `default-features = false, features = ["aes", "alloc"]` — no "std" feature bloat, no hazmat exports. **GOOD.**
- `zeroize` pulled with `zeroize_derive`. **GOOD.**

### 13.3 Multiple `getrandom` versions

- `getrandom 0.2.17` (primary, used directly by `pcloud-crypto`).
- `getrandom 0.3.4` (transitive via `rand`, indirectly).
- `getrandom 0.4.2` (transitive via something newer).

- **LOW-3.W (multiple `getrandom` versions in tree).** Not a correctness issue — each is a functional OS CSPRNG wrapper — but ships three copies of effectively the same code, bloats build, and complicates audit. Remediation: run `cargo update -p getrandom --precise 0.3.4` + dependency reconciliation to converge on a single version.

### 13.4 FIPS posture

- **No FIPS claim in the crate doc.** The primitives (AES-256-GCM, SHA-256, SHA-512, HMAC, PBKDF2, Argon2id) are all NIST-approved **except** Argon2id, which is not FIPS-140-3 approved. Enterprise deployments that require FIPS-140-3 would need to swap `argon2` for PBKDF2 (or PBKDF2-HMAC-SHA-512, already available on the passphrase path).
- The KMS providers (AWS KMS, HashiCorp Vault transit, PKCS#11 HSM) **can** be FIPS-validated depending on the backing HSM / KMS configuration.

- **MEDIUM-3.X (no FIPS mode switch).** For enterprise claims ("stricter than C on …" per CLAUDE.md Final Rule), consider adding a `CryptoPolicy::fips_mode: bool` gate that switches Argon2id → PBKDF2-HMAC-SHA-512 (same iteration count as the server API-password path — already implemented in `password_scorer.rs`) and refuses any non-FIPS-approved primitive.

### 13.5 Advisories

- No `cargo audit` output captured in this audit (offline environment).
- aes-gcm 0.10.x has no open RUSTSEC advisories as of my cutoff.
- argon2 0.5.x has no open RUSTSEC advisories as of my cutoff.
- `getrandom 0.2.x` has had RUSTSEC-2024-0331 closed; no known open issues.

- **LOW-3.Y (no CI-gated `cargo audit`).** Recommend adding `cargo audit --deny warnings` as a CI gate on the `pcloud-crypto` + `pcloud-kms` crates.

---

## Severity-ranked findings ledger

### CRITICAL

- **CRITICAL-3.A — No cross-client KAT for interop claims.**
  Files: `crates/pcloud-crypto/tests/*.rs`, `CLAUDE.md` "Crypto parity progress".
  The Rust crate uses new primitives (AES-256-GCM, HMAC-SHA256-based filename encoding, HMAC-based setup fingerprint, HMAC-based temppass signature). These are **not** byte-compatible with the legacy C `pclsync/pcryptofolder.c` format. No test asserts, and no documentation explicitly states, this incompatibility. If the product ships with "crypto is active on the retained Rust path" language while users expect to open legacy-C encrypted folders, this is a silent data-access failure.
  Remediation: (a) add explicit "NOT byte-compatible" language in the parity matrix and in a new `docs/enterprise/crypto-compat.md`; (b) add a `tests/legacy_c_kat.rs` that either (i) proves interop against a captured C-client ciphertext (ideal) or (ii) explicitly asserts that legacy-C-shape ciphertext is rejected, matching the documented non-compat contract.

### HIGH

- **HIGH-3.C — Argon2id vs legacy C KDF interop is unverified.**
  Files: `crates/pcloud-crypto/src/keys.rs:134-160`, `CLAUDE.md`.
  See CRITICAL-3.A for the umbrella issue; HIGH-3.C is the specific master-key-KDF drift.
  Remediation: folded into CRITICAL-3.A.

- **HIGH-3.F — No wrong-password rate-limit or lockout at the crypto layer.**
  Files: `crates/pcloud-crypto/src/lib.rs:713-738`, `crates/pcloud-daemon/src/runtime.rs:2533-2564`.
  Remediation: add a `KeyManager::consecutive_failures: u32` counter with exponential backoff; reset on success; consider a hard lockout after N failures.

- **HIGH-3.H — CLAUDE.md is out of date re: `change_crypto_pass` family.**
  Files: `crates/pcloud-crypto/src/lib.rs:837-967`, `CLAUDE.md` "Still missing" list.
  Remediation: fix CLAUDE.md; update `C_FEATURE_PARITY_MATRIX.csv` row.

- **HIGH-3.I — Password rotation silently invalidates existing sector ciphertext.**
  File: `crates/pcloud-crypto/src/lib.rs:837-896`.
  Per-file keys are `HMAC-SHA256(master, "..." || seed)`; rotating `master` rotates every per-file key. Old ciphertext becomes unreadable. No test warns, no doc flags.
  Remediation: either introduce a KEK layer so master rotation does not invalidate file keys, or add an integration test that (a) writes a sector, (b) rotates the password, (c) asserts the old frame is now unreadable — and document the invariant in the `change_password_unlocked` docblock.

- **HIGH-3.L — CLAUDE.md is out of date re: `send_change_user_private` and `priv_key_flags`.**
  Files: `crates/pcloud-crypto/src/lib.rs:814-817`, `crates/pcloud-daemon/src/runtime.rs:2667-2698`, `CLAUDE.md` "Still missing" list.
  Remediation: fix CLAUDE.md; update parity matrix rows for `PSYNC_CRYPTO_FLAG_TEMP_PASS` and `psync_crypto_send_change_user_private`.

- **HIGH-3.N — Temppass blob has no expiry, no revocation, no sequence number.**
  File: `crates/pcloud-crypto/src/share_temppass.rs:158-163`.
  Remediation: add `issued_at: u64`, `expires_at: u64`, `sequence: u64` to `TemppassBlob`; bind them into AAD; have the daemon reject decodes whose `expires_at` is in the past.

- **HIGH-3.T — Filename encoding is irreversible; plaintext is not recoverable.**
  File: `crates/pcloud-crypto/src/metadata.rs:90-108`.
  HMAC-SHA256 is a MAC, not a cipher — there is no inverse. A client listing a crypto folder sees 64-char hex blobs.
  Remediation: switch to deterministic authenticated encryption (AES-SIV) for filenames, or add a local `encrypted_name -> plaintext_name` cache populated by `mkdir`/`rmdir` and persisted in the profile store.

### MEDIUM

- **MEDIUM-3.B — No per-sector key; sector rekey schedule is documented but not enforced.**
  File: `crates/pcloud-crypto/src/lib.rs:1096-1101`, `crates/pcloud-crypto/src/content.rs:177-207`.

- **MEDIUM-3.E — 96-bit random nonce birthday bound (2⁴⁸ sectors per file key).**
  File: `crates/pcloud-crypto/src/content.rs:186-206`. Not reachable today; not future-proof.

- **MEDIUM-3.G — Recovery-code binding is at the IPC layer only.**
  Files: `crates/pcloud-daemon/src/runtime.rs:2714-2720, 2771-2776`. No crypto-level enforcement.

- **MEDIUM-3.J — `ReencodedPrivateKey.private_key_hex` does not include account identity.**
  File: `crates/pcloud-crypto/src/lib.rs:876-895`.

- **MEDIUM-3.M — Temppass HMAC signature is a shared-secret proof, not identity proof.**
  File: `crates/pcloud-crypto/src/share_temppass.rs:38-45, 213-220`.

- **MEDIUM-3.Q — HMAC inner-state key residue is not zeroized.**
  Files: every `Hmac<Sha256>::new_from_slice(...)` call site across the crate. Upstream limitation of the `hmac` crate.

- **MEDIUM-3.U — No Unicode NFC normalisation on encrypted filenames.**
  File: `crates/pcloud-crypto/src/metadata.rs:90-108`.

- **MEDIUM-3.X — No FIPS mode switch.**
  Files: `crates/pcloud-crypto/src/policy.rs`, `crates/pcloud-crypto/src/keys.rs`.

### LOW

- **LOW-3.D — Error discipline divergence on `getrandom` failure (panic vs error).**
  Files: `crates/pcloud-crypto/src/content.rs:189`, `crates/pcloud-crypto/src/share_temppass.rs:304-305`, `crates/pcloud-crypto/src/lib.rs:540`, `crates/pcloud-kms/src/lib.rs:964`.

- **LOW-3.K — `change_password_unlocked` allows rotating to the same passphrase.**
  File: `crates/pcloud-crypto/src/lib.rs:837-896`.

- **LOW-3.O — Share-temppass AAD is fixed; no upgrade test.**
  File: `crates/pcloud-crypto/src/share_temppass.rs:69`.

- **LOW-3.P — Hand-rolled base64 encoder/decoder not fuzzed.**
  File: `crates/pcloud-crypto/src/share_temppass.rs:410-491`.

- **LOW-3.R — Hex encoder output not zeroized.**
  File: `crates/pcloud-crypto/src/lib.rs:971-979`. Inputs are non-secret; cosmetic only.

- **LOW-3.S — `ct_eq(...).unwrap_u8() == 1` idiom is unusual.**
  File: `crates/pcloud-crypto/src/share_temppass.rs:227`.

- **LOW-3.V — No upper bound on `encrypt_filename` plaintext length.**
  File: `crates/pcloud-crypto/src/metadata.rs:90-108`.

- **LOW-3.W — Three `getrandom` versions in the dep graph.**
  File: `Cargo.lock` (`getrandom 0.2.17`, `0.3.4`, `0.4.2`).

- **LOW-3.Y — No CI-gated `cargo audit`.**
  Files: CI config (not in scope for this audit, but relevant for the crypto crates).

---

## What is good (explicit positives)

- `#![forbid(unsafe_code)]` + zero `unsafe` blocks across `pcloud-crypto`, `pcloud-kms`, `pcloud-secret`.
- Master key never persisted; `#[serde(skip)]` on `active_key_material`.
- Policy gate `persist_master_key` rejected with `UnsafePolicy` **before** any key derivation — see `lib.rs:664-668`.
- Constant-time fingerprint compare and constant-time temppass signature compare.
- `SecretBytes::PartialEq` is constant-time.
- `SecretBytes` / `SecretString` are `!Clone` — explicit `clone_secret()` only.
- Sector AEAD binds the sector index as AAD **and** verifies the embedded index before the AEAD call (defence-in-depth against AAD-swap) — `content.rs:260-269`.
- Temppass module verifies signature **before** AEAD unwrap (prevents chosen-ciphertext oracle).
- KMS `PlaintextDek` is `ZeroizeOnDrop`; process-local cache evicts on `stop()` (eviction triggers `Drop` → zeroize).
- `NullKms` is explicit: it refuses every wrap/unwrap call rather than silently falling back.
- KMS cache disambiguates by `(provider, key_id, wrapped_bytes, context)` — wrap-blob replay across contexts is blocked.
- Property tests cover seal/open round-trip, AAD-swap rejection, wrong-key rejection.
- RFC 6070-style KAT exists for PBKDF2-HMAC-SHA-512 (account API password path).
- Password scorer is a byte-faithful port of the C scorer with stricter secret handling.

---

## Actionable remediation summary (ranked)

1. Close CRITICAL-3.A: publish `docs/enterprise/crypto-compat.md`; add explicit non-compat language in CLAUDE.md + parity matrix; add a regression test (`legacy_c_kat.rs`) that either proves interop with captured C ciphertext or asserts explicit rejection.
2. Close HIGH-3.I: decide whether to re-architect per-file key derivation around a KEK layer (preferred) or document + test the "rotation invalidates all existing content" contract.
3. Close HIGH-3.F: add wrong-password backoff / lockout in `KeyManager`.
4. Close HIGH-3.N: add `issued_at` + `expires_at` + `sequence` to `TemppassBlob`; bind into AAD.
5. Close HIGH-3.T: switch filename encoding to AES-SIV (reversible, deterministic, authenticated), or add a local plaintext-name cache.
6. Close HIGH-3.H + HIGH-3.L: sync CLAUDE.md + parity matrix with actual code.
7. Close MEDIUM-3.Q: wrap `Hmac<T>` usage in a zeroize-on-drop helper, or block until the `hmac` crate upstreams `ZeroizeOnDrop`.
8. Close MEDIUM-3.U: add Unicode NFC normalisation before HMAC in `encrypt_filename`.
9. Close MEDIUM-3.X: add a FIPS mode policy bit that swaps Argon2id → PBKDF2-HMAC-SHA-512.
10. Close MEDIUM-3.E: enforce a sector-rekey hook at the daemon once >2³² sectors on a single file key.
11. Tidy LOW items as follow-ups.

---

## End of Section 3
