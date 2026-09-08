[CmdletBinding()]
param(
    [string]$AppDirectory
)

$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($AppDirectory)) {
    $AppDirectory = Join-Path $PSScriptRoot '..\dist\app'
}
$appRoot = [System.IO.Path]::GetFullPath($AppDirectory)
$required = @(
    'SayFlex.exe',
    'SayFlex.InputHost.exe',
    'SayFlex.Speech.Cpu.exe',
    'SayFlex.Speech.Cuda.exe',
    'SayFlex.Speech.Vulkan.exe',
    'SayFlex.Translation.exe',
    'SayFlex.Companion.exe',
    'speech-backends.json',
    'runtimes\cuda\cublas64_13.dll',
    'runtimes\cuda\cublasLt64_13.dll'
)

$missing = $required | Where-Object {
    -not (Test-Path -LiteralPath (Join-Path $appRoot $_) -PathType Leaf)
}
if (@($missing).Count -gt 0) {
    throw "Release is incomplete. Missing: $($missing -join ', ')"
}

$manifest = Get-Content -LiteralPath (Join-Path $appRoot 'speech-backends.json') -Raw |
    ConvertFrom-Json
if ($manifest.schema_version -ne 1) {
    throw "Unsupported speech backend manifest schema: $($manifest.schema_version)"
}
$backendIds = @($manifest.backends | ForEach-Object id)
foreach ($backend in @('cuda', 'vulkan', 'cpu')) {
    if ($backendIds -notcontains $backend) {
        throw "Release manifest is missing the $backend backend."
    }
}
$cudaArchitectures = @(
    $manifest.backends |
        Where-Object id -eq 'cuda' |
        ForEach-Object cuda_architectures
)
foreach ($architecture in @('86-real', '89-real', '120a-real')) {
    if ($cudaArchitectures -notcontains $architecture) {
        throw "CUDA release manifest is missing architecture $architecture."
    }
}

$files = foreach ($name in $required) {
    $path = Join-Path $appRoot $name
    $item = Get-Item -LiteralPath $path
    $hash = Get-FileHash -LiteralPath $path -Algorithm SHA256
    [pscustomobject]@{
        File = $name
        Bytes = $item.Length
        SHA256 = $hash.Hash.ToLowerInvariant()
    }
}

$files | Format-Table -AutoSize
# Deliberately not called validation. Nothing here is compared against a
# reference, nothing is signed, and no binary is executed: this checks that the
# expected files are present, that the backend manifest names what it should,
# and prints each file's SHA-256 so a release can be identified afterwards.
Write-Host 'Universal release file set and backend manifest are present. Hashes above are a record, not a check against a reference.'
Write-Host 'Nothing here runs a binary: install the package and do the smoke test before shipping it.'
