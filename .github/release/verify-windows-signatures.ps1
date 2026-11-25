<#
.SYNOPSIS
  Checks Authenticode signatures: valid, SHA-256, and RFC 3161 timestamped.

.DESCRIPTION
  .github/release/verify-windows-signatures.ps1 -Files <file>... [-Publisher <CN>]

  Each file must be validly signed (Get-AuthenticodeSignature Status Valid, so the chain ends in
  a root Windows trusts), with a SHA-256 signer certificate digest and a timestamp (short-lived
  cloud-signing certificates stay valid only through it). With -Publisher, the signer's simple
  name must match. Writes the signer of each file to the job summary.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string[]] $Files,
    [string] $Publisher = ''
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

foreach ($File in $Files) {
    $Path = (Resolve-Path -LiteralPath $File).Path
    $Signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($Signature.Status -ne 'Valid') {
        throw "$Path is not validly signed: $($Signature.Status) $($Signature.StatusMessage)"
    }
    if (-not $Signature.TimeStamperCertificate) { throw "$Path has no RFC 3161 timestamp." }
    if ($Signature.SignerCertificate.SignatureAlgorithm.FriendlyName -notmatch 'sha256|sha384|sha512') {
        throw "$Path's signer certificate uses $($Signature.SignerCertificate.SignatureAlgorithm.FriendlyName)."
    }
    $Name = $Signature.SignerCertificate.GetNameInfo([Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
    if ($Publisher -and $Name -ne $Publisher) { throw "$Path is signed by '$Name', not '$Publisher'." }
    $Line = "- $(Split-Path -Leaf $Path): signed by $Name, timestamped by $($Signature.TimeStamperCertificate.GetNameInfo([Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false))"
    Write-Output $Line
    if ($env:GITHUB_STEP_SUMMARY) { Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value $Line }
}
