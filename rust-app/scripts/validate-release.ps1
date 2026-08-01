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
    'SayFlex.Translation.exe',
    'SayFlex.Pet.exe'
)
$petAssetManifestName = 'runtime-assets.json'
$petAssetManifestPath = Join-Path $appRoot "pet-assets\$petAssetManifestName"
if (-not (Test-Path -LiteralPath $petAssetManifestPath -PathType Leaf)) {
    throw "Release is missing the desktop pet runtime asset manifest: $petAssetManifestPath"
}
$petAssetManifest = Get-Content -LiteralPath $petAssetManifestPath -Raw | ConvertFrom-Json
if ($petAssetManifest.version -ne 1 -or -not $petAssetManifest.files) {
    throw 'Release contains an invalid desktop pet runtime asset manifest.'
}
$petAssets = @($petAssetManifest.files | ForEach-Object { [string]$_ })
if (($petAssets | Select-Object -Unique).Count -ne $petAssets.Count) {
    throw 'Release desktop pet runtime asset manifest contains duplicate paths.'
}
foreach ($asset in $petAssets) {
    if ([string]::IsNullOrWhiteSpace($asset) -or
        $asset -match '[\\/]' -or
        $asset -in @('.', '..') -or
        [System.IO.Path]::GetExtension($asset) -notin @('.json', '.png')) {
        throw "Release desktop pet runtime asset manifest contains an unsafe path: $asset"
    }
}

$missing = $required | Where-Object {
    -not (Test-Path -LiteralPath (Join-Path $appRoot $_))
}
if ($missing.Count -gt 0) {
    throw "Release is incomplete. Missing: $($missing -join ', ')"
}
$missingPetAssets = $petAssets | Where-Object {
    -not (Test-Path -LiteralPath (Join-Path $appRoot "pet-assets\$_") -PathType Leaf)
}
if ($missingPetAssets.Count -gt 0) {
    throw "Release is missing desktop pet assets: $($missingPetAssets -join ', ')"
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

$files += foreach ($name in @($petAssetManifestName) + $petAssets) {
    $relative = "pet-assets\$name"
    $path = Join-Path $appRoot $relative
    $item = Get-Item -LiteralPath $path
    $hash = Get-FileHash -LiteralPath $path -Algorithm SHA256
    [pscustomobject]@{
        File = $relative
        Bytes = $item.Length
        SHA256 = $hash.Hash.ToLowerInvariant()
    }
}

$files | Format-Table -AutoSize
Write-Host 'Release file set and SHA-256 validation passed.'
