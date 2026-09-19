[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$OutputImagePath,
    [Parameter(Mandatory)][string]$ResultPath,
    [Parameter(Mandatory)][string]$ErrorPath
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$image = $null
try {
    $image = [System.Windows.Forms.Clipboard]::GetImage()
    if ($null -eq $image) {
        throw 'The restored clipboard did not expose an image.'
    }
    $image.Save($OutputImagePath, [System.Drawing.Imaging.ImageFormat]::Bmp)
    [ordered]@{
        process_id = $PID
        sha256 = (Get-FileHash -LiteralPath $OutputImagePath -Algorithm SHA256).Hash
    } | ConvertTo-Json | Set-Content -LiteralPath $ResultPath -Encoding utf8
} catch {
    [System.IO.File]::WriteAllText($ErrorPath, ($_ | Out-String))
    exit 1
} finally {
    if ($null -ne $image) { $image.Dispose() }
}
