# Crypto Backend Plan (Wave 2)

Status: Planning. No code changes yet. Wave 1 primitives (A–F) are being
built in parallel; Wave 2 executes this plan once they land.

## 1. `CryptoBackend` enum

Lives in `crates/pcloud-crypto/src/lib.rs` (top-level, re-exported from
the crate root). Serialized via `serde` as a lowercase string tag for
forward-readability in the profile JSON.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CryptoBackend {
    /// Interoperable with pCloud official apps (pcloudcc / mobile / web).
    /// PBKDF2-HMAC-SHA512 KDF, RSA-4096 master key wrap, AES-CTR sectors.
    PclsyncCompat,
    /// Rust-native hardened backend. NOT interoperable with pCloud apps.
    /// Argon2id KDF, AES-256-GCM sectors. Files encrypted here CANNOT be
    /// decrypted by any other pCloud client.
    Enhanced,
}

impl Default for CryptoBackend {
    fn default() -> Self { CryptoBackend::PclsyncCompat }
}

impl CryptoBackend {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::PclsyncCompat => "pclsync-compat (interoperable)",
            Self::Enhanced      => "enhanced (Rust-only, NOT interoperable)",
        }
    }
}
```

## 2. Persistence layout

Existing profile (Enhanced-only, historical):

```json
{ "salt": "...", "argon_params": {...}, "verifier": "..." }
```

New profile:

```json
{
  "backend": "pclsync-compat",
  "salt": "...",
  "kdf_params": { "iterations": 5000, "algo": "pbkdf2-hmac-sha512" },
  "wrapped_priv_key": "base64-der...",
  "pub_key": "base64-der...",
  "verifier": "..."
}
```

For `Enhanced` the `kdf_params` block is the Argon2id struct; for
`PclsyncCompat` it is the PBKDF2 iteration count. Migration: absence of
a `backend` field means historical Enhanced — loader injects
`backend = Enhanced` and rewrites on next save. No destructive rewrite
is forced.

Extend `CryptoShell::save` / `load` in `pcloud-crypto/src/lib.rs` behind
a `CryptoProfileV2` struct with `#[serde(default)]` on `backend`.

## 3. Setup dispatch

`CryptoShell::setup(password, backend)` becomes a thin dispatcher:

- `Enhanced` -> `setup_enhanced` (current behaviour, unchanged).
- `PclsyncCompat` -> `setup_pclsync_compat`:
  1. generate 64-byte salt,
  2. run PBKDF2-HMAC-SHA512 (5000 iters) -> master key (primitive A),
  3. generate RSA-4096 keypair (primitive B),
  4. AES-CTR-encrypt the DER-encoded priv key with master key
     (primitive C),
  5. call `crypto_setuserkeys` API with base64(priv_blob) + base64(pub).
  6. persist profile with `backend: PclsyncCompat`.

## 4. Unlock dispatch

`CryptoShell::start(password)` loads the profile, reads `backend`, and
dispatches:

- `Enhanced` -> current Argon2id verifier path.
- `PclsyncCompat` -> `crypto_getuserkeys` -> PBKDF2 re-derive -> AES-CTR
  unwrap -> verify RSA key parses -> cache master key in `SecretBytes`.

Cross-backend unlock attempt returns
`CryptoError::BackendMismatch { expected, provided }` — no silent
fallback.

## 5. Sector seal/open dispatch

`content.rs` becomes backend-aware. Each in-memory file handle records
`sealed_with: CryptoBackend`. Seal picks AES-GCM (Enhanced) or
AES-CTR + per-sector HMAC-SHA512 truncation (PclsyncCompat, primitive
D). Open rejects a handle whose `sealed_with` differs from the active
profile backend.

## 6. Filename encoding dispatch

`metadata.rs` dispatches similarly. `PclsyncCompat` uses the
deterministic AES-ECB-on-SHA1 folder-key scheme from pcloudcc
(primitive E). Enhanced keeps the current SIV scheme.

## 7. Share-temppass dispatch

`share_temppass.rs` today is HMAC-only (Enhanced). `PclsyncCompat` path
(primitive F) wraps the per-share key with the recipient's RSA-4096
public key fetched via `crypto_getpubkey`. Dispatch happens on the
sender-side profile backend.

## 8. IPC surface

New request variant, backward-compat by defaulting to
`PclsyncCompat`:

```rust
Request::CryptoSetup {
    backend: Option<CryptoBackend>, // None -> PclsyncCompat
    password: SecretString,
    hint: Option<String>,
}
```

Existing `Request::CryptoStart` unchanged — backend is resolved from
the stored profile.

## 9. UX warnings

CLI interactive flow (`pcloudc crypto setup`) prompts:

```
Choose crypto backend:
  1) pclsync-compat  (interoperable with pCloud apps)  [default]
  2) enhanced        (Rust-only, NOT INTEROPERABLE)
```

If user picks `enhanced`, a required confirmation:

```
WARNING: Files encrypted with the `enhanced` backend CANNOT be decrypted
by the official pCloud applications (web, mobile, pcloudcc). You will
lose access to these files if you stop using pcloud-rs.

Type YES (uppercase) to acknowledge and proceed:
```

Flags for scripts:

- `--backend {pclsync-compat|enhanced}`
- `--acknowledge-not-interop` (required companion to
  `--backend enhanced`; otherwise the CLI errors out).

Daemon logs on every unlock:

```
crypto unlocked: backend=pclsync-compat
```

`pcloudc crypto status` first line:

```
Backend: pclsync-compat (interoperable)
```

## 10. Parity matrix implications

- Row 124 `crypto_share_folder` -> Implemented once PclsyncCompat +
  primitive F land and live-roundtrip passes.
- Row 142 `crypto_account_teamshare` -> same gate.
- `Enhanced` is documented as a **non-parity extension**, not a C-client
  port. Add a "Rust-only extensions" section to
  `C_FEATURE_PARITY_REVIEW.md` when rollout completes.
- Bead `s1p.13` (C-client KAT) closes on the live PclsyncCompat KAT
  from pcloudcc ciphertext (Wave 3).

## 11. Test strategy

1. Per-primitive self-tests from Wave 1 A–F must all pass.
2. `tests/pclsync_compat_roundtrip.rs` — seal then open a file
   in-process under PclsyncCompat.
3. Cross-backend test: setup under PclsyncCompat, lock, attempt unlock
   with a profile claiming Enhanced -> expect
   `BackendMismatch` error.
4. `tests/pclsync_compat_live.rs` gated on `PCLOUD_LIVE_E2E=1` — decode
   pcloudcc-produced ciphertext fixture and re-seal a Rust-produced
   fixture, verify pcloudcc decodes it (manual step in Wave 3).
5. Profile migration test: load a historical Enhanced-only JSON, verify
   `backend = Enhanced` is injected, save, reload, assert round-trip.

## 12. Rollout order

1. Land `CryptoBackend` enum + `CryptoProfileV2` + migration loader.
2. Wire dispatch stubs at setup/unlock/seal/open/filename/share —
   PclsyncCompat branches return `CryptoError::Todo(...)`.
3. Flesh out PclsyncCompat branches using Wave 1 primitives A–F.
4. Add IPC variants, CLI flags, interactive confirmation, log lines.
5. Add integration + cross-backend + migration tests.
6. Keep Enhanced intact — no deletions this wave.
7. Parity matrix flips (rows 124, 142), CHANGELOG, docs, bead closures
   (`s1p.13`, related children).

## 13. Risks & mitigation

- **Primitive drift**: if Wave 1 agents return constants differing from
  the pcloudcc spec (iterations, salt length, sector size), a
  pre-Wave-2 verification step cross-checks primitive outputs against a
  pcloudcc-generated KAT vector committed under
  `crates/pcloud-crypto/tests/kat/`.
- **Schema migration**: absent `backend` field in historical profiles
  defaults to `Enhanced` (historical behaviour). `#[serde(default)]`
  plus explicit unit test prevents silent data loss.
- **Server-side compat**: `crypto_setuserkeys` may reject an RSA priv
  key whose DER encoding diverges from mbedtls output. Mitigation:
  commit a pcloudcc-generated priv-key DER as a KAT; Wave 2 asserts
  our DER is byte-identical for a fixed RNG seed (test-only seed).

## 14. Out-of-scope for Wave 2

- Migration tool converting an existing Enhanced profile to
  PclsyncCompat (requires re-encrypting every sealed file — future
  bead).
- Hybrid sharing between an Enhanced user and a PclsyncCompat user —
  not possible by construction (different wrap primitives).
