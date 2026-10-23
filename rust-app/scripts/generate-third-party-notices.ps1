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
$lines.Add('SayFlex.exe is distributed under GPL-3.0-only and the other SayFlex programs')
$lines.Add('under the MIT License; both texts are in the adjacent LICENSE.txt. The')
$lines.Add('dependency inventory below is generated from the exact locked Cargo graph;')
$lines.Add('`cargo-deny` remains the authoritative SPDX policy check.')
$lines.Add('')
$lines.Add('## Slint')
$lines.Add('')
$lines.Add('The user interface uses Slint 1.17 under the GNU General Public License')
$lines.Add('version 3 (GPL-3.0-only), one of the licenses Slint is offered under.')
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
$mitText = @'
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
'@

function Add-LicenseText([string]$Title, [string]$Text) {
    $lines.Add("### $Title")
    $lines.Add('')
    $lines.Add('```text')
    foreach ($line in ($Text.TrimEnd() -split "`r?`n")) { $lines.Add($line) }
    $lines.Add('```')
    $lines.Add('')
}

function Find-PackageDirectory([string]$Name) {
    $package = $metadata.packages | Where-Object name -eq $Name | Select-Object -First 1
    if (-not $package) { throw "The package $Name is not in the locked graph." }
    Split-Path -Parent $package.manifest_path
}

# Native code compiled into the executables, and files shipped beside them.
# Rust packages are listed further below; these are the parts cargo cannot see.
$lines.Add('## Native components')
$lines.Add('')
$lines.Add('- whisper.cpp and ggml (speech recognition, compiled into the speech workers): MIT.')
$lines.Add('- Silero VAD v6.2.0 (`speech-vad/ggml-silero-v6.2.0.bin`, voice activity detection): MIT.')
$lines.Add('- ONNX Runtime (screen-text recognition, compiled into `SayFlex.Ocr.exe`): MIT;')
$lines.Add('  its own third-party notices: <https://github.com/microsoft/onnxruntime/blob/main/ThirdPartyNotices.txt>.')
$lines.Add('- gemmology (int8 matrix kernel in the translation worker): MIT.')
$lines.Add('- xsimd (SIMD wrappers used by gemmology): BSD-3-Clause.')
$lines.Add('- Microsoft Visual C++ runtime (`vcruntime140*.dll`, `msvcp140*.dll`): redistributed')
$lines.Add('  unmodified under the Microsoft Visual Studio redistribution terms, which allow')
$lines.Add('  app-local deployment; it is not covered by the SayFlex MIT license.')
$lines.Add('- llama.cpp (contextual translation, MIT) is downloaded from its GitHub release only')
$lines.Add('  after the user chooses a contextual model; it is not in the installer.')
$lines.Add('')
Add-LicenseText 'whisper.cpp and ggml (MIT)' (
    Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path (Find-PackageDirectory 'whisper-rs-sys') 'whisper.cpp\LICENSE'))
Add-LicenseText 'Silero VAD (MIT)' ("MIT License`n`nCopyright (c) 2020-present Silero Team`n`n" + $mitText)
Add-LicenseText 'ONNX Runtime (MIT)' ("MIT License`n`nCopyright (c) Microsoft Corporation`n`n" + $mitText)
Add-LicenseText 'gemmology (MIT)' (
    Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $rustRoot 'vendor\fxtranslate\vendor\gemmology\LICENSE'))
Add-LicenseText 'xsimd (BSD-3-Clause)' (
    Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $rustRoot 'vendor\fxtranslate\vendor\xsimd\LICENSE'))

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
$lines.Add('')

# The full texts, as each package publishes them. Packages that ship the same
# text share one copy; a package without a license file is covered by the SPDX
# license it declares above.
$lines.Add('## License texts of the Rust dependencies')
$lines.Add('')
$texts = [System.Collections.Generic.Dictionary[string, object]]::new([System.StringComparer]::Ordinal)
$withoutText = [System.Collections.Generic.List[string]]::new()
foreach ($package in $dependencies) {
    $directory = Split-Path -Parent $package.manifest_path
    # Ordinal order: Sort-Object compares differently in Windows PowerShell
    # and PowerShell 7, and the notices must be identical from either.
    $files = [System.Collections.Generic.List[System.IO.FileInfo]]::new()
    foreach ($file in @(Get-ChildItem -LiteralPath $directory -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^(?i)(licen[cs]e|copying|notice)([-._].*)?$' })) {
        $files.Add($file)
    }
    $files.Sort([System.Comparison[System.IO.FileInfo]] {
        param($left, $right)
        [string]::CompareOrdinal($left.Name, $right.Name)
    })
    if ($files.Count -eq 0) {
        $withoutText.Add("$($package.name) $($package.version)")
        continue
    }
    foreach ($file in $files) {
        $text = (Get-Content -Raw -Encoding UTF8 -LiteralPath $file.FullName) -replace "`r`n", "`n"
        $key = (Get-FileHash -InputStream ([System.IO.MemoryStream]::new(
            [System.Text.Encoding]::UTF8.GetBytes($text.Trim()))) -Algorithm SHA256).Hash
        if (-not $texts.ContainsKey($key)) {
            $texts[$key] = [pscustomobject]@{ Text = $text; Users = [System.Collections.Generic.List[string]]::new() }
        }
        $texts[$key].Users.Add("$($package.name) $($package.version) ($($file.Name))")
    }
}
$index = 0
foreach ($entry in $texts.Values) {
    $index++
    $lines.Add("### Text $index")
    $lines.Add('')
    $users = [System.Collections.Generic.SortedSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($user in $entry.Users) { [void]$users.Add($user) }
    $lines.Add('Used by: ' + (@($users) -join ', '))
    $lines.Add('')
    $lines.Add('```text')
    foreach ($line in ($entry.Text.TrimEnd() -split "`n")) { $lines.Add($line) }
    $lines.Add('```')
    $lines.Add('')
}
if ($withoutText.Count -gt 0) {
    $lines.Add('Packages without a license file in their published source (their declared')
    $lines.Add('SPDX license in the inventory applies): ' + ($withoutText -join ', ') + '.')
    $lines.Add('')
}

$parent = Split-Path -Parent $resolvedOutput
if (-not (Test-Path -LiteralPath $parent)) {
    New-Item -ItemType Directory -Path $parent | Out-Null
}
[System.IO.File]::WriteAllLines($resolvedOutput, $lines, [System.Text.UTF8Encoding]::new($false))
Write-Host "Generated $resolvedOutput with $($dependencies.Count) third-party package versions."
