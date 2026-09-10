Set-StrictMode -Version Latest

function Get-InstallerArtifactFailures {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$InstallerPath,
        [Parameter(Mandatory)]
        [string]$ExpectedVersion,
        [switch]$RequireSignature
    )

    $failures = New-Object System.Collections.Generic.List[string]
    $resolved = [System.IO.Path]::GetFullPath($InstallerPath)
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        $failures.Add("the installer named on the command line is not there: $resolved")
        return $failures
    }

    $installer = Get-Item -LiteralPath $resolved
    if ($installer.Name -notlike "*-$ExpectedVersion-*") {
        $failures.Add("$($installer.Name) is not the $ExpectedVersion installer")
    }

    $sidecar = "$resolved.sha256"
    if (-not (Test-Path -LiteralPath $sidecar -PathType Leaf)) {
        $failures.Add("$($installer.Name) has no .sha256 beside it")
    } else {
        $parts = "$(Get-Content -LiteralPath $sidecar -Raw)".Trim() -split '\s+', 2
        $recorded = $parts[0].ToLowerInvariant()
        $recordedName = if ($parts.Count -gt 1) { $parts[1].Trim() } else { '' }
        $actual = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($recorded -ne $actual) {
            $failures.Add("$($installer.Name) does not match its .sha256")
        }
        if ($recordedName -ne $installer.Name) {
            $failures.Add("$($installer.Name) sidecar names '$recordedName' instead of the exact installer")
        }
    }

    if ($RequireSignature) {
        $signature = Get-AuthenticodeSignature -LiteralPath $resolved
        if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
            $failures.Add(
                "$($installer.Name) does not carry a valid Authenticode signature (status: $($signature.Status))"
            )
        }
    }

    return $failures
}

function Get-SmokeEvidenceFailures {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [psobject]$Evidence,
        [Parameter(Mandatory)]
        [string]$ExpectedVersion,
        [Parameter(Mandatory)]
        [string]$ExpectedCommit,
        [Parameter(Mandatory)]
        [string]$ExpectedInstallerSha256,
        [Parameter(Mandatory)]
        [string[]]$AllowedChecks
    )

    $failures = New-Object System.Collections.Generic.List[string]
    $requiredFields = @(
        'schema_version', 'product', 'version', 'git_commit',
        'installer_sha256', 'witnessed_utc', 'checks'
    )
    foreach ($field in $requiredFields) {
        if ($null -eq $Evidence.PSObject.Properties[$field]) {
            $failures.Add("smoke evidence is missing '$field'")
        }
    }
    $knownFields = @($requiredFields)
    foreach ($field in $Evidence.PSObject.Properties.Name) {
        if ($knownFields -notcontains $field) {
            $failures.Add("smoke evidence contains unknown field '$field'")
        }
    }
    if ($failures.Count -gt 0) {
        return $failures
    }

    if ($Evidence.schema_version -ne 1) {
        $failures.Add("smoke evidence schema '$($Evidence.schema_version)' is unsupported")
    }
    if ($Evidence.product -ne 'SayFlex') {
        $failures.Add("smoke evidence names product '$($Evidence.product)', not SayFlex")
    }
    if ($Evidence.version -ne $ExpectedVersion) {
        $failures.Add("smoke evidence version '$($Evidence.version)' is not '$ExpectedVersion'")
    }
    if ($Evidence.git_commit -ne $ExpectedCommit) {
        $failures.Add("smoke evidence commit '$($Evidence.git_commit)' is not '$ExpectedCommit'")
    }
    if (-not "$($Evidence.installer_sha256)".Equals(
        $ExpectedInstallerSha256,
        [StringComparison]::OrdinalIgnoreCase)) {
        $failures.Add('smoke evidence installer SHA-256 does not match the exact installer')
    }
    $witnessed = [DateTimeOffset]::MinValue
    if (-not [DateTimeOffset]::TryParse("$($Evidence.witnessed_utc)", [ref]$witnessed)) {
        $failures.Add("smoke evidence witnessed_utc '$($Evidence.witnessed_utc)' is invalid")
    }

    if ($null -eq $Evidence.checks) {
        $failures.Add('smoke evidence checks must be an object')
        return $failures
    }
    foreach ($property in $Evidence.checks.PSObject.Properties) {
        if ($AllowedChecks -notcontains $property.Name) {
            $failures.Add("smoke evidence contains unknown or protected check '$($property.Name)'")
            continue
        }
        $status = "$($property.Value.status)".ToUpperInvariant()
        if ($status -notin @('PASS', 'FAIL', 'BLOCKED', 'NOT_RUN')) {
            $failures.Add("smoke evidence check '$($property.Name)' has invalid status '$status'")
        }
        if ([string]::IsNullOrWhiteSpace("$($property.Value.detail)")) {
            $failures.Add("smoke evidence check '$($property.Name)' has no detail")
        }
    }
    return $failures
}

Export-ModuleMember -Function Get-InstallerArtifactFailures, Get-SmokeEvidenceFailures
