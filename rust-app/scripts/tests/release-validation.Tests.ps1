$ErrorActionPreference = 'Stop'

$module = Join-Path $PSScriptRoot '..\release-validation.psm1'
Import-Module $module -Force

Describe 'SayFlex public repository presentation' {
    It 'presents the current product instead of the retired Keyboop release' {
        $repositoryReadme = Join-Path $PSScriptRoot '..\..\..\README.md'
        $content = Get-Content -LiteralPath $repositoryReadme -Raw

        $content | Should Match '(?m)^# SayFlex$'
        $content | Should Not Match '(?m)^# Keyboop for Windows$'
        $content | Should Match '0\.8\.11'
        $content | Should Match 'rust-app'
    }

    It 'preserves legacy copyright while naming the current SayFlex contributors' {
        $license = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\..\..\LICENSE') -Raw

        $license | Should Match '(?m)^MIT License\r?$'
        $license | Should Match '(?m)^Copyright \(c\) 2026 Keyboop for Windows contributors\r?$'
        $license | Should Match '(?m)^Copyright \(c\) 2026 SayFlex contributors\r?$'
    }

    It 'keeps the 0.8.11 product version aligned across source installer and public notes' {
        $rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
        $repositoryReadme = Get-Content -LiteralPath (Join-Path $rustRoot '..\README.md') -Raw
        $workspace = Get-Content -LiteralPath (Join-Path $rustRoot 'Cargo.toml') -Raw
        $installer = Get-Content -LiteralPath (Join-Path $rustRoot 'installer\SayFlex.iss') -Raw
        $releaseNotes = Join-Path $rustRoot 'RELEASE_0.8.11.md'

        $workspace | Should Match '(?m)^version = "0\.8\.11"\r?$'
        $installer | Should Match '(?m)^\s*#define AppVersion "0\.8\.11"\r?$'
        $repositoryReadme | Should Match 'version \*\*0\.8\.11\*\*'
        $repositoryReadme | Should Match 'rust-app/RELEASE_0\.8\.11\.md'
        $releaseNotes | Should Exist
    }

    It 'documents the screen capture glossary and local OCR privacy boundaries' {
        $rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
        $readme = Get-Content -LiteralPath (Join-Path $rustRoot 'README.md') -Raw
        $security = Get-Content -LiteralPath (Join-Path $rustRoot 'SECURITY.md') -Raw
        $architecture = Get-Content -LiteralPath (
            Join-Path $rustRoot 'docs\TRANSLATION_ARCHITECTURE.md'
        ) -Raw

        $readme | Should Match 'Ctrl \+ Alt \+ G'
        $readme | Should Match '24 hours'
        $security | Should Match 'selected crop'
        $security | Should Match '500 MB'
        $security | Should Match 'personal glossary'
        $architecture | Should Match 'block_id'
        $architecture | Should Match '(?s)compact.*limited'
    }

    It 'drives the OCR benchmark through the shipped worker protocol' {
        $benchmark = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\benchmark-ocr.ps1') -Raw

        $benchmark | Should Match 'SayFlex\.Ocr\.exe'
        $benchmark | Should Match '--ocr-worker'
        $benchmark | Should Match '--ocr-bundle'
        $benchmark | Should Match 'Shipped PP-OCR worker path verified'
    }

    It 'keeps every repository-relative README link resolvable' {
        $repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
        $repositoryReadme = Join-Path $repositoryRoot 'README.md'
        $content = Get-Content -LiteralPath $repositoryReadme -Raw
        $missing = @(
            [regex]::Matches($content, '\[[^\]]+\]\((?!https?://|#)([^)]+)\)') |
                ForEach-Object {
                    $relativePath = $_.Groups[1].Value.Split('#')[0]
                    if (-not (Test-Path -LiteralPath (Join-Path $repositoryRoot $relativePath))) {
                        $relativePath
                    }
                }
        )

        $missing | Should BeNullOrEmpty
    }

    It 'documents only released recording visualizer preview arguments' {
        $readme = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\..\README.md') -Raw

        $readme | Should Not Match '--indicator-preview=(orb|spectrum|ribbon)\b'
        foreach ($style in @('mic', 'circle', 'flow-sphere', 'pulse-matrix', 'writing-slate')) {
            $readme | Should Match "--indicator-preview=$([regex]::Escape($style))\b"
        }
    }

    It 'describes only translation providers that the current router can use' {
        $security = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\..\SECURITY.md') -Raw

        $security | Should Match 'DeepL'
        $security | Should Match 'Azure Translator'
        $security | Should Not Match 'Google Cloud Translation'
        $security | Should Match 'Every cloud row in both lists starts switched off'
    }

    It 'resolves the notice output after Windows PowerShell initializes PSScriptRoot' {
        $generator = Get-Content -LiteralPath (
            Join-Path $PSScriptRoot '..\generate-third-party-notices.ps1'
        ) -Raw

        $generator | Should Not Match '\$OutputPath\s*=\s*\(Join-Path\s+\$PSScriptRoot'
        $generator | Should Match "if \(\[string\]::IsNullOrWhiteSpace\(\`$OutputPath\)\)"
        $generator | Should Match '\$OutputPath\s*=\s*Join-Path\s+\$rustRoot'
    }

    It 'generates identical UTF-8 notices in Windows PowerShell and PowerShell 7' {
        $generator = Join-Path $PSScriptRoot '..\generate-third-party-notices.ps1'
        $windowsPowerShellOutput = Join-Path $TestDrive 'notices-windows-powershell.md'
        $powerShellOutput = Join-Path $TestDrive 'notices-powershell.md'

        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $generator `
            -OutputPath $windowsPowerShellOutput *> $null
        $LASTEXITCODE | Should Be 0

        & pwsh.exe -NoProfile -File $generator -OutputPath $powerShellOutput *> $null
        $LASTEXITCODE | Should Be 0

        (Get-FileHash -LiteralPath $windowsPowerShellOutput -Algorithm SHA256).Hash |
            Should Be (Get-FileHash -LiteralPath $powerShellOutput -Algorithm SHA256).Hash
        $content = Get-Content -LiteralPath $windowsPowerShellOutput -Raw -Encoding utf8
        $copyrightNotice = "Copyright $([char]0x00A9) SixtyFPS"
        $mojibakeNotice = "Copyright $([char]0x00C2)$([char]0x00A9) SixtyFPS"
        $content | Should Match ([regex]::Escape($copyrightNotice))
        $content | Should Not Match ([regex]::Escape($mojibakeNotice))
    }

    It 'discloses the proprietary NVIDIA runtime bundled by the universal installer' {
        $publish = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\publish.ps1') -Raw
        $generator = Get-Content -LiteralPath (
            Join-Path $PSScriptRoot '..\generate-third-party-notices.ps1'
        ) -Raw
        $notice = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\..\THIRD_PARTY_NOTICES.md') -Raw

        $publish | Should Match 'cublas64_13\.dll'
        $publish | Should Match 'cublasLt64_13\.dll'
        $generator | Should Match 'cublas64_13\.dll'
        $generator | Should Match 'cublasLt64_13\.dll'
        $notice | Should Match '(?s)## NVIDIA CUDA runtime.*proprietary NVIDIA software.*NVIDIA CUDA.*Toolkit EULA'
    }

    It 'ships and validates the OCR worker in the ordinary release payload' {
        $publish = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\publish.ps1') -Raw
        $validator = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\validate-release.ps1') -Raw
        $generator = Get-Content -LiteralPath (
            Join-Path $PSScriptRoot '..\generate-third-party-notices.ps1'
        ) -Raw
        $notice = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\..\THIRD_PARTY_NOTICES.md') -Raw

        $publish | Should Match "'sayflex-ocr-worker\.exe'\s*=\s*'SayFlex\.Ocr\.exe'"
        $publish | Should Match "SayFlex\.Ocr\.exe'\)[\s``]+-ArgumentList '--install-models'"
        $validator | Should Match "'SayFlex\.Ocr\.exe'"
        $validator | Should Match "ocr-models\\screen-ocr-ppocrv5-mobile-v3\.9\.2\\bundle\.verified\.json"
        $validator | Should Match '(?s)Every process that speaks the protocol.*SayFlex\.Ocr\.exe'
        $generator | Should Match 'screen-ocr-ppocrv5-mobile-v3\.9\.2'
        $notice | Should Match 'screen-ocr-ppocrv5-mobile-v3\.9\.2'
        $notice | Should Not Match '(?m)^## HY-MT2'
    }

    It 'pins every supported compact Firefox language pack without bundling another runtime' {
        $catalogPath = Join-Path $PSScriptRoot '..\..\crates\translation-engine\models\catalog.json'
        $catalog = Get-Content -LiteralPath $catalogPath -Raw | ConvertFrom-Json
        $compact = @($catalog.packs | Where-Object tier -eq 'compact')
        $expected = @(
            'firefox-ru-en-v3.1', 'firefox-uk-en-v3', 'firefox-es-en-v3',
            'firefox-pt-en-v3', 'firefox-de-en-v3', 'firefox-fr-en-v3',
            'firefox-it-en-v3', 'firefox-zh-hans-en-v3', 'firefox-zh-hant-en-v3',
            'firefox-ja-en-v3', 'firefox-ko-en-v3', 'firefox-hi-en-v3',
            'firefox-ar-en-v3'
        )
        $generator = Get-Content -LiteralPath (
            Join-Path $PSScriptRoot '..\generate-third-party-notices.ps1'
        ) -Raw
        $notice = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\..\THIRD_PARTY_NOTICES.md') -Raw
        $validator = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\validate-release.ps1') -Raw

        $catalog.schema_version | Should Be 4
        $compact.Count | Should Be 13
        @($compact.id | Sort-Object) -join ',' | Should Be (@($expected | Sort-Object) -join ',')
        foreach ($pack in $compact) {
            $pack.license | Should Be 'MPL-2.0'
            @($pack.backend.directions).Count | Should Be 2
            (@($pack.languages) -contains 'english') | Should Be $true
            @($pack.artifacts).Count | Should BeGreaterThan 5
            @($pack.artifacts | Where-Object {
                $_.compression -eq 'zstd' -and
                $_.bytes -gt 0 -and
                $_.sha256 -match '^[0-9a-f]{64}$' -and
                $_.expanded_bytes -gt 0 -and
                $_.expanded_sha256 -match '^[0-9a-f]{64}$'
            }).Count | Should Be @($pack.artifacts).Count
        }
        $generator | Should Match 'fxtranslate'
        $generator | Should Match 'Firefox Translations'
        $notice | Should Match '(?s)fxtranslate.*MPL-2\.0'
        $notice | Should Match '(?s)Firefox Translations.*installed separately on demand'
        $validator | Should Match 'expectedCompactIds'
        $validator | Should Match 'Python\|WASM\|MKL'
    }
}

Describe 'SayFlex installed smoke contract' {
    It 'publishes one authoritative interactive check catalog' {
        Get-Command Get-SayFlexInteractiveSmokeCheckIds -ErrorAction SilentlyContinue |
            Should Not BeNullOrEmpty
        Get-Command Get-SayFlexAutomatedSmokeCheckIds -ErrorAction SilentlyContinue |
            Should Not BeNullOrEmpty
    }

    It 'requires every release interaction and translation outcome separately' {
        $actual = @(Get-SayFlexInteractiveSmokeCheckIds)
        $expected = @(
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

        ($actual -join ',') | Should Be ($expected -join ',')
        (@(Get-SayFlexAutomatedSmokeCheckIds) -join ',') |
            Should Be 'clipboard_delayed_image_fixture'
    }

    It 'keeps witnessed evidence schema aligned with the interactive catalog' {
        $schema = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\smoke-evidence.schema.json') -Raw |
            ConvertFrom-Json
        $schemaIds = @($schema.properties.checks.properties.PSObject.Properties.Name | Sort-Object)
        $interactiveIds = @(Get-SayFlexInteractiveSmokeCheckIds | Sort-Object)

        ($schemaIds -join ',') | Should Be ($interactiveIds -join ',')
    }
}

Describe 'SayFlex installer artifact validation' {
    It 'reuses the existing install directory during an in-place upgrade' {
        $installerScript = Get-Content -LiteralPath (
            Join-Path $PSScriptRoot '..\..\installer\SayFlex.iss'
        ) -Raw

        $installerScript | Should Match '(?m)^UsePreviousAppDir=yes\r?$'
        $installerScript | Should Not Match '(?m)^UsePreviousAppDir=no\r?$'
    }

    It 'never removes a legacy path when it is the active install directory' {
        $installerScript = Get-Content -LiteralPath (
            Join-Path $PSScriptRoot '..\..\installer\SayFlex.iss'
        ) -Raw

        $installerScript | Should Match 'function CanonicalInstallPath\('
        $installerScript | Should Match 'PathNormalizeSlashes\(ExpandFileName\(Path\)\)'
        $installerScript | Should Match 'CompareText\(CanonicalInstallPath\(LegacyDir\), CurrentDir\) <> 0'

        function Get-TestCanonicalInstallPath([string]$Path) {
            $canonical = [System.IO.Path]::GetFullPath($Path.Replace('/', '\')).TrimEnd('\')
            while ($canonical.EndsWith('.') -or $canonical.EndsWith(' ')) {
                $canonical = $canonical.Substring(0, $canonical.Length - 1)
            }
            $canonical.ToUpperInvariant()
        }

        $styleFlex = Join-Path $TestDrive 'Programs\StyleFlex'
        $cases = @(
            $styleFlex,
            $styleFlex.ToUpperInvariant(),
            ($styleFlex + '\'),
            ($styleFlex.Replace('\', '/')),
            ($styleFlex + '.')
        )
        $expected = Get-TestCanonicalInstallPath $styleFlex
        foreach ($case in $cases) {
            (Get-TestCanonicalInstallPath $case) | Should Be $expected
        }
    }

    It 'rejects a staged executable whose Windows identity names another file' {
        $validator = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\validate-release.ps1') -Raw

        $validator | Should Match '\$info\.OriginalFilename\s+-ne\s+\$name'
        $validator | Should Match 'reports original filename'
    }

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

Describe 'SayFlex delayed clipboard image evidence validation' {
    BeforeEach {
        $script:imageHash = ('a' * 64)
        $script:receipt = [pscustomobject]@{
            schema_version = 2
            product = 'SayFlex'
            input_host_sha256 = ('b' * 64)
            witnessed_utc = '2026-09-19T12:30:00Z'
            cases = @(
                0, 1, 10, 60 | ForEach-Object {
                    [pscustomobject]@{
                        provider_process = 'SayFlex.DelayedImageProvider'
                        consumer_process = 'FixtureConsumer'
                        delay_seconds = $_
                        source_sha256 = $script:imageHash
                        received_sha256 = $script:imageHash
                        responsive = $true
                        provider_probe_exit_code = 0
                        input_host_exit_code = 0
                        consumer_probe_exit_code = 0
                        status = 'PASS'
                    }
                }
            )
            competitive_copy = [pscustomobject]@{
                materialized_by_non_target = $true
                expected_text = 'USER_COPY_MUST_WIN'
                received_text = 'USER_COPY_MUST_WIN'
                source_sha256 = $script:imageHash
                received_sha256 = $script:imageHash
                input_host_exit_code = 1
                consumer_probe_exit_code = 0
                status = 'PASS'
            }
        }
    }

    It 'accepts exactly one successful case for every required delay and consumer' {
        $failures = @(Get-DelayedClipboardImageReceiptFailures `
            -Receipt $script:receipt `
            -ExpectedDelays @(0, 1, 10, 60) `
            -ExpectedConsumers @('FixtureConsumer'))

        $failures | Should BeNullOrEmpty
    }

    It 'accepts a receipt timestamp after a JSON round trip converts it to DateTime' {
        $roundTripped = $script:receipt | ConvertTo-Json -Depth 6 | ConvertFrom-Json

        $failures = @(Get-DelayedClipboardImageReceiptFailures `
            -Receipt $roundTripped `
            -ExpectedDelays @(0, 1, 10, 60) `
            -ExpectedConsumers @('FixtureConsumer'))

        $failures | Should BeNullOrEmpty
    }

    It 'rejects missing delays, changed images, an unresponsive consumer, and failed probes' {
        $script:receipt.cases = @($script:receipt.cases | Where-Object delay_seconds -ne 10)
        $script:receipt.cases[0].received_sha256 = ('c' * 64)
        $script:receipt.cases[1].responsive = $false
        $script:receipt.cases[2].consumer_probe_exit_code = 5

        $failures = @(Get-DelayedClipboardImageReceiptFailures `
            -Receipt $script:receipt `
            -ExpectedDelays @(0, 1, 10, 60) `
            -ExpectedConsumers @('FixtureConsumer'))

        ($failures -join "`n") | Should Match 'delay 10'
        ($failures -join "`n") | Should Match 'image hash'
        ($failures -join "`n") | Should Match 'responsive'
        ($failures -join "`n") | Should Match 'consumer probe exit code'
    }

    It 'rejects a competitive Copy that was overwritten or accepted as delivered' {
        $script:receipt.competitive_copy.received_text = 'OLDER_CLIPBOARD'
        $script:receipt.competitive_copy.received_sha256 = ('c' * 64)
        $script:receipt.competitive_copy.input_host_exit_code = 0

        $failures = @(Get-DelayedClipboardImageReceiptFailures `
            -Receipt $script:receipt `
            -ExpectedDelays @(0, 1, 10, 60) `
            -ExpectedConsumers @('FixtureConsumer'))

        ($failures -join "`n") | Should Match 'competitive Copy text'
        ($failures -join "`n") | Should Match 'competitive Copy image'
        ($failures -join "`n") | Should Match 'superseded insertion'
    }

    It 'validates a complete persisted receipt through the public smoke command' {
        $receiptPath = Join-Path $TestDrive 'delayed-image-receipt.json'
        $script:receipt | ConvertTo-Json -Depth 6 |
            Set-Content -LiteralPath $receiptPath -Encoding utf8

        & powershell.exe -NoProfile -ExecutionPolicy Bypass `
            -File (Join-Path $PSScriptRoot '..\test-delayed-clipboard-image.ps1') `
            -ValidateReceiptPath $receiptPath

        $LASTEXITCODE | Should Be 0
    }
}

Describe 'SayFlex installed smoke runner wiring' {
    It 'imports the evidence validator it invokes' {
        $runner = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\smoke-installed.ps1') -Raw

        $runner | Should Match 'Import-Module \$validationModule -Force'
        $runner | Should Match 'Get-SmokeEvidenceFailures'
        $runner | Should Match 'Get-DelayedClipboardImageReceiptFailures'
        $runner | Should Match 'DelayedImageReceiptPath'
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
