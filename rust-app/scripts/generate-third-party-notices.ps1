[CmdletBinding()]
param(
    [string]$OutputPath = (Join-Path $PSScriptRoot '..\THIRD_PARTY_NOTICES.md')
)

$ErrorActionPreference = 'Stop'
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
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
$dependencies = @(
    $metadata.packages |
        Where-Object { $null -ne $_.source } |
        Sort-Object name, version -Unique
)
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
$lines.Add('Copyright © SixtyFPS GmbH <info@slint.dev>')
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
$lines.Add('## Translation models')
$lines.Add('')
$lines.Add('Optional translation packs and llama.cpp are downloaded only after an')
$lines.Add('explicit user choice. Exact artifacts, checksums, origins, and model licenses')
$lines.Add('are documented in `docs/THIRD_PARTY_MODELS.md`.')
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
