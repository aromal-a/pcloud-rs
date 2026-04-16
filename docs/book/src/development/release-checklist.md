# Cross-Platform Release Checklist

> **Honesty note:** `pcloud-rs-rust` is **pre-alpha**. There is **no release
> tag yet**; `bd-1du.10` (final parity proof) remains open. Every entry in
> `CHANGELOG.md` lives under `[Unreleased]`. This page is the standing
> gauntlet that will apply when the first tag is cut — read it, test it
> against dry-runs, but do **not** cut `v0.1.0-alpha.1` until every row
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

- `rust-toolchain.toml`: `channel = "stable"`, components `clippy, rustfmt`.
- Workspace `Cargo.toml`: `edition = "2024"`, `rust-version = "1.85"`.
- Release builds use `--profile release-repro` (inherits from
  `release-dist`; see `reproducible-builds.md`).

### System dependencies

- `gpg` (≥ 2.2) — for tag and `SHA256SUMS.asc` signing.
- `cargo-deny` — license / advisory / sources audit.
- `cargo-audit` — RustSec advisory database check.
- `cargo-llvm-cov` — coverage enforcement.
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
cargo doc    --workspace --no-deps --document-private-items
cargo audit  --deny warnings
cargo deny   check
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
      pyramid. See `testing.md` for the layer breakdown (1247 unit tests,
      7 proptest properties × 128 cases, 4 fuzz targets, 5 chaos scenarios).
      *Prevents:* regressions that slip past a single-crate test run.
- [ ] **`cargo doc --workspace --no-deps --document-private-items`** with
      **zero warnings** in rustdoc output. Capture to a file and grep for
      `warning:` to be safe:

      ```sh
      cargo doc --workspace --no-deps --document-private-items 2>&1 \
          | tee /tmp/doc.log
      ! grep -E '^warning:' /tmp/doc.log
      ```
      *Prevents:* dead intra-doc links that break under the mdBook build.
- [ ] **`cargo audit --deny warnings`** — RustSec database clean.
      *Prevents:* shipping a known-CVE transitive dep.
- [ ] **`cargo deny check`** — licences, advisories, bans, sources (see
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
- **`audit.toml` mirrors `deny.toml`.** The nightly
  `deny-audit` CI job runs `cargo audit` against the live RustSec feed
  and the CLI no longer passes per-advisory `--ignore` flags — the
  ignore list is read from `audit.toml` so the two files cannot drift.
  If you add an entry to `deny.toml`, add the same entry to
  `audit.toml` in the same PR.

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
cargo llvm-cov --workspace --summary-only
```

- [ ] Workspace floor: **≥ 65 %** (ratcheting up toward 80 % by
      `bd-1du.10` close). See `testing.md §6`.
- [ ] Per-crate floors override the workspace number for security-critical
      crates: `pcloud-secret 90 %`, `pcloud-crypto 85 %`,
      `pcloud-auth 85 %`, `pcloud-resilience 80 %`, `pcloud-ipc 80 %`.
- [ ] The floor never lowers automatically; if a release requires a
      temporary dip, land an explicit commit updating
      `ci/coverage-floor.toml` with a changelog entry first.

*Why:* coverage is a crude but leading indicator. A drop on a release tag is
the single strongest signal of deletion-without-replacement.

### 4.3 Benchmark regression guard (blocking, >10%)

```sh
cargo bench -p pcloud-bench -- chunked_flush upload_session page_cache_evict
```

- [ ] Compare against `target/criterion/` baselines committed to the
      release branch. **No benchmark regresses more than 10%** versus the
      previous release baseline.
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
      marked `text`. The mdBook-compile-check in CI enforces this.
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

### 4.7 Live E2E (blocking)

```sh
PCLOUD_LIVE_E2E=1 \
PCLOUD_E2E_USERNAME=staging-bot@example.com \
PCLOUD_E2E_PASSWORD=… \
cargo test -p pcloud-live-e2e --locked
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

All builds use `--profile release-repro` with `SOURCE_DATE_EPOCH` pinned to
the tag commit time (`git log -1 --pretty=%ct`). See
`reproducible-builds.md`.

#### Linux (x86_64, aarch64) — blocking

- [ ] Build under the release container (Debian stable + pinned toolchain)
      for glibc compatibility.
- [ ] Artefacts: `.deb` (cargo-deb), `.rpm` (cargo-generate-rpm),
      `.tar.xz`, `.AppImage` (linuxdeploy).
- [ ] `ldd` confirms no unexpected dynamic deps.
- [ ] `readelf -n` confirms deterministic build-id or absent per policy.

#### macOS (universal) — blocking when macOS is in scope

- [ ] `MACOSX_DEPLOYMENT_TARGET=12.0`.
- [ ] Lipo-joined `.pkg`, signed with Developer ID Application cert,
      notarised via `xcrun notarytool submit --wait`, stapled.
- [ ] `spctl -a -vv` reports accepted.

#### Windows (x86_64) — blocking when Windows is in scope

- [ ] MSI via WiX, signed with the EV HSM-backed cert.
- [ ] SmartScreen submission for new version families.

#### BSD ports — informational

- [ ] Source tarball published; ports committers notified.

### 4.11 Integrity and signatures (blocking)

```sh
sha256sum pcloud-rs-*.{tar.xz,deb,rpm,AppImage,pkg,msi} > SHA256SUMS
gpg --detach-sign --armor SHA256SUMS
```

- [ ] The release GPG key is distinct from platform code-signing certs.
      Its public half is in `SECURITY.md` and on `keys.openpgp.org`.
- [ ] Verify the signature from a fresh checkout on a clean VM before
      proceeding:

      ```sh
      gpg --verify SHA256SUMS.asc SHA256SUMS
      sha256sum -c SHA256SUMS
      ```

- [ ] **CI packaging pipeline (blocking).** `.github/workflows/packaging.yml`
      fires on `release: published` and produces a signed artifact per
      target. For every release verify:

      - The six packaging jobs (`linux-deb-rpm`, `linux-appimage`,
        `linux-flatpak`, `macos-pkg`, `windows-msi`, `docker-image`) all
        completed with status `success` (Flatpak is advisory today — see
        packaging-matrix §12b).
      - Each blob artifact attached to the release has a paired `<file>.sig`
        and `<file>.pem`. Pick one and verify from a clean VM:

        ```sh
        cosign verify-blob \
          --certificate-identity-regexp "https://github.com/${REPO}/.github/workflows/packaging.yml@.*" \
          --certificate-oidc-issuer https://token.actions.githubusercontent.com \
          --signature <file>.sig --certificate <file>.pem <file>
        ```

      - The Docker image has a cosign OCI signature:

        ```sh
        cosign verify ghcr.io/${REPO}/pcloud-rs:<version> \
          --certificate-identity-regexp "https://github.com/${REPO}/.github/workflows/packaging.yml@.*" \
          --certificate-oidc-issuer https://token.actions.githubusercontent.com
        ```

      - `release-artifacts.txt` is present and lists every artifact with a
        SHA256 line.

- [ ] **Supply-chain PR gates (blocking, per commit).** Confirm the
      following jobs are green on the release commit in `rust.yml`:
      `cargo deny check` (`deny` job), `cargo doc --workspace --no-deps`
      under `RUSTDOCFLAGS="-D warnings"` (`doc` job), and the PR-path
      `cargo audit` (`audit` job). The nightly `deny-audit` run against
      the live RustSec feed must be green within 24h of the tag.

- [ ] **Scaffolded signing paths.** Apple Dev-ID (`macos-pkg`) and
      Windows EV (`windows-msi`) are intentionally marked
      `continue-on-error` until certs land; their failures are
      informational and **must not** be used to claim the release is
      signed end-to-end. The authoritative supply-chain signature is
      cosign keyless until that changes.

### 4.12 Publication (blocking)

- [ ] GitHub Release from the signed tag; title `v<version>`.
- [ ] Upload every artefact + `SHA256SUMS` + `SHA256SUMS.asc`.
- [ ] Paste the `CHANGELOG.md` block into the release notes.
- [ ] Mark pre-releases as such; only promote to "Latest" after §4.14 smoke
      tests pass.

### 4.13 Package manifests — warning (async)

Each may land asynchronously. None block the tag.

- [ ] Homebrew tap (`pcloud-rs.rb`).
- [ ] Chocolatey (`pcloud-rs.nuspec`, `chocolateyinstall.ps1`).
- [ ] winget (`manifests/p/pcloudcom/pcloud-rs/<version>/`).
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
Signed by the release GPG key (fingerprint `<FP>`, also in SECURITY.md).
Reproduce:

```sh
export SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct v<version>)
cargo build --profile release-repro --locked -p pcloud-cli
sha256sum target/release-repro/pcloud-cli
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
- [ ] coverage ≥ floor, per-crate floors met
- [ ] benchmarks within 10 % of baseline
- [ ] mdBook + rustdoc 0 warnings
- [ ] parity matrix unchanged or growing
- [ ] live E2E green within 24 h
- [ ] CHANGELOG entry + version bump committed
- [ ] tag GPG-signed
- [ ] artefacts built under `release-repro` with pinned
      `SOURCE_DATE_EPOCH`
- [ ] `SHA256SUMS.asc` signed + verified on clean VM
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

**Q: Can a release go out with `bd-1du.10` open?**
A: Yes, as a pre-alpha, but the release notes must say so explicitly and
must not claim parity. Once `bd-1du.10` closes, parity claims unlock.

**Q: Do package-manager manifests block the tag?**
A: No. §4.13 is async. The blocking path ends at §4.14 smoke tests.

**Q: What key signs the release?**
A: The release GPG key, distinct from platform code-signing certs, held in
the release team vault with an offline HSM backup. Fingerprint is in
`SECURITY.md`.
