[CmdletBinding()]
param(
    [string]$Worker = (
        Join-Path $PSScriptRoot '..\dist\app\StyleFlex.Translation.exe'
    ),
    [ValidateSet('tiny', 'teacher')]
    [string]$Tier = 'teacher',
    [ValidateRange(1, 50)]
    [int]$Iterations = 5
)

$ErrorActionPreference = 'Stop'
$workerPath = [System.IO.Path]::GetFullPath($Worker)
if (-not (Test-Path -LiteralPath $workerPath)) {
    throw "Translation worker was not found: $workerPath"
}

$startInfo = [System.Diagnostics.ProcessStartInfo]::new()
$startInfo.FileName = $workerPath
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true
$startInfo.RedirectStandardInput = $true
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
$utf8 = [System.Text.UTF8Encoding]::new($false)
$startInfo.StandardInputEncoding = $utf8
$startInfo.StandardOutputEncoding = $utf8
$startInfo.StandardErrorEncoding = $utf8
$process = [System.Diagnostics.Process]::new()
$process.StartInfo = $startInfo
if (-not $process.Start()) {
    throw 'Could not start the translation worker.'
}

function Send-Command {
    param([hashtable]$Value)
    $json = $Value | ConvertTo-Json -Compress -Depth 8
    $process.StandardInput.WriteLine($json)
    $process.StandardInput.Flush()
}

function Read-Event {
    while ($true) {
        $line = $process.StandardOutput.ReadLine()
        if ($null -eq $line) {
            $diagnostics = $process.StandardError.ReadToEnd()
            throw "Translation worker exited unexpectedly. $diagnostics"
        }
        if (-not [string]::IsNullOrWhiteSpace($line)) {
            return $line | ConvertFrom-Json
        }
    }
}

try {
    Send-Command @{
        type = 'initialize'
        payload = @{
            selected_tier = $Tier
            keep_warm = $true
            history_enabled = $false
        }
    }
    $ready = Read-Event
    if ($ready.type -ne 'ready' -or $ready.payload.selected_tier -ne $Tier) {
        throw "The selected '$Tier' translation pack must be installed before benchmarking."
    }
    while ($true) {
        $event = Read-Event
        if ($event.type -eq 'model_ready') {
            break
        }
        if ($event.type -eq 'failed') {
            throw "$($event.payload.code): $($event.payload.message)"
        }
    }

    $samples = [System.Collections.Generic.List[long]]::new()
    $texts = @(
        'Привет! Это проверка скорости локального перевода.',
        'The application translates this sentence entirely on the local computer.'
    )
    for ($index = 0; $index -lt $Iterations; $index++) {
        $requestId = [guid]::NewGuid().ToString()
        $text = $texts[$index % $texts.Count]
        $timer = [System.Diagnostics.Stopwatch]::StartNew()
        Send-Command @{
            type = 'translate'
            payload = @{
                request_id = $requestId
                text = $text
                source_language = $null
                target_language = $null
            }
        }
        while ($true) {
            $event = Read-Event
            if ($event.type -eq 'completed' -and
                $event.payload.result.request_id -eq $requestId) {
                $timer.Stop()
                $samples.Add($timer.ElapsedMilliseconds)
                $line = (
                    "iteration={0} wall_ms={1} inference_ms={2} first_token_ms={3} " +
                    "tokenize_ms={4} chunks={5} requests={6} direction={7}->{8}"
                )
                Write-Host ($line -f
                    ($index + 1),
                    $timer.ElapsedMilliseconds,
                    $event.payload.result.processing_duration_ms,
                    $event.payload.result.first_token_duration_ms,
                    $event.payload.result.tokenize_duration_ms,
                    $event.payload.result.chunk_count,
                    $event.payload.result.backend_request_count,
                    $event.payload.result.source_language,
                    $event.payload.result.target_language
                )
                break
            }
            if ($event.type -eq 'failed') {
                throw "$($event.payload.code): $($event.payload.message)"
            }
        }
    }

    $ordered = $samples | Sort-Object
    $p50 = $ordered[[Math]::Max(0, [Math]::Ceiling($ordered.Count * 0.50) - 1)]
    $p95 = $ordered[[Math]::Max(0, [Math]::Ceiling($ordered.Count * 0.95) - 1)]
    Write-Host "translation benchmark: iterations=$Iterations p50_ms=$p50 p95_ms=$p95"
}
finally {
    if (-not $process.HasExited) {
        Send-Command @{ type = 'shutdown' }
        $process.StandardInput.Close()
        if (-not $process.WaitForExit(3000)) {
            $process.Kill()
        }
    }
    $process.Dispose()
}
