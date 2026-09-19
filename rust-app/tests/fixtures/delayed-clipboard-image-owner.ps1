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

$image = $null
try {
    $image = [System.Drawing.Bitmap]::FromFile([System.IO.Path]::GetFullPath($ImagePath))
    $data = [System.Windows.Forms.DataObject]::new()
    $data.SetData([System.Windows.Forms.DataFormats]::Bitmap, $false, $image)
    [System.Windows.Forms.Clipboard]::SetDataObject($data, $false, 10, 100)
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
