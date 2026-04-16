# Windows

Platform notes for running `pcloud-daemon` (as `pcloudd.exe`) and
`pcloud-cli` (as `pcloudc.exe`) on Windows 10 / 11 and Windows Server
2019 / 2022.

## Support status

- **Scaffolded, not live-tested.** WinFSP adapter compiles, the
  service wrapper exists, MSI/Chocolatey/winget/Scoop recipes ship,
  and the Authenticode signing pipeline is scripted — but no human
  has completed a live mount + sync round-trip on a Windows host
  yet. Treat the mount path as pre-alpha.
- Source of truth:
  [`architecture/platform-support.md`](../../architecture/platform-support.md).

> **Landing status (2026-04-15):** Tier 1 target, Tier 2 in practice
> until host bring-up. Phases P0–P5 are **wired, not yet live-verified**
> on a Windows host: the WinFSP adapter compiles with all 17 callbacks
> (`cleanup` included), the `pcloud-daemon-win` Service wrapper is in
> tree, and WiX MSI + Chocolatey / winget / Scoop recipes ship. Drive-
> letter lifecycle, forced-unmount recovery, and MSI Authenticode signing
> are still tracked under `bd-1du.4` / signing-pipeline bring-up. See
> [Packaging reference](../../reference/packaging.md) for the full
> channel matrix including EV Authenticode.

## OS version matrix

| Windows               | Build      | Arch           | Status                             |
|-----------------------|------------|----------------|------------------------------------|
| Windows 10 22H2       | 19045      | x86_64         | Build-only, no mount verification  |
| Windows 11 22H2/23H2  | 22621+     | x86_64, arm64  | Build-only, no mount verification  |
| Windows 11 24H2       | 26100      | x86_64, arm64  | Expected to build; untried         |
| Windows Server 2019   | 17763      | x86_64         | Build-only (Server Core supported) |
| Windows Server 2022   | 20348      | x86_64         | Build-only                         |
| Windows 8.1 / 7       | any        | any            | **Not supported**                  |
| WinFSP < 2022         | —          | —              | **Not supported** — symlink gaps   |

arm64 native binaries build with the MSVC aarch64 toolchain; running
under x86 emulation works but is not routinely tested.

## Install

### MSI (recommended for fleet deployment)

The signed MSI is the supported fleet-distribution format:

```powershell
# Verify the Authenticode signature
Get-AuthenticodeSignature .\pcloud-rs-<version>-x64.msi |
  Format-List Status, SignerCertificate

# Silent install with logging (admin)
msiexec /i pcloud-rs-<version>-x64.msi /qn /norestart /l*v install.log
```

The MSI installs binaries into `C:\Program Files\pCloudCC\`, registers
`pcloudd` as a Windows service via SCM, and installs WinFSP if
`INSTALL_WINFSP=1` is passed.

### winget

```powershell
winget install --id pCloudCC.pCloudCC --exact --scope machine
```

### Chocolatey

```powershell
choco install pcloud-rs
```

### From source

Install Rust (MSVC toolchain) plus the Windows SDK and WinFSP dev
headers. Clean release build on a Xeon workstation: **5–8 minutes.**

```powershell
# Requires the Visual Studio Build Tools + MSVC v143 toolset
# and the WinFSP 2022+ developer package.
git clone https://github.com/pcloud-rs/pcloud-rs
cd pcloud-rs\
cargo build --release -p pcloud-daemon -p pcloud-cli
```

### Authenticode signing

```powershell
# EV Authenticode signing (HSM-backed recommended)
pwsh -File packaging\signing\sign-windows.ps1 `
  -CertificateThumbprint "<EV cert thumbprint>" `
  -TimestampUrl "http://timestamp.digicert.com" `
  -Files "target\release\pcloudd.exe","target\release\pcloudc.exe"

# Sign the MSI (WiX output) the same way
pwsh -File packaging\signing\sign-windows.ps1 `
  -CertificateThumbprint "<EV cert thumbprint>" `
  -Files "target\wix\pcloud-rs-<version>-x64.msi"
```

Always stamp with a timestamp server so signatures remain valid past
the cert expiry.

### Verification

```powershell
# SHA256
Get-FileHash .\pcloudd.exe -Algorithm SHA256
# Authenticode
Get-AuthenticodeSignature .\pcloudd.exe | Format-List *
```

## Config paths (AppData)

Windows uses roaming and local AppData; the daemon splits its state so
the vault and store are **never** in a roaming profile (roaming a
vault is equivalent to emailing a credential).

| Role               | Path                                                                     | ACL              |
|--------------------|--------------------------------------------------------------------------|------------------|
| Config             | `%APPDATA%\pCloudCC\config.toml`                                         | user + SYSTEM    |
| State (store)      | `%LOCALAPPDATA%\pCloudCC\store.sqlite`                                   | user + SYSTEM    |
| Vault              | `%LOCALAPPDATA%\pCloudCC\vault.dat`                                      | user-only        |
| Journal            | `%LOCALAPPDATA%\pCloudCC\journal\`                                       | user-only        |
| Cache              | `%LOCALAPPDATA%\pCloudCC\cache\`                                         | user-only        |
| IPC endpoint       | `\\.\pipe\pCloudCC\<sid>\daemon` (named pipe, per-user SID in path)      | owner-only DACL  |
| Log               | `%LOCALAPPDATA%\pCloudCC\logs\daemon.log`                                | user-only        |

On install the MSI applies owner-only DACLs to the vault and journal
paths. The daemon re-validates the ACL on open; a vault with a relaxed
DACL is rejected (the Windows analogue of the Linux mode check).

Create the directories once if installing from source:

```powershell
New-Item -ItemType Directory -Force `
  -Path $env:APPDATA\pCloudCC, `
        $env:LOCALAPPDATA\pCloudCC, `
        $env:LOCALAPPDATA\pCloudCC\journal, `
        $env:LOCALAPPDATA\pCloudCC\cache, `
        $env:LOCALAPPDATA\pCloudCC\logs | Out-Null

# Restrict the vault parent to the current user only
icacls $env:LOCALAPPDATA\pCloudCC /inheritance:r
icacls $env:LOCALAPPDATA\pCloudCC /grant:r "$($env:USERNAME):(OI)(CI)F"
icacls $env:LOCALAPPDATA\pCloudCC /grant:r "SYSTEM:(OI)(CI)F"
```

## Service management (SCM)

The daemon registers as a per-user Windows service named `pcloudd`.
Unlike Linux/macOS, Windows service activation is centralized through
SCM.

```powershell
# Start/stop/query
sc start pcloudd
sc stop pcloudd
sc query pcloudd
sc queryex pcloudd     # includes PID for tooling

# Change startup behavior
sc config pcloudd start= auto
sc config pcloudd start= demand

# Inspect via modern tooling
Get-Service pcloudd
Get-WinEvent -LogName Application -FilterXPath `
  "*[System[Provider[@Name='pcloudd']]]" -MaxEvents 200
```

The service runs under the user's LocalSystem-constrained account
with `SeServiceLogonRight`; it does NOT run as LocalSystem. The MSI
configures the service with:

- `Restart-On-Failure` policy (3 restart attempts, 60-second reset),
- `DelayedAutoStart = 1` (reduce boot contention),
- a dedicated service SID (`NT SERVICE\pcloudd`) used in the named-pipe
  DACL,
- stdout/stderr redirected to `%LOCALAPPDATA%\pCloudCC\logs\`.

Per-user daemons on shared terminals are supported by running one
service instance per interactive session; the named-pipe path includes
the user SID so instances do not collide.

## Mount setup (WinFSP)

Windows mounts require [**WinFSP**](https://winfsp.dev) — a user-mode
filesystem driver that exposes the daemon's filesystem as a
drive-letter or directory mount.

### Install WinFSP

```powershell
# Manual
Start-Process msiexec.exe -ArgumentList `
  "/i winfsp-<version>.msi /qn" -Wait

# Or via the pcloud-rs MSI
msiexec /i pcloud-rs-<version>-x64.msi INSTALL_WINFSP=1 /qn
```

Verify:

```powershell
Get-Service WinFsp.Launcher
fsutil behavior query SymlinkEvaluation
```

### Configure the mount

```toml
[mount]
enabled = true
path    = "P:"          # drive-letter mount
policy  = "default"

# Or as a directory mount:
# path = "C:\\Users\\alice\\pCloudDrive"
```

Apply with:

```powershell
pcloudc.exe mount --path P:
pcloudc.exe mount --status
pcloudc.exe mount --unmount
```

### Wedged-mount manual cleanup

See [runbook.md Playbook 7](../runbook.md#playbook-7-kernel-mount-recovery).
The Windows path is:

```powershell
# 1. Stop the service
sc stop pcloudd

# 2. Remove the drive-letter mapping
net use P: /delete

# 3. If a stale WinFSP device remains in Device Manager, remove it
pnputil /enum-devices /class "System"
pnputil /remove-device "WinFspNet"     # admin shell

# 4. Restart
sc start pcloudd
pcloudc.exe mount --force-umount
```

Do not reboot as a first resort — the recovery commands above drain
the WinFSP state cleanly.

## Vault backend

The Windows vault backend is a file at
`%LOCALAPPDATA%\pCloudCC\vault.dat` with an owner-only DACL, validated
on every open. It is **not** encrypted under DPAPI at this time;
DPAPI-backed vault storage is a tracked future improvement. In-memory
secrets use `SecretString` / `SecretBytes` with zeroize-on-drop.

For hardware-backed root protection, use BitLocker on the system
volume — the vault file inherits BitLocker protection. The MSI does
not enable BitLocker for you; coordinate with the endpoint baseline
team.

## Upgrade

See [Upgrade](../upgrade.md). Quick path via MSI:

```powershell
pcloudc.exe --json status > C:\Temp\pre.json
sc stop pcloudd
msiexec /i pcloud-rs-<new-version>-x64.msi /qn /norestart
sc start pcloudd
pcloudc.exe doctor --json
pcloudc.exe status            # inline summary: auth, sync, crypto, engine
```

MSI upgrades in place; no uninstall+install dance required within a
major series.

## Uninstall

```powershell
# 1. Stop and remove the service
sc stop pcloudd
sc delete pcloudd

# 2. Remove the package
$app = Get-Package -Name 'pCloudCC' -ErrorAction SilentlyContinue
if ($app) { Uninstall-Package $app -Force }
# or:
winget uninstall --id pCloudCC.pCloudCC

# 3. Remove per-user state (this deletes the vault)
Remove-Item -Recurse -Force $env:APPDATA\pCloudCC
Remove-Item -Recurse -Force $env:LOCALAPPDATA\pCloudCC

# 4. Remove WinFSP if no other software needs it (optional)
# Uninstall-Package winfsp
```

Verify clean uninstall:

```powershell
Get-Service pcloudd -ErrorAction SilentlyContinue
Get-Process pcloudd -ErrorAction SilentlyContinue
Get-PSDrive -PSProvider FileSystem | Where-Object { $_.Root -like '*pCloud*' }
```

## First-run bootstrap

Beginner path:

```powershell
# 1. Install WinFSP (if not already handled by the MSI)
winget install --id WinFsp.WinFsp

# 2. Create AppData directories with proper ACLs
New-Item -ItemType Directory -Force `
  -Path $env:APPDATA\pCloudCC,$env:LOCALAPPDATA\pCloudCC,
        $env:LOCALAPPDATA\pCloudCC\journal,
        $env:LOCALAPPDATA\pCloudCC\cache,
        $env:LOCALAPPDATA\pCloudCC\logs | Out-Null

icacls $env:LOCALAPPDATA\pCloudCC /inheritance:r
icacls $env:LOCALAPPDATA\pCloudCC /grant:r "$($env:USERNAME):(OI)(CI)F"
icacls $env:LOCALAPPDATA\pCloudCC /grant:r "SYSTEM:(OI)(CI)F"

# 3. Start the service
sc start pcloudd

# 4. Sanity check
pcloudc.exe doctor --json
pcloudc.exe status
```

FAANG-ops tuning callouts:

- In Group Policy / Intune, deploy the MSI with transformations
  (`.mst`) setting `INSTALL_WINFSP=1` and pinning `DELAYED_AUTOSTART`.
- Grant `SeServiceLogonRight` to the dedicated service account via
  `secpol.msc` or `ntrights.exe`; do **not** run as LocalSystem.
- Feed structured events into Event Forwarding (`wecutil`) and
  correlate by `corr_id`.

## Service management cheat-sheet

| Action            | Command                                                                 |
|-------------------|-------------------------------------------------------------------------|
| Start             | `sc start pcloudd` or `Start-Service pcloudd`                           |
| Stop              | `sc stop pcloudd` or `Stop-Service pcloudd`                             |
| Enable auto       | `sc config pcloudd start= auto`                                         |
| Disable           | `sc config pcloudd start= disabled`                                     |
| Status            | `sc queryex pcloudd` or `Get-Service pcloudd`                           |
| Tail logs (file)  | `Get-Content -Wait $env:LOCALAPPDATA\pCloudCC\logs\daemon.log`          |
| Event log         | `Get-WinEvent -LogName Application -FilterXPath "*[System[Provider[@Name='pcloudd']]]" -MaxEvents 500` |
| Crash dumps       | `%LOCALAPPDATA%\CrashDumps\pcloudd.exe.*.dmp`                           |

Crash-dump capture (WER) once per host:

```powershell
$key = "HKLM:\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps\pcloudd.exe"
New-Item -Path $key -Force | Out-Null
Set-ItemProperty $key DumpFolder "$env:LOCALAPPDATA\CrashDumps"
Set-ItemProperty $key DumpCount 10 -Type DWord
Set-ItemProperty $key DumpType 2  -Type DWord   # full
```

## Peer-cred and IPC

- Transport: Windows **named pipe** at
  `\\.\pipe\pCloudCC\<sid>\daemon`. The `<sid>` segment is the
  user's SID so multiple interactive sessions do not collide on RDS
  / Citrix hosts.
- Security descriptor: owner-only DACL. The server assigns an ACL
  granting only the pipe owner SID `GENERIC_READ | GENERIC_WRITE`,
  denies everyone else.
- Peer identity: the daemon calls `GetNamedPipeClientProcessId()` and
  `ImpersonateNamedPipeClient()` just long enough to read the
  connecting thread token SID, then compares against its own SID.
  Non-matching peers are rejected with `peer.denied.sid_mismatch`.
- No `SO_PEERCRED` analogue is needed — pipe ACLs plus impersonation
  give us a stronger guarantee than the Unix path.

## Secret storage backend

- In-memory: `SecretString` / `SecretBytes` (zeroize-on-drop).
- On-disk: file at `%LOCALAPPDATA%\pCloudCC\vault.dat` with owner-only
  DACL, ACL re-verified on every open.
- **DPAPI is _not_ wired** yet — the vault is file + ACL protected
  only. Wrapping with DPAPI `CryptProtectData` is a tracked follow-up.
- **Credential Manager is _not_ wired** — do not advertise Windows
  credential roaming.
- BitLocker provides the hardware-backed at-rest layer; recommend it
  on every managed endpoint.

## Observability integration

- Service events go to the Windows Event Log under the `pcloudd`
  source. Query with `Get-WinEvent` (see the cheat-sheet above).
- Structured JSON logs are additionally written to
  `%LOCALAPPDATA%\pCloudCC\logs\daemon.log` (the Event Log only
  carries severities ≥ warning; JSON has the full firehose).
- ETW provider: not yet registered; tracked as a future improvement
  for fleet-wide tracing via `logman`.
- Prometheus endpoint: if enabled in `config.toml`, bind to
  `127.0.0.1` and open an inbound firewall hole only for the
  monitoring host.

## Defender, Firewall, Smart App Control, EDR

- **Windows Defender real-time scanning.** Adding the mount root to
  exclusions prevents readdir storms:
  ```powershell
  Add-MpPreference -ExclusionPath "P:\"
  Add-MpPreference -ExclusionProcess "pcloudd.exe"
  ```
  Coordinate with the EDR policy owner — exclusions may be governed
  centrally via Intune / Defender for Endpoint.
- **Smart App Control / WDAC.** Pin a rule allowing the daemon's
  Authenticode publisher or file hash. Unsigned builds will be
  blocked.
- **Windows Firewall.** The daemon makes outbound HTTPS only; no
  inbound rule is needed. If the optional Prometheus port is on,
  add a rule bound to `127.0.0.1` explicitly.
- **AppLocker.** Allow by publisher (EV cert subject) rather than
  path.

## Troubleshooting (top 10)

1. **`Access is denied` on service start** — service account missing
   `SeServiceLogonRight`. Run `secpol.msc → Local Policies → User
   Rights Assignment → Log on as a service`.
2. **WinFSP service not running** — `Get-Service WinFsp.Launcher`;
   start it. Without the launcher, `mount` fails immediately.
3. **`pcloudc` reports pipe not found** — the service isn't running,
   or you are on a different Windows session. Each session has its
   own SID-scoped pipe.
4. **`sc start pcloudd` returns 1053** — binary crashed during
   startup. Check `%LOCALAPPDATA%\pCloudCC\logs\daemon.log` and WER.
5. **Drive-letter collision** — `P:` already mapped. `net use` to
   inspect, then choose a different letter or a directory mount.
6. **Defender quarantines `pcloudd.exe`** — unsigned build. Re-sign
   with the EV cert, or add a Defender exclusion for the file hash.
7. **MSI upgrade fails with `1603`** — existing install is corrupted.
   `msiexec /x` the old install, reboot, then `msiexec /i` the new
   MSI with `/l*v install.log` for forensic logging.
8. **Long-path failures in Explorer but not in CLI** — enable:
   ```powershell
   Set-ItemProperty "HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem" `
     LongPathsEnabled -Value 1 -Type DWord
   ```
9. **TLS handshake failures against the API** — corporate TLS
   inspection. Import the inspection CA into
   `Cert:\LocalMachine\Root`.
10. **RDP session leaves a zombie mount** — run
    `pcloudc.exe mount --force-umount`; if that fails see the wedged
    mount recovery above.

## Upgrading

- In-place MSI upgrade within a minor series is safe; the MSI
  stops the service, swaps binaries, re-applies ACLs, and restarts.
- WinFSP bumps must come from the official installer; the daemon
  refuses to attach to an unknown WinFSP version.

## Uninstalling

See the **Uninstall** section below for the step-by-step removal.

## Known gaps (Windows)

- No DPAPI vault.
- No Credential Manager integration.
- No ETW provider.
- No native Universal installer (separate x86_64 and arm64 MSIs).
- No Windows Sandbox / WSL recipe — WSL1 has no kernel FUSE, WSL2
  mounts are scaffolded but untested.

## Known issues

- **WinFSP version skew.** The daemon is tested against WinFSP 2022
  and later. Older WinFSP versions may boot but lack symlink and
  reparse-point support; avoid in fleet deployments.
- **Drive-letter collisions.** If `P:` is already mapped (network
  share, BitLocker unlock drive), the mount fails with a clear error.
  Reassign or choose a directory mount instead.
- **Anti-virus real-time scanning.** Defender and third-party EDR will
  walk the pCloud drive on first mount; exclude the mount root from
  real-time scans to avoid readdir storms. Coordinate with your EDR
  policy owner — exclusions must be on allowlist.
- **Long paths.** Enable `LongPathsEnabled = 1` in
  `HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem` for users whose
  cloud tree contains paths > 260 characters. The daemon supports
  long paths internally; Explorer does not unless this policy is set.
- **DPAPI not wired.** The vault is file+ACL protected but not
  DPAPI-encrypted. Rely on BitLocker for at-rest hardware protection.
- **Named-pipe permissions.** Do not relax the named-pipe DACL.
  Remote-desktop multiplexing works without relaxation — each session
  gets its own pipe via the SID-scoped path.
- **Server Core.** Supported, but install WinFSP and `pcloud-rs` via
  MSI with `/qn`. There is no GUI wizard path on Server Core.
