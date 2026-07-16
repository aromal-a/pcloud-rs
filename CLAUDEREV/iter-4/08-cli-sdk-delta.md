# Iter-4 Delta — Audit 08 (CLI & SDK Surface)

Iter-1 baseline: `CLAUDEREV/08-cli-sdk.md` (CRITICAL 0 / HIGH 1 / MEDIUM 5 / LOW 4).
Iter-2 delta: `CLAUDEREV/iter-2/08-cli-sdk-delta.md` (+1 LOW).
Iter-3 delta: `CLAUDEREV/iter-3/08-cli-sdk-delta.md` (0 new).

Iter-3 fix campaign: no edits in CLI/SDK scope.

Read-only re-verification. **Zero new findings. Zero retractions. Zero
regressions.** Audit 08 has now converged twice — iter-3 and iter-4
both produce no movement.

---

## HIGH-08-1 — completion.rs hand-rolled parallel clap tree

**Status: still open. No movement since iter-3.**

- `crates/pcloud-cli/build.rs` — 54 lines, unchanged shape (still only
  embeds `GIT_HASH` / `BUILD_PROFILE`; no `clap_complete::generate`,
  no parser-tree codegen).
- `crates/pcloud-cli/src/completion.rs` — **791 lines**, identical to
  iter-3 byte-for-byte (no new growth this iter, but no consolidation
  either).
- `crates/pcloud-cli/src/app.rs` runtime parser still does not import
  `clap` and does not consume `build_cli()`. No shared AST.

Drift surface unchanged from iter-3. Stands.

---

## LOW-iter2-08-12 — pcloud-compat duplicate inner attributes

**Status: still open. File:line confirmed.**

- `crates/pcloud-compat/src/lib.rs:1` — `#![warn(unsafe_op_in_unsafe_fn)]`
- `crates/pcloud-compat/src/lib.rs:93` — `#![deny(unsafe_op_in_unsafe_fn)]`

Cosmetic, deny wins. Stands.

---

## SDK example build sample (master-prompt §8 task)

Picked 3 of the 5 examples and ran `cargo build --example <name>`:

```
cargo build -p pcloud-sdk \
  --example login_and_list \
  --example upload_and_download \
  --example crypto_lifecycle
```

Result: **`Finished dev profile [unoptimized + debuginfo] target(s)`.**
No errors, no example warnings (only an expected `pcloud-crypto`
build-script note about vendored password dictionary, which is the
documented behavior when the legacy C header is absent — see
`crates/pcloud-crypto/build.rs` and `vendored/password_dict.rs`).

Conclusion: SDK examples are real and link clean against the current
workspace. No regression from iter-2's "Real, no stubs" verdict.
LOW-08-11 (no documented CI gate for `cargo build --examples`) still
stands as a process gap, not a code defect.

---

## SDK examples directory snapshot

`crates/pcloud-sdk/examples/` — **5 files, unchanged** since iter-3:

- `login_and_list.rs`
- `upload_and_download.rs`
- `crypto_lifecycle.rs`
- `public_link.rs`
- `create_tree_public_link_from_paths.rs`

No new examples added; none removed. Mtimes unchanged.

---

## Convergence signal

Iter-4 produces **0 new findings, 0 retractions, 0 regressions** in
CLI & SDK Surface. All 11 prior findings (1 HIGH + 5 MEDIUM + 5 LOW)
remain open as documented; none have been closed by code changes since
iter-1. The audit-08 dimension has now converged across two consecutive
iterations (iter-3 + iter-4) and will not surface new issues without an
active fix campaign on HIGH-08-1 (clap tree unification) or
MEDIUM-08-2..7.

delta count: 0 new, 0 retractions, 0 regressions
