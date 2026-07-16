# Iter-5 Delta — Audit 08 (CLI & SDK Surface)

Iter-1 baseline: `CLAUDEREV/08-cli-sdk.md` (CRITICAL 0 / HIGH 1 / MEDIUM 5 / LOW 4).
Iter-2 delta: `CLAUDEREV/iter-2/08-cli-sdk-delta.md` (+1 LOW).
Iter-3 delta: `CLAUDEREV/iter-3/08-cli-sdk-delta.md` (0 new).
Iter-4 delta: `CLAUDEREV/iter-4/08-cli-sdk-delta.md` (0 new).

Iter-4 fix campaign: no edits in CLI/SDK scope.

Read-only re-verification. **Zero new findings. Zero retractions. Zero
regressions.** Audit 08 has now converged across **three** consecutive
iterations (iter-3, iter-4, iter-5).

---

## HIGH-08-1 — completion.rs hand-rolled parallel clap tree

**Status: still open. No movement since iter-4.**

- `crates/pcloud-cli/build.rs` — **54 lines**, byte-identical to iter-4
  (still only stamps `GIT_HASH` / `BUILD_PROFILE`; no
  `clap_complete::generate`, no parser-tree codegen).
- `crates/pcloud-cli/src/completion.rs` — **791 lines**, byte-identical
  to iter-4. No new growth this iter, no consolidation.
- `crates/pcloud-cli/src/app.rs` runtime parser still does not import
  `clap` and does not consume `build_cli()`. No shared AST.

Drift surface unchanged. HIGH-08-1 stands.

---

## LOW-iter2-08-12 — pcloud-compat duplicate inner attributes

**Status: still open. File:line confirmed byte-for-byte.**

- `crates/pcloud-compat/src/lib.rs:1` — `#![warn(unsafe_op_in_unsafe_fn)]`
- `crates/pcloud-compat/src/lib.rs:93` — `#![deny(unsafe_op_in_unsafe_fn)]`

Cosmetic, deny wins. Stands.

---

## SDK examples directory snapshot

`crates/pcloud-sdk/examples/` — **5 files, unchanged** since iter-3:

- `login_and_list.rs`
- `upload_and_download.rs`
- `crypto_lifecycle.rs`
- `public_link.rs`
- `create_tree_public_link_from_paths.rs`

No new examples added; none removed. Set identical to iter-4.

---

## Iter-4 fix-campaign drift check (CLI/SDK scope)

The iter-4 fix campaign landed two narrow edits:

- `packaging/systemd/README.md` lines 4-6 (Deploy & Operations scope)
- `C_FEATURE_PARITY_MATRIX.csv` rows 81/82/83 (Documentation scope)

Neither edit touches CLI/SDK source, help text, completion script, or
SDK examples. No CLI/SDK drift introduced.

---

## Convergence signal

Iter-5 produces **0 new findings, 0 retractions, 0 regressions** in
CLI & SDK Surface. All 11 prior findings (1 HIGH + 5 MEDIUM + 5 LOW)
remain open as documented. The audit-08 dimension has now converged
across **three** consecutive iterations (iter-3 + iter-4 + iter-5)
and will not surface new issues without an active fix campaign on
HIGH-08-1 (clap tree unification) or MEDIUM-08-2..7.

delta count: 0 new, 0 retractions, 0 regressions
