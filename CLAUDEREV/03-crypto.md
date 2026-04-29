# pcloud-rs Enterprise Audit — Dimension 3: Crypto Subsystem

Auditor: Claude Agent (Opus 4.7, 1M ctx)
Date: 2026-04-29
Scope: `crates/pcloud-crypto/` (sources + tests)
Authoritative spec: `docs/crypto-reference-pclsync.md`

## Summary

The `pcloud-crypto` crate now ships a **dual-backend** model. The
**Enhanced** backend (Argon2id + AES-256-GCM, monolithic frame, AAD =
BE u32 sector index) is mature, internally consistent, and well-tested.
The **PclsyncCompat** backend (PBKDF2-HMAC-SHA512 + RSA-4096-OAEP +
custom CBC-CS sector AEAD + base32 filenames) is implemented at the
primitive layer with a published KAT against the documented spec for
the KDF, a CTR regression vector, an offline fixture-shape KAT, and a
gated *live* KAT (`pclsync_compat_kat_live.rs`, `#[ignore]` /
`PCLOUD_KAT_PASSWORD` env-gated). Backend isolation is enforced
(`BackendMismatch`, no silent fallback), and the `--acknowledge-not-interop`
gate for Enhanced is enforced both at the daemon dispatch
(`runtime.rs:3042`) and in the IPC method shape
(`pcloud-ipc/src/methods.rs:1306`).

Security hygiene is strong overall: secrets are wrapped (`SecretBytes` /
`SecretString`), constant-time comparisons (`subtle::ConstantTimeEq`)
are used at every password / fingerprint / signature gate, lazy TTL
eviction, brute-force lockout (10 attempts) with exponential backoff
*persisted across restarts* and a monotonic-floor against clock-rewind,
nonce budget enforced via CAS (`u32::MAX − 64`), Unicode NFC
normalization on passwords/filenames, a redacted `Debug` impl for
`SetupFingerprint` and `TemppassBlob`, and a hard policy gate against
master-key persistence (`policy.rs:79`).

The remaining material gaps are: (1) **NO genuine cross-client KAT**
against an *officially-produced* C-client byte stream is committed —
the KAT in `tests/fixtures/c_client_kat/` is generated from the *spec*
using Python `cryptography`, which proves spec-conformance, not
interop; (2) the **`share_temppass` HMAC-SHA256 substitute** is still
the only path that exists for the Enhanced backend, and the
PclsyncCompat path **explicitly refuses** with `RsaBackendRequired`
(audit-06 ncx.5 guard) — STATUS.md rows 124/142 remain Partial; a
real RSA-4096 wrap exists (`share_rsa::wrap_share_invitation_b64`)
but is not yet wired into the temppass flow; (3) the **Merkle auth
tree** primitive (`pclsync_auth_tree.rs`) is documented as a partial
implementation — the AES-256-ECB step over the HMAC-SHA512[0..32]
result is not applied at the tree level (the comment at
`pclsync_auth_tree.rs:36-47` is explicit), so byte-exactness with C
on-disk auth sectors is not yet established.

## Findings by severity

- **CRITICAL:** 0
- **HIGH:** 3
- **MEDIUM:** 5
- **LOW:** 4

---

## HIGH

### H-1 — No real C-client cross-interop KAT for the Enhanced wire layout

**File:** `crates/pcloud-crypto/tests/round_trip.rs:223-273`
**Severity:** HIGH
**Evidence:** The "C client KAT" is described in `round_trip.rs:230-234`
as "**not** a cross-client vector against the legacy C client … It is
generated from the same spec using the Python `cryptography` library so
the wire format is locked against regression. Cross-client KAT is
tracked under bd-1du.10." Same statement at
`round_trip.rs:1-12`. There is **no fixture under
`tests/fixtures/c_client_kat/`** that originated from `pcloudcc` or
the official apps for the *Enhanced* path. (The Enhanced backend is
*explicitly designed not to interop with C*, so this is honest, but it
also means a future drift in nonce / AAD / file-key derivation cannot
be caught against an external party.)
**Risk:** The Enhanced backend is correct *against itself*. There is no
external anchor. A subtle change to AAD width or label would still pass
all `cargo test` — the spec is documented inline in the same crate.
**Remediation:** Either (a) acknowledge in `STATUS.md` that the
Enhanced backend is by design self-locked and the spec doc is the
authority (this is already true for the *PclsyncCompat* path via
`pclsync_compat_kat_live.rs` whose live fixture comes from a real
account), or (b) add an external Python-`cryptography` sealed fixture
**built outside the workspace** and ship the build script separately
so a reviewer can rebuild it. Mark the in-tree generator as
`docs/`-tracked, not co-resident with the test that consumes it.

### H-2 — PclsyncCompat share-invitation flow is **gated off**, not implemented end-to-end

**File:** `crates/pcloud-crypto/src/share_temppass.rs:97-110, 343-345`
**Severity:** HIGH
**Evidence:** `derive_temppass_wire` rejects the PclsyncCompat backend
unconditionally with `TemppassError::RsaBackendRequired` (mapped to
`CryptoError::NotYetWired`). The audit-06 ncx.5 guard is correctly in
place — silent garbage-blob issuance would be worse — but the C wire
contract (`PSYNC_CRYPTO_FLAG_TEMP_PASS` rewrap in
`pcryptofolder_change_pass_unlocked`) is not actually performed. A
real `share_rsa::wrap_share_invitation_b64` exists
(`share_rsa.rs:193-205`) and uses RSA-4096-OAEP-SHA1, but it is *not*
called from `derive_temppass_wire`. STATUS.md rows 124 / 142 are still
Partial. CLAUDE.md "previously-missing items now claimed implemented"
listing for `share_temppass` therefore is **misleading**: the
*Enhanced* substitute is implemented, the *C-interop* path is not.
**Risk:** Operators reading CLAUDE.md may believe team-share with
crypto is wire-compatible. It is not. The sharer can call the API and
get a clean error; the C invitee will simply never see a usable blob
because the Rust client refuses to issue one. Documentation drift.
**Remediation:** Wire `share_rsa::wrap_share_invitation_b64` into
`derive_temppass_wire` for the PclsyncCompat backend (the recipient
pubkey blob comes off the share request) and gate the symmetric
HMAC substitute behind `CryptoBackend::Enhanced` only. Update CLAUDE.md
to call `share_temppass` *partial* until that wiring lands. The bead
is `pcloud-rs-ncx.89`.

### H-3 — Merkle auth-tree parent tags are HMAC-SHA512 truncated only — no AES-ECB step

**File:** `crates/pcloud-crypto/src/pclsync_auth_tree.rs:36-47`
**Severity:** HIGH
**Evidence:** The C reference is explicit: parent tags are
`AES-256-ECB(aes_key, HMAC-SHA512(hmac_key, level_block)[0..32])`
(`pcrypto.c:644-654`, `pcryptofolder.c:85-90`, see also
`docs/crypto-reference-pclsync.md §3`). The Rust module's own header
admits: *"This module therefore implements the
HMAC-SHA512(hmac_key, concat_of_children)[0..32] half of the parent
construction — the pure-HMAC half of the C routine — and documents
the missing AES step. … Consumers building on this module MUST NOT
assume the returned tags are byte-for-byte compatible with on-disk
pclsync auth sectors until that wrapper lands."*
**Risk:** A file produced via the PclsyncCompat backend will round-trip
*locally* (sectors decode under the same Rust impl) but the master auth
tag at `masterauthoff` will not match what the C client produces, so a
genuine pCloud Crypto Folder file written by the official desktop will
fail tree-level verification under Rust, and vice-versa. This breaks
the "byte-compatible with the official pCloud C client" claim in
`lib.rs:201-203` for files larger than one sector.
**Remediation:** Land the AES-256-ECB-2-blocks wrap step over the
HMAC-SHA512[0..32] result for parent tags, mirroring
`pcrypto_sign_sec` (`pcrypto.c:644-654`). Add a KAT vector at the
master-tag level using a multi-sector fixture from the live KAT
(`tests/fixtures/pclsync_v2/`). Track under `bd-1du.10` / Wave 1
Primitive E follow-up.

---

## MEDIUM

### M-1 — Filename encoder for **Enhanced** is non-reversible HMAC-hex

**File:** `crates/pcloud-crypto/src/metadata.rs:54-125`
**Severity:** MEDIUM
**Evidence:** `encrypt_filename` is documented as
`HMAC-SHA256(master, "pcloud-crypto/filename/v1" || nfc(name))` and
returns 64-char hex. The function is one-way. The C client uses
*reversible* base32-of-AES-CBC-tweak (deterministic but
decodable, see `docs/crypto-reference-pclsync.md §4`).
**Risk:** Determinism is preserved (server lookup works), but the
Enhanced client cannot recover plaintext filenames from a server
listing without an out-of-band map. This is an architectural choice
called out in spec §7 (delta table) and in the file's own out-of-scope
section, but it is not a *bug* — only a deviation worth confirming
documentation is honest about for the Enhanced path. (PclsyncCompat
filenames go through the byte-exact reversible primitive in
`pclsync_filename.rs`.)
**Remediation:** Add a one-line note in `metadata.rs` doc that lookup
is the *only* operation supported on Enhanced filenames; reversal must
go via PclsyncCompat. No code change required. Already covered by
matrix row, but elevate visibility.

### M-2 — Auth tag truncation in `share_temppass` HMAC verify uses 32 byte fixed

**File:** `crates/pcloud-crypto/src/share_temppass.rs:244-254`
**Severity:** MEDIUM
**Evidence:** `TemppassBlob::verify` requires the signature to be
exactly `TEMPPASS_HMAC_LEN = 32` bytes; if a caller passes a different
length the function returns `BadSignature`. The constant-time compare
itself is correct (`ct_eq`). The length check, however, is *not*
constant-time — a wrong length returns earlier than a wrong value.
**Risk:** Length oracle on the signature field. The signature is
publicly transmitted on the wire, so length is *not* secret in the
threat model — but a future caller might pass a partially-controlled
signature buffer.
**Remediation:** Either accept exactly 32 bytes (current behavior) and
document that the early return is intentional because length is
public, or use `subtle::ConstantTimeEq` with a fixed-size array slice
post-equal-length. Low practical risk.

### M-3 — `change_password_unlocked` Enhanced path: salt rotates, but old derivation is NOT compared

**File:** `crates/pcloud-crypto/src/lib.rs:1995-2001`
**Severity:** MEDIUM
**Evidence:** Comment at `lib.rs:1995-2001` is explicit: *"we
deliberately do NOT compare `new_key` against `current_key` here. The
derivation salt is rotated on each call, so the derived keys differ
even when the password stays the same. Callers that want to reject
identical passwords use `Self::change_password`, which performs a
constant-time byte-comparison of the two plaintext passwords up
front."* So `change_password_unlocked(new_password, flags)` will
silently re-derive a *different* key for the *same* plaintext
password. The wire-uploaded `ReencodedPrivateKey` then re-records the
same plaintext, but with new key material on the local shell.
**Risk:** A scripted caller that hits `change_password_unlocked`
directly without going through `change_password` could rotate the
local key under the *same* user-typed passphrase, producing a
divergence between the on-server `crypto_changeuserprivate` blob and
the user's mental model. Limited blast radius — the user's password
still works because the salt is server-side too — but the audit
record will show a rotation that the user did not intend.
**Remediation:** Document at the public API level that
`change_password_unlocked` is the *low-level* primitive; the only
sanctioned caller is `change_password`. Consider gating `pub` to
`pub(crate)` or behind a doc-explicit `unsafe`-flavoured wrapper.

### M-4 — `change_password_pclsync_compat_reencoded` returns hex-encoded synthetic blob, not real `priv_key_ver1`

**File:** `crates/pcloud-crypto/src/lib.rs:2241-2299`
**Severity:** MEDIUM
**Evidence:** The function rotates the local priv_key_ver1 blob
correctly (PBKDF2 + AES-256-CTR + DER re-wrap, see also
`pclsync_compat_profile::PclsyncCompatProfile::parse_priv_blob`). It
**also** packages the result into a `ReencodedPrivateKey {
private_key_hex, signature_hex }` — same wire shape as the Enhanced
path — for "signature compatibility with existing SDK / daemon
callers". Stage 4b is supposed to wire the actual
`crypto_changeuserkeys` RPC. Until then, the daemon receives an
opaque hex string that is not the protocol's expected
`base64(priv_key_ver1)`.
**Risk:** Any caller that takes `ReencodedPrivateKey.private_key_hex`
and posts it to the server via `crypto_changeuserprivate` will upload
a malformed blob. The retained C parity surface row 124/142 is still
Partial in STATUS.md, so this is consistent with tracked status — but
the *Enhanced-shaped return type for a PclsyncCompat code path* is a
hazard the next refactor must remove.
**Remediation:** Introduce a separate `PclsyncReencodedPrivateKey {
priv_key_ver1_blob: Zeroizing<Vec<u8>>, signature_b64: String }`
return type, OR mark the PclsyncCompat branch return as an internal
opaque type until Stage 4b lands the real RPC. Track under
`bd-1du.10`.

### M-5 — Live KAT (`pclsync_compat_kat_live.rs`) is `#[ignore]`-gated

**File:** `crates/pcloud-crypto/tests/pclsync_compat_kat_live.rs:1-100`
**Severity:** MEDIUM
**Evidence:** The genuinely-cross-client KAT (which decrypts a real
file produced by the pCloud server / official app from
`tests/fixtures/pclsync_v2/`) is gated behind `#[ignore]` and the
`PCLOUD_KAT_PASSWORD` env var. Default `cargo test` runs only the
*offline* fixture-shape KAT (`pclsync_compat_kat_offline.rs`), which
verifies file sizes and SHA-256s of the fixtures plus parsing of the
header layouts. The full PBKDF2 → CTR-unwrap → RSA-OAEP-unwrap →
sector-decode chain is exercised only when the env var is set.
**Risk:** CI may not actually run the cross-client KAT on every PR.
A regression in the PBKDF2 / RSA-OAEP path would only surface when an
operator runs the gated test by hand. Given the upstream fixture is
committed (no network needed), there is no good reason for the gate
beyond not committing the password.
**Remediation:** Commit a *non-prod* test password (or an HKDF-derived
constant) to a CI secret, or replace the live fixture's password
encryption with a constant so the KAT runs unconditionally. Until
then, ensure `STATUS.md` calls out that the live KAT is not in the
default `cargo test` gate.

---

## LOW

### L-1 — `KeyManager::derive_key_material` uses `Argon2::default()`, parameters are not pinned to constants

**File:** `crates/pcloud-crypto/src/keys.rs:198-204`
**Severity:** LOW
**Evidence:** Argon2 parameters are sourced from `Argon2::default()`
(crate defaults: `m = 19456 KiB`, `t = 2`, `p = 1`). These are
documented in the doc comment but are not exposed as `pub const`s for
runtime / KAT verification. A future minor-version bump in the
`argon2` crate could change defaults silently.
**Remediation:** Pin parameters explicitly via
`Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::new(19456,
2, 1, Some(32))?)` and expose `ARGON2_M_KIB / ARGON2_T / ARGON2_P` as
`pub const`. Add a regression test that asserts the parameters at
runtime.

### L-2 — `metadata::encrypt_filename` does NFC after the empty / `/` check

**File:** `crates/pcloud-crypto/src/metadata.rs:101-110`
**Severity:** LOW
**Evidence:** Empty / `/` is rejected on the *raw* input
(`plaintext.is_empty() || plaintext.contains('/')`); NFC is applied
*after*. Some Unicode strings (very rare) decompose into a `/`
sequence under NFD, but **not** the other way around — NFC will not
introduce a `/`. Functionally safe, but the check ordering relative to
NFC is non-obvious.
**Remediation:** Move NFC up; recheck `/` and emptiness on the
normalized form. No security impact.

### L-3 — `password_scorer::PBKDF2_ITERS_LEGACY = 5000` reachable behind `legacy-c-compat` cargo feature

**File:** `crates/pcloud-crypto/src/password_scorer.rs:538-545,
675-680`
**Severity:** LOW
**Evidence:** When the `legacy-c-compat` feature is on, password
derivation drops from 210k to 5k PBKDF2 iterations. The feature is
`OFF` by default in the workspace `Cargo.toml`. There is no compile
warning, no `compile_error!`, and no runtime audit log if it is on.
**Remediation:** Emit a `compile_warning!` (via `proc-macro` or a
hard-coded `#[deprecated]`) and a runtime `log::warn!` on the first
auth call when `legacy-c-compat` is active. Already partially mitigated
by `crypto-provider-aws-lc-fips`'s loud `compile_error!` pattern.

### L-4 — `share_temppass::TEMPPASS_BLOB_VERSION = 1` lacks per-version migration table

**File:** `crates/pcloud-crypto/src/share_temppass.rs:65-66`
**Severity:** LOW
**Evidence:** Only version `1` is accepted; future versions will
silently reject as `Malformed`. Acceptable, but a forward-compat
comment block listing planned-future versions and the migration
posture would help. Already noted in `lib.rs:237-253` for the
profile-format epoch.
**Remediation:** Add a comment-only "version 2 reserved for RSA-OAEP
share invitation; see bd-1du.5".

---

## Verification Table — Algorithm Fidelity vs pclsync Spec

| Item                                         | Claimed-impl                                    | Verified-impl                                                    | Verdict       |
|----------------------------------------------|-------------------------------------------------|------------------------------------------------------------------|---------------|
| **PBKDF2-HMAC-SHA512, 20000 iters, 64 B salt** | `pclsync_kdf::derive_kek` (compat backend)    | `pclsync_kdf.rs:99-105`, KAT in `kat_test_vector_from_spec`      | **PASS**      |
| **AES-256-CTR priv-key wrap (counter=0)**    | `pclsync_modes::aes256_ctr_pclsync_xor_inplace` | `pclsync_ctr_kat.rs` regression vector + non-zero offset KAT     | **PASS** (self-consistent) |
| **RSA-4096-OAEP-SHA1 wrap of `sym_key_ver1`**| `pclsync_rsa::oaep_wrap` / `oaep_unwrap`        | `pclsync_compat_kat_live.rs` (gated) decodes server-produced blobs | **PASS** under gated KAT |
| **Sector cipher: CBC-CS3 + HMAC-SHA512 tweak + 32-byte external auth** | `pclsync_sector::seal_sector` / `open_sector` | Module docs cite `pcrypto.c:487-642` line-by-line; live KAT decodes server fixture | **PASS** under gated KAT |
| **Sector size 4096 plaintext, no padding**   | `PCLSYNC_SECTOR_SIZE = 4096`                    | `pclsync_sector.rs:90`                                           | **PASS**      |
| **128-ary Merkle auth tree, master tag**     | `pclsync_auth_tree.rs`                          | Module documents it as **PARTIAL** (HMAC-SHA512 only, no AES step) | **FAIL — H-3** |
| **Filename: AES-CBC-tweak + base32**         | `pclsync_filename::encode_filename`             | Spec-cited at `pclsync_filename.rs:9-43`                         | **PASS** (PclsyncCompat) |
| **Enhanced: AES-256-GCM + Argon2id**         | `content::seal_sector` / `keys::derive_key_material` | `round_trip.rs` self-consistency + `kat_c_client_vector` against spec-built fixture | **PASS** (self-locked) |
| **AAD endianness (Enhanced)**                | BE u32 sector index                              | `hand_computed_aad_roundtrip` proves BE; LE-AAD frame rejected   | **PASS**      |
| **Backend isolation, no silent fallback**    | `BackendMismatch` error from `start_with_backend` | `lib.rs:1640-1648`                                              | **PASS**      |
| **`--acknowledge-not-interop` gate**         | Daemon `runtime.rs:3042`, IPC field `methods.rs:1306` | Enforced before any state mutation                          | **PASS**      |
| **Constant-time password compare**           | `subtle::ConstantTimeEq`                        | `keys.rs:287-292`, `lib.rs:2208-2214`, `share_temppass.rs:249`   | **PASS**      |
| **AES-GCM nonce: fresh per encrypt**         | `getrandom(&mut nonce_bytes)` per call          | `content.rs:190-191`                                             | **PASS**      |
| **Nonce budget (Enhanced)**                  | `u32::MAX − 64` cap, CAS-loop reservation       | `lib.rs:2900-2917` (audit-06 ncx.19)                             | **PASS**      |
| **Zeroize on Drop**                          | `SecretBytes` / `SecretString` / `Zeroizing` / `ZeroizeOnDrop` | All key buffers; `Dk48` in `pclsync_kdf.rs:82`         | **PASS**      |
| **Lock zeroizes resident master + caches**   | `CryptoShell::stop`                             | `lib.rs:1864-1902`                                               | **PASS**      |
| **Brute-force lockout, restart-persistent**  | `consecutive_failures` + `last_fail_at` + monotonic floor | `lib.rs:1696-1766`, atomic_*_serde shims at `:892-1000`  | **PASS**      |
| **TTL eviction of master**                   | `KeyManager::check_and_evict_if_stale`          | `keys.rs:250-265`                                                | **PASS**      |
| **Cross-file key isolation**                 | Per-file HMAC-SHA256 derivation                 | `round_trip.rs:362-386` `cross_file_seed_isolation`              | **PASS**      |
| **`Debug` redaction (fingerprint + temppass)** | Custom `Debug` impl                           | `keys.rs:48-60`, `share_temppass.rs:185-193`                     | **PASS**      |

---

## CLAUDE.md "previously-missing" items — status

| Item                          | Wired?                                                                                       | Tested?                                                                                          | Round-trip vs C-client?              |
|-------------------------------|----------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------|--------------------------------------|
| **`change_crypto_pass` family** | YES — `CryptoShell::change_password` (`lib.rs:2153`), `change_password_unlocked` (`lib.rs:1966`), PclsyncCompat variant `change_password_pclsync_compat_reencoded` (`lib.rs:2261`). Reachable from SDK (`crates/pcloud-sdk/src/lib.rs`) and daemon (`crates/pcloud-daemon/src/runtime.rs:4323`). | Unit tests in `lib.rs` test module + `crates/pcloud-daemon/tests/crypto_change_password.rs` + live `crates/pcloud-live-e2e/tests/change_crypto_pass.rs`. | **NO.** PclsyncCompat path returns synthetic hex blob (M-4); not yet posted to the real `crypto_changeuserkeys` RPC. Stage 4b. |
| **`send_change_user_private`** | YES — `pcloud_proto::crypto_api::CryptoApi::send_change_user_private` (`crypto_api.rs:118`), `SendChangeUserPrivateRequest` method (`methods/crypto.rs:108`), backend (`crypto_backend.rs:231`), SDK (`pcloud-sdk/src/lib.rs:1980`), daemon (`runtime.rs:4345`). | Unit test `send_change_user_private_ok` (`crypto_api.rs:433`), encoder test (`methods/crypto.rs:621`), one-param wire-shape locked. | Spec-conformant request; no live KAT exercising the *server* response shape (the test mocks the API). |
| **`priv_key_flags`**          | YES — `CryptoShell::priv_key_flags` (`lib.rs:1944`), backed by `KeyManager::private_flags` (`keys.rs:103`); `PRIV_KEY_FLAG_TEMP_PASS = 1` matches C `PSYNC_CRYPTO_FLAG_TEMP_PASS`. | Doctest at `lib.rs:1941`, integration via `change_password_unlocked` flow.                       | Constant value matches C; flag is set by `change_password*` flow. |
| **`share_temppass`**          | **PARTIAL.** Enhanced path: full HMAC-SHA256 substitute (`derive_temppass_wire` / `accept_temppass_wire`). PclsyncCompat path: **REFUSES** with `RsaBackendRequired` (audit-06 ncx.5 guard, `share_temppass.rs:343-345`). | 12 unit tests in `share_temppass.rs::tests` cover round-trip, tamper, wrong-pass, debug redaction, both-backend posture. | **NO** for the C-interop path. STATUS.md rows 124 / 142 still Partial. See **H-2**. |

---

## Conclusions

The crypto subsystem is in a strong shape for the **Enhanced** backend
(self-consistent, well-tested, security-disciplined) and at primitive
parity for the **PclsyncCompat** backend (KDF / CTR / RSA-OAEP / sector
cipher / filename codec all pass spec-conformance KATs, and the
gated live-fixture KAT decodes a real pCloud-server-produced sector).

The honest blockers for full C-client byte-compat parity remain:

- **H-3** Merkle auth tree parent-tag construction (HMAC-SHA512 only,
  no AES-256-ECB-2-blocks step) — multi-sector files won't round-trip
  master tag.
- **H-2** `share_temppass` PclsyncCompat path is gated off; the
  RSA-4096-OAEP wrap exists in `share_rsa.rs` but is not yet wired
  into the temppass derivation.
- **M-4** `change_password` for PclsyncCompat returns a synthetic hex
  blob that is not the protocol's expected `base64(priv_key_ver1)` —
  Stage 4b RPC wiring still missing.
- **M-5** the live cross-client KAT is `#[ignore]`-gated and not in
  the default CI gate.

These line up with the `bd-1du.10` open Partial rows (124 / 142) and
the documented Stage 4b daemon-RPC follow-ups. Nothing in this audit
contradicts the parity matrix; rather, this audit confirms the
matrix's "Partial" entries.

No CRITICAL findings.
