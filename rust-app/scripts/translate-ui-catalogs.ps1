param(
    [string]$Worker = (Join-Path $PSScriptRoot '..\target\debug\sayflex-translation-worker.exe'),
    [int]$BatchSize = 24
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Worker -PathType Leaf)) {
    throw "Translation worker was not found: $Worker"
}

$catalogRoot = Join-Path $PSScriptRoot '..\apps\desktop\translations'
$uiRoot = Join-Path $PSScriptRoot '..\apps\desktop\ui'
$messageIds = Get-ChildItem -LiteralPath $uiRoot -Filter '*.slint' -File |
    ForEach-Object {
        $source = Get-Content -Raw -LiteralPath $_.FullName
        [regex]::Matches($source, '@tr\("([^"\\]*(?:\\.[^"\\]*)*)"\)') |
            ForEach-Object { $_.Groups[1].Value.Replace('\"', '"').Replace('\n', "`n").Replace('\\', '\') }
    }
$typedMessages = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\apps\desktop\ui_texts.txt') |
    Where-Object { $_ -and -not $_.StartsWith('#') } |
    ForEach-Object { ($_ -split '\|', 2)[1] }
$messageIds = @($messageIds) + @($typedMessages) | Sort-Object -Unique -CaseSensitive

$targets = [ordered]@{
    'ru'    = 'russian'
    'uk'    = 'ukrainian'
    'es'    = 'spanish'
    'pt-BR' = 'portuguese_brazil'
    'de'    = 'german'
    'fr'    = 'french'
    'it'    = 'italian'
    'ko'    = 'korean'
}

function Start-TranslationWorker {
    $info = [Diagnostics.ProcessStartInfo]::new((Resolve-Path -LiteralPath $Worker).Path)
    $info.UseShellExecute = $false
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $utf8 = [Text.UTF8Encoding]::new($false)
    if ($info.PSObject.Properties.Name -contains 'StandardInputEncoding') {
        $info.StandardInputEncoding = $utf8
        $info.StandardOutputEncoding = $utf8
        $info.StandardErrorEncoding = $utf8
    }
    $info.CreateNoWindow = $true
    $process = [Diagnostics.Process]::Start($info)
    $process.StandardInput.WriteLine('{"type":"set_translation_providers","payload":{"providers":[{"id":"deepl","enabled":true},{"id":"local","enabled":false}]}}')
    $process.StandardInput.Flush()
    do {
        $line = $process.StandardOutput.ReadLine()
        if ($null -eq $line) {
            $details = $process.StandardError.ReadToEnd()
            throw "The translation worker closed before reporting providers: $details"
        }
        $status = $line | ConvertFrom-Json
    } until ($status.type -eq 'translation_providers')
    $deepl = $status.payload.statuses | Where-Object id -eq 'deepl'
    if (-not $deepl.available) {
        throw "DeepL is not available through the local encrypted credential store: $($deepl.detail)"
    }
    return $process
}

function Invoke-BatchTranslation($Process, [string[]]$Messages, [string]$Target) {
    $parts = for ($index = 0; $index -lt $Messages.Count; $index++) {
        $protected = $Messages[$index]
        $slots = [regex]::Matches($protected, '\{[A-Za-z0-9_]+\}')
        for ($slotIndex = $slots.Count - 1; $slotIndex -ge 0; $slotIndex--) {
            $slot = $slots[$slotIndex]
            $marker = '[[[SFXP{0:D3}]]]' -f $slotIndex
            $protected = $protected.Substring(0, $slot.Index) + $marker +
                $protected.Substring($slot.Index + $slot.Length)
        }
        "[[[SAYFLEX_{0:D3}]]]`n{1}" -f $index, $protected
    }
    $requestId = [guid]::NewGuid().ToString()
    $command = @{
        type = 'translate'
        payload = @{
            request_id = $requestId
            text = $parts -join "`n"
            source_language = 'english'
            target_language = $Target
            language_policy = @{ primary = 'english'; secondary = $Target }
        }
    } | ConvertTo-Json -Compress -Depth 6
    $Process.StandardInput.WriteLine($command)
    $Process.StandardInput.Flush()

    do {
        $line = $Process.StandardOutput.ReadLine()
        if ($null -eq $line) {
            $details = $Process.StandardError.ReadToEnd()
            throw "The translation worker closed its output unexpectedly: $details"
        }
        $event = $line | ConvertFrom-Json
    } until ($event.type -in @('completed', 'failed', 'cancelled'))

    if ($event.type -ne 'completed') {
        throw "Translation failed: $($event.payload.message)"
    }
    if ($event.payload.result.provider -ne 'deepl') {
        throw "Expected DeepL, got $($event.payload.result.provider)."
    }

    $translated = @{}
    $matches = [regex]::Matches(
        $event.payload.result.translated_text,
        '(?ms)^\[\[\[SAYFLEX_(\d{3})\]\]\]\r?\n(.*?)(?=^\[\[\[SAYFLEX_\d{3}\]\]\]\r?$|\z)'
    )
    foreach ($match in $matches) {
        $messageIndex = [int]$match.Groups[1].Value
        $value = $match.Groups[2].Value.TrimEnd()
        $slots = [regex]::Matches($Messages[$messageIndex], '\{[A-Za-z0-9_]+\}')
        for ($slotIndex = 0; $slotIndex -lt $slots.Count; $slotIndex++) {
            $value = $value.Replace(
                ('[[[SFXP{0:D3}]]]' -f $slotIndex),
                $slots[$slotIndex].Value
            )
        }
        $translated[$messageIndex] = $value
    }
    if ($translated.Count -ne $Messages.Count) {
        throw "DeepL returned $($translated.Count) marked segments for $($Messages.Count) messages."
    }
    return 0..($Messages.Count - 1) | ForEach-Object { $translated[$_] }
}

function ConvertTo-PoLiteral([string]$Value) {
    return $Value.Replace('\', '\\').Replace('"', '\"').Replace("`r", '').Replace("`n", '\n')
}

function ConvertFrom-PoLiteral([string]$Value) {
    return $Value.Replace('\n', "`n").Replace('\"', '"').Replace('\\', '\')
}

function Read-PoCatalog([string]$Path) {
    $catalog = @{}
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $catalog
    }
    $lines = Get-Content -LiteralPath $Path
    for ($index = 0; $index + 1 -lt $lines.Count; $index++) {
        if (-not $lines[$index].StartsWith('msgid "') -or
            -not $lines[$index + 1].StartsWith('msgstr "')) {
            continue
        }
        $id = ConvertFrom-PoLiteral $lines[$index].Substring(7, $lines[$index].Length - 8)
        $value = ConvertFrom-PoLiteral $lines[$index + 1].Substring(8, $lines[$index + 1].Length - 9)
        if ($id -and $value) {
            $catalog[$id] = $value
        }
        $index++
    }
    return $catalog
}

$process = Start-TranslationWorker
try {
    foreach ($entry in $targets.GetEnumerator()) {
        $destination = Join-Path $catalogRoot "$($entry.Key)\LC_MESSAGES\sayflex-desktop.po"
        $catalog = Read-PoCatalog $destination
        $missing = @($messageIds | Where-Object { -not $catalog.ContainsKey($_) })
        for ($offset = 0; $offset -lt $missing.Count; $offset += $BatchSize) {
            $count = [Math]::Min($BatchSize, $missing.Count - $offset)
            $batch = [string[]]$missing[$offset..($offset + $count - 1)]
            $translatedBatch = @(Invoke-BatchTranslation $process $batch $entry.Value)
            for ($translatedIndex = 0; $translatedIndex -lt $batch.Count; $translatedIndex++) {
                $catalog[$batch[$translatedIndex]] = $translatedBatch[$translatedIndex]
            }
        }

        $output = [Collections.Generic.List[string]]::new()
        $output.Add('msgid ""')
        $output.Add('msgstr ""')
        $output.Add(('"Language: {0}\n"' -f $entry.Key))
        $output.Add('"Content-Type: text/plain; charset=UTF-8\n"')
        $output.Add('"Content-Transfer-Encoding: 8bit\n"')
        $output.Add('')
        for ($index = 0; $index -lt $messageIds.Count; $index++) {
            $output.Add(('msgid "{0}"' -f (ConvertTo-PoLiteral $messageIds[$index])))
            $output.Add(('msgstr "{0}"' -f (ConvertTo-PoLiteral $catalog[$messageIds[$index]])))
            $output.Add('')
        }

        [IO.File]::WriteAllLines($destination, $output, [Text.UTF8Encoding]::new($false))
        Write-Host "Translated $($missing.Count) missing messages for $($entry.Key); $($messageIds.Count) total."
    }
}
finally {
    if ($null -ne $process -and -not $process.HasExited) {
        $process.StandardInput.WriteLine('{"type":"shutdown"}')
        $process.StandardInput.Close()
        if (-not $process.WaitForExit(5000)) {
            $process.Kill()
        }
    }
}
