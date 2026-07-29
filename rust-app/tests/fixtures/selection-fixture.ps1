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
$form.Text = 'StyleFlex Selection Fixture'
$form.StartPosition = 'Manual'
$form.Location = [System.Drawing.Point]::new(80, 80)
$form.Size = [System.Drawing.Size]::new(640, 240)

$editor = [System.Windows.Forms.TextBox]::new()
$editor.Multiline = $true
$editor.Dock = [System.Windows.Forms.DockStyle]::Fill
$editor.Font = [System.Drawing.Font]::new('Segoe UI', 14)
$editor.Text = 'STYLEFLEX_SELECTION_FIXTURE_TEXT'
$form.Controls.Add($editor)

$form.Add_Shown({
    $editor.Focus()
    $editor.SelectAll()

    if ($env:STYLEFLEX_RUN_SELECTION_SMOKE -eq '1') {
        if ($env:STYLEFLEX_SELECTION_SMOKE_CLIPBOARD_MARKER) {
            [System.Windows.Forms.Clipboard]::SetText(
                $env:STYLEFLEX_SELECTION_SMOKE_CLIPBOARD_MARKER
            )
        }
        $form.TopMost = $true
        $script:focusUntil = [DateTime]::UtcNow.AddSeconds(5)
        $script:selectionFocusTimer = [System.Windows.Forms.Timer]::new()
        $script:selectionFocusTimer.Interval = 25
        $script:selectionFocusTimer.Add_Tick({
            if (
                $script:smokeOutputPath -and
                (Get-Item -LiteralPath $script:smokeOutputPath).Length -gt 0
            ) {
                $script:selectionFocusTimer.Stop()
                $form.Close()
                return
            }
            if ([DateTime]::UtcNow -ge $script:focusUntil) {
                $script:selectionFocusTimer.Stop()
                $form.TopMost = $false
                $form.Close()
                return
            }
            [FixtureForeground]::Focus($form.Handle)
            $form.Activate()
            $editor.Focus()
            $editor.SelectAll()
        })
        $script:selectionFocusTimer.Start()

        $rustRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
        $hostPath = if ($env:STYLEFLEX_SELECTION_SMOKE_HOST_PATH) {
            $env:STYLEFLEX_SELECTION_SMOKE_HOST_PATH
        } else {
            Join-Path $rustRoot 'target\debug\styleflex-input-host.exe'
        }
        $outputPath = Join-Path $rustRoot 'target\window-message-smoke.out'
        $errorPath = Join-Path $rustRoot 'target\window-message-smoke.err'
        $script:smokeOutputPath = $outputPath
        [System.IO.File]::WriteAllText($outputPath, '')
        [System.IO.File]::WriteAllText($errorPath, '')
        $handle = $form.Handle.ToInt64()
        $command = "set STYLEFLEX_SELECTION_SMOKE_DELAY_MS=300&& set STYLEFLEX_SELECTION_SMOKE_HWND=$handle&& `"$hostPath`" --selection-capture-smoke > `"$outputPath`" 2> `"$errorPath`""
        [FixtureForeground]::Focus($form.Handle)
        Start-Process -FilePath 'cmd.exe' -ArgumentList '/d', '/c', $command -WindowStyle Hidden
    }
})

$form.Add_Activated({
    $editor.Focus()
    $editor.SelectAll()
})

[System.Windows.Forms.Application]::Run($form)
