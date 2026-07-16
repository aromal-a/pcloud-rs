# Iter-4 Delta: Crypto Subsystem

**Scope**: Re-verify iter-3 fix-campaign edits to the doc comments at
`crates/pcloud-proto/src/methods/shares.rs:107`, `shares.rs:343`, and
`crates/pcloud-proto/src/shares_api.rs:477`. Re-affirm iter-1 H-1/H-2/H-3
status. pcloud-kms re-audit.

## Verification of iter-3 doc-comment rewrite

### shares.rs:107 (ShareFolderRequest::shared_folder_key)

Read verbatim. Comment now states:

> Wired by `pcloud_crypto::share_rsa::wrap_share_invitation_b64`
> (intra-doc link disabled — cross-crate path resolution is unreliable
> from `pcloud-proto`; the symbol is `pub` and wired through
> `crypto_share_folder_rsa` / `crypto_account_team_share_rsa`; the gate
> flagged by `CLAUDEREV/03-crypto.md` HIGH-2 is on the temppass-style
> `derive_temppass_wire` path, not on this symbol).

**Code reality check**:
- `wrap_share_invitation_b64` is `pub fn` at
  `crates/pcloud-crypto/src/share_rsa.rs:193`. Confirmed.
- It is invoked by `SharesRuntime::crypto_share_folder_rsa` at
  `crates/pcloud-backends/src/shares_backend.rs:564,580` and by
  `crypto_account_team_share_rsa` at `shares_backend.rs:607,623`.
  Confirmed.
- Both runtime delegates call into `SharesApi::crypto_share_folder_rsa`
  (`crates/pcloud-proto/src/shares_api.rs:486`) and
  `crypto_account_team_share_rsa` (`shares_api.rs:527`). Confirmed.
- Live e2e proof exists at
  `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs:230,306,371`.

Doc-comment matches code reality. Accurate.

### shares.rs:343 (AccountTeamShareRequest::team_share_key)

Read verbatim. Identical wording to shares.rs:107, same wiring claim.
Same evidence applies. Accurate.

### shares_api.rs:477 (crypto_share_folder_rsa method-level comment)

Read verbatim. Identical wording inserted as a parenthetical inside the
existing method doc-comment. Accurate.

## cargo doc warning suppression check

`cargo doc -p pcloud-proto --no-deps --document-private-items 2>&1`
produced 5 warnings on `pcloud-proto`:

1. `unresolved link to userid`
2. `unresolved link to mail`
3. `unresolved link to ResiliencePolicy::endpoint_label`
4. `unresolved link to TransportError`
5. `unresolved link to io::ErrorKind`

**None** mention `wrap_share_invitation_b64`, `share_rsa`, or any
symbol in the iter-3 rewrite. The intra-doc-link disabling (replacing
the bracketed `[...]` form with bare backticks) successfully suppressed
the cross-crate warning that iter-3 introduced. The 5 remaining
warnings are pre-existing and unrelated to iter-3's scope (they target
unrelated rustdoc references in resilience/error code paths). No
regression.

## Iter-1 HIGH findings status

- **H-1** (PclsyncCompat KDF cost / RSA-4096 timing in default backend):
  unchanged; still open, still tracked under `bd-1du.5`.
- **H-2** (`derive_temppass_wire` mock-fingerprint gating in retained
  share-temppass path): unchanged; still open. The iter-3 doc-comment
  explicitly clarifies this gate is **not** on the
  `wrap_share_invitation_b64` path — that clarification is consistent
  with my iter-1 finding and does not silently close H-2.
- **H-3** (Enhanced backend Argon2id parameter conservatism): unchanged;
  still open.

All three iter-1 HIGH findings remain open and accurately scoped.

## pcloud-kms re-audit

`crates/pcloud-kms/src/lib.rs` unchanged since iter-3 (1332 lines, mtime
unchanged). No new findings.

## Result

delta count: 0 new, 0 retractions, 0 regressions
