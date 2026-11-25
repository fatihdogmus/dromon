<#
.SYNOPSIS
  Authenticode-signs files in place with SSL.com eSigner (cloud HSM) through CodeSignTool.

.DESCRIPTION
  .github/release/sign-windows-sslcom.ps1 -Files <file>...

  Downloads CodeSignTool for Windows (it bundles its Java runtime) from SSL.com's official
  GitHub releases, refuses it unless its SHA-256 is the one pinned here, and signs each file
  (SHA-256 digest, RFC 3161 timestamp from SSL.com's TSA, http://ts.ssl.com, which the tool's
  conf/code_sign_tool.properties sets). The file is replaced by its signed copy (-override).

  To move to a newer CodeSignTool: download it, check it, and update both values below.

  Environment (secrets): ESIGNER_USERNAME, ESIGNER_PASSWORD, ESIGNER_TOTP_SECRET,
  ESIGNER_CREDENTIAL_ID. GitHub masks their values in logs; the tool's batch file has echo off.
#>
[CmdletBinding()]
param([Parameter(Mandatory)] [string[]] $Files)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ToolUrl = 'https://github.com/SSLcom/CodeSignTool/releases/download/v1.3.2/CodeSignTool-v1.3.2-windows.zip'
$ToolSha256 = '4afc32e8b7f79bbe1de7e4e7049aaad4e0f754357613b9bbec0e3052f06fd36b'

foreach ($name in 'ESIGNER_USERNAME', 'ESIGNER_PASSWORD', 'ESIGNER_TOTP_SECRET', 'ESIGNER_CREDENTIAL_ID') {
    if (-not [Environment]::GetEnvironmentVariable($name)) { throw "$name is not set." }
}

$Work = Join-Path $env:RUNNER_TEMP "codesigntool-$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $Work | Out-Null
try {
    $Zip = Join-Path $Work 'CodeSignTool.zip'
    Invoke-WebRequest -Uri $ToolUrl -OutFile $Zip -UseBasicParsing
    $Actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Zip).Hash
    if ($Actual -ne $ToolSha256) { throw "CodeSignTool's SHA-256 is $Actual, not the pinned $ToolSha256." }
    Expand-Archive -LiteralPath $Zip -DestinationPath (Join-Path $Work 'tool')
    $Tool = Join-Path $Work 'tool'
    if (-not (Test-Path -LiteralPath (Join-Path $Tool 'CodeSignTool.bat'))) { throw 'The CodeSignTool archive has no CodeSignTool.bat.' }

    # The batch file runs its bundled Java with relative paths: run it from its folder.
    Push-Location $Tool
    try {
        foreach ($File in $Files) {
            $Path = (Resolve-Path -LiteralPath $File).Path
            $Log = & .\CodeSignTool.bat sign `
                "-username=$env:ESIGNER_USERNAME" `
                "-password=$env:ESIGNER_PASSWORD" `
                "-credential_id=$env:ESIGNER_CREDENTIAL_ID" `
                "-totp_secret=$env:ESIGNER_TOTP_SECRET" `
                "-input_file_path=$Path" `
                -override 2>&1 | Out-String
            # CodeSignTool exits 0 on some failures; its output says "Error" then.
            if ($LASTEXITCODE -ne 0 -or $Log -match '(?im)^\s*Error') {
                Write-Output $Log
                throw "CodeSignTool could not sign $Path."
            }
            Write-Output "signed $Path"
        }
    }
    finally { Pop-Location }
}
finally {
    Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue
}
