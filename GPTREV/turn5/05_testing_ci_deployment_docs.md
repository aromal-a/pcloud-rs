# Turn 5 Testing, CI, Deployment, and Docs Audit

Date: 2026-04-30

Scope: read-only review of dirty working tree after Turn 4 fixes, using `pcloud_rev.md` as master prompt. No repository files were edited by the review agent.

## Commands Run

| Command | Result |
|---|---|
| `python3` YAML parse over `.github/workflows/*.yml` | Passed. |
| `cargo fmt --all --check` | Passed. |
| `CARGO_TARGET_DIR=/tmp/pcloud-rs-codex-target cargo test --workspace --locked --no-fail-fast` | Failed during compile because `/tmp` filled: `No space left on device`, linker bus error. Cleaned that temp target with `cargo clean`. |
| `cargo test --workspace --locked --no-fail-fast` | Passed using repo `target/`. Live/FUSE/chaos-gated ignored tests remain unexecuted. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed. |
| `cargo audit --deny warnings --ignore ...` | Passed with reviewed ignores. |
| `cargo deny check` | Passed with duplicate-dependency and unmatched-license warnings. |
| `cargo bench -p pcloud-fs --bench chunked_flush --no-run --locked` | Passed compile-only bench gate. |
| `mdbook --version` | Failed: `mdbook` not installed. |
| `docker --version` | Failed: `docker` not installed. |
| `nix --version` | Failed: `nix` not installed. |
| `cargo fuzz --version` | Passed: `cargo-fuzz 0.13.1`. |
| `cargo fuzz list` from repo root | Failed: missing `fuzz/Cargo.toml`. Crate-local `--fuzz-dir` lists passed. |
| `rg -n "cargo bench|criterion" .github/workflows` | No benchmark workflow found. |

## Findings by Severity

| Severity | Count |
|---|---:|
| Critical | 1 |
| High | 4 |
| Medium | 6 |
| Low | 2 |

## Detailed Findings

### T5-CRIT-01: Release publishing still bypasses documented blocking release gates

Severity: Critical

Evidence: `.github/workflows/release.yml:62-80` gates only fmt, clippy, test, audit, and deny before build/publish, and `.github/workflows/release.yml:267-296` publishes artifacts after `build-artifacts`, `sbom`, and `sign`. `.github/workflows/release-packaging.yml:79-97` has the same reduced source gate, then `.github/workflows/release-packaging.yml:197-204` uploads `.deb`, `.rpm`, and `SHA256SUMS`. The documented release checklist still marks coverage as blocking at `docs/book/src/development/release-checklist.md:196-209`, benchmarks as blocking at `:214-224`, mdBook as blocking at `:233-241`, and live E2E as target-blocking at `:268-280`. Current CI keeps live E2E `continue-on-error` at `.github/workflows/ci.yml:313-318` and coverage `continue-on-error` plus `|| true` at `.github/workflows/ci.yml:368-386`. No workflow contains `cargo bench`.

Impact: a tag can publish release artifacts without coverage floors, benchmark regression checks, mdBook, rustdoc release checks, or live E2E proving retained parity flows.

Remediation: make release publish depend on a single release-candidate workflow that runs the documented gauntlet with hard failures. Add coverage floors, benchmark baseline comparison, mdBook/rustdoc, live E2E with required secrets and skip accounting, then make both release workflows depend on that workflow conclusion.

### T5-HIGH-01: Live E2E can still report green while major live parity flows are skipped or stubbed

Severity: High

Evidence: the CI live job is advisory at `.github/workflows/ci.yml:313-318` and supplies only `PCLOUD_TEST_USER` / `PCLOUD_TEST_PASSWORD` at `.github/workflows/ci.yml:327-332`. The harness soft-skips missing gates and env vars in `crates/pcloud-live-e2e/tests/common/mod.rs:71-85`, and `authed_daemon` soft-skips auth failures at `crates/pcloud-live-e2e/tests/common/mod.rs:230-237`. Additional live families require env vars documented at `crates/pcloud-live-e2e/README.md:105-116`, but CI does not pass them. Crypto password rotation remains a `todo!()` at `crates/pcloud-live-e2e/tests/change_crypto_pass.rs:33-47`. The A-to-B share test can degrade to a send-only pass and return without accept-side proof at `crates/pcloud-live-e2e/tests/shares_a_to_b.rs:302-339`.

Impact: live E2E does not provide release-grade evidence for crypto rotation, full two-account sharing, FUSE live mounts, GPG snapshot coverage, or other optional credential-gated flows.

Remediation: split live E2E into explicit jobs per feature family, require the relevant secrets per job, fail on missing required release secrets, emit machine-readable skip counts, and remove `continue-on-error` for release candidates. Replace `change_crypto_pass` with an automatable OTP/email fixture or mark the feature unverified outside the release gate.

### T5-HIGH-02: Reproducibility checks do not reproduce the artifact that release CI signs

Severity: High

Evidence: release artifacts are built with `cargo auditable build --locked --profile release-repro` and custom `RUSTFLAGS` at `.github/workflows/release.yml:121-127`. The CI reproducibility job still builds only `pcloudd` with plain `cargo build --release -p pcloud-daemon` and hashes `target/release/pcloudd` at `.github/workflows/ci.yml:120-127`. The local reproducibility script uses plain `cargo build --profile release-repro` at `packaging/scripts/verify-reproducibility.sh:75-85`, not `cargo auditable`. The Nix derivation claims to reproduce the release artifact at `flake.nix:62-63`, but it uses normal Cargo with `SOURCE_DATE_EPOCH = "1"` and different flags at `flake.nix:64-74`. The docs say the release job runs an equivalent double-build check at `docs/book/src/development/reproducible-builds.md:251-252`, but the release workflow has no such double-build job.

Impact: green reproducibility checks can validate a different binary from the signed release binary.

Remediation: create one canonical release build script used by release CI, reproducibility CI, Nix, and local verification. It must use `cargo auditable`, `--profile release-repro`, both `pcloudc` and `pcloudd`, identical `SOURCE_DATE_EPOCH`, identical `RUSTFLAGS`, and compare against the exact artifacts uploaded to GitHub Releases.

### T5-HIGH-03: Linux package artifacts still ship without package signatures or provenance

Severity: High

Evidence: `.github/workflows/release-packaging.yml:182-204` computes `SHA256SUMS`, uploads packages, and attaches them to the GitHub release, but there is no package signing, checksum signing, Sigstore bundle, or provenance step. Raw binary signing covers only the artifacts listed at `.github/workflows/release.yml:230-236`. The packaging matrix honestly records no provenance and unsigned packages at `docs/book/src/operations/packaging-matrix.md:14-19`, `:52-59`, and `:63-65`.

Impact: downstream Linux package consumers get weaker supply-chain guarantees than raw binary consumers.

Remediation: sign `.deb`, `.rpm`, and `SHA256SUMS`; publish verification material; add SLSA provenance for every published release asset; make unsigned package upload impossible on release tags.

### T5-HIGH-04: Linux platform docs still advertise nonexistent channels and wrong paths

Severity: High

Evidence: the Linux platform chapter claims nfpm/AUR/Nix/Flatpak/Snap/Docker/AppImage packaging is live-verified at `docs/book/src/operations/platforms/linux.md:17-23`, then gives APT, DNF, zypper, AUR, Nix, and Alpine install commands at `docs/book/src/operations/platforms/linux.md:49-72`. The actual packaging matrix says only raw Linux binaries, `.deb`, and `.rpm` are CI-built, while Docker/AppImage/Flatpak/Snap are scaffolds and no Docker/SLSA/macOS/Windows workflow exists at `docs/book/src/operations/packaging-matrix.md:18-35`. The same Linux page installs nonexistent build outputs `target/release/pcloud-daemon` and `target/release/pcloud-cli` at `docs/book/src/operations/platforms/linux.md:94-97`; the real bin names are `pcloudd` and `pcloudc` at `crates/pcloud-daemon/Cargo.toml:105-110` and `crates/pcloud-cli/Cargo.toml:46-50`. It also documents `config.toml`, `store.sqlite`, `vault.dat`, and `daemon.sock` at `docs/book/src/operations/platforms/linux.md:119-125`, while the current config reference documents JSON envelope paths and `PCLOUD_ROOT` layout at `docs/book/src/reference/config.md:25-28` and `:75-82`, and the socket helper returns `pcloud.sock` at `crates/pcloud-config/src/paths.rs:81-93`.

Impact: operators following the Linux chapter will run nonfunctional install commands and look for state/config files in the wrong locations.

Remediation: make `docs/book/src/operations/platforms/linux.md` defer to `packaging-matrix.md`; remove unpublished package-manager commands; fix binary names; replace old C-era config/state/socket paths with the current JSON/XDG/`PCLOUD_ROOT` layout.

### T5-MED-01: Main PR CI is weaker than release CI and can miss lockfile/advisory drift

Severity: Medium

Evidence: Linux CI runs `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and cargo-deny without `--locked` at `.github/workflows/ci.yml:33-40`. macOS and Windows test jobs also omit `--locked` at `.github/workflows/ci.yml:49-71`. The security workflow runs cargo-audit only on weekly schedule or dependency-file push at `.github/workflows/security.yml:3-24`; it is not a normal pull-request gate.

Impact: dependency graph drift can pass PR CI and only fail later in release CI or a scheduled audit.

Remediation: add `--locked` to all CI Cargo invocations that resolve dependencies, add a PR cargo-audit gate for Cargo changes, and keep the same reviewed ignore list in CI/release/docs.

### T5-MED-02: Benchmark regression guard is documented but absent from CI

Severity: Medium

Evidence: release docs require benchmark regression checks at `docs/book/src/development/release-checklist.md:214-224`. Benchmarks exist under `crates/pcloud-fs/benches/chunked_flush.rs`, `crates/pcloud-fs/benches/page_cache.rs`, `crates/pcloud-sdk/benches/upload_session.rs`, and other crate-local benches, but `rg -n "cargo bench|criterion" .github/workflows` returned no workflow matches. A representative compile-only check, `cargo bench -p pcloud-fs --bench chunked_flush --no-run --locked`, passed locally.

Impact: performance regressions cannot block release despite being release-blocking in docs.

Remediation: add a benchmark workflow with stored baselines and explicit thresholds, then wire it into release-candidate dependencies.

### T5-MED-03: Fuzz assurance is advisory, and fuzz docs still contain stale commands and target scope

Severity: Medium

Evidence: every fuzz job uses `continue-on-error` at `.github/workflows/fuzz.yml:27-30`, `:64-67`, `:99-102`, and `:136-139`. Crash artifacts are uploaded, which is an improvement, at `.github/workflows/fuzz.yml:31-38`, `:68-75`, `:103-110`, and `:140-147`. `fuzz/README.md:20-26` says fuzz targets live only in proto/ipc, omitting crypto and daemon. `fuzz/README.md:52-64` tells users to run `cargo fuzz run <target>` from repo root, but local `cargo fuzz list` failed because `fuzz/Cargo.toml` does not exist. `fuzz/README.md:67-70` says the CI cap is 600 seconds, while workflow targets use 300 seconds.

Impact: contributors get bad reproduction instructions, and fuzz crashes remain non-blocking.

Remediation: update `fuzz/README.md` to use `--fuzz-dir` and all 11 targets, align the time budget, and make fuzz hard-fail on release-candidate schedules while optionally remaining advisory on nightly exploratory runs.

### T5-MED-04: Credential-file environment docs are now false after Turn 4 bootstrap changes

Severity: Medium

Evidence: `packaging/README.md:78-83` says its environment table is the list the daemon/CLI actually read and anything absent is inert. The table omits `PCLOUDRS_TOKEN_FILE`, `PCLOUDRS_USERNAME_FILE`, `PCLOUDRS_PASSWORD_FILE`, `PCLOUDRS_TFA_CODE_FILE`, `PCLOUDRS_RECOVERY_CODE_FILE`, and `PCLOUDRS_TRUST_DEVICE`. The daemon now reads those variables at `crates/pcloud-daemon/src/bootstrap.rs:161-187`, validates credential file permissions at `crates/pcloud-daemon/src/bootstrap.rs:104-154`, and applies them during bootstrap at `crates/pcloud-daemon/src/bootstrap.rs:555-561`. Docker Compose uses `PCLOUDRS_TOKEN_FILE` at `packaging/docker/docker-compose.yml:57-62`.

Impact: operators may ignore the supported secret-file bootstrap path or believe Docker Compose is setting an inert variable.

Remediation: add the `PCLOUDRS_*` credential-file variables and systemd `CREDENTIALS_DIRECTORY` behavior to `packaging/README.md`, `docs/book/src/reference/config.md`, Docker docs, and deployment docs. Mark them as secret bootstrap variables, not general config overrides.

### T5-MED-05: Testing and release docs contradict actual live/chaos/audit behavior

Severity: Medium

Evidence: testing docs correctly say live E2E is advisory at `docs/book/src/development/testing.md:239-244`, then immediately claim it runs only on release candidate tags or labelled PRs and blocks release at `:258-259`; actual CI runs only schedule/manual and `continue-on-error` at `.github/workflows/ci.yml:313-318`. The live E2E example uses `PCLOUD_E2E_USERNAME` and `PCLOUD_E2E_PASSWORD` at `docs/book/src/development/testing.md:246-253`, but the harness uses `PCLOUD_TEST_USER` and `PCLOUD_TEST_PASSWORD` at `crates/pcloud-live-e2e/tests/common/mod.rs:33-43`. The same testing doc says the heavy chaos trio runs in a weekly chaos job at `docs/book/src/development/testing.md:179-181`, then says chaos is not in CI at `:198-200`; `.github/workflows/ci.yml:420-441` says it is deferred. Release checklist text says cargo-audit reads `audit.toml` and no longer passes ignore flags at `docs/book/src/development/release-checklist.md:178-181`, but release and security workflows pass explicit `--ignore` flags at `.github/workflows/release.yml:71-77` and `.github/workflows/security.yml:18-24`.

Impact: release managers can run the wrong live E2E command, expect nonexistent chaos gates, or misunderstand advisory-policy enforcement.

Remediation: make testing/release docs generated from workflow snippets where possible; otherwise update env names, `--ignored`, trigger conditions, and cargo-audit policy text in one PR.

### T5-MED-06: Deployment guide has stale systemd/AppArmor/platform install claims

Severity: Medium

Evidence: the deployment hardening table says `Type=notify` and READY notification behavior at `docs/book/src/operations/deployment-guide.md:369-372`, but the shipped unit is `Type=simple` at `packaging/systemd/pcloudd.service:30-36` and explicitly says no READY notification is emitted at `packaging/systemd/pcloudd.service:12-16`. The deployment guide says no AppArmor profile ships in-tree at `docs/book/src/operations/deployment-guide.md:184-187`, but `packaging/apparmor/usr.local.bin.pcloudd` exists. macOS and Windows install sections still show Homebrew, Chocolatey, winget, and MSI install commands at `docs/book/src/operations/deployment-guide.md:204-210` and `:269-278`, while the platform chapters correctly mark those artifacts as scaffolds with no release workflow at `docs/book/src/operations/platforms/macos.md:50-55` and `docs/book/src/operations/platforms/windows.md:47-49`.

Impact: deployment operators can apply the wrong readiness semantics and install unsupported platform artifacts.

Remediation: update the deployment guide hardening matrix from `packaging/systemd/pcloudd.service`; document the AppArmor profile; mark macOS/Windows package commands as scaffold-only or remove them until release workflows exist.

### T5-LOW-01: Hard-coded test/parity counts remain inconsistent across docs

Severity: Low

Evidence: `STATUS.md:318` records `2 033 passed / 46 ignored`, while `STATUS.md:637` records `2029 passed / 46 ignored`. `docs/book/src/development/testing.md:54-55` still says current unit count is `1247 passing`. `STATUS.md:330` retains an older parity count `158 / 0 / 0 / 28`, while the current summary at `STATUS.md:632-636` says `149 Implemented / 7 Partial / 0 Missing / 30 Rejected`.

Impact: audit readers cannot tell which counts are authoritative without manual reconciliation.

Remediation: replace hard-coded counts with a generated status block or link all historical sections to the current summary with dates.

### T5-LOW-02: Mutation-testing config still references a nonexistent workflow

Severity: Low

Evidence: `.cargo/mutants.toml:3-5` says the MMR floor is enforced by `.github/workflows/rust.yml`, but that workflow does not exist. Testing docs now say mutation testing is not yet in CI at `docs/book/src/development/testing.md:145-147`.

Impact: config comments point maintainers to a dead CI gate.

Remediation: update `.cargo/mutants.toml` to match current manual-only status or add the promised mutation workflow.
