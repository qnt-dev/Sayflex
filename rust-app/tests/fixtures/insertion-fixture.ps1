Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class FixtureForeground
{
    [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hwnd, IntPtr processId);
    [DllImport("kernel32.dll")] private static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] private static extern bool AttachThreadInput(uint source, uint target, bool attach);
    [DllImport("user32.dll")] private static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern uint GetClipboardSequenceNumber();

    public static void Focus(IntPtr hwnd)
    {
        uint current = GetCurrentThreadId();
        uint foreground = GetWindowThreadProcessId(GetForegroundWindow(), IntPtr.Zero);
        bool attached = foreground != 0 && foreground != current &&
                        AttachThreadInput(current, foreground, true);
        try { SetForegroundWindow(hwnd); }
        finally { if (attached) AttachThreadInput(current, foreground, false); }
    }
}
'@

$form = [System.Windows.Forms.Form]::new()
$form.Text = 'SayFlex Insertion Fixture'
$form.StartPosition = 'Manual'
$form.Location = [System.Drawing.Point]::new(80, 80)
$form.Size = [System.Drawing.Size]::new(640, 240)

$editor = [System.Windows.Forms.TextBox]::new()
$editor.Multiline = $true
$editor.Dock = [System.Windows.Forms.DockStyle]::Fill
$editor.Font = [System.Drawing.Font]::new('Segoe UI', 14)
$script:inputDelayApplied = $false
$editor.Add_KeyPress({
    param($sender, $eventArgs)
    if (
        -not $script:inputDelayApplied -and
        $env:SAYFLEX_INSERTION_SMOKE_INPUT_DELAY_MS
    ) {
        $script:inputDelayApplied = $true
        Start-Sleep -Milliseconds ([int]$env:SAYFLEX_INSERTION_SMOKE_INPUT_DELAY_MS)
    }
})
$form.Controls.Add($editor)

$form.Add_Shown({
    $editor.Focus()
    if (
        $env:SAYFLEX_RUN_INSERTION_SMOKE -ne '1'
    ) {
        return
    }

    $form.TopMost = $true
    $rustRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
    $hostPath = if ($env:SAYFLEX_INSERTION_SMOKE_HOST_PATH) {
        $env:SAYFLEX_INSERTION_SMOKE_HOST_PATH
    } else {
        Join-Path $rustRoot 'target\debug\sayflex-input-host.exe'
    }
    $outputPath = Join-Path $rustRoot 'target\insertion-smoke.out'
    $errorPath = Join-Path $rustRoot 'target\insertion-smoke.err'
    $resultPath = Join-Path $rustRoot 'target\insertion-smoke.txt'
    $script:insertionOutputPath = $outputPath
    $script:insertionResultPath = $resultPath
    $script:insertionErrorPath = $errorPath
    [System.IO.File]::WriteAllText($outputPath, '')
    [System.IO.File]::WriteAllText($errorPath, '')
    [System.IO.File]::WriteAllText($resultPath, '')
    $handle = $form.Handle.ToInt64()
    $text = $env:SAYFLEX_INSERTION_SMOKE_TEXT
    $smokeArgument = '--insertion-smoke'
    $env:SAYFLEX_INSERTION_SMOKE_HWND = [string]$handle
    $env:SAYFLEX_INSERTION_SMOKE_TEXT = $text
    $env:SAYFLEX_INSERTION_SMOKE_DELAY_MS = '600'
    [FixtureForeground]::Focus($form.Handle)
    $script:insertionProcess = Start-Process `
        -FilePath $hostPath `
        -ArgumentList $smokeArgument `
        -RedirectStandardOutput $outputPath `
        -RedirectStandardError $errorPath `
        -WindowStyle Hidden `
        -PassThru
    $script:insertionDeadline = [DateTime]::UtcNow.AddSeconds(20)

    $script:insertionTimer = [System.Windows.Forms.Timer]::new()
    $script:insertionTimer.Interval = 25
    $script:insertionTimer.Add_Tick({
        if ([DateTime]::UtcNow -ge $script:insertionDeadline) {
            $script:insertionFailure = "timed out waiting for exact text; received: $($editor.Text)"
            $script:insertionTimer.Stop()
            $form.Close()
            return
        }
        if ($env:SAYFLEX_INSERTION_SMOKE_OBSERVE_CLIPBOARD -eq '1') {
            [void][System.Windows.Forms.Clipboard]::GetText()
        }
        [FixtureForeground]::Focus($form.Handle)
        $form.Activate()
        $editor.Focus()
        if (
            -not (Test-Path -LiteralPath $script:insertionOutputPath) -or
            (Get-Item -LiteralPath $script:insertionOutputPath).Length -eq 0
        ) {
            if ($script:insertionProcess.HasExited) {
                $script:insertionFailure = Get-Content -LiteralPath $script:insertionErrorPath -Raw -ErrorAction SilentlyContinue
                $script:insertionTimer.Stop()
                $form.Close()
            }
            return
        }
        if ($editor.Text -eq $env:SAYFLEX_INSERTION_SMOKE_TEXT) {
            $script:insertionTimer.Stop()
            [System.IO.File]::WriteAllText($script:insertionResultPath, $editor.Text)
            $form.Close()
        }
    })
    $script:insertionTimer.Start()
})

[System.Windows.Forms.Application]::Run($form)
if ($script:insertionFailure) {
    throw "insertion smoke failed: $script:insertionFailure"
}
