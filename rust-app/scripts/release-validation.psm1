Set-StrictMode -Version Latest

function Get-SayFlexInteractiveSmokeCheckIds {
    @(
        'dictation_editor'
        'dictation_browser'
        'dictation_terminal'
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
        'fresh_install'
        'upgrade_install'
    )
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

Export-ModuleMember -Function Get-SayFlexInteractiveSmokeCheckIds, Get-InstallerArtifactFailures, Get-SmokeEvidenceFailures, Get-MsixLayoutFailures, Get-MsixArtifactFailures
