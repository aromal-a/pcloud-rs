param(
    [Parameter(Mandatory = $true)]
    [string]$Workspace,

    [Parameter(Mandatory = $true)]
    [string]$Toolchain
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Invoke-Gate {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [scriptblock]$Body
    )

    Write-Host "[windows-ci] >>> $Name"
    & $Body
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE"
    }
    Write-Host "[windows-ci] <<< $Name"
}

function Assert-NativeSuccess {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Description
    )

    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE"
    }
}

Set-Location $Workspace

Invoke-Gate "install Rust $Toolchain" {
    rustup toolchain install $Toolchain --profile minimal --component rustfmt,clippy
}
Invoke-Gate "rustfmt" {
    cargo "+$Toolchain" fmt --all -- --check
}
Invoke-Gate "workspace check" {
    cargo "+$Toolchain" check --workspace --all-targets --locked
}
Invoke-Gate "workspace clippy" {
    cargo "+$Toolchain" clippy --workspace --all-targets --locked -- -D warnings
}
Invoke-Gate "workspace tests without kernel mount crate" {
    cargo "+$Toolchain" test --workspace --exclude pcloud-fs --locked
}
Invoke-Gate "pcloud-fs portable tests" {
    cargo "+$Toolchain" test -p pcloud-fs --lib --locked
    Assert-NativeSuccess "pcloud-fs library tests"
    cargo "+$Toolchain" test -p pcloud-fs --test fuse_adapter_unit --locked
    Assert-NativeSuccess "pcloud-fs fuse adapter tests"
    cargo "+$Toolchain" test -p pcloud-fs --test inode_unit --locked
    Assert-NativeSuccess "pcloud-fs inode tests"
    cargo "+$Toolchain" test -p pcloud-fs --test write_path_unit --locked
    Assert-NativeSuccess "pcloud-fs write path tests"
}
Invoke-Gate "build daemon and CLI" {
    cargo "+$Toolchain" build -p pcloud-daemon -p pcloud-cli --locked
}

$root = Join-Path $env:TEMP ("pcloud-local-ci-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Force $root | Out-Null
$stdout = Join-Path $root "pcloudd.stdout.log"
$stderr = Join-Path $root "pcloudd.stderr.log"
$exitCodePath = Join-Path $root "pcloudd.exitcode"
$launcher = Join-Path $root "run-pcloudd.cmd"
$daemonExe = Join-Path $Workspace "target\debug\pcloudd.exe"
$env:PCLOUD_ROOT = $root
$env:PCLOUD_ENV = "test"
$launcherLines = @(
    "@echo off",
    "`"$daemonExe`" serve 1>`"$stdout`" 2>`"$stderr`"",
    "set `"PCLOUD_DAEMON_EXIT=%ERRORLEVEL%`"",
    ">`"$exitCodePath`" echo %PCLOUD_DAEMON_EXIT%",
    "exit /b %PCLOUD_DAEMON_EXIT%"
)
Set-Content -LiteralPath $launcher -Value $launcherLines -Encoding Ascii
$daemon = Start-Process `
    -FilePath $env:ComSpec `
    -ArgumentList "/d /c `"$launcher`"" `
    -PassThru `
    -WindowStyle Hidden

try {
    $ready = $false
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        if ($daemon.HasExited) {
            $out = Get-Content $stdout -Raw -ErrorAction SilentlyContinue
            $err = Get-Content $stderr -Raw -ErrorAction SilentlyContinue
            throw "pcloudd exited before readiness (code=$($daemon.ExitCode))`n$out`n$err"
        }
        & (Join-Path $Workspace "target\debug\pcloudc.exe") status *> $null
        if ($LASTEXITCODE -eq 0) {
            $ready = $true
            break
        }
        Start-Sleep -Milliseconds 100
    }
    if (-not $ready) {
        throw "pcloudd named pipe did not become ready within 10 seconds"
    }

    & (Join-Path $Workspace "target\debug\pcloudc.exe") shutdown *> $null
    if ($LASTEXITCODE -ne 0) {
        throw "pcloudc shutdown failed with exit code $LASTEXITCODE"
    }
    if (-not $daemon.WaitForExit(15000)) {
        throw "pcloudd did not exit within 15 seconds after shutdown"
    }
    $daemon.WaitForExit()
    if (-not (Test-Path -LiteralPath $exitCodePath)) {
        throw "pcloudd launcher did not record an exit code"
    }
    $daemonExitText = (Get-Content -LiteralPath $exitCodePath -Raw).Trim()
    $daemonExitCode = 0
    if (-not [int]::TryParse($daemonExitText, [ref]$daemonExitCode)) {
        throw "pcloudd launcher recorded an invalid exit code: $daemonExitText"
    }
    if ($daemonExitCode -ne 0) {
        $out = Get-Content $stdout -Raw -ErrorAction SilentlyContinue
        $err = Get-Content $stderr -Raw -ErrorAction SilentlyContinue
        throw "pcloudd exited with code $daemonExitCode`n$out`n$err"
    }
}
finally {
    if (-not $daemon.HasExited) {
        Stop-Process -Id $daemon.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
}

$winFsp = Get-ItemProperty `
    "HKLM:\SOFTWARE\WOW6432Node\WinFsp" `
    -ErrorAction SilentlyContinue
if ($null -ne $winFsp) {
    $env:PCLOUD_WINFSP_TEST = "1"
    Invoke-Gate "WinFSP live mount" {
        cargo "+$Toolchain" test -p pcloud-fs --test winfsp_mount_live --locked -- --ignored
    }
}
else {
    Write-Warning "WinFSP is not installed; kernel mount qualification was not run"
}

Write-Host "[windows-ci] native Windows pipeline passed"
