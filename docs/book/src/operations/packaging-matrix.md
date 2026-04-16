# Packaging Matrix (Operations View)

## 1. Purpose

Consolidated, operations-facing packaging reference. Every target the
`pcloud-rs` Rust rewrite publishes (or is wired to publish) across the
cross-platform waves (X/Y/Z/W/V/U) is documented here with its OS
version range, packager recipe, install layout, service-manager entry,
signing posture, and known gaps.

For the deeper recipe reference (per-channel build steps, CI
invocation, artefact layout), see
[Reference → Packaging](../reference/packaging.md). For in-tree
assets, see [`packaging/README.md`](../../../../../packaging/README.md)
at the repository root.

> **Honesty header (2026-04-16).**
> - Linux is the **only live-tested mount path**.
> - macOS `fuse-t`, Windows WinFSP, and *BSD mounts are **scaffolded
>   but not hardware-verified**.
> - macOS `.pkg` notarisation is **pending an active Apple Developer
>   ID** (vendor-bound).
> - Windows MSI **Authenticode EV** signing is **stubbed** — the WiX
>   recipe emits an unsigned MSI today; the sign step awaits an EV
>   HSM token.
> - Linux `.deb` / `.rpm` GPG detached signatures and cosign OCI
>   signatures **are** reproducible and exercised in CI, but no tagged
>   release has been cut yet.

## 2. Prereqs

- Decide the tier of your rollout (T1 fully supported, T2/T3 scaffolded).
- Build toolchain for your target (rustc matching MSRV; `nfpm` / `wix`
  / `pkgbuild` / `rpmbuild` as per channel).
- Access to the signing identities for your target (see §3).
- CI or a clean container for reproducible builds.

## 3. Conceptual background

### Tiering

Tier definitions track `PLAN_CROSSPLATFORM.md §1`:

- **T1** — live CI, reproducible build, signed artefacts target,
  known-good install path. Linux distros, Homebrew, Windows MSI.
- **T2** — scaffolded; recipe exists, CI exercises it, no live
  install-base yet. FreeBSD.
- **T3** — ports / pkgsrc contributions; maintained upstream. OpenBSD,
  NetBSD.

### Signing posture glossary

- **cosign (sigstore)** — keyless OIDC signing of OCI images via
  GitHub Actions; verified by `cosign verify` with the same OIDC
  issuer identity.
- **GPG detached sig** — release key signs each artefact; clients
  verify with the release public key.
- **Authenticode EV** — Microsoft-approved EV code-signing
  certificate on a hardware HSM token (DigiCert / Sectigo).
- **Apple Developer ID** — Apple-issued Developer ID Installer
  certificate (macOS `.pkg`).
- **Apple notarisation** — Apple-side scan + staple; required for
  Gatekeeper-free install on modern macOS.
- **Hash-verified only** — no signature; integrity via recorded
  sha256 in a trusted manifest (Nix, Scoop).

### Honesty table — mount parity

| Platform | Mount runtime       | Hardware-tested?         | Status                                |
|----------|---------------------|--------------------------|---------------------------------------|
| Linux    | FUSE3 (`libfuse3`)  | **Yes (live-tested)**    | Supported                             |
| macOS    | `fuse-t`            | No                       | Scaffolded; `bd-1du.4` covers proof   |
| Windows  | WinFSP              | No                       | Scaffolded; `bd-1du.4` covers proof   |
| FreeBSD  | `fusefs`            | No                       | Scaffolded; ports recipe only         |
| OpenBSD  | `fusefs`            | No                       | Scaffolded; ports recipe only         |
| NetBSD   | `puffs` / `rump`    | No                       | Scaffolded; pkgsrc recipe only        |

Packaging recipes ship for every row; **packaging a binary does not
imply a live mount** on non-Linux targets today.

## 4. Consolidated target table

| Tier | OS / family          | Channel            | Format                     | OS version range          | Signing posture                                  | Install path (default)                                       | Service manager entry                       | Packager recipe                          |
|------|----------------------|--------------------|----------------------------|---------------------------|--------------------------------------------------|--------------------------------------------------------------|---------------------------------------------|------------------------------------------|
| T1   | Linux (glibc)        | Debian / Ubuntu    | `.deb` (nfpm)              | Debian 11+, Ubuntu 22.04+ | GPG detached sig (release key)                   | `/usr/bin/pcloudc`, `/usr/sbin/pcloudd`                      | systemd: `pcloudd.service` (user + system)  | `packaging/debian/` + nfpm               |
| T1   | Linux (glibc)        | Fedora / RHEL / SUSE | `.rpm` (nfpm)            | Fedora 38+, RHEL 9, SLES 15 | GPG detached sig + RPM header sig              | `/usr/bin/pcloudc`, `/usr/sbin/pcloudd`                      | systemd: `pcloudd.service`                  | nfpm RPM                                 |
| T1   | Linux (any)          | Nix / NixOS        | flake output               | NixOS 23.11+              | Nix store hash                                    | `$out/bin/pcloudc`                                           | systemd module (`nixos/pcloud-rs.nix`)       | repo `flake.nix`                          |
| T1   | Linux (any)          | AppImage           | `.AppImage`                | glibc ≥ 2.31              | GPG detached sig + zsync                          | portable (`$HOME/Applications`)                              | XDG autostart optional                      | `packaging/appimage/`                    |
| T1   | Linux (any)          | Flatpak            | Flatpak                    | freedesktop runtime 23.08 | Flathub GPG                                       | `/var/lib/flatpak/app/com.pcloud.pcloud-rs`                  | `systemctl --user` unit (per-app)           | `packaging/flatpak/`                     |
| T1   | Linux (any)          | Snap               | Snap                       | snapd 2.58+               | Snap store key                                    | `/snap/pcloud-rs/current`                                    | snapd-managed service                       | `packaging/snap/snapcraft.yaml`          |
| T1   | Linux (any)          | Arch (AUR)         | `PKGBUILD`                 | Arch rolling              | maintainer GPG                                    | `/usr/bin/pcloudc`                                           | systemd: `pcloudd.service`                  | AUR `PKGBUILD`                           |
| T1   | Linux (any)          | Docker / OCI       | OCI image                  | any OCI runtime           | **cosign (sigstore) keyless**                     | container-internal `/usr/local/bin/`                         | container PID 1 / compose                   | `packaging/docker/Dockerfile`            |
| T1   | macOS 12+            | Homebrew tap       | formula / cask             | macOS 12 Monterey+        | Developer ID (bottle) — **pending**               | `/opt/homebrew/bin/pcloudc` (ARM) / `/usr/local/bin/pcloudc` (Intel) | launchd: `com.pcloud.pcloudd.plist` | `packaging/homebrew/`                    |
| T1   | macOS 12+            | Signed `.pkg`      | `pkgbuild` + `productsign` | macOS 12 Monterey+        | Developer ID Installer + **notarisation pending** | `/Applications/pCloud.app`, `/usr/local/bin/`                | launchd LaunchDaemon                        | `packaging/macos/` + `packaging/signing/sign-macos.sh` |
| T1   | Windows 10 / 11      | WiX MSI            | `.msi`                     | Windows 10 1809+, 11      | Authenticode EV — **stub (vendor-bound)**         | `%ProgramFiles%\pCloud\`                                     | SCM: `pcloudd` service (automatic)          | `packaging/windows/wix/pcloud-rs.wxs`    |
| T1   | Windows 10 / 11      | winget             | manifest                   | Windows 10 1809+, 11      | inherits MSI Authenticode                         | via MSI                                                      | via MSI (SCM)                               | `packaging/winget/`                      |
| T1   | Windows 10 / 11      | Chocolatey         | `.nuspec`                  | Windows 10 1809+, 11      | inherits MSI Authenticode                         | `%ChocolateyInstall%\lib\pcloud-rs\`                         | via MSI (SCM)                               | `packaging/chocolatey/`                  |
| T1   | Windows 10 / 11      | Scoop              | bucket manifest            | Windows 10 1809+, 11      | SHA-256 + inherited MSI sig                       | `%USERPROFILE%\scoop\apps\pcloud-rs\`                        | user-scope wrapper (no SCM)                 | `packaging/scoop/`                       |
| T2   | FreeBSD 13+          | pkg / ports        | `Makefile` + rc.d          | FreeBSD 13.x, 14.x        | ports tree signing                                | `/usr/local/bin/pcloudc`                                     | rc.d: `/usr/local/etc/rc.d/pcloudd`         | `packaging/freebsd/pcloudd.rc`           |
| T3   | OpenBSD 7.x          | ports              | `Makefile` + rc.d          | OpenBSD 7.4+              | ports tree signing (signify)                      | `/usr/local/bin/pcloudc`                                     | rc.d: `/etc/rc.d/pcloudd`                   | `packaging/openbsd/pcloudd`              |
| T3   | NetBSD 10            | pkgsrc             | `Makefile` + rc.d          | NetBSD 10                 | pkgsrc signing                                    | `/usr/pkg/bin/pcloudc`                                       | rc.d: `/etc/rc.d/pcloudd`                   | `packaging/netbsd/pcloudd`               |

## 5. Per-row deep dive

### 5.1 Linux `.deb` (Debian / Ubuntu)

- **OS range.** Debian 11 "bullseye" and later; Ubuntu 22.04 "jammy"
  and later. Older glibc (< 2.31) unsupported.
- **Packager recipe.** `nfpm pkg --packager deb --config nfpm.yaml`
  driven from `packaging/debian/`.
- **Install layout.** `/usr/bin/pcloudc`, `/usr/sbin/pcloudd`,
  `systemd/user/pcloudd.service`, `etc/pcloud-rs/config.toml.example`.
- **Service-manager entry.** `pcloudd.service` (both `--user` and
  system units ship).
- **Signing.** `dpkg-sig` detached sig + release public key
  published.
- **Known gaps.** None for install; mount parity is Linux-tested.

### 5.2 Linux `.rpm` (Fedora / RHEL / SUSE)

- **OS range.** Fedora 38+, RHEL/Rocky/Alma 9, SLES 15 SP5+.
- **Packager recipe.** `nfpm pkg --packager rpm`.
- **Install layout.** Same as `.deb` plus RPM header signature.
- **Service-manager entry.** `pcloudd.service`.
- **Signing.** GPG detached sig + RPM header sig (both required).

### 5.3 Nix / NixOS flake

- **OS range.** NixOS 23.11+ and stable Nix 2.18+.
- **Packager recipe.** `flake.nix` at repo root; `nix build`.
- **Install layout.** `$out/bin/pcloudc` etc.; reproducible by store
  hash.
- **Service-manager entry.** `nixos/pcloud-rs.nix` module registers a
  systemd unit.
- **Signing.** Hash-verified only.
- **Known gaps.** Nix consumers pin by hash; rotation is a flake
  input update.

### 5.4 AppImage

- **OS range.** Any glibc ≥ 2.31.
- **Packager recipe.** `appimagetool` + `packaging/appimage/AppDir/`.
- **Install layout.** Portable `.AppImage`; conventional home is
  `$HOME/Applications`.
- **Service-manager entry.** Optional XDG autostart `.desktop`.
- **Signing.** GPG detached sig + zsync file for deltas.

### 5.5 Flatpak

- **OS range.** freedesktop runtime 23.08.
- **Packager recipe.** `packaging/flatpak/com.pcloud.pcloud-rs.yml`.
- **Install layout.** Flathub path `/var/lib/flatpak/app/...`.
- **Service-manager entry.** `systemctl --user` unit scoped to the
  Flatpak.
- **Signing.** Flathub GPG.
- **Known gaps.** FUSE mount within the Flatpak sandbox requires
  additional finish-args; document in the Flatpak manifest.

### 5.6 Snap

- **OS range.** snapd 2.58+.
- **Packager recipe.** `packaging/snap/snapcraft.yaml`.
- **Install layout.** `/snap/pcloud-rs/current/`.
- **Service-manager entry.** snapd-managed service.
- **Signing.** Snap store key.
- **Known gaps.** Confinement interacts with FUSE; classic
  confinement may be required.

### 5.7 Arch (AUR)

- **OS range.** Arch rolling; Manjaro inherits.
- **Packager recipe.** `PKGBUILD` in AUR.
- **Install layout.** `/usr/bin/pcloudc`.
- **Service-manager entry.** `pcloudd.service`.
- **Signing.** Maintainer GPG.

### 5.8 Docker / OCI

- **OS range.** Any OCI runtime (Docker, podman, containerd).
- **Packager recipe.** `packaging/docker/Dockerfile` — multi-stage
  reproducible build.
- **Install layout.** `/usr/local/bin/pcloudc`, `/usr/local/bin/pcloud-daemon`.
- **Service-manager entry.** Container PID 1; compose / k8s drives
  lifecycle.
- **Signing.** cosign keyless OIDC via GitHub Actions.
- **Known gaps.** FUSE inside a container requires
  `CAP_SYS_ADMIN` + `/dev/fuse` bind; see
  [Deployment §5.3](./deployment.md#53-container-deployment-docker--systemd-nspawn).

### 5.9 macOS Homebrew tap

- **OS range.** macOS 12 Monterey+.
- **Packager recipe.** `packaging/homebrew/Formula/pcloud-rs.rb`.
- **Install layout.** `/opt/homebrew/bin/pcloudc` (Apple Silicon) or
  `/usr/local/bin/pcloudc` (Intel).
- **Service-manager entry.** `brew services start pcloud-rs` wraps
  `com.pcloud.pcloudd.plist` under launchd.
- **Signing.** Developer ID bottle **pending**.
- **Known gaps.** Bottle signing blocked on active Developer ID.

### 5.10 macOS signed `.pkg`

- **OS range.** macOS 12 Monterey+.
- **Packager recipe.** `pkgbuild` + `productsign` wrapped in
  `packaging/signing/sign-macos.sh`; `xcrun notarytool submit` in
  `notarize-macos.sh`.
- **Install layout.** `/Applications/pCloud.app`, CLI symlinks under
  `/usr/local/bin/`.
- **Service-manager entry.** launchd LaunchDaemon.
- **Signing.** Developer ID Installer **+** Apple notarisation
  **pending** (vendor-bound on Apple Developer Program membership).

### 5.11 Windows MSI (WiX)

- **OS range.** Windows 10 1809+ and Windows 11.
- **Packager recipe.** WiX v4 `packaging/windows/wix/pcloud-rs.wxs`.
- **Install layout.** `%ProgramFiles%\pCloud\`.
- **Service-manager entry.** SCM registration via MSI custom action
  — `pcloudd` service, start type `Automatic`.
- **Signing.** Authenticode EV **stub** — the WiX recipe emits an
  unsigned MSI; `packaging/signing/sign-windows.ps1` wraps
  `signtool.exe` and runs only when an EV HSM token is present.
- **Known gaps.** EV token procurement is the gate. Until then, MSI
  ships unsigned; do not distribute through winget / Chocolatey
  channels that require signed artefacts.

### 5.12 Windows winget / Chocolatey / Scoop

- **winget.** `packaging/winget/<version>.yaml`; inherits the MSI
  Authenticode signature.
- **Chocolatey.** `packaging/chocolatey/pcloud-rs.nuspec`; installs
  via the MSI.
- **Scoop.** `packaging/scoop/pcloud-rs.json`; user-scope install
  under `%USERPROFILE%\scoop\apps\`; SHA-256 verified + inherited
  MSI signature.
- **Known gaps.** All three inherit the MSI signing posture — same
  EV-token blocker.

### 5.13 FreeBSD pkg / ports

- **OS range.** FreeBSD 13.x, 14.x.
- **Packager recipe.** Port-tree `Makefile` + `pkg-plist` +
  `packaging/freebsd/pcloudd.rc`.
- **Install layout.** `/usr/local/bin/pcloudc`.
- **Service-manager entry.** rc.d `/usr/local/etc/rc.d/pcloudd`.
- **Signing.** FreeBSD ports tree signing.
- **Known gaps.** Mount parity (fusefs) not hardware-verified.

### 5.14 OpenBSD ports

- **OS range.** OpenBSD 7.4+.
- **Packager recipe.** Ports tree `Makefile` + rc.d.
- **Install layout.** `/usr/local/bin/pcloudc`.
- **Service-manager entry.** `/etc/rc.d/pcloudd`.
- **Signing.** `signify`-based ports tree.
- **Known gaps.** Mount parity not hardware-verified.

### 5.15 NetBSD pkgsrc

- **OS range.** NetBSD 10.
- **Packager recipe.** pkgsrc `Makefile` + rc.d.
- **Install layout.** `/usr/pkg/bin/pcloudc`.
- **Service-manager entry.** `/etc/rc.d/pcloudd`.
- **Signing.** pkgsrc signing.
- **Known gaps.** Mount parity not hardware-verified; `puffs` / `rump`
  FUSE bridge untested.

## 6. Signing posture summary

| Posture                  | Where applied                                        | Status                                  |
|--------------------------|------------------------------------------------------|-----------------------------------------|
| **cosign (sigstore)**    | OCI container images (`packaging/docker/`)           | Wired; keyless OIDC via GitHub Actions  |
| **GPG detached sig**     | `.deb`, `.rpm`, AppImage, AUR, tarballs              | Wired; release key rotation documented  |
| **Authenticode (EV)**    | Windows MSI + inherited by winget / Chocolatey       | **Stub** — awaits EV HSM token          |
| **Apple Developer ID**   | macOS `.pkg`, Homebrew bottles                       | **Stub** — awaits active Developer ID   |
| **Apple notarisation**   | macOS `.pkg` (stapled)                               | **Stub** — requires Developer ID first  |
| **Hash-verified (only)** | Nix flake, Scoop bucket                              | Live                                    |

Signing scripts live under
[`packaging/signing/`](../../../../../packaging/signing/):

- `sign-macos.sh` — `codesign` + `productsign` wrapper.
- `notarize-macos.sh` — `xcrun notarytool submit` + `stapler`.
- `sign-windows.ps1` — `signtool.exe` Authenticode wrapper (EV token).

## 7. Service-manager integration

| OS       | Supervisor | Unit / manifest                                              | Lives under                               |
|----------|------------|--------------------------------------------------------------|-------------------------------------------|
| Linux    | systemd    | `pcloudd.service` (system + `--user`)                        | ships inside `.deb` / `.rpm` / Nix module |
| macOS    | launchd    | `com.pcloud.pcloudd.plist`, `com.pcloud.pcloud-rs.plist`     | `packaging/macos/`                        |
| Windows  | SCM        | registered by MSI custom action                              | `packaging/windows/wix/pcloud-rs.wxs`     |
| FreeBSD  | rc.d       | `pcloudd`                                                    | `packaging/freebsd/pcloudd.rc`            |
| OpenBSD  | rc.d       | `pcloudd`                                                    | `packaging/openbsd/pcloudd`               |
| NetBSD   | rc.d       | `pcloudd`                                                    | `packaging/netbsd/pcloudd`                |

## 8. Verification

For any given channel, after install the following must all succeed:

```bash
pcloudc version --json | jq '.daemon'          # pinned version
pcloudc doctor --json | jq '.checks[] | select(.level=="error")'  # empty
# Service manager reports the daemon as active:
systemctl --user is-active pcloud-rs-daemon     # Linux
launchctl list | grep com.pcloud.pcloudd       # macOS
sc query pcloudd                                # Windows
rcctl check pcloudd                             # OpenBSD
service pcloudd status                          # FreeBSD / NetBSD
```

Supply-chain verification (before install):

```bash
sha256sum -c SHA256SUMS.txt
# Depending on channel:
cosign verify-blob --key release.pub \
  --signature pcloud-daemon.sig pcloud-daemon             # Linux / OCI
codesign --verify --verbose /Applications/pCloud.app      # macOS
signtool verify /pa C:\pcloud-rs.msi                      # Windows
```

## 9. Rollback

Rollback is channel-specific. The canonical recipe:

- **`.deb` / `.rpm`**: keep the previous version in your artefact repo
  and `apt install pcloud-daemon=<prev>` / `dnf downgrade pcloud-daemon`.
- **Nix / NixOS**: revert the flake input and `nixos-rebuild switch
  --rollback`.
- **AppImage / Flatpak / Snap**: downgrade via the channel’s native
  mechanism (`flatpak downgrade`, `snap revert`).
- **Homebrew**: `brew uninstall pcloud-rs && brew install
  pcloud-rs@<previous-version>` from the tap.
- **macOS `.pkg`**: run the previous `.pkg` installer; `productsign`
  signed.
- **Windows MSI**: `msiexec /x {GUID}` then install the previous MSI.
- **BSD rc.d**: `pkg install -f <prev>` / re-run the ports
  `make install` for the prior tag.

In every case, follow up with
[Upgrade §6 Rollback](./upgrade.md#6-rollback) for the daemon-level
state verification.

## 10. Tradeoffs / tuning

| Knob                                   | Default        | Tradeoff                                                     |
|----------------------------------------|----------------|--------------------------------------------------------------|
| Preferred channel (per-OS)             | platform-native | Native integration vs cross-OS uniformity (Flatpak / Snap / Docker). |
| OCI base image                         | `debian:stable-slim` | Minimal base vs distro-provided CA bundles.              |
| Reproducible build                     | required       | Builds are slower in CI but provenance is stronger.          |
| EV token storage                       | vendor-bound   | Hardware token or cloud HSM; both have audit trails.         |
| Release cadence                        | every minor    | Faster = more surface area per cycle; slower = batched risk. |

## 11. Common failure modes

1. **`dpkg-sig` verification fails post-install.**
   - Cause: release key rotated; old public key cached.
   - Fix: `apt-key add` (or, modern) import the new key under
     `/etc/apt/keyrings/`, `apt update`, re-verify.
2. **macOS `"app is damaged"` dialog.**
   - Cause: unsigned or not-notarised `.pkg`.
   - Fix: this is the current state of the pipeline; wait for the
     vendor unblocker or install via Homebrew tap (also pending).
     Do not `xattr -d com.apple.quarantine` on production machines.
3. **Windows SmartScreen blocks MSI.**
   - Cause: MSI unsigned (Authenticode EV stub not yet live).
   - Fix: temporarily accept, or delay deployment until the
     signing pipeline is complete.
4. **Flatpak FUSE mount refused.**
   - Cause: missing finish-arg permission.
   - Fix: add `--device=fuse` + `--talk-name=org.freedesktop.Flatpak`
     to the Flatpak manifest; re-build the package.
5. **snapd confinement blocks mount.**
   - Cause: strict confinement.
   - Fix: request classic confinement on the Snap store, or document
     that the Snap channel ships without the mount feature.

## 12. Security / compliance notes

- **Signing posture is operator-visible**: every artefact that is
  stubbed or pending ships with a `UNSIGNED-PREVIEW` tag in its
  filename until credentials are available. Do **not** rename these
  to strip the tag.
- **`cargo audit` / `cargo deny`** run on every release commit; any
  unresolved RUSTSEC advisory fails the supply-chain gate.
- **Release-key rotation** is documented in
  [`docs/ops/release.md`](../../../ops/release.md).
- **First tagged release** has not been cut. Every channel is
  CI-exercised (reproducible build + unsigned verification) but not
  yet publicly distributed.
- **Binary transparency** (Sigstore Rekor) publishes entries for
  every signed artefact; operators can prove retrospectively which
  binary their seats ran.

## 12b. CI gates (2026-04-16)

The `.github/workflows/packaging.yml` workflow runs on `release: published`
(and manual `workflow_dispatch`) and builds every supported target in its
own isolated job:

| Job | Target | Signing |
|-----|--------|---------|
| `linux-deb-rpm`   | `.deb` + `.rpm` (fpm)                    | cosign keyless blob sig (`.sig` + `.pem`) |
| `linux-appimage`  | portable `.AppImage`                     | cosign keyless blob sig |
| `linux-flatpak`   | `.flatpak` bundle                        | cosign keyless blob sig |
| `macos-pkg`       | universal tarball + optional Dev-ID `.pkg` | Dev-ID scaffold (`continue-on-error`) + cosign keyless blob sig |
| `windows-msi`     | WiX `.msi` (unsigned by default)         | EV signtool scaffold (`continue-on-error`) + cosign keyless blob sig |
| `docker-image`    | `linux/amd64` + `linux/arm64` OCI image  | cosign keyless OCI signature (no sidecar) |
| `publish-manifest`| aggregate `release-artifacts.txt`        | n/a — attaches to release |

Design rules enforced by the workflow:

- **No maintainer private keys in CI.** Blob and OCI signing use sigstore
  keyless via GitHub OIDC (`id-token: write`) and `cosign sign-blob --yes` /
  `cosign sign --yes`.
- **Apple Dev-ID** and **Windows EV** paths are intentionally
  `continue-on-error: true` until real certificates land. Their absence
  never blocks a release; the fallback is an unsigned artefact plus the
  cosign keyless attestation.
- **Every artifact gets a companion `.sig` + `.pem`.** Verification recipe
  is embedded at the top of `release-artifacts.txt` and uses
  `--certificate-identity-regexp` against this repository's
  `packaging.yml` workflow identity.
- **Supply-chain gates on every PR.** `rust.yml` runs `cargo deny check`,
  `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"`, and
  a nightly `cargo audit` against the live RustSec feed (a separate
  `rustsec-watchdog` job files an issue on any new hit).

## 13. Residual honest gaps

1. **Mount runtime live-proof.** `bd-1du.4` still covers mounted-drive
   parity. macOS fuse-t, WinFSP, and *BSD `fusefs` mounts have not
   been hardware-verified; packaging recipes do **not** imply a
   functional mount on those targets yet.
2. **Notarisation / EV signing.** Both vendor-bound. The gate is
   credential procurement, not code. Once credentials exist the
   scripts in `packaging/signing/` are expected to complete the
   pipelines.
3. **First tagged release.** Until a tag is cut, every channel above
   is exercised in CI (reproducible build + unsigned artefact
   verification) but not yet publicly distributed.

## 14. Cross-references

- [Reference → Packaging](../reference/packaging.md) — per-channel
  build steps, CI invocation, artefact layout.
- [`packaging/README.md`](../../../../../packaging/README.md) —
  in-tree recipes and signing scripts.
- [Deployment](./deployment.md) — fleet rollout that consumes these
  artefacts.
- [Upgrade](./upgrade.md) — per-host upgrade semantics per service
  manager.
- [Runbook](./runbook.md) — live playbooks post-install.
- [Platforms](./platforms/) — per-OS install walkthroughs (owned by
  other documentation agents).
- [`PLAN_CROSSPLATFORM.md`](../../../../PLAN_CROSSPLATFORM.md)
  — tier definitions.
