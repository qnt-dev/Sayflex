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
$form.Controls.Add($editor)

$form.Add_Shown({
    $editor.Focus()
    if ($env:SAYFLEX_RUN_INSERTION_SMOKE -ne '1') {
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
    [System.IO.File]::WriteAllText($outputPath, '')
    [System.IO.File]::WriteAllText($resultPath, '')
    $handle = $form.Handle.ToInt64()
    $text = $env:SAYFLEX_INSERTION_SMOKE_TEXT
    $command = "set SAYFLEX_RUN_INSERTION_SMOKE=1&& set SAYFLEX_INSERTION_SMOKE_HWND=$handle&& set SAYFLEX_INSERTION_SMOKE_TEXT=$text&& `"$hostPath`" --insertion-smoke > `"$outputPath`" 2> `"$errorPath`""
    [FixtureForeground]::Focus($form.Handle)
    $script:insertionProcess = Start-Process -FilePath 'cmd.exe' -ArgumentList '/d', '/c', $command -WindowStyle Hidden -PassThru

    $script:insertionTimer = [System.Windows.Forms.Timer]::new()
    $script:insertionTimer.Interval = 25
    $script:insertionTimer.Add_Tick({
        [FixtureForeground]::Focus($form.Handle)
        $form.Activate()
        $editor.Focus()
        if (
            -not (Test-Path -LiteralPath $script:insertionOutputPath) -or
            (Get-Item -LiteralPath $script:insertionOutputPath).Length -eq 0
        ) {
            return
        }
        $script:insertionTimer.Stop()
        [System.IO.File]::WriteAllText($script:insertionResultPath, $editor.Text)
        $form.Close()
    })
    $script:insertionTimer.Start()
})

[System.Windows.Forms.Application]::Run($form)
