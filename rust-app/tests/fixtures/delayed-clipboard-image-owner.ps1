[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ImagePath,
    [Parameter(Mandatory)][string]$ReadyPath,
    [Parameter(Mandatory)][string]$StopPath,
    [Parameter(Mandatory)][string]$ErrorPath
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class SayFlexUnavailableClipboardFormat
{
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint RegisterClipboardFormat(string format);
    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool OpenClipboard(IntPtr owner);
    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool CloseClipboard();
    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr SetClipboardData(uint format, IntPtr data);
    [DllImport("user32.dll")]
    private static extern bool IsClipboardFormatAvailable(uint format);

    public static void Advertise(string name)
    {
        uint format = RegisterClipboardFormat(name);
        if (format == 0) {
            throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        if (!OpenClipboard(IntPtr.Zero)) {
            throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        try {
            Marshal.GetLastWin32Error();
            SetClipboardData(format, IntPtr.Zero);
        }
        finally {
            CloseClipboard();
        }
        if (!IsClipboardFormatAvailable(format)) {
            throw new InvalidOperationException("The unavailable private clipboard format was not advertised.");
        }
    }
}
'@

$image = $null
try {
    $image = [System.Drawing.Bitmap]::FromFile([System.IO.Path]::GetFullPath($ImagePath))
    $data = [System.Windows.Forms.DataObject]::new()
    $data.SetData([System.Windows.Forms.DataFormats]::Bitmap, $false, $image)
    [System.Windows.Forms.Clipboard]::SetDataObject($data, $false, 10, 100)
    [SayFlexUnavailableClipboardFormat]::Advertise('application/x-moz-nativeimage')
    [System.IO.File]::WriteAllText($ReadyPath, [string]$PID)

    $context = [System.Windows.Forms.ApplicationContext]::new()
    $timer = [System.Windows.Forms.Timer]::new()
    $timer.Interval = 25
    $timer.Add_Tick({
        if (Test-Path -LiteralPath $StopPath -PathType Leaf) {
            $timer.Stop()
            $context.ExitThread()
        }
    })
    $timer.Start()
    [System.Windows.Forms.Application]::Run($context)
    $timer.Dispose()
    $context.Dispose()
} catch {
    [System.IO.File]::WriteAllText($ErrorPath, ($_ | Out-String))
    exit 1
} finally {
    if ($null -ne $image) { $image.Dispose() }
}
