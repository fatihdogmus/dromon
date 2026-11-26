# Dromon installer for Windows (x64; on Arm PCs the x64 build runs emulated).
#
#   irm https://raw.githubusercontent.com/fatihdogmus/dromon/main/install.ps1 | iex
#
# Finds the latest GitHub release, downloads Dromon-<version>-windows-x64.msi, checks its SHA-256
# against the release's SHA256SUMS and installs it silently (msiexec /qn) for all users. The MSI
# is per-machine, so a session that is not elevated gets one UAC prompt.
#
# Options, as environment variables (they work through a pipe):
#
#   $env:DROMON_VERSION      install this release instead of the latest ("0.1.0" or "v0.1.0")
#   $env:DROMON_FORCE=1      install even if that version is already installed
#   $env:DROMON_DRY_RUN=1    do everything up to and including the download and the checksum check,
#                            then print the install command instead of running it (also skips the
#                            Windows check)
#   $env:DROMON_RELEASE_URL  test hook: base URL that holds the release's files, used instead of
#                            github.com (needs DROMON_VERSION)
#
# Works in Windows PowerShell 5.1 and PowerShell 7. Everything runs inside one script block that
# throws on failure (never exit), so piping it to iex cannot close your shell, and a truncated
# download does not parse, so it runs nothing.

& {
    $ErrorActionPreference = 'Stop'
    $repo = 'fatihdogmus/dromon'
    $dry = ($env:DROMON_DRY_RUN -eq '1')

    function Say([string]$Message) { Write-Host "dromon-install: $Message" }
    function Fail([string]$Message) { throw "dromon-install: $Message" }

    # Windows and architecture. A dry run only downloads and checks, so it may run anywhere.
    $onWindows = ($PSVersionTable.PSEdition -eq 'Desktop') -or ($IsWindows -eq $true)
    if (-not $onWindows -and -not $dry) {
        Fail 'this installer is for Windows. On macOS and Linux use install.sh. Build from source: https://github.com/fatihdogmus/dromon#build-from-source'
    }
    if ($onWindows) {
        $arch = $env:PROCESSOR_ARCHITEW6432
        if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
        if ($arch -eq 'ARM64') {
            Say 'note: Arm64 Windows detected; the x64 build runs emulated.'
        } elseif ($arch -ne 'AMD64') {
            Fail "no Dromon build for Windows $arch (x64 only). Build from source: https://github.com/fatihdogmus/dromon#build-from-source"
        }
    }

    # Windows PowerShell 5.1 does not use TLS 1.2 by default, which GitHub requires.
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {
        Write-Verbose 'could not enable TLS 1.2'
    }

    # Version: pinned, or the latest published release (the API's "latest" skips drafts and prereleases).
    if ($env:DROMON_VERSION) {
        $version = $env:DROMON_VERSION -replace '^v', ''
    } else {
        if ($env:DROMON_RELEASE_URL) { Fail 'DROMON_RELEASE_URL needs DROMON_VERSION' }
        try {
            $release = Invoke-RestMethod -UseBasicParsing -UserAgent 'dromon-install' -Uri "https://api.github.com/repos/$repo/releases/latest"
        } catch {
            Fail "could not find the latest release (none published yet, or GitHub is unreachable): $($_.Exception.Message). Build from source: https://github.com/fatihdogmus/dromon#build-from-source"
        }
        $version = "$($release.tag_name)" -replace '^v', ''
    }
    if ($version -notmatch '^[0-9A-Za-z._+-]+$') { Fail "invalid version '$version'" }

    # Already installed? dromon.exe is a GUI program, so ask the MSI's uninstall entry.
    if ($onWindows -and $env:DROMON_FORCE -ne '1') {
        $entry = Get-ItemProperty -Path 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
            Where-Object { $_.DisplayName -eq 'Dromon' } | Select-Object -First 1
        if ($entry -and $entry.DisplayVersion -eq $version) {
            Say "Dromon $version is already installed (set `$env:DROMON_FORCE=1 to reinstall)"
            return
        }
    }

    if ($env:DROMON_RELEASE_URL) {
        $base = $env:DROMON_RELEASE_URL.TrimEnd('/')
    } else {
        $base = "https://github.com/$repo/releases/download/$version"
    }
    $name = "Dromon-$version-windows-x64.msi"

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('dromon-install-' + [guid]::NewGuid().ToString('N'))
    [void](New-Item -ItemType Directory -Path $tmp)
    $msi = Join-Path $tmp $name
    $sums = Join-Path $tmp 'SHA256SUMS'
    $log = Join-Path $tmp 'dromon-install.log'
    $failed = $true
    $oldProgress = $ProgressPreference
    try {
        Say "installing Dromon $version ($name)"

        # Progress bars make Windows PowerShell 5.1 downloads very slow.
        $ProgressPreference = 'SilentlyContinue'
        try {
            Say "downloading $name"
            Invoke-WebRequest -UseBasicParsing -Uri "$base/$name" -OutFile $msi
            Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums
        } catch {
            Fail "download failed: $($_.Exception.Message)"
        } finally {
            $ProgressPreference = $oldProgress
        }

        # Check the MSI against its line in SHA256SUMS ("<sha256>  <name>" or "<sha256> *<name>").
        $expected = $null
        foreach ($line in Get-Content -LiteralPath $sums) {
            if ($line -match '^\s*([0-9A-Fa-f]{64})\s+\*?(.+?)\s*$' -and $Matches[2] -eq $name) {
                $expected = $Matches[1]
                break
            }
        }
        if (-not $expected) { Fail "SHA256SUMS has no line for $name; not installing" }
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $msi).Hash
        if ($actual -ine $expected) {
            Fail "SHA-256 mismatch for $name (expected $expected, got $actual): the download is corrupt or was tampered with; not installing"
        }
        Say "SHA-256 verified ($($actual.ToLower()))"

        $arguments = "/i `"$msi`" /qn /norestart /log `"$log`""
        if ($dry) {
            Say "dry run: would run: msiexec.exe $arguments"
            $failed = $false
            return
        }

        $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
        $elevated = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        $start = @{ FilePath = 'msiexec.exe'; ArgumentList = $arguments; Wait = $true; PassThru = $true }
        if ($elevated) {
            Say 'installing'
        } else {
            Say 'installing (Windows will ask for permission once: the install is for all users)'
            $start.Verb = 'RunAs'
        }
        try {
            $process = Start-Process @start
        } catch {
            Fail "could not start the installer (was the permission prompt declined?): $($_.Exception.Message)"
        }
        $code = $process.ExitCode
        if ($code -ne 0 -and $code -ne 3010) {
            Fail "msiexec failed with exit code $code. Log: $log"
        }
        $failed = $false
        if ($code -eq 3010) { Say 'installed; Windows needs a restart to finish.' }

        # dromon.exe is a GUI-subsystem program: piping its output makes PowerShell wait for it,
        # and it may print nothing at all.
        $exe = Join-Path $env:ProgramFiles 'Dromon\dromon.exe'
        $printed = ''
        try { $printed = (& $exe --version 2>&1 | Out-String).Trim() } catch { $printed = '' }
        if ($printed) {
            Say "installed: $printed"
        } else {
            Say "installed: dromon $version at $exe"
        }
    } finally {
        $ProgressPreference = $oldProgress
        if ($failed -and (Test-Path -LiteralPath $log)) {
            # Keep the installer log for the error message; drop the downloads.
            Remove-Item -LiteralPath $msi, $sums -Force -ErrorAction SilentlyContinue
        } else {
            Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
