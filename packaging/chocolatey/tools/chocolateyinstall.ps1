# PLATFORM: Windows 10/11 (x64)
# STATUS: scaffolding; release URLs and SHA256s must be filled at release time.
#
# Purpose:
#   Chocolatey install hook. Downloads the signed pcloud-rs MSI from GitHub
#   Releases, verifies its SHA256 against the pinned checksum, and invokes
#   msiexec silently via Chocolatey's Install-ChocolateyPackage helper.
#
# Invocation:
#   `choco install pcloud-rs` -> Chocolatey auto-runs this script as part of
#   package install. Not intended to be run directly.
#
# Inputs:
#   $url64      - signed MSI URL; must be updated per release.
#   $checksum64 - SHA256 of the MSI; must be updated per release.
#
# Outputs:
#   Program Files layout populated by the MSI; Windows service `pcloudd`
#   registered (see packaging/windows/wix/pcloud-rs.wxs).
#
# Security:
#   - The `SHA256_PLACEHOLDER` string MUST be replaced with the real
#     checksum before this manifest is published; Chocolatey will reject
#     mismatched checksums, but shipping the placeholder itself is a
#     supply-chain hazard (typosquat risk if someone builds against it).
#   - /quiet prevents UAC prompts but the MSI still installs per-machine.
#   - Exit 3010 means "success, reboot required"; 1641 means "reboot
#     initiated". Both are success-adjacent.
#
# Side effects:
#   - Writes to `C:\Program Files\pcloud-rs\`.
#   - Installs the WinFSP dependency (declared in pcloud-rs.nuspec).
#   - Creates the `pcloudd` Windows service.
#
# Test:
#   choco pack .\packaging\chocolatey\pcloud-rs.nuspec
#   choco install pcloud-rs --source=.\packaging\chocolatey -fdv --whatif
#   # (drop --whatif for a real install in a disposable VM)

$ErrorActionPreference = 'Stop'

$packageName = 'pcloud-rs'
$url64       = 'https://github.com/pcloudcom/pcloud-rs/releases/download/vX.Y.Z/pcloud-rs-X.Y.Z-x64.msi'
$checksum64  = 'SHA256_PLACEHOLDER'

$packageArgs = @{
  packageName    = $packageName
  fileType       = 'msi'
  url64bit       = $url64
  checksum64     = $checksum64
  checksumType64 = 'sha256'
  silentArgs     = '/quiet /norestart'
  validExitCodes = @(0, 3010, 1641)
}

Install-ChocolateyPackage @packageArgs
