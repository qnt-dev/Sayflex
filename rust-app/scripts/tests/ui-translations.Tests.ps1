Describe 'UI translation catalog validation' {
    BeforeAll {
        $script:checker = Join-Path $PSScriptRoot '..\check-ui-translations.ps1'
    }

    It 'rejects stale entries and placeholder drift in a controlled catalog' {
        $root = Join-Path $TestDrive 'fixture'
        $ui = Join-Path $root 'ui'
        $catalogs = Join-Path $root 'translations'
        New-Item -ItemType Directory -Path $ui -Force | Out-Null
        Set-Content -LiteralPath (Join-Path $ui 'app.slint') -Encoding utf8 -Value 'Text { text: @tr("Hello {name}"); }'
        Set-Content -LiteralPath (Join-Path $root 'ui_texts.txt') -Encoding utf8 -Value ''
        foreach ($locale in @('ru', 'uk', 'es', 'pt-BR', 'de', 'fr', 'it', 'ko')) {
            $folder = Join-Path $catalogs "$locale\LC_MESSAGES"
            New-Item -ItemType Directory -Path $folder -Force | Out-Null
            @(
                'msgid ""'
                'msgstr ""'
                ''
                'msgid "Hello {name}"'
                'msgstr "Hello {person}"'
                ''
                'msgid "Removed control"'
                'msgstr "removed"'
            ) | Set-Content -LiteralPath (Join-Path $folder 'sayflex-desktop.po') -Encoding utf8
        }

        & pwsh -NoProfile -File $script:checker `
            -CatalogRoot $catalogs `
            -UiRoot $ui `
            -TypedTextPath (Join-Path $root 'ui_texts.txt') 2>$null
        $LASTEXITCODE | Should Not Be 0
    }
}
