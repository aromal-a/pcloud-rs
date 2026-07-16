# Packaging reference

> Authoritative sources: the `packaging/` subtree at the repo root and
> `.github/workflows/`. The operations-view summary table lives at
> [`operations/packaging-matrix.md`](../operations/packaging-matrix.md);
> this page is the deep per-channel reference. Anything that conflicts
> with either file is wrong — the code and workflow YAMLs win.

> **Evidence callout (2026-07-16).** There is no public GitHub release and no
> published package channel. Linux raw and package jobs, strict
> signed/notarized macOS and signed Windows installer jobs, and Tier-2 NAS
> candidate jobs are defined in CI. Definitions are not successful-run
> evidence: consult the release-commit logs and signatures. Docker/GHCR and
> SLSA provenance remain unimplemented.

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

Every channel lives under `packaging/` at the repo root. See also
[`packaging/README.md`](../../../../packaging/README.md) for the
in-tree index.

| Path | Scope | Key files |
|---|---|---|
| `packaging/appimage/` | Linux AppImage | `AppRun`, `build-appimage.sh`, `pcloud-rs.desktop` |
| `packaging/bsd/` | Shared BSD notes | `README.md` |
| `packaging/chocolatey/` | Windows Chocolatey | `pcloud-rs.nuspec`, `tools/` |
| `packaging/docker/` | Local OCI container image recipe | `Dockerfile`, `docker-compose.yml` |
| `packaging/flatpak/` | Linux Flatpak | `com.pcloud.pcloud-rs.yaml`, `.metainfo.xml`, `.desktop` |
| `packaging/freebsd/` | FreeBSD rc.d | `pcloudd.rc` |
| `packaging/homebrew/` | macOS Homebrew | `pcloud-rs.rb`, `Casks/` |
| `packaging/macos/` | launchd + entitlements | `com.pcloud.pcloudd.plist`, `com.pcloud.pcloud-rs.plist`, `entitlements.plist` |
| `packaging/man/` | Man pages | `pcloudc.1`, `pcloudd.1`, `pcloud.conf.5` |
| `packaging/nas/` | Synology/QNAP/ASUSTOR | package builders, metadata, shared supervisor, validation |
| `packaging/netbsd/` | NetBSD rc.d | `pcloudd` |
| `packaging/openbsd/` | OpenBSD rc.d | `pcloudd` |
| `packaging/scoop/` | Windows Scoop bucket | `pcloud-rs.json` |
| `packaging/signing/` | Cross-channel signing wrappers | `sign-macos.sh`, `notarize-macos.sh`, `sign-windows.ps1` |
| `packaging/snap/` | Linux Snap | `snapcraft.yaml` |
| `packaging/windows/wix/` | WiX MSI/Burn source | `pcloud-rs.wxs`, `pcloud-rs-bundle.wxs`, `License.rtf` |
| `packaging/winget/` | Windows winget | `pcloud-rs.yaml` |

The package is called `pcloud-rs`; its two executable entry points are
`pcloudc` (client) and `pcloudd` (daemon).

## Linux channels

### Debian / Ubuntu — `.deb`

- **Recipe**: `cargo-deb` metadata in `crates/pcloud-daemon/Cargo.toml`.
- **Build command (local)**: `cargo build --release --workspace -p
  pcloud-cli -p pcloud-daemon` followed by `cargo deb --no-build
  --no-strip --package pcloud-daemon`.
- **Artefact**: `pcloud-rs_<version>_amd64.deb`.
- **Signing**: `release-packaging.yml` emits `SHA256SUMS` and GPG
  detached signatures when all release-key secrets are configured. Dry runs
  remain visibly unsigned when they are absent.
- **Install layout**:
  - `/usr/bin/pcloudc`
  - `/usr/bin/pcloudd`
  - `/etc/pcloud-rs/pcloudd.env.example`
  - `/usr/share/man/man1/pcloudc.1`, `pcloudd.1`
  - `/usr/share/man/man5/pcloud.conf.5`
  - `/lib/systemd/system/pcloudd.service`
- **Post-install**: `systemctl daemon-reload`; service is **not**
  auto-enabled.

### Fedora / RHEL / openSUSE — `.rpm`

- **Recipe**: `cargo-generate-rpm` metadata in
  `crates/pcloud-daemon/Cargo.toml`.
- **Artefact**: `pcloud-rs-<version>-<rel>.x86_64.rpm`.
- **Signing**: `release-packaging.yml` emits `SHA256SUMS` and GPG detached
  signatures when all release-key secrets are configured. No package has been
  published.
- **Install layout**: identical to `.deb`.

### Arch Linux (AUR)

No `pcloud-rs`, `pcloud-rs-bin`, or `pcloud-rs-git` AUR recipe is published.
There is no in-tree `PKGBUILD`; source builds are the only current path.

### Nix / NixOS

- **Recipe**: `flake.nix` at the repo root (not under `packaging/`).
  Exposes `packages.<system>.{pcloud-rs,pcloud-rs-repro,pcloudc,pcloudd}`
  and `apps.<system>.{pcloudc,pcloudd}`. `default` runs `pcloudc`.
  **No `nixosModules`
  output exists yet** — any documentation referring to
  `nixosModules.pcloud-rs` is incorrect; a NixOS service module is a
  planned contribution, not a current flake output.
- **Build command**: `nix build .#pcloud-rs`.
- **Artefact**: `$out/bin/{pcloudc,pcloudd}` in the Nix store.
  The `apps.pcloudd` and `apps.pcloudc` outputs point at the correct
  binary names (`pcloudc` and `pcloudd`).
- **Signing**: Nix store hash (no external signature required).
- **Reproducibility**: `flake.lock` is committed; the build is
  pinned via a nixpkgs revision in the flake inputs.

### Flatpak

- **Recipe**: `packaging/flatpak/com.pcloud.pcloud-rs.yaml`, targeting
  `org.freedesktop.Platform//24.08`.
- **Build command**:
  `flatpak-builder --install-deps-from=flathub build/
  packaging/flatpak/com.pcloud.pcloud-rs.yaml`.
- **Artefact**: Flatpak export ready for `flatpak build-bundle`.
- **Signing**: none locally. Flathub would sign a future accepted submission;
  no Flathub application exists today.
- **Sandbox posture**: `--share=network` grants general network access; Flatpak
  does not provide the host allow-list previously claimed here. The manifest is
  development scaffolding and still needs offline vendored sources.

### Snap

- **Recipe**: `packaging/snap/snapcraft.yaml`.
- **Build command**: `snapcraft` in the `packaging/snap/` directory
  (or via `snapcraft remote-build`).
- **Artefact**: local `pcloud-rs_<version>_amd64.snap` candidate.
- **Signing**: none locally. No Snap Store package exists.
- **Confinement**: `strict`. The `fuse-support` and `removable-media`
  interfaces are **declared but not auto-connected** — operators
  connect manually (`snap connect pcloud-rs:fuse-support`).

### AppImage

- **Recipe**: `packaging/appimage/AppRun`,
  `packaging/appimage/build-appimage.sh`,
  `packaging/appimage/pcloud-rs.desktop`.
- **Build command**: `bash packaging/appimage/build-appimage.sh`.
- **Artefact**: local `pcloud-rs-<arch>.AppImage` candidate.
- **Signing**: unsigned by current CI; `appimagetool --sign` is a future
  release workflow step.

### Docker / OCI

- **Recipe**: `packaging/docker/Dockerfile` and
  `packaging/docker/docker-compose.yml`.
- **Build command**:
  `docker build -f packaging/docker/Dockerfile -t pcloud-rs/pcloud-rs:<tag> .`
- **Artefact**: local OCI image. No GHCR publish workflow exists today.
- **Signing**: none today. Add a Docker publish workflow before advertising
  cosign OCI signatures.

## macOS channels

### Homebrew formula / cask scaffold

- **Recipe**: `packaging/homebrew/pcloud-rs.rb`;
  `packaging/homebrew/Casks/` holds the cask variant that ships the
  signed `.pkg` installer.
- **Build status**: the formula still contains a future tag URL and checksum.
  There is no tap and no supported `brew install pcloud-rs` command.
- **Artefact**: no published formula, cask, or bottle.
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

### Signed and notarized `.pkg`

- **Recipe**: `packaging/macos/build-pkg.sh`, invoked by the
  `macos-installer` job in `release-packaging.yml` on a labelled native
  fuse-t runner.
- **Artefact**: `pcloud-rs-<version>-macos-<arch>.pkg`.
- **Signing**:
  1. `codesign --options runtime --timestamp --entitlements
     entitlements.plist --sign "Developer ID Application: ..."` each
     binary.
  2. `pkgbuild` → raw installer.
  3. `productsign --sign "Developer ID Installer: ..."`.
  4. `xcrun notarytool submit --wait` → `stapler staple`.
- **Gate**: required secrets are checked before build; native fuse-t
  read/write/unmount tests must pass; `pkgutil`, `spctl`, and stapler validate
  the result before upload. A release is supported only after this job has
  actually passed with active Apple credentials. See
  [`packaging/signing/README.md`](../../../../packaging/signing/README.md)
  §7 for the first-time runbook.

## Windows channels

### WiX MSI and WinFSP Burn bootstrapper

- **Recipe**: `packaging/windows/wix/pcloud-rs.wxs` and
  `pcloud-rs-bundle.wxs`.
- **Build command**:
  `candle pcloud-rs.wxs && light pcloud-rs.wixobj -o pcloud-rs-<version>-x64.msi`.
- **Artefacts**: signed `pcloud-rs-<version>-x64.msi` and a signed
  `pcloud-rs-<version>-x64-setup.exe` bundle containing the pcloud-rs MSI and
  checksum-pinned, vendor-signature-verified WinFSP 2.1 MSI.
- **Signing**: Authenticode EV via
  `packaging/signing/sign-windows.ps1`
  (wraps `signtool.exe /fd SHA256 /tr <timestamp> /td SHA256`).
- **Gate**: the public job fails if its PFX/password secrets are absent. Rust
  executables, MSI, detached Burn engine, and final bundle are signed in the
  required WiX sequence. A supported release still requires a successful
  native run with a real credential. See
  [`packaging/signing/README.md`](../../../../packaging/signing/README.md)
  §2 for EV-cert acquisition guidance.
- **Install layout**: `%ProgramFiles%\pcloud-rs\{pcloudc.exe,pcloudd.exe}`.
  `pcloudc start` launches the daemon under the interactive user's SID; the
  MSI deliberately does not register an incompatible machine service.

### winget

- **Recipe**: `packaging/winget/pcloud-rs.yaml`.
- **Artefact**: channel manifest intended to point at a versioned signed MSI.
- **Signing**: inherits Authenticode from the strict MSI workflow once its
  placeholder version, URL, and hash are updated.

### Chocolatey

- **Recipe**: `packaging/chocolatey/pcloud-rs.nuspec` +
  `packaging/chocolatey/tools/chocolateyinstall.ps1`.
- **Build command**: `choco pack` in that directory.
- **Artefact**: `pcloud-rs.<version>.nupkg`.
- **Install**: scaffold package downloads + verifies the MSI SHA-256, then
  chains `msiexec /i`.
- **Signing**: inherits Authenticode from the strict MSI workflow once the
  channel manifest is updated.

### Scoop

- **Recipe**: `packaging/scoop/pcloud-rs.json`.
- **Install**: unavailable; no public bucket exists.
- **Artefact**: scaffold portable `.zip` manifest. SHA-256 is verified by
  Scoop itself; Authenticode inheritance requires a signed MSI/ZIP workflow.

## BSD channels

### FreeBSD

- **Recipe**: in-tree `packaging/freebsd/pcloudd.rc` lifecycle asset only.
- **Publication**: no downstream port or binary package exists.

### OpenBSD

- **Recipe**: in-tree `packaging/openbsd/pcloudd` rc.d asset only.
- **Publication**: no downstream port or binary package exists.

### NetBSD

- **Recipe**: in-tree `packaging/netbsd/pcloudd` rc.d asset only.
- **Publication**: no downstream pkgsrc package exists.

### DragonFly BSD

- **Recipe**: `packaging/dragonfly/pcloudd` plus the deterministic Unix
  candidate builder. No downstream port or binary package exists.

> FreeBSD, NetBSD, OpenBSD, and DragonFly BSD have explicit native
> workspace and live FUSE jobs. Those definitions become proof only after the
> corresponding release-commit jobs pass.

## NAS channels (Tier 2 candidates)

- **Synology DSM 7:** `packaging/nas/synology/build-spk.sh` produces one
  `.spk` per architecture.
- **QNAP QTS/QuTS hero:** `packaging/nas/qnap/build-qpkg.sh` uses the pinned
  official QDK to produce and re-extract a `.qpkg`.
- **ASUSTOR ADM 5:** `packaging/nas/asustor/build-apk.sh` uses ASUSTOR's
  checksum-pinned official APKG tool and a validated 90×90 PNG.
- **CI posture:** x86-64 and arm64 static-musl payload candidates are uploaded
  as Actions artifacts, not attached to the public release.
- **Qualification:** install, upgrade, start/stop, reboot,
  uninstall/reinstall, live transfer, and optional FUSE testing on vendor
  hardware are mandatory before promotion.

## Signing pipelines

### Credential inventory

| Channel | Credential | Format | Status |
|---|---|---|---|
| Raw Linux binaries + SBOMs | GitHub OIDC identity → sigstore | ephemeral (keyless) | **Live in `release.yml`** |
| Linux `.deb` / `.rpm` | release GPG key, passphrase, and key id | armored key in an ephemeral keyring | workflow signs when secrets exist; public policy must require them |
| Docker / OCI | none today | n/a | Publish/sign workflow pending |
| Linux AppImage / tarball | none today | n/a | Release workflow pending |
| macOS signing | Developer ID Application + Installer identities | ephemeral Keychain imported from `.p12` | mandatory in strict job |
| macOS notarisation | Apple ID + app-specific password + Team ID | `notarytool` | mandatory in strict job |
| Windows Authenticode | code-signing certificate | ephemeral `.pfx` | mandatory in strict job |
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

### Sigstore cosign (raw Linux binaries and SBOMs)

- Workflow: `.github/workflows/release.yml` signs raw `pcloudd`,
  `pcloudc`, their `.sha256` files, and SBOM JSON files with
  `cosign sign-blob`.
- Identity: this repository's `release.yml` workflow at the selected tag.
- No long-lived secret is required for the keyless path. The GitHub OIDC
  token is issued per-run and exchanged with Fulcio for a short-lived
  signing cert. If `COSIGN_KEY` is configured, the workflow falls back to
  key-based signatures and no `.pem` certificate is emitted.

### GPG (Linux artefacts)

- Release key fingerprint is published in the README at release time
  (not hard-coded here to avoid drift).
- `release-packaging.yml` emits detached armored signatures for `.deb`,
  `.rpm`, and `SHA256SUMS` when all GPG release secrets exist. It logs and
  continues without signatures for contributor dry runs, so public promotion
  policy must verify that the expected signature files exist.
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
- CI: `.github/workflows/release-packaging.yml` imports the `.p12` into an
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
- The current strict CI path imports a base64-encoded PFX into an ephemeral
  runner file and uses `sign-windows.ps1`. CSP/KSP and cloud-HSM providers
  require a separate provider-specific integration and are not selected
  automatically.

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

## Local CI/CD matrix

GitHub Actions is inactive. `cargo xtask ci` is the authoritative gate:

| Stage | Execution host | Scope |
|---|---|---|
| `cargo xtask compat` | local Linux/Unix host | portable-core/Wasmtime MSRV and optional features |
| `cargo xtask host` | local Linux/Unix host | fmt, check, Clippy, tests, docs, dependency audit |
| `cargo xtask coverage` | local Linux host | workspace LCOV plus live FUSE and 90% policy |
| `cargo xtask package` | local host | NAS/portable-Unix metadata and SDK packages |
| `cargo xtask docker` | local Docker daemon | OCI/musl build, CLI smoke, Debian compile, manpage lint |
| `cargo xtask windows` | native Windows over key-only SSH | MSVC check/test/build and named-pipe/WinFSP smoke |
| `cargo xtask release` | operator release host | full CI plus reproducible Linux binaries |

macOS signing/notarization and native BSD/Solaris qualification still require
operator-provided native hosts. Publishing and signing remain explicit
operator actions; the local pipeline does not upload artifacts by itself.

## SLSA provenance

No SLSA provenance is emitted by the current local pipeline. Treat SLSA as a
target-state requirement, not a current release property.

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
- [`architecture/platform-support.md`](../architecture/platform-support.md)
  — current platform support posture.
