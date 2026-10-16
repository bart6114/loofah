param([Parameter(Mandatory = $true)][string]$FilePath)

$ErrorActionPreference = 'Stop'
foreach ($name in @('AZURE_SIGNING_ENDPOINT', 'AZURE_SIGNING_ACCOUNT', 'AZURE_SIGNING_PROFILE')) {
    if (-not [Environment]::GetEnvironmentVariable($name)) { throw "Missing $name" }
}
if (-not (Test-Path -LiteralPath $FilePath -PathType Leaf)) { throw 'The signing input must be a file' }

Import-Module ArtifactSigning -RequiredVersion 0.1.20
Invoke-ArtifactSigning `
    -Endpoint $env:AZURE_SIGNING_ENDPOINT `
    -CodeSigningAccountName $env:AZURE_SIGNING_ACCOUNT `
    -CertificateProfileName $env:AZURE_SIGNING_PROFILE `
    -Files $FilePath `
    -FileDigest SHA256 `
    -TimestampRfc3161 'http://timestamp.acs.microsoft.com' `
    -TimestampDigest SHA256 `
    -ExcludeEnvironmentCredential $true `
    -ExcludeWorkloadIdentityCredential $true `
    -ExcludeManagedIdentityCredential $true `
    -ExcludeSharedTokenCacheCredential $true `
    -ExcludeVisualStudioCredential $true `
    -ExcludeVisualStudioCodeCredential $true `
    -ExcludeAzureCliCredential $false `
    -ExcludeAzurePowerShellCredential $true `
    -ExcludeAzureDeveloperCliCredential $true `
    -ExcludeInteractiveBrowserCredential $true
