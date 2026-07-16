# Crypto Subsystem — Iter-3 Delta vs Iter-2

Iter-1: 0 CRITICAL / 3 HIGH / 5 MEDIUM / 4 LOW.
Iter-2: +0 HIGH / +0 MEDIUM / +1 LOW (D-2.1 PBKDF2 stack
intermediates not zeroized). Plus an iter-2 reframe: H-2 should drop
HIGH→MEDIUM because the production share-invite path *is* wired via
`crypto_share_folder_rsa`/`crypto_account_team_share_rsa`; the gate is
only on the `derive_temppass_wire` path.

This iteration: regression check, doc-site verification, and re-audit
of `pcloud-kms`. No `cargo` runs, no source edits.

## Regression check: `wrap_share_invitation_b64` visibility

The fix-recipe in `iter-2-fixes.md:23` was explicit: convert the 3
broken intra-doc links to plain code spans **without** wiring the
symbol public to silently close CRYPTO-H-2.

Verified state of the symbol today:

- `crates/pcloud-crypto/src/share_rsa.rs:193` — `pub fn wrap_share_invitation_b64(...)`
- `crates/pcloud-crypto/src/lib.rs:169` — `pub mod share_rsa;`
- `crates/pcloud-backends/src/shares_backend.rs:580` and `:623` —
  call sites in `crypto_share_folder_rsa` and
  `crypto_account_team_share_rsa`, exercised by
  `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs:230,306,371`.

The symbol is **already public** and **already wired** through the
shares-backend path. It was public before iter-2 (the iter-2 delta at
lines 17-18 explicitly noted iter-1 missed this wiring). The iter-2
fix did not change visibility. So:

- **No regression** from the iter-2-fixes edit. The fix-recipe was
  honoured: the rustdoc warnings were silenced via plain code-span
  text + a redirect comment to `CLAUDEREV/03-crypto.md HIGH-2`, and
  no public-API surface was added or removed.

## Doc-site comment audit

The 3 sites are:

| Site | Reads as | Verdict |
|---|---|---|
| `crates/pcloud-proto/src/methods/shares.rs:107` | plain code span + comment "intra-doc link disabled — the symbol is currently gated and not exported as a public item; see CLAUDEREV/03-crypto.md HIGH-2" | matches recipe; comment **content** is incorrect (see below) |
| `crates/pcloud-proto/src/methods/shares.rs:343` | same shape | same |
| `crates/pcloud-proto/src/shares_api.rs:477` | same shape | same |

Recipe compliance: **OK** — link is disabled, comment redirects to
the parity-review doc, no fake symbol introduced.

Comment-text accuracy: **WRONG**. The wording *"the symbol is
currently gated and not exported as a public item"* is factually
incorrect — `share_rsa::wrap_share_invitation_b64` is `pub`,
`share_rsa` is `pub mod`, and the symbol is reachable from
`pcloud-backends` (and is in fact called from production shares
paths). The actually-gated thing is the **temppass-style flow**
(`share_temppass::derive_temppass_wire` returns `RsaBackendRequired`
under `CryptoBackend::PclsyncCompat`, `share_temppass.rs:343-345`),
which is a different code path.

This is a **doc-text accuracy regression introduced by iter-2-fixes**:
the comment was meant to redirect the reader to HIGH-2, but it now
mis-describes the cause as a public-API gate when it is actually a
backend-runtime gate on a sibling helper.

- **D-1 (NEW, LOW)**: 3 doc-comment regression — the
  intra-doc-link-disabled redirect comments at
  `crates/pcloud-proto/src/methods/shares.rs:107,343` and
  `crates/pcloud-proto/src/shares_api.rs:477` describe the symbol as
  "gated and not exported as a public item" when it is in fact `pub`
  and exported. The actual gating is on `derive_temppass_wire`, not
  on `wrap_share_invitation_b64`. **Remediation**: rewrite the comment
  to say "intra-doc link disabled — the cross-crate path resolution
  is unreliable; see `CLAUDEREV/03-crypto.md HIGH-2` and the iter-2
  reframe in `CLAUDEREV/iter-2/03-crypto-delta.md` (H-2 should drop
  HIGH→MEDIUM; the gate is on `derive_temppass_wire`, not on
  `wrap_share_invitation_b64`)."

## Re-verify iter-1 H-1 / H-2 / H-3

| Iter-1 finding | Verdict | Notes |
|---|---|---|
| H-1 No real C-client KAT for Enhanced | **Still OPEN.** No new external fixture under `tests/fixtures/c_client_kat/`. Same posture as iter-2. |
| H-2 Share-invitation gated off | **Re-affirm iter-2 reframe.** Production share-invite path (`crypto_share_folder_rsa`, `crypto_account_team_share_rsa`) is implemented and wired; only `derive_temppass_wire` rejects PclsyncCompat. Severity should drop HIGH→MEDIUM at next reroll, with the bead retitled to "wire RSA-OAEP into `derive_temppass_wire` for the password-rotation flow." |
| H-3 Merkle parent tags missing AES-ECB | **Still OPEN.** `pclsync_auth_tree.rs` header still admits the AES-256-ECB step is absent. Multi-sector files written by C clients will not verify under Rust at the master tag. No code change. |

## Re-audit: `pcloud-kms` (iter-2 D-1 follow-up)

Iter-2 D-1 was scoped to "is it integrated and reachable". Iter-3
goes one level deeper — full-file read of `crates/pcloud-kms/src/lib.rs`
(1332 lines):

Verified (no new finding):

- `PlaintextDek` is `Zeroize` + `zeroize(drop)`; `Debug` redacts
  (lines 120-149). `WrappedDek` correctly NOT zeroized (ciphertext).
- `unwrap_cached` cache key includes `provider` + `key_id` +
  `wrapped_bytes` + `context`; eviction by TTL drops the
  `PlaintextDek` which zeroizes (lines 244-253). `cache_store` does
  not bound the cache size, but DEKs are at most a few hundred per
  process and the TTL = 300s default, so this is a non-issue.
- `NullKms` returns `NotImplemented` on real ops — no silent
  fallback (lines 309-335). Confirmed.
- `Pkcs11Hsm::new_from_module` (lines 892-924) probe-logs in once at
  construction to fail fast on a bad PIN, then logs out and creates
  per-call sessions. PIN held in `SecretString`. AES-GCM with
  `context` mapped to AAD (lines 967-971). IV: 12-byte random per
  call, vendor module is allowed to overwrite via `&mut [u8]` and
  the actually-stored IV is the post-call slice (lines 962-983). The
  iter-2 D-1 noted this aliasing — verified on re-read it is
  documented inline ("`iv_slice` may have been rewritten by the
  vendor module … store whatever the HSM chose").
- AWS provider (`AwsKms`, lines 362-525): credentials come from
  default chain, never from pcloud-rs config. `encryption_context`
  binds the `context` to the ciphertext under the
  `pcloud_context` key. Async bridge offloads to a fresh OS thread
  inside an existing tokio runtime to avoid reentrancy deadlock —
  this is correct.
- Vault provider (`HashicorpVault`, lines 540-697): token in
  `SecretString`, sent in `X-Vault-Token` per-request. Status-code
  taxonomy mapping is exhaustive: 401/403→`AuthFailed`,
  404→`KeyNotFound`, other non-200→`Other`. `health_check` accepts
  200 OR 429 (Vault standby) — correct per Vault docs.
- Tests: `cache_returns_plaintext_within_ttl`,
  `cache_expires_after_ttl`, `cache_distinguishes_context` —
  cache invariants are unit-tested. Live AWS/Vault tests gated on
  env vars + `PCLOUD_KMS_AWS_TEST=1` / `PCLOUD_KMS_VAULT_TEST=1`.

One observation, **not a finding**: the Vault provider's
`encrypt_dek`/`decrypt_dek` ignores the `key_id` parameter (lines
632, 654, marked `_key_id`) — Vault's transit engine identifies the
key by the URL path (`/v1/transit/encrypt/<transit_key>`), not by an
opaque KeyId. This is a Vault-API peculiarity and is documented
inline in the struct doc. No issue.

No key-leak surface, no silent fallback, no dangerous cache eviction
gap. KMS is integration-clean.

## Bottom line

- **1 new LOW** (D-1: doc-text regression introduced by iter-2-fixes).
- **0 retractions.**
- **0 code regressions** from the iter-2-fixes edit.
- **H-1 / H-2 / H-3 all still OPEN** at iter-1 severities (H-2 still
  awaiting reroll to MEDIUM per iter-2 reframe).
- **pcloud-kms**: deep re-audit confirms iter-2 D-1's "no key-leak
  surface" verdict.

delta count: 1 new, 0 retractions, 0 regressions
