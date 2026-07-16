# pcloud-rs Enterprise Readiness Audit Report

Date: 2026-04-30
Auditor: Turn 3 subagent 05
Scope: testing, CI, QA, deployment, packaging, operations, observability docs/artifacts.
Mode: read-only static audit; no files modified.

## Executive Summary

This slice is not enterprise-ready. The main blockers are that live E2E tests are advisory and can pass by skipping core behavior, container and systemd packaging have default deployment failures, release/signing workflows do not match the documented provenance contract, and platform/support docs conflict with the actual CI gates.

Findings: 2 Critical, 7 High, 3 Medium.

## Findings

### F-01 Live E2E "gate" can pass without validating required live behavior

Severity: Critical

Evidence: `.github/workflows/ci.yml:313-318` runs live E2E only on weekly/manual triggers and sets `continue-on-error: true`. `.github/workflows/ci.yml:327-332` provides only `PCLOUD_TEST_USER` and `PCLOUD_TEST_PASSWORD`, while test families require token, crypto password, FUSE gate, backup-capable account, peer/share variables, and scratch/public-link paths. `crates/pcloud-live-e2e/tests/common/mod.rs:71-85` treats missing env as a passing skip, and `crates/pcloud-live-e2e/tests/common/mod.rs:230-237` soft-skips auth failures. `crates/pcloud-live-e2e/tests/change_crypto_pass.rs:33-47` is still a `todo!()` body. `crates/pcloud-live-e2e/tests/mount_linux.rs:73-90` and `crates/pcloud-live-e2e/tests/mount_linux.rs:110-127` soft-skip FUSE/environment failures. `docs/book/src/development/testing.md:258-259` claims release-candidate/label blocking that is not implemented in the workflow.

Impact: A release can appear to have live backend coverage while crypto password rotation, FUSE mount, backup lifecycle, sharing, and sync-upload completion are untested or skipped. This undermines parity and enterprise-readiness claims.

Remediation: Split live E2E into blocking and exploratory suites. Make the blocking suite fail on missing required env or unsupported account state. Provision dedicated CI accounts with token, crypto, backup, peer/share, scratch, and FUSE-capable runners. Remove or quarantine `todo!()` tests from the blocking set. Add protected-branch/release-candidate triggers that fail the release on live E2E failure.

### F-02 Docker image is not buildable/runnable as packaged

Severity: Critical

Evidence: `packaging/docker/Dockerfile:10` pins `RUST_VERSION=1.82`, but the workspace requires Rust 1.85 and edition 2024 at `Cargo.toml:63-68`. The image starts `ENTRYPOINT ["/usr/local/bin/pcloudd"]` with `CMD ["--socket", "/run/pcloud-rs/daemon.sock"]` at `packaging/docker/Dockerfile:79-80`, while the daemon accepts only no subcommand, `serve`, help, or version and rejects unknown args at `crates/pcloud-daemon/src/main.rs:56-64`. Docker sets `PCLOUDRS_STATE_DIR`, `PCLOUDRS_RUNTIME_DIR`, and `PCLOUDRS_SOCKET` at `packaging/docker/Dockerfile:70-72`, but the daemon documents and reads `PCLOUD_ROOT` instead at `crates/pcloud-daemon/src/main.rs:45-49` and `crates/pcloud-config/src/env.rs:78-85`. OCI labels still point to the upstream C client and GPL at `packaging/docker/Dockerfile:55-59`, conflicting with `Cargo.toml:66-69`.

Impact: Docker build or startup is expected to fail. The scheduled Trivy container scan in `.github/workflows/security.yml:82-119` inherits this failure, and documented container deployment is not usable.

Remediation: Pin a Rust toolchain compatible with MSRV, use `CMD ["serve"]`, set `PCLOUD_ROOT=/var/lib/pcloud-rs`, align socket/state paths with actual config, fix OCI source/license labels, and add a CI smoke test that builds the image and verifies `pcloudd serve` plus healthcheck startup.

### F-03 Systemd/Debian service packaging has contradictory install modes and blocks API egress by default

Severity: High

Evidence: `packaging/systemd/pcloudd.service:7-10` says the unit is intended for user scope, but `packaging/systemd/pcloudd.service:49-52` uses `DynamicUser=yes`. The required user-unit override itself says the base unit is rejected in user units at `packaging/systemd/override-user.conf.example:1-6`. The Debian postinst tells users to run `systemctl --user start pcloudd.service` at `packaging/debian/postinst:21-23`, while cargo-deb installs the units under `lib/systemd/system` at `crates/pcloud-daemon/Cargo.toml:156-162`. The default service denies all outbound IP except localhost at `packaging/systemd/pcloudd.service:117-122`. The shipped socket listens at `%t/pcloud-rs/daemon.sock` at `packaging/systemd/pcloudd.socket:8`, while the daemon's configured socket path is `pcloud.sock` at `crates/pcloud-config/src/paths.rs:92-94`. Logrotate creates logs as a fixed `pcloud-rs` user/group at `packaging/debian/pcloud-rs.logrotate:1-12`, but the package does not create that user and the service defaults to `DynamicUser`.

Impact: A fresh package install can present a start command that does not match installed units, a default system unit that cannot reach pCloud APIs, socket path drift, and logrotate ownership failures.

Remediation: Ship separate system and user units with matching install instructions. Either remove default `IPAddressDeny=any` or ship a tested egress drop-in. Align socket activation paths with `ConfigProfile::paths.ipc_socket_path()` or do not ship the socket unit. Create a fixed service user if logrotate requires one, or rely on journald/systemd-managed logs.

### F-04 Platform support claims conflict across README, mdBook, STATUS, and CI

Severity: High

Evidence: `README.md:3-4` says Linux Tier 1, macOS/Windows Tier 2, BSD Tier 3. `docs/book/src/introduction.md:55-59` says macOS and Windows are Tier 1 and release-blocking. `docs/book/src/architecture/platform-support.md:20-23` labels macOS/Windows T1 while saying mount live verification is "no." `STATUS.md:644-646` says Linux only is Tier 1 live, Windows is Tier 2, macOS is scaffolded/unverified, and FreeBSD is best-effort. CI matches the weaker claim: macOS excludes real FUSE at `.github/workflows/ci.yml:42-61`, Windows excludes `pcloud-fs` at `.github/workflows/ci.yml:63-71`, and FreeBSD is `continue-on-error` at `.github/workflows/ci.yml:73-82`.

Impact: Release managers and operators cannot determine which regressions block release. Documentation overstates support for macOS/Windows relative to the real CI gates.

Remediation: Make `STATUS.md` the canonical platform table and propagate it everywhere. If macOS/Windows are T1, add hard CI for integration tests, service startup, installer smoke tests, and mount hardware verification. If they are T2/T3, downgrade mdBook and packaging matrix claims.

### F-05 Release, signing, and provenance workflows do not implement the documented contract

Severity: High

Evidence: `Cargo.toml:73-80` says release CI should use `release-dist`; `docs/book/src/development/release-checklist.md:326-328` says all release builds use `--profile release-repro`. Actual release builds use `cargo auditable build --release` at `.github/workflows/release.yml:41-44` and `cargo build --release` at `.github/workflows/release-packaging.yml:71-78`. The release workflow builds only Linux x86_64 raw binaries at `.github/workflows/release.yml:20-25`. Packaging workflow builds only Linux x86_64 `.deb`/`.rpm` at `.github/workflows/release-packaging.yml:36-129`. Docs claim a `.github/workflows/packaging.yml` workflow with six jobs and `.sig + .pem` sidecars at `docs/book/src/operations/packaging-matrix.md:396-424`, but the actual workflow files are `ci.yml`, `fuzz.yml`, `release.yml`, `release-packaging.yml`, and `security.yml`. Cosign emits only `.sig` at `.github/workflows/release.yml:130-165`, and publish attaches `dist/*.sig` only at `.github/workflows/release.yml:186-196`.

Impact: Consumers cannot verify artifacts using the documented recipe. macOS, Windows, AppImage, Flatpak, Docker, certificate sidecars, and reproducible profile claims are not backed by the workflows.

Remediation: Either implement the documented packaging workflow or downgrade the docs. Build release artifacts with `release-repro` or `release-dist` consistently. Emit `SHA256SUMS`, signatures, certificates (`--output-certificate` for keyless `cosign sign-blob`), and provenance/attestations. Add release workflow assertions that fail when documented artifacts are missing.

### F-06 Coverage, fuzz, benchmark, and Nix QA gates are advisory or missing

Severity: High

Evidence: Coverage is weekly/manual only, `continue-on-error: true`, and swallows failures with `|| true` at `.github/workflows/ci.yml:368-387`. `codecov.yml:5-10` says gates should become required on 2026-04-29, but the current date is 2026-04-30 and all relevant statuses remain `informational: true` at `codecov.yml:31`, `codecov.yml:37`, `codecov.yml:43`, `codecov.yml:49`, `codecov.yml:55`, `codecov.yml:61`, and `codecov.yml:69`. Every fuzz job is `continue-on-error: true` at `.github/workflows/fuzz.yml:27-30`, `.github/workflows/fuzz.yml:56-59`, `.github/workflows/fuzz.yml:83-86`, and `.github/workflows/fuzz.yml:112-115`. Docs require `cargo bench -p pcloud-bench` at `docs/book/src/architecture/performance.md:31-36` and `docs/book/src/development/release-checklist.md:207-215`, but no `pcloud-bench` package exists; real benches are under per-crate `benches/`. Two benches are explicit stubs at `crates/pcloud-daemon/benches/vault_open_close.rs:14-31` and `crates/pcloud-fs/benches/writeback_flush.rs:14-31`. Nix package checks inherit packages with `doCheck = false` at `flake.nix:46-49`, `flake.nix:64-68`, and `flake.nix:144-146`.

Impact: Coverage, fuzz, performance, and Nix "checks" do not prevent regressions. Release checklist commands can fail because they reference a nonexistent benchmark crate.

Remediation: Make coverage required with a fail-under threshold. Make security-critical fuzz targets blocking on nightly/release branches. Create the documented `pcloud-bench` crate or update docs to per-crate bench commands. Add criterion baseline comparison in CI. Implement stub benches. Make `nix flake check` run meaningful test, clippy, mdBook, and release-profile build checks.

### F-07 Config/deployment docs still describe TOML and `auth.vault` while daemon config is JSON and `auth_token`

Severity: High

Evidence: The authoritative config reference says "JSON, not TOML" at `docs/book/src/reference/config.md:23-29`, and the loader parses JSON at `crates/pcloud-config/src/loader.rs:137-168`. The daemon auth vault path ends in `auth_token` at `crates/pcloud-config/src/paths.rs:96-107`. Packaging docs still prescribe `config.toml`, `pcloud-rs.toml`, `pcloudd.toml`, and `$PCLOUD_ROOT/auth.vault` at `packaging/README.md:64-66`, and list `PCLOUD_CONFIG` as a TOML path at `packaging/README.md:93`. Many operator docs still reference TOML, including `docs/book/src/getting-started/install.md:90`, `docs/book/src/operations/platforms/linux.md:119`, `docs/book/src/operations/platforms/macos.md:123`, and `docs/book/src/operations/platforms/windows.md:121`.

Impact: Operators following install/deployment docs can create configuration files the daemon will not load, or look for vault files that do not exist.

Remediation: Document the CLI TOML surface separately from the daemon JSON profile if both intentionally exist. Update packaging and operations docs to the daemon's JSON envelope and actual vault filename. Add a doc lint that fails on stale `config.toml` references outside the CLI-only sections.

### F-08 Live E2E environment documentation is inconsistent

Severity: High

Evidence: The live harness code expects `PCLOUD_LIVE_E2E`, `PCLOUD_TEST_USER`, `PCLOUD_TEST_PASSWORD`, and `PCLOUD_TEST_TOKEN` at `crates/pcloud-live-e2e/tests/common/mod.rs:33-43`. The crate-level docs instead say `PCLOUD_LIVE`, `PCLOUD_USERNAME`, and `PCLOUD_PASSWORD` at `crates/pcloud-live-e2e/src/lib.rs:23-46`. `.env.example:18-32` also sets `PCLOUD_USERNAME` and `PCLOUD_PASSWORD`, which do not satisfy `common::skip_if_not_live`. The README uses the `PCLOUD_TEST_*` convention at `crates/pcloud-live-e2e/README.md:27-48`.

Impact: Developers and CI operators can source the provided `.env.example` and still silently skip most live tests, reinforcing F-01.

Remediation: Standardize on one variable set. Prefer `PCLOUD_TEST_*` for live tests, update `.env.example` and `lib.rs`, or add backward-compatible aliases in `common::optional_env`. Add a test that validates `.env.example` contains all required names.

### F-09 Windows and macOS packaging docs overclaim runtime behavior

Severity: High

Evidence: Windows WiX declares only `<PackageDependency Id="winfsp" />` at `packaging/windows/wix/pcloud-rs.wxs:38-40`, while `packaging/windows/wix/README.md:27-46` claims the MSI bundles WinFSP with `<Binary>` and deferred `<CustomAction>`. The MSI installs and starts a Windows service at `packaging/windows/wix/pcloud-rs.wxs:84-98`, while `STATUS.md:70-82` says Windows integration tests, named-pipe IPC, live WinFSP mount, and service serving path are still open. The daemon's `pcloudd serve` mode is Unix-only at `crates/pcloud-daemon/src/main.rs:82-94`. macOS plist/config docs are stale: the LaunchAgent sets `PCLOUD_CONFIG` to `config.toml` at `packaging/macos/com.pcloud.pcloud-rs.plist:116-117`, and the README claims plists set removed/unstated variables like `PCLOUD_HOME`, `PCLOUD_AUTH_VAULT`, `PCLOUD_MOUNT_POINT`, `PCLOUD_IPC_SOCKET`, and `PCLOUD_API_SERVER` at `packaging/macos/README.md:122-127`.

Impact: Installers can ship/start nonfunctional or partially wired services, and platform docs set expectations beyond verified behavior.

Remediation: Implement a real WinFSP bootstrapper or require a preflight check with a blocking installer condition. Do not auto-start Windows service until IPC/service loop is verified. Update macOS plist docs to actual env vars and daemon JSON config. Add installer smoke tests on real or self-hosted macOS/Windows runners.

### F-10 Observability SLO surface is partially unwired and docs link to missing metrics reference

Severity: Medium

Evidence: Alert/dashboard metric names mostly match `MetricFamilies` in `crates/pcloud-observability/src/metrics.rs:15-26`, `ops/prometheus/pcloud-rs-alerts.yml:13-21`, and `ops/grafana/pcloud-rs-overview.json:35-151`. However, canonical SLOs include auth success, upload throughput, mount read latency, integrity sweeper, and audit verification at `crates/pcloud-observability/src/slo.rs:12-20`, while the metrics server explicitly says upload retry/started counters are not wired and the endpoint reports `pass: true` for that SLI until then at `crates/pcloud-daemon/src/metrics_server.rs:172-178`. The SLO module also documents that empty registries return `pass: true` at `crates/pcloud-observability/src/slo.rs:193-200`. Operations docs link to a missing Prometheus reference at `docs/book/src/operations/deployment.md:405` and `docs/book/src/operations/web-ui.md:333`; `docs/book/src/reference/metrics.md` is absent.

Impact: Operators can see "pass" or import docs while key SLOs are unmeasured. Missing metrics reference weakens alert/dashboard operationalization.

Remediation: Treat no-data SLOs as explicit `no_data` in dashboards and release gates. Wire or remove each canonical SLO before claiming operational coverage. Add `docs/book/src/reference/metrics.md` with exact metric names, labels, feature gates, scrape config, and alert semantics.

### F-11 mdBook/root docs contain stale links and false historical/status claims

Severity: Medium

Evidence: `README.md:41`, `docs/book/src/architecture/platform-support.md:8`, `docs/book/src/operations/packaging-matrix.md:42`, and `docs/book/src/reference/packaging.md:416` reference missing `PLAN_CROSSPLATFORM.md`. `docs/book/src/architecture/platform-support.md:160` references missing `packaging/windows/pcloudd-service.xml`, and `docs/book/src/architecture/platform-support.md:224-230` references missing peer/DACL/vault tests. `docs/book/src/reference/cli.md:1566` and `docs/book/src/reference/ipc-protocol.md:443` reference missing `docs/book/src/enterprise/tracing.md`, while the real file is `docs/enterprise/tracing.md`. `docs/book/src/introduction.md:12-18` claims legacy C/C++ sources live side-by-side, conflicting with `README.md:5-9`. `docs/book/src/faq.md:13-20` says the FUSE write path has not had a live host run, conflicting with `STATUS.md:580-590`.

Impact: mdBook may build but still ship broken or stale cross-references. Operators and auditors cannot rely on docs as evidence.

Remediation: Add a link-checking CI job. Remove or replace missing plan/report/test references. Make `STATUS.md` the single source for parity/platform status. Update introduction, FAQ, and architecture pages to current repository layout.

### F-12 Live E2E cleanup/account hygiene is not yet safe for shared CI accounts

Severity: Medium

Evidence: The live E2E README claims mutating flows clean up created uploads, links, and sync roots at `crates/pcloud-live-e2e/README.md:22-23`. The transfer test instead logs uploaded file IDs for human cleanup because deletefile is not on the active Rust path at `crates/pcloud-live-e2e/tests/transfers.rs:112-132`. Backup and crypto flows can skip based on account provisioning or backend refusal at `crates/pcloud-live-e2e/tests/backup_lifecycle.rs:165-180` and `crates/pcloud-live-e2e/tests/crypto.rs:124-140`.

Impact: Repeated live runs can pollute shared accounts and make future tests non-deterministic. This also discourages making live E2E blocking.

Remediation: Add delete/cleanup API coverage before enabling shared-account CI. Isolate every test under a unique scratch prefix and enforce cleanup in `Drop`/teardown. Fail the blocking suite when cleanup cannot be verified, or run against disposable accounts reset after each run.

## Remediation Roadmap

Immediate blockers: fix Docker startup/build, make live E2E honest and non-skipping for release candidates, align platform support claims with CI, and fix systemd/Debian install defaults.

Next hardening wave: implement release provenance as documented, convert coverage/fuzz/bench/Nix checks into real gates, and reconcile daemon JSON config docs with packaging instructions.

Documentation cleanup: add link-checking, restore or remove missing references, create the missing metrics reference, and make `STATUS.md` the canonical status source.

## Commands Run

All commands were read-only.

- `sed -n '1,240p' pcloud_rev.md` and `sed -n '241,520p' pcloud_rev.md`
- `rg --files .github packaging ops docs docs/book crates tests fuzz`
- `nl -ba .github/workflows/ci.yml`, `fuzz.yml`, `security.yml`, `release.yml`, `release-packaging.yml`
- `nl -ba crates/pcloud-live-e2e/README.md`, `src/lib.rs`, `Cargo.toml`, and selected `tests/*.rs`
- `rg -n "live|e2e|PCLOUD_|ignore|continue-on-error|schedule|freebsd|windows|macos|fuse|mdbook|coverage|llvm-cov"`
- `find crates -path '*/fuzz/fuzz_targets/*.rs' -print | sort`
- `rg --files crates | rg '/benches/.*\\.rs$' | sort`
- `nl -ba packaging/systemd/*`, `packaging/debian/*`, `packaging/docker/*`, `packaging/macos/*`, `packaging/windows/wix/*`
- `nl -ba ops/prometheus/pcloud-rs-alerts.yml`, `ops/grafana/pcloud-rs-overview.json`, `crates/pcloud-observability/src/metrics.rs`, `crates/pcloud-observability/src/slo.rs`, `crates/pcloud-daemon/src/metrics_server.rs`
- `nl -ba flake.nix`, `Cargo.toml`, `codecov.yml`, `.env.example`, `README.md`, `STATUS.md`, `ARCHITECTURE.md`, selected `docs/book/src/**/*.md`
- `rg -n "config\\.toml|pcloud-rs\\.toml|pcloudd\\.toml|auth\\.vault|PCLOUD_CONFIG"`
- `for p in PLAN_CROSSPLATFORM.md ...; do test -e "$p"; done`
- `git status --short`

## Limitations

This was a static audit only. I did not run `cargo test`, `mdbook build`, Docker builds, release workflows, package installers, fuzzers, benchmarks, or live pCloud E2E tests because the lead-agent override required no modifications and no live credentials/hardware were provided. I excluded `target/`, `vendor/`, `.beads/`, `GPTREV/`, `CLAUDEREV/`, and generated tracker output. The worktree was already dirty in unrelated source files; I did not modify or revert anything.
