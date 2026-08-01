[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release'
)

$ErrorActionPreference = 'Stop'
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'dist\app'))
$profile = $Configuration.ToLowerInvariant()
$cpuTarget = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'target\publish-cpu'))
$cudaTarget = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'target\publish-cuda'))
$petRoot = [System.IO.Path]::GetFullPath((Join-Path $rustRoot '..\desktop-pet'))
$petTarget = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'target\publish-pet'))
$petAssetSource = Join-Path $petRoot 'assets'
$petAssetOutput = Join-Path $output 'pet-assets'
$petAssetManifestName = 'runtime-assets.json'
$petAssetManifestPath = Join-Path $petAssetSource $petAssetManifestName
if (-not (Test-Path -LiteralPath $petAssetManifestPath -PathType Leaf)) {
    throw "Desktop pet runtime asset manifest is missing: $petAssetManifestPath"
}
$petAssetManifest = Get-Content -LiteralPath $petAssetManifestPath -Raw | ConvertFrom-Json
if ($petAssetManifest.version -ne 1 -or -not $petAssetManifest.files) {
    throw 'Desktop pet runtime asset manifest is invalid or unsupported.'
}
$petAssets = @($petAssetManifest.files | ForEach-Object { [string]$_ })
if (($petAssets | Select-Object -Unique).Count -ne $petAssets.Count) {
    throw 'Desktop pet runtime asset manifest contains duplicate paths.'
}
foreach ($asset in $petAssets) {
    if ([string]::IsNullOrWhiteSpace($asset) -or
        $asset -match '[\\/]' -or
        $asset -in @('.', '..') -or
        [System.IO.Path]::GetExtension($asset) -notin @('.json', '.png')) {
        throw "Desktop pet runtime asset manifest contains an unsafe path: $asset"
    }
}

if (-not $output.StartsWith(
    $rustRoot + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to clean an output path outside the Rust workspace: $output"
}

New-Item -ItemType Directory -Force -Path $output | Out-Null
Get-ChildItem -LiteralPath $output -File -ErrorAction SilentlyContinue |
    Remove-Item -Force
if (Test-Path -LiteralPath $petAssetOutput) {
    $resolvedPetAssets = [System.IO.Path]::GetFullPath($petAssetOutput)
    if (-not $resolvedPetAssets.StartsWith(
        $output + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to clean a pet asset path outside the publish directory: $resolvedPetAssets"
    }
    Remove-Item -LiteralPath $resolvedPetAssets -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $petAssetOutput | Out-Null

Push-Location $rustRoot
try {
    cargo build --target-dir $cpuTarget --profile $profile --workspace
    if ($LASTEXITCODE -ne 0) {
        throw 'The Rust workspace build failed.'
    }
    Copy-Item -LiteralPath (
        Join-Path $cpuTarget "$profile\sayflex-speech-worker.exe"
    ) -Destination (
        Join-Path $output 'SayFlex.Speech.Cpu.exe'
    )
    Copy-Item -LiteralPath (
        Join-Path $cpuTarget "$profile\sayflex-desktop.exe"
    ) -Destination (
        Join-Path $output 'SayFlex.exe'
    )
    Copy-Item -LiteralPath (
        Join-Path $cpuTarget "$profile\sayflex-input-host.exe"
    ) -Destination (
        Join-Path $output 'SayFlex.InputHost.exe'
    )
    Copy-Item -LiteralPath (
        Join-Path $cpuTarget "$profile\sayflex-translation-worker.exe"
    ) -Destination (
        Join-Path $output 'SayFlex.Translation.exe'
    )
    Copy-Item -LiteralPath (
        Join-Path $rustRoot '..\LICENSE'
    ) -Destination (
        Join-Path $output 'LICENSE.txt'
    )
    Copy-Item -LiteralPath (
        Join-Path $rustRoot 'THIRD_PARTY_NOTICES.md'
    ) -Destination (
        Join-Path $output 'THIRD_PARTY_NOTICES.md'
    )
    Copy-Item -LiteralPath (
        Join-Path $rustRoot 'SECURITY.md'
    ) -Destination (
        Join-Path $output 'SECURITY.md'
    )

    cargo build --manifest-path (Join-Path $petRoot 'Cargo.toml') `
        --target-dir $petTarget --profile $profile -p pet_host
    if ($LASTEXITCODE -ne 0) {
        throw 'The desktop pet build failed.'
    }
    Copy-Item -LiteralPath (
        Join-Path $petTarget "$profile\pet_host.exe"
    ) -Destination (
        Join-Path $output 'SayFlex.Pet.exe'
    )
    Copy-Item -LiteralPath $petAssetManifestPath -Destination (
        Join-Path $petAssetOutput $petAssetManifestName
    )
    foreach ($asset in $petAssets) {
        $source = Join-Path $petAssetSource $asset
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
            throw "Required desktop pet asset is missing: $source"
        }
        Copy-Item -LiteralPath $source -Destination (Join-Path $petAssetOutput $asset)
    }

    cargo build --target-dir $cudaTarget --profile $profile -p sayflex-speech-worker --features cuda
    if ($LASTEXITCODE -ne 0) {
        throw 'The CUDA speech worker build failed.'
    }
    Copy-Item -LiteralPath (
        Join-Path $cudaTarget "$profile\sayflex-speech-worker.exe"
    ) -Destination (
        Join-Path $output 'SayFlex.Speech.Cuda.exe'
    )
}
finally {
    Pop-Location
}

Write-Host "SayFlex published to $output"
