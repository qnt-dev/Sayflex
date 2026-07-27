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

if (-not $output.StartsWith(
    $rustRoot + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to clean an output path outside the Rust workspace: $output"
}

New-Item -ItemType Directory -Force -Path $output | Out-Null
Get-ChildItem -LiteralPath $output -File -ErrorAction SilentlyContinue |
    Remove-Item -Force

Push-Location $rustRoot
try {
    cargo build --target-dir $cpuTarget --profile $profile --workspace
    if ($LASTEXITCODE -ne 0) {
        throw 'The Rust workspace build failed.'
    }
    Copy-Item -LiteralPath (
        Join-Path $cpuTarget "$profile\rust-speech-worker.exe"
    ) -Destination (
        Join-Path $output 'rust-speech-worker-cpu.exe'
    )
    Copy-Item -LiteralPath (
        Join-Path $cpuTarget "$profile\rust-desktop.exe"
    ) -Destination $output
    Copy-Item -LiteralPath (
        Join-Path $cpuTarget "$profile\rust-input-host.exe"
    ) -Destination $output
    Copy-Item -LiteralPath (
        Join-Path $cpuTarget "$profile\rust-translation-worker.exe"
    ) -Destination $output
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

    cargo build --target-dir $cudaTarget --profile $profile -p rust-speech-worker --features cuda
    if ($LASTEXITCODE -ne 0) {
        throw 'The CUDA speech worker build failed.'
    }
    Copy-Item -LiteralPath (
        Join-Path $cudaTarget "$profile\rust-speech-worker.exe"
    ) -Destination (
        Join-Path $output 'rust-speech-worker-cuda.exe'
    )
}
finally {
    Pop-Location
}

Write-Host "Rust application published to $output"
