[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release',
    [switch]$KeepBuildArtifacts,
    [string]$SigningThumbprint,
    [switch]$RequireSignature,
    [string]$VulkanTarget,
    # An isolated, explicitly unofficial local package. Normal publishing keeps
    # its clean-source and signature gates; this mode never uploads anything.
    [switch]$TestNvidia
)

$ErrorActionPreference = 'Stop'
if ($TestNvidia -and ($RequireSignature -or -not [string]::IsNullOrWhiteSpace($SigningThumbprint))) {
    throw 'The local test package cannot use public-release signing options.'
}
if ($RequireSignature -and [string]::IsNullOrWhiteSpace($SigningThumbprint)) {
    throw 'A public artifact requires -SigningThumbprint.'
}
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$gitCommit = (& git -C $rustRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $gitCommit -notmatch '^[0-9a-f]{40}$') {
    throw 'The release source commit could not be identified.'
}
$dirty = @(& git -C $rustRoot status --porcelain)
if ($LASTEXITCODE -ne 0 -or (-not $TestNvidia -and $dirty.Count -ne 0)) {
    throw 'A release must be built from a clean committed worktree.'
}
if ($TestNvidia) {
    Import-Module (Join-Path $PSScriptRoot 'test-nvidia-packaging.psm1') -Force
    $sourceSnapshot = Get-TestNvidiaSourceSnapshot -RepositoryPath $rustRoot
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
$outputRelative = if ($TestNvidia) { 'dist\test-nvidia\app' } else { 'dist\app' }
$output = [System.IO.Path]::GetFullPath((Join-Path $rustRoot $outputRelative))
$profile = $Configuration.ToLowerInvariant()
$cpuTargetRelative = if ($TestNvidia) { 'target' } else { 'target\publish-cpu' }
$cudaTargetRelative = if ($TestNvidia) { 'target' } else { 'target\publish-cuda' }
$cpuTarget = [System.IO.Path]::GetFullPath((Join-Path $rustRoot $cpuTargetRelative))
$cudaTarget = [System.IO.Path]::GetFullPath((Join-Path $rustRoot $cudaTargetRelative))
# whisper.cpp builds its Vulkan shader generator as a deeply nested CMake project.
# Keep this target deliberately short so MSBuild stays below MAX_PATH on Windows.
$vulkanTarget = [System.IO.Path]::GetFullPath(
    $(if ([string]::IsNullOrWhiteSpace($VulkanTarget)) {
        if ($TestNvidia) { 'C:\sf-build\nt' } else { 'C:\sf-build\v' }
    } else { $VulkanTarget })
)
$compatTarget = [System.IO.Path]::GetFullPath((Join-Path $rustRoot 'target\publish-cpu-compat'))
$temporaryTargets = @($cpuTarget, $cudaTarget, $vulkanTarget, $compatTarget)
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
$previousNativeCReleaseFlags = $env:CMAKE_C_FLAGS_RELEASE
$previousNativeCxxReleaseFlags = $env:CMAKE_CXX_FLAGS_RELEASE
$previousGgml = @{}
$previousCudaRuntimeLibrary = $env:CMAKE_CUDA_RUNTIME_LIBRARY
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
    # cmake-rs 0.1.58 replaces MSVC's Release flags when the generator is
    # implicit, dropping /O2. Pin native optimization for whisper/ggml too;
    # Cargo's --release optimizes Rust but cannot repair these CMake flags.
    $env:CMAKE_C_FLAGS_RELEASE = '/MD /O2 /Ob2 /DNDEBUG'
    $env:CMAKE_CXX_FLAGS_RELEASE = '/MD /O2 /Ob2 /DNDEBUG'
    # whisper-rs-sys passes GGML_* variables to CMake. GGML_NATIVE detected the
    # build machine's instructions, so a build host with AVX-512 shipped
    # AVX-512. Pin the x86-64-v3 level (AVX2, FMA, F16C, BMI2) instead; the
    # compatibility CPU worker below covers older processors.
    $ggmlLevel = [ordered]@{
        GGML_NATIVE = 'OFF'; GGML_AVX512 = 'OFF'; GGML_AVX2 = 'ON'; GGML_AVX = 'ON'
        GGML_FMA = 'ON'; GGML_F16C = 'ON'; GGML_BMI2 = 'ON'; GGML_SSE42 = 'ON'
    }
    foreach ($name in $ggmlLevel.Keys) {
        $previousGgml[$name] = [Environment]::GetEnvironmentVariable($name)
        [Environment]::SetEnvironmentVariable($name, $ggmlLevel[$name])
    }
    New-Item -ItemType Directory -Force -Path $output | Out-Null
    Get-ChildItem -LiteralPath $output -Force -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force
    if (-not $TestNvidia) {
        foreach ($target in $temporaryTargets) {
            if (Test-Path -LiteralPath $target) {
                Assert-OwnedBuildTarget $target
                Remove-Item -LiteralPath $target -Recurse -Force
            }
        }
    }
    if ($TestNvidia) {
        # This must precede every executable probe, including OCR staging.
        Set-Content -LiteralPath (Join-Path $output 'sayflex-test-channel') -Value 'test-nvidia' -Encoding ascii
        @'
SayFlex Test - local NVIDIA runtime experiment

This is an unsigned, unofficial local test build, not a public release.
Install: %LOCALAPPDATA%\Programs\SayFlex-Test-Nvidia
Separate settings, history, models and runtimes: %LOCALAPPDATA%\SayFlex-Test-Nvidia
The normal SayFlex installation and data are not imported or modified.
Windows autostart is disabled for this build. Launch it from SayFlex Test.

The CUDA speech worker links the NVIDIA CUDA runtime dynamically. No NVIDIA
file is bundled: cudart and cuBLAS are downloaded from NVIDIA only after your
explicit action in the test app.
CPU/Vulkan speech and translation remain available without that download.
This build is for local functional testing and is not approved by SignPath Foundation.

Uninstall only "SayFlex Test" in Windows Settings > Apps. Its private data
is retained. To remove it as well, close SayFlex Test and manually delete
only %LOCALAPPDATA%\SayFlex-Test-Nvidia. Do not delete %LOCALAPPDATA%\SayFlex.
'@ | Set-Content -LiteralPath (Join-Path $output 'TEST-BUILD-README.txt') -Encoding utf8
    }

    Push-Location $rustRoot
    try {
        & $cargoPath build --locked --target-dir $cpuTarget --profile $profile --workspace --exclude sayflex-desktop
        if ($LASTEXITCODE -ne 0) { throw 'The Rust workspace build failed.' }

        # The workspace enables PP-OCR for the OCR worker. Cargo unifies that
        # dependency feature into desktop too, unnecessarily linking ONNX and
        # DirectML into the UI. Build desktop separately before staging, retaining
        # the full-featured worker executables produced by the workspace build.
        & $cargoPath build --locked --target-dir $cpuTarget --profile $profile -p sayflex-desktop
        if ($LASTEXITCODE -ne 0) { throw 'The isolated desktop build failed.' }

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
        $symbolsRelative = if ($TestNvidia) { 'dist\test-nvidia\symbols' } else { 'dist\symbols' }
        $symbolsOutput = [System.IO.Path]::GetFullPath((Join-Path $rustRoot $symbolsRelative))
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
        # Run the installer as its own process. Called inline, its progress on
        # stderr went through PowerShell's error stream under
        # ErrorActionPreference=Stop, and when this script's output was
        # redirected the call was cut short mid-download while $LASTEXITCODE
        # still held cargo's 0 (2026-10-03). Its log stays outside the payload.
        $ocrLog = [System.IO.Path]::GetTempFileName()
        $ocrInstall = Start-Process -FilePath (Join-Path $output 'SayFlex.Ocr.exe') `
            -ArgumentList '--install-models' -NoNewWindow -Wait -PassThru `
            -RedirectStandardError $ocrLog
        if ($ocrInstall.ExitCode -ne 0) {
            throw "The verified OCR model bundle could not be staged: $(Get-Content -LiteralPath $ocrLog -Tail 3)"
        }
        Remove-Item -LiteralPath $ocrLog -Force -ErrorAction SilentlyContinue
        # The exit code alone is not proof; bundle.verified.json is written only
        # after every file of the bundle is verified.
        if (-not (Test-Path -LiteralPath (Join-Path $env:SAYFLEX_OCR_BUNDLE 'bundle.verified.json'))) {
            throw 'The OCR model bundle was not staged completely.'
        }
        # Inno Setup keeps file times to two seconds. The receipt records exact
        # times, so the installed bundle never matched it and SayFlex rewrote
        # the receipt inside its own installation. Round the times down to two
        # seconds now and verify again, so the receipt matches what is installed.
        Get-ChildItem -LiteralPath $env:SAYFLEX_OCR_BUNDLE -File -Recurse |
            Where-Object Name -ne 'bundle.verified.json' |
            ForEach-Object {
                $ticks = $_.LastWriteTimeUtc.Ticks
                $_.LastWriteTimeUtc = [DateTime]::new($ticks - ($ticks % 20000000), [DateTimeKind]::Utc)
            }
        $ocrLog = [System.IO.Path]::GetTempFileName()
        $ocrInstall = Start-Process -FilePath (Join-Path $output 'SayFlex.Ocr.exe') `
            -ArgumentList '--install-models' -NoNewWindow -Wait -PassThru `
            -RedirectStandardError $ocrLog
        if ($ocrInstall.ExitCode -ne 0) {
            throw "The OCR model bundle could not be verified again: $(Get-Content -LiteralPath $ocrLog -Tail 3)"
        }
        Remove-Item -LiteralPath $ocrLog -Force -ErrorAction SilentlyContinue

        # Visual C++ runtime beside the executables (app-local deployment, as
        # the Visual Studio redistribution terms allow). A clean Windows has no
        # VCRUNTIME140.dll, and every SayFlex process imports it.
        # Only numbered versions: Build Tools also ship a "v143" directory there.
        $crtRoot = Get-ChildItem -LiteralPath (Join-Path $visualStudioInstance 'VC\Redist\MSVC') -Directory |
            Where-Object Name -match '^\d+(\.\d+){1,3}$' |
            Sort-Object { [version]$_.Name } -Descending |
            ForEach-Object { Get-ChildItem -LiteralPath (Join-Path $_.FullName 'x64') -Directory -Filter 'Microsoft.VC14*.CRT' -ErrorAction SilentlyContinue } |
            Select-Object -First 1
        if (-not $crtRoot) {
            throw 'The Visual C++ x64 redistributable files were not found.'
        }
        foreach ($dll in @('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'msvcp140_1.dll')) {
            $source = Join-Path $crtRoot.FullName $dll
            if (-not (Test-Path -LiteralPath $source)) {
                throw "The Visual C++ runtime is incomplete: $source is missing."
            }
            Copy-Item -LiteralPath $source -Destination (Join-Path $output $dll)
        }

        # Silero VAD (MIT) keeps silence and noise from becoming text. It is
        # pinned by revision and SHA-256, cached outside the wiped targets,
        # and verified again by the speech workers before they load it.
        $vadFile = 'ggml-silero-v6.2.0.bin'
        $vadSha256 = '2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987'
        $vadUrl = 'https://huggingface.co/ggml-org/whisper-vad/resolve/' +
            "9ffd54a1e1ee413ddf265af9913beaf518d1639b/$vadFile"
        $vadCache = Join-Path $rustRoot "target\speech-vad\$vadFile"
        $vadCached = (Test-Path -LiteralPath $vadCache) -and
            (Get-FileHash -LiteralPath $vadCache -Algorithm SHA256).Hash.ToLowerInvariant() -eq $vadSha256
        if (-not $vadCached) {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $vadCache) | Out-Null
            $vadDownload = "$vadCache.download"
            Invoke-WebRequest -Uri $vadUrl -OutFile $vadDownload -UseBasicParsing
            $vadActual = (Get-FileHash -LiteralPath $vadDownload -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($vadActual -ne $vadSha256) {
                Remove-Item -LiteralPath $vadDownload -Force
                throw "The Silero VAD model does not match its pinned SHA-256 ($vadActual)."
            }
            Move-Item -LiteralPath $vadDownload -Destination $vadCache -Force
        }
        $vadOutput = Join-Path $output 'speech-vad'
        New-Item -ItemType Directory -Force -Path $vadOutput | Out-Null
        Copy-Item -LiteralPath $vadCache -Destination (Join-Path $vadOutput $vadFile)

        Copy-Item -LiteralPath (Join-Path $rustRoot '..\LICENSE') `
            -Destination (Join-Path $output 'LICENSE.txt')
        foreach ($document in @('THIRD_PARTY_NOTICES.md', 'SECURITY.md', 'PRIVACY.md')) {
            Copy-Item -LiteralPath (Join-Path $rustRoot $document) `
                -Destination (Join-Path $output $document)
        }
        & (Join-Path $PSScriptRoot 'generate-third-party-notices.ps1') `
            -OutputPath (Join-Path $output 'THIRD_PARTY_NOTICES.md') -TestNvidia:$TestNvidia

        $env:CUDA_PATH = $cudaRoot
        $env:CMAKE_CUDA_ARCHITECTURES = $cudaArchitectures
        # cargo hands its job count to whisper.cpp's CMake build as --parallel.
        # At 24 jobs, two dozen nvcc processes compile ggml's heaviest kernels
        # for six GPU architectures at once; on 2026-10-03 one of them (mmvq.cu)
        # exited 1 without a diagnostic. Eight is enough to keep the GPU build
        # short without that pressure.
        $cudaJobs = [Math]::Max(1, [Math]::Min(8, [Environment]::ProcessorCount))
        # Link the CUDA runtime dynamically: the installer then carries no
        # NVIDIA code, and cudart64_13.dll comes with the opt-in download.
        $env:CMAKE_CUDA_RUNTIME_LIBRARY = 'Shared'
        & $cargoPath build --locked --target-dir $cudaTarget --profile $profile `
            --jobs $cudaJobs -p sayflex-speech-worker --features cuda
        if ($LASTEXITCODE -ne 0) { throw 'The CUDA speech worker build failed.' }
        $env:CMAKE_CUDA_RUNTIME_LIBRARY = $previousCudaRuntimeLibrary
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

        # Processors without AVX2 (before 2013, and many later Pentium, Celeron
        # and Atom models) get a CPU worker built for SSE4.2. The desktop picks
        # it by CPUID. Its own target directory keeps the AVX2 build cached.
        foreach ($name in @('GGML_AVX2', 'GGML_AVX', 'GGML_FMA', 'GGML_F16C', 'GGML_BMI2')) {
            [Environment]::SetEnvironmentVariable($name, 'OFF')
        }
        & $cargoPath build --locked --target-dir $compatTarget --profile $profile `
            -p sayflex-speech-worker --features cpu-compat
        if ($LASTEXITCODE -ne 0) { throw 'The compatibility CPU speech worker build failed.' }
        foreach ($name in $ggmlLevel.Keys) {
            [Environment]::SetEnvironmentVariable($name, $ggmlLevel[$name])
        }
        Copy-Item -LiteralPath (Join-Path $compatTarget "$profile\sayflex-speech-worker.exe") `
            -Destination (Join-Path $output 'SayFlex.Speech.CpuCompat.exe')

        # No NVIDIA file ships in any installer. The CUDA worker links the CUDA
        # runtime dynamically; cudart64_13.dll and cuBLAS are downloaded from
        # NVIDIA inside the app, only after the user accepts the NVIDIA EULA.

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
                    runtime_directory = 'external-managed-runtime'
                    runtime_bundled = $false
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
        $buildManifest = [ordered]@{
            schema_version = 1
            product = 'SayFlex'
            version = $sourceVersion
            protocol_version = [int]$protocolVersion
            git_commit = $gitCommit
            cargo_lock_sha256 = (Get-FileHash -LiteralPath (Join-Path $rustRoot 'Cargo.lock') -Algorithm SHA256).Hash.ToLowerInvariant()
            built_utc = [DateTime]::UtcNow.ToString('o')
            files = $payloadFiles
        }
        if ($TestNvidia) {
            Assert-TestNvidiaSourceUnchanged -RepositoryPath $rustRoot -Snapshot $sourceSnapshot
            $buildManifest.channel = 'test-nvidia'
            $buildManifest.build_kind = 'local_test'
            $buildManifest.official_release = $false
            $buildManifest.source_snapshot = $sourceSnapshot
            Assert-TestNvidiaPayload -AppDirectory $output -Manifest ([pscustomobject]$buildManifest)
        }
        $buildManifest | ConvertTo-Json -Depth 8 |
            Set-Content -LiteralPath (Join-Path $output 'build-manifest.json') -Encoding utf8

        & (Join-Path $PSScriptRoot 'validate-release.ps1') `
            -AppDirectory $output `
            -RequireSignature:$RequireSignature `
            -TestNvidia:$TestNvidia
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
    $env:CMAKE_C_FLAGS_RELEASE = $previousNativeCReleaseFlags
    $env:CMAKE_CXX_FLAGS_RELEASE = $previousNativeCxxReleaseFlags
    $env:CMAKE_CUDA_RUNTIME_LIBRARY = $previousCudaRuntimeLibrary
    foreach ($name in $previousGgml.Keys) {
        [Environment]::SetEnvironmentVariable($name, $previousGgml[$name])
    }
    $env:SAYFLEX_OCR_BUNDLE = $previousOcrBundle
    if (-not $KeepBuildArtifacts -and -not $TestNvidia) {
        foreach ($target in $temporaryTargets) {
            if (Test-Path -LiteralPath $target) {
                Assert-OwnedBuildTarget $target
                Remove-Item -LiteralPath $target -Recurse -Force
            }
        }
    }
}

if ($publishSucceeded) {
    $label = if ($TestNvidia) { 'SayFlex local NVIDIA test build' } else { 'SayFlex universal release' }
    Write-Host "$label staged at $output"
}
