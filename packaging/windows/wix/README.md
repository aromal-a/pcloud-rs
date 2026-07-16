<!-- PLATFORM: Windows 10/11 -->
<!-- TODO: set SIGNING_CERT_PATH and SIGNING_CERT_PASSWORD in your CI secret store. -->

# pcloud-rs WiX MSI

This directory contains the WiX v3 sources for two artifacts:

- `pcloud-rs-X.Y.Z-x64.msi`: the standalone application MSI. It requires an
  existing WinFSP installation and checks the official WinFSP registry key.
- `pcloud-rs-X.Y.Z-x64-setup.exe`: the public Burn bundle. It chains the
  official checksum-pinned WinFSP MSI before the pcloud-rs MSI.

## Prerequisites

### Build host

- Windows 10 or 11 build host. `cargo xtask windows` uses the configured
  native Windows SSH host for compile/test qualification.
- The [WiX Toolset v3](https://wixtoolset.org/) installed and on `PATH`
  (`candle.exe`, `light.exe`). v4 is convertible but not yet wired.
- Rust toolchain (`rustup default stable`, target `x86_64-pc-windows-msvc`).
- Windows SDK / `signtool.exe` on `PATH` (normally under
  `C:\Program Files (x86)\Windows Kits\10\bin\*\x64\`).

### Target machine runtime: WinFSP

`pcloud-fs` uses WinFSP for mounted drives. The public setup executable embeds
the official `winfsp-2.1.25156.msi` and verifies its published SHA-256 before
building:

```text
073A70E00F77423E34BED98B86E600DEF93393BA5822204FAC57A29324DB9F7A
```

The standalone pcloud-rs MSI does not run a nested installer custom action.
It searches the documented 32-bit `HKLM\SOFTWARE\WinFsp` registry view and
fails with a remediation message if WinFSP is absent. The Burn bundle is the
normal end-user artifact because Burn is designed to chain MSI packages.

### Driver signing: user-space vs kernel-space

- **User-space WinFSP** (current path, what we ship) uses a filter / minifilter
  that is **already signed by the WinFSP project** with a Microsoft-attested
  EV cert. The end user does **not** need `pnputil /add-driver`. The MSI
  installs a standard user-mode DLL plus the pre-signed kernel filter; no
  reboot required on Windows 10 1809+ / Windows 11.
- **Kernel-space fallback (future)** — if we ever ship a bespoke
  filesystem minifilter, we would need an **EV kernel-mode code-signing
  cert** plus a **Microsoft Hardware attestation** through the Partner
  Center dashboard (HLK tests + portal submission). Installation would then
  use:

  ```powershell
  pnputil /add-driver pcloudfs.inf /install
  ```

  run from a deferred, elevated `CustomAction` with `Execute="deferred"` and
  `Impersonate="no"`. This path is **not** enabled today; documented here so
  a future kernel-mode variant has a clear onboarding checklist.

## Build

From the repository root, after building and staging `pcloudc.exe` and
`pcloudd.exe` under `dist\stage`:

```powershell
$version = "0.1.0"
$wix = "packaging\windows\wix"

candle.exe -nologo -wx `
  -dStageDir="dist\stage" `
  -dProductVersion=$version `
  -out "dist\pcloud-rs.wixobj" `
  "$wix\pcloud-rs.wxs"
light.exe -nologo -wx -ext WixUIExtension `
  -b "$wix" `
  -out "dist\pcloud-rs-$version-x64.msi" `
  "dist\pcloud-rs.wixobj"

candle.exe -nologo -wx -ext WixBalExtension `
  -dProductVersion=$version `
  -dWinFspMsi="dist\vendor\winfsp-2.1.25156.msi" `
  -dPcloudMsi="dist\pcloud-rs-$version-x64.msi" `
  -out "dist\pcloud-rs-bundle.wixobj" `
  "$wix\pcloud-rs-bundle.wxs"
light.exe -nologo -wx -ext WixBalExtension `
  -out "dist\pcloud-rs-$version-x64-setup.exe" `
  "dist\pcloud-rs-bundle.wixobj"
```

## Authenticode signing

Release CI signs both Rust executables and the pcloud-rs MSI. Burn bundles
must be signed in two pieces: detach and sign the engine, reattach it, then
sign the complete bundle.

```powershell
insignia.exe -ib pcloud-rs-0.1.0-x64-setup.exe -o engine.exe
signtool.exe sign /fd sha256 /td sha256 /tr http://timestamp.digicert.com engine.exe
insignia.exe -ab engine.exe pcloud-rs-0.1.0-x64-setup.exe -o signed-setup.exe
signtool.exe sign /fd sha256 /td sha256 /tr http://timestamp.digicert.com signed-setup.exe
signtool.exe verify /pa /v signed-setup.exe
```

`packaging/signing/sign-windows.ps1` implements the PFX signing and verification
step used by CI. Public tag builds fail if the signing credentials are absent.

## Files

- `pcloud-rs.wxs` — WiX source.
- `pcloud-rs-bundle.wxs` — WinFSP + pcloud-rs Burn chain.
- `License.rtf` — license shown by the installer UI (MIT OR Apache-2.0).
- `README.md` — this file.

## Daemon identity

The MSI installs no SCM service. `pcloudc start` launches a no-console daemon
under the interactive user's SID. This is required because named-pipe peer
authentication, DPAPI, and WinFSP mounts are user-scoped; a machine service
account would create an endpoint and vault the installed user cannot access.

## Uninstall

`Settings > Apps > pcloud-rs > Uninstall`, or:

```powershell
msiexec /x pcloud-rs.msi /qn
```
