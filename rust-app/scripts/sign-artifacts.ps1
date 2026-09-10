[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string[]]$Path,
    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9A-Fa-f]{40}$')]
    [string]$CertificateThumbprint,
    [string]$TimestampUrl = 'http://timestamp.digicert.com'
)

$ErrorActionPreference = 'Stop'
$thumbprint = $CertificateThumbprint.ToUpperInvariant()
$certificate = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
    Where-Object Thumbprint -eq $thumbprint |
    Select-Object -First 1
if (-not $certificate) {
    throw "No current-user code-signing certificate matches thumbprint $thumbprint."
}

$signTool = Get-Command signtool.exe -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty Source
if (-not $signTool) {
    $signTool = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' `
        -Filter signtool.exe -File -Recurse -ErrorAction SilentlyContinue |
        Where-Object FullName -Match '\\x64\\signtool\.exe$' |
        Sort-Object FullName -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $signTool) {
    throw 'signtool.exe was not found.'
}

foreach ($item in $Path) {
    $resolved = [System.IO.Path]::GetFullPath($item)
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        throw "Cannot sign a missing artifact: $resolved"
    }
    # Only the certificate thumbprint reaches the process command line. The PFX
    # password is used once by CI while importing the certificate into the
    # current-user store and is never accepted or printed by this script.
    & $signTool sign /sha1 $thumbprint /fd SHA256 /tr $TimestampUrl /td SHA256 $resolved
    if ($LASTEXITCODE -ne 0) {
        throw "signtool failed for $resolved."
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $resolved
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Authenticode verification failed for ${resolved}: $($signature.Status)."
    }
}
