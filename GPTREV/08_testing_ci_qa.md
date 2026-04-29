# pcloud-rs Testing / CI / QA Audit Report
## Date: 2026-04-29
## Auditor: Subagent 08

## Executive Summary
The repository has a broad QA surface: integration tests, live-E2E harnesses, proptests, fuzz targets, and Criterion benches are present. However, enterprise readiness is blocked because several of the highest-value gates are advisory, skipped, manual-only, or stale relative to documentation claims.

I found **0 CRITICAL**, **8 HIGH**, and **4 MEDIUM** testing/QA findings. No files were modified.

## Findings by Severity
### CRITICAL: 0

### HIGH: 8

### H-01 Live E2E Is Advisory And Often Soft-Skips
Severity: HIGH  
Evidence: `.github/workflows/ci.yml:269-288` runs live E2E only on manual/schedule, with `continue-on-error: true`; it provides only `PCLOUD_TEST_USER` / `PCLOUD_TEST_PASSWORD`. Many families require extra env such as crypto password, peer accounts, fleet controller, GPG, and FUSE opt-in (`crates/pcloud-live-e2e/README.md:107-116`; examples: `crates/pcloud-live-e2e/tests/crypto.rs:39`, `shares.rs:95`, `mount_linux.rs:74`, `fleet_mtls.rs:54`).  
Impact: CI can be green while crypto, sharing, FUSE, fleet, and backup live verification did not run.  
Remediation: Make live E2E a protected, singleton, failing gate for release candidates; provision required envs; emit a skip summary and fail if required parity families skip.

### H-02 `sync_loop_live` Is Never Exercised By The Live Job
Severity: HIGH  
Evidence: The live-E2E README promises every test is `#[ignore]` (`crates/pcloud-live-e2e/README.md:11-15`), and CI runs only `--ignored` tests (`.github/workflows/ci.yml:283-288`). But `crates/pcloud-live-e2e/tests/sync_loop_live.rs:36-37` is a plain `#[test]`, not ignored, so the live job excludes it. It also returns before real work when no gate/token exists (`sync_loop_live.rs:38-40`, `sync_loop_live.rs:59-61`).  
Impact: Background sync-loop live coverage is effectively absent.  
Remediation: Add `#[ignore]`, authenticate via the shared helper, run it in the live job, and assert a real remote-visible sync result or explicitly demote the claim.

### H-03 Mounted-Drive / FUSE Proof Is Manual And Soft-Skipped
Severity: HIGH  
Evidence: CI's Linux test uses `cargo test --workspace`, which skips ignored FUSE tests (`.github/workflows/ci.yml:35-36`). Core FUSE live tests are ignored (`crates/pcloud-fs/tests/fuse_write_path_live.rs:241`, `fuse_kernel_e2e.rs:206`, `fuse_read_path_live.rs:120`) and return success on missing gates or mount refusal (`fuse_write_path_live.rs:243-250`, `fuse_write_path_live.rs:285-287`, `fuse_write_path_live.rs:294-297`).  
Impact: FUSE regressions can pass CI despite matrix/docs relying on gated mount proof.  
Remediation: Add a dedicated Linux FUSE runner with `/dev/fuse`; fail on skip in that job; preserve soft-skip only for developer machines.

### H-04 Cross-Platform CI Does Not Match Tier Claims
Severity: HIGH  
Evidence: macOS excludes `pcloud-fs` from workspace tests and runs only selected mock/unit pcloud-fs tests (`.github/workflows/ci.yml:49-61`). Windows excludes `pcloud-fs` entirely (`.github/workflows/ci.yml:70-71`). FreeBSD is `continue-on-error` and excludes `pcloud-fs` (`.github/workflows/ci.yml:79-91`). Docs still list macOS and Windows as T1 in the platform matrix (`docs/book/src/architecture/platform-support.md:20`) while README says macOS/Windows are Tier 2 (`README.md:3-4`).  
Impact: Platform-specific mount, IPC, and packaging regressions can merge while platform support claims remain ambiguous.  
Remediation: Align tier docs, make claimed tier platforms blocking, and add self-hosted or dedicated runners for macOS FUSE, WinFSP, and FreeBSD mount paths.

### H-05 Fuzzing Is Non-Gating And Missing Targets
Severity: HIGH  
Evidence: All fuzz jobs are `continue-on-error: true` (`.github/workflows/fuzz.yml:27-30`, `50-53`, `79-82`). The workflow runs only `fuzz_open_sector` for crypto (`.github/workflows/fuzz.yml:37-40`) and omits `fuzz_pclsync_filename_decode` (`crates/pcloud-crypto/fuzz/Cargo.toml:24-26`) and the daemon vault fuzzer (`crates/pcloud-daemon/fuzz/Cargo.toml:19-21`). `fuzz/README.md:3-9` also references a non-existent `.github/workflows/rust.yml` and overclaims target auto-discovery.  
Impact: Crashes in security-sensitive parsers may not fail CI, and some fuzz targets never run.  
Remediation: Auto-discover all `*/fuzz/fuzz_targets/*.rs`, remove `continue-on-error`, upload crash artifacts, and keep docs synchronized with `.github/workflows/fuzz.yml`.

### H-06 Optional Feature Builds Are Not Tested
Severity: HIGH  
Evidence: CI runs default-feature clippy/test only (`.github/workflows/ci.yml:33-36`). Enterprise features exist for KMS (`crates/pcloud-config/Cargo.toml:20-33`, `crates/pcloud-kms/Cargo.toml:48-58`), daemon metrics/OTLP (`crates/pcloud-daemon/Cargo.toml:15-35`), observability OTLP (`crates/pcloud-observability/Cargo.toml:9-23`), and IDP insecure test mode (`crates/pcloud-idp/Cargo.toml:10-19`).  
Impact: Enterprise-only feature combinations can rot unnoticed.  
Remediation: Add `cargo hack` or equivalent for `--no-default-features`, selected `--all-features`, and provider feature sets; explicitly exclude intentional compile-error features such as FIPS scaffolding.

### H-07 Path Validation Tests Are Orphaned
Severity: HIGH  
Evidence: `crates/pcloud-ipc/src/path_validation.rs:53` defines `validate_local_sync_path`, with unit tests at `path_validation.rs:128-249`, but `crates/pcloud-ipc/src/lib.rs:54-61` does not declare `mod path_validation`, and repository search found no references outside the orphan file.  
Impact: These tests are not compiled or run, and the validation code is not reachable from IPC/daemon paths.  
Remediation: Either wire `pub mod path_validation` and call it from sync-root request handling, or delete the dead file; add integration/proptest coverage for traversal, NUL, symlink, length, and platform path cases.

### H-08 Crypto Password Rotation Has No Live Proof
Severity: HIGH  
Evidence: The parity matrix marks `psync_crypto_change_crypto_pass` Implemented with integration-tested wording (`C_FEATURE_PARITY_MATRIX.csv:120`), but the live test is explicitly a stub (`crates/pcloud-live-e2e/tests/change_crypto_pass.rs:8-11`) and ends in `todo!()` when enabled (`change_crypto_pass.rs:40-47`).  
Impact: A retained crypto lifecycle path is not live-verifiable.  
Remediation: Add an automatable OTP/mailbox fixture or demote the live-verification claim; ensure a configured live run fails until rotation is actually exercised.

### MEDIUM: 4

### M-01 Coverage, Mutation, And Chaos Docs Overclaim CI Gates
Severity: MEDIUM  
Evidence: `docs/book/src/development/testing.md:3-21` says every layer gates CI, including fuzz, mutation, coverage, chaos, and live E2E. Actual coverage is advisory and `continue-on-error` (`.github/workflows/ci.yml:300-343`), chaos is deferred (`.github/workflows/ci.yml:376-397`), and no mutation workflow exists under `.github/workflows/`.  
Impact: QA posture appears stronger than the enforced gates.  
Remediation: Either implement the documented gates or rewrite the docs as advisory/manual with exact workflow names and skip conditions.

### M-02 Bench Coverage Includes Stubs And Has No CI Gate
Severity: MEDIUM  
Evidence: `crates/pcloud-daemon/benches/vault_open_close.rs:14-31` and `crates/pcloud-fs/benches/writeback_flush.rs:14-31` are placeholder benchmarks. `crates/pcloud-sdk/benches/upload_session.rs:17-20` and `crates/pcloud-fs/benches/chunked_flush.rs:16-18` mention future bench CI. No workflow contains `cargo bench`.  
Impact: Performance regressions in vault startup and writeback flush are not measured.  
Remediation: Replace stubs with real Criterion benches, add nightly benchmark CI, persist baselines, and fail on agreed regression thresholds.

### M-03 Weak Smoke Tests With No Assertions
Severity: MEDIUM  
Evidence: Examples include `crates/pcloud-fs/tests/macos_platform_integration.rs:143-150`, `macos_platform_integration.rs:156-160`, `crates/pcloud-backends/src/mount_discovery.rs:407-413`, `crates/pcloud-engine/src/power.rs:215-225`, and `crates/pcloud-proto/tests/smoke_fuzz_arbitrary.rs:45-63`.  
Impact: These tests can pass without checking meaningful behavior beyond "did not panic."  
Remediation: Add concrete invariants or mark them with an explicit `smoke_no_panic` convention plus reviewer-approved rationale.

### M-04 Live Transfer Cleanup Violates Harness Contract
Severity: MEDIUM  
Evidence: The live-E2E README says mutating flows clean up created uploads/links/roots (`crates/pcloud-live-e2e/README.md:22-23`). The transfer test states deletefile is not active and writes IDs to a temp trace for human cleanup (`crates/pcloud-live-e2e/tests/transfers.rs:112-130`).  
Impact: Live accounts can accumulate artifacts, consuming quota and making later runs flaky.  
Remediation: Wire deletefile cleanup or add a mandatory scratch-folder cleanup step that fails if created objects remain.

## Commands Run
`sed -n` / `nl -ba` on `pcloud_rev.md`, workflows, READMEs, docs, and representative tests.  
`rg --files`, `find`, and `rg -n` inventories for tests, fuzz targets, benches, ignored tests, feature flags, proptest usage, and CI claims.  
Python read-only scans for weak tests and non-ignored live tests.

## Limitations
I did not run credentialed live tests, real FUSE/WinFSP/macOS mount tests, `cargo fuzz`, `cargo bench`, `cargo llvm-cov`, or the full workspace test suite. This report is based on static repository and workflow inspection only.
