[CmdletBinding()]
param(
    [string]$OutputPath,
    [switch]$TestNvidia
)

$ErrorActionPreference = 'Stop'
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    # Windows PowerShell evaluates default parameter expressions before it has
    # populated PSScriptRoot. Resolve the script-relative default in the body
    # so the same release command works in both powershell.exe and pwsh.exe.
    $OutputPath = Join-Path $rustRoot 'THIRD_PARTY_NOTICES.md'
}
$resolvedOutput = [System.IO.Path]::GetFullPath($OutputPath)

Push-Location $rustRoot
try {
    # The inventory renderer must not attempt to be a second SPDX parser.
    # In particular, accepting an expression merely because it contains one
    # familiar identifier would incorrectly accept `MIT AND Unknown-License`.
    # cargo-deny evaluates the complete SPDX expression, including AND/OR/WITH
    # semantics and the workspace policy in deny.toml.
    & cargo deny check licenses
    if ($LASTEXITCODE -ne 0) {
        throw 'cargo-deny rejected the locked dependency license graph.'
    }
    $metadataJson = & cargo metadata --format-version 1 --locked
    if ($LASTEXITCODE -ne 0) {
        throw 'cargo metadata failed while generating third-party notices.'
    }
} finally {
    Pop-Location
}

$metadata = $metadataJson | ConvertFrom-Json
$dependencyIndex = [System.Collections.Generic.SortedDictionary[string, object]]::new(
    [System.StringComparer]::Ordinal
)
foreach ($package in @($metadata.packages | Where-Object { $null -ne $_.source })) {
    $sortKey = "$($package.name)`0$($package.version)`0$($package.source)"
    $dependencyIndex.Add($sortKey, $package)
}
$dependencies = @($dependencyIndex.Values)
foreach ($package in $dependencies) {
    if ([string]::IsNullOrWhiteSpace($package.license)) {
        throw "Dependency $($package.name) $($package.version) has no declared license."
    }
}

$lines = [System.Collections.Generic.List[string]]::new()
$lines.Add('# Third-party notices')
$lines.Add('')
$lines.Add('The Rust application is distributed under the adjacent MIT license. The')
$lines.Add('dependency inventory below is generated from the exact locked Cargo graph;')
$lines.Add('`cargo-deny` remains the authoritative SPDX policy check.')
$lines.Add('')
$lines.Add('## Slint')
$lines.Add('')
$lines.Add('The user interface uses Slint 1.17 under the Slint Royalty-free Desktop,')
$lines.Add('Mobile, and Web Applications License 2.0. Public distribution must also')
$lines.Add('satisfy one of the attribution methods in section 2 of that license.')
$lines.Add('')
$lines.Add("Copyright $([char]0x00A9) SixtyFPS GmbH <info@slint.dev>")
$lines.Add('')
$lines.Add('<https://slint.dev/>')
$lines.Add('')
$lines.Add('## Whisper')
$lines.Add('')
$lines.Add('Speech recognition uses OpenAI Whisper model weights and the whisper.cpp')
$lines.Add('runtime through `whisper-rs`. Model artifacts are downloaded only after a')
$lines.Add('user choice and are verified by size and SHA-256 before activation.')
$lines.Add('')
$lines.Add('- OpenAI Whisper: <https://github.com/openai/whisper>')
$lines.Add('- whisper.cpp: <https://github.com/ggml-org/whisper.cpp>')
$lines.Add('- whisper-rs: <https://github.com/tazz4843/whisper-rs>')
$lines.Add('')
$lines.Add('## NVIDIA CUDA runtime')
$lines.Add('')
$lines.Add('The installer contains no NVIDIA files. The optional CUDA speech worker is')
$lines.Add('built from public source and links the CUDA runtime dynamically. Its NVIDIA')
$lines.Add('runtime files (`cudart64_13.dll`, `cublas64_13.dll`, `cublasLt64_13.dll`)')
$lines.Add('are downloaded from developer.download.nvidia.com inside SayFlex, only')
$lines.Add('after the user presses the download button and accepts the NVIDIA CUDA')
$lines.Add('Toolkit EULA: <https://docs.nvidia.com/cuda/eula/>. SayFlex checks the')
$lines.Add('SHA-256 of every archive and file. These files are proprietary NVIDIA')
$lines.Add('software and are not covered by the SayFlex MIT license.')
$lines.Add('')
$lines.Add('## Translation models')
$lines.Add('')
$lines.Add('Optional translation packs and llama.cpp are downloaded only after an')
$lines.Add('explicit user choice. Exact artifacts, checksums, origins, and model licenses')
$lines.Add('are documented in `docs/THIRD_PARTY_MODELS.md`.')
$lines.Add('')
$lines.Add('The compact English language-pair packages use `fxtranslate` under MPL-2.0 and')
$lines.Add('Firefox Translations v3 model artifacts under MPL-2.0. Packages are available')
$lines.Add('for Russian, Ukrainian, Spanish, Portuguese, German, French, Italian,')
$lines.Add('Simplified and Traditional Chinese, Japanese, Korean, Hindi, and Arabic. Model')
$lines.Add('files are installed separately on demand from Mozilla, verified in compressed')
$lines.Add('and expanded form, and are not bundled in the SayFlex installer.')
$lines.Add('')
$lines.Add('- fxtranslate: <https://github.com/gregtatum/translations>')
$lines.Add('- Firefox Translations: <https://github.com/mozilla/translations>')
$lines.Add('')
$lines.Add('## Screen translation OCR models')
$lines.Add('')
$lines.Add('Screen translation includes the PP-OCRv5 mobile ONNX detector, recognizers,')
$lines.Add('and dictionaries distributed by RapidOCR. These model artifacts are licensed')
$lines.Add('under Apache-2.0 and verified by size and SHA-256 before packaging. The shipped')
$lines.Add('bundle identity is `screen-ocr-ppocrv5-mobile-v3.9.2`; recognition is CPU-only.')
$lines.Add('HY-MT2 and other benchmark-only candidates are not shipped.')
$lines.Add('')
$lines.Add('- RapidOCR: <https://github.com/RapidAI/RapidOCR>')
$lines.Add('- PaddleOCR: <https://github.com/PaddlePaddle/PaddleOCR>')
$lines.Add('')
$lines.Add('## Locked Rust dependency inventory')
$lines.Add('')
$lines.Add('| Package | Version | Declared license | Source |')
$lines.Add('|---|---:|---|---|')
foreach ($package in $dependencies) {
    $source = [string]$package.source
    $source = $source.Replace('|', '\|')
    $license = ([string]$package.license).Replace('|', '\|')
    $lines.Add("| $($package.name) | $($package.version) | $license | $source |")
}
$lines.Add('')
$lines.Add("Generated from Cargo.lock; $($dependencies.Count) registry or git package versions listed.")

$parent = Split-Path -Parent $resolvedOutput
if (-not (Test-Path -LiteralPath $parent)) {
    New-Item -ItemType Directory -Path $parent | Out-Null
}
[System.IO.File]::WriteAllLines($resolvedOutput, $lines, [System.Text.UTF8Encoding]::new($false))
Write-Host "Generated $resolvedOutput with $($dependencies.Count) third-party package versions."
