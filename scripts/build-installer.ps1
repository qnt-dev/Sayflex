[CmdletBinding()]
param(
    [string]$Version = '0.1.0',
    [switch]$SkipPublish
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $PSScriptRoot '..'))

if (-not $SkipPublish) {
    & (Join-Path $PSScriptRoot 'publish.ps1') `
        -Configuration Release `
        -Version $Version
    if ($LASTEXITCODE -ne 0) {
        throw 'Publishing failed.'
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

& $compiler "/DAppVersion=$Version" `
    (Join-Path $repositoryRoot 'installer\Keyboop.iss')
if ($LASTEXITCODE -ne 0) {
    throw 'Inno Setup compilation failed.'
}

Write-Host "Installer created in $(Join-Path $repositoryRoot 'dist')"
