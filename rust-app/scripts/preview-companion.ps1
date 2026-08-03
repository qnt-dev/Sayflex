[CmdletBinding()]
param(
    [int]$IdleSeconds = 0,
    [int]$ListeningSeconds = 12,
    [string]$Executable,
    [UInt64]$TargetWindowHandle = 0,
    [switch]$LiveMicrophone,
    [string]$SpeechExecutable,
    [switch]$RecordingOnly
)

$ErrorActionPreference = 'Stop'
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ([string]::IsNullOrWhiteSpace($Executable)) {
    $Executable = Join-Path $rustRoot 'target\debug\sayflex-companion-renderer.exe'
}
$Executable = [System.IO.Path]::GetFullPath($Executable)
if (-not (Test-Path -LiteralPath $Executable)) {
    throw "Companion renderer is missing: $Executable"
}

$info = [System.Diagnostics.ProcessStartInfo]::new()
$info.FileName = $Executable
$info.UseShellExecute = $false
$info.RedirectStandardInput = $true
$info.RedirectStandardOutput = $true
$info.RedirectStandardError = $true
$info.CreateNoWindow = $true
$process = [System.Diagnostics.Process]::new()
$process.StartInfo = $info
$speechProcess = $null
$sessionId = $null

function Send-CompanionCommand {
    param([hashtable]$Command)
    $json = $Command | ConvertTo-Json -Depth 8 -Compress
    $process.StandardInput.WriteLine($json)
    $process.StandardInput.Flush()
}

try {
    if (-not $process.Start()) {
        throw 'The Companion Orb renderer did not start.'
    }
    Send-CompanionCommand @{
        type = 'set_enabled'
        payload = @{ enabled = -not $RecordingOnly }
    }
    if ($IdleSeconds -gt 0) {
        Start-Sleep -Seconds $IdleSeconds
    }
    $sessionId = [guid]::NewGuid().ToString()
    Send-CompanionCommand @{
        type = 'activate'
        payload = @{
            session_id = $sessionId
            target_window_handle = $TargetWindowHandle
        }
    }

    if ($LiveMicrophone) {
        $forwardedAudioLevels = 0
        if ([string]::IsNullOrWhiteSpace($SpeechExecutable)) {
            $speechCandidates = @(
                (Join-Path $rustRoot 'target\publish-cpu\release\sayflex-speech-worker.exe'),
                (Join-Path $rustRoot 'target\debug\sayflex-speech-worker.exe')
            )
            $SpeechExecutable = $speechCandidates | Where-Object {
                Test-Path -LiteralPath $_
            } | Select-Object -First 1
        }
        if ([string]::IsNullOrWhiteSpace($SpeechExecutable) -or
            -not (Test-Path -LiteralPath $SpeechExecutable)) {
            throw "Live microphone mode requires sayflex-speech-worker.exe. Pass -SpeechExecutable explicitly."
        }
        $SpeechExecutable = [System.IO.Path]::GetFullPath($SpeechExecutable)

        $speechInfo = [System.Diagnostics.ProcessStartInfo]::new()
        $speechInfo.FileName = $SpeechExecutable
        $speechInfo.UseShellExecute = $false
        $speechInfo.RedirectStandardInput = $true
        $speechInfo.RedirectStandardOutput = $true
        $speechInfo.RedirectStandardError = $true
        $speechInfo.CreateNoWindow = $true
        $speechProcess = [System.Diagnostics.Process]::new()
        $speechProcess.StartInfo = $speechInfo
        if (-not $speechProcess.Start()) {
            throw 'The speech worker did not start.'
        }

        $speechProcess.StandardInput.WriteLine((@{
            type = 'set_fast_release_previews'
            payload = @{ enabled = $false }
        } | ConvertTo-Json -Depth 8 -Compress))
        $speechProcess.StandardInput.WriteLine((@{
            type = 'begin'
            payload = @{
                session_id = $sessionId
                started_unix_ms = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
            }
        } | ConvertTo-Json -Depth 8 -Compress))
        $speechProcess.StandardInput.Flush()

        # Forward AudioLevel events on this PowerShell thread. Using
        # DataReceivedEventHandler here would invoke a script block on a .NET
        # thread-pool thread without a PowerShell runspace. ReadLineAsync is
        # polled instead, so the pipe is drained without blocking the timer.
        $liveDeadline = (Get-Date).AddSeconds([Math]::Max(1, $ListeningSeconds))
        $speechReadTask = $speechProcess.StandardOutput.ReadLineAsync()
        while ((Get-Date) -lt $liveDeadline) {
            while ($speechReadTask.IsCompleted) {
                $line = $speechReadTask.Result
                if ([string]::IsNullOrWhiteSpace($line)) {
                    if ($speechProcess.HasExited) {
                        break
                    }
                    $speechReadTask = $speechProcess.StandardOutput.ReadLineAsync()
                    continue
                }
                try {
                    $speechEvent = $line | ConvertFrom-Json -Depth 8
                    if ($speechEvent.type -eq 'audio_level' -and
                        [string]$speechEvent.payload.session_id -eq $sessionId) {
                        Send-CompanionCommand @{
                            type = 'audio_level'
                            payload = @{
                                session_id = $sessionId
                                peak = [double]$speechEvent.payload.peak
                                spectrum = @($speechEvent.payload.spectrum | ForEach-Object {
                                    [double]$_
                                })
                            }
                        }
                        $forwardedAudioLevels++
                        if (($forwardedAudioLevels % 10) -eq 0) {
                            Write-Verbose "Forwarded live AudioLevel events: $forwardedAudioLevels"
                        }
                    }
                }
                catch {
                    Write-Verbose "Ignoring malformed speech-worker event: $($_.Exception.Message)"
                }
                if ($speechProcess.HasExited) {
                    break
                }
                $speechReadTask = $speechProcess.StandardOutput.ReadLineAsync()
            }
            Start-Sleep -Milliseconds 10
        }
    }
    else {
        $frames = [Math]::Max(1, $ListeningSeconds * 20)
        for ($index = 0; $index -lt $frames; $index++) {
            $level = 0.10 + 0.76 * [Math]::Abs([Math]::Sin($index * 0.16))
            Send-CompanionCommand @{
                type = 'audio_level'
                payload = @{
                    session_id = $sessionId
                    peak = $level
                    spectrum = @(0.10, 0.34, 0.82, 0.58, 0.22)
                }
            }
            Start-Sleep -Milliseconds 50
        }
    }

    if ($LiveMicrophone -and $null -ne $speechProcess -and -not $speechProcess.HasExited) {
        $speechProcess.StandardInput.WriteLine((@{
            type = 'cancel'
            payload = @{ session_id = $sessionId }
        } | ConvertTo-Json -Depth 8 -Compress))
        $speechProcess.StandardInput.Flush()
    }
    Send-CompanionCommand @{
        type = 'processing'
        payload = @{ session_id = $sessionId }
    }
    Start-Sleep -Milliseconds 650
    Send-CompanionCommand @{
        type = 'dismiss'
        payload = @{ session_id = $sessionId }
    }
    Start-Sleep -Milliseconds 350
}
finally {
    if ($null -ne $speechProcess) {
        try {
            if (-not $speechProcess.HasExited) {
                $speechProcess.StandardInput.WriteLine((@{
                    type = 'cancel'
                    payload = @{ session_id = $sessionId }
                } | ConvertTo-Json -Depth 8 -Compress))
                $speechProcess.StandardInput.WriteLine((@{
                    type = 'shutdown'
                } | ConvertTo-Json -Depth 8 -Compress))
                $speechProcess.StandardInput.Flush()
                $null = $speechProcess.WaitForExit(2000)
            }
        }
        catch {}
        if (-not $speechProcess.HasExited) {
            $speechProcess.Kill()
        }
        $speechError = $speechProcess.StandardError.ReadToEnd()
        if (-not [string]::IsNullOrWhiteSpace($speechError)) {
            Write-Warning $speechError
        }
        $speechProcess.Dispose()
    }
    if (-not $process.HasExited) {
        try {
            Send-CompanionCommand @{ type = 'shutdown' }
            $null = $process.WaitForExit(2000)
        }
        catch {}
    }
    if (-not $process.HasExited) {
        $process.Kill()
    }
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    if (-not [string]::IsNullOrWhiteSpace($stdout)) {
        Write-Verbose $stdout
    }
    if (-not [string]::IsNullOrWhiteSpace($stderr)) {
        Write-Warning $stderr
    }
    $process.Dispose()
}
