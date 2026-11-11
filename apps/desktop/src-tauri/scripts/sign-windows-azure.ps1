param([Parameter(Mandatory = $true)][string]$FilePath)

$ErrorActionPreference = 'Stop'
foreach ($name in @('AZURE_SIGNING_ENDPOINT', 'AZURE_SIGNING_ACCOUNT', 'AZURE_SIGNING_PROFILE', 'AZURE_CLIENT_ID', 'AZURE_TENANT_ID', 'ACTIONS_ID_TOKEN_REQUEST_URL', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN', 'RUNNER_TEMP')) {
    if (-not [Environment]::GetEnvironmentVariable($name)) { throw "Missing $name" }
}
if (-not (Test-Path -LiteralPath $FilePath -PathType Leaf)) { throw 'The signing input must be a file' }

$tokenResponse = Invoke-RestMethod `
    -Uri "$($env:ACTIONS_ID_TOKEN_REQUEST_URL)&audience=api%3A%2F%2FAzureADTokenExchange" `
    -Headers @{ Authorization = "Bearer $env:ACTIONS_ID_TOKEN_REQUEST_TOKEN" }
if (-not $tokenResponse.value) { throw 'GitHub did not return an OIDC token' }

$tokenPath = Join-Path $env:RUNNER_TEMP "loofah-oidc-$([Guid]::NewGuid().ToString('N')).token"
$previousTokenPath = $env:AZURE_FEDERATED_TOKEN_FILE
try {
    [System.IO.File]::WriteAllText($tokenPath, $tokenResponse.value)
    $env:AZURE_FEDERATED_TOKEN_FILE = $tokenPath

    Import-Module ArtifactSigning -RequiredVersion 0.1.20
    Invoke-ArtifactSigning `
        -Endpoint $env:AZURE_SIGNING_ENDPOINT `
        -CodeSigningAccountName $env:AZURE_SIGNING_ACCOUNT `
        -CertificateProfileName $env:AZURE_SIGNING_PROFILE `
        -Files $FilePath `
        -FileDigest SHA256 `
        -TimestampRfc3161 'http://timestamp.acs.microsoft.com' `
        -TimestampDigest SHA256 `
        -ExcludeEnvironmentCredential `
        -ExcludeManagedIdentityCredential `
        -ExcludeSharedTokenCacheCredential `
        -ExcludeVisualStudioCredential `
        -ExcludeVisualStudioCodeCredential `
        -ExcludeAzureCliCredential `
        -ExcludeAzurePowerShellCredential `
        -ExcludeAzureDeveloperCliCredential `
        -ExcludeInteractiveBrowserCredential
} finally {
    $env:AZURE_FEDERATED_TOKEN_FILE = $previousTokenPath
    Remove-Item -LiteralPath $tokenPath -Force -ErrorAction SilentlyContinue
}
