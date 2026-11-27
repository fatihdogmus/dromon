<#
.SYNOPSIS
  Builds Dromon-<version>-windows-<arch>.msi around a dromon.exe with WiX Toolset v5.

.DESCRIPTION
  packaging/windows/build-msi.ps1 -Exe <dromon.exe> -Arch x64|arm64 -Out <folder> [-RepoUrl <url>]

  Compiles packaging/windows/dromon.wxs. The release workflow passes the exe the signing job
  signed (or build-exe.ps1's unsigned one when there are no Windows credentials), so the MSI
  installs exactly that file. Needs WiX v5 as a .NET tool:
    dotnet tool install --global wix --version 5.*
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [ValidateSet('x64', 'arm64')] [string] $Arch,
    [Parameter(Mandatory)] [string] $Out,
    [string] $RepoUrl = 'https://github.com/fatihdogmus/dromon'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$section = ''
$Version = $null
foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
    if ($line -match '^\[') { $section = $line.Trim() }
    elseif ($section -eq '[workspace.package]' -and $line -match '^version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
}
if (-not $Version) { throw 'No [workspace.package] version in Cargo.toml.' }
# MSI versions are numeric (major.minor.build); an rc installs as its final version.
$MsiVersion = ($Version -split '-')[0]

$Wix = Get-Command wix -ErrorAction SilentlyContinue
if (-not $Wix) { throw 'WiX is not installed: dotnet tool install --global wix --version 5.*' }
$WixVersion = (& wix --version | Out-String).Trim()
if ($WixVersion -notmatch '^5\.') { throw "WiX $WixVersion is installed; this script needs WiX v5." }

$ExePath = (Resolve-Path -LiteralPath $Exe).Path
$ExeVersion = (Get-Item -LiteralPath $ExePath).VersionInfo.ProductVersion
if ($ExeVersion -ne $Version) { throw "$ExePath is version '$ExeVersion', not $Version (build it with build-exe.ps1)." }

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$OutDir = (Resolve-Path $Out).Path
$Msi = Join-Path $OutDir "Dromon-$Version-windows-$Arch.msi"
# wix writes a .wixpdb beside its output: build in a scratch folder, keep only the MSI.
$Scratch = Join-Path ([IO.Path]::GetTempPath()) "dromon-msi-$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $Scratch | Out-Null
try {
    & wix build (Join-Path $PSScriptRoot 'dromon.wxs') `
        -arch $Arch `
        -d "Version=$MsiVersion" `
        -d "DisplayVersion=$Version" `
        -d "Exe=$ExePath" `
        -d "Icon=$(Join-Path $Root 'assets\brand\windows\dromon.ico')" `
        -d "License=$(Join-Path $Root 'LICENSE')" `
        -d "InterLicense=$(Join-Path $Root 'crates\dromon-app\assets\fonts\Inter-LICENSE.txt')" `
        -d "GeistMonoLicense=$(Join-Path $Root 'crates\dromon-app\assets\fonts\GeistMono-OFL.txt')" `
        -d "RepoUrl=$RepoUrl" `
        -o (Join-Path $Scratch 'dromon.msi')
    if ($LASTEXITCODE -ne 0) { throw 'wix build failed.' }
    Move-Item -LiteralPath (Join-Path $Scratch 'dromon.msi') -Destination $Msi -Force
}
finally {
    Remove-Item -LiteralPath $Scratch -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Output "$Msi (WiX $WixVersion, MSI version $MsiVersion)"
