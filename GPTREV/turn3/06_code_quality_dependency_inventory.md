# pcloud-rs Enterprise Readiness Audit Report
## Date: 2026-04-30
## Auditor: Turn 3 subagent 06

## Executive Summary
This read-only audit focused on repo-wide code quality, robustness, dependency/license/advisory posture, and feature/MSRV readiness. Default `cargo check` and `cargo clippy` pass, but the repository is not enterprise-gate clean: `cargo fmt --all --check` fails, `cargo check --workspace --all-features` fails by design, and `cargo audit` fails on a reachable crypto advisory.

The strongest positives are that most crates forbid unsafe code, the workspace has `cargo-deny`, CI covers Linux/macOS/Windows plus optional feature smoke checks, and much of the TODO debt is bead-tagged. The blockers are the known RSA advisory in production crypto paths, stale/inconsistent security gates, a moving stable toolchain instead of a real MSRV gate, and large un-enforced panic/silent-drop debt.

## Findings by Severity
### CRITICAL [1]
### HIGH [5]
### MEDIUM [7]
### LOW [3]

## Detailed Findings

### CRITICAL

#### CRIT-01: Known RustSec RSA timing vulnerability is reachable from production crypto code
Severity: CRITICAL

Evidence: `Cargo.lock:4211` pins `rsa 0.9.10`; `deny.toml:28` to `deny.toml:39` explicitly ignores `RUSTSEC-2023-0071`; production crypto uses RSA-OAEP unwrap in `crates/pcloud-crypto/src/pclsync_rsa.rs:282` and calls it from `crates/pcloud-crypto/src/lib.rs:2636` / `crates/pcloud-crypto/src/lib.rs:2678`. `cargo audit` exited 1 with `RUSTSEC-2023-0071 Marvin Attack`.

Impact: Enterprise builds ship a known timing side-channel advisory in cryptographic key unwrap/share paths. Even if exploitation is bounded operationally, this blocks a defensible enterprise security posture and causes the standard RustSec audit gate to fail.

Remediation: Remove the `rsa` dependency from production release paths until a constant-time backend is available, or replace it with a vetted constant-time provider. If legacy interop requires RSA-OAEP, isolate it behind an explicit enterprise risk flag, hard-disable attacker-controlled decrypt loops, and make `cargo audit`/`cargo deny` fail unless a current, reviewed exception is approved.

### HIGH

#### HIGH-01: `cargo fmt --all --check` fails on the current tree
Severity: HIGH

Evidence: `cargo fmt --all --check` returned exit code 1. Reported diffs include `crates/pcloud-backends/src/transfer_backend.rs:1030`, `crates/pcloud-cli/src/app.rs:1081`, `crates/pcloud-cli/src/main.rs:1731`, `crates/pcloud-daemon/src/runtime.rs:796`, `crates/pcloud-engine/src/planner.rs:113`, and `crates/pcloud-fs/src/write_path.rs:1130`.

Impact: The primary Linux CI job runs `cargo fmt --all --check` at `.github/workflows/ci.yml:31`, so the current tree is CI-blocked before tests, clippy, or deny can run.

Remediation: Run `cargo fmt --all`, review the formatting-only diff, and add a pre-submit or pre-commit check so formatting drift cannot land.

#### HIGH-02: All-features compile is not clean
Severity: HIGH

Evidence: `CARGO_TARGET_DIR=/tmp/pcloud-rs-target-turn3-06 cargo check --workspace --all-features` failed at `crates/pcloud-crypto/src/lib.rs:75` because `crypto-provider-rustcrypto` and `crypto-provider-aws-lc-fips` are mutually exclusive. The placeholder FIPS feature is documented in `crates/pcloud-crypto/Cargo.toml:68` to `crates/pcloud-crypto/Cargo.toml:81`. CI optional feature checks in `.github/workflows/ci.yml:177` to `.github/workflows/ci.yml:197` do not cover this all-features matrix.

Impact: The workspace cannot satisfy the audit requirement that feature combinations compile. Operators and downstream packagers using broad `--all-features` validation will fail.

Remediation: Exclude compile-error placeholder features from all-features validation using a documented CI matrix, or restructure the FIPS seam so `--all-features` is not a supported command and CI explicitly proves every supported feature set.

#### HIGH-03: Security gate behavior is inconsistent across `cargo audit`, `cargo deny`, and CI
Severity: HIGH

Evidence: `cargo audit` fails on `rsa 0.9.10` plus warns on unmaintained `bincode 2.0.1`, `paste 1.0.15`, and `rustls-pemfile 2.2.0` (`Cargo.lock:774`, `Cargo.lock:2958`, `Cargo.lock:4310`). `cargo deny check` exits 0 because `deny.toml:27` to `deny.toml:48` ignores two advisories and `deny.toml:20` scopes unmaintained warnings. `.github/workflows/security.yml:47` and `.github/workflows/security.yml:56` invoke `cargo deny ... --all-features`; with local `cargo-deny 0.19.0`, `cargo deny check --all-features` is an invalid invocation.

Impact: The project can appear "deny clean" while `cargo audit` fails. The security workflow may also fail or produce no useful SARIF depending on the cargo-deny version semantics.

Remediation: Pick one authoritative advisory gate and make local and CI behavior identical. Remove stale `--all-features` CLI flags from cargo-deny if `[graph] all-features = true` is the intended mechanism, and require reviewed exceptions to expire or be revalidated by CI.

#### HIGH-04: Production code still silently drops meaningful errors
Severity: HIGH

Evidence: The filtered production-prefix inventory found 435 `.ok()` / `let _ =` silent-drop sites. High-risk examples include ignored config persistence at `crates/pcloud-cli/src/main.rs:359`, ignored audit persistence at `crates/pcloud-daemon/src/serve.rs:621`, ignored IPC response write in `crates/pcloud-ipc/src/transport.rs:907`, best-effort web-token permission hardening at `crates/pcloud-web/src/lib.rs:324`, and mount pidfile permission/removal drops at `crates/pcloud-daemon/src/mount_runtime.rs:369` and `crates/pcloud-daemon/src/mount_runtime.rs:382`.

Impact: Configuration, audit, IPC, and cleanup failures can be hidden from callers and operators. In enterprise deployments this weakens forensic guarantees and makes recovery from partial failures nondeterministic.

Remediation: Replace silent drops with typed propagation where callers can act, structured `warn!`/`error!` plus counters where best-effort is intentional, and tests that assert failures are surfaced for audit/config/security-sensitive paths.

#### HIGH-05: MSRV is declared but not actually verified
Severity: HIGH

Evidence: Workspace MSRV is `1.85` in `Cargo.toml:68`, but `rust-toolchain.toml:2` uses moving `stable`. CI also installs `dtolnay/rust-toolchain@stable` at `.github/workflows/ci.yml:25`. Local verification ran on `rustc 1.94.1`; no `1.85` toolchain was installed. Several crate docs still claim Rust 1.82, e.g. `crates/pcloud-daemon/src/lib.rs:24`, `crates/pcloud-fs/src/lib.rs:29`, `crates/pcloud-proto/src/lib.rs:54`.

Impact: The project may accidentally adopt APIs newer than 1.85 while CI remains green. Enterprise packagers cannot rely on the documented MSRV.

Remediation: Add an explicit `1.85` CI job for `cargo check --workspace --all-targets` and selected feature sets. Update stale rustdoc MSRV comments to `1.85` or remove per-crate MSRV claims in favor of the workspace manifest.

### MEDIUM

#### MED-01: Panic and unwrap discipline is not machine-enforced
Severity: MEDIUM

Evidence: `Cargo.toml:241` to `Cargo.toml:257` documents that `unwrap_used` and `expect_used` remain commented out. Raw inventory found 3,085 `unwrap/expect` hits under `crates/**/src`; filtered pre-test, non-comment inventory still found 72 `unwrap/expect`, 5 `panic!/unreachable!`, and 25 assert macro sites. Examples include thread-spawn panics at `crates/pcloud-daemon/src/integrity_sweeper_service.rs:815` and `crates/pcloud-daemon/src/integrity_sweeper_service.rs:1082`, lock-poison panics at `crates/pcloud-observability/src/lock_ext.rs:87`, and RNG panic wrappers at `crates/pcloud-web/src/lib.rs:164`.

Impact: Clippy passing does not mean panic-safe production code. Some panics are justified, but they remain policy-by-comment rather than policy-by-lint.

Remediation: Start with `clippy::unwrap_used` and `clippy::expect_used` as `warn` per crate, allow justified invariants locally with comments, and make daemon/IPC/FUSE crates deny reachable panics first.

#### MED-02: Unsafe surface is large and not uniformly documented at the unsafe site
Severity: MEDIUM

Evidence: Most crates use `#![forbid(unsafe_code)]`, but unsafe-enabled crates include `pcloud-cli`, `pcloud-ipc`, `pcloud-daemon`, `pcloud-fs`, and `pcloud-compat`. Inventory found 491 raw unsafe tokens and 445 pre-test unsafe tokens. Suspicious local gaps include `crates/pcloud-cli/src/main.rs:1252`, `crates/pcloud-cli/src/main.rs:1395`, `crates/pcloud-cli/src/main.rs:1769`, and `crates/pcloud-cli/src/prompt.rs:187`. Some FFI impls are well documented, e.g. `crates/pcloud-fs/src/mount_service.rs:395` and `crates/pcloud-fs/src/platform/winfsp_ffi.rs:488`.

Impact: Unsafe invariants are review-critical. Missing local `SAFETY:` notes make later audits and refactors more error-prone, especially around signals, terminal state, and FFI.

Remediation: Require a `SAFETY:` comment immediately adjacent to every unsafe block/impl, and upgrade `unsafe_op_in_unsafe_fn` from warn to deny in unsafe-enabled crates after cleanup.

#### MED-03: Test/mock fixtures are exported in production public APIs
Severity: MEDIUM

Evidence: `crates/pcloud-backends/src/lib.rs:155` exports `pub mod mock`; `crates/pcloud-backends/src/mock.rs:1` identifies it as shared mock primitives; backend mock modules are public, e.g. `crates/pcloud-backends/src/auth_backend.rs:521` and `crates/pcloud-backends/src/public_link_backend.rs:1221`. `crates/pcloud-fs/src/backend.rs:946` to `crates/pcloud-fs/src/backend.rs:949` explicitly exposes mock backends publicly and not under `#[cfg(test)]`.

Impact: Production consumers can accidentally depend on test fixtures, and mock-only poison/panic policies become part of the public API surface.

Remediation: Gate mocks behind a `test-helpers` feature that is off by default and excluded from production builds. Keep integration tests using dev-dependency features rather than unconditional public modules.

#### MED-04: Raw ID use remains widespread despite newtypes
Severity: MEDIUM

Evidence: Newtypes exist in `crates/pcloud-model/src/ids.rs:36` to `crates/pcloud-model/src/ids.rs:128`, but IPC/proto/SDK surfaces still expose many raw `u64` identifiers: `crates/pcloud-ipc/src/methods.rs:1159`, `crates/pcloud-ipc/src/methods.rs:1235`, `crates/pcloud-proto/src/transfer_api.rs:103`, `crates/pcloud-proto/src/public_links_api.rs:284`, `crates/pcloud-sdk/src/lib.rs:245`, and `crates/pcloud-sdk/src/upload_session.rs:254`.

Impact: Folder IDs, file IDs, link IDs, upload IDs, and sync IDs can be confused at compile time, especially across IPC and SDK boundaries.

Remediation: Use newtypes internally and convert to raw protocol numbers only at serialization boundaries. Add serde-transparent wrappers for stable wire compatibility.

#### MED-05: Drop/resource cleanup still hides failure paths
Severity: MEDIUM

Evidence: `MountHandle::drop` logs and stores Linux unmount failures at `crates/pcloud-fs/src/mount_service.rs:641`, which is good. In contrast, `MountControl::drop` explicitly swallows ordered-shutdown errors at `crates/pcloud-daemon/src/mount_runtime.rs:760` to `crates/pcloud-daemon/src/mount_runtime.rs:776`; `BoundIpcServer::drop` silently removes the socket at `crates/pcloud-ipc/src/transport.rs:625`.

Impact: Process-exit mount and socket cleanup failures can leave orphaned resources without a health/status signal.

Remediation: Reuse the `last_drop_error` pattern for daemon mount control and IPC socket cleanup, and expose cleanup failures in health/doctor output.

#### MED-06: License/advisory policy file is stale and internally contradictory
Severity: MEDIUM

Evidence: `deny.toml:65` to `deny.toml:68` allow `Apache-2.0 WITH LLVM-exception`, `CC0-1.0`, `BSL-1.0`, and `OpenSSL`, while `deny.toml:81` to `deny.toml:85` says several of those are blocked. `cargo deny check` warned about unmatched license allowances and many unmatched/unnecessary skip entries such as `deny.toml:138` to `deny.toml:151`.

Impact: Reviewers cannot tell which licenses and duplicate dependency families are intentional. Stale skip entries hide whether dependency hygiene has improved or regressed.

Remediation: Regenerate deny configuration against the current lockfile, remove stale skip entries, and make the allow/block comments exactly match enforced policy.

#### MED-07: TODO/STUB debt is tracked but still production-significant
Severity: MEDIUM

Evidence: Raw inventory found 46 TODO/STUB/FIXME/HACK markers under `crates/**/src`; filtered pre-test inventory found 42. Examples include upload resumability gaps at `crates/pcloud-daemon/src/transfer_bridge.rs:217`, API parity follow-ups at `crates/pcloud-proto/src/methods/upload.rs:69`, FUSE platform work at `crates/pcloud-fs/src/platform/bsd.rs:46`, sandboxing at `crates/pcloud-daemon/src/bootstrap.rs:415`, and integrity bootstrap follow-up at `crates/pcloud-daemon/src/runtime.rs:6784`. Negative grep did not find untracked TODOs without a bead-like reference, excluding a non-gap `unimplemented!()` wording comment.

Impact: Tracker discipline is mostly present, but production claims must continue to exclude these incomplete areas until their beads close.

Remediation: Keep TODOs bead-linked, add owner/phase metadata for parity-blocking TODOs, and fail CI on new untracked TODO/FIXME/STUB markers.

### LOW

#### LOW-01: Clippy is clean but broad lint allows reduce signal
Severity: LOW

Evidence: `cargo clippy --workspace --all-targets -- -D warnings` passed. However, workspace clippy allows broad pedantic categories at `Cargo.toml:220` to `Cargo.toml:239`, and unwrap/expect lints are intentionally disabled at `Cargo.toml:256` to `Cargo.toml:257`.

Impact: "Clippy clean" currently means clean under a permissive policy, not production-hardening clean.

Remediation: Track a staged tightening plan per crate, beginning with daemon, IPC, FUSE, crypto, and transport.

#### LOW-02: Vendored crypto password dictionary warning persists
Severity: LOW

Evidence: `cargo check` and `cargo clippy` emitted `pcloud-crypto: using vendored password dictionary ... legacy C header ... not present`.

Impact: The build is reproducible, but provenance is weaker than a directly verified upstream parity source.

Remediation: Document the vendored dictionary provenance and checksum in the crypto crate, or replace it with generated source plus a reproducible generation script.

#### LOW-03: Cargo-deny duplicate dependency policy is intentionally warning-only
Severity: LOW

Evidence: `deny.toml:121` sets `multiple-versions = "warn"`. `cargo deny check` emitted duplicate warnings for AWS, hashbrown/foldhash, Windows, zbus/zvariant, rand/getrandom, and other families.

Impact: This is acceptable as a temporary hygiene posture, but it increases audit noise and dependency attack surface.

Remediation: Keep the documented skip list current and promote duplicate families to deny as upstream stacks converge.

## Commands Run

- `sed -n '1,620p' pcloud_rev.md`
- `sed -n '1,560p' Cargo.toml`
- `sed -n '1,260p' deny.toml`
- `sed -n '1,260p' .github/workflows/ci.yml`
- `sed -n '1,260p' .github/workflows/security.yml`
- `sed -n '1,220p' .github/workflows/fuzz.yml`
- `cargo fmt --all --check` failed.
- `CARGO_TARGET_DIR=/tmp/pcloud-rs-target-turn3-06 cargo check --workspace --all-targets` passed.
- `CARGO_TARGET_DIR=/tmp/pcloud-rs-target-turn3-06 cargo clippy --workspace --all-targets -- -D warnings` passed.
- `CARGO_TARGET_DIR=/tmp/pcloud-rs-target-turn3-06 cargo deny check --all-features` failed locally because cargo-deny 0.19.0 rejects `--all-features`.
- `CARGO_TARGET_DIR=/tmp/pcloud-rs-target-turn3-06 cargo deny check` passed with warnings.
- `cargo audit` failed on `RUSTSEC-2023-0071` and reported three unmaintained warnings.
- `CARGO_TARGET_DIR=/tmp/pcloud-rs-target-turn3-06 cargo check --workspace --all-features` failed on the FIPS feature compile error.
- `rustc --version` reported `rustc 1.94.1`; `rustup toolchain list` showed no 1.85 toolchain installed.

## Limitations

No files were modified and no `AUDIT_REPORT.md` was written. I did not run live tests, `cargo test --workspace`, cross-compilation, or an actual Rust 1.85 MSRV build. Inventory counts were generated from `crates/**/src/**/*.rs` with generated/excluded directories omitted; filtered counts excluded comments and the first `#[cfg(test)]`/`mod tests` suffix per file, so they should be treated as audit triage counts rather than a compiler-accurate reachability proof.
