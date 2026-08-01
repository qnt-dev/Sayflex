[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release'
)

$ErrorActionPreference = 'Stop'
$petRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = [System.IO.Path]::GetFullPath((Join-Path $petRoot 'dist\app'))
$target = [System.IO.Path]::GetFullPath((Join-Path $petRoot 'target\publish'))
$profile = $Configuration.ToLowerInvariant()
$assetSource = Join-Path $petRoot 'assets'
$assetOutput = Join-Path $output 'assets'
$manifestName = 'runtime-assets.json'
$manifestPath = Join-Path $assetSource $manifestName

if (-not $output.StartsWith(
    $petRoot + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to clean an output path outside the desktop-pet workspace: $output"
}
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Runtime asset manifest is missing: $manifestPath"
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
    if (-not (Test-Path -LiteralPath (Join-Path $assetSource $asset) -PathType Leaf)) {
        throw "Required runtime asset is missing: $asset"
    }
}

New-Item -ItemType Directory -Force -Path $output | Out-Null
Get-ChildItem -LiteralPath $output -Force -ErrorAction SilentlyContinue |
    Remove-Item -Recurse -Force
New-Item -ItemType Directory -Force -Path $assetOutput | Out-Null

Push-Location $petRoot
try {
    cargo build --target-dir $target --profile $profile -p pet_host
    if ($LASTEXITCODE -ne 0) {
        throw 'The desktop pet build failed.'
    }
}
finally {
    Pop-Location
}

Copy-Item -LiteralPath (Join-Path $target "$profile\pet_host.exe") `
    -Destination (Join-Path $output 'DesktopPet.exe')
Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $assetOutput $manifestName)
foreach ($asset in $assets) {
    Copy-Item -LiteralPath (Join-Path $assetSource $asset) `
        -Destination (Join-Path $assetOutput $asset)
}

Write-Host "Desktop Pet published independently to $output"
