# Packaging reference

> Authoritative sources: the `packaging/` subtree at the repo root and
> `.github/workflows/`. The operations-view summary table lives at
> [`operations/packaging-matrix.md`](../operations/packaging-matrix.md);
> this page is the deep per-channel reference. Anything that conflicts
> with either file is wrong — the code and workflow YAMLs win.

> **Honesty callout (2026-04-16).** The project is pre-alpha. Every
> channel below has a recipe in-tree, but only Linux Docker / OCI
> (cosign, keyless OIDC) and the Linux nfpm-equivalent GPG paths are
> proven end-to-end in CI. macOS notarisation, Windows Authenticode EV
> signing, and the full `.deb`/`.rpm` GPG release-key handoff are all
> vendor-bound on credentials and await the first signed tag. The
> signing **scripts** are present under `packaging/signing/`; the
> **credentials** are not.

## Who this page is for

- **Downstream packagers**: skip to the per-format section
  ([Linux](#linux-channels) / [macOS](#macos-channels) /
  [Windows](#windows-channels) / [BSD](#bsd-channels)). Each section
  lists the recipe path, build command, output artefact, signing
  posture, install layout, and honest status.
- **Release engineers**: read
  [Signing pipelines](#signing-pipelines) for the current vs
  target state of each credential, and
  [CI matrix](#ci-matrix) for what the GitHub Actions workflow does
  per platform.
- **Auditors**: focus on
  [Reproducibility](#reproducibility) and the
  [Credential inventory](#credential-inventory) table under signing.

## Directory layout

Every channel lives under `packaging/` at the repo root (not under
``). See also
[`packaging/README.md`](../../../../packaging/README.md) for the
in-tree index.

| Path | Scope | Key files |
|---|---|---|
| `packaging/appimage/` | Linux AppImage | `AppRun`, `build-appimage.sh`, `pcloud-rs.desktop` |
| `packaging/bsd/` | Shared BSD notes | `README.md` |
| `packaging/chocolatey/` | Windows Chocolatey | `pcloud-rs.nuspec`, `tools/` |
| `packaging/docker/` | OCI container image (cosign-signed) | `Dockerfile`, `entrypoint.sh` |
| `packaging/flatpak/` | Linux Flatpak | `com.pcloud.pcloud-rs.yaml`, `.metainfo.xml`, `.desktop` |
| `packaging/freebsd/` | FreeBSD rc.d | `pcloudd.rc` |
| `packaging/homebrew/` | macOS Homebrew | `pcloud-rs.rb`, `Casks/` |
| `packaging/macos/` | launchd + entitlements | `com.pcloud.pcloudd.plist`, `com.pcloud.pcloud-rs.plist`, `entitlements.plist` |
| `packaging/man/` | Man pages | `pcloudc.1`, `pcloudd.1`, `pcloud.conf.5` |
| `packaging/netbsd/` | NetBSD rc.d | `pcloudd` |
| `packaging/openbsd/` | OpenBSD rc.d | `pcloudd` |
| `packaging/scoop/` | Windows Scoop bucket | `pcloud-rs.json` |
| `packaging/signing/` | Cross-channel signing wrappers | `sign-macos.sh`, `notarize-macos.sh`, `sign-windows.ps1` |
| `packaging/snap/` | Linux Snap | `snapcraft.yaml` |
| `packaging/windows/wix/` | WiX MSI source | `pcloud-rs.wxs`, `License.rtf` |
| `packaging/winget/` | Windows winget | `pcloud-rs.yaml` |

The product binary is called `pcloud-rs` in packaging metadata (to
distinguish it from the legacy C `pcloud-rs`). The CLI exposes both
binaries: `pcloudc` (client) and `pcloudd` (daemon).

## Linux channels

### Debian / Ubuntu — `.deb`

- **Recipe**: not committed as a single file; nfpm-style packaging is
  produced from the release workflow using the `packaging/man/` man
  pages and the `packaging/snap/`/`flatpak/` desktop assets.
- **Build command (local)**: `cargo build --release -p pcloud-cli -p
  pcloud-daemon` followed by an nfpm run (nfpm config committed at
  tag time).
- **Artefact**: `pcloud-rs_<version>_amd64.deb` (also `arm64`).
- **Signing**: GPG detached signature with the release key.
- **Install layout**:
  - `/usr/bin/pcloudc`
  - `/usr/sbin/pcloudd`
  - `/etc/pcloud/pcloud.conf.example`
  - `/usr/share/man/man1/pcloudc.1`, `pcloudd.1`
  - `/usr/share/man/man5/pcloud.conf.5`
  - `/lib/systemd/system/pcloudd.service`
- **Post-install**: `systemctl daemon-reload`; service is **not**
  auto-enabled.

### Fedora / RHEL / openSUSE — `.rpm`

- **Recipe**: same nfpm pipeline as `.deb`; RPM-specific scriptlets
  use `%systemd_post` / `%systemd_preun`.
- **Artefact**: `pcloud-rs-<version>-<rel>.x86_64.rpm`.
- **Signing**: GPG detached signature + RPM header signature.
- **Install layout**: identical to `.deb`.

### Arch Linux (AUR) — `PKGBUILD`

- **Recipe**: downstream-maintained `pcloud-rs` / `pcloud-rs-git`
  PKGBUILDs pointing at release tag tarballs. Hashes pinned at tag
  time.
- **Build command**: `makepkg -s` in the AUR working directory.
- **Artefact**: `pcloud-rs-<version>-<rel>-x86_64.pkg.tar.zst`.
- **Signing**: maintainer GPG.

### Nix / NixOS

- **Recipe**: `flake.nix` at the repo root (not under `packaging/`).
  Exposes `packages.<system>.pcloud-rs` and a
  `nixosModules.pcloud-rs` module.
- **Build command**: `nix build .#pcloud-rs`.
- **Artefact**: `$out/bin/{pcloudc,pcloudd}` in the Nix store.
- **Signing**: Nix store hash (no external signature required).
- **Reproducibility**: `flake.lock` is committed; the toolchain is
  pinned via `rust-overlay`.

### Flatpak

- **Recipe**: `packaging/flatpak/com.pcloud.pcloud-rs.yaml`, targeting
  `org.freedesktop.Platform//23.08` (or the version pinned in the
  manifest).
- **Build command**:
  `flatpak-builder --install-deps-from=flathub build/
  packaging/flatpak/com.pcloud.pcloud-rs.yaml`.
- **Artefact**: Flatpak export ready for `flatpak build-bundle`.
- **Signing**: Flathub GPG (inherited from the submission pipeline).
- **Sandbox posture**: network is granted only to the pCloud API host
  via `--share=network` + finish-args allow-list.

### Snap

- **Recipe**: `packaging/snap/snapcraft.yaml`.
- **Build command**: `snapcraft` in the `packaging/snap/` directory
  (or via `snapcraft remote-build`).
- **Artefact**: `pcloud-rs_<version>_amd64.snap`.
- **Signing**: Snap Store key (handled by `snapcraft upload`).
- **Confinement**: `strict`. The `fuse-support` and `removable-media`
  interfaces are **declared but not auto-connected** — operators
  connect manually (`snap connect pcloud-rs:fuse-support`).

### AppImage

- **Recipe**: `packaging/appimage/AppRun`,
  `packaging/appimage/build-appimage.sh`,
  `packaging/appimage/pcloud-rs.desktop`.
- **Build command**: `bash packaging/appimage/build-appimage.sh`.
- **Artefact**: `pcloud-rs-<version>-x86_64.AppImage` plus zsync
  update file.
- **Signing**: `appimagetool --sign` with the release GPG key.

### Docker / OCI

- **Recipe**: `packaging/docker/Dockerfile`,
  `packaging/docker/entrypoint.sh`.
- **Build command**:
  `docker build -f packaging/docker/Dockerfile -t pcloud-rs/pcloud-rs:<tag> .`
- **Artefact**: OCI image pushed to GHCR.
- **Signing**: `cosign sign --yes` (keyless, sigstore, OIDC-backed).
  No long-lived secret required.
- **Verification**:
  ```bash
  cosign verify ghcr.io/pcloud-rs/pcloud-rs:<tag> \
    --certificate-identity-regexp 'https://github\.com/pcloud-rs/pcloud-rs/\.github/workflows/.+' \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com
  ```

## macOS channels

### Homebrew tap / cask

- **Recipe**: `packaging/homebrew/pcloud-rs.rb`;
  `packaging/homebrew/Casks/` holds the cask variant that ships the
  signed `.pkg` installer.
- **Build command**:
  `brew tap pcloud-rs/pcloud-rs && brew install pcloud-rs`.
- **Artefact**: formula (source build) or bottle (pre-built).
- **Signing posture**: bottles inherit the Developer ID signature
  applied during the release build (**pending** — see below).

### launchd integration

- **Recipe**:
  - `packaging/macos/com.pcloud.pcloudd.plist` — LaunchDaemon (system
    scope).
  - `packaging/macos/com.pcloud.pcloud-rs.plist` — LaunchAgent (user
    scope).
  - `packaging/macos/entitlements.plist` — minimum entitlements for
    the hardened runtime.

### Signed `.pkg`

- **Recipe**: invoked via CI; uses `pkgbuild` + `productsign` +
  `notarytool` wrapped by
  `packaging/signing/sign-macos.sh` and
  `packaging/signing/notarize-macos.sh`.
- **Artefact**: `pcloud-rs-<version>.pkg`.
- **Signing**:
  1. `codesign --options runtime --timestamp --entitlements
     entitlements.plist --sign "Developer ID Application: ..."` each
     binary.
  2. `pkgbuild` → raw installer.
  3. `productsign --sign "Developer ID Installer: ..."`.
  4. `xcrun notarytool submit --wait` → `stapler staple`.
- **Status**: **pending** first notarised artefact. An active Apple
  Developer ID + app-specific password must be provisioned in CI
  secrets. See
  [`packaging/signing/README.md`](../../../../packaging/signing/README.md)
  §7 for the first-time runbook.

## Windows channels

### WiX MSI

- **Recipe**: `packaging/windows/wix/pcloud-rs.wxs` (also
  `License.rtf`).
- **Build command**:
  `candle pcloud-rs.wxs && light pcloud-rs.wixobj -o pcloud-rs-<version>-x64.msi`.
- **Artefact**: `pcloud-rs-<version>-x64.msi`.
- **Signing**: Authenticode EV via
  `packaging/signing/sign-windows.ps1`
  (wraps `signtool.exe /fd SHA256 /tr <timestamp> /td SHA256`).
  Dual-timestamping supported.
- **Status**: **stub** — EV cert (HSM token or cloud signing) is
  vendor-bound; the MSI is emitted unsigned by default until the
  credential is provisioned. See
  [`packaging/signing/README.md`](../../../../packaging/signing/README.md)
  §2 for EV-cert acquisition guidance.
- **Install layout**: `%ProgramFiles%\pCloud\{pcloudc.exe,pcloudd.exe}`
  registered as a Windows Service via a WiX custom action.

### winget

- **Recipe**: `packaging/winget/pcloud-rs.yaml` (submitted to
  `microsoft/winget-pkgs`).
- **Artefact**: manifest pointing at the signed MSI on GitHub
  Releases.
- **Signing**: inherits Authenticode from the MSI (no extra
  signature).

### Chocolatey

- **Recipe**: `packaging/chocolatey/pcloud-rs.nuspec` +
  `packaging/chocolatey/tools/chocolateyinstall.ps1`.
- **Build command**: `choco pack` in that directory.
- **Artefact**: `pcloud-rs.<version>.nupkg`.
- **Install**: `choco install pcloud-rs` downloads + verifies the MSI
  SHA-256, then chains `msiexec /i`.
- **Signing**: inherits Authenticode from the MSI.

### Scoop

- **Recipe**: `packaging/scoop/pcloud-rs.json`.
- **Install**:
  `scoop bucket add pcloud-rs <url> && scoop install pcloud-rs`.
- **Artefact**: portable `.zip` (no SCM registration). SHA-256 is
  verified by Scoop itself; inherits MSI Authenticode when the MSI
  variant is selected.

## BSD channels

### FreeBSD

- **Recipe**: `packaging/freebsd/pcloudd.rc`. The ports `Makefile`
  lives downstream in `/usr/ports/net/pcloud-rs/`.
- **Artefact**: ports tarball via `make package`.
- **Signing**: ports tree signing.

### OpenBSD

- **Recipe**: `packaging/openbsd/pcloudd` rc.d script; ports
  `Makefile` is maintained downstream.
- **Signing**: ports tree signing (signify).

### NetBSD

- **Recipe**: `packaging/netbsd/pcloudd`; pkgsrc `Makefile` is
  maintained downstream.
- **Signing**: pkgsrc signing.

> All three BSDs have rc.d scripts in-tree but **no live mount proof**
> today — see `bd-1du.4`. The product compiles and the daemon runs;
> the FUSE/`puffs`/`fusefs` paths are scaffolded only.

## Signing pipelines

### Credential inventory

| Channel | Credential | Format | Status |
|---|---|---|---|
| Docker / OCI | GitHub OIDC identity → sigstore | ephemeral (keyless) | **Live** |
| Linux `.deb` / `.rpm` / AppImage / tarball | Release GPG key (4096 RSA, signing subkey) | detached `.sig` | Wired; release-key handoff pending |
| macOS `codesign` | Developer ID Application cert (`.p12`) | Keychain | **Pending Developer ID enrolment** |
| macOS `productsign` | Developer ID Installer cert | Keychain | **Pending enrolment** |
| macOS notarisation | Apple ID + app-specific password + Team ID | `notarytool` | **Pending** |
| Windows Authenticode | EV code-signing cert (HSM token or cloud HSM) | `.pfx` or KSP | **Pending EV issuance** |
| Windows timestamp | RFC 3161 URL (DigiCert / Sectigo / GlobalSign) | public | ready |

### Scripts

Live under [`packaging/signing/`](../../../../packaging/signing/):

- `sign-macos.sh` — `codesign` + `productsign` wrapper. Enforces
  `--options runtime` and `--timestamp`.
- `notarize-macos.sh` — `xcrun notarytool submit --wait` +
  `stapler staple` + `stapler validate`.
- `sign-windows.ps1` — `signtool.exe` wrapper (EV token or cloud
  HSM).
- `README.md` — full operator guide, including EV cert acquisition,
  CI keychain setup, rejection rollback plan.

### Sigstore cosign (OCI images)

- Workflow: `.github/workflows/docker.yml` pushes the image, then
  `cosign sign --yes <image>@<digest>`.
- Identity: `github.com/pcloud-rs/pcloud-rs/.github/workflows/docker.yml`
  at `refs/tags/v*` (pinned by the verification command under
  [Docker / OCI](#docker--oci)).
- No long-lived secret. The GitHub OIDC token is issued per-run and
  exchanged with Fulcio for a short-lived signing cert.

### GPG (Linux artefacts)

- Release key fingerprint is published in the README at release time
  (not hard-coded here to avoid drift).
- Detached signatures are emitted for every `.deb`, `.rpm`,
  `.AppImage`, and for the `SHA256SUMS.txt` roll-up.
- Key rotation: tracked in the release checklist; requires republishing
  artefacts with the new signature and advising downstream packagers.

### Apple Developer ID + notarisation

- Hardened runtime: enabled via `--options runtime` in
  `sign-macos.sh`.
- Entitlements: `packaging/macos/entitlements.plist` — the file
  intentionally requests the **minimum** entitlements. Enabling
  `com.apple.security.cs.disable-library-validation` (needed for
  fuse-t on some configurations) is gated behind explicit review
  because it weakens hardened-runtime DYLD protections.
- CI: `.github/workflows/release.yml` imports the `.p12` into an
  ephemeral build keychain, signs, and runs
  `xcrun notarytool submit --wait` + `stapler`. See
  `packaging/signing/README.md` §7 for the first-time runbook,
  including the dry-run on a `v0.0.0-rc1` tag.

### Windows Authenticode EV

- Token options (cost/tradeoff table in
  [`packaging/signing/README.md`](../../../../packaging/signing/README.md)
  §2): DigiCert USB SafeNet token, SSL.com eSigner cloud, DigiCert
  KeyLocker, Azure Key Vault with an imported EV cert. CI compatibility
  varies by issuer.
- `sign-windows.ps1` abstracts over PFX vs KSP vs cloud-HSM; the CI
  workflow picks the invocation based on which secrets are populated.

## Reproducibility

- `cargo build --locked --release`: `Cargo.lock` is committed and
  pinned. `--locked` refuses to update it at release time.
- `SOURCE_DATE_EPOCH`: every release job sets
  `SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct "$GITHUB_REF_NAME")`.
  Most toolchains honour it transitively.
- `flake.lock`: pins `rust-overlay`, `nixpkgs`, and any other flake
  input. A NixOS rebuilder can reproduce the tree byte-for-byte from
  the lock file.
- Build-info embed: the binary embeds the short commit hash and
  `rustc` version via a `build.rs` so that signed artefacts can be
  traced back to a reproducible commit.
- **Never modify artefact contents after signing.** Signing is a leaf
  step. If a post-sign tweak is needed, re-build, re-sign.

## CI matrix

The release workflow at `.github/workflows/release.yml` fans out per
OS:

| Job | Runs on | Produces | Signs with | Current state |
|---|---|---|---|---|
| `build-linux-deb-rpm` | `ubuntu-latest` | `.deb`, `.rpm` | release GPG | wired; needs release-key rotation doc |
| `build-linux-appimage` | `ubuntu-latest` | `.AppImage` + `.zsync` | release GPG | wired |
| `build-linux-flatpak` | `ubuntu-latest` | Flatpak export | Flathub GPG | submitted via Flathub |
| `build-linux-snap` | `ubuntu-latest` (or LXD) | `.snap` | Snap Store | wired |
| `build-docker` | `ubuntu-latest` | OCI image | cosign (keyless OIDC) | **live** |
| `build-macos` | `macos-14` | signed `.pkg` | Developer ID + notarytool | pending credential |
| `build-windows` | `windows-latest` | `.msi` | Authenticode EV | pending credential |
| `slsa-provenance` | `ubuntu-latest` | `*.intoto.jsonl` | `slsa-framework/slsa-github-generator` | wired |

The rust-specific PR workflow (`.github/workflows/rust.yml`) only
tests the workspace; it does not produce artefacts. The C-legacy
workflow (`.github/workflows/c-cpp.yml`) is unrelated to packaging.

## SLSA provenance

Release artefacts carry a SLSA v1.0 provenance attestation produced by
the `slsa-framework/slsa-github-generator` action. Verify with:

```bash
slsa-verifier verify-artifact <artifact> \
  --provenance-path <artifact>.intoto.jsonl \
  --source-uri github.com/pcloud-rs/pcloud-rs
```

## See also

- [`operations/packaging-matrix.md`](../operations/packaging-matrix.md)
  — operations-view summary table (tier, service-manager entry,
  signing posture).
- [`packaging/README.md`](../../../../packaging/README.md) — in-tree
  index with the honest per-channel status.
- [`packaging/signing/README.md`](../../../../packaging/signing/README.md)
  — deep operator guide for Apple + Windows signing (cert acquisition,
  CI keychain setup, rejection rollback).
- [Release checklist](../development/release-checklist.md).
- [Reproducible builds](../development/reproducible-builds.md).
- [`PLAN_CROSSPLATFORM.md`](../../../../PLAN_CROSSPLATFORM.md)
  — phase landing record.
