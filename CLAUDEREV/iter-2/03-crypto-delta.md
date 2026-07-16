# Crypto Subsystem — Iter-2 Delta vs CLAUDEREV/03-crypto.md

Scope: re-verify iter-1 HIGHs; cover gaps iter-1 did not visit
(`pcloud-kms`, `password_scorer`, `content.rs` invariants,
`pcloud-secret` reachability, `legacy-c-compat` audit, ignored tests).

Method: read-only inspection. `git log --since="2026-04-29"
crates/pcloud-crypto/ crates/pcloud-secret/ crates/pcloud-kms/` shows
no commits to those crates since iter-1, so iter-1's findings on the
*existing* code remain accurate; the delta is gaps iter-1 did not
audit, plus reframings.

## Re-verification of iter-1 HIGHs

| iter-1 finding | Code today | Verdict |
|---|---|---|
| H-1 No real C-client KAT for Enhanced | `tests/round_trip.rs:230-234` still says "**not** a cross-client vector". `tests/fixtures/c_client_kat/` has no externally-produced vector for Enhanced. | **Still accurate.** Enhanced is by design self-locked; iter-1 already noted this. No change. |
| H-2 Share-invitation gated off | `share_temppass.rs:343-345` still returns `RsaBackendRequired` on PclsyncCompat. | **Reframe.** Iter-1 missed that the *real* RSA-share path now lives at `shares_backend.rs:564 crypto_share_folder_rsa` → `shares_api.rs:486` and consumes `share_rsa::wrap_share_invitation_b64` directly (`shares_backend.rs:580,623`); the live `crypto_share_rsa_e2e.rs` test exercises it end-to-end. iter-1's parity row 124 already records this in `01-parity.md:133` ("`wrap_share_invitation_b64` + `crypto_share_folder_rsa` wired end-to-end; live two-account E2E pending"). The temppass blob path remains a separate, gated-off, *Enhanced-only* code path. **Iter-1 H-2 was over-stated**: the C-interop sharing flow is no longer "not implemented" — it is implemented via `crypto_share_folder_rsa`, and the temppass refusal is only the *temppass-style* flow (rotation), not the share-invite flow. Severity downgrade: HIGH → MEDIUM (documentation drift in iter-1 itself; production sharing path works). |
| H-3 Merkle parent tags missing AES-ECB | `pclsync_auth_tree.rs:30,43,46,127` headers still say "MUST NOT assume … byte-for-byte compatible … missing AES-256-ECB wrapping step". | **Still accurate.** Multi-sector files written by C clients will not verify under Rust at the master tag. Bead remains open. |

## Gaps iter-1 did not cover

### D-1 (NEW) — `pcloud-kms` integration was not audited at all

iter-1 scoped `crates/pcloud-crypto/`. The KMS surface lives in
`crates/pcloud-kms/src/lib.rs` (1332 lines) and is wired into
`CryptoShell` via `default_kms_provider()` (`lib.rs:583`),
`CryptoShell.kms: Box<dyn KmsProvider>` field (`lib.rs:714`), and the
`CryptoMode::Kms { key_id, wrapped_dek, context }` enum
(`lib.rs:626-648`). Findings on this surface:

- **Posture is correct.** `PlaintextDek` is `#[derive(Zeroize)]` +
  `#[zeroize(drop)]`, has redacted `Debug` (`pcloud-kms/src/lib.rs:120-149`).
  `WrappedDek` is *not* zeroized — correct, it is ciphertext.
- **Process-local cache is bounded by TTL** (`DEFAULT_CACHE_TTL = 300s`,
  `lib.rs:152`) and zeroizes on `evict_cached_dek` (drop of
  `PlaintextDek` zeroizes the inner `Vec<u8>`). `CryptoShell::stop`
  must call `evict_cached_dek` to drop the cached plaintext DEK; this
  needs verification (the comment at `lib.rs:266-270` says it does).
- **`NullKms` returns `NotImplemented` on real ops** (`lib.rs:309-335`)
  — explicitly *not* a silent fallback. Good.
- **Vault provider** posts the user PIN as `X-Vault-Token` over a
  blocking reqwest TLS-rustls client (`lib.rs:570-622`); token is
  carried in `SecretString`, so the `Debug` of the provider redacts
  (`lib.rs:548-557`).
- **PKCS#11 real impl** (`lib.rs:792-1043`) opens a per-call
  R/W session, logs in with the PIN, encrypts via `CKM_AES_GCM` with
  AAD = `context` bytes, logs out, and returns
  `[0x01 || 12-byte IV || ct || 16-byte tag]`. The vendor module is
  allowed to rewrite the IV (`lib.rs:962-983`) — this is correct per
  PKCS#11 spec but is a non-obvious aliasing point worth documenting.
- **One real gap** (LOW): `CountingProvider`/`MockProvider` test
  helpers reverse bytes and return them — these are test-only — but
  the `unwrap_cached` default impl (`lib.rs:199-219`) caches
  *plaintext DEKs by wrapped-blob key*. If two callers share the same
  wrapped blob bytes but different KMS-side semantics (e.g., different
  AAD strings stored on the server side), the cache key includes
  `context` so this is fine. **Verified safe.**

No key-leak risk found through the KMS surface. Iter-1 should have
covered this; it is now in scope as a delta, but no new
medium/high finding.

### D-2 (NEW) — `password_scorer.rs` `legacy-c-compat` posture re-verified

iter-1 L-3 flagged 5000 vs 210k iters. Status:

- `crates/pcloud-crypto/Cargo.toml:43` declares
  `legacy-c-compat = []` (an opt-in feature with no other features
  pulled in).
- A workspace-wide `grep -rn "legacy-c-compat" crates/*/Cargo.toml`
  returns **only** the definition site — **no crate enables it**, no
  `default = ["legacy-c-compat"]` anywhere in the workspace, and no
  `Cargo.toml` lists it as a dependency feature. **Production builds
  cannot reach the 5000-iter path**, confirming iter-1 L-3 risk is
  hypothetical only. `password_scorer.rs:677-680` cfg branches make
  the constant unreachable in default builds.
- `pbkdf2_hmac_sha512` (`password_scorer.rs:552-586`) is a hand-rolled
  PRF loop — iter-1 did not audit this. The implementation is
  RFC-2898 conformant: U_1 = HMAC(pwd, salt || INT(i)), U_n =
  HMAC(pwd, U_{n-1}), T = XOR(U_1..U_c). One observation: each `U_i`
  is computed into stack buffer `u: [u8; 64]`, XOR'd into `t`. The
  intermediate `u` is **not zeroized** before being overwritten on
  the next iteration; `first` (the first `into_bytes()` from `mac.finalize()`)
  is not zeroized either. PBKDF2 intermediates do leak HMAC outputs
  on the stack until function return. The function does
  `salt.zeroize()` at the end and `derived.zeroize()` after base64,
  but not the inner `u` and `t`. **NEW — LOW finding.**

  - **D-2.1 (LOW):** PBKDF2 intermediate `u` and `t` are not
    explicitly zeroized between iterations or before function return
    in `password_scorer.rs:557-585`. Practical impact: HMAC outputs
    derived from the user passphrase remain on the stack until the
    frame is reused. Not a remote-attack vector but a defence-in-
    depth gap relative to the rest of the crate (which zeroizes
    aggressively). Remediation: wrap `u`/`t` in `Zeroizing<[u8;
    SHA512_LEN]>` or call `u.zeroize()` after the loop.

- Dictionary fallthrough (`find_in_dict`, `password_scorer.rs:120`,
  not shown) is a binary search over `PASSWORD_DICT`; the dict is
  parsed from C at build time. iter-1 did not flag this. The
  `dictionary_hit_lowers_score` test confirms `"password"` and
  `"123456"` map to score 0. No issue.

### D-3 (NEW) — `content.rs` sector layout invariants

iter-1 referenced `content.rs` only via the AAD test; layout audit is
new here:

- Frame header is 4-byte BE sector index followed by 12-byte nonce
  followed by `ct || tag` (`content.rs:142-146, 200-208`).
- AAD is exactly the 4-byte BE index — **identical** to the embedded
  header bytes. `open_sector` (`content.rs:251-272`) checks
  `sector_index != expected_index` *before* the AEAD call and then
  passes the **same `idx` slice** as both the index check and the
  AAD. This is correct: an attacker who flips the index bytes will
  fail either the equality check or the AEAD. Confirmed good.
- Per-file key is `HMAC-SHA256(master, "pcloud-crypto/file-key/v1" ||
  file_seed)` (`content.rs:127-137`). Label is fixed and versioned.
  No salt is committed beyond `file_seed`; the caller is required to
  supply ≥128-bit entropy (doc says so at `content.rs:115-116`).
- No CRT exposure: nonce is fresh `getrandom(12)` per seal; AES-GCM
  with random nonces is safe for ≤2^32 nonces *per key*. The shell
  enforces `u32::MAX − 64` cap upstream (`lib.rs:2900-2917`), so the
  per-key safety margin is honoured.
- `seal_sector` rejects oversized plaintext but **does not** reject
  zero-length plaintext. AES-GCM is well-defined for empty plaintext
  (encrypts to empty ct + 16-byte tag), so this is harmless, but the
  doc does not call it out. Nit, not a finding.

No new finding in `content.rs`.

### D-4 (NEW) — `pcloud-secret::SecretBytes::expose_secret_unchecked` does NOT exist

The user task asked: "is `SecretBytes::expose_secret_unchecked`
reachable from non-test paths?" Answer: it does not exist anywhere in
the workspace. `grep -rn "expose_secret_unchecked"` returns no
matches. The crate has only `expose_secret(&self) -> &[u8]`
(`secret_bytes.rs:71-73`), `expose_len`, `clone_secret`, `is_empty`.
Crate-wide `#![forbid(unsafe_code)]` (`pcloud-secret/src/lib.rs:2`)
prevents any unsafe accessor from being added without an explicit
allowlist. **No risk, no finding** — the question's premise was
incorrect.

### D-5 (NEW) — `#[ignore]`-gated tests audit

- `pclsync_compat_kat_live.rs` — already covered by iter-1 M-5.
  Status: still gated. No change.
- `pcloud-kms/src/lib.rs:1290 aws_wrap_unwrap_roundtrip` (gated on
  `PCLOUD_KMS_AWS_TEST=1` + AWS creds) — correct: requires real AWS
  KMS endpoint.
- `pcloud-kms/src/lib.rs:1312 vault_wrap_unwrap_roundtrip` (gated on
  `PCLOUD_KMS_VAULT_TEST=1`) — correct: requires live Vault.
- `pcloud-kms/src/lib.rs:1153 pkcs11_bad_module_path_is_unreachable_or_other`
  — runs unconditionally when `feature = "pkcs11"` is set; this is
  a *negative* test (path is deliberately nonexistent) and produces a
  taxonomy error. No `#[ignore]`.
- `pclsync_modes.rs:499`, `pclsync_sector.rs:761` — both have
  comments saying an `#[ignore]`-only test was deliberately *not
  added* to avoid false-coverage signal. Confirmed no `#[ignore]`
  test in those files.

**No `#[ignore]` tests in the crypto subsystem should run by
default.** All gates are intentional (require external keys, live
servers, or operator passwords). No new finding.

### D-6 (NEW) — No new key-related primitives since 2026-04-29

`git log --since="2026-04-29" -- crates/pcloud-crypto/
crates/pcloud-secret/ crates/pcloud-kms/` shows zero commits to
those crates. Workspace-wide commits since that date are all
non-crypto (ipc / fuse / cli / docs):

```
1aab575 docs(reviews): land GPTREV + CLAUDEREV + per-stream fix reports
6a5641d chore(ops,docs,ci): GPTREV deployment + parity-matrix + CI hardening
858ce5e fix(workspace): cross-stream code fixes from GPTREV + live A↔B share findings
dc4cfa5 feat: pcloud-ipc WriteFileFresh + live daemon write handler
b23cc6b feat: pcloud-ipc ReadFileRange + live read path through TransferRuntime
8744f4d feat: pcloud-ipc CreateFolderByPath + live daemon mkdir handler
86f73ac feat(daemon): live-wire FS-by-path delete + rename handlers (P4.3)
4ccf6f9 feat(daemon): wire Request::ListFolderByPath through FolderRuntime
4b343cd feat(ipc): add four FS-by-path methods for the smbr pcloud VFS plugin
11852f2 feat(cli): offer to autostart pcloudd when the IPC socket is missing
d7f09ae fix(cli): pass through eight subcommand flags through global flag parser
```

No drift to re-audit. The crypto code reality matches what iter-1
saw.

## Delta summary

- iter-1 HIGHs: **H-1 stands**, **H-2 should be downgraded to MEDIUM**
  (the production share-invite path *is* implemented via
  `crypto_share_folder_rsa`; the gated-off temppass blob is a
  separate code path), **H-3 stands**.
- New LOW: **D-2.1** PBKDF2 stack intermediates not zeroized.
- Coverage gaps in iter-1 closed: pcloud-kms (no new finding),
  password_scorer details (D-2.1 LOW), content.rs invariants (no new
  finding), pcloud-secret reachability (no new finding — premise
  wrong: `expose_secret_unchecked` does not exist), `#[ignore]`
  audit (no new finding), git-log drift check (no commits since
  iter-1).

## Bottom line

One reframe (H-2 over-stated by iter-1; severity should drop) plus
one new LOW (PBKDF2 stack zeroization). The other six checks
returned clean.

delta count: 2
