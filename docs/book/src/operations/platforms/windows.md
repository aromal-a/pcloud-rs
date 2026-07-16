# Windows

Windows 10/11 x64 is a Tier 1 target for the library, CLI, per-user daemon,
DPAPI vault, named-pipe IPC, and WinFSP mounted drive. A public installer is
supported only after the strict Windows release job passes for that commit.

## Identity model

`pcloudd` runs as the interactive user. This is a security and correctness
requirement: the named-pipe DACL, client TokenUser SID check, DPAPI ciphertext,
and WinFSP mount are all user-scoped.

The public MSI deliberately does **not** register a machine-wide SCM service.
A virtual service account or LocalSystem would create a different named pipe
and DPAPI scope, leaving the installed user's CLI unable to authenticate.
Enterprise service/broker operation requires a separate multi-user design and
is not a supported package mode.

## Release gate

The `windows-installer` job in `release-packaging.yml`:

1. validates the tag and workspace version;
2. requires Authenticode PFX credentials;
3. builds and signs `pcloudc.exe` and `pcloudd.exe`;
4. downloads the pinned official WinFSP 2.1 MSI and verifies both its SHA-256
   and vendor signature;
5. builds and signs the pcloud-rs MSI;
6. builds the WiX Burn bootstrapper using the required detached-engine signing
   sequence and signs the final bundle;
7. verifies signatures and publishes checksums.

Hosted Windows CI separately runs the named-pipe tests and a live WinFSP
mount/read/write/unmount test. Workflow definitions are not passing evidence;
retain the successful logs for each release.

## Install

Prefer the signed `-setup.exe` bundle, which installs verified WinFSP before
pcloud-rs. The standalone MSI refuses a fresh install when WinFSP is absent.

```powershell
Get-AuthenticodeSignature .\pcloud-rs-<version>-x64-setup.exe |
  Format-List Status,SignerCertificate
Get-FileHash .\pcloud-rs-<version>-x64-setup.exe -Algorithm SHA256
Start-Process .\pcloud-rs-<version>-x64-setup.exe -Verb RunAs -Wait
```

Binaries install under `C:\Program Files\pcloud-rs`. Start the per-user daemon
from that user's session:

```powershell
pcloudc start
pcloudc status
```

`pcloudc start` creates a no-console child in a new process group, redirects
output to the user's pcloud-rs data directory, and waits for a real
authenticated health response from the named pipe.

## Paths and secrets

`PcloudDirs` uses Windows Known Folders:

- config: `%APPDATA%\pcloud\pcloud-rs\config`;
- state: `%APPDATA%\pcloud\pcloud-rs\data`;
- cache: `%LOCALAPPDATA%\pcloud\pcloud-rs\cache`;
- runtime diagnostics: `<cache>\pcloud-rs-runtime`.

The actual IPC endpoint is an owner-specific NT named pipe derived from the
current TokenUser SID; the runtime path is retained only for portable
diagnostics. `VaultBackend::Auto` encrypts the token with user-scope DPAPI and
stores only ciphertext on disk.

## IPC

The pipe DACL grants access only to the owner SID. On accept, the daemon calls
`GetNamedPipeClientProcessId`, opens the client process token, extracts
`TokenUser`, and requires exact SID equality before reading a request. There
is no AF_UNIX fallback.

## Mounted drive

The adapter loads the installed WinFSP runtime through direct FFI. The
canonical `RemoteFs` provides remote metadata and transfer semantics; the
WinFSP layer only translates Windows filesystem callbacks.

```powershell
New-Item -ItemType Directory -Force "$env:USERPROFILE\pCloud" | Out-Null
pcloudc mount "$env:USERPROFILE\pCloud"
pcloudc mount status
pcloudc unmount "$env:USERPROFILE\pCloud"
```

RAII teardown owns ordinary unmount. A process-wide active-mount registry and
console-control reaper arbitrate stop/delete against normal drop so WinFSP
handles are destroyed at most once.

## Known qualification limits

- The public release job currently builds x64. ARM64 must not be advertised
  until it has its own native build, IPC, WinFSP, and installer gates.
- Drive-letter and directory mounts are enumerated cross-process through the
  private `pcloud-rs` filesystem marker and Win32 volume-path APIs. Forced-
  crash cleanup remains a native release qualification case because WinFSP
  normally tears down a volume when its user-mode process exits.
- winget, Chocolatey, and Scoop manifests require real release URLs and hashes
  and do not inherit qualification merely because the MSI exists.
