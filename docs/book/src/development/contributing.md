# Contributing

This chapter is the mdBook-friendly digest of [`CONTRIBUTING.md`](https://github.com/pcloud-rs/pcloud-rs/blob/main/CONTRIBUTING.md)
at the workspace root. `CONTRIBUTING.md` is the canonical reference — if this
chapter ever disagrees with it, `CONTRIBUTING.md` wins. Here we focus on the
project-specific discipline expected on every patch: tracker hygiene, branch
policy, commit style, signing, mandatory local gates, platform banner
discipline from the X5 cross-platform wave, and honesty rules around parity
claims.

## Workflow At A Glance

1. **Pick up a bead.** Open or claim a tracker item in the `bd` tracker so
   nobody duplicates work. Parity work is rooted at `bd-1du`; see `CLAUDE.md`
   and `STATUS.md` for the current open set.
2. **Branch from `main`.** Use a descriptive branch name
   (`feature/quota-command`, `fix/ipc-framer-overflow`). Small patches may go
   direct; larger work uses a feature branch with a draft PR open for early
   visibility.
3. **Implement in small, reviewable commits.** One logical change per commit.
4. **Run the full local gate** (see [Mandatory Review Gates](#mandatory-review-gates)).
   Discovering failures in CI wastes reviewer time.
5. **Open the PR.** Fill in the template. Link the bead in the body. Tick the
   security checklist when applicable.
6. **Address review,** squash fixups where it helps readability, and merge
   via **rebase-and-merge** — no merge commits on `main`.

## Bead Tracker Hygiene

We use [`bd`](https://github.com/steveyegge/beads) as the source of truth for
work in flight. The parity epic lives at `bd-1du`.

Rules:

- **Every PR links at least one bead.** If no bead exists for your change,
  open one first with `bd add` and paste the ID in the PR body.
- **Update the bead when reality changes.** Status, scope, blockers, and
  cross-links must match what the code does. Stale tracker entries are a
  review blocker.
- **Close the bead from the PR description,** not manually. Use
  `Closes bd-<id>` and `Refs bd-<id>` footers so the merge automation picks
  them up.
- **Do not reopen closed parity beads to add scope.** Open a follow-up bead
  and reference the predecessor. This keeps the parity-proof audit trail
  clean.
- **Do not rubber-stamp a row as `Implemented`** without citing exact file
  paths with line ranges. The parity matrix is audit evidence, not a
  wish-list.

Quick commands:

```sh
bd list --status=open
bd show bd-1du
bd show bd-1du.4
bd show bd-1du.10
```

## Branch Policy

- `main` is **always releasable.** CI must be green before merge.
- No direct pushes to `main` — everything lands via PR.
- No force-push to shared branches. Force-push to your own PR branch *before
  review* is fine, *after review* is discouraged because reviewers lose
  context.
- Long-lived feature branches should rebase on `main` at least weekly to keep
  the parity matrix and code in sync.
- Release branches (`release/x.y`) are protected: only release managers land
  cherry-picks there.

## Commit Message Style

We follow a Conventional-Commits-adjacent style adapted for the parity
workstream:

```
<type>(<scope>): <subject>

<body — what and why, not how>

Refs bd-<id>
Closes bd-<id>   (optional)

Signed-off-by: Full Name <email@example.com>
```

Types in use: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `security`,
`parity` (reserved for matrix-impacting changes).

Subjects:

- present-tense imperative (`add quota command`, not `added` or `adds`),
- ≤ 72 characters,
- no trailing period.

Bodies:

- wrap at 72 columns,
- explain *why* the change is correct, not what the diff already shows,
- call out parity matrix row changes explicitly.

## DCO and Signed Commits

Every commit must be:

1. **DCO-signed** (`git commit -s`) — appends the `Signed-off-by:` trailer
   and asserts you have the right to contribute the change under the project
   licence.
2. **GPG- or SSH-signed** (`git commit -S`) so GitHub shows the "Verified"
   badge. Unsigned commits are rejected by the branch protection rule.

Set up signing once:

```sh
git config --global user.signingkey <key-id>
git config --global commit.gpgsign true
git config --global format.signOff true
```

If your signing key changes, update `SECURITY.md` in the same PR so the trust
root stays auditable.

## Mandatory Review Gates

Every PR must pass the following **locally** before you open it. CI runs the
same gates.

```sh
cd .
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check  --workspace --all-targets --locked
cargo test   --workspace --locked
cargo doc    --workspace --no-deps --document-private-items
cargo audit  --deny warnings
cargo deny   check
```

The `-D warnings` flag on clippy is non-negotiable: the workspace has held at
zero clippy warnings across every reconciliation wave, and we do not regress.

Optional but encouraged:

- `cargo llvm-cov --workspace --summary-only` — coverage must not regress
  (see [testing.md](./testing.md#coverage) for the ratcheting floor).
- `cargo +nightly fmt --all --check` if you touched nightly-only code.
- `typos` for docstring spelling.

Security-sensitive changes (anything touching secrets, IPC, crypto, the
vault, or transport) add **security-review.md** on top of the above.

## PLATFORM / GATING Banner Discipline (from X5)

Cross-platform code in this tree carries a **PLATFORM banner** — a short
comment block at the top of any file, module, or function that diverges on
OS behaviour. The banner states:

- which platforms are **supported**,
- which are **rejected** (with a one-line rationale),
- which **CI jobs** exercise the code path.

Example:

```rust
// PLATFORM: linux, macos (tier-1); windows (tier-2, WSL fallback).
// REJECTED: bsd (no FUSE3 today — revisit after fuser 0.14).
// CI: ci-linux, ci-macos, ci-windows-msvc.
```

Gating banners take the same shape but document feature-flag gating rather
than OS gating:

```rust
// GATING: compiled only when feature = "fuse-mount".
// REJECTED: always-on — FUSE is heavyweight and not needed for sync-only.
// CI: ci-linux-fuse.
```

PRs that add `#[cfg(target_os = "…")]` branches or `#[cfg(feature = "…")]`
divergence without a PLATFORM / GATING banner are sent back. The banner is
how a downstream packager knows at a glance whether a given module is
expected to work on their target.

## Parity Matrix Discipline

If your change implements, enhances, or retires a feature tracked in
`C_FEATURE_PARITY_MATRIX.csv`:

- update the matrix row **in the same PR**,
- update the narrative in `C_FEATURE_PARITY_REVIEW.md` if the classification
  moves between `Implemented`, `Partial`, `Rejected`, or `Missing`,
- reconcile the counts in `STATUS.md` (it is the single source of truth for
  totals),
- add a `CHANGELOG.md` entry under the correct bucket (`Added`, `Changed`,
  `Fixed`, `Security`, `Known limitations`),
- do **not** claim parity in docs or release notes until `bd-1du.10` marks
  the row proven.

## Honesty Rules

Inherited from `CLAUDE.md` and `CONTRIBUTING.md`. Non-negotiable:

- Do not claim **"full parity"**, **"production ready"**, **"enterprise
  ready"**, or **"drop-in replacement"** in docs, release notes, PRs, or
  commits unless `bd-1du.10` is actually satisfied by code, tests, docs, and
  parity-matrix evidence. "Substantially implemented" or "implemented for
  the retained surface" are acceptable.
- Do not weaken security defaults to match C behaviour. If legacy C conflicts
  with the Rust rewrite's security posture, keep Rust secure and mark the
  legacy behaviour `Rejected` with rationale in
  `REJECTED-RATIONALES-14042026.md`.
- Do not fabricate test results. If a validation command fails, fix the
  code — do not loosen the test, do not `#[ignore]` it, do not remove the
  assertion.
- Do not let docs drift. Update the bead, the review, the matrix, the
  changelog, and (if the global handoff changed materially) `CLAUDE.md` in
  the same PR as the code change.

## Licensing

By contributing you agree that your contribution is dual-licensed under MIT
OR Apache-2.0, matching the workspace policy declared in `Cargo.toml`. The
DCO sign-off is your assertion of that right.

## Getting Help

- `bd` tracker for work coordination.
- `#pcloud-rs-dev` on the project chat for real-time questions.
- **Security issues**: do **not** open a public PR or issue — see
  `SECURITY.md` for private disclosure.
