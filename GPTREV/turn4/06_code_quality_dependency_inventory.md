# pcloud-rs Code Quality / Dependency Inventory Audit

Date: 2026-04-30

Read-only audit using `pcloud_rev.md` as the master prompt. No files were edited.

## Executive Summary

Default build health is good: `cargo fmt`, default `cargo check`, default `cargo clippy -D warnings`, MSRV `1.85.0` check, SDK examples, and `RUSTFLAGS=-Ddead_code` all pass. The dependency/security gate is not clean: plain `cargo audit` fails on `rsa 0.9.10` / `RUSTSEC-2023-0071`, and the GitHub security workflow contains an invalid `cargo deny check --all-features` invocation for the installed cargo-deny CLI.

The main quality risks are advisory handling around default-enabled RSA crypto, an impossible `cargo --all-features` build because mutually exclusive crypto-provider features are exposed to Cargo, weak MSRV enforcement across most member crates, and production code paths that still convert poisoned locks or errors into silent `None`/`false` outcomes.

## Findings by Severity

### CRITICAL [0]

No critical code-quality/dependency finding was confirmed in this pass.

### HIGH-1: `cargo audit` fails on default-enabled `rsa 0.9.10` Marvin timing advisory

Evidence: `cargo audit` exits `1` on `RUSTSEC-2023-0071`, `rsa 0.9.10`, Marvin Attack timing side channel, no fixed upgrade available. `crates/pcloud-crypto/Cargo.toml:28` declares optional `rsa`. `crates/pcloud-crypto/Cargo.toml:46` enables `pclsync-v2` by default, making RSA present in default builds. `crates/pcloud-crypto/src/pclsync_rsa.rs:282-292` performs RSA-OAEP private-key decrypt. `deny.toml:28-39` explicitly ignores the advisory, but `audit.toml:8-26` does not ignore `RUSTSEC-2023-0071`, so local audit and deny policy diverge.

Impact: default builds include a known vulnerable RSA implementation on crypto share unwrap paths, and CI/developer environments that run plain `cargo audit` fail.

Remediation: replace `rsa` with a constant-time maintained implementation or externally reviewed provider. If no fixed crate exists, isolate RSA unwrap behind an explicit non-default compatibility feature and live-test waiver. Keep `deny.toml` and `audit.toml` synchronized, or remove `audit.toml` claims that it mirrors deny policy.

### HIGH-2: Security workflow uses invalid `cargo deny check --all-features`

Evidence: `.github/workflows/security.yml:47` runs `cargo deny check --all-features`. `.github/workflows/security.yml:56` runs `cargo deny --format sarif check --all-features`. Local `cargo deny 0.19.0` returns `error: unexpected argument '--all-features' found`. `deny.toml:4-5` already configures `[graph] all-features = true`.

Impact: the security workflow's deny job is likely broken or version-fragile.

Remediation: remove `--all-features` from cargo-deny workflow commands. Keep all-feature graph selection in `deny.toml`. Add local CI parity commands `cargo deny check` and `cargo deny --format sarif check`.

### HIGH-3: Auth refresh single-flight guard silently suppresses poisoned-lock errors

Evidence: `crates/pcloud-auth/src/lifecycle.rs:252` uses `self.in_flight.lock().ok()?`, returning `None` on poison as if another refresh is already running. `crates/pcloud-auth/src/lifecycle.rs:266` maps lock failure to `false`. `crates/pcloud-auth/src/lifecycle.rs:283-285` ignores lock failure in `Drop`, so the refresh slot may remain stuck.

Impact: a panic while holding the refresh guard can permanently suppress token refresh and force avoidable logout/session expiry in the daemon.

Remediation: replace `Option` return with `Result<Option<RefreshTicket>, RefreshGuardError>`, log poison distinctly and either fail closed or recover with `into_inner()` after documenting invariants, or replace this mutex with `AtomicBool` compare-exchange to avoid poison semantics.

### MEDIUM-1: Workspace `--all-features` build is structurally impossible

Evidence: `cargo check --workspace --all-targets --all-features --locked` fails. `crates/pcloud-crypto/src/lib.rs:71-80` emits `compile_error!` when `crypto-provider-rustcrypto` and `crypto-provider-aws-lc-fips` are both enabled. `crates/pcloud-crypto/src/lib.rs:59-70` emits `compile_error!` when FIPS is selected alone. `crates/pcloud-crypto/Cargo.toml:46` makes `crypto-provider-rustcrypto` default.

Impact: standard Rust gates like `cargo check --all-features` and `cargo clippy --all-features` cannot be used.

Remediation: use `cargo hack --feature-powerset` with explicit exclusions for invalid provider combinations, or move FIPS provider selection out of Cargo features into build-time cfg/env validation. Document the exact supported feature matrix in CI.

### MEDIUM-2: MSRV is verified locally but not enforced across most crates

Evidence: `Cargo.toml:63-68` declares workspace `rust-version = "1.85"`. `cargo metadata` reports `rust_version` unset for `32 / 35` workspace packages. `crates/pcloud-model/Cargo.toml:1-7` lacks `rust-version.workspace = true`. `crates/pcloud-crypto/Cargo.toml:1-7` lacks `rust-version.workspace = true`. `crates/pcloud-daemon/src/lib.rs:24` still documents `MSRV: Rust 1.82`.

Impact: Cargo does not enforce MSRV for most member crates, and published/standalone crate consumers may get inconsistent metadata.

Remediation: add `rust-version.workspace = true` to every workspace crate manifest, add CI job running `cargo +1.85.0 check --workspace --all-targets --locked`, and update stale rustdoc MSRV references.

### MEDIUM-3: Unwrap/expect lint is intentionally disabled despite large inventory

Evidence: inventory found `3,085` `.unwrap()` / `.expect()` matches under `crates/**/src/**/*.rs`. `Cargo.toml:241-257` documents staged rollout and leaves `unwrap_used` / `expect_used` commented out. Highest-count files include `crates/pcloud-fs/src/write_path.rs` with 251, `crates/pcloud-sdk/src/lib.rs` with 172, `crates/pcloud-cli/src/app.rs` with 153, and `crates/pcloud-crypto/src/lib.rs` with 129.

Impact: panics can remain hidden in production-adjacent source because CI does not warn on new unwraps.

Remediation: enable `clippy::unwrap_used` and `clippy::expect_used` at least as `warn` for daemon, IPC, auth, fs, crypto. Allow tests with `#[cfg(test)]` or per-module allowances. Track per-crate burn-down with CI budgets.

### MEDIUM-4: Production lock-poison policy can abort release builds

Evidence: `Cargo.toml:81-85` sets release `panic = "abort"`. `crates/pcloud-observability/src/lock_ext.rs:67-87` logs then panics on mutex poison, and `crates/pcloud-observability/src/lock_ext.rs:122-146` panics on rwlock poison. Production callers include `crates/pcloud-daemon/src/mount_runtime.rs:1008` and `crates/pcloud-sdk/src/upload_session.rs:395`.

Impact: a single poisoned lock can terminate a release daemon instead of degrading or restarting a subsystem.

Remediation: keep panic-on-poison only where invariants are explicitly unrecoverable. For daemon hot paths, return typed errors or trigger controlled subsystem restart. Add a policy lint/check so new `lock_or_poisoned` uses require justification.

### MEDIUM-5: Unsafe inventory still has weakly documented call sites

Evidence: inventory found `450` `unsafe {}` / `unsafe fn` / `unsafe impl` / `unsafe extern` matches. `crates/pcloud-cli/src/main.rs:1252` and `crates/pcloud-cli/src/main.rs:1395` call `libc::kill` without adjacent `SAFETY:` comments. `crates/pcloud-cli/src/main.rs:1766` enters `pre_exec` unsafe block with a general detach comment but no explicit `SAFETY:` invariant. `crates/pcloud-cli/src/prompt.rs:187` and `crates/pcloud-cli/src/prompt.rs:194` lack adjacent `SAFETY:` comments for `assume_init` / `tcsetattr`.

Impact: most major FFI areas are documented, but review discipline is not uniformly enforced.

Remediation: add `SAFETY:` comments to every unsafe block, not just complex FFI modules. Add a lightweight script/CI check for unsafe blocks without nearby `SAFETY`.

### MEDIUM-6: Full workspace test run is not bounded; RSA E2E tests exceeded local review budget

Evidence: `cargo test --workspace --all-targets --locked` was interrupted after RSA share tests exceeded 60 seconds and continued long-running. `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs:58-63` states each test generates RSA-4096 keys. Test entry points are at `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs:209`, `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs:291`, and `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs:356`.

Impact: default test runs can stall on CPU-heavy crypto integration tests.

Remediation: use committed deterministic RSA test fixtures for mock-backed tests. Keep one ignored/live/slow RSA keygen test if key generation itself must be exercised. Add CI timeout per test binary.

### LOW-1: cargo-deny passes but still emits dependency hygiene warnings

Evidence: `cargo deny --locked check` exits `0` but warns about 20 duplicate dependencies and 2 license-not-encountered entries. Duplicate lock entries include `Cargo.lock:491`, `Cargo.lock:512`, `Cargo.lock:1550`, `Cargo.lock:1556`, `Cargo.lock:1816`, `Cargo.lock:1825`, and `Cargo.lock:1836`.

Remediation: continue pruning stale skip entries, track duplicate families by upstream blocker, and remove skips as upstreams converge.

### LOW-2: Unmaintained advisory warnings remain in lockfile

Evidence: `cargo audit` reports unmaintained `bincode 2.0.1` via `regorus`, `paste 1.0.15` via `cryptoki`, and `rustls-pemfile 2.2.0`. `crates/pcloud-policy/Cargo.toml:13` depends on `regorus = "0.9"`, `crates/pcloud-kms/Cargo.toml:56-58` enable `cryptoki` under `pkcs11`, and `crates/pcloud-fleet/Cargo.toml:26` depends on `rustls-pemfile = "2"`.

Remediation: track upstream replacement timelines; for direct `rustls-pemfile`, migrate to `rustls-pki-types` parsing if feasible; for optional `pkcs11`, document advisory scope and avoid enabling by default.

### LOW-3: `.ok()` inventory includes silent error conversion in operational paths

Evidence: inventory found `172` `.ok()` matches. `crates/pcloud-daemon/src/serve.rs:561-564` silently treats invalid `PCLOUD_HEALTH_PORT` as disabled health server. `crates/pcloud-fs/src/inode.rs:124` and `crates/pcloud-fs/src/inode.rs:133` return `None` on poisoned inode-table locks. `crates/pcloud-fs/src/mount_service.rs:704` hides `LAST_DROP_ERROR` lock poison.

Remediation: keep documented infallible `.ok()` cases; replace operational `.ok()` with typed errors or warning logs; add an allowlist for intentional `.ok()` drops.

### LOW-4: Build emits recurring vendored password dictionary warning

Evidence: `crates/pcloud-crypto/build.rs:55-67` copies the vendored dictionary and emits `cargo:warning`. Every checked cargo build emitted the vendored password dictionary warning.

Remediation: downgrade to `cargo:warning` only under an explicit audit env var, or document the warning as expected in CI. Add a test that verifies vendored dictionary hash instead of warning every build.

## Inventory Summary

| Inventory | Count | Notes |
|---|---:|---|
| `.unwrap()` / `.expect()` in `crates/**/src` | 3,085 | Includes in-source test modules; lints disabled in `Cargo.toml:241-257`. |
| `.ok()` drops/conversions | 172 | Several are documented infallible drops; auth/fs operational cases need cleanup. |
| `unsafe` blocks/functions/impls/externs | 450 | Concentrated in FUSE/macOS/Windows/IPC FFI. |
| `TODO/FIXME/STUB/XXX/HACK/todo!/unimplemented!/panic!/unreachable!` | 183 | PCRE scan found no clear untracked actionable TODO; false positives were docs. |
| `panic!/unreachable!` matches | 124 | Mostly tests; production `LockExt` panics are intentional but availability-sensitive. |

## Commands / Results

| Command | Result |
|---|---|
| `cargo fmt --all --check` | Pass |
| `cargo check --workspace --all-targets --locked` | Pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Pass |
| `cargo check --workspace --all-targets --all-features --locked` | Fail by `pcloud-crypto` provider `compile_error!` |
| `cargo deny --locked check` | Pass with warnings |
| `cargo audit` | Fail: `RUSTSEC-2023-0071` on `rsa 0.9.10`; 3 allowed warnings |
| `cargo test --workspace --all-targets --locked` | Interrupted after RSA E2E tests ran long; partial earlier crates passed |
| `cargo check -p pcloud-crypto --no-default-features --locked` | Pass |
| `cargo check -p pcloud-crypto --no-default-features --features crypto-provider-aws-lc-fips --locked` | Expected fail: FIPS seam not implemented |
| `cargo check -p pcloud-kms --all-features --locked` | Pass |
| `cargo build -p pcloud-sdk --examples --locked` | Pass |
| `cargo +1.85.0 check --workspace --all-targets --locked` | Pass |
| `RUSTFLAGS='-Ddead_code' cargo check --workspace --all-targets --locked` | Pass |
| `cargo deny check --all-features` | Fail: invalid cargo-deny argument |

## Remediation Roadmap

1. Fix advisory posture first: address or explicitly isolate `rsa` `RUSTSEC-2023-0071`, and make `cargo audit` / `cargo deny` policy consistent.
2. Repair security CI by removing invalid `cargo deny --all-features` usage.
3. Add crate-level `rust-version.workspace = true` everywhere and add an MSRV CI job.
4. Replace the auth refresh mutex poison behavior with explicit error handling or atomics.
5. Add feature-matrix CI that avoids invalid crypto-provider combinations.
6. Start unwrap/expect and `.ok()` burn-down on daemon/auth/fs/IPC before broad workspace lint escalation.
