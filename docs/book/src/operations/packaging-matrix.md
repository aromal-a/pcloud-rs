# Packaging matrix

This page records what the repository can build and what must still be proven
before an artifact is advertised. A recipe or workflow definition is not, by
itself, evidence that a published package passed.

For build details see [Reference → Packaging](../reference/packaging.md). The
in-tree asset index is [`packaging/README.md`](../../../../packaging/README.md).

## Release workflows

| Workflow/job | Output | Enforcement | Publication posture |
|---|---|---|---|
| `release.yml` | Linux x86-64 binaries, checksums, CycloneDX/SPDX SBOMs | workspace release gate and cosign blob signing | would attach to a qualifying versioned GitHub release; none exists |
| `release-packaging.yml` / `build-packages` | `.deb`, `.rpm`, `SHA256SUMS` | full package gate; GPG detached signatures when release-key secrets exist | would attach after qualification; unsigned dry runs are visibly possible when secrets are absent |
| `release-packaging.yml` / `linux-live-mount` | no artifact | labelled native FUSE create/write/read/journal/unmount test | release-blocking prerequisite for every installer/candidate job |
| `release-packaging.yml` / `windows-installer` | signed MSI and signed WinFSP WiX Burn bootstrapper | signing secrets required; binaries and installers signed; pinned vendor WinFSP checksum/signature verified | would attach only after the strict job passes; none published |
| `release-packaging.yml` / `macos-installer` | signed, notarized, stapled macOS `.pkg` | Apple credentials required; native fuse-t mount tests, signature checks, Gatekeeper assessment | would attach only after the strict job passes; none published |
| `release-packaging.yml` / `nas-package-candidates` | Synology `.spk`, QNAP `.qpkg`, ASUSTOR `.apk`, per-architecture checksums | static musl payload check, official package tools where required, extraction/metadata validation | Actions artifacts only; deliberately not a public release asset |
| `ci.yml` / DragonFly, OmniOS, Solaris native jobs | native deterministic `.tar.gz` plus SHA-256 | release binary build, rc.d/SMF validation, reproducible archive layout and internal manifest | retained CI candidates; not attached public release assets |

These are intended tag-job behaviors; no public release has exercised them.
All jobs validate that the checkout matches the requested tag. The package
gate runs formatting, locked workspace checks/tests, strict clippy, rustdoc,
mdBook, NAS metadata checks, `cargo-audit`, and `cargo-deny` before downstream
installer jobs start.

## Channel status

| Tier | Platform/channel | In-tree path | Current repository state | External qualification still required |
|---|---|---|---|---|
| T1 | Linux raw binaries | `release.yml` | release job with checksums, SBOMs, cosign signatures | successful release-commit run |
| T1 | Debian/RPM | daemon Cargo metadata + `release-packaging.yml` | packages and checksums; optional GPG signatures | package install/upgrade smoke tests; require signing secrets for public policy |
| T1 | macOS `.pkg` | `packaging/macos/`, `packaging/signing/` | strict build/sign/notarize/staple workflow and safe LaunchAgent helper | labelled fuse-t runner and real Apple credentials must pass |
| T1 | Windows MSI/Burn | `packaging/windows/wix/`, `packaging/signing/` | strict signed MSI + pinned WinFSP bootstrapper workflow | real signing credential and successful Windows release run |
| T1 | FreeBSD/OpenBSD/NetBSD | `packaging/{freebsd,openbsd,netbsd}/` | rc.d assets and native runtime/mount CI | downstream ports/pkgsrc recipes and installation tests |
| T1 | DragonFly BSD | `packaging/dragonfly/`, `packaging/unix/` | supervised rc.d asset plus native runtime/mount and deterministic candidate workflow | successful release run, downstream port, installation/upgrade tests |
| T1 | illumos/Solaris | `packaging/solarish/`, `packaging/unix/` | SMF service plus native API/CLI and deterministic candidate workflows; mount explicitly unsupported | successful release runs, IPS publication, installation/upgrade tests |
| T2 | Synology DSM | `packaging/nas/synology/` | DSM 7 SPK candidate builder | vendor hardware matrix |
| T2 | QNAP QTS/QuTS hero | `packaging/nas/qnap/` | official-QDK QPKG candidate builder | vendor hardware matrix |
| T2 | ASUSTOR ADM | `packaging/nas/asustor/` | official-APKG `.apk` candidate builder with checked 90×90 icon | vendor hardware matrix |
| Experimental | AppImage/Flatpak/Snap/Homebrew/winget/Chocolatey/Scoop | matching `packaging/` directories | local or downstream-oriented recipes | real versions/hashes, channel submission, and install testing |
| Experimental | OCI/Docker | `packaging/docker/` | local image and compose recipe | registry publishing, signing, and runtime qualification |

## Signing and provenance

| Artifact | Current mechanism | Release rule |
|---|---|---|
| Linux raw files/SBOMs | keyless cosign blob signatures | verify identity, signature, checksum, and SBOM before promotion |
| `.deb`, `.rpm`, `SHA256SUMS` | GPG detached signatures when configured | public policy must reject a run that omitted expected signatures |
| macOS `.pkg` | Developer ID Application + Installer signatures, notarization, stapling, Gatekeeper assessment | credentials are mandatory for the public installer job |
| Windows binaries/MSI/Burn bundle | Authenticode with RFC 3161 timestamp | credentials are mandatory; the final Burn engine and bundle are both signed |
| NAS candidates | SHA-256 checksums | candidates are not public supported releases until hardware qualification |
| OCI image | none | do not claim a published or signed OCI channel |

## Linux service layout

The `.deb`/`.rpm` system unit installs at
`/lib/systemd/system/pcloudd.service` and executes `/usr/bin/pcloudd serve`.
Per-user deployments use `packaging/systemd/pcloudd-user.service`; the system
unit contains directives that are invalid in a user manager. The included
socket unit is not a supported socket-activation path because the daemon owns
its IPC listener.

## NAS policy

NAS packages never auto-mount and never elevate the daemon merely to gain
`/dev/fuse` access. Each vendor package must pass install, upgrade, start/stop,
reboot, uninstall/reinstall, live upload/download/copy/share tests, and any
model-specific FUSE test before it becomes a supported Tier 2 release.

## Residual gaps

- Workflow definitions need successful release-commit runs and retained logs.
- Docker has no publish/sign/provenance workflow.
- BSD and Solaris-family candidates are not yet published through downstream
  ports/pkgsrc/IPS repositories and lack retained native install/upgrade tests.
- Community channel manifests still contain future-version placeholders.
- NAS appliance qualification cannot be replaced by archive inspection on a
  Linux host.
