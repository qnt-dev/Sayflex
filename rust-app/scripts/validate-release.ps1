[CmdletBinding()]
param(
    [string]$AppDirectory
)

$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($AppDirectory)) {
    $AppDirectory = Join-Path $PSScriptRoot '..\dist\app'
}
$appRoot = [System.IO.Path]::GetFullPath($AppDirectory)
$required = @(
    'StyleFlex.exe',
    'StyleFlex.InputHost.exe',
    'StyleFlex.Speech.Cpu.exe',
    'StyleFlex.Speech.Cuda.exe',
    'StyleFlex.Translation.exe'
)

$missing = $required | Where-Object {
    -not (Test-Path -LiteralPath (Join-Path $appRoot $_))
}
if ($missing.Count -gt 0) {
    throw "Release is incomplete. Missing: $($missing -join ', ')"
}

$files = foreach ($name in $required) {
    $path = Join-Path $appRoot $name
    $item = Get-Item -LiteralPath $path
    $hash = Get-FileHash -LiteralPath $path -Algorithm SHA256
    [pscustomobject]@{
        File = $name
        Bytes = $item.Length
        SHA256 = $hash.Hash.ToLowerInvariant()
    }
}

$files | Format-Table -AutoSize
Write-Host 'Release file set and SHA-256 validation passed.'
