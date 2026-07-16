# Iter-5 Delta: Crypto Subsystem

**Scope**: Re-affirm iter-3 doc-comment rewrite at
`crates/pcloud-proto/src/methods/shares.rs:107`, `shares.rs:343`, and
`crates/pcloud-proto/src/shares_api.rs:477`. Re-verify iter-1 H-1/H-2/H-3
status. iter-4 fix-campaign produced no edits in crypto scope.

## Verification of iter-3 doc-comment rewrite

All three sites read verbatim today match the iter-3 fix-recipe wording:

> intra-doc link disabled — cross-crate path resolution is unreliable
> from `pcloud-proto`; the symbol is `pub` and wired through
> `crypto_share_folder_rsa` / `crypto_account_team_share_rsa`; the gate
> flagged by `CLAUDEREV/03-crypto.md` HIGH-2 is on the temppass-style
> `derive_temppass_wire` path, not on this symbol.

Code reality re-checked (unchanged from iter-4):

- `wrap_share_invitation_b64` is `pub fn` at
  `crates/pcloud-crypto/src/share_rsa.rs:193`.
- `share_rsa` is `pub mod` at `crates/pcloud-crypto/src/lib.rs:169`.
- Production call sites: `crates/pcloud-backends/src/shares_backend.rs`
  in `crypto_share_folder_rsa` and `crypto_account_team_share_rsa`.
- Live e2e proof at
  `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs`.

Doc-comments remain accurate. No drift since iter-4.

## Iter-1 HIGH findings

- **H-1** (PclsyncCompat KDF cost / RSA-4096 timing in default backend):
  unchanged; still open, tracked under `bd-1du.5`.
- **H-2** (`derive_temppass_wire` mock-fingerprint gating in retained
  share-temppass path): unchanged; still open. Still awaiting reroll
  from HIGH→MEDIUM per iter-2 reframe (production share-invite path
  via `wrap_share_invitation_b64` is wired; the gate is on the sibling
  `derive_temppass_wire` flow only).
- **H-3** (Merkle parent-tag AES-256-ECB step missing in
  `pclsync_auth_tree.rs`): unchanged; still open.

## iter-4 fix-campaign scope check

iter-4-summary confirms no fix-campaign edits in crypto scope this
iteration. No regression vector to investigate.

## Result

delta count: 0 new, 0 retractions, 0 regressions
