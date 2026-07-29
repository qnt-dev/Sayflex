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

Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing -TypeDefinition @'
using System;
using System.Windows.Forms;

public sealed class ShortcutSelectionFixture : Form
{
    private const int WmCopy = 0x0301;
    private readonly ShortcutOnlyTextBox editor = new ShortcutOnlyTextBox();

    public ShortcutSelectionFixture()
    {
        Text = "StyleFlex Shortcut Selection Fixture";
        StartPosition = FormStartPosition.Manual;
        Location = new System.Drawing.Point(80, 360);
        Size = new System.Drawing.Size(640, 240);
        KeyPreview = true;

        editor.Multiline = true;
        editor.Dock = DockStyle.Fill;
        editor.Font = new System.Drawing.Font("Segoe UI", 14);
        editor.Text = "STYLEFLEX_SHORTCUT_SELECTION_TEXT";
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
    if ($env:STYLEFLEX_RUN_SELECTION_SMOKE -ne '1') {
        return
    }

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
    })
    $script:selectionFocusTimer.Start()

    $rustRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
    $hostPath = if ($env:STYLEFLEX_SELECTION_SMOKE_HOST_PATH) {
        $env:STYLEFLEX_SELECTION_SMOKE_HOST_PATH
    } else {
        Join-Path $rustRoot 'target\debug\styleflex-input-host.exe'
    }
    $outputPath = Join-Path $rustRoot 'target\shortcut-smoke.out'
    $errorPath = Join-Path $rustRoot 'target\shortcut-smoke.err'
    $script:smokeOutputPath = $outputPath
    [System.IO.File]::WriteAllText($outputPath, '')
    [System.IO.File]::WriteAllText($errorPath, '')
    $handle = $form.Handle.ToInt64()
    $command = "set STYLEFLEX_SELECTION_SMOKE_DELAY_MS=300&& set STYLEFLEX_SELECTION_SMOKE_HWND=$handle&& `"$hostPath`" --selection-capture-smoke > `"$outputPath`" 2> `"$errorPath`""
    [FixtureForeground]::Focus($form.Handle)
    Start-Process -FilePath 'cmd.exe' -ArgumentList '/d', '/c', $command -WindowStyle Hidden
})

[System.Windows.Forms.Application]::Run($form)
