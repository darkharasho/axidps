# Signs files with Azure Artifact Signing and verifies the result.
# Needs AZURE_TENANT_ID, AZURE_CLIENT_ID and AZURE_CLIENT_SECRET in the environment.
param(
  [Parameter(Mandatory = $true)][string[]]$Files
)
$ErrorActionPreference = 'Stop'

foreach ($v in 'AZURE_TENANT_ID', 'AZURE_CLIENT_ID', 'AZURE_CLIENT_SECRET') {
  if (-not [Environment]::GetEnvironmentVariable($v)) { throw "$v is not set" }
}

Install-Module TrustedSigning -Force -Scope CurrentUser

Invoke-TrustedSigning `
  -Endpoint https://eus.codesigning.azure.net/ `
  -CodeSigningAccountName darkharasho-signing `
  -CertificateProfileName public-trust `
  -FileDigest SHA256 `
  -TimestampRfc3161 http://timestamp.acs.microsoft.com `
  -TimestampDigest SHA256 `
  -Files ($Files -join ',')

$bad = $false
foreach ($f in $Files) {
  $sig = Get-AuthenticodeSignature -FilePath $f
  Write-Host "$f : $($sig.Status)"
  if ($sig.Status -ne 'Valid') { $bad = $true }
}
if ($bad) { throw 'One or more files are not validly signed' }
