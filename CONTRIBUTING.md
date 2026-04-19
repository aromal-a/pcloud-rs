# Contributing to pcloud-rs-rust-dev

<!-- Purpose: dev setup, quality gates, supply-chain checks, and honesty rules for the Rust rewrite. -->

Thank you for your interest in contributing. This document covers the
developer setup, the quality gates your change must pass, the
supply-chain checks you are expected to run locally, the commit style,
and — critically — the project's honesty rules around security and
parity claims. Please read all of it before opening a pull request.

The authoritative project handoff is [`CLAUDE.md`](./CLAUDE.md). The
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

Contributions are welcome to the `pcloud-rs` workspace. The legacy C/C++
client (`main.cpp`, `pclsync_lib.cpp`, `pclsync/`) has been **removed from
this fork**. Those sources were deleted once the Rust rewrite reached
functional parity. The upstream C reference tree remains available read-only
at `https://github.com/pcloudcom/pcloud-rs`; it is not maintained here. Do
not add a `C_CODE/` drop or re-introduce the C build system into this fork.

## Dev Setup

### Toolchain

- Rust edition: **2024**, workspace resolver 3.
- Rust toolchain: pinned via `rust-toolchain.toml` in `pcloud-rs`.
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

### bd tracker

The `bd` tracker is the source of truth for open work items. Before
starting non-trivial work, check:

```bash
cd /home/ezechiel203/Projects/FORKS/pcloud-rs
bd list --status=open
```

## Daily Workflow

All commands run from the `pcloud-rs` directory.

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

Fuzz targets live under:

- `crates/pcloud-proto/fuzz` (7 targets: response/JSON/binary
  request parsers, IPC method decoder, listfolder parser, path
  canonicalizer, auth-flow state),
- `crates/pcloud-ipc/fuzz` (1 target: IPC frame decoder),
- `crates/pcloud-crypto/fuzz` (2 targets: `fuzz_open_sector` for
  the sector AEAD open path, and `fuzz_pclsync_filename_decode` for
  the pclsync reversible filename codec — audit-06 wave-2
  `bd-pcloud-rs-ncx.70`),
- `crates/pcloud-daemon/fuzz` (1 target:
  `fuzz_auth_vault_decode`, fuzzes the file-based auth-vault token
  parser — audit-06 wave-2 `bd-pcloud-rs-ncx.70`).

See [`TESTING-FUZZ-STRESS.md`](./TESTING-FUZZ-STRESS.md) for
`cargo fuzz run` invocations and corpus conventions.

### `#[ignore]`-gated tests

The workspace runs ~110 `#[ignore]`-annotated `#[test]` functions.
Each one is gated for a documented reason — none are hidden or
"temporarily broken". When you add a new `#[ignore]`, you **must**
include a `reason = "..."` string and document the gate here. The
fabrication rule in _Honesty Rules_ below is non-negotiable: if a
test fails, fix the code, do not add `#[ignore]` to hide it.

**Categories and why each is ignored**

| Category | Location(s) | Why ignored | How to run |
|----------|-------------|-------------|------------|
| Live pCloud auth / API | `crates/pcloud-daemon/tests/live_auth.rs`, `crates/pcloud-live-e2e/tests/*.rs` | Require real account credentials (`PCLOUD_LIVE_E2E=1` + `PCLOUD_TEST_USER` / `PCLOUD_TEST_PASSWORD`). Running them on every PR would leak tokens and rate-limit the test account. | `cargo test -p pcloud-live-e2e -- --ignored --test-threads=1` (with secrets in env) |
| Live Linux FUSE kernel mount | `crates/pcloud-fs/tests/fuse_{read,write,dyn_shim,kernel,lifecycle,small,mount}*.rs`, `crates/pcloud-fs/src/mount_service.rs`, `crates/pcloud-live-e2e/tests/mount_linux.rs` | Require `/dev/fuse` + kernel module + `fusermount3` binary. Sandboxed CI cannot satisfy this. | `PCLOUD_FUSE_TEST=1 cargo test -p pcloud-fs -- --ignored` (must be root or have fuse group) |
| Live macOS FUSE (fuse-t / macFUSE) | `crates/pcloud-fs/tests/macos_mount_live.rs`, `crates/pcloud-daemon/tests/macos_pcloud_live.rs` | Require `fuse-t` installed on a real macOS host. GitHub macos-latest runners do not ship fuse-t. | `PCLOUD_FUSE_TEST=1 cargo test -p pcloud-fs --test macos_mount_live -- --ignored` on macOS |
| macOS Keychain vault | `crates/pcloud-daemon/src/vault/keychain.rs` | Requires exclusive access to the login Keychain; parallel test runners corrupt state. | `cargo test -p pcloud-daemon vault::keychain -- --include-ignored` (macOS only) |
| pclsync compat known-answer tests (KAT) | `crates/pcloud-crypto/tests/pclsync_compat_kat_{live,offline}.rs` | Require extracted reference fixtures (`PCLOUD_KAT_PASSWORD` + fixture tarball). Offline KAT vectors are published out-of-band to avoid shipping encrypted fixtures in the repo. | `PCLOUD_KAT_PASSWORD=... cargo test -p pcloud-crypto --test pclsync_compat_kat_live -- --ignored` |
| Chaos / fault-injection | `crates/pcloud-chaos/tests/{disk_full_journal,sigkill_mid_flush,slowloris_timeout}.rs` | Long-running (seconds) and fault-inject real processes (SIGKILL subprocess, disk-full via FUSE overlay). Not safe on shared CI runners. | `cargo test -p pcloud-chaos -- --ignored` on a dedicated host |
| IPC stress | `crates/pcloud-ipc/tests/stress_concurrent_clients.rs` | 50 clients × 500 requests = 25 000 IPC round-trips; ~30–60 s wall time. Wired into the `ipc-stress` nightly CI job (audit-06 `bd-pcloud-rs-ncx.75`). | `cargo test --release -p pcloud-ipc -- --ignored stress` |
| Cross-platform IPC (Windows named-pipe) | `crates/pcloud-ipc/tests/platform_ipc_crossplat.rs` | The Windows named-pipe backend is a compile-only stub (Tier-3). Ignored until `bd-xplat-windows` lands a live accept loop. | — (not runnable until Windows IPC is live-verified) |
| Cross-process shared-memory | `crates/pcloud-compat/tests/cross_process_shm.rs`, `crates/pcloud-compat/src/shm_producer.rs` | Spawn a sibling process via `cargo run`; requires the workspace to already be built. | `cargo test -p pcloud-compat -- --ignored` |
| Mock-server flows | `crates/pcloud-mockserver/tests/mock_flows.rs` | Require a spawned mock HTTP server; long-running and port-binding. | `cargo test -p pcloud-mockserver -- --ignored` |
| Dev-only unit hints | `crates/pcloud-kms/src/lib.rs`, `crates/pcloud-backends/src/snapshot.rs`, `crates/pcloud-crypto/src/pclsync_{modes,sector}.rs` | Comments referencing `#[ignore]`, not actual ignored tests. Kept to document the project's **rejection** of placeholder tests: an empty `#[ignore]` body gives false "coverage" and is banned. | — (no-op) |

**CI coverage**

- `live-e2e` workflow job: runs the `pcloud-live-e2e` crate with
  `--ignored` on the weekly Sunday 02:00 UTC schedule and on
  `workflow_dispatch`. Gated on `PCLOUD_LIVE_E2E=1` +
  repository-level credentials. See `ci.yml` `live-e2e:` block.
- `ipc-stress` workflow job (audit-06 `bd-pcloud-rs-ncx.75`): runs
  `stress_concurrent_clients` on the weekly schedule with a 5-minute
  timeout. Linux-only.
- Local-only: everything else. Running these in CI would either
  require privileged hosts (FUSE), macOS hardware with fuse-t, or
  leak the live-pCloud account.

**When adding a new `#[ignore]`**

1. Annotate with `#[ignore = "…concrete reason…"]` — never bare
   `#[ignore]`.
2. Add a row to the table above (or expand an existing row).
3. Wire it into a CI job if it can run in a sandbox; otherwise
   document the hardware / credential requirement.
4. Never use `#[ignore]` to hide a failing test. Fix the code.

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
[`CLAUDE.md`](./CLAUDE.md). All contributors — human and agent —
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
5. [`CLAUDE.md`](./CLAUDE.md) if the global handoff state changed
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
