[CmdletBinding()]
param(
    [string]$Version = '0.5.19',
    [switch]$SkipPublish
)

$ErrorActionPreference = 'Stop'
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))

if (-not $SkipPublish) {
    & (Join-Path $PSScriptRoot 'publish.ps1') -Configuration Release
    if ($LASTEXITCODE -ne 0) {
        throw 'Rust publishing failed.'
    }
}

$compilerCandidates = @(
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
    'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'
)
$compiler = $compilerCandidates |
    Where-Object { Test-Path -LiteralPath $_ } |
    Select-Object -First 1
if (-not $compiler) {
    throw 'Inno Setup 6 was not found.'
}

& $compiler "/DAppVersion=$Version" (
    Join-Path $rustRoot 'installer\StyleFlex.iss'
)
if ($LASTEXITCODE -ne 0) {
    throw 'Inno Setup compilation failed.'
}

$installer = Join-Path $rustRoot "dist\StyleFlex-$Version-win-x64-setup.exe"
$checksum = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
Set-Content -LiteralPath "$installer.sha256" -Encoding ascii -NoNewline -Value (
    "$checksum  $([System.IO.Path]::GetFileName($installer))"
)

Write-Host "StyleFlex installer created in $(Join-Path $rustRoot 'dist')"
