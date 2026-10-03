[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release',
    [switch]$KeepBuildArtifacts,
    [string]$SigningThumbprint,
    [switch]$RequireSignature,
    [string]$VulkanTarget
)

$ErrorActionPreference = 'Stop'
if ($RequireSignature -and [string]::IsNullOrWhiteSpace($SigningThumbprint)) {
    throw 'A public artifact requires -SigningThumbprint.'
}
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$gitCommit = (& git -C $rustRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $gitCommit -notmatch '^[0-9a-f]{40}$') {
    throw 'The release source commit could not be identified.'
}
$dirty = @(& git -C $rustRoot status --porcelain)
if ($LASTEXITCODE -ne 0 -or $dirty.Count -ne 0) {
    throw 'A release must be built from a clean committed worktree.'
}

# One publish at a time.
#
# Every run wipes target\publish-cpu, target\publish-cuda, C:\sf-build\v and
# dist\app before it starts. A second run therefore destroys the first one's
# work in progress, and both fail — which is exactly what happened, twice, and
# is not something an operator should have to remember. The mutex is held for
# the life of the process and released however it exits.
$publishMutex = New-Object System.Threading.Mutex($false, 'Global\SayFlex.Publish')
if (-not $publishMutex.WaitOne(0)) {
    throw (
        'Another SayFlex publish is already running. ' +
        'They share their build and staging directories, so a second one would destroy the first.'
    )
}
$output = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'dist\app'))
$profile = $Configuration.ToLowerInvariant()
$cpuTarget = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'target\publish-cpu'))
$cudaTarget = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'target\publish-cuda'))
# whisper.cpp builds its Vulkan shader generator as a deeply nested CMake project.
# Keep this target deliberately short so MSBuild stays below MAX_PATH on Windows.
$vulkanTarget = [System.IO.Path]::GetFullPath(
    $(if ([string]::IsNullOrWhiteSpace($VulkanTarget)) { 'C:\sf-build\v' } else { $VulkanTarget })
)
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

# Git for Windows ships a GNU `link` in usr\bin. If this script is started from
# anything that put that directory on PATH — a Git Bash shell, or a tool that
# inherited one — rustc finds it instead of the MSVC linker and every build
# script fails with "extra operand". The CUDA pass survives because cargo is
# invoked from PowerShell; the Vulkan pass hands PATH through to cmd.exe and
# does not. Take those entries out once, for the whole script.
$env:PATH = (
    $env:PATH -split ';' |
    Where-Object { $_ -and $_ -notmatch '\\Git\\usr\\bin' -and $_ -notmatch '\\Git\\bin$' }
) -join ';'

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
$previousBuildCommit = $env:SAYFLEX_BUILD_COMMIT
$previousOcrBundle = $env:SAYFLEX_OCR_BUNDLE
$previousRequireSimd = $env:FXTRANSLATE_REQUIRE_SIMD
$publishSucceeded = $false

try {
    $libclang = 'C:\Program Files\LLVM\bin\libclang.dll'
    if (-not (Test-Path -LiteralPath $libclang)) {
        throw 'LLVM libclang is required to generate Windows whisper bindings.'
    }
    $env:LIBCLANG_PATH = Split-Path -Parent $libclang
    $env:CMAKE_GENERATOR_INSTANCE = $visualStudioInstance
    $env:SAYFLEX_BUILD_COMMIT = $gitCommit
    # The compact translator's int8 SIMD kernel must be compiled in. Without
    # this, a C++ build failure silently shipped the ~8x slower scalar kernel.
    $env:FXTRANSLATE_REQUIRE_SIMD = '1'
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
        & $cargoPath build --locked --target-dir $cpuTarget --profile $profile --workspace
        if ($LASTEXITCODE -ne 0) { throw 'The Rust workspace build failed.' }

        $binaryMap = [ordered]@{
            'sayflex-speech-worker.exe' = 'SayFlex.Speech.Cpu.exe'
            'sayflex-desktop.exe' = 'SayFlex.exe'
            'sayflex-input-host.exe' = 'SayFlex.InputHost.exe'
            'sayflex-translation-worker.exe' = 'SayFlex.Translation.exe'
            'sayflex-ocr-worker.exe' = 'SayFlex.Ocr.exe'
            'sayflex-companion-renderer.exe' = 'SayFlex.Companion.exe'
        }
        foreach ($sourceName in $binaryMap.Keys) {
            Copy-Item -LiteralPath (Join-Path $cpuTarget "$profile\$sourceName") `
                -Destination (Join-Path $output $binaryMap[$sourceName])
        }
        # Symbols stay beside the payload, never inside it: panic logs record
        # module offsets, and these PDBs from the same build resolve them.
        $symbolsOutput = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'dist\symbols'))
        New-Item -ItemType Directory -Force -Path $symbolsOutput | Out-Null
        Get-ChildItem -LiteralPath $symbolsOutput -Force -ErrorAction SilentlyContinue |
            Remove-Item -Recurse -Force
        foreach ($sourceName in $binaryMap.Keys) {
            $pdb = Join-Path $cpuTarget ("$profile\" + ($sourceName -replace '-', '_' -replace '\.exe$', '.pdb'))
            if (Test-Path -LiteralPath $pdb) {
                Copy-Item -LiteralPath $pdb -Destination $symbolsOutput
            }
        }

        $env:SAYFLEX_OCR_BUNDLE = Join-Path $output `
            'ocr-models\screen-ocr-ppocrv5-mobile-v3.9.2'
        & (Join-Path $output 'SayFlex.Ocr.exe') --install-models
        if ($LASTEXITCODE -ne 0) { throw 'The verified OCR model bundle could not be staged.' }

        Copy-Item -LiteralPath (Join-Path $rustRoot '..\LICENSE') `
            -Destination (Join-Path $output 'LICENSE.txt')
        foreach ($document in @('THIRD_PARTY_NOTICES.md', 'SECURITY.md')) {
            Copy-Item -LiteralPath (Join-Path $rustRoot $document) `
                -Destination (Join-Path $output $document)
        }

        $env:CUDA_PATH = $cudaRoot
        $env:CMAKE_CUDA_ARCHITECTURES = $cudaArchitectures
        & $cargoPath build --locked --target-dir $cudaTarget --profile $profile `
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
            "`"$cargoPath`" build --locked --target-dir `"$vulkanTarget`" --profile $profile -p sayflex-speech-worker --features vulkan"
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

        if (-not [string]::IsNullOrWhiteSpace($SigningThumbprint)) {
            $executables = Get-ChildItem -LiteralPath $output -Filter '*.exe' -File |
                Select-Object -ExpandProperty FullName
            & (Join-Path $PSScriptRoot 'sign-artifacts.ps1') `
                -Path $executables `
                -CertificateThumbprint $SigningThumbprint
            if ($LASTEXITCODE -ne 0) { throw 'Executable signing failed.' }
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

        # Bind the staged payload to its exact source and dependency graph.
        # This manifest intentionally does not hash itself.
        $desktopPath = Join-Path $output 'SayFlex.exe'
        $protocolOutput = [System.IO.Path]::GetTempFileName()
        try {
            $probe = Start-Process -FilePath $desktopPath -ArgumentList '--protocol-version' `
                -NoNewWindow -PassThru -RedirectStandardOutput $protocolOutput
            $null = $probe.Handle
            if (-not $probe.WaitForExit(20000)) {
                $probe.Kill()
                throw 'The built desktop did not report its protocol version.'
            }
            $protocolVersion = "$(Get-Content -LiteralPath $protocolOutput -Raw)".Trim()
            if ($probe.ExitCode -ne 0 -or $protocolVersion -notmatch '^\d+$') {
                throw "The built desktop reported an invalid protocol version: '$protocolVersion'."
            }
        } finally {
            Remove-Item -LiteralPath $protocolOutput -Force -ErrorAction SilentlyContinue
        }
        $sourceVersion = (
            Select-String -Path (Join-Path $rustRoot 'Cargo.toml') -Pattern '^version = "(.+)"' |
                Select-Object -First 1
        ).Matches[0].Groups[1].Value
        $payloadFiles = @(
            Get-ChildItem -LiteralPath $output -File -Recurse |
                Where-Object Name -ne 'build-manifest.json' |
                Sort-Object FullName |
                ForEach-Object {
                    [ordered]@{
                        path = $_.FullName.Substring($output.Length + 1).Replace('\', '/')
                        bytes = $_.Length
                        sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
                    }
                }
        )
        [ordered]@{
            schema_version = 1
            product = 'SayFlex'
            version = $sourceVersion
            protocol_version = [int]$protocolVersion
            git_commit = $gitCommit
            cargo_lock_sha256 = (Get-FileHash -LiteralPath (Join-Path $rustRoot 'Cargo.lock') -Algorithm SHA256).Hash.ToLowerInvariant()
            built_utc = [DateTime]::UtcNow.ToString('o')
            files = $payloadFiles
        } | ConvertTo-Json -Depth 6 |
            Set-Content -LiteralPath (Join-Path $output 'build-manifest.json') -Encoding utf8

        & (Join-Path $PSScriptRoot 'validate-release.ps1') `
            -AppDirectory $output `
            -RequireSignature:$RequireSignature
        if ($LASTEXITCODE -ne 0) { throw 'Release validation failed.' }
        $publishSucceeded = $true
    }
    finally {
        Pop-Location
    }
}
finally {
    $publishMutex.ReleaseMutex()
    $publishMutex.Dispose()
    $env:CUDA_PATH = $previousCudaPath
    $env:VULKAN_SDK = $previousVulkanSdk
    $env:CMAKE_CUDA_ARCHITECTURES = $previousCudaArchitectures
    $env:LIBCLANG_PATH = $previousLibclangPath
    $env:CMAKE_GENERATOR_INSTANCE = $previousGeneratorInstance
    $env:SAYFLEX_BUILD_COMMIT = $previousBuildCommit
    $env:FXTRANSLATE_REQUIRE_SIMD = $previousRequireSimd
    $env:SAYFLEX_OCR_BUNDLE = $previousOcrBundle
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
