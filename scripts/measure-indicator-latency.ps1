[CmdletBinding()]
param(
    [int]$TimeoutMilliseconds = 2000,
    [int]$HoldMilliseconds = 0
)

$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class KeyboopVisibleWindowCounter
{
    private delegate bool EnumWindowsProc(IntPtr window, IntPtr parameter);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(
        EnumWindowsProc callback,
        IntPtr parameter);

    [DllImport("user32.dll")]
    private static extern bool IsWindowVisible(IntPtr window);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(
        IntPtr window,
        out uint processId);

    public static int Count(uint targetProcessId)
    {
        var count = 0;
        EnumWindows(
            (window, _) =>
            {
                GetWindowThreadProcessId(window, out var processId);
                if (processId == targetProcessId && IsWindowVisible(window))
                {
                    count++;
                }

                return true;
            },
            IntPtr.Zero);
        return count;
    }
}
'@

function Read-Frame {
    param([IO.Stream]$Stream)

    $header = [byte[]]::new(4)
    $offset = 0
    while ($offset -lt $header.Length) {
        $read = $Stream.Read($header, $offset, $header.Length - $offset)
        if ($read -le 0) {
            throw 'Pipe closed before the frame header completed.'
        }
        $offset += $read
    }

    $length = [BitConverter]::ToInt32($header, 0)
    $body = [byte[]]::new($length)
    $offset = 0
    while ($offset -lt $body.Length) {
        $read = $Stream.Read($body, $offset, $body.Length - $offset)
        if ($read -le 0) {
            throw 'Pipe closed before the frame body completed.'
        }
        $offset += $read
    }

    [Text.Encoding]::UTF8.GetString($body) | ConvertFrom-Json
}

function Write-Frame {
    param(
        [IO.Stream]$Stream,
        [string]$Json
    )

    $body = [Text.Encoding]::UTF8.GetBytes($Json)
    $header = [BitConverter]::GetBytes([int]$body.Length)
    $Stream.Write($header, 0, $header.Length)
    $Stream.Write($body, 0, $body.Length)
    $Stream.Flush()
}

function Invoke-SpeechCommand {
    param(
        [string]$PipeName,
        [string]$Kind,
        [hashtable]$Payload
    )

    $pipe = [IO.Pipes.NamedPipeClientStream]::new(
        '.',
        $PipeName,
        [IO.Pipes.PipeDirection]::InOut,
        [IO.Pipes.PipeOptions]::Asynchronous)
    try {
        $pipe.Connect(2000)
        $null = Read-Frame -Stream $pipe
        $request = @{
            version = 2
            kind = $Kind
            requestId = [guid]::NewGuid().ToString()
            payload = $Payload
        } | ConvertTo-Json -Compress -Depth 4
        Write-Frame -Stream $pipe -Json $request
        return Read-Frame -Stream $pipe
    }
    finally {
        $pipe.Dispose()
    }
}

$uiProcess = Get-Process Keyboop.UI -ErrorAction Stop |
    Where-Object { $_.Path -like '*\Keyboop.UI.exe' } |
    Select-Object -First 1
if (-not $uiProcess) {
    throw 'Keyboop.UI is not running.'
}

$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$sha256 = [Security.Cryptography.SHA256]::Create()
try {
    $hash = $sha256.ComputeHash([Text.Encoding]::UTF8.GetBytes($sid))
}
finally {
    $sha256.Dispose()
}
$suffix = -join ($hash[0..7] | ForEach-Object { $_.ToString('x2') })
$pipeName = "keyboop-speech-$suffix"

$baseline = [KeyboopVisibleWindowCounter]::Count([uint32]$uiProcess.Id)
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$began = $false
$sessionId = [guid]::NewGuid()
try {
    $response = Invoke-SpeechCommand `
        -PipeName $pipeName `
        -Kind 'BeginDictation' `
        -Payload @{
            sessionId = $sessionId.ToString()
            startedAt = [DateTimeOffset]::UtcNow.ToString('O')
        }
    $began = $response.kind -eq 'Acknowledgement'
    $visibleCount = $baseline
    while ($stopwatch.ElapsedMilliseconds -lt $TimeoutMilliseconds) {
        $visibleCount = [KeyboopVisibleWindowCounter]::Count(
            [uint32]$uiProcess.Id)
        if ($visibleCount -gt $baseline) {
            break
        }

        Start-Sleep -Milliseconds 2
    }

    $result = [pscustomobject]@{
        IndicatorAppeared = $visibleCount -gt $baseline
        VisibleAfterMilliseconds = $stopwatch.ElapsedMilliseconds
        BaselineVisibleWindows = $baseline
        ListeningVisibleWindows = $visibleCount
        UiProcessId = $uiProcess.Id
    }
    if ($HoldMilliseconds -gt 0) {
        Start-Sleep -Milliseconds $HoldMilliseconds
    }

    $result
}
finally {
    if ($began) {
        $null = Invoke-SpeechCommand `
            -PipeName $pipeName `
            -Kind 'EndDictation' `
            -Payload @{
                sessionId = $sessionId.ToString()
            }

        $hideDeadline = [DateTime]::UtcNow.AddSeconds(10)
        do {
            $visibleAfterEnd = [KeyboopVisibleWindowCounter]::Count(
                [uint32]$uiProcess.Id)
            if ($visibleAfterEnd -le $baseline) {
                break
            }

            Start-Sleep -Milliseconds 10
        } while ([DateTime]::UtcNow -lt $hideDeadline)

        if ($visibleAfterEnd -gt $baseline) {
            Write-Warning 'The dictation indicator did not return to its baseline state.'
        }
    }
}
