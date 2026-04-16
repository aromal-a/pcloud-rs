<!-- PLATFORM: Windows 10/11 -->
<!-- TODO: set SIGNING_CERT_PATH and SIGNING_CERT_PASSWORD in your CI secret store. -->

# pcloud-rs WiX MSI

This directory contains the WiX (v3 compatible, v4 convertible) scaffolding for
building a Windows MSI installer for `pcloud-rs`.

## Prerequisites

### Build host

- Windows 10 or 11 build host (or an equivalent CI runner — `windows-latest`
  is used in `.github/workflows/release.yml`).
- The [WiX Toolset v3](https://wixtoolset.org/) installed and on `PATH`
  (`candle.exe`, `light.exe`). v4 is convertible but not yet wired.
- Rust toolchain (`rustup default stable`, target `x86_64-pc-windows-msvc`).
- Windows SDK / `signtool.exe` on `PATH` (bundled with `windows-latest` under
  `C:\Program Files (x86)\Windows Kits\10\bin\*\x64\`).

### Target machine runtime: WinFSP

`pcloud-fs` uses the **user-space WinFSP** FUSE-compatible driver (the
same component used by rclone, cbfs, sshfs-win, etc.). It must be present on
the end-user machine before `pcloudc.exe` can mount a drive.

The MSI bundles the **WinFSP 2.x installer** and invokes it as a deferred
`<CustomAction>` during `InstallExecuteSequence`:

```xml
<!-- excerpt from pcloud-rs.wxs -->
<Binary Id="WinFspInstaller" SourceFile="$(var.StageDir)\vendor\winfsp-2.0.msi" />

<CustomAction Id="InstallWinFsp"
              BinaryKey="WinFspInstaller"
              ExeCommand="/qn /norestart"
              Execute="deferred"
              Impersonate="no"
              Return="check" />

<InstallExecuteSequence>
  <Custom Action="InstallWinFsp" Before="InstallFinalize">
    NOT Installed AND NOT REMOVE
  </Custom>
</InstallExecuteSequence>
```

The bundled MSI is fetched at build time from the official release feed:

```
https://winfsp.dev/rel/
```

Pin a specific version (`winfsp-2.0.23075.msi` at time of writing) in the
release workflow; never pull `latest`. Verify the published SHA256 against
`winfsp.dev/rel/` before staging.

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

Install the helper:

```powershell
cargo install cargo-wix
```

Then from the repository root:

```powershell
cargo wix --install-version X.Y.Z
```

This produces `target\wix\pcloud-rs-X.Y.Z-x86_64.msi`.

## EV Code Signing (stub)

After the MSI is built and before publishing:

```powershell
signtool sign /v /fd sha256 ^
  /tr http://timestamp.digicert.com ^
  /td sha256 ^
  /f "%SIGNING_CERT_PATH%" ^
  /p "%SIGNING_CERT_PASSWORD%" ^
  pcloud-rs.msi
```

<!-- TODO: replace the /f path with a hardware token (EV cert) reference,
     e.g. `/sha1 <thumbprint> /csp "eToken Base Cryptographic Provider"`. -->

## Files

- `pcloud-rs.wxs` — WiX source.
- `License.rtf` — license shown by the installer UI (MIT OR Apache-2.0).
- `README.md` — this file.

## Uninstall

`Settings > Apps > pcloud-rs > Uninstall`, or:

```powershell
msiexec /x pcloud-rs.msi /qn
```
