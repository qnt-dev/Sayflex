[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release',
    [switch]$KeepBuildArtifacts
)

$ErrorActionPreference = 'Stop'
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'dist\app'))
$profile = $Configuration.ToLowerInvariant()
$cpuTarget = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'target\publish-cpu'))
$cudaTarget = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'target\publish-cuda'))
# whisper.cpp builds its Vulkan shader generator as a deeply nested CMake project.
# Keep this target deliberately short so MSBuild stays below MAX_PATH on Windows.
$vulkanTarget = [System.IO.Path]::GetFullPath('C:\sf-build\v')
$temporaryTargets = @($cpuTarget, $cudaTarget, $vulkanTarget)
$cudaArchitectures = '75-virtual;80-virtual;86-real;89-real;120a-real;121a-real'

function Assert-WorkspaceChild([string]$Path) {
    if (-not $Path.StartsWith(
        $rustRoot + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to modify a path outside the Rust workspace: $Path"
    }
}

function Assert-OwnedBuildTarget([string]$Path) {
    $resolved = [System.IO.Path]::GetFullPath($Path)
    if (-not ($temporaryTargets | Where-Object {
        $resolved.Equals($_, [System.StringComparison]::OrdinalIgnoreCase)
    })) {
        throw "Refusing to modify an unowned build target: $resolved"
    }
}

Assert-WorkspaceChild $output
foreach ($target in $temporaryTargets) {
    Assert-OwnedBuildTarget $target
}

$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargo) {
    $candidate = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (Test-Path -LiteralPath $candidate) {
        $cargo = Get-Item -LiteralPath $candidate
    }
}
if (-not $cargo) {
    throw 'Rust cargo was not found.'
}
$cargoPath = if ($cargo.Source) {
    $cargo.Source
} elseif ($cargo.FullName) {
    $cargo.FullName
} else {
    throw 'The resolved cargo command does not expose an executable path.'
}

$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
$visualStudioInstance = if (Test-Path -LiteralPath $vswhere) {
    & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath
}
if (-not $visualStudioInstance) {
    throw 'Visual Studio 2022 C++ Build Tools were not found.'
}
$visualStudioInstance = [System.IO.Path]::GetFullPath($visualStudioInstance.Trim())
$vsDevCmd = Join-Path $visualStudioInstance 'Common7\Tools\VsDevCmd.bat'
$ninja = Join-Path $visualStudioInstance `
    'Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe'
foreach ($tool in @($vsDevCmd, $ninja)) {
    if (-not (Test-Path -LiteralPath $tool)) {
        throw "A required Visual Studio build tool is missing: $tool"
    }
}

$cmakeDirectory = 'C:\Program Files\CMake\bin'
if (Test-Path -LiteralPath (Join-Path $cmakeDirectory 'cmake.exe')) {
    $env:PATH = "$cmakeDirectory;$env:PATH"
}

$cudaRoot = if ($env:CUDA_PATH -and (Test-Path -LiteralPath (Join-Path $env:CUDA_PATH 'bin\nvcc.exe'))) {
    [System.IO.Path]::GetFullPath($env:CUDA_PATH)
} else {
    Get-ChildItem 'C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA' -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'bin\nvcc.exe') } |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $cudaRoot) {
    throw 'A complete CUDA Toolkit with nvcc is required to publish the universal installer.'
}

$vulkanRoot = if ($env:VULKAN_SDK -and (Test-Path -LiteralPath (Join-Path $env:VULKAN_SDK 'Lib\vulkan-1.lib'))) {
    [System.IO.Path]::GetFullPath($env:VULKAN_SDK)
} else {
    Get-ChildItem 'C:\VulkanSDK' -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'Lib\vulkan-1.lib') } |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $vulkanRoot) {
    throw 'The Vulkan SDK is required to publish the universal installer.'
}

$previousCudaPath = $env:CUDA_PATH
$previousVulkanSdk = $env:VULKAN_SDK
$previousCudaArchitectures = $env:CMAKE_CUDA_ARCHITECTURES
$previousLibclangPath = $env:LIBCLANG_PATH
$previousGeneratorInstance = $env:CMAKE_GENERATOR_INSTANCE
$publishSucceeded = $false

try {
    $libclang = 'C:\Program Files\LLVM\bin\libclang.dll'
    if (-not (Test-Path -LiteralPath $libclang)) {
        throw 'LLVM libclang is required to generate Windows whisper bindings.'
    }
    $env:LIBCLANG_PATH = Split-Path -Parent $libclang
    $env:CMAKE_GENERATOR_INSTANCE = $visualStudioInstance
    New-Item -ItemType Directory -Force -Path $output | Out-Null
    Get-ChildItem -LiteralPath $output -Force -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force
    foreach ($target in $temporaryTargets) {
        if (Test-Path -LiteralPath $target) {
            Assert-OwnedBuildTarget $target
            Remove-Item -LiteralPath $target -Recurse -Force
        }
    }

    Push-Location $rustRoot
    try {
        & $cargoPath build --target-dir $cpuTarget --profile $profile --workspace
        if ($LASTEXITCODE -ne 0) { throw 'The Rust workspace build failed.' }

        $binaryMap = [ordered]@{
            'sayflex-speech-worker.exe' = 'SayFlex.Speech.Cpu.exe'
            'sayflex-desktop.exe' = 'SayFlex.exe'
            'sayflex-input-host.exe' = 'SayFlex.InputHost.exe'
            'sayflex-translation-worker.exe' = 'SayFlex.Translation.exe'
            'sayflex-companion-renderer.exe' = 'SayFlex.Companion.exe'
        }
        foreach ($sourceName in $binaryMap.Keys) {
            Copy-Item -LiteralPath (Join-Path $cpuTarget "$profile\$sourceName") `
                -Destination (Join-Path $output $binaryMap[$sourceName])
        }

        Copy-Item -LiteralPath (Join-Path $rustRoot '..\LICENSE') `
            -Destination (Join-Path $output 'LICENSE.txt')
        foreach ($document in @('THIRD_PARTY_NOTICES.md', 'SECURITY.md')) {
            Copy-Item -LiteralPath (Join-Path $rustRoot $document) `
                -Destination (Join-Path $output $document)
        }

        $env:CUDA_PATH = $cudaRoot
        $env:CMAKE_CUDA_ARCHITECTURES = $cudaArchitectures
        & $cargoPath build --target-dir $cudaTarget --profile $profile `
            -p sayflex-speech-worker --features cuda
        if ($LASTEXITCODE -ne 0) { throw 'The CUDA speech worker build failed.' }
        Copy-Item -LiteralPath (Join-Path $cudaTarget "$profile\sayflex-speech-worker.exe") `
            -Destination (Join-Path $output 'SayFlex.Speech.Cuda.exe')

        # whisper.cpp's nested Vulkan shader generator exceeds MSBuild's legacy
        # path/tracking constraints. Build this backend with Visual Studio's Ninja.
        $vulkanBuildCommand = @(
            "call `"$vsDevCmd`" -arch=x64 -host_arch=x64 >nul"
            "set `"PATH=$cmakeDirectory;$($env:LIBCLANG_PATH);$(Split-Path -Parent $ninja);%PATH%`""
            "set `"VULKAN_SDK=$vulkanRoot`""
            "set `"LIBCLANG_PATH=$($env:LIBCLANG_PATH)`""
            'set "CMAKE_GENERATOR=Ninja"'
            "set `"CMAKE_MAKE_PROGRAM=$ninja`""
            'set "CMAKE_GENERATOR_INSTANCE="'
            "`"$cargoPath`" build --target-dir `"$vulkanTarget`" --profile $profile -p sayflex-speech-worker --features vulkan"
        ) -join ' && '
        & $env:ComSpec /d /s /c $vulkanBuildCommand
        if ($LASTEXITCODE -ne 0) { throw 'The Vulkan speech worker build failed.' }
        Copy-Item -LiteralPath (Join-Path $vulkanTarget "$profile\sayflex-speech-worker.exe") `
            -Destination (Join-Path $output 'SayFlex.Speech.Vulkan.exe')

        $cudaRuntimeOutput = Join-Path $output 'runtimes\cuda'
        New-Item -ItemType Directory -Force -Path $cudaRuntimeOutput | Out-Null
        foreach ($dll in @('cublas64_13.dll', 'cublasLt64_13.dll')) {
            $source = Join-Path $cudaRoot "bin\x64\$dll"
            if (-not (Test-Path -LiteralPath $source)) {
                throw "The CUDA runtime is incomplete: $source is missing."
            }
            Copy-Item -LiteralPath $source -Destination (Join-Path $cudaRuntimeOutput $dll)
        }

        $manifest = [ordered]@{
            schema_version = 1
            selection_order = @('cuda', 'vulkan', 'cpu')
            backends = @(
                [ordered]@{
                    id = 'cuda'
                    executable = 'SayFlex.Speech.Cuda.exe'
                    probe = 'whisper_model_warmup'
                    cuda_architectures = $cudaArchitectures.Split(';')
                    runtime_directory = 'runtimes/cuda'
                },
                [ordered]@{
                    id = 'vulkan'
                    executable = 'SayFlex.Speech.Vulkan.exe'
                    probe = 'whisper_model_warmup'
                },
                [ordered]@{
                    id = 'cpu'
                    executable = 'SayFlex.Speech.Cpu.exe'
                    probe = 'whisper_model_warmup'
                }
            )
        }
        $manifest | ConvertTo-Json -Depth 8 |
            Set-Content -LiteralPath (Join-Path $output 'speech-backends.json') -Encoding utf8

        & (Join-Path $PSScriptRoot 'validate-release.ps1') -AppDirectory $output
        if ($LASTEXITCODE -ne 0) { throw 'Release validation failed.' }
        $publishSucceeded = $true
    }
    finally {
        Pop-Location
    }
}
finally {
    $env:CUDA_PATH = $previousCudaPath
    $env:VULKAN_SDK = $previousVulkanSdk
    $env:CMAKE_CUDA_ARCHITECTURES = $previousCudaArchitectures
    $env:LIBCLANG_PATH = $previousLibclangPath
    $env:CMAKE_GENERATOR_INSTANCE = $previousGeneratorInstance
    if (-not $KeepBuildArtifacts) {
        foreach ($target in $temporaryTargets) {
            if (Test-Path -LiteralPath $target) {
                Assert-OwnedBuildTarget $target
                Remove-Item -LiteralPath $target -Recurse -Force
            }
        }
    }
}

if ($publishSucceeded) {
    Write-Host "SayFlex universal release published to $output"
}
