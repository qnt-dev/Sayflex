$ErrorActionPreference = 'Stop'

$module = Join-Path $PSScriptRoot '..\release-validation.psm1'
Import-Module $module -Force

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
}
