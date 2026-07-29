[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$WavPath,

    [Parameter(Mandatory)]
    [string]$ModelPath,

    [ValidateRange(1, 25)]
    [int]$Iterations = 5,

    [string]$WorkerPath = (
        Join-Path $PSScriptRoot '..\dist\app\StyleFlex.Speech.Cuda.exe'
    ),

    [ValidateRange(1, 5000)]
    [int]$MedianTargetMs = 300,

    [ValidateSet('OriginalLanguage', 'TranslateToEnglish')]
    [string]$OutputMode = 'OriginalLanguage'
)

$ErrorActionPreference = 'Stop'
$worker = [System.IO.Path]::GetFullPath($WorkerPath)
$wav = [System.IO.Path]::GetFullPath($WavPath)
$model = [System.IO.Path]::GetFullPath($ModelPath)

foreach ($required in @($worker, $wav, $model)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Required benchmark file is missing: $required"
    }
}

$samples = for ($iteration = 1; $iteration -le $Iterations; $iteration++) {
    $stderr = Join-Path $env:TEMP (
        "styleflex-benchmark-{0}-{1}.stderr.log" -f $PID, $iteration
    )
    $modeArgument = if ($OutputMode -eq 'TranslateToEnglish') {
        '--translate-to-english'
    } else {
        '--original-language'
    }
    $json = & $worker --transcribe-wav $wav $model $modeArgument 2>$stderr
    if ($LASTEXITCODE -ne 0) {
        throw "Speech benchmark iteration $iteration failed. See $stderr"
    }
    $result = $json | ConvertFrom-Json
    [pscustomobject]@{
        Iteration = $iteration
        AudioMs = [long]$result.audio_duration_ms
        PreprocessMs = [long]$result.latency.preprocessing_ms
        InferenceMs = [long]$result.latency.inference_ms
        AfterReleaseMs = [long]$result.latency.after_release_ms
        Text = [string]$result.text
    }
}

$ordered = @($samples.InferenceMs | Sort-Object)
$medianIndex = [math]::Floor(($ordered.Count - 1) * 0.5)
$p95Index = [math]::Ceiling(($ordered.Count - 1) * 0.95)
$median = $ordered[$medianIndex]
$p95 = $ordered[$p95Index]

$samples | Format-Table Iteration, AudioMs, PreprocessMs, InferenceMs, AfterReleaseMs
Write-Host "Median inference: $median ms"
Write-Host "P95 inference: $p95 ms"

if ($median -gt $MedianTargetMs) {
    throw "Median inference $median ms exceeds the $MedianTargetMs ms release target."
}
