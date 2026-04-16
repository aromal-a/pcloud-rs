# PLATFORM: Windows only.
#
# Sign a Windows binary (EXE / MSI / DLL) with signtool using a PFX file.
#
# Usage:
#   pwsh -File sign-windows.ps1 `
#     -BinaryPath .\build\pcloud-rs.exe `
#     -PfxPath   .\cert.pfx `
#     -PfxPassword (ConvertTo-SecureString "secret" -AsPlainText -Force)
#
# For EV cloud HSM signing (DigiCert KeyLocker, SSL.com eSigner, Azure Key
# Vault) do NOT use this script: the PFX flow does not apply. Use the
# provider's dedicated CSP / KSP wrapper instead.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$BinaryPath,
    [Parameter(Mandatory = $true)][string]$PfxPath,
    [Parameter(Mandatory = $true)][SecureString]$PfxPassword,
    [string]$TimestampUrl = "http://timestamp.digicert.com"
)

$ErrorActionPreference = "Stop"

if (-not $IsWindows -and $PSVersionTable.PSVersion.Major -ge 6) {
    Write-Error "sign-windows.ps1 must run on Windows"
    exit 1
}

if (-not (Test-Path -LiteralPath $BinaryPath)) {
    Write-Error "Binary not found: $BinaryPath"
    exit 66
}
if (-not (Test-Path -LiteralPath $PfxPath)) {
    Write-Error "PFX not found: $PfxPath"
    exit 66
}

# Locate signtool. Prefer the one on PATH; fall back to the Windows SDK.
$signtool = (Get-Command signtool.exe -ErrorAction SilentlyContinue)?.Source
if (-not $signtool) {
    $candidates = Get-ChildItem -ErrorAction SilentlyContinue `
        "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe"
    if ($candidates) {
        $signtool = ($candidates | Sort-Object FullName -Descending | Select-Object -First 1).FullName
    }
}
if (-not $signtool) {
    Write-Error "signtool.exe not found on PATH or in Windows Kits"
    exit 69
}

# Convert SecureString -> plaintext only for the duration of the signtool
# invocation. We rely on PowerShell 7's built-in -AsPlainText.
$plainPassword = ConvertFrom-SecureString -SecureString $PfxPassword -AsPlainText

Write-Host "[sign-windows] signing $BinaryPath"

& $signtool sign `
    /v `
    /fd sha256 `
    /td sha256 `
    /tr $TimestampUrl `
    /f  $PfxPath `
    /p  $plainPassword `
    /a  /as `
    $BinaryPath

if ($LASTEXITCODE -ne 0) {
    Write-Error "signtool sign failed with exit code $LASTEXITCODE"
    exit $LASTEXITCODE
}

# Scrub password variable ASAP.
$plainPassword = $null
[System.GC]::Collect()

Write-Host "[sign-windows] verifying signature"
& $signtool verify /pa /v $BinaryPath
if ($LASTEXITCODE -ne 0) {
    Write-Error "signtool verify failed with exit code $LASTEXITCODE"
    exit $LASTEXITCODE
}

Write-Host "[sign-windows] done"
