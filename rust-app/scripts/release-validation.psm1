Set-StrictMode -Version Latest

function Get-SayFlexInteractiveSmokeCheckIds {
    @(
        'dictation_editor'
        'dictation_browser'
        'dictation_terminal'
        'clipboard_delayed_image_firefox'
        'clipboard_delayed_image_telegram'
        'translation_popup'
        'translation_orig'
        'translation_dual'
        'translation_pin_resize'
        'translation_multi_pin'
        'translation_click_away'
        'translation_quota_fallback'
        'translation_bad_key'
        'translation_no_providers'
        'translation_timeout'
        'translation_unsupported_pair'
        'translation_empty_source'
        'translation_ctrl_enter'
        'translation_copy'
        'translation_swap'
        'microphone_switch'
        'provider_fallback'
        'settings_wheel_touchpad_rail'
        'recording_visuals'
        'screen_translation_capture'
        'screen_translation_orig_dual'
        'screen_translation_on_image'
        'screen_translation_edit_save_retry'
        'screen_translation_glossary_rules'
        'screen_translation_context_fallback'
        'screen_translation_retention'
        'screen_translation_monitor_dpi'
        'screen_translation_ocr_restart'
        'resource_release_reload'
        'resource_game_budget'
        'gpu_counters_localized'
        'fresh_install'
        'upgrade_install'
    )
}

function Get-SayFlexAutomatedSmokeCheckIds {
    @('clipboard_delayed_image_fixture')
}

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

function Get-DelayedClipboardImageReceiptFailures {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [psobject]$Receipt,
        [Parameter(Mandatory)]
        [int[]]$ExpectedDelays,
        [Parameter(Mandatory)]
        [string[]]$ExpectedConsumers
    )

    $failures = New-Object System.Collections.Generic.List[string]
    foreach ($field in @(
        'schema_version',
        'product',
        'input_host_sha256',
        'witnessed_utc',
        'cases',
        'competitive_copy'
    )) {
        if ($null -eq $Receipt.PSObject.Properties[$field]) {
            $failures.Add("delayed-image receipt is missing '$field'")
        }
    }
    if ($failures.Count -gt 0) { return $failures }

    if ($Receipt.schema_version -ne 2) {
        $failures.Add("delayed-image receipt schema '$($Receipt.schema_version)' is unsupported")
    }
    if ($Receipt.product -ne 'SayFlex') {
        $failures.Add("delayed-image receipt names product '$($Receipt.product)', not SayFlex")
    }
    if ("$($Receipt.input_host_sha256)" -notmatch '^[0-9A-Fa-f]{64}$') {
        $failures.Add('delayed-image receipt has an invalid input-host SHA-256')
    }
    $witnessed = [DateTimeOffset]::MinValue
    $timestampIsValid = $Receipt.witnessed_utc -is [DateTime] -or
        $Receipt.witnessed_utc -is [DateTimeOffset] -or
        [DateTimeOffset]::TryParse(
            "$($Receipt.witnessed_utc)",
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind,
            [ref]$witnessed
        )
    if (-not $timestampIsValid) {
        $failures.Add("delayed-image receipt witnessed_utc '$($Receipt.witnessed_utc)' is invalid")
    }

    $cases = @($Receipt.cases)
    foreach ($consumer in $ExpectedConsumers) {
        if ([string]::IsNullOrWhiteSpace($consumer)) {
            $failures.Add('expected delayed-image consumer cannot be empty')
            continue
        }
        foreach ($delay in $ExpectedDelays) {
            $matching = @($cases | Where-Object {
                "$($_.consumer_process)" -ceq $consumer -and $_.delay_seconds -eq $delay
            })
            if ($matching.Count -ne 1) {
                $failures.Add(
                    "delayed-image receipt requires exactly one case for consumer '$consumer' at delay $delay seconds; found $($matching.Count)"
                )
            }
        }
    }

    foreach ($case in $cases) {
        $identity = "consumer '$($case.consumer_process)' at delay $($case.delay_seconds) seconds"
        if ([string]::IsNullOrWhiteSpace("$($case.provider_process)")) {
            $failures.Add("delayed-image case $identity has no provider process")
        }
        if ([string]::IsNullOrWhiteSpace("$($case.consumer_process)")) {
            $failures.Add("delayed-image case at delay $($case.delay_seconds) seconds has no consumer process")
        }
        $sourceHash = "$($case.source_sha256)"
        $receivedHash = "$($case.received_sha256)"
        if ($sourceHash -notmatch '^[0-9A-Fa-f]{64}$' -or
            $receivedHash -notmatch '^[0-9A-Fa-f]{64}$' -or
            -not $sourceHash.Equals($receivedHash, [StringComparison]::OrdinalIgnoreCase)) {
            $failures.Add("delayed-image case $identity has a changed image hash")
        }
        if ($case.responsive -ne $true) {
            $failures.Add("delayed-image case $identity did not remain responsive")
        }
        foreach ($exitCode in @(
            @('provider probe exit code', $case.provider_probe_exit_code),
            @('input-host exit code', $case.input_host_exit_code),
            @('consumer probe exit code', $case.consumer_probe_exit_code)
        )) {
            if ($exitCode[1] -ne 0) {
                $failures.Add("delayed-image case $identity has $($exitCode[0]) '$($exitCode[1])'")
            }
        }
        if ("$($case.status)" -cne 'PASS') {
            $failures.Add("delayed-image case $identity has status '$($case.status)', not PASS")
        }
    }

    $competitive = $Receipt.competitive_copy
    if ($null -ne $competitive) {
        if ($competitive.materialized_by_non_target -ne $true) {
            $failures.Add('competitive Copy did not materialize the SayFlex payload from a non-target process')
        }
        if ([string]::IsNullOrWhiteSpace("$($competitive.expected_text)") -or
            "$($competitive.received_text)" -cne "$($competitive.expected_text)") {
            $failures.Add('competitive Copy text was overwritten during restoration')
        }
        $sourceHash = "$($competitive.source_sha256)"
        $receivedHash = "$($competitive.received_sha256)"
        if ($sourceHash -notmatch '^[0-9A-Fa-f]{64}$' -or
            $receivedHash -notmatch '^[0-9A-Fa-f]{64}$' -or
            -not $sourceHash.Equals($receivedHash, [StringComparison]::OrdinalIgnoreCase)) {
            $failures.Add('competitive Copy image was overwritten during restoration')
        }
        if ($competitive.input_host_exit_code -eq 0) {
            $failures.Add('competitive Copy was not reported as a superseded insertion')
        }
        if ($competitive.consumer_probe_exit_code -ne 0) {
            $failures.Add("competitive Copy consumer exit code is '$($competitive.consumer_probe_exit_code)'")
        }
        if ("$($competitive.status)" -cne 'PASS') {
            $failures.Add("competitive Copy status is '$($competitive.status)', not PASS")
        }
    }
    return $failures
}

function Get-MsixLayoutFailures {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$LayoutPath,
        [Parameter(Mandatory)]
        [string]$BuildManifestPath,
        [Parameter(Mandatory)]
        [string]$ExpectedIdentityName,
        [Parameter(Mandatory)]
        [string]$ExpectedPublisher
    )

    $failures = New-Object System.Collections.Generic.List[string]
    $layoutRoot = [System.IO.Path]::GetFullPath($LayoutPath)
    $buildManifestFile = [System.IO.Path]::GetFullPath($BuildManifestPath)
    if (-not (Test-Path -LiteralPath $layoutRoot -PathType Container)) {
        $failures.Add("MSIX layout is not there: $layoutRoot")
        return $failures
    }
    if (-not (Test-Path -LiteralPath $buildManifestFile -PathType Leaf)) {
        $failures.Add("staged build manifest is not there: $buildManifestFile")
        return $failures
    }

    try {
        $build = Get-Content -LiteralPath $buildManifestFile -Raw | ConvertFrom-Json
    } catch {
        $failures.Add("staged build manifest is unreadable: $($_.Exception.Message)")
        return $failures
    }
    if ($build.schema_version -ne 1 -or $build.product -ne 'SayFlex') {
        $failures.Add('staged build manifest has an unsupported identity or schema')
        return $failures
    }

    $appxManifestPath = Join-Path $layoutRoot 'AppxManifest.xml'
    if (-not (Test-Path -LiteralPath $appxManifestPath -PathType Leaf)) {
        $failures.Add('MSIX layout has no AppxManifest.xml')
        return $failures
    }
    try {
        [xml]$appx = Get-Content -LiteralPath $appxManifestPath -Raw
        $identity = $appx.Package.Identity
    } catch {
        $failures.Add("AppxManifest.xml is unreadable: $($_.Exception.Message)")
        return $failures
    }

    if ($identity.Name -cne $ExpectedIdentityName) {
        $failures.Add("MSIX identity '$($identity.Name)' is not '$ExpectedIdentityName'")
    }
    if ($identity.Publisher -cne $ExpectedPublisher) {
        $failures.Add("MSIX publisher '$($identity.Publisher)' is not '$ExpectedPublisher'")
    }
    $expectedPackageVersion = "$($build.version).0"
    if ($identity.Version -ne $expectedPackageVersion) {
        $failures.Add("MSIX version '$($identity.Version)' is not '$expectedPackageVersion'")
    }
    if ($identity.ProcessorArchitecture -ne 'x64') {
        $failures.Add("MSIX architecture '$($identity.ProcessorArchitecture)' is not 'x64'")
    }

    $requiredPackageFiles = @(
        @($build.files | ForEach-Object { "$($_.path)".Replace('\', '/') })
        'build-manifest.json'
        'AppxManifest.xml'
        'Assets/StoreLogo.png'
        'Assets/Square44x44Logo.png'
        'Assets/Square150x150Logo.png'
    ) | Sort-Object -Unique
    $actualPackageFiles = @(
        Get-ChildItem -LiteralPath $layoutRoot -File -Recurse |
            ForEach-Object { $_.FullName.Substring($layoutRoot.Length + 1).Replace('\', '/') } |
            Sort-Object -Unique
    )
    foreach ($difference in Compare-Object $requiredPackageFiles $actualPackageFiles) {
        if ($difference.SideIndicator -eq '=>') {
            $failures.Add("MSIX layout contains unexpected file '$($difference.InputObject)'")
        } else {
            $failures.Add("MSIX layout is missing '$($difference.InputObject)'")
        }
    }

    foreach ($entry in @($build.files)) {
        $relative = "$($entry.path)".Replace('/', '\')
        $packaged = Join-Path $layoutRoot $relative
        if (-not (Test-Path -LiteralPath $packaged -PathType Leaf)) { continue }
        $item = Get-Item -LiteralPath $packaged
        $hash = (Get-FileHash -LiteralPath $packaged -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($item.Length -ne $entry.bytes -or $hash -ne "$($entry.sha256)".ToLowerInvariant()) {
            $failures.Add("$($entry.path) does not match the staged build manifest")
        }
    }

    return $failures
}

function Get-MsixArtifactFailures {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$MsixPath,
        [Parameter(Mandatory)]
        [string]$ExpectedVersion,
        [Parameter(Mandatory)]
        [string]$ExpectedIdentityName,
        [Parameter(Mandatory)]
        [string]$ExpectedPublisher
    )

    $failures = New-Object System.Collections.Generic.List[string]
    $packagePath = [System.IO.Path]::GetFullPath($MsixPath)
    if (-not (Test-Path -LiteralPath $packagePath -PathType Leaf)) {
        $failures.Add("MSIX package is not there: $packagePath")
        return $failures
    }
    $package = Get-Item -LiteralPath $packagePath
    if ($package.Name -notlike "*-$ExpectedVersion-*") {
        $failures.Add("$($package.Name) is not a $ExpectedVersion package")
    }
    $sidecar = "$packagePath.sha256"
    if (-not (Test-Path -LiteralPath $sidecar -PathType Leaf)) {
        $failures.Add("$($package.Name) has no .sha256 beside it")
    } else {
        $parts = "$(Get-Content -LiteralPath $sidecar -Raw)".Trim() -split '\s+', 2
        $actual = (Get-FileHash -LiteralPath $packagePath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($parts[0].ToLowerInvariant() -ne $actual) {
            $failures.Add("$($package.Name) does not match its .sha256")
        }
        if ($parts.Count -lt 2 -or $parts[1].Trim() -ne $package.Name) {
            $failures.Add("$($package.Name) sidecar does not name the exact package")
        }
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = $null
    try {
        $archive = [System.IO.Compression.ZipFile]::OpenRead($packagePath)
        $entries = @{}
        foreach ($entry in $archive.Entries) {
            if (-not [string]::IsNullOrEmpty($entry.Name)) {
                $entries[$entry.FullName.Replace('\', '/')] = $entry
            }
        }
        foreach ($required in @('AppxManifest.xml', 'build-manifest.json')) {
            if (-not $entries.ContainsKey($required)) {
                $failures.Add("MSIX package is missing '$required'")
            }
        }
        if ($failures.Count -gt 0) { return $failures }

        function Read-PackageText([System.IO.Compression.ZipArchiveEntry]$Entry) {
            $stream = $Entry.Open()
            try {
                $reader = New-Object System.IO.StreamReader($stream, [Text.Encoding]::UTF8, $true, 4096, $true)
                try { return $reader.ReadToEnd() } finally { $reader.Dispose() }
            } finally {
                $stream.Dispose()
            }
        }

        [xml]$appx = Read-PackageText $entries['AppxManifest.xml']
        $identity = $appx.Package.Identity
        if ($identity.Name -cne $ExpectedIdentityName) {
            $failures.Add("MSIX identity '$($identity.Name)' is not '$ExpectedIdentityName'")
        }
        if ($identity.Publisher -cne $ExpectedPublisher) {
            $failures.Add("MSIX publisher '$($identity.Publisher)' is not '$ExpectedPublisher'")
        }
        if ($identity.Version -ne "$ExpectedVersion.0") {
            $failures.Add("MSIX version '$($identity.Version)' is not '$ExpectedVersion.0'")
        }

        $build = (Read-PackageText $entries['build-manifest.json']) | ConvertFrom-Json
        if ($build.schema_version -ne 1 -or $build.product -ne 'SayFlex' -or
            $build.version -ne $ExpectedVersion) {
            $failures.Add('the packaged build manifest has an unsupported or stale identity')
        }
        $expectedFiles = @(
            @($build.files | ForEach-Object { "$($_.path)".Replace('\', '/') })
            'build-manifest.json'
            'AppxManifest.xml'
            'Assets/StoreLogo.png'
            'Assets/Square44x44Logo.png'
            'Assets/Square150x150Logo.png'
        ) | Sort-Object -Unique
        $packageMetadata = @('[Content_Types].xml', 'AppxBlockMap.xml', 'AppxSignature.p7x', 'CodeIntegrity.cat')
        $actualFiles = @($entries.Keys | Where-Object { $packageMetadata -notcontains $_ } | Sort-Object)
        foreach ($difference in Compare-Object $expectedFiles $actualFiles) {
            if ($difference.SideIndicator -eq '=>') {
                $failures.Add("MSIX package contains unexpected file '$($difference.InputObject)'")
            } else {
                $failures.Add("MSIX package is missing '$($difference.InputObject)'")
            }
        }
        foreach ($declared in @($build.files)) {
            $name = "$($declared.path)".Replace('\', '/')
            if (-not $entries.ContainsKey($name)) { continue }
            $entry = $entries[$name]
            $stream = $entry.Open()
            try {
                $sha = [System.Security.Cryptography.SHA256]::Create()
                try {
                    $bytes = $sha.ComputeHash($stream)
                    $hash = ([BitConverter]::ToString($bytes)).Replace('-', '').ToLowerInvariant()
                } finally {
                    $sha.Dispose()
                }
            } finally {
                $stream.Dispose()
            }
            if ($entry.Length -ne $declared.bytes -or
                $hash -ne "$($declared.sha256)".ToLowerInvariant()) {
                $failures.Add("$name does not match the packaged build manifest")
            }
        }
    } catch {
        $failures.Add("MSIX package is unreadable: $($_.Exception.Message)")
    } finally {
        if ($null -ne $archive) { $archive.Dispose() }
    }
    return $failures
}

Export-ModuleMember -Function Get-SayFlexInteractiveSmokeCheckIds, Get-SayFlexAutomatedSmokeCheckIds, Get-InstallerArtifactFailures, Get-SmokeEvidenceFailures, Get-DelayedClipboardImageReceiptFailures, Get-MsixLayoutFailures, Get-MsixArtifactFailures
