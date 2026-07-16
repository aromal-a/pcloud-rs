# Iter-3 Delta — Audit 08 (CLI & SDK Surface)

Iter-1 baseline: `CLAUDEREV/08-cli-sdk.md` (CRITICAL 0 / HIGH 1 / MEDIUM 5 / LOW 4).
Iter-2 delta: `CLAUDEREV/iter-2/08-cli-sdk-delta.md` (+1 LOW, no retractions).

Read-only re-verification. **Zero new findings.** All iter-1 + iter-2 findings
that have not been fixed in `iter-2-fixes.md` still stand. No regressions
introduced by the iter-2 fix campaign in CLI/SDK scope.

---

## HIGH-08-1 — completion.rs hand-rolled parallel clap tree

**Status: still open.**

- `crates/pcloud-cli/build.rs` — 54 lines, unchanged shape: only stamps
  `GIT_HASH` and propagates `BUILD_PROFILE`. No `clap_complete::generate`,
  no parser-tree codegen.
- `crates/pcloud-cli/src/completion.rs` — now **791 lines** (up from 652
  in iter-2, 651 in iter-1 — i.e. **+139 lines hand-written since iter-1**,
  more drift surface, not less). `build_cli()` is still the sole source
  of the clap tree, mirroring `app::help_text()` and `Command` enum by
  convention only.
- `crates/pcloud-cli/src/app.rs` runtime parser still does not import
  `clap` and does not consume `build_cli()`. No shared AST.

The drift risk is structurally **larger** in iter-3 than in iter-1, since
the hand-rolled tree continues to grow without a unifying source of truth.
HIGH-08-1 stands.

---

## LOW-iter2-08-12 — pcloud-compat duplicate inner attributes

**Status: still open. File:line confirmed.**

- `crates/pcloud-compat/src/lib.rs:1` — `#![warn(unsafe_op_in_unsafe_fn)]`
- `crates/pcloud-compat/src/lib.rs:93` — `#![deny(unsafe_op_in_unsafe_fn)]`

The deny on line 93 wins; the warn on line 1 is dead. No fix landed in
`iter-2-fixes.md` (the campaign skipped this LOW). Cosmetic, no runtime
risk. Stands.

---

## SDK examples re-scan (since 2026-04-29)

`crates/pcloud-sdk/examples/` still contains exactly **5 files**:

| Example | mtime |
|---------|-------|
| `login_and_list.rs` | 17 avril |
| `upload_and_download.rs` | 17 avril |
| `crypto_lifecycle.rs` | 17 avril |
| `public_link.rs` | 28 avril |
| `create_tree_public_link_from_paths.rs` | 28 avril |

No new examples added on or after 2026-04-29. The two 28-avril mtimes
predate the iter-3 audit window.

---

## Iter-2 fix-campaign drift check (CLI/SDK scope)

The iter-2 fix campaign (`iter-2-fixes.md`) landed three changes that
could plausibly affect CLI/SDK help-vs-implementation alignment. Each
verified:

- **`docs/book/src/getting-started/install.md`** — `pcloud-daemon` →
  `pcloudd`. Grep confirms **zero** remaining `pcloud-daemon` refs in
  the file. The CLI ships a binary named `pcloudc` (per
  `Cargo.toml:48`) and the daemon binary is `pcloudd` (per
  `crates/pcloud-daemon/Cargo.toml`). No drift introduced.
- **`README.md`** — `27 crates` → `35 crates` (two sites at lines 24
  and 134). `ls crates/ | wc -l` returns `35`. README is now accurate;
  no drift.
- **`STATUS.md` 149/7/0/30 alignment** — out of scope for audit 08, but
  the row 93 (`upload_writefromfile`) text is consistent with how the
  CLI hides the `diff`/`restore` stub commands (MEDIUM-08-5) — both
  classes now correctly admit Partial / Unavailable instead of
  pretending parity.

No new doc-vs-help drift detected. The iter-2 fixes did not regress
the CLI/SDK surface.

---

## Convergence signal

Iter-3 produces **0 new findings, 0 retractions, 0 regressions** in CLI &
SDK Surface. All 11 prior findings (1 HIGH + 5 MEDIUM + 4 LOW iter-1 +
1 LOW iter-2) remain open as documented; none have been closed by code
changes. The audit-08 dimension is **converged** for read-only purposes
— no further iteration will surface new issues without an active fix
campaign on HIGH-08-1 (clap tree unification) or MEDIUM-08-2..7.

delta count: 0 new, 0 retractions, 0 regressions
