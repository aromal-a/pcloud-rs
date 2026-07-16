# PLATFORM: Windows 10/11 (x64)
# STATUS: scaffolding
#
# Purpose:
#   Chocolatey uninstall hook. Invokes msiexec /x on the previously-installed
#   MSI via Uninstall-ChocolateyPackage.
#
# Invocation:
#   `choco uninstall pcloud-rs` -> Chocolatey auto-runs this script.
#
# Inputs:
#   None (path resolved via Get-AppInstallLocation).
#
# Outputs:
#   Removes `C:\Program Files\pcloud-rs\`.
#   Does NOT remove user state (%APPDATA%\pcloud-rs) by design; the daemon's
#   auth vault and audit log must be manually purged if desired.
#
# Security:
#   - Exit codes 1605 and 1614 indicate "already uninstalled"; we accept them
#     as success to make the hook idempotent.
#   - /quiet avoids UAC re-prompt on re-uninstall.
#
# Side effects:
#   - Leaves any running per-user daemon for the user to stop before uninstall.
#   - Leaves user state intact (see above).
#
# Test:
#   choco uninstall pcloud-rs -fdv

$ErrorActionPreference = 'Stop'

$packageName = 'pcloud-rs'

$packageArgs = @{
  packageName    = $packageName
  fileType       = 'msi'
  silentArgs     = '/quiet /norestart'
  validExitCodes = @(0, 3010, 1605, 1614, 1641)
  file           = "$(Get-AppInstallLocation $packageName)\pcloud-rs.msi"
}

Uninstall-ChocolateyPackage @packageArgs
