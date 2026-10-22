[CmdletBinding()]
param(
    [string]$Version,
    [switch]$SkipPublish,
    [string]$SigningThumbprint,
    [switch]$RequireSignature,
    [string]$VulkanTarget,
    [switch]$KeepBuildArtifacts,
    [switch]$TestNvidia
)

$ErrorActionPreference = 'Stop'
if ($TestNvidia -and ($RequireSignature -or -not [string]::IsNullOrWhiteSpace($SigningThumbprint))) {
    throw 'The local test installer cannot use public-release signing options.'
}
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
        -VulkanTarget $VulkanTarget `
        -KeepBuildArtifacts:$KeepBuildArtifacts `
        -TestNvidia:$TestNvidia
    if ($LASTEXITCODE -ne 0) {
        throw 'Rust publishing failed.'
    }
}

$appRelative = if ($TestNvidia) { 'dist\test-nvidia\app' } else { 'dist\app' }
$appDirectory = Join-Path $rustRoot $appRelative
if ($TestNvidia) {
    # -SkipPublish may reuse only the exact final source snapshot and verified
    # payload, never arbitrary old executables copied into the test directory.
    & (Join-Path $PSScriptRoot 'validate-release.ps1') -AppDirectory $appDirectory -TestNvidia
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

$compilerArguments = @("/DAppVersion=$Version", "/DAppSourceDir=$appDirectory")
if ($TestNvidia) { $compilerArguments += '/DTestNvidia' }
$compilerArguments += (Join-Path $rustRoot 'installer\SayFlex.iss')
& $compiler @compilerArguments
if ($LASTEXITCODE -ne 0) {
    throw 'Inno Setup compilation failed.'
}

$installerRelative = if ($TestNvidia) {
    "dist\test-nvidia\SayFlex-$Version-nvidia-test-win-x64-setup.exe"
} else { "dist\SayFlex-$Version-win-x64-setup.exe" }
$installer = Join-Path $rustRoot $installerRelative
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
    -AppDirectory $appDirectory `
    -InstallerPath $installer `
    -RequireSignature:$RequireSignature `
    -TestNvidia:$TestNvidia
if ($LASTEXITCODE -ne 0) {
    throw 'The installer did not pass validation.'
}

Write-Host "SayFlex installer created: $installer"
