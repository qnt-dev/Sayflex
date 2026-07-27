Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$form = [System.Windows.Forms.Form]::new()
$form.Text = 'Qntflex Selection Fixture'
$form.StartPosition = 'Manual'
$form.Location = [System.Drawing.Point]::new(80, 80)
$form.Size = [System.Drawing.Size]::new(640, 240)

$editor = [System.Windows.Forms.TextBox]::new()
$editor.Multiline = $true
$editor.Dock = [System.Windows.Forms.DockStyle]::Fill
$editor.Font = [System.Drawing.Font]::new('Segoe UI', 14)
$editor.Text = 'QNTFLEX_SELECTION_FIXTURE_TEXT'
$form.Controls.Add($editor)

$form.Add_Shown({
    $editor.Focus()
    $editor.SelectAll()

    if ($env:QNTFLEX_RUN_SELECTION_SMOKE -eq '1') {
        if ($env:QNTFLEX_SELECTION_SMOKE_CLIPBOARD_MARKER) {
            [System.Windows.Forms.Clipboard]::SetText(
                $env:QNTFLEX_SELECTION_SMOKE_CLIPBOARD_MARKER
            )
        }
        $form.TopMost = $true
        $focusUntil = [DateTime]::UtcNow.AddMilliseconds(800)
        $script:selectionFocusTimer = [System.Windows.Forms.Timer]::new()
        $script:selectionFocusTimer.Interval = 25
        $script:selectionFocusTimer.Add_Tick({
            if ([DateTime]::UtcNow -ge $focusUntil) {
                $script:selectionFocusTimer.Stop()
                $form.TopMost = $false
                return
            }
            $form.Activate()
            $editor.Focus()
            $editor.SelectAll()
        })
        $script:selectionFocusTimer.Start()

        $rustRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
        $hostPath = Join-Path $rustRoot 'target\debug\rust-input-host.exe'
        $outputPath = Join-Path $rustRoot 'target\window-message-smoke.out'
        $errorPath = Join-Path $rustRoot 'target\window-message-smoke.err'
        $handle = $form.Handle.ToInt64()
        $command = "set QNTFLEX_SELECTION_SMOKE_HWND=$handle&& `"$hostPath`" --selection-capture-smoke > `"$outputPath`" 2> `"$errorPath`""
        Start-Process -FilePath 'cmd.exe' -ArgumentList '/d', '/c', $command -WindowStyle Hidden
    }
})

$form.Add_Activated({
    $editor.Focus()
    $editor.SelectAll()
})

[System.Windows.Forms.Application]::Run($form)
