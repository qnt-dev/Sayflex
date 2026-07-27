Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing -TypeDefinition @'
using System;
using System.Windows.Forms;

public sealed class ShortcutSelectionFixture : Form
{
    private const int WmCopy = 0x0301;
    private readonly ShortcutOnlyTextBox editor = new ShortcutOnlyTextBox();

    public ShortcutSelectionFixture()
    {
        Text = "Qntflex Shortcut Selection Fixture";
        StartPosition = FormStartPosition.Manual;
        Location = new System.Drawing.Point(80, 360);
        Size = new System.Drawing.Size(640, 240);
        KeyPreview = true;

        editor.Multiline = true;
        editor.Dock = DockStyle.Fill;
        editor.Font = new System.Drawing.Font("Segoe UI", 14);
        editor.Text = "QNTFLEX_SHORTCUT_SELECTION_TEXT";
        Controls.Add(editor);

        Shown += (_, __) => SelectFixtureText();
        Activated += (_, __) => SelectFixtureText();
        KeyDown += OnKeyDown;
    }

    private void SelectFixtureText()
    {
        editor.Focus();
        editor.SelectAll();
    }

    private void OnKeyDown(object sender, KeyEventArgs args)
    {
        if (args.Control && !args.Alt && !args.Shift && args.KeyCode == Keys.C)
        {
            Clipboard.SetText(editor.SelectedText);
            args.Handled = true;
            args.SuppressKeyPress = true;
        }
    }

    private sealed class ShortcutOnlyTextBox : TextBox
    {
        protected override void WndProc(ref Message message)
        {
            if (message.Msg == WmCopy)
            {
                return;
            }
            base.WndProc(ref message);
        }
    }
}
'@

$form = [ShortcutSelectionFixture]::new()
$form.Add_Shown({
    if ($env:QNTFLEX_RUN_SELECTION_SMOKE -ne '1') {
        return
    }

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
    })
    $script:selectionFocusTimer.Start()

    $rustRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
    $hostPath = Join-Path $rustRoot 'target\debug\rust-input-host.exe'
    $outputPath = Join-Path $rustRoot 'target\shortcut-smoke.out'
    $errorPath = Join-Path $rustRoot 'target\shortcut-smoke.err'
    $handle = $form.Handle.ToInt64()
    $command = "set QNTFLEX_SELECTION_SMOKE_HWND=$handle&& `"$hostPath`" --selection-capture-smoke > `"$outputPath`" 2> `"$errorPath`""
    Start-Process -FilePath 'cmd.exe' -ArgumentList '/d', '/c', $command -WindowStyle Hidden
})

[System.Windows.Forms.Application]::Run($form)
