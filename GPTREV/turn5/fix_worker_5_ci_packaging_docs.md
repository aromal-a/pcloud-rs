# Turn 5 Fix Worker 5: CI, Packaging, Deployment Docs

Date: 2026-04-30

## Scope

Worked only in the worker-5 ownership set:

- `.github/workflows/**`
- `packaging/**`
- `docs/book/src/development/**`
- `docs/book/src/operations/**`
- `docs/book/src/reference/config.md`
- `fuzz/README.md`
- `.cargo/mutants.toml`
- `flake.nix`
- `packaging/scripts/verify-reproducibility.sh`

No files under `crates/` were edited.

## Fixes

- Added `--locked` to CI Cargo resolver/build/test/doc/check invocations where supported, and added a Cargo-change PR trigger to `.github/workflows/security.yml`.
- Strengthened release/package gates with `cargo check --locked`, rustdoc, mdBook, and `cargo deny --locked check`.
- Aligned CI reproducibility with release artifacts by building both binaries with `cargo auditable build --locked --profile release-repro`.
- Updated the local reproducibility script and docs to use `cargo auditable`; marked the Nix repro output as not byte-identical to signed GitHub assets until it also uses cargo-auditable.
- Split systemd units: `pcloudd.service` is now system-only, `pcloudd-user.service` is the user unit, and `WatchdogSec=30s` was removed until daemon heartbeats are watchdog-aware.
- Removed macOS launchd `PCLOUD_CONFIG=config.toml` and `api.pcloud.com` API endpoint overrides; fixed plist XML comments so the templates parse.
- Documented `PCLOUDRS_*_FILE`, `PCLOUDRS_TRUST_DEVICE`, and `CREDENTIALS_DIRECTORY` in packaging, Docker, deployment, and config docs.
- Corrected Linux docs for published package channels, binary names, JSON/XDG paths, systemd unit names, fuzz commands, live E2E env names, mutation status, audit policy, and scaffolded macOS/Windows deployment claims.

## Changed Paths

- `.cargo/mutants.toml`
- `.github/workflows/ci.yml`
- `.github/workflows/fuzz.yml`
- `.github/workflows/release-packaging.yml`
- `.github/workflows/release.yml`
- `.github/workflows/security.yml`
- `docs/book/src/development/release-checklist.md`
- `docs/book/src/development/reproducible-builds.md`
- `docs/book/src/development/testing.md`
- `docs/book/src/operations/deployment-guide.md`
- `docs/book/src/operations/packaging-matrix.md`
- `docs/book/src/operations/platforms/linux.md`
- `docs/book/src/reference/config.md`
- `flake.nix`
- `fuzz/README.md`
- `packaging/README.md`
- `packaging/docker/README.md`
- `packaging/macos/README.md`
- `packaging/macos/build-dmg.sh`
- `packaging/macos/build-pkg.sh`
- `packaging/macos/com.pcloud.pcloud-rs.plist`
- `packaging/macos/com.pcloud.pcloudd.plist`
- `packaging/macos/install.sh`
- `packaging/scripts/verify-reproducibility.sh`
- `packaging/systemd/README.md`
- `packaging/systemd/override-user.conf.example`
- `packaging/systemd/pcloudd-user.service`
- `packaging/systemd/pcloudd.service`

## Verification

- `python3` YAML parse over `.github/workflows/*.yml`: passed.
- `bash -n packaging/scripts/verify-reproducibility.sh packaging/macos/install.sh packaging/macos/build-pkg.sh packaging/macos/build-dmg.sh`: passed.
- `python3 -m json.tool packaging/scoop/pcloud-rs.json`: passed.
- `python3` `plistlib.load()` over `packaging/macos/*.plist`: passed.
- `git diff --check` over edited worker-5 files: passed.
- `cargo deny --locked check`: passed with existing duplicate/unmatched-license warnings; final status `advisories ok, bans ok, licenses ok, sources ok`.
- `cargo audit --deny warnings --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2025-0134 --ignore RUSTSEC-2025-0141`: passed.
- Targeted grep checks confirmed no macOS plist exports `PCLOUD_CONFIG`, `PCLOUD_API_HOST`, `PCLOUD_API_SERVER_NAME`, or `api.pcloud.com`.
- Targeted grep checks confirmed no single-line workflow `run: cargo check|clippy|test|build|doc` command remains without `--locked`.
- `systemd-analyze verify packaging/systemd/pcloudd.service packaging/systemd/pcloudd.socket`: only reports `/usr/bin/pcloudd` missing in this dev environment.
- `systemd-analyze --user verify packaging/systemd/pcloudd-user.service`: only reports `/usr/bin/pcloudd` missing in this dev environment.

## Not Run / Residual

- `mdbook build docs/book`: not run because `mdbook` is not installed locally.
- `nix flake show/check`: not run because `nix` is not installed locally.
- Full `packaging/scripts/verify-reproducibility.sh`: not run because `cargo-auditable` is not installed locally and the full double build is expensive.
- `.deb`/`.rpm` package signing and SLSA provenance still require new release credentials/workflow design; docs now state the gap instead of claiming coverage.
- Installing `pcloudd-user.service` in `.deb`/`.rpm` metadata likely requires edits under `crates/pcloud-daemon/Cargo.toml`, which is outside this worker's ownership and was not changed.
