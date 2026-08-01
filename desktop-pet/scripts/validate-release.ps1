[CmdletBinding()]
param(
    [string]$AppDirectory
)

$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($AppDirectory)) {
    $AppDirectory = Join-Path $PSScriptRoot '..\dist\app'
}
$appRoot = [System.IO.Path]::GetFullPath($AppDirectory)
$executable = Join-Path $appRoot 'DesktopPet.exe'
$manifestName = 'runtime-assets.json'
$manifestPath = Join-Path $appRoot "assets\$manifestName"

if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Independent release is missing DesktopPet.exe: $executable"
}
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Independent release is missing its runtime asset manifest: $manifestPath"
}

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.version -ne 1 -or -not $manifest.files) {
    throw 'Runtime asset manifest is invalid or unsupported.'
}
$assets = @($manifest.files | ForEach-Object { [string]$_ })
if (($assets | Select-Object -Unique).Count -ne $assets.Count) {
    throw 'Runtime asset manifest contains duplicate paths.'
}
foreach ($asset in $assets) {
    if ([string]::IsNullOrWhiteSpace($asset) -or
        $asset -match '[\\/]' -or
        $asset -in @('.', '..') -or
        [System.IO.Path]::GetExtension($asset) -notin @('.json', '.png')) {
        throw "Runtime asset manifest contains an unsafe path: $asset"
    }
    if (-not (Test-Path -LiteralPath (Join-Path $appRoot "assets\$asset") -PathType Leaf)) {
        throw "Independent release is missing runtime asset: $asset"
    }
}

$files = @('DesktopPet.exe', "assets\$manifestName") + @(
    $assets | ForEach-Object { "assets\$_" }
)
$results = foreach ($relative in $files) {
    $path = Join-Path $appRoot $relative
    $item = Get-Item -LiteralPath $path
    $hash = Get-FileHash -LiteralPath $path -Algorithm SHA256
    [pscustomobject]@{
        File = $relative
        Bytes = $item.Length
        SHA256 = $hash.Hash.ToLowerInvariant()
    }
}

$results | Format-Table -AutoSize
Write-Host 'Independent Desktop Pet release validation passed.'
