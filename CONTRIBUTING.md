# Contributing to pcloud-rs-rust-dev

<!-- Purpose: dev setup, quality gates, supply-chain checks, and honesty rules for the Rust rewrite. -->

Thank you for your interest in contributing. This document covers the
developer setup, the quality gates your change must pass, the
supply-chain checks you are expected to run locally, the commit style,
and — critically — the project's honesty rules around security and
parity claims. Please read all of it before opening a pull request.

The authoritative project handoff is [`CLAUDE.md`](../CLAUDE.md). The
parity truth files are
[`C_FEATURE_PARITY_REVIEW.md`](./C_FEATURE_PARITY_REVIEW.md) and
[`C_FEATURE_PARITY_MATRIX.csv`](./C_FEATURE_PARITY_MATRIX.csv). Parity
counts are centralised in [`STATUS.md`](./STATUS.md) — every document
must link there rather than hard-code totals.

Structured contributor guidance (workflow diagrams, ADR authoring,
platform-specific build notes, release playbook) lives in the mdBook
contributing chapter at
[`docs/book/src/development/`](./docs/book/src/development/). The security
chapter at [`docs/book/src/security/`](./docs/book/src/security/) covers
the threat model and the non-negotiable security invariants summarised
below.

## Scope

Contributions are welcome to the `` workspace. The legacy C/C++
client (`pcloud-rs/main.cpp`, `pclsync_lib.cpp`, `pclsync/`) is in
maintenance mode — bug fixes only, no new feature parity shims, unless
directly needed by the Rust rewrite.

## Dev Setup

### Toolchain

- Rust edition: **2024**, workspace resolver 3.
- Rust toolchain: pinned via `rust-toolchain.toml` in ``.
  Install `rustup` and let it pick the pinned channel automatically.
- Components required: `rustfmt`, `clippy`.

```bash
rustup show                                # verify pinned toolchain
rustup component add rustfmt clippy        # if missing
```

### One-time supply-chain tooling

```bash
cargo install cargo-deny cargo-audit
```

### C build (only for cross-parity work)

```bash
cd /home/ezechiel203/Projects/FORKS/pcloud-rs
make -j4
```

### bd tracker

The `bd` tracker is the source of truth for open work items. Before
starting non-trivial work, check:

```bash
cd /home/ezechiel203/Projects/FORKS/pcloud-rs
bd list --status=open
```

## Daily Workflow

All commands run from the `` directory.

```bash
cd /home/ezechiel203/Projects/FORKS/pcloud-rs/

cargo fmt --all
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

All five must be green before you push. `-D warnings` on clippy is
non-negotiable; the workspace has been held at zero clippy warnings
across every reconciliation wave.

### Focused test commands

For faster iteration on a subsystem:

```bash
cargo test -p pcloud-proto -p pcloud-daemon -p pcloud-cli
cargo test -p pcloud-config -p pcloud-store -p pcloud-daemon -p pcloud-sdk
cargo test -p pcloud-engine -p pcloud-daemon
```

### Supply-chain gates

Before opening a PR, run both:

```bash
cargo deny --manifest-path Cargo.toml check
cargo audit
```

`cargo deny` consumes `deny.toml`; `cargo audit` consumes `audit.toml`
(which time-boxes the known `fuser` advisory — see
[`SECURITY.md`](./SECURITY.md) and
[`SECURITY-AUDIT-FINAL-14042026.md`](./SECURITY-AUDIT-FINAL-14042026.md)).

If `cargo audit` surfaces a **new** advisory, do not silence it. Open a
tracker item and fix it or document the mitigation before merging.

### Fuzz / stress (optional but encouraged)

Fuzz targets live under `crates/pcloud-proto/fuzz` and
`crates/pcloud-ipc/fuzz`. See
[`TESTING-FUZZ-STRESS.md`](./TESTING-FUZZ-STRESS.md) for concrete
invocations.

## Commit Style

- Write commits in the imperative mood: *"Add readdir handler"*,
  *"Fix clippy manual_div_ceil in share_temppass"*.
- Keep the subject line ≤ 72 characters.
- Use the body to explain **why**, not just **what**. Cite affected
  file paths (`crates/pcloud-fs/src/mount_service.rs`) and tracker IDs
  (`bd-1du.4.3`) where relevant.
- One logical change per commit. Separate refactors from feature work.
- If a commit touches the parity matrix or review file, update both in
  the same commit and cite the code change that motivates the row
  movement.

## Pull Requests

A PR must:

1. Pass `cargo fmt --all --check`, `cargo check`, `cargo clippy -D
   warnings`, `cargo test`, `cargo doc`, `cargo deny check`, and
   `cargo audit`.
2. Include tests covering the new behavior. Security-sensitive changes
   require regression tests (e.g. permissions, redaction, validator
   rejection paths).
3. Update parity artefacts if applicable:
   - `C_FEATURE_PARITY_MATRIX.csv` (status + Rust file citation),
   - `C_FEATURE_PARITY_REVIEW.md`,
   - `CHANGELOG.md` (`[Unreleased]` section, correct Added / Changed /
     Fixed / Security / Known-limitations bucket).
4. Not weaken any security invariant listed in
   [`SECURITY.md`](./SECURITY.md) or in `CLAUDE.md` →
   *Security and Enterprise Rules*.
5. Not introduce new `unsafe` without a `// SAFETY:` justification
   comment for every block.

## Honesty Rules (Non-Negotiable)

This project inherits strict honesty discipline from
[`CLAUDE.md`](../CLAUDE.md). All contributors — human and agent —
must follow these rules.

### Do not claim parity you have not earned

The words

- **"full parity"**,
- **"production ready"**,
- **"enterprise ready"**,
- **"drop-in replacement"**

must not appear in documentation, release notes, PR descriptions, or
commit messages unless `bd-1du.10` is actually satisfied by code, tests,
docs, and parity-matrix evidence. "Substantially implemented" or
"implemented for the retained surface" are acceptable.

### Do not rubber-stamp capability maps

When you mark a parity row `Implemented`:

- cite the **exact** Rust file(s) with line ranges,
- make sure the C source feature actually exists (some `psynclib.h`
  declarations are ghost surfaces and should stay `Rejected`),
- classify the row as retained, rejected, or out-of-scope — not "close
  enough".

`Partial` rows must describe the **exact** missing behavior with C and
Rust file citations.

### Do not weaken security defaults to match C

If a legacy C behavior conflicts with the Rust rewrite's security
posture, the correct move is:

1. keep the Rust path secure,
2. document the legacy behavior,
3. mark the insecure legacy behavior as intentionally not carried
   forward (`Rejected` with rationale in
   [`REJECTED-RATIONALES-14042026.md`](./REJECTED-RATIONALES-14042026.md)).

Specifically, do not:

- reintroduce cleartext password persistence,
- loosen auth-vault permissions (`0600` file / `0700` parent),
- loosen IPC socket permissions or drop the `SO_PEERCRED` UID check,
- enable plaintext transport in production,
- add `danger_accept_invalid_certs` / `accept_invalid_hostnames` /
  custom cert-validator shortcuts,
- permit `allow_other` on writable FUSE mounts, or `allow_root` /
  `setuid` mounts,
- log secrets, tokens, passwords, or keys,
- silently swallow persistence / audit failures on active control
  paths.

### Do not fabricate test results

If a validation command fails, fix the code — do not loosen the test,
do not `#[ignore]` the test, do not remove the assertion. Matrix
upgrades require real green validation, not speculation.

### Do not let docs drift

Whenever code reality changes, update:

1. the relevant `bd` tracker comment,
2. [`C_FEATURE_PARITY_REVIEW.md`](./C_FEATURE_PARITY_REVIEW.md),
3. [`C_FEATURE_PARITY_MATRIX.csv`](./C_FEATURE_PARITY_MATRIX.csv),
4. [`CHANGELOG.md`](./CHANGELOG.md),
5. [`CLAUDE.md`](../CLAUDE.md) if the global handoff state changed
   materially.

## Reporting Security Issues

**Not here.** Please follow [`SECURITY.md`](./SECURITY.md) for private
disclosure. Do not open a public issue, PR, or discussion for anything
that looks like a vulnerability.

## License

By contributing, you agree that your contributions will be dual-licensed
under the [MIT License](./LICENSE-MIT) and the
[Apache License 2.0](./LICENSE-APACHE), matching the workspace policy
declared in `Cargo.toml` (`license = "MIT OR Apache-2.0"`).
