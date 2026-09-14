$ErrorActionPreference = 'Stop'

$module = Join-Path $PSScriptRoot '..\release-validation.psm1'
Import-Module $module -Force

Describe 'SayFlex installed smoke contract' {
    It 'publishes one authoritative interactive check catalog' {
        Get-Command Get-SayFlexInteractiveSmokeCheckIds -ErrorAction SilentlyContinue |
            Should Not BeNullOrEmpty
    }

    It 'requires every release interaction and translation outcome separately' {
        $actual = @(Get-SayFlexInteractiveSmokeCheckIds)
        $expected = @(
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

        ($actual -join ',') | Should Be ($expected -join ',')
    }
}

Describe 'SayFlex installer artifact validation' {
    BeforeEach {
        $script:fixture = Join-Path $TestDrive 'SayFlex-0.7.0-win-x64-setup.exe'
        [System.IO.File]::WriteAllBytes($script:fixture, [byte[]](1, 2, 3, 4))
        $hash = (Get-FileHash -LiteralPath $script:fixture -Algorithm SHA256).Hash
        Set-Content -LiteralPath "$script:fixture.sha256" -Encoding ascii -NoNewline -Value (
            "$hash  $([System.IO.Path]::GetFileName($script:fixture))"
        )
    }

    It 'rejects a stale installer name' {
        $stale = Join-Path $TestDrive 'SayFlex-0.6.3-win-x64-setup.exe'
        Move-Item -LiteralPath $script:fixture -Destination $stale
        Move-Item -LiteralPath "$script:fixture.sha256" -Destination "$stale.sha256"

        $failures = @(Get-InstallerArtifactFailures -InstallerPath $stale -ExpectedVersion '0.7.0')

        ($failures -join "`n") | Should Match 'not the 0.7.0 installer'
    }

    It 'rejects a checksum copied from another installer' {
        Set-Content -LiteralPath "$script:fixture.sha256" -Encoding ascii -NoNewline -Value (
            ('0' * 64) + '  SayFlex-0.7.0-win-x64-setup.exe'
        )

        $failures = @(Get-InstallerArtifactFailures -InstallerPath $script:fixture -ExpectedVersion '0.7.0')

        ($failures -join "`n") | Should Match 'does not match its .sha256'
    }

    It 'rejects an installer changed after its sidecar was written' {
        [System.IO.File]::AppendAllText($script:fixture, 'changed')

        $failures = @(Get-InstallerArtifactFailures -InstallerPath $script:fixture -ExpectedVersion '0.7.0')

        ($failures -join "`n") | Should Match 'does not match its .sha256'
    }

    It 'rejects an unsigned public installer' {
        $failures = @(
            Get-InstallerArtifactFailures `
                -InstallerPath $script:fixture `
                -ExpectedVersion '0.7.0' `
                -RequireSignature
        )

        ($failures -join "`n") | Should Match 'valid Authenticode signature'
    }
}

Describe 'SayFlex witnessed smoke evidence validation' {
    BeforeEach {
        $script:allowed = @('dictation_editor', 'translation_popup')
        $script:evidence = [pscustomobject]@{
            schema_version = 1
            product = 'SayFlex'
            version = '0.7.0'
            git_commit = ('a' * 40)
            installer_sha256 = ('b' * 64)
            witnessed_utc = '2026-09-10T12:30:00Z'
            checks = [pscustomobject]@{
                dictation_editor = [pscustomobject]@{ status = 'PASS'; detail = 'witnessed' }
            }
        }
    }

    It 'accepts evidence bound to the exact artifact' {
        $failures = @(Get-SmokeEvidenceFailures `
            -Evidence $script:evidence `
            -ExpectedVersion '0.7.0' `
            -ExpectedCommit ('a' * 40) `
            -ExpectedInstallerSha256 ('b' * 64) `
            -AllowedChecks $script:allowed)

        $failures | Should BeNullOrEmpty
    }

    It 'rejects stale evidence even when every claimed check passes' {
        $script:evidence.version = '0.6.3'
        $script:evidence.git_commit = ('c' * 40)
        $script:evidence.installer_sha256 = ('d' * 64)

        $failures = @(Get-SmokeEvidenceFailures `
            -Evidence $script:evidence `
            -ExpectedVersion '0.7.0' `
            -ExpectedCommit ('a' * 40) `
            -ExpectedInstallerSha256 ('b' * 64) `
            -AllowedChecks $script:allowed)

        ($failures -join "`n") | Should Match 'version'
        ($failures -join "`n") | Should Match 'commit'
        ($failures -join "`n") | Should Match 'installer SHA-256'
    }

    It 'rejects unknown checks and attempts to override artifact identity' {
        $script:evidence.checks = [pscustomobject]@{
            artifact_identity = [pscustomobject]@{ status = 'PASS'; detail = 'trust me' }
            invented_check = [pscustomobject]@{ status = 'PASS'; detail = 'trust me too' }
        }

        $failures = @(Get-SmokeEvidenceFailures `
            -Evidence $script:evidence `
            -ExpectedVersion '0.7.0' `
            -ExpectedCommit ('a' * 40) `
            -ExpectedInstallerSha256 ('b' * 64) `
            -AllowedChecks $script:allowed)

        ($failures -join "`n") | Should Match 'artifact_identity'
        ($failures -join "`n") | Should Match 'invented_check'
    }
}

Describe 'SayFlex installed smoke runner wiring' {
    It 'imports the evidence validator it invokes' {
        $runner = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\smoke-installed.ps1') -Raw

        $runner | Should Match 'Import-Module \$validationModule -Force'
        $runner | Should Match 'Get-SmokeEvidenceFailures'
    }

    It 'records the exact installer hash without requiring witnessed evidence' {
        $runner = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\smoke-installed.ps1') -Raw

        $runner | Should Match '\$installerHash = if \(-not \[string\]::IsNullOrWhiteSpace\(\$InstallerPath\)'
        $runner | Should Match 'installer_sha256 = \$installerHash'
    }
}

Describe 'SayFlex MSIX layout validation' {
    BeforeEach {
        $fixtureId = [Guid]::NewGuid().ToString('N')
        $script:staged = Join-Path $TestDrive "$fixtureId\staged"
        $script:layout = Join-Path $TestDrive "$fixtureId\layout"
        New-Item -ItemType Directory -Path $script:staged, $script:layout | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $script:staged 'SayFlex.exe'), 'desktop')
        Copy-Item -LiteralPath (Join-Path $script:staged 'SayFlex.exe') -Destination $script:layout
        $hash = (Get-FileHash -LiteralPath (Join-Path $script:staged 'SayFlex.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
        [ordered]@{
            schema_version = 1
            product = 'SayFlex'
            version = '0.8.0'
            files = @([ordered]@{ path = 'SayFlex.exe'; bytes = 7; sha256 = $hash })
        } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $script:staged 'build-manifest.json') -Encoding utf8
        Copy-Item -LiteralPath (Join-Path $script:staged 'build-manifest.json') -Destination $script:layout
        New-Item -ItemType Directory -Path (Join-Path $script:layout 'Assets') | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $script:layout 'Assets\StoreLogo.png'), 'logo')
        [System.IO.File]::WriteAllText((Join-Path $script:layout 'Assets\Square44x44Logo.png'), 'logo')
        [System.IO.File]::WriteAllText((Join-Path $script:layout 'Assets\Square150x150Logo.png'), 'logo')
        @'
<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
         xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
         IgnorableNamespaces="uap rescap">
  <Identity Name="qnt-dev.SayFlex" Publisher="CN=SayFlex Development" Version="0.8.0.0" ProcessorArchitecture="x64" />
  <Properties><DisplayName>SayFlex</DisplayName><PublisherDisplayName>SayFlex</PublisherDisplayName><Logo>Assets\StoreLogo.png</Logo></Properties>
  <Dependencies><TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.19041.0" MaxVersionTested="10.0.26100.0" /></Dependencies>
  <Resources><Resource Language="en" /></Resources>
  <Applications><Application Id="SayFlex" Executable="SayFlex.exe" EntryPoint="Windows.FullTrustApplication"><uap:VisualElements DisplayName="SayFlex" Description="SayFlex" BackgroundColor="transparent" Square44x44Logo="Assets\Square44x44Logo.png" Square150x150Logo="Assets\Square150x150Logo.png" /></Application></Applications>
  <Capabilities><rescap:Capability Name="runFullTrust" /></Capabilities>
</Package>
'@ | Set-Content -LiteralPath (Join-Path $script:layout 'AppxManifest.xml') -Encoding utf8
    }

    It 'accepts a layout bound to its staged bytes and Partner Center identity' {
        $failures = @(Get-MsixLayoutFailures `
            -LayoutPath $script:layout `
            -BuildManifestPath (Join-Path $script:staged 'build-manifest.json') `
            -ExpectedIdentityName 'qnt-dev.SayFlex' `
            -ExpectedPublisher 'CN=SayFlex Development')

        $failures | Should BeNullOrEmpty
    }

    It 'rejects identity and version drift' {
        [xml]$manifest = Get-Content -LiteralPath (Join-Path $script:layout 'AppxManifest.xml') -Raw
        $manifest.Package.Identity.Name = 'stale.identity'
        $manifest.Package.Identity.Version = '0.7.2.0'
        $manifest.Save((Join-Path $script:layout 'AppxManifest.xml'))

        $failures = @(Get-MsixLayoutFailures `
            -LayoutPath $script:layout `
            -BuildManifestPath (Join-Path $script:staged 'build-manifest.json') `
            -ExpectedIdentityName 'qnt-dev.SayFlex' `
            -ExpectedPublisher 'CN=SayFlex Development')

        ($failures -join "`n") | Should Match 'identity'
        ($failures -join "`n") | Should Match 'version'
    }

    It 'rejects missing, changed, and extra payload files' {
        [System.IO.File]::WriteAllText((Join-Path $script:layout 'SayFlex.exe'), 'changed')
        [System.IO.File]::WriteAllText((Join-Path $script:layout 'stale.dll'), 'extra')

        $failures = @(Get-MsixLayoutFailures `
            -LayoutPath $script:layout `
            -BuildManifestPath (Join-Path $script:staged 'build-manifest.json') `
            -ExpectedIdentityName 'qnt-dev.SayFlex' `
            -ExpectedPublisher 'CN=SayFlex Development')

        ($failures -join "`n") | Should Match 'SayFlex.exe does not match'
        ($failures -join "`n") | Should Match 'unexpected file'
    }

    It 'validates the identity and payload inside the final package' {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $msix = Join-Path $TestDrive 'SayFlex-0.8.0-win-x64-store-unsigned.msix'
        [System.IO.Compression.ZipFile]::CreateFromDirectory($script:layout, $msix)
        $hash = (Get-FileHash -LiteralPath $msix -Algorithm SHA256).Hash
        Set-Content -LiteralPath "$msix.sha256" -Encoding ascii -NoNewline -Value (
            "$hash  $([System.IO.Path]::GetFileName($msix))"
        )

        $failures = @(Get-MsixArtifactFailures `
            -MsixPath $msix `
            -ExpectedVersion '0.8.0' `
            -ExpectedIdentityName 'qnt-dev.SayFlex' `
            -ExpectedPublisher 'CN=SayFlex Development')

        $failures | Should BeNullOrEmpty
    }
}
