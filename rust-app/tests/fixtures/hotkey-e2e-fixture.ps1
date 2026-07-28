[CmdletBinding()]
param(
    [ValidateSet('translation', 'insert-last')]
    [string]$Action,
    [Parameter(Mandatory)]
    [string]$InputHost,
    [string]$Text = 'Hello world.',
    [string]$ResultPath = ''
)

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
$form.Text = "Qntflex Hotkey E2E - $Action"
$form.StartPosition = 'Manual'
$form.Location = [System.Drawing.Point]::new(80, 80)
$form.Size = [System.Drawing.Size]::new(720, 240)

$editor = [System.Windows.Forms.TextBox]::new()
$editor.Multiline = $true
$editor.Dock = [System.Windows.Forms.DockStyle]::Fill
$editor.Font = [System.Drawing.Font]::new('Segoe UI', 16)
$editor.Text = if ($Action -eq 'translation') { $Text } else { '' }
$form.Controls.Add($editor)

$form.Add_Shown({
    $editor.Focus()
    if ($Action -eq 'translation') {
        $editor.SelectAll()
    }
    $form.TopMost = $true
    [FixtureForeground]::Focus($form.Handle)

    $script:started = [DateTime]::UtcNow
    $script:sent = $false
    $script:timer = [System.Windows.Forms.Timer]::new()
    $script:timer.Interval = 25
    $script:timer.Add_Tick({
        if ([DateTime]::UtcNow -lt $script:started.AddSeconds(2)) {
            [FixtureForeground]::Focus($form.Handle)
            $form.Activate()
            $editor.Focus()
            if ($Action -eq 'translation') {
                $editor.SelectAll()
            }
        }
        if (-not $script:sent -and [DateTime]::UtcNow -ge $script:started.AddMilliseconds(350)) {
            $script:sent = $true
            $env:QNTFLEX_RUN_HOTKEY_E2E = '1'
            Start-Process -FilePath $InputHost `
                -ArgumentList "--hotkey-e2e-send=$Action" `
                -WindowStyle Hidden
        }
        if (
            $Action -eq 'insert-last' -and
            -not [string]::IsNullOrEmpty($editor.Text)
        ) {
            $script:timer.Stop()
            if (-not [string]::IsNullOrEmpty($ResultPath)) {
                [System.IO.File]::WriteAllText($ResultPath, $editor.Text)
            }
            $form.Close()
            return
        }
        if ([DateTime]::UtcNow -ge $script:started.AddSeconds(15)) {
            $script:timer.Stop()
            if (-not [string]::IsNullOrEmpty($ResultPath)) {
                [System.IO.File]::WriteAllText($ResultPath, $editor.Text)
            }
            $form.Close()
        }
    })
    $script:timer.Start()
})

[System.Windows.Forms.Application]::Run($form)
