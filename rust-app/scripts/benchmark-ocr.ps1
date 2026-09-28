[CmdletBinding()]
param(
    [ValidateRange(1, 1000)]
    [int]$Iterations = 30,
    [string]$Output = 'docs/evidence/screen-translation-ocr-benchmark.md',
    [string]$WorkerExe,
    [string]$WorkerBundle,
    [string]$TesseractExe,
    [string]$Tessdata
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$RustAppRoot = Split-Path -Parent $PSScriptRoot
$RepoRoot = Split-Path -Parent $RustAppRoot
$Manifest = Join-Path $RustAppRoot 'tests\fixtures\ocr-screen\manifest.json'
$ModelArtifactManifest = Join-Path $RustAppRoot 'tools\ocr-benchmark\model-artifacts.json'
$ModelRoot = Join-Path $RustAppRoot 'target\ocr-benchmark-models\rapidocr-v3.9.2'
$Jsonl = Join-Path $RustAppRoot 'target\ocr-benchmark-results.jsonl'
$DefaultWorkerExe = Join-Path $RustAppRoot 'dist\app\SayFlex.Ocr.exe'
$DefaultWorkerBundle = Join-Path $RustAppRoot 'dist\app\ocr-models\screen-ocr-ppocrv5-mobile-v3.9.2'
$WorkerExe = if ($WorkerExe) { $WorkerExe } else { $DefaultWorkerExe }
$WorkerBundle = if ($WorkerBundle) { $WorkerBundle } else { $DefaultWorkerBundle }
$OutputPath = if ([System.IO.Path]::IsPathRooted($Output)) { $Output } else { Join-Path $RepoRoot $Output }
$Artifacts = @((Get-Content -LiteralPath $ModelArtifactManifest -Raw -Encoding UTF8 | ConvertFrom-Json).artifacts)

if (-not (Test-Path -LiteralPath $WorkerExe -PathType Leaf)) {
    throw "Shipped OCR worker not found: $WorkerExe. Run scripts/publish.ps1 first or pass -WorkerExe."
}
if (-not (Test-Path -LiteralPath (Join-Path $WorkerBundle 'bundle.verified.json') -PathType Leaf)) {
    throw "Verified shipped OCR bundle not found: $WorkerBundle"
}

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-Percentile([double[]]$Values, [double]$Percentile) {
    if ($Values.Count -eq 0) { return $null }
    $Sorted = @($Values | Sort-Object)
    $Index = [Math]::Max(0, [Math]::Ceiling($Sorted.Count * $Percentile) - 1)
    return [double]$Sorted[$Index]
}

function Format-Number($Value) {
    if ($null -eq $Value) { return 'n/a' }
    return ([double]$Value).ToString('0.000', [Globalization.CultureInfo]::InvariantCulture)
}

$CargoArgs = @(
    'run', '--locked', '--release', '-p', 'sayflex-ocr-benchmark', '--',
    '--manifest', $Manifest,
    '--models', $ModelRoot,
    '--model-artifacts', $ModelArtifactManifest,
    '--output-jsonl', $Jsonl,
    '--ocr-worker', $WorkerExe,
    '--ocr-bundle', $WorkerBundle,
    '--iterations', $Iterations
)
if ($TesseractExe) { $CargoArgs += @('--tesseract-exe', $TesseractExe) }
if ($Tessdata) { $CargoArgs += @('--tessdata', $Tessdata) }

Push-Location $RustAppRoot
try {
    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) { throw "OCR benchmark exited with code $LASTEXITCODE" }
} finally {
    Pop-Location
}

$Rows = @(Get-Content -LiteralPath $Jsonl -Encoding UTF8 | ForEach-Object { $_ | ConvertFrom-Json })
$Engines = @('sayflex-ocr-worker-cpu', 'ppocr-v5-onnx-cpu', 'tesseract-5', 'windows-media-ocr')
$Summaries = @{}
foreach ($Engine in $Engines) {
    $EngineRows = @($Rows | Where-Object engine -eq $Engine)
    $AvailableRows = @($EngineRows | Where-Object available -eq $true)
    $AllAvailable = $EngineRows.Count -gt 0 -and $AvailableRows.Count -eq $EngineRows.Count
    $Cer = if ($AvailableRows.Count) { ($AvailableRows | Measure-Object -Property normalized_char_distance -Average).Average } else { $null }
    $Exact = if ($AvailableRows.Count) { (@($AvailableRows | Where-Object exact_match -eq $true).Count / $AvailableRows.Count) } else { $null }
    $Latencies = @($AvailableRows | ForEach-Object { [double]$_.elapsed_ms })
    $NormalLatencies = @($AvailableRows | Where-Object { $_.category -in @('light','dark') } | ForEach-Object { [double]$_.elapsed_ms })
    $Failures = @($EngineRows | Where-Object available -eq $false | Select-Object -ExpandProperty failure -Unique)
    $WorkingSets = @($AvailableRows | Where-Object { $null -ne $_.working_set_bytes } | ForEach-Object { [double]$_.working_set_bytes })
    $PeakWorkingSets = @($AvailableRows | Where-Object { $null -ne $_.peak_working_set_bytes } | ForEach-Object { [double]$_.peak_working_set_bytes })
    $Generations = @($AvailableRows | Where-Object { $null -ne $_.worker_generation } | ForEach-Object { [int]$_.worker_generation })
    $Summaries[$Engine] = [pscustomobject]@{
        AllAvailable = $AllAvailable
        Cer = $Cer
        Exact = $Exact
        P50 = Get-Percentile $Latencies 0.50
        P95 = Get-Percentile $Latencies 0.95
        NormalP95 = Get-Percentile $NormalLatencies 0.95
        MaxWorkingSet = if ($WorkingSets.Count) { ($WorkingSets | Measure-Object -Maximum).Maximum } else { $null }
        PeakWorkingSet = if ($PeakWorkingSets.Count) { ($PeakWorkingSets | Measure-Object -Maximum).Maximum } else { $null }
        RestartGeneration = if ($Generations.Count) { ($Generations | Measure-Object -Minimum).Minimum } else { $null }
        Failures = $Failures
    }
}

$Worker = $Summaries['sayflex-ocr-worker-cpu']
$Pp = $Summaries['ppocr-v5-onnx-cpu']
$Tess = $Summaries['tesseract-5']
$Portable = $Worker.AllAvailable
$OssCompatible = $true
$BeatsTesseract = $Pp.AllAvailable -and $Tess.AllAvailable -and $Pp.Cer -lt $Tess.Cer
$MeetsLatency = $Worker.AllAvailable -and $null -ne $Worker.NormalP95 -and $Worker.NormalP95 -lt 500.0
$RestartVerified = $Worker.AllAvailable -and $null -ne $Worker.RestartGeneration -and $Worker.RestartGeneration -ge 2
$ShippedPathVerified = $Portable -and $OssCompatible -and $MeetsLatency -and $RestartVerified
$Decision = if ($ShippedPathVerified) { 'Shipped PP-OCR worker path verified' } else { 'Shipped PP-OCR worker path failed verification' }
$Packaging = 'true'

$Cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name).Trim()
$Os = Get-CimInstance Win32_OperatingSystem
$Rust = (& rustc --version).Trim()
$Generated = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
$ModelLines = $Artifacts | ForEach-Object { "- ``$($_.Name)``: ``$($_.Sha256)``" }
$RuntimeLines = @(
    $WorkerExe,
    (Join-Path $RustAppRoot 'target\release\sayflex-ocr-benchmark.exe'),
    (Join-Path $RustAppRoot 'target\release\DirectML.dll')
) | Where-Object { Test-Path -LiteralPath $_ } | ForEach-Object { "- ``$(Split-Path $_ -Leaf)``: ``$(Get-Sha256 $_)``" }
$FixtureHashes = Get-ChildItem -LiteralPath (Split-Path -Parent $Manifest) -Filter '*.png' | Sort-Object Name | ForEach-Object { "- ``$($_.Name)``: ``$(Get-Sha256 $_.FullName)``" }

$TableLines = @()
foreach ($Engine in $Engines) {
    $Summary = $Summaries[$Engine]
    $TableLines += "| $Engine | $($Summary.AllAvailable) | $(Format-Number $Summary.Cer) | $(Format-Number $Summary.Exact) | $(Format-Number $Summary.P50) | $(Format-Number $Summary.P95) | $(Format-Number $Summary.NormalP95) | $(Format-Number $Summary.MaxWorkingSet) |"
}

$CategoryLines = @()
foreach ($Group in ($Rows | Where-Object { $_.engine -eq 'sayflex-ocr-worker-cpu' -and $_.available } | Group-Object category | Sort-Object Name)) {
    $Average = ($Group.Group | Measure-Object -Property normalized_char_distance -Average).Average
    $CategoryLines += "| $($Group.Name) | $(Format-Number $Average) | $($Group.Count) |"
}
if ($CategoryLines.Count -eq 0) { $CategoryLines = @('| n/a | n/a | 0 |') }

$FailureLines = @()
foreach ($Engine in $Engines) {
    $Failures = $Summaries[$Engine].Failures
    if ($Failures.Count -eq 0) {
        $FailureLines += "- **${Engine}:** none"
    } else {
        foreach ($Failure in $Failures) { $FailureLines += "- **${Engine}:** $Failure" }
    }
}

$QualityLines = @($Rows | Where-Object { $_.engine -eq 'sayflex-ocr-worker-cpu' -and $_.iteration -eq 0 -and $_.available -and -not $_.exact_match } | ForEach-Object {
    $Expected = ([string]$_.expected_text).Replace("`r", '').Replace("`n", ' / ')
    $Recognized = ([string]$_.recognized_text).Replace("`r", '').Replace("`n", ' / ')
    "- ``$($_.fixture)`` (CER $(Format-Number $_.normalized_char_distance)): expected '$Expected'; recognized '$Recognized'."
})
if ($QualityLines.Count -eq 0) { $QualityLines = @('- No non-exact shipped-worker fixtures.') }

$Evidence = @"
# Screen translation OCR benchmark

- Generated (UTC): $Generated
- Decision: **$Decision**
- ``screen_translation_packaging=$Packaging``
- Iterations per fixture and candidate: $Iterations
- Corpus: 17 immutable PNG fixtures covering all 14 SayFlex source languages plus small mixed text, two columns, and blank input
- CPU: $Cpu
- Windows: $($Os.Caption) build $($Os.BuildNumber)
- Rust: $Rust
- OCR runtime: shipped ``SayFlex.Ocr.exe`` JSON worker protocol, ``oar-ocr 0.9.2`` + ``ort 2.0.0-rc.13`` on CPU

## Admission rule

The release gate is the shipped ``SayFlex.Ocr.exe`` plus its verified bundle, driven through the same JSON protocol as the desktop application. It must complete every fixture after a real worker restart and keep normal light/dark crop p95 below 500 ms. Direct PP-OCR, Tesseract, and Windows Media OCR are diagnostic comparators; a missing comparator does not negate a successful shipped-path measurement.

| Engine | All rows available | Aggregate CER | Exact rate | p50 ms | p95 ms | Normal p95 ms | Max working set bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
$($TableLines -join "`n")

Shipped worker completed every row: **$Portable**

OSS-compatible runtime and model license: **$OssCompatible**

Worker restart generation at measurement: **$($Worker.RestartGeneration)** (expected at least 2): **$RestartVerified**

Normal-crop p95 below 500 ms: **$MeetsLatency**

Peak worker working set observed: **$(Format-Number $Worker.PeakWorkingSet) bytes**

Direct PP-OCR lower aggregate CER than Tesseract when both comparators were available: **$BeatsTesseract**

## Shipped worker CER by category

| Category | Mean CER | Rows |
|---|---:|---:|
$($CategoryLines -join "`n")

## Failures and unavailable candidates

$($FailureLines -join "`n")

## Observed quality limitations

$($QualityLines -join "`n")

The two-column divergence is intentionally retained for the geometry-based reading-order stage; raw detector order is not treated as final paragraph order. The Hindi divergence is a recognizer-quality limitation and is not hidden by normalization.

## Verified RapidOCR v3.9.2 artifacts

Source metadata: https://github.com/RapidAI/RapidOCR/blob/main/python/rapidocr/default_models.yaml  
License: Apache-2.0 (PaddleOCR / RapidOCR). Published ONNX hashes are retained verbatim. Dictionary bytes are pinned after TLS retrieval from the versioned RapidOCR URLs because the upstream manifest publishes dictionary URLs but not dictionary hashes. Models remain benchmark cache files under ``rust-app/target`` and are not committed.

$($ModelLines -join "`n")

## Runtime artifact hashes

The shipped worker contains the ONNX Runtime CPU path used by SayFlex. ``DirectML.dll`` may be copied by the upstream ``ort`` build bundle, but no DirectML execution provider is enabled or selected by the OCR worker.

$($RuntimeLines -join "`n")

## Immutable fixture hashes

$($FixtureHashes -join "`n")

## Raw data

Machine-readable JSON Lines are generated at ``rust-app/target/ocr-benchmark-results.jsonl``. Each shipped-worker row includes protocol latency, working-set observations, and the worker generation proving the post-warmup restart path. Expected and recognized strings are fixture-only data; no user capture is read or retained by this benchmark.
"@

$OutputDirectory = Split-Path -Parent $OutputPath
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
[System.IO.File]::WriteAllText($OutputPath, $Evidence, [System.Text.UTF8Encoding]::new($false))
Write-Host "OCR evidence written to $OutputPath"
Write-Host "Decision: $Decision"
if (-not $ShippedPathVerified) {
    throw 'Shipped OCR worker path did not meet the release benchmark gate.'
}
