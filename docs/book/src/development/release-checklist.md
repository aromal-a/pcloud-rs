# Cross-Platform Release Checklist

> **Honesty note:** `pcloud-rs-rust` is **pre-alpha**. There is **no release
> tag yet**. The current development tree is not a clean integrated release
> commit, the SDK is not published, and native/package qualification remains
> incomplete. Every entry in
> `CHANGELOG.md` lives under `[Unreleased]`. This page is the standing
> gauntlet that will apply when the first tag is cut — read it, test it
> against dry-runs, but do **not** cut `v0.1.0-alpha.1` until every blocking row
> below is genuinely green.
>
> When the first tag lands, promote the header to the released version and
> open a retro bead with any step that had to be hand-patched.

## 1. Purpose

This page is the complete pre-release gauntlet for the project. Release
managers use it as the on-duty runbook; maintainers reference it when
reviewing release-preparation PRs. Every row states:

- **why it matters** (the specific incident class it prevents),
- **exact command** (so there is no tribal-knowledge gap),
- **gate status** (`blocking`, `warning`, or `informational`).

A release manager on duty owns every blocking row. If a step cannot be
completed, the release does not ship — open a bead and reschedule.

## 2. Prerequisites

### Toolchain pin

- `rust-toolchain.toml`: exact `channel = "1.91.0"`, components
  `clippy, rustfmt`.
- Workspace `Cargo.toml`: `edition = "2024"`, `rust-version = "1.89"`.
- `pcloud-plugin-wasmtime` is the sole MSRV exception at Rust 1.91 because the
  advisory-fixed Wasmtime 43 line requires it. CI checks the portable
  workspace-minus-Wasmtime at 1.89 and that isolated plugin at 1.91.
- Release builds use `--profile release-repro` (inherits from
  `release-dist`; see `reproducible-builds.md`).

### System dependencies

- `gpg` (≥ 2.2) — for tag and `SHA256SUMS.asc` signing.
- `cargo-deny` — license / advisory / sources audit.
- `cargo-audit` — RustSec advisory database check.
- `cargo-llvm-cov` — coverage reporting; the scheduled/manual CI job hard-gates
  the workspace and critical-crate floors, and the release manager repeats it.
- `cargo-mutants` (weekly; not on the blocking path).
- `mdbook` — build the contributor handbook artefact.
- `cargo-deb`, `cargo-generate-rpm`, `linuxdeploy` — Linux packaging.
- `WiX` (Windows), Developer ID + `notarytool` (macOS).
- `fuse3` headers — only if the release includes `pcloud-fs` mount binary.

Verify once on the release host:

```sh
rustc --version
cargo deny --version
cargo audit --version
cargo llvm-cov --version
mdbook --version
gpg --list-secret-keys release@pcloud-rs.example
```

## 3. Conceptual preamble — where a release sits in the lifecycle

```
 bead close      parity proof      tag cut       repro build     sign
 ────────▶  ──────────────────▶  ─────────▶  ───────────────▶  ────▶
              (bd-1du.10)                    (release-repro)
                                                    │
                                                    ▼
                                            smoke tests on clean VMs
                                                    │
                                                    ▼
                                             package managers PRs
                                                    │
                                                    ▼
                                              announcement + retro
```

A release is a distinct supply-chain artefact, not a commit. The steps below
are what separates a commit from an artefact trustworthy enough to ship.

## 4. Detailed walkthrough

### 4.0 Pre-flight (blocking)

- [ ] All `bd-1du.*` parity beads relevant to the release are closed.
      **In particular**, `bd-1du.10` must be satisfied — no release that
      claims parity may ship without it.
- [ ] `CLAUDE.md`, `STATUS.md`, `C_FEATURE_PARITY_MATRIX.csv`,
      and `C_FEATURE_PARITY_REVIEW.md` agree on the feature state. If
      they disagree, stop and reconcile; `STATUS.md` wins.
- [ ] No open `P0` bead in any subsystem.
- [ ] `git status --porcelain` is empty. The tag candidate contains no
      unstaged, staged-but-uncommitted, or untracked release input.
- [ ] The focused `pcloud-sdk` has an intentional SemVer and a reviewed public
      API diff. Publish and verify the dependency chain in order:
      `pcloud-model` -> `pcloud-ipc` -> `pcloud-sdk`. For each crate, run
      `cargo package --locked`, publish from the clean tag candidate, wait for
      registry indexing, then verify a fresh project can resolve the registry
      package without workspace paths. The broad `pcloud-embedded-sdk` remains
      `publish = false` and is not part of this chain.

*Why:* a release whose own dossier lies is an outage waiting to happen. The
one-file mismatch between `STATUS.md` and the matrix is the most common
source of false parity claims in this tree.

### 4.1 Source tree — the gauntlet (blocking)

```sh
cd .
cargo fmt --all --check
cargo check  --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test   --workspace --locked
cargo doc    --workspace --locked --no-deps --document-private-items
cargo audit  --deny warnings \
  --ignore RUSTSEC-2023-0071
cargo deny   --locked check
```

Row-by-row:

- [ ] **`cargo fmt --all --check`** — deterministic diff discipline.
      *Prevents:* bikeshed reviewer comments delaying the tag window.
- [ ] **`cargo check --workspace --all-targets --locked`** — every target,
      every feature combination actually compiles.
      *Prevents:* "tests compile but the main bin doesn't on Windows".
- [ ] **`cargo clippy ... -- -D warnings`** — zero clippy warnings across
      the workspace. The gate has held at zero across every wave.
      *Prevents:* the `-D warnings` erosion that kills code-quality trends.
- [ ] **`cargo test --workspace --locked`** — unit + integration + doctest
      pyramid. See `testing.md` for the layer breakdown; use the current
      command output rather than hard-coded historical counts.
      *Prevents:* regressions that slip past a single-crate test run.
- [ ] **`cargo doc --workspace --no-deps --document-private-items`** with
      **zero warnings** in rustdoc output. Capture to a file and grep for
      `warning:` to be safe:

      ```sh
cargo doc --workspace --locked --no-deps --document-private-items 2>&1 \
          | tee /tmp/doc.log
      ! grep -E '^warning:' /tmp/doc.log
      ```
      *Prevents:* dead intra-doc links that break under the mdBook build.
- [ ] **`cargo audit --deny warnings ... --ignore <time-boxed IDs>`** —
      RustSec database clean except for the reviewed exceptions listed in
      `audit.toml` and mirrored in workflow flags. cargo-audit 0.22 does not
      read `audit.toml` automatically, so the flags are required.
      *Prevents:* shipping an unreviewed known-CVE transitive dep.
- [ ] **`cargo deny --locked check`** — licences, advisories, bans, sources (see
      `deny.toml`; GPL-only families are blocked at the workspace level).
      *Prevents:* accidental GPL-only dep leaking into an MIT/Apache-2.0
      artefact.

#### 4.1.1 `cargo deny check` expectations

`deny.toml` is the single source of truth for supply-chain policy.
The release gauntlet expects the following on a clean tree:

- **Exit 0.** `cargo deny --locked check` must print
  `advisories ok, bans ok, licenses ok, sources ok`. Anything else blocks
  the release.
- **No `advisory-not-detected` warnings.** Every entry in
  `[advisories].ignore` must still apply to a crate in the current lock
  graph. A stale entry means either the dep is gone (drop the ignore) or
  the advisory moved to a different crate (update the comment). Stale
  entries erode trust in the policy.
- **No `unmatched-skip` warnings.** Every entry under `[bans].skip` must
  match at least one duplicate. Upgrade cascades routinely resolve
  duplicates; stale `skip` rows hide genuine new duplicates behind a
  silent free pass.
- **Every `[advisories].ignore` entry carries a `review: YYYY-MM-DD`
  comment.** The reviewer named at the top of the `ignore` block sweeps
  the list on that date and either removes the entry or re-stamps it with
  a new review date and a one-line justification for the extension.
- **`[bans].multiple-versions = "warn"`, justified.** The current policy
  is intentionally `"warn"` (not `"deny"`) because four upstream stacks
  (AWS SDK, zbus/secret-service, regorus, Windows target graph) pin
  incompatible majors that we do not control. The justification comment
  in `deny.toml` lists each stack and its upstream tracker. **Flipping to
  `"deny"` requires** at least one of those stacks to collapse a major
  **and** an audit of the remaining `skip` list to remove any entries
  that become stale. Do not change the policy without updating the
  comment.
- **`[bans].wildcards = "deny"` with `allow-wildcard-paths = true`.** Path
  deps inside the workspace legitimately use `version = "*"`; external
  wildcards still hard-fail. Do not relax either knob.
- **`audit.toml` mirrors the workflow ignore flags.** cargo-audit does
  not read `audit.toml` automatically in the current CLI path, so
  `cargo xtask host` passes the reviewed RustSec IDs as explicit `--ignore`
  flags. If you add an advisory exception, update `audit.toml`, `deny.toml`,
  and the xtask audit invocation in the same change.

Sweep command used by the release manager to verify the above:

```sh
cd .
cargo deny --locked check 2>&1 | tee /tmp/deny.log
! grep -E 'advisory-not-detected|unmatched-skip' /tmp/deny.log
```

If either of those two warning classes fires, the deny policy is out of
date — fix it in the release PR, not in a follow-up.

### 4.2 Coverage (blocking)

```sh
cargo xtask coverage
```

- [ ] Workspace line floor: **≥ 90 %**. See `testing.md §6`. A current
      successful local run is mandatory.
- [ ] Per-crate floors override the workspace number for security-critical
      crates: `pcloud-secret 90 %`, `pcloud-crypto 85 %`,
      `pcloud-auth 85 %`, `pcloud-resilience 80 %`, `pcloud-ipc 80 %`.
- [ ] The floor never lowers automatically; if a release requires a
      temporary dip, land an explicit commit updating
      `ci/coverage-floor.toml` with a changelog entry first.

*Why:* coverage is a crude but leading indicator. A drop on a release tag is
the single strongest signal of deletion-without-replacement.

### 4.3 Benchmark regression guard (manual target; not automated)

```sh
cargo bench -p pcloud-fs --bench chunked_flush
cargo bench -p pcloud-fs --bench page_cache
cargo bench -p pcloud-embedded-sdk --bench upload_session
```

- [ ] Compare against `target/criterion/` baselines committed to the
      release branch. **No benchmark regresses more than 10%** versus the
      previous release baseline. No GitHub Actions benchmark workflow
      exists today, so this is a manual release-manager check until a
      baseline/threshold workflow lands.
- [ ] Any regression between 5 % and 10 % is a **warning**: document it in
      the release ticket and the CHANGELOG `### Performance` bullet.
- [ ] See `architecture/performance.md` for the wave-1 optimisation dossier
      and the expected numbers per benchmark.

*Why:* we tightened the page cache and chunked flush paths deliberately; a
silent regression there undoes months of work.

### 4.4 Documentation build (blocking)

```sh
mdbook build docs/book
```

- [ ] mdBook builds clean with **zero warnings**.
- [ ] Every code snippet with a language tag must actually compile or be
      marked `text`. The current workflow runs `mdbook build`; snippet
      compile-checking must not be claimed until a dedicated checker lands.
- [ ] `docs/book/book/` artefact is deterministic — same `SOURCE_DATE_EPOCH`
      applies here too.

*Why:* a docs build that warns in CI is a docs build that breaks for
downstream packagers who script over it.

### 4.5 Rustdoc sanity (blocking)

- [ ] `cargo doc --workspace --no-deps --document-private-items` emits
      **0 warnings**. The gate is already folded into §4.1 but repeated
      here because it's a frequent miss on release day when a private
      item's docs drift.

### 4.6 Parity matrix (blocking)

- [ ] `C_FEATURE_PARITY_MATRIX.csv`: row counts match `STATUS.md`.
- [ ] No row regressed from `Implemented` → `Partial` since the previous
      release baseline. Additions and closures are fine; the matrix must
      be **unchanged or growing**.
- [ ] `REJECTED-RATIONALES-14042026.md` covers every `Rejected` row with a
      dated justification.
- [ ] `C_FEATURE_PARITY_REVIEW.md` narrative agrees with the matrix.

*Why:* the parity matrix is the single audit artefact downstream auditors
will consult. A silent regression there is worse than a missing feature.

### 4.7 Live E2E (target blocking; current CI advisory)

```sh
PCLOUD_LIVE_E2E=1 \
PCLOUD_TEST_USER=staging-bot@example.com \
PCLOUD_TEST_PASSWORD=… \
cargo test -p pcloud-live-e2e --locked -- --ignored --test-threads=1
```

- [ ] Green against the **staging** pCloud account within the last 24 h.
- [ ] TFA smoke case included for any release touching `pcloud-auth`.
- [ ] Credentials come from the release team's 1Password vault only;
      never from a personal account.

### 4.8 CHANGELOG + version bump (blocking)

**Version bump policy** (SemVer, with pre-alpha caveat):

- **Pre-1.0**: `0.MINOR.PATCH`. Breaking IPC changes bump `MINOR`; backwards
  compatible additions and bug fixes bump `PATCH`. Pre-release tags carry a
  `-alpha.N` / `-beta.N` suffix.
- **1.0 onward**: strict SemVer. IPC wire breakage is a major bump.

Bump instructions:

```sh
# Edit [workspace.package].version in Cargo.toml.
cargo check --workspace --locked   # regenerates Cargo.lock; commit it.
```

CHANGELOG template (append a new dated block above `[Unreleased]`):

```md
## [0.1.0-alpha.1] — YYYY-MM-DD

### Added
- …

### Changed
- …

### Fixed
- …

### Security
- …

### Known limitations
- `bd-1du.4` FUSE mount parity still gated behind the `fuse-mount` feature.
```

- [ ] Every bullet cites its PR / bead / CVE.
- [ ] No placeholder lines.

### 4.9 Tag cut (blocking)

```sh
git tag -s v<version> -m "Release v<version>"
git push origin v<version>
```

- [ ] Tag is GPG-signed with the release key.
- [ ] Tag commit is `main` HEAD after the release PR lands — not a detached
      commit.

### 4.10 Build phase — per platform

Raw Linux release binaries in `.github/workflows/release.yml` use
`--profile release-repro` with `SOURCE_DATE_EPOCH` pinned to the tag commit
time (`git log -1 --pretty=%ct`). See `reproducible-builds.md`.

The `.deb` / `.rpm` workflow currently builds with `cargo build --release`
because the cargo-deb/cargo-generate-rpm metadata consumes
`target/release/{pcloudd,pcloudc}`. Treat those packages as release-candidate
artefacts until the packaging workflow is switched to the reproducible
profile or records a separate reproducibility proof.

#### Linux (x86_64) — blocking

- [ ] On the labelled native FUSE runner, execute
      `scripts/linux-release-mount-gate.sh`. It runs all 16 practical
      real-kernel mount/probe tests serially, the separate 2 GiB
      transient-retry transfer test, and fails if a pcloud-rs mount remains.
- [ ] Build under the release container (Debian stable + pinned toolchain)
      for glibc compatibility.
- [ ] Current CI artefacts: raw `pcloudd` and `pcloudc` binaries, CycloneDX
      and SPDX SBOMs, `.deb`, `.rpm`, and checksum files.
- [ ] AppImage, Flatpak, Snap, Docker, and aarch64 release artefacts are not
      produced. macOS and Windows jobs exist but count only after their native
      live-mount, signing, and publication steps are green for this tag.
- [ ] `ldd` confirms no unexpected dynamic deps.
- [ ] `readelf -n` confirms deterministic build-id or absent per policy.

#### macOS (universal) — blocking only when macOS is promoted into scope

- [ ] `MACOSX_DEPLOYMENT_TARGET=12.0`.
- [ ] Lipo-joined `.pkg`, signed with Developer ID Application cert,
      notarised via `xcrun notarytool submit --wait`, stapled.
- [ ] `spctl -a -vv` reports accepted.

#### Windows (x86_64) — blocking only when Windows is promoted into scope

- [ ] MSI via WiX, signed with the EV HSM-backed cert.
- [ ] SmartScreen submission for new version families.

#### BSD ports — informational

- [ ] Source tarball published; ports committers notified.

### 4.11 Integrity and signatures (blocking)

```sh
sha256sum pcloud-rs-*.{tar.xz,deb,rpm,AppImage,pkg,msi} > SHA256SUMS
gpg --detach-sign --armor SHA256SUMS
```

- [ ] If a GPG release key is used, it is distinct from platform code-signing
      certs and its public half is in `SECURITY.md` and on `keys.openpgp.org`.
      Current CI uses cosign for raw binaries/SBOMs and does not emit GPG
      package signatures.
- [ ] Verify the signature from a fresh checkout on a clean VM before
      proceeding:

      ```sh
      gpg --verify SHA256SUMS.asc SHA256SUMS
      sha256sum -c SHA256SUMS
      ```

- [ ] **CI release pipelines (blocking for what they publish).**
      `.github/workflows/release.yml` runs `release-candidate` first
      (`fmt`, `check`, `clippy -D warnings`, `test`, `doc`, `mdbook`,
      `cargo audit --deny warnings`
      with the `audit.toml` exceptions mirrored as `--ignore` flags,
      and `cargo deny --locked check`), then builds raw Linux x86_64 `pcloudd` and
      `pcloudc` binaries plus SBOMs. The signing job emits cosign detached
      signatures for the raw binaries, their `.sha256` files, and the SBOMs.
      Keyless signing also emits `<file>.pem` certificates.

      ```sh
      cosign verify-blob \
        --certificate-identity-regexp "https://github.com/${REPO}/.github/workflows/release.yml@.*" \
        --certificate-oidc-issuer https://token.actions.githubusercontent.com \
        --signature <file>.sig --certificate <file>.pem <file>
      ```

      `.github/workflows/release-packaging.yml` runs the same source gate and a
      strict Linux FUSE gate before building Linux and Tier-2 NAS candidates.
      Its Windows job requires signing credentials and signs the executables,
      MSI, and Burn bundle. Its self-hosted macOS job requires fuse-t, runs live
      mounts, signs and notarizes the package, and assesses it before upload.
      Linux package GPG signing remains conditional on configured secrets, and
      the workflow does not produce SLSA provenance or Docker images. Existing
      job definitions are not evidence: require a green run for the tag.

- [ ] **Supply-chain PR gates (blocking, per commit).** Confirm the
      current workflow jobs are green on the release commit:
      `.github/workflows/ci.yml` (`test-linux`, `mdbook`, `cargo-doc`)
      and `.github/workflows/security.yml` (`audit`, `deny`). The release
      workflow repeats `cargo audit --deny warnings` and
      `cargo deny --locked check`
      before any publish job can run.

- [ ] **Native signing paths.** Apple Developer ID/notarization and Windows
      Authenticode jobs are strict and require their release secrets. A missing
      credential or failed signature blocks the corresponding public artifact;
      do not substitute an unsigned candidate. Raw Linux binaries and SBOMs
      continue to use cosign keyless signatures.

### 4.12 Publication (blocking)

- [ ] GitHub Release from the signed tag; title `v<version>`.
- [ ] Upload every artefact plus the signature/checksum files the workflows
      actually emit. Current state: raw binary `.sha256`, raw binary/SBOM
      `.sig` and keyless `.pem`, and package `SHA256SUMS` without a detached
      package signature.
- [ ] Paste the `CHANGELOG.md` block into the release notes.
- [ ] Mark pre-releases as such; only promote to "Latest" after §4.14 smoke
      tests pass.

### 4.13 Package manifests — warning (async)

Each may land asynchronously. None block the tag.

- [ ] Homebrew tap (`pcloud-rs.rb`).
- [ ] Chocolatey (`pcloud-rs.nuspec`, `chocolateyinstall.ps1`).
- [ ] winget (`manifests/e/ezechiel203/pcloud-rs/<version>/`).
- [ ] Debian APT repo (`reprepro includedeb`).
- [ ] RPM YUM repo (`createrepo_c` + sign `repomd.xml`).
- [ ] BSD ports notifications.

### 4.14 Post-release smoke tests (blocking)

- [ ] Debian 12 VM: `apt install pcloud-rs`, `pcloud-rs --version`, login +
      sync smoke.
- [ ] Fedora 40 VM: `dnf install`, same smoke.
- [ ] macOS 14 VM: `.pkg` install, smoke.
- [ ] Windows 11 VM: MSI install, smoke.
- [ ] Auto-update channel (if applicable) sees the new version.

### 4.15 Announcement — informational

- [ ] Website download page.
- [ ] Blog post.
- [ ] Mastodon / X / LinkedIn with checksum + sig URLs.
- [ ] `pcloud-rs-announce` mailing list.
- [ ] Archive this page to `docs/releases/<version>.md`.

## 5. Release notes template

```md
# pcloud-rs-rust v<version>

<one-line tagline>

## Highlights
- …

## Breaking changes
- …

## Security
- …

## Parity matrix
- Implemented: <n>
- Partial:     <n>
- Rejected:    <n>
- Missing:     <n>

Full dossier: C_FEATURE_PARITY_MATRIX.csv @ <tag>.

## Verification
Signed by the workflow-emitted signature files. If a future release also uses
a GPG key, include its fingerprint here and in `SECURITY.md`.
Reproduce:

```sh
export SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct v<version>)
CARGO_PROFILE_RELEASE_REPRO_ACTIVE=1 \
  cargo auditable build --profile release-repro --locked -p pcloud-cli
sha256sum target/release-repro/pcloudc
```

## Known limitations
- …
```

## 6. Rollback plan (incident hooks)

If any step from §4.10 onward fails after artefacts are public:

1. **Mark** the GitHub Release as "Pre-release" or delete it outright if
   the artefact contains a security defect.
2. **Advise** users via `pcloud-rs-announce` and a pinned GitHub issue.
3. **Publish** a security advisory (GHSA) if user-facing impact exists.
4. **Yank** any broken package-manager PRs before they merge.
5. **Open** a retro bead linking the failing step; add the lesson to this
   checklist before the next release.

Package-manager clients that already pulled the broken artefact can be
blocked by publishing a patched release with the broken version's SHA
listed as a known-bad in `SECURITY.md`.

## 7. Gate checklist — TL;DR

A one-screen summary maintainers copy into the release ticket:

- [ ] pre-flight bead/matrix/status reconciled
- [ ] `fmt / check / clippy -D warnings / test / doc / audit / deny` all
      green
- [ ] coverage advisory reviewed; no automated release floor exists yet
- [ ] benchmark manual check reviewed; no workflow baseline exists yet
- [ ] mdBook + rustdoc 0 warnings
- [ ] parity matrix unchanged or growing
- [ ] live E2E green within 24 h
- [ ] CHANGELOG entry + version bump committed
- [ ] tag GPG-signed
- [ ] artefacts built under `release-repro` with pinned
      `SOURCE_DATE_EPOCH`
- [ ] emitted signatures/checksums verified on clean VM
- [ ] GitHub Release uploaded, marked pre-release
- [ ] smoke tests on 4 clean VMs green
- [ ] announcement sent

## 8. Common mistakes

- **Tagged without regenerating `Cargo.lock`.** *How reviewers catch it:*
  the reproducible build on a clean checkout resolves a different graph;
  the double-build hash diff fails.
- **Forgot `SOURCE_DATE_EPOCH`.** *How reviewers catch it:* tarball mtimes
  differ across rebuilds; `diffoscope` surfaces it in CI.
- **Pushed `v<version>` before the release PR merged.** *How reviewers
  catch it:* tag commit SHA ≠ `main` HEAD at the time of tagging.
- **Dropped a parity row silently.** *How reviewers catch it:* the
  parity-matrix CI diff in §4.6 blocks the PR.
- **Shipped without `--locked`.** *How reviewers catch it:* `Cargo.lock`
  regenerates during release build; the double-build verification in
  `reproducible-builds.md` fails.
- **Used a personal pCloud account for live E2E.** *How reviewers catch
  it:* secret scanner in CI flags the credential, release is aborted.
- **Skipped `cargo deny`.** *How reviewers catch it:* the license-bot PR
  back-propagates the audit and a GPL-only dep is surfaced retroactively.

## 9. FAQ

**Q: We're pre-alpha with no tag. Why rehearse this?**
A: Because cutting the first tag is the least forgiving release. Every step
that fails for the first time at `v0.1.0-alpha.1` cascades into retroactive
patches.

**Q: What if a benchmark regresses exactly 10%?**
A: Treat it as a regression. The threshold is `<10%`, not `≤10%`.

**Q: Can a release go out while a native or package gate is missing?**
A: Only an explicitly labelled development snapshot may do so. A public
platform-support claim requires every blocking gate for that platform and the
release notes must list any excluded platform honestly.

**Q: Do package-manager manifests block the tag?**
A: No. §4.13 is async. The blocking path ends at §4.14 smoke tests.

**Q: What key signs the release?**
A: Current CI uses cosign keyless blob signatures for raw binaries/SBOMs.
Linux package GPG signing is conditional on configured release secrets;
Windows Authenticode and Apple Developer ID/notarization use separate native
credentials. Every key identity must be documented in `SECURITY.md`.
