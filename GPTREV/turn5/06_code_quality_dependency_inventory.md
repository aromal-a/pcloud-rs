# pcloud-rs Code Quality / Dependency Inventory Audit - Turn 5

Date: 2026-04-30

Scope: read-only review of current dirty working tree after Turn 4 fixes. No files edited by the review agent.

## Executive Summary

Default build health is good: `fmt`, default `check`, default `clippy -D warnings`, MSRV Rust `1.85.0`, SDK examples, dead-code check, and `cargo deny` all pass. Several Turn 4 items are fixed: the invalid `cargo deny --all-features` workflow command is gone, `RefreshGuard` now uses `AtomicBool` instead of a poisonable mutex, and all workspace packages now report `rust-version`.

Remaining quality risks are concentrated in dependency/advisory posture, feature-combination CI, and test boundedness. Plain `cargo audit` still fails on default-enabled `rsa 0.9.10`; `--all-features` remains structurally impossible; `--no-default-features` builds but fails clippy; and the full workspace test run times out in RSA-4096 share E2E tests.

## Findings by Severity

### CRITICAL [0]

No critical code-quality/dependency finding confirmed in this pass.

### HIGH [1]

#### HIGH-1: Default build includes `rsa 0.9.10`, and plain `cargo audit` fails on RUSTSEC-2023-0071

Evidence:

- `Cargo.lock:4214-4215` locks `rsa 0.9.10`.
- `crates/pcloud-crypto/Cargo.toml:45-52` enables `pclsync-v2` by default, which pulls `dep:rsa`.
- `crates/pcloud-crypto/src/pclsync_rsa.rs:282-292` performs RSA-OAEP private-key decrypt.
- `deny.toml:27-40` and `audit.toml:11-20` explicitly accept the advisory, but plain `cargo audit` still exits `1`.
- `.github/workflows/security.yml:20-24` passes explicit `--ignore` flags, so CI policy and local plain-audit behavior differ.

Impact: default builds contain a known timing-side-channel RSA implementation on crypto share unwrap paths. Local/developer `cargo audit` fails unless the exact ignore flags are used.

Remediation: replace RustCrypto `rsa` with a constant-time maintained/provider-backed implementation, or move legacy RSA unwrap behind an explicit non-default compatibility feature. Add a documented local command matching CI, or place cargo-audit config where the installed cargo-audit actually reads it.

### MEDIUM [5]

#### MEDIUM-1: `--no-default-features` clippy fails in `pcloud-idp`

Evidence:

- `crates/pcloud-idp/Cargo.toml:15-16` makes `oidc-http-exchange` default-only.
- `crates/pcloud-idp/src/exchange.rs:35` imports `Duration` unconditionally.
- `crates/pcloud-idp/src/exchange.rs:37` imports `ExposeSecret` unconditionally.
- `crates/pcloud-idp/src/exchange.rs:112-119` uses those imports only inside `#[cfg(feature = "oidc-http-exchange")]`.

Command result: `cargo clippy --workspace --all-targets --no-default-features --locked -- -D warnings` fails with two unused imports.

Remediation: move those imports into `http_exchanger`, use fully qualified names there, or gate the imports with `#[cfg(feature = "oidc-http-exchange")]`. Add no-default clippy to CI.

#### MEDIUM-2: Workspace `--all-features` remains structurally impossible

Evidence:

- `crates/pcloud-crypto/src/lib.rs:59-70` intentionally fails when `crypto-provider-aws-lc-fips` is selected without a real provider.
- `crates/pcloud-crypto/src/lib.rs:71-80` intentionally fails when `crypto-provider-rustcrypto` and `crypto-provider-aws-lc-fips` are both enabled.
- `crates/pcloud-crypto/Cargo.toml:47` enables `crypto-provider-rustcrypto` by default.

Command result: `cargo check --workspace --all-targets --all-features --locked` fails at the compile guard.

Remediation: do not use naive `--all-features` as the feature gate. Install/use `cargo hack --feature-powerset` with explicit exclusions for invalid provider combinations, and document the supported feature matrix.

#### MEDIUM-3: Release builds abort on lock poison through shared `LockExt` hot paths

Evidence:

- `Cargo.toml:81-85` and `Cargo.toml:88-93` set `panic = "abort"` for release profiles.
- `crates/pcloud-observability/src/lock_ext.rs:67-87` panics on mutex poison.
- `crates/pcloud-observability/src/lock_ext.rs:122-146` panics on rwlock poison.
- Production-adjacent uses include `crates/pcloud-daemon/src/mount_runtime.rs:1008` and `crates/pcloud-sdk/src/upload_session.rs:396`.

Impact: a panic while holding a shared lock can terminate a release daemon instead of surfacing a typed subsystem failure.

Remediation: keep fail-fast poison handling only where invariants are unrecoverable. For daemon mount/upload paths, return typed errors, reset the affected subsystem, or isolate the panic behind a supervised restart boundary.

#### MEDIUM-4: Full workspace test run is not bounded; RSA share E2E timed out again

Evidence:

- `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs:60-63` states each test generates RSA-4096 keypairs.
- Test entry points are `crypto_share_rsa_e2e.rs:209`, `:291`, and `:356`.
- `timeout 120s env TMPDIR=/var/tmp cargo test --workspace --all-targets --locked` exited `124`; all three RSA tests reported running for over 60 seconds.

Remediation: use committed deterministic RSA fixtures for mock-backed tests, keep one ignored slow key-generation test if needed, and add per-test-binary CI timeouts.

#### MEDIUM-5: `unwrap` / `expect` lint remains disabled despite large source inventory

Evidence:

- `Cargo.toml:248-259` leaves `unwrap_used` and `expect_used` commented out.
- Raw inventory under `crates/**/src/**/*.rs`: 3,156 `.unwrap()` / `.expect()` matches.
- Highest-count files: `crates/pcloud-fs/src/write_path.rs` 261, `crates/pcloud-sdk/src/lib.rs` 175, `crates/pcloud-cli/src/app.rs` 157, `crates/pcloud-crypto/src/lib.rs` 129.

Remediation: enforce per-crate budgets first for daemon, IPC, auth, fs, crypto, and SDK. Then enable `clippy::unwrap_used` / `clippy::expect_used` as `warn`, with explicit test-only allowances.

### LOW [4]

#### LOW-1: `.ok()` silent-drop patterns remain in operational paths

Evidence:

- `crates/pcloud-daemon/src/serve.rs:565-568` treats invalid `PCLOUD_HEALTH_PORT` as port `0`.
- `crates/pcloud-fs/src/inode.rs:123-134` turns lock poison into `None`.
- `crates/pcloud-kms/src/lib.rs:249-250` treats KMS cache lock poison as cache miss.
- `crates/pcloud-fs/src/metadata_cache.rs:241-248` hides metadata cache lock poison.

Remediation: keep `.ok()` only for documented lossy parsing or best-effort cleanup. For locks/config/runtime state, log and return typed errors or use the project's explicit poison policy.

#### LOW-2: `cargo deny` passes but dependency hygiene warnings remain

Evidence:

- `cargo deny --locked check` exits `0`.
- Warning inventory: 20 `duplicate` warnings and 2 `license-not-encountered` warnings.
- Unmatched license allowances are in `deny.toml:60` and `deny.toml:68`.

Remediation: prune stale license allowances and keep reducing duplicate dependency families as upstreams converge.

#### LOW-3: Unmaintained advisory warnings are explicitly accepted

Evidence:

- `Cargo.lock:774-775` locks `bincode 2.0.1`, pulled through `crates/pcloud-policy/Cargo.toml:14` via `regorus`.
- `Cargo.lock:2958-2959` locks `paste 1.0.15`, pulled through optional PKCS#11 KMS at `crates/pcloud-kms/Cargo.toml:56-58`.
- `Cargo.lock:4313-4314` locks `rustls-pemfile 2.2.0`, directly used at `crates/pcloud-fleet/Cargo.toml:26`.

Remediation: keep time-boxed exceptions, migrate direct `rustls-pemfile` parsing first, and track upstream replacements for `regorus`/`cryptoki`.

#### LOW-4: MSRV metadata is fixed, but crate docs still claim Rust 1.82

Evidence:

- `cargo metadata` reports `missing=0 total=35` for package `rust_version`.
- Stale docs remain at `crates/pcloud-backends/src/lib.rs:18`, `crates/pcloud-daemon/src/lib.rs:24`, `crates/pcloud-fs/src/lib.rs:29`, `crates/pcloud-ipc/src/lib.rs:28`, `crates/pcloud-proto/src/lib.rs:54`, and `crates/pcloud-session/src/lib.rs:21`.

Remediation: update crate-level rustdoc MSRV text to Rust `1.85`. Consider a simple CI grep for stale MSRV strings.

## Inventory Summary

| Inventory | Count | Notes |
|---|---:|---|
| `.unwrap()` / `.expect()` in `crates/**/src` | 3,156 | Raw source count includes inline `#[cfg(test)]` modules and examples. Lints remain disabled. |
| `.ok()` conversions/drops | 175 | Several are benign parsers; lock/config paths still need triage. |
| `unsafe {}` / `unsafe fn` / `unsafe impl` / `unsafe extern` | 455 | Concentrated in macOS/Windows FUSE and IPC/compat FFI. Spot-checked high-risk callsites have `SAFETY` docs. |
| `TODO/FIXME/STUB/XXX/HACK/todo!/unimplemented!/panic!/unreachable!` | 186 | No new untracked production `unimplemented!()` found. |
| Rustdoc warnings | 41 observed | `cargo doc --workspace --no-deps --locked` passes but emits broken/private intra-doc links. |

## Commands / Results

| Command | Result |
|---|---|
| `cargo fmt --all --check` | Pass |
| `cargo check --workspace --all-targets --locked` | Pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Pass |
| `cargo deny --locked check` | Pass; 22 warnings |
| `cargo audit` | Fail; `RUSTSEC-2023-0071` on `rsa 0.9.10` |
| `cargo audit --deny warnings --ignore ...` | Pass with project exceptions |
| `cargo check --workspace --all-targets --all-features --locked` | Fail by `pcloud-crypto` provider compile guard |
| `cargo check --workspace --all-targets --no-default-features --locked` | Pass with `pcloud-idp` warnings |
| `cargo clippy --workspace --all-targets --no-default-features --locked -- -D warnings` | Fail on `pcloud-idp` unused imports |
| `cargo +1.85.0 check --workspace --all-targets --locked` | Pass |
| `RUSTFLAGS=-Ddead_code cargo check --workspace --all-targets --locked` | Pass |
| `cargo check -p pcloud-crypto --no-default-features --locked` | Pass |
| `cargo check -p pcloud-crypto --no-default-features --features crypto-provider-aws-lc-fips --locked` | Expected fail: FIPS seam not wired |
| `cargo check -p pcloud-kms --all-features --locked` | Pass with `TMPDIR=/var/tmp`; first attempt failed because `/tmp` was full |
| `cargo build -p pcloud-sdk --examples --locked` | Pass |
| `timeout 120s cargo test --workspace --all-targets --locked` | Timeout in RSA share E2E; partial earlier suites passed |
| `cargo doc --workspace --no-deps --locked` | Pass with rustdoc warnings |
| `cargo hack --version` | Not installed |
