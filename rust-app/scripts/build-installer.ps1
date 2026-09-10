[CmdletBinding()]
param(
    [string]$Version,
    [switch]$SkipPublish,
    [string]$SigningThumbprint,
    [switch]$RequireSignature,
    [string]$VulkanTarget
)

$ErrorActionPreference = 'Stop'
if ($RequireSignature -and [string]::IsNullOrWhiteSpace($SigningThumbprint)) {
    throw 'A public installer requires -SigningThumbprint.'
}
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = (
        Select-String -Path (Join-Path $rustRoot 'Cargo.toml') -Pattern '^version = "(.+)"' |
            Select-Object -First 1
    ).Matches[0].Groups[1].Value
}

if (-not $SkipPublish) {
    & (Join-Path $PSScriptRoot 'publish.ps1') `
        -Configuration Release `
        -SigningThumbprint $SigningThumbprint `
        -RequireSignature:$RequireSignature `
        -VulkanTarget $VulkanTarget
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
if (-not [string]::IsNullOrWhiteSpace($SigningThumbprint)) {
    & (Join-Path $PSScriptRoot 'sign-artifacts.ps1') `
        -Path $installer `
        -CertificateThumbprint $SigningThumbprint
    if ($LASTEXITCODE -ne 0) {
        throw 'Installer signing failed.'
    }
}
$checksum = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
Set-Content -LiteralPath "$installer.sha256" -Encoding ascii -NoNewline -Value (
    "$checksum  $([System.IO.Path]::GetFileName($installer))"
)

# Checked here, after it exists and after its sidecar is written. The publish
# script's own run of the validator happens before Inno Setup has produced
# anything, so it can say nothing about this file.
& (Join-Path $PSScriptRoot 'validate-release.ps1') `
    -InstallerPath $installer `
    -RequireSignature:$RequireSignature
if ($LASTEXITCODE -ne 0) {
    throw 'The installer did not pass validation.'
}

Write-Host "SayFlex installer created in $(Join-Path $rustRoot 'dist')"
