[CmdletBinding()]
param(
    [string]$AppDirectory = (Join-Path $env:LOCALAPPDATA 'Programs\SayFlex'),
    [string]$StagedDirectory,
    [string]$InstallerPath,
    [string]$EvidencePath,
    [string]$DelayedImageReceiptPath,
    [string]$ReceiptPath,
    [switch]$RequireComplete,
    [switch]$RequireSignature
)

$ErrorActionPreference = 'Stop'
$validationModule = Join-Path $PSScriptRoot 'release-validation.psm1'
Import-Module $validationModule -Force
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ([string]::IsNullOrWhiteSpace($StagedDirectory)) {
    $StagedDirectory = Join-Path $rustRoot 'dist\app'
}
if ([string]::IsNullOrWhiteSpace($ReceiptPath)) {
    $ReceiptPath = Join-Path $rustRoot 'dist\smoke-receipt.json'
}
$installerHash = if (-not [string]::IsNullOrWhiteSpace($InstallerPath) -and
    (Test-Path -LiteralPath $InstallerPath -PathType Leaf)) {
    (Get-FileHash -LiteralPath $InstallerPath -Algorithm SHA256).Hash
} else {
    $null
}

$checks = [ordered]@{}
function Set-SmokeResult([string]$Id, [string]$Status, [string]$Detail) {
    if ($Status -notin @('PASS', 'FAIL', 'BLOCKED', 'NOT_RUN')) {
        throw "Unsupported smoke status '$Status' for $Id."
    }
    $script:checks[$Id] = [ordered]@{ status = $Status; detail = $Detail }
}

try {
    $arguments = @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass',
        '-File', (Join-Path $PSScriptRoot 'validate-release.ps1'),
        '-AppDirectory', $StagedDirectory,
        '-Installed'
    )
    if (-not [string]::IsNullOrWhiteSpace($InstallerPath)) {
        $arguments += @('-InstallerPath', $InstallerPath)
    }
    if ($RequireSignature) { $arguments += '-RequireSignature' }
    $validation = & powershell.exe @arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw ($validation -join [Environment]::NewLine)
    }
    Set-SmokeResult 'artifact_identity' 'PASS' 'Installed files, versions, manifest, protocol and registration match staging.'
} catch {
    Set-SmokeResult 'artifact_identity' 'FAIL' $_.Exception.Message
}

$interactive = @(Get-SayFlexInteractiveSmokeCheckIds)
foreach ($id in $interactive) {
    Set-SmokeResult $id 'NOT_RUN' 'No witnessed evidence was supplied for this installed artifact.'
}
$automated = @(Get-SayFlexAutomatedSmokeCheckIds)
foreach ($id in $automated) {
    Set-SmokeResult $id 'NOT_RUN' 'No delayed-image receipt was supplied for this installed artifact.'
}

$manifestPath = Join-Path $StagedDirectory 'build-manifest.json'
$manifest = if (Test-Path -LiteralPath $manifestPath) {
    Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
} else {
    $null
}
$evidence = $null
if (-not [string]::IsNullOrWhiteSpace($EvidencePath)) {
    if ($null -eq $manifest) {
        throw 'Witnessed evidence cannot be bound because build-manifest.json is missing.'
    }
    if ([string]::IsNullOrWhiteSpace($InstallerPath) -or
        -not (Test-Path -LiteralPath $InstallerPath -PathType Leaf)) {
        throw 'Witnessed evidence requires the exact installer path.'
    }
    $evidence = Get-Content -LiteralPath $EvidencePath -Raw | ConvertFrom-Json
    $evidenceFailures = @(Get-SmokeEvidenceFailures `
        -Evidence $evidence `
        -ExpectedVersion "$($manifest.version)" `
        -ExpectedCommit "$($manifest.git_commit)" `
        -ExpectedInstallerSha256 $installerHash `
        -AllowedChecks $interactive)
    if ($evidenceFailures.Count -gt 0) {
        throw "Witnessed smoke evidence is invalid: $($evidenceFailures -join '; ')"
    }
    foreach ($property in $evidence.checks.PSObject.Properties) {
        Set-SmokeResult `
            $property.Name `
            "$($property.Value.status)".ToUpperInvariant() `
            "$($property.Value.detail)"
    }
}

if (-not [string]::IsNullOrWhiteSpace($DelayedImageReceiptPath)) {
    try {
        $resolvedDelayedReceipt = [System.IO.Path]::GetFullPath($DelayedImageReceiptPath)
        if (-not (Test-Path -LiteralPath $resolvedDelayedReceipt -PathType Leaf)) {
            throw "Delayed-image receipt was not found: $resolvedDelayedReceipt"
        }
        $delayedReceipt = Get-Content -LiteralPath $resolvedDelayedReceipt -Raw | ConvertFrom-Json
        $delayedFailures = @(Get-DelayedClipboardImageReceiptFailures `
            -Receipt $delayedReceipt `
            -ExpectedDelays @(0, 1, 10, 60) `
            -ExpectedConsumers @('FixtureConsumer'))
        $installedInputHost = Join-Path $AppDirectory 'SayFlex.InputHost.exe'
        if (-not (Test-Path -LiteralPath $installedInputHost -PathType Leaf)) {
            $delayedFailures += "installed input host was not found: $installedInputHost"
        } else {
            $installedInputHostHash = (Get-FileHash -LiteralPath $installedInputHost -Algorithm SHA256).Hash
            if (-not $installedInputHostHash.Equals(
                    "$($delayedReceipt.input_host_sha256)",
                    [StringComparison]::OrdinalIgnoreCase)) {
                $delayedFailures += 'delayed-image receipt belongs to a different InputHost binary'
            }
        }
        if ($delayedFailures.Count -gt 0) {
            throw ($delayedFailures -join '; ')
        }
        Set-SmokeResult `
            'clipboard_delayed_image_fixture' `
            'PASS' `
            'Installed InputHost restored the exact image after 0, 1, 10 and 60 seconds without an unresponsive consumer.'
    } catch {
        Set-SmokeResult 'clipboard_delayed_image_fixture' 'FAIL' $_.Exception.Message
    }
}

$receipt = [ordered]@{
    schema_version = 1
    product = 'SayFlex'
    version = if ($manifest) { $manifest.version } else { $null }
    git_commit = if ($manifest) { $manifest.git_commit } else { $null }
    installed_directory = [System.IO.Path]::GetFullPath($AppDirectory)
    staged_directory = [System.IO.Path]::GetFullPath($StagedDirectory)
    created_utc = [DateTime]::UtcNow.ToString('o')
    witnessed_utc = if ($evidence) { $evidence.witnessed_utc } else { $null }
    installer_sha256 = $installerHash
    delayed_image_receipt_path = if ($DelayedImageReceiptPath) {
        [System.IO.Path]::GetFullPath($DelayedImageReceiptPath)
    } else { $null }
    checks = $checks
}

$receiptDirectory = Split-Path -Parent ([System.IO.Path]::GetFullPath($ReceiptPath))
New-Item -ItemType Directory -Path $receiptDirectory -Force | Out-Null
$receipt | ConvertTo-Json -Depth 6 |
    Set-Content -LiteralPath $ReceiptPath -Encoding utf8

$failed = @($checks.GetEnumerator() | Where-Object { $_.Value.status -eq 'FAIL' })
$incomplete = @($checks.GetEnumerator() | Where-Object { $_.Value.status -ne 'PASS' })
Write-Host "Smoke receipt: $([System.IO.Path]::GetFullPath($ReceiptPath))"
Write-Host "PASS=$($checks.Count - $incomplete.Count) FAIL=$($failed.Count) NOT_COMPLETE=$($incomplete.Count)"
if ($failed.Count -gt 0) {
    throw "Installed smoke recorded $($failed.Count) failure(s)."
}
if ($RequireComplete -and $incomplete.Count -gt 0) {
    throw "Installed smoke is incomplete: $($incomplete.Count) check(s) are not PASS."
}
