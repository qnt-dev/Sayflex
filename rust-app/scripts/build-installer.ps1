[CmdletBinding()]
param(
    [string]$Version = '0.6.0',
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
    Join-Path $rustRoot 'installer\SayFlex.iss'
)
if ($LASTEXITCODE -ne 0) {
    throw 'Inno Setup compilation failed.'
}

$installer = Join-Path $rustRoot "dist\SayFlex-$Version-win-x64-setup.exe"
$checksum = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
Set-Content -LiteralPath "$installer.sha256" -Encoding ascii -NoNewline -Value (
    "$checksum  $([System.IO.Path]::GetFileName($installer))"
)

# Checked here, after it exists and after its sidecar is written. The publish
# script's own run of the validator happens before Inno Setup has produced
# anything, so it can say nothing about this file.
& (Join-Path $PSScriptRoot 'validate-release.ps1') -InstallerPath $installer
if ($LASTEXITCODE -ne 0) {
    throw 'The installer did not pass validation.'
}

Write-Host "SayFlex installer created in $(Join-Path $rustRoot 'dist')"
