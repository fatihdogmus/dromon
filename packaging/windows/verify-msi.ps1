<#
.SYNOPSIS
  Installs a Dromon MSI the way a user would, checks it, and uninstalls it.

.DESCRIPTION
  packaging/windows/verify-msi.ps1 -Msi <file> -Version <version> [-Signed] [-Publisher <CN>]

  msiexec /i /qn (per machine, silent), then: C:\Program Files\Dromon\dromon.exe exists and
  "dromon.exe --version" prints "dromon <version>", the Start-menu shortcut and the Add/Remove
  Programs entry exist; with -Signed, the MSI and the installed dromon.exe have valid,
  timestamped Authenticode signatures (by -Publisher when given). Then msiexec /x removes it and
  the program folder must be gone. Needs an elevated shell (GitHub's Windows runners are).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Msi,
    [Parameter(Mandatory)] [string] $Version,
    [switch] $Signed,
    [string] $Publisher = ''
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$MsiPath = (Resolve-Path -LiteralPath $Msi).Path
$Name = Split-Path -Leaf $MsiPath
$Exe = Join-Path $env:ProgramFiles 'Dromon\dromon.exe'
$Shortcut = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\Dromon.lnk'
$Logs = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$VerifySignatures = Join-Path $PSScriptRoot '..\..\.github\release\verify-windows-signatures.ps1'

function Write-Summary([string] $Line) {
    Write-Output $Line
    if ($env:GITHUB_STEP_SUMMARY) { Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value $Line }
}

function Invoke-Msiexec([string] $Action, [string] $Log) {
    $Process = Start-Process -FilePath msiexec.exe -ArgumentList @($Action, "`"$MsiPath`"", '/qn', '/norestart', '/l*v', "`"$Log`"") -Wait -PassThru
    if ($Process.ExitCode -ne 0) {
        Get-Content -LiteralPath $Log -Tail 60 -ErrorAction SilentlyContinue
        throw "msiexec $Action $Name exited with $($Process.ExitCode)."
    }
}

if ($Signed) { & $VerifySignatures -Files $MsiPath -Publisher $Publisher }
if (Test-Path -LiteralPath $Exe) { throw "$Exe exists before the install." }

Invoke-Msiexec '/i' (Join-Path $Logs 'dromon-install.log')
if (-not (Test-Path -LiteralPath $Exe)) { throw "The install left no $Exe." }
if (-not (Test-Path -LiteralPath $Shortcut)) { throw "The install left no Start-menu shortcut ($Shortcut)." }
$Entry = Get-ItemProperty 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
    Where-Object { $_.PSObject.Properties['DisplayName'] -and $_.DisplayName -eq 'Dromon' }
if (@($Entry).Count -ne 1) { throw "Expected one Add/Remove Programs entry for Dromon, found $(@($Entry).Count)." }
if ($Signed) { & $VerifySignatures -Files $Exe -Publisher $Publisher }

# A GUI-subsystem program: PowerShell waits for it and captures stdout only through a pipeline.
$Reported = (& $Exe --version | Out-String).Trim()
if ($Reported -ne "dromon $Version") { throw "dromon.exe --version printed '$Reported', not 'dromon $Version'." }
Write-Summary "- ${Name}: installs silently per machine, Start-menu shortcut and Add/Remove Programs entry (version $($Entry.DisplayVersion)), ``dromon.exe --version`` = $Reported$(if ($Signed) { ', MSI and exe signed' } else { ', unsigned' })"

Invoke-Msiexec '/x' (Join-Path $Logs 'dromon-uninstall.log')
if (Test-Path -LiteralPath $Exe) { throw "The uninstall left $Exe." }
Write-Summary "- ${Name}: uninstalls cleanly"
