<#
.SYNOPSIS
  Builds dromon.exe for Windows x64 or arm64 and stamps its version resource and icon.

.DESCRIPTION
  packaging/windows/build-exe.ps1 -Arch x64|arm64 -Out <folder>

  cargo builds the release binary for the target (arm64 cross-builds on an x64 machine), then
  rcedit (electron/rcedit, a pinned release checked against its SHA-256) writes the version
  resource Explorer shows and SignPath checks (ProductName "Dromon", ProductVersion the release
  version) and the icon. The app's Cargo build has no resource step of its own yet; when it
  gets one (a build.rs), drop rcedit here. Writes <Out>\dromon.exe, and checks that an x64 build
  prints "dromon <version>" for --version.

  Needs Rust (rust-toolchain.toml) with the MSVC toolchain and, for arm64, Visual Studio's
  ARM64 build tools (GitHub's windows-2025 image has both).
#>
[CmdletBinding()]
param(
    [ValidateSet('x64', 'arm64')] [string] $Arch = 'x64',
    [Parameter(Mandatory)] [string] $Out
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$RceditUrl = 'https://github.com/electron/rcedit/releases/download/v2.0.0/rcedit-x64.exe'
$RceditSha256 = '3e7801db1a5edbec91b49a24a094aad776cb4515488ea5a4ca2289c400eade2a'

$section = ''
$Version = $null
foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
    if ($line -match '^\[') { $section = $line.Trim() }
    elseif ($section -eq '[workspace.package]' -and $line -match '^version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
}
if (-not $Version) { throw 'No [workspace.package] version in Cargo.toml.' }
$Numeric = ($Version -split '-')[0]

$Target = @{ x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }[$Arch]
& rustup target add $Target
if ($LASTEXITCODE -ne 0) { throw "rustup target add $Target failed." }
& cargo build --release --locked -p dromon-app --target $Target --manifest-path (Join-Path $Root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed.' }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
$Built = Join-Path $TargetDir "$Target\release\dromon.exe"

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Exe = Join-Path (Resolve-Path $Out).Path 'dromon.exe'
Copy-Item -LiteralPath $Built -Destination $Exe -Force

$Rcedit = Join-Path ([IO.Path]::GetTempPath()) "rcedit-$RceditSha256.exe"
if (-not (Test-Path -LiteralPath $Rcedit) -or (Get-FileHash -Algorithm SHA256 -LiteralPath $Rcedit).Hash -ne $RceditSha256) {
    Invoke-WebRequest -Uri $RceditUrl -OutFile $Rcedit -UseBasicParsing
}
$Actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Rcedit).Hash
if ($Actual -ne $RceditSha256) { throw "rcedit's SHA-256 is $Actual, not $RceditSha256." }

& $Rcedit $Exe `
    --set-icon (Join-Path $Root 'assets\brand\windows\dromon.ico') `
    --set-file-version "$Numeric.0" `
    --set-product-version "$Numeric.0" `
    --set-version-string ProductName 'Dromon' `
    --set-version-string ProductVersion $Version `
    --set-version-string FileVersion $Version `
    --set-version-string FileDescription 'Dromon' `
    --set-version-string CompanyName 'Dromon' `
    --set-version-string LegalCopyright 'Copyright (c) 2026 Muhammed Fatih Dogmus. MIT License.' `
    --set-version-string OriginalFilename 'dromon.exe' `
    --set-version-string InternalName 'dromon'
if ($LASTEXITCODE -ne 0) { throw 'rcedit failed.' }

$Info = (Get-Item -LiteralPath $Exe).VersionInfo
if ($Info.ProductName -ne 'Dromon' -or $Info.ProductVersion -ne $Version) {
    throw "dromon.exe's version resource reads '$($Info.ProductName)' '$($Info.ProductVersion)'."
}
# A GUI-subsystem program: PowerShell waits for it and captures stdout only through a pipeline.
if ($Arch -eq 'x64' -and $env:PROCESSOR_ARCHITECTURE -eq 'AMD64') {
    $Reported = (& $Exe --version | Out-String).Trim()
    if ($Reported -ne "dromon $Version") { throw "dromon.exe --version printed '$Reported', not 'dromon $Version'." }
}
Write-Output "$Exe ($Arch, $Version)"
