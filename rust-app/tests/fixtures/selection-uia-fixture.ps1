Add-Type -AssemblyName PresentationCore
Add-Type -AssemblyName PresentationFramework
Add-Type -AssemblyName WindowsBase
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
        finally
        {
            if (attached) AttachThreadInput(current, foreground, false);
        }
    }
}
'@

$window = [System.Windows.Window]::new()
$window.Title = 'Qntflex UIA Selection Fixture'
$window.WindowStartupLocation = [System.Windows.WindowStartupLocation]::Manual
$window.Left = 760
$window.Top = 80
$window.Width = 640
$window.Height = 240

$editor = [System.Windows.Controls.TextBox]::new()
$editor.AcceptsReturn = $true
$editor.FontFamily = [System.Windows.Media.FontFamily]::new('Segoe UI')
$editor.FontSize = 20
$editor.Text = 'QNTFLEX_UIA_SELECTION_TEXT'
$window.Content = $editor

$window.Add_Loaded({
    $editor.Focus()
    $editor.SelectAll()

    if ($env:QNTFLEX_RUN_SELECTION_SMOKE -ne '1') {
        return
    }

    if ($env:QNTFLEX_SELECTION_SMOKE_CLIPBOARD_MARKER) {
        [System.Windows.Clipboard]::SetText(
            $env:QNTFLEX_SELECTION_SMOKE_CLIPBOARD_MARKER
        )
    }
    $window.Topmost = $true
    $rustRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
    $hostPath = if ($env:QNTFLEX_SELECTION_SMOKE_HOST_PATH) {
        $env:QNTFLEX_SELECTION_SMOKE_HOST_PATH
    } else {
        Join-Path $rustRoot 'target\debug\rust-input-host.exe'
    }
    $outputPath = Join-Path $rustRoot 'target\uia-smoke.out'
    $errorPath = Join-Path $rustRoot 'target\uia-smoke.err'
    $script:smokeOutputPath = $outputPath
    [System.IO.File]::WriteAllText($outputPath, '')
    [System.IO.File]::WriteAllText($errorPath, '')
    $handle = [System.Windows.Interop.WindowInteropHelper]::new($window).Handle.ToInt64()
    $script:fixtureHandle = $handle
    $script:smokeCommand = "set QNTFLEX_SELECTION_SMOKE_DELAY_MS=300&& set QNTFLEX_SELECTION_SMOKE_HWND=$handle&& `"$hostPath`" --selection-capture-smoke > `"$outputPath`" 2> `"$errorPath`""
    $script:smokeStarted = $true
    [FixtureForeground]::Focus([IntPtr]::new($handle))
    Start-Process -FilePath 'cmd.exe' -ArgumentList '/d', '/c', $script:smokeCommand -WindowStyle Hidden
    $script:focusUntil = [DateTime]::UtcNow.AddSeconds(5)
    $script:selectionFocusTimer = [System.Windows.Threading.DispatcherTimer]::new()
    $script:selectionFocusTimer.Interval = [TimeSpan]::FromMilliseconds(25)
    $script:selectionFocusTimer.Add_Tick({
        if (
            $script:smokeStarted -and
            (Get-Item -LiteralPath $script:smokeOutputPath).Length -gt 0
        ) {
            $script:selectionFocusTimer.Stop()
            $window.Close()
            return
        }
        if ([DateTime]::UtcNow -ge $script:focusUntil) {
            $script:selectionFocusTimer.Stop()
            $window.Topmost = $false
            $window.Close()
            return
        }
        [FixtureForeground]::Focus([IntPtr]::new($script:fixtureHandle))
        $window.Activate()
        $editor.Focus()
        $editor.SelectAll()
    })
    $script:selectionFocusTimer.Start()
})

$window.Add_Activated({
    $editor.Focus()
    $editor.SelectAll()
    if (
        $env:QNTFLEX_RUN_SELECTION_SMOKE -eq '1' -and
        $script:smokeCommand -and
        -not $script:smokeStarted
    ) {
        $script:smokeStarted = $true
        Start-Process -FilePath 'cmd.exe' -ArgumentList '/d', '/c', $script:smokeCommand -WindowStyle Hidden
    }
})

$window.ShowDialog() | Out-Null
