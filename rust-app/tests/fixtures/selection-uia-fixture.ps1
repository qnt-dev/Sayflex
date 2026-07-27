Add-Type -AssemblyName PresentationCore
Add-Type -AssemblyName PresentationFramework
Add-Type -AssemblyName WindowsBase

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
    $focusUntil = [DateTime]::UtcNow.AddMilliseconds(800)
    $script:selectionFocusTimer = [System.Windows.Threading.DispatcherTimer]::new()
    $script:selectionFocusTimer.Interval = [TimeSpan]::FromMilliseconds(25)
    $script:selectionFocusTimer.Add_Tick({
        if ([DateTime]::UtcNow -ge $focusUntil) {
            $script:selectionFocusTimer.Stop()
            $window.Topmost = $false
            return
        }
        $window.Activate()
        $editor.Focus()
        $editor.SelectAll()
    })
    $script:selectionFocusTimer.Start()

    $rustRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
    $hostPath = Join-Path $rustRoot 'target\debug\rust-input-host.exe'
    $outputPath = Join-Path $rustRoot 'target\uia-smoke.out'
    $errorPath = Join-Path $rustRoot 'target\uia-smoke.err'
    $handle = [System.Windows.Interop.WindowInteropHelper]::new($window).Handle.ToInt64()
    $command = "set QNTFLEX_SELECTION_SMOKE_HWND=$handle&& `"$hostPath`" --selection-capture-smoke > `"$outputPath`" 2> `"$errorPath`""
    Start-Process -FilePath 'cmd.exe' -ArgumentList '/d', '/c', $command -WindowStyle Hidden
})

$window.Add_Activated({
    $editor.Focus()
    $editor.SelectAll()
})

$window.ShowDialog() | Out-Null
