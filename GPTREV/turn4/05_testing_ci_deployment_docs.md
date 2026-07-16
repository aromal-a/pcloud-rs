# Turn 4 Testing, CI, Deployment, and Docs Audit

Date: 2026-04-30

Read-only audit using `pcloud_rev.md` as the master prompt. No files were edited.

## Commands Run

| Command | Result |
|---|---|
| `cargo fmt --all --check` | Passed. |
| `CARGO_TARGET_DIR=/tmp/pcloud-rs-codex-target cargo test --workspace` | Passed. Default run skipped live/FUSE/chaos-gated tests. Notable slow tests included `pcloud-crypto` lib tests at about 442s and `crypto_share_rsa_e2e` at about 250s. |
| `CARGO_TARGET_DIR=/tmp/pcloud-rs-codex-target cargo clippy --workspace --all-targets -- -D warnings` | Passed. |
| `cargo deny check` | Passed, with duplicate dependency and unmatched-license warnings. |
| `cargo audit --deny warnings` | Failed. Reported `RUSTSEC-2023-0071` for `rsa`, plus denied warnings for `bincode`, `paste`, and `rustls-pemfile`. |
| `cargo bench -p pcloud-bench --no-run` | Failed: package `pcloud-bench` does not exist. |
| `mdbook --version` | Failed: `mdbook` not installed. |
| `docker --version` | Failed: `docker` not installed. |
| `nix --version` | Failed: `nix` not installed. |
| `find .github/workflows -maxdepth 1 -type f -print | sort` | Only `ci.yml`, `fuzz.yml`, `release.yml`, `release-packaging.yml`, `security.yml` exist. No `docker.yml`, `packaging.yml`, or `rust.yml`. |
| `rg -n "cargo bench|criterion" .github/workflows` | No benchmark workflow found. |
| `rg -n "upload-artifact|actions/upload-artifact" .github/workflows/fuzz.yml` | No fuzz crash artifact upload found. |
| `rg -n "slsa|provenance|intoto|attestation|notary|signtool|APPLE|WINDOWS|docker" .github/workflows/release.yml .github/workflows/release-packaging.yml` | No SLSA, macOS, Windows, Docker, notarization, or Authenticode release steps found. |

## Findings

### T4-CRIT-01: Release publishing is disconnected from the documented release gauntlet

Severity: Critical

Evidence: `.github/workflows/release.yml:4-12` triggers on tags/manual dispatch, then builds and publishes through `build-artifacts`, `sbom`, `sign`, and `publish` only at `.github/workflows/release.yml:20-44`, `.github/workflows/release.yml:99-165`, and `.github/workflows/release.yml:169-196`. The release checklist marks source checks, `cargo audit`, and `cargo deny` as blocking at `docs/book/src/development/release-checklist.md:96-105`, coverage as blocking at `docs/book/src/development/release-checklist.md:189-202`, benchmarks as blocking at `docs/book/src/development/release-checklist.md:207-215`, and live E2E as blocking at `docs/book/src/development/release-checklist.md:259-270`. Actual live E2E is weekly/manual and `continue-on-error` at `.github/workflows/ci.yml:303-318`; coverage is weekly/manual, `continue-on-error`, and masks `llvm-cov` failure with `|| true` at `.github/workflows/ci.yml:368-386`; fuzz jobs are all `continue-on-error` at `.github/workflows/fuzz.yml:27-30`, `.github/workflows/fuzz.yml:56-59`, `.github/workflows/fuzz.yml:83-86`, and `.github/workflows/fuzz.yml:112-115`.

Impact: a tag can publish signed release artifacts even if live E2E, coverage floors, fuzzing, benchmark regression checks, or `cargo audit --deny warnings` are failing or never ran.

Remediation: make release publishing depend on a single protected release-candidate workflow that runs the documented gauntlet with `--locked`, hard-fails live E2E for release tags, enforces coverage floors, runs benchmark regression checks, runs `cargo audit --deny warnings`, and only then signs/publishes artifacts. Remove `continue-on-error` for release-candidate and tag paths.

### T4-CRIT-02: Signing/provenance coverage is incomplete while docs claim end-to-end supply-chain guarantees

Severity: Critical

Evidence: raw release signing signs only `dist/pcloudd`, `dist/pcloudc`, and two SBOM JSON files at `.github/workflows/release.yml:141-157`, uploads only `dist/*.sig` at `.github/workflows/release.yml:162-165`, and publishes raw binaries/SBOM/signatures at `.github/workflows/release.yml:186-196`. Package release builds `.deb` and `.rpm`, computes `SHA256SUMS`, uploads packages, and attaches them to GitHub releases at `.github/workflows/release-packaging.yml:80-130`, but it does not sign packages, sign `SHA256SUMS`, emit certificates/bundles, or generate provenance. Docs claim detached signatures for `.deb`, `.rpm`, `.AppImage`, and `SHA256SUMS` at `docs/book/src/reference/packaging.md:323-329`, a broad release matrix including Docker/macOS/Windows/SLSA jobs at `docs/book/src/reference/packaging.md:373-388`, and SLSA v1.0 provenance at `docs/book/src/reference/packaging.md:393-401`.

Impact: downstream users can be told artifacts are signed/provenanced when the workflow does not actually produce those signatures or attestations.

Remediation: sign every published artifact, including `.deb`, `.rpm`, `SHA256SUMS`, and any future Docker/macOS/Windows assets. Emit Sigstore certificates/bundles or GPG signatures consistently. Add SLSA provenance generation for all release assets or remove all SLSA claims until implemented.

### T4-HIGH-01: Live E2E can pass by doing nothing, and one critical crypto-rotation test is a `todo!()`

Severity: High

Evidence: the live E2E README states every test is ignored and runtime-gated, so even `cargo test --workspace -- --ignored` is a no-op without `PCLOUD_LIVE_E2E=1` at `crates/pcloud-live-e2e/README.md:11-15`. The helper returns success by early-returning when the gate or required env vars are absent at `crates/pcloud-live-e2e/tests/common/mod.rs:71-85`. CI only supplies `PCLOUD_TEST_USER` and `PCLOUD_TEST_PASSWORD` at `.github/workflows/ci.yml:327-332`, while the README documents additional variables at `crates/pcloud-live-e2e/README.md:27-47` and `crates/pcloud-live-e2e/README.md:105-116`. `change_crypto_pass` is a stub and calls `todo!()` when the gate is satisfied at `crates/pcloud-live-e2e/tests/change_crypto_pass.rs:8-11` and `crates/pcloud-live-e2e/tests/change_crypto_pass.rs:33-47`.

Impact: CI can report a green live E2E job without exercising important flows. If the crypto password rotation environment is ever fully provisioned, that test panics instead of validating the release.

Remediation: in CI, fail fast when required release-live secrets are missing. Split optional families into explicit jobs with explicit skip accounting. Replace the crypto-rotation stub with an automatable OTP/email fixture or remove it from any release gate and mark the feature unverified.

### T4-HIGH-02: The documented `cargo audit --deny warnings` release gate currently fails

Severity: High

Evidence: the release checklist requires `cargo audit --deny warnings` at `docs/book/src/development/release-checklist.md:96-105` and says `audit.toml` must mirror `deny.toml` at `docs/book/src/development/release-checklist.md:171-176`. Local execution failed with `RUSTSEC-2023-0071` for `rsa`, plus denied warnings for `bincode`, `paste`, and `rustls-pemfile`. `deny.toml` ignores `RUSTSEC-2023-0071` and `RUSTSEC-2025-0134` at `deny.toml:27-48`, while `audit.toml` claims it must mirror deny at `audit.toml:3-6` but ignores a different set at `audit.toml:8-26`.

Impact: the release gauntlet cannot pass as written, and cargo-deny/cargo-audit policy drift means CI can give conflicting security results.

Remediation: decide the authoritative advisory policy, then make `deny.toml`, `audit.toml`, and CI execute the same policy. Add a CI job that runs exactly `cargo audit --deny warnings` or update the release checklist to the real command and exception model. Remove stale or non-mirrored ignores.

### T4-HIGH-03: Reproducible-build claims do not match actual release builds

Severity: High

Evidence: the workspace defines `release-dist` and says to use it for releases at `Cargo.toml:73-80`, and defines `release-repro` at `Cargo.toml:129-144`. The toolchain file pins only `stable`, not an exact compiler, at `rust-toolchain.toml:1-3`. CI's reproducibility job builds only `pcloud-daemon` with plain `cargo build --release` at `.github/workflows/ci.yml:108-153`. The tag release builds with `cargo auditable build --release -p pcloud-daemon -p pcloud-cli` at `.github/workflows/release.yml:41-44`; packaging builds with `cargo build --release --workspace -p pcloud-daemon -p pcloud-cli` at `.github/workflows/release-packaging.yml:71-78`. Docs claim the reproducibility script uses `--locked --profile release-repro -p pcloud-cli -p pcloud-daemon` at `docs/book/src/development/reproducible-builds.md:237-242`, and the release checklist says release builds use `--profile release-repro` at `docs/book/src/development/release-checklist.md:30-33`.

Impact: the artifact that is actually signed and published is not the artifact covered by the reproducibility contract.

Remediation: pin an exact Rust toolchain, build release artifacts with `--locked --profile release-repro`, compare both `pcloudc` and `pcloudd`, and make signing consume those exact verified artifacts. If `cargo-auditable` is required, integrate it into the reproducible profile or document why byte reproducibility is not currently guaranteed.

### T4-HIGH-04: Manual release `tag` inputs can build the wrong commit

Severity: High

Evidence: `release.yml` defines a manual `tag` input at `.github/workflows/release.yml:8-12`, but checkout uses default `actions/checkout@v4` with no `ref` at `.github/workflows/release.yml:27`. `release-packaging.yml` defines a manual `tag` input at `.github/workflows/release-packaging.yml:22-26`, checks out the default ref at `.github/workflows/release-packaging.yml:40`, then derives package version from the input at `.github/workflows/release-packaging.yml:57-69`.

Impact: a manually dispatched workflow can label artifacts as one tag while building another commit.

Remediation: for manual dispatch, validate that the input tag exists and check out `ref: ${{ inputs.tag }}`. Fail if `git rev-parse HEAD` does not equal the tag object's target.

### T4-HIGH-05: Benchmark release gate is documented but nonfunctional

Severity: High

Evidence: performance docs say benchmarks live under `crates/pcloud-bench/benches/` and are reproduced with `cargo bench -p pcloud-bench` at `docs/book/src/architecture/performance.md:3-7`, `docs/book/src/architecture/performance.md:29-36`, and `docs/book/src/architecture/performance.md:222-232`. The release checklist blocks on `cargo bench -p pcloud-bench -- chunked_flush upload_session page_cache_evict` at `docs/book/src/development/release-checklist.md:207-215`. The workspace member list has no `pcloud-bench` at `Cargo.toml:17-54`, and local `cargo bench -p pcloud-bench --no-run` failed because the package does not exist. Actual benches are spread across crate-local `benches/` directories, and no GitHub workflow contains `cargo bench`.

Impact: the documented performance regression gate cannot run and therefore cannot block releases.

Remediation: either create a real `pcloud-bench` crate that aggregates the named benchmarks or update docs and CI to run the existing distributed benches. Add a scheduled and release-candidate benchmark job with stored baselines and explicit regression thresholds.

### T4-HIGH-06: Docker build and Docker documentation are materially wrong

Severity: High

Evidence: the Dockerfile defaults to `ARG RUST_VERSION=1.82` at `packaging/docker/Dockerfile:10`, but the workspace requires Rust `1.85` at `Cargo.toml:63-68`. The Dockerfile uses an Alpine builder and distroless runtime at `packaging/docker/Dockerfile:17` and `packaging/docker/Dockerfile:52`, while Docker docs claim `rust:1.82-bookworm` to `debian:bookworm-slim`, UID/GID 1000, and `tini` at `packaging/docker/README.md:8-13`. The local build example omits the build context at `packaging/docker/README.md:19-24`. Docs claim `.github/workflows/docker.yml` publishes multi-arch images at `packaging/docker/README.md:48-52`, but the workflow file does not exist. OCI labels point to `https://github.com/pcloudcom/console-client` and `GPL-3.0-or-later` at `packaging/docker/Dockerfile:55-59`, while workspace metadata says `MIT OR Apache-2.0` and `https://github.com/ezechiel203/pcloud-rs` at `Cargo.toml:65-70`.

Impact: the default Docker build is likely broken, published-image documentation is false, and image metadata misidentifies source and license.

Remediation: bump the Docker Rust version to at least the workspace `rust-version`, fix the build command, correct OCI labels, and either add the documented Docker publish workflow or mark Docker publishing as local/scaffolded only.

### T4-HIGH-07: Nix flake outputs and checks are misleading

Severity: High

Evidence: the flake disables tests for both derivations at `flake.nix:46-49` and `flake.nix:64-68`, then exposes `checks` that merely inherit those build derivations at `flake.nix:144-146`. The default package/app use `mainProgram = "pcloud-rs"` and app name `pcloud-rs` at `flake.nix:46-53` and `flake.nix:89-97`, but actual binary names are `pcloudc` and `pcloudd` at `crates/pcloud-cli/Cargo.toml:45-49` and `crates/pcloud-daemon/Cargo.toml:104-109`. Docs claim `apps.<system>.{pcloud-rs,pcloudd,pcloudc}` and correct `apps.pcloudc` output at `docs/book/src/reference/packaging.md:103-114`, while install docs claim `packages.<system>.{pcloudc,pcloudd}` and `checks.<system>.integration` at `docs/book/src/getting-started/install.md:223-230`.

Impact: `nix run` can target a nonexistent binary, and `nix flake check` can appear green without running tests or integration checks.

Remediation: expose packages/apps matching actual binaries, add `apps.pcloudc`, make `default` point to a real binary, and add real `checks` that run tests/smoke checks. Update docs to match actual outputs.

### T4-MED-01: Coverage, fuzz, mutation, and docs-build assurance layers are advisory or absent despite stronger claims

Severity: Medium

Evidence: testing docs honestly mark fuzz, coverage, and live E2E advisory at `docs/book/src/development/testing.md:20-28`, but also say crashes generate artifacts at `docs/book/src/development/testing.md:123-125`; the fuzz workflow has no upload-artifact step and all fuzz jobs use `continue-on-error` at `.github/workflows/fuzz.yml:27-30`, `.github/workflows/fuzz.yml:56-59`, `.github/workflows/fuzz.yml:83-86`, and `.github/workflows/fuzz.yml:112-115`. Mutation config claims enforcement by a `mutants` job in `.github/workflows/rust.yml` at `.cargo/mutants.toml:1-5`, but `rust.yml` does not exist and testing docs say mutation is not yet in CI at `docs/book/src/development/testing.md:142-147`. The release checklist says mdBook is blocking and CI-enforced at `docs/book/src/development/release-checklist.md:224-233`, but `mdbook` was not installed locally and no mdBook workflow exists.

Impact: assurance coverage is weaker than the release and contributor docs imply.

Remediation: add the missing fuzz artifact upload, mutation, mdBook, and coverage-enforcement workflows, or downgrade the docs to explicit manual-only/advisory status until those workflows exist.

### T4-MED-02: Debian/RPM install docs and maintainer scripts disagree with actual package layout

Severity: Medium

Evidence: install docs claim every Linux channel installs the daemon at `/usr/libexec/pcloud-rs/pcloudd` and a systemd user unit at `/usr/lib/systemd/user/pcloudd.service` at `docs/book/src/getting-started/install.md:79-92`. Actual `cargo-deb` assets install `pcloudd` and `pcloudc` to `usr/bin/` and system units to `lib/systemd/system/` at `crates/pcloud-daemon/Cargo.toml:156-166`. The Debian postinst tells users to start a user service at `packaging/debian/postinst:21-23` even though the package installs system units. Install docs advertise an apt repository at `docs/book/src/getting-started/install.md:150-171`, while the only package release workflow attaches `.deb`/`.rpm` to GitHub releases at `.github/workflows/release-packaging.yml:123-130`.

Impact: users following docs may start the wrong systemd scope, look for binaries in wrong locations, or rely on a repository that is not produced by CI.

Remediation: pick the intended package layout and make Cargo metadata, nfpm metadata, postinst, and install docs agree. Remove apt/dnf repository instructions until repository publishing exists.

### T4-MED-03: Deployment docs still describe the old systemd network sandbox

Severity: Medium

Evidence: the shipped unit explicitly does not set `IPAddressDeny` or `IPAddressAllow` by default at `packaging/systemd/pcloudd.service:117-128`. The override file says strict egress allow-listing is opt-in at `packaging/systemd/override.conf.example:1-9` and applies `IPAddressDeny=any` only when installed at `packaging/systemd/override.conf.example:32-44`. Deployment docs still say the API-access drop-in is mandatory because the shipped unit blocks pCloud API access at `docs/book/src/operations/deployment-guide.md:59-75` and `docs/book/src/operations/deployment-guide.md:103-107`. The hardening table still lists default `IPAddressDeny=any` and `IPAddressAllow=localhost` at `docs/book/src/operations/deployment-guide.md:395-414`. Packaging README also still calls the API-access drop-in required at `packaging/README.md:39-40`.

Impact: operators may install unnecessary broad egress drop-ins, and documentation contradicts the shipped security model.

Remediation: update deployment docs and packaging README to say outbound traffic is host-firewall governed by default, with `override.conf.example` only for opt-in cgroup-level egress allow-listing.

### T4-MED-04: Docker runtime environment variables appear inert relative to the documented daemon env surface

Severity: Medium

Evidence: Dockerfile sets `PCLOUDRS_STATE_DIR`, `PCLOUDRS_RUNTIME_DIR`, and `PCLOUDRS_SOCKET` at `packaging/docker/Dockerfile:70-72`; docker-compose sets `PCLOUDRS_TOKEN_FILE` at `packaging/docker/docker-compose.yml:61-64`. The packaging env truth table says the daemon and CLI actually read `PCLOUD_*` variables at `packaging/README.md:77-107`, and explicitly says anything not on the list is inert at `packaging/README.md:79-82`.

Impact: container state, socket, and token configuration may not be consumed by the daemon, making the documented container deployment nonfunctional or misleading.

Remediation: either implement and test those `PCLOUDRS_*` variables or change Docker/Compose to use the real supported env surface such as `PCLOUD_ROOT` and documented auth/token mechanisms.

### T4-MED-05: macOS and Windows deployment/signing docs advertise channels that release CI does not build

Severity: Medium

Evidence: macOS docs advertise Homebrew and notarized `.pkg` install paths at `docs/book/src/operations/deployment-guide.md:199-211`, but release workflows contain no macOS signing/notarization steps. Windows docs advertise Chocolatey, winget, and signed MSI install paths at `docs/book/src/operations/deployment-guide.md:264-275`, while later admitting the Windows daemon is evaluation-only/no-op for key paths at `docs/book/src/operations/deployment-guide.md:290-301`. WiX comments require Authenticode signing in release CI at `packaging/windows/wix/pcloud-rs.wxs:6-14`, and signing docs list Apple/Windows secrets expected by `.github/workflows/release.yml` at `packaging/signing/README.md:159-179`; the release workflows contain no such references.

Impact: users and auditors can believe platform packages are signed and shipped when they are scaffolds or not CI-produced.

Remediation: mark macOS/Windows channels as scaffold/evaluation-only until release CI builds, signs, notarizes, and verifies them. If they are intended to be release channels, add platform jobs and make unsigned artifacts unpublishable.

### T4-LOW-01: Fuzz and test documentation is stale in smaller but trust-eroding ways

Severity: Low

Evidence: `fuzz/README.md` says fuzz targets live only under `pcloud-proto` and `pcloud-ipc` at `fuzz/README.md:20-29`, while `.github/workflows/fuzz.yml:32-86` also fuzzes `pcloud-crypto` and `pcloud-daemon`. `tests/README.md` lists only `crates/pcloud-daemon/tests/live_auth.rs` as a live verification entry point at `tests/README.md:10-12`, omitting `crates/pcloud-live-e2e`. Testing docs say current count is `1247 passing` at `docs/book/src/development/testing.md:54-55`, while the current workspace test run is substantially larger and includes many ignored live/FUSE/chaos tests.

Impact: contributors looking for the authoritative test surface get incomplete or stale guidance.

Remediation: make `.github/workflows/fuzz.yml` the generated source of truth for fuzz target docs, update `tests/README.md` to include `pcloud-live-e2e`, and replace hard-coded test counts with current CI output or generated counts.
