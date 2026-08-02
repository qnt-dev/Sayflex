[CmdletBinding()]
param(
    [int]$IdleSeconds = 0,
    [int]$ListeningSeconds = 12,
    [string]$Executable
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
    if ($IdleSeconds -gt 0) {
        Start-Sleep -Seconds $IdleSeconds
    }
    $sessionId = [guid]::NewGuid().ToString()
    Send-CompanionCommand @{
        type = 'activate'
        payload = @{
            session_id = $sessionId
            target_window_handle = 0
        }
    }

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

    Send-CompanionCommand @{
        type = 'processing'
        payload = @{ session_id = $sessionId }
    }
    Start-Sleep -Seconds 2
    Send-CompanionCommand @{
        type = 'complete'
        payload = @{ session_id = $sessionId }
    }
    Start-Sleep -Seconds 1
}
finally {
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
