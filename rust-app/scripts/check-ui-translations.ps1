param(
    [string]$CatalogRoot = (Join-Path $PSScriptRoot '..\apps\desktop\translations'),
    [string]$UiRoot = (Join-Path $PSScriptRoot '..\apps\desktop\ui'),
    [string]$TypedTextPath = (Join-Path $PSScriptRoot '..\apps\desktop\ui_texts.txt')
)

$ErrorActionPreference = 'Stop'

function ConvertFrom-PoLiteral([string]$Value) {
    $Value.Replace('\n', "`n").Replace('\"', '"').Replace('\\', '\')
}

function Get-UiMessageIds {
    $slint = Get-ChildItem -LiteralPath $UiRoot -Filter '*.slint' -File |
        ForEach-Object {
            $source = Get-Content -Raw -LiteralPath $_.FullName
            # Capture the message literal even when @tr also receives format
            # arguments. Restricting the expression to @tr("message") made
            # every parameterized Slint message invisible to this validator.
            [regex]::Matches($source, '@tr\("([^"\\]*(?:\\.[^"\\]*)*)"') |
                ForEach-Object { ConvertFrom-PoLiteral $_.Groups[1].Value }
        }
    $typed = Get-Content -LiteralPath $TypedTextPath |
        Where-Object { $_ -and -not $_.StartsWith('#') } |
        ForEach-Object { ($_ -split '\|', 2)[1] }
    @($slint) + @($typed) | Sort-Object -Unique -CaseSensitive
}

function Read-PoCatalog([string]$Path) {
    $catalog = [Collections.Generic.Dictionary[string,string]]::new([StringComparer]::Ordinal)
    $duplicates = [Collections.Generic.List[string]]::new()
    $lines = Get-Content -LiteralPath $Path
    for ($index = 0; $index + 1 -lt $lines.Count; $index++) {
        if (-not $lines[$index].StartsWith('msgid "') -or
            -not $lines[$index + 1].StartsWith('msgstr "')) {
            continue
        }
        $id = ConvertFrom-PoLiteral $lines[$index].Substring(7, $lines[$index].Length - 8)
        $value = ConvertFrom-PoLiteral $lines[$index + 1].Substring(8, $lines[$index + 1].Length - 9)
        if (-not $id) { continue }
        if ($catalog.ContainsKey($id)) { [void]$duplicates.Add($id) }
        $catalog[$id] = $value
        $index++
    }
    [pscustomobject]@{ Entries = $catalog; Duplicates = @($duplicates) }
}

function Get-Placeholders([string]$Value) {
    @([regex]::Matches($Value, '\{(?:[A-Za-z0-9_]+)?\}') |
        ForEach-Object Value | Sort-Object)
}

$messages = @(Get-UiMessageIds)
$messageSet = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
$messages | ForEach-Object { [void]$messageSet.Add($_) }
$locales = @('ru', 'uk', 'es', 'pt-BR', 'de', 'fr', 'it', 'ko')
$literalAllowlist = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
@(
    'AUTO', 'CPU', 'CUDA', 'Vulkan', 'NVIDIA', 'PIN', 'ORIG', 'DUAL',
    'SayFlex', 'SayFlex {}', 'Say', 'Flex', 'Ctrl + Alt + F', 'CTRL + ALT + F',
    'Deutsch', 'Español', 'Français', 'Italiano', 'Português (Brasil)', 'Hindi',
    'local', 'Monochrome', 'Region', 'Engine', 'Account', 'PALETTE', 'STYLE',
    'SURFACE', 'ACTION', 'VERSION', 'INTERFACE', 'ENTER', 'via {provider}',
    '{code}: {message}'
) | ForEach-Object { [void]$literalAllowlist.Add($_) }

$failures = [Collections.Generic.List[string]]::new()
foreach ($locale in $locales) {
    $path = Join-Path $CatalogRoot "$locale\LC_MESSAGES\sayflex-desktop.po"
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        $failures.Add("$locale`: catalog is missing")
        continue
    }
    $parsed = Read-PoCatalog $path
    foreach ($duplicate in $parsed.Duplicates) {
        $failures.Add("$locale`: duplicate '$duplicate'")
    }
    foreach ($message in $messages) {
        if (-not $parsed.Entries.ContainsKey($message)) {
            $failures.Add("$locale`: missing '$message'")
            continue
        }
        $translated = [string]$parsed.Entries[$message]
        if (-not $translated) {
            $failures.Add("$locale`: empty '$message'")
        }
        $expectedSlots = @(Get-Placeholders $message)
        $actualSlots = @(Get-Placeholders $translated)
        if (($expectedSlots -join '|') -ne ($actualSlots -join '|')) {
            $failures.Add("$locale`: placeholders changed in '$message'")
        }
        if ($translated -ceq $message -and $message -match '[A-Za-z]{4}' -and
            -not $literalAllowlist.Contains($message)) {
            $failures.Add("$locale`: untranslated English '$message'")
        }
    }
    foreach ($id in $parsed.Entries.Keys) {
        if (-not $messageSet.Contains($id)) {
            $failures.Add("$locale`: stale '$id'")
        }
    }
}

if ($failures.Count -gt 0) {
    throw "UI translation validation failed ($($failures.Count)):`n$($failures -join "`n")"
}

Write-Host "UI translations: $($messages.Count) active messages across $($locales.Count) locales; no missing, stale, duplicate, empty, placeholder-drift, or unapproved English entries."
