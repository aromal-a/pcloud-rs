# Section 3 – Crypto Subsystem Audit (Sonnet, Audit 05)

**Date:** 2026-04-18  
**Auditor:** Sonnet (claude-sonnet-4-6), independent of Opus cross-validation

---

## Scope

`crates/pcloud-crypto/src/` — lib.rs, content.rs, keys.rs, metadata.rs,
password_scorer.rs, share_temppass.rs, pclsync_kdf.rs, pclsync_sector.rs,
pclsync_modes.rs, pclsync_rsa.rs, pclsync_compat_profile.rs,
pclsync_auth_tree.rs, pclsync_filename.rs  
`crates/pcloud-crypto/tests/` — round_trip.rs, pclsync_compat_kat_live.rs,
pclsync_compat_roundtrip.rs, integration.rs, kms_routing.rs  
`crates/pcloud-crypto/tests/fixtures/` — both fixture trees

---

## CRITICAL

### C3-SON-CRIT-1 — KAT fixture is Python-generated, NOT from the C client

**File:** `crates/pcloud-crypto/tests/fixtures/c_client_kat/README.md:1`  
**File:** `crates/pcloud-crypto/tests/round_trip.rs:1–14`

The KAT fixture named `c_client_kat` was generated from the Python
`cryptography` library against the **Rust-defined** wire format
(AES-256-GCM + HMAC-SHA-256 per-file-key label), not from a sector
encrypted by `pclsync/pcryptofolder.c`. The fixture README itself says:

> "This fixture does NOT prove compatibility with the legacy C client … Cross-client compatibility is tracked under bd-1du.10 and remains Partial."

The test `kat_c_client_vector` therefore does not close the cross-client
byte-interop claim. Files encrypted by the official pCloud C client use
PBKDF2-SHA512 + RSA-4096 + AES-256-CBC-CTS (not GCM). A user who encrypts
files on the official desktop client and then tries to decrypt with this
Rust client, or vice versa, will receive `AuthFailed`. No test currently
catches that regression.

**Remediation:** Capture a real C-client-encrypted sector (via
`pcryptofolder_fileencoder_get`) and add a decrypt-only KAT in
`round_trip.rs`. Until that test passes, every matrix row claiming
crypto-folder byte-compatibility must be downgraded from `Implemented` to
`Partial`. Tracking bead: `bd-1du.10`.

---

## HIGH

### C3-SON-HIGH-1 — `sectors_sealed` counter is not serialised; resets on daemon restart

**File:** `crates/pcloud-crypto/src/lib.rs:678` (`#[serde(skip)]`)

The nonce-budget counter (`sectors_sealed: AtomicU64`) tracks how many
AES-256-GCM sectors have been sealed in the current session and refuses
further seals once it approaches `u32::MAX - 64`. However, the field is
`#[serde(skip)]`, so it resets to zero on every daemon restart. A
long-lived daemon that encrypts fewer than `u32::MAX` sectors in any single
process lifetime is safe, but a daemon that restarts repeatedly (e.g. due to
crashes) with the same master key and per-file seeds will accrue nonce risk
across restarts without any counter. The per-sector 96-bit random nonce
means the per-restart nonce space is independent, but the safety argument
depends on per-restart independence being true — and file seeds are
deterministic per file, which means key material repeats across restarts.

**Remediation:** Either serialise `sectors_sealed` alongside the profile
(summing across restarts) or, preferably, rotate file seeds on each restart
to ensure per-restart key independence. Open a tracking bead.

### C3-SON-HIGH-2 — PBKDF2 iteration count is 5 000 on default build (legacy path active)

**File:** `crates/pcloud-crypto/src/password_scorer.rs:540–681`

`psync_derive_password_from_passphrase` selects `PBKDF2_ITERS_LEGACY = 5000`
when `feature = "legacy-c-compat"` is active, and `PBKDF2_ITERS_OWASP =
210_000` otherwise. The legacy count (5 000 × HMAC-SHA-512) is far below
OWASP 2023 guidance (600 000 × HMAC-SHA-256 / 210 000 × HMAC-SHA-512). The
`Cargo.toml` default feature set must be inspected to confirm which path is
active by default. If `legacy-c-compat` is a default feature, every
production auth derivation uses 5 000 iterations — an order-of-magnitude
below the minimum bar for offline-guessing resistance.

**Remediation:** Confirm the default feature set. If `legacy-c-compat` is
default, remove it from `[features] default = [...]` and document the
migration in the deployment runbook. The live auth path (password → API
token) must use OWASP-level iterations except for byte-compatible C-server
interop flows explicitly opted in.

### C3-SON-HIGH-3 — RSA-4096 keypair for signature in temppass flow is not yet landed

**File:** `crates/pcloud-crypto/src/share_temppass.rs:40–46`

The `TemppassBlob::sign` method uses HMAC-SHA-256 under the active master
key as a substitute for the C client's `prsa_sign_sha256_hash(crypto_privkey,
...)`. The comment says:

> "When RSA keypair mirroring lands under bd-1du.5, `TemppassBlob::sign` is the single place to swap."

HMAC proves *current-session-master-key-holder* origin (symmetric) rather
than *user-identity* binding (asymmetric). A compromised master-key bearer
can forge signatures that appear to come from the user. The C protocol
relies on the invitee verifying the RSA signature against the user's public
key; the Rust path cannot satisfy that server/peer check until RSA is wired.
This means crypto folder sharing does not interoperate with official clients
and the signature does not provide the same security guarantee as the C
client.

**Remediation:** Track bd-1du.5 to landing. Until it lands, the share-
temppass feature should be marked `Partial` in the parity matrix and the
`derive_temppass_wire` docs should state the limitation more prominently.

---

## MEDIUM

### C3-SON-MED-1 — `cache_ttl_secs` policy field is dead (auto-stop timer not wired)

**File:** `crates/pcloud-crypto/src/keys.rs:58–68`

The `KeyManager::cache_ttl_secs` field is serialised and documented but the
daemon does not yet spawn a timer to call `CryptoShell::stop()` when the TTL
expires. The comment in the code reads:

> "Current status: dead policy state (audit-04 LOW §3-opus L-2). The daemon does not yet start an auto-stop timer keyed on this value."

Any user who configures a short TTL expecting automatic key eviction will not
get it. This is a silent security non-delivery.

**Remediation:** Wire the auto-stop timer in the daemon bootstrap or promote
`cache_ttl_secs` to a `TODO(bd-1du.X)` with an explicit bead reference so
it is tracked.

### C3-SON-MED-2 — Merkle / auth-tree layer (`pclsync_auth_tree`) has no cross-client KAT

**File:** `crates/pcloud-crypto/src/pclsync_auth_tree.rs` (gated on `pclsync-v2`)

The 128-ary Merkle authentication tree mirrors `pfs_crpt_*` in
`pclsync/pfscrypto.c`. No known-answer test against C-generated tree
structures is present in the fixture set. Any divergence in tag layout,
endianness, or branching factor would silently produce unverifiable auth
trees.

**Remediation:** Add a committed C-client-extracted fixture (sector tree root
+ leaf tags) and a decode KAT. Reference: `bd-1du.10`.

### C3-SON-MED-3 — `pclsync_sector` short-plaintext path (< 16 bytes) needs a C-vector KAT

**File:** `crates/pcloud-crypto/src/pclsync_sector.rs:27–43` (short-plaintext branch)

The short-path (ciphertext length == plaintext length, auth-blob from
`rnd XOR pt`) is faithfully described from the C source (`pcrypto.c:505–513`)
but no committed fixture tests it against a real C-encrypted short-plaintext
sector. A subtle ordering difference in the XOR or AES-ECB block placement
would produce a decryption failure only for short inputs.

**Remediation:** Capture a C-client short-plaintext sector (e.g. 4 bytes) and
add a KAT. Track under `bd-1du.10`.

---

## LOW

### C3-SON-LOW-1 — Nonce-budget safety margin (64) is not justified

**File:** `crates/pcloud-crypto/src/lib.rs:541`

`NONCE_BUDGET_SAFETY_MARGIN = 64` caps the in-session seal count at
`u32::MAX - 64`. The margin was chosen to give the daemon a small window to
rotate keys before nonce-space overflow. Sixty-four sectors is extremely
tight — a high-throughput write burst could fill the margin in milliseconds.
NIST SP 800-38D recommends a 2^32 nonce limit for random-nonce GCM; the
current counter tracks per-session seals regardless of key, so the margin
is applied per master-key lifetime, not per file-key lifetime. A larger
margin (e.g. 1 000 000) or per-file-key tracking would be more robust.

**Remediation:** Either increase the margin substantially or track per-file-key
seal counts so the budget is bounded per derived key rather than per session.

### C3-SON-LOW-2 — `pclsync_compat_kat_live` test gated on `PCLOUD_KAT_PASSWORD` env var

**File:** `crates/pcloud-crypto/tests/pclsync_compat_kat_live.rs:1`

The live pclsync-v2 KAT requires a real account password at test time. The
fixture files are committed, but CI will skip the test unless the env var is
set. This means the full decrypt-against-real-server path is not exercised in
standard CI and could silently regress between releases.

**Remediation:** Derive a static test password from the committed fixture and
ensure the KAT can run without a real account credential (if the fixture was
generated from a known password, embed it as a test constant behind a clear
disclaimer).

---

## Summary Table

| ID | Severity | Short description |
|----|----------|-------------------|
| C3-SON-CRIT-1 | CRITICAL | Python-generated KAT ≠ C-client byte interop |
| C3-SON-HIGH-1 | HIGH | `sectors_sealed` not persisted; nonce budget resets on restart |
| C3-SON-HIGH-2 | HIGH | PBKDF2 5 000-iter legacy path may be active by default |
| C3-SON-HIGH-3 | HIGH | RSA signature in temppass not yet landed; HMAC substitute |
| C3-SON-MED-1 | MEDIUM | `cache_ttl_secs` auto-stop timer is dead code |
| C3-SON-MED-2 | MEDIUM | Merkle auth-tree has no C-vector KAT |
| C3-SON-MED-3 | MEDIUM | `pclsync_sector` short-path has no C-vector KAT |
| C3-SON-LOW-1 | LOW | Nonce-budget safety margin (64) is too small |
| C3-SON-LOW-2 | LOW | Live pclsync-v2 KAT skipped in standard CI |

---

## What is confirmed correct

- **Constant-time comparisons:** `subtle::ConstantTimeEq` is used in
  `keys::KeyManager::matches_setup` (fingerprint check) and
  `share_temppass::TemppassBlob::verify` (signature check). No plain `==` on
  secret bytes.
- **Zeroize discipline:** `SecretBytes`/`SecretString` wrap all key material;
  intermediate buffers in `password_scorer.rs` explicitly call `.zeroize()`.
  `ZeroizeOnDrop` on `UnlockedKek` and related types.
- **Fresh per-sector nonce:** `getrandom` is called inside `seal_sector` for
  every invocation; no nonce reuse across calls to the same key.
- **Sector-index AAD binding:** sector index is big-endian encoded into both
  the frame header and the GCM AAD, preventing sector-swap replay. Test
  `hand_computed_aad_roundtrip` validates the endianness.
- **No secret persistence:** `active_key_material` is `#[serde(skip)]`;
  passwords are never written to disk. Policy layer (`CryptoPolicy`) refuses
  `persist_master_key = true`.
- **Brute-force lockout:** `consecutive_failures` is persisted across
  restarts and enforces exponential backoff up to 30 minutes.
- **Domain separation:** HMAC labels for file-key derivation, fingerprint,
  and filename encoding are distinct (`/file-key/v1`, `/fingerprint/v1`,
  `/filename/v1`).
- **NFC normalisation:** applied in both `encrypt_filename` and
  `psync_derive_password_from_passphrase` so macOS-NFD input matches
  Linux-NFC.
- **`#![forbid(unsafe_code)]`** enforced at crate root.
- **Backend dispatch:** `CryptoBackend::PclsyncCompat` (default) vs
  `Enhanced` is explicit and a mismatch returns `CryptoError::BackendMismatch`
  rather than silently decrypting with the wrong algorithm.
