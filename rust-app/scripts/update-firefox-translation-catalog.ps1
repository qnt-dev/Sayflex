[CmdletBinding()]
param(
    [string]$SourceUri = 'https://firefox.settings.services.mozilla.com/v1/buckets/main/collections/translations-models-v2/records',
    [string]$InputPath,
    [string]$OutputPath = (Join-Path $PSScriptRoot '..\crates\translation-engine\models\catalog.json')
)

$ErrorActionPreference = 'Stop'

$languages = @(
    [ordered]@{ Code = 'ru'; Rust = 'russian'; Name = 'Russian'; Id = 'firefox-ru-en-v3.1'; Locale = 'ru' },
    [ordered]@{ Code = 'uk'; Rust = 'ukrainian'; Name = 'Ukrainian'; Id = 'firefox-uk-en-v3'; Locale = 'uk' },
    [ordered]@{ Code = 'es'; Rust = 'spanish'; Name = 'Spanish'; Id = 'firefox-es-en-v3'; Locale = 'es' },
    [ordered]@{ Code = 'pt'; Rust = 'portuguese_brazil'; Name = 'Portuguese (Brazil)'; Id = 'firefox-pt-en-v3'; Locale = 'pt' },
    [ordered]@{ Code = 'de'; Rust = 'german'; Name = 'German'; Id = 'firefox-de-en-v3'; Locale = 'de' },
    [ordered]@{ Code = 'fr'; Rust = 'french'; Name = 'French'; Id = 'firefox-fr-en-v3'; Locale = 'fr' },
    [ordered]@{ Code = 'it'; Rust = 'italian'; Name = 'Italian'; Id = 'firefox-it-en-v3'; Locale = 'it' },
    [ordered]@{ Code = 'zh-Hans'; Rust = 'chinese_simplified'; Name = 'Chinese (Simplified)'; Id = 'firefox-zh-hans-en-v3'; Locale = 'zh' },
    [ordered]@{ Code = 'zh-Hant'; Rust = 'chinese_traditional'; Name = 'Chinese (Traditional)'; Id = 'firefox-zh-hant-en-v3'; Locale = 'zh' },
    [ordered]@{ Code = 'ja'; Rust = 'japanese'; Name = 'Japanese'; Id = 'firefox-ja-en-v3'; Locale = 'ja' },
    [ordered]@{ Code = 'ko'; Rust = 'korean'; Name = 'Korean'; Id = 'firefox-ko-en-v3'; Locale = 'ko' },
    [ordered]@{ Code = 'hi'; Rust = 'hindi'; Name = 'Hindi'; Id = 'firefox-hi-en-v3'; Locale = 'hi' },
    [ordered]@{ Code = 'ar'; Rust = 'arabic'; Name = 'Arabic'; Id = 'firefox-ar-en-v3'; Locale = 'ar' }
)

function Get-VersionRank([string]$Version) {
    if ($Version -match '^(\d+)\.(\d+)(?:a(\d+))?$') {
        $rank = ([int]$Matches[1] * 10000) + ([int]$Matches[2] * 100)
        if ($Matches[3]) {
            $rank -= 50
            $rank += [int]$Matches[3]
        }
        return $rank
    }
    return 0
}

function Get-ArchitectureRank([string]$Architecture) {
    switch ($Architecture) {
        'base-memory' { return 3 }
        'base' { return 2 }
        'tiny' { return 1 }
        default { return 0 }
    }
}

function Select-CompleteGeneration($Records, [string]$Source, [string]$Target) {
    $candidates = $Records | Where-Object {
        $_.sourceLanguage -eq $Source -and $_.targetLanguage -eq $Target
    } | Group-Object { "$($_.version)|$($_.architecture)" } | Where-Object {
        $types = @($_.Group.fileType)
        ($types -contains 'model') -and
        ($types -contains 'lex') -and
        (($types -contains 'vocab') -or (($types -contains 'srcvocab') -and ($types -contains 'trgvocab')))
    } | Sort-Object @(
        @{ Expression = { Get-VersionRank $_.Group[0].version }; Descending = $true },
        @{ Expression = { Get-ArchitectureRank $_.Group[0].architecture }; Descending = $true },
        @{ Expression = { ($_.Group | Measure-Object last_modified -Maximum).Maximum }; Descending = $true }
    )

    $selected = $candidates | Select-Object -First 1
    if (-not $selected) {
        throw "No complete Firefox generation for $Source->$Target"
    }
    return @($selected.Group)
}

function Select-OneFile($Generation, [string]$Type) {
    $matches = @($Generation | Where-Object fileType -eq $Type)
    if ($matches.Count -ne 1) {
        throw "Expected one $Type file, found $($matches.Count)"
    }
    return $matches[0]
}

function New-Artifact($Record, [string]$RelativePath) {
    if ($Record.attachment.hash -notmatch '^[0-9a-f]{64}$' -or
        $Record.decompressedHash -notmatch '^[0-9a-f]{64}$' -or
        [uint64]$Record.attachment.size -eq 0 -or
        [uint64]$Record.decompressedSize -eq 0) {
        throw "Invalid artifact metadata for $($Record.name)"
    }
    return [ordered]@{
        relative_path = $RelativePath
        url = "https://firefox-settings-attachments.cdn.mozilla.net/$($Record.attachment.location)"
        bytes = [uint64]$Record.attachment.size
        sha256 = [string]$Record.attachment.hash
        compression = 'zstd'
        expanded_bytes = [uint64]$Record.decompressedSize
        expanded_sha256 = [string]$Record.decompressedHash
    }
}

function New-Direction($Records, [string]$SourceCode, [string]$TargetCode, [string]$SourceRust, [string]$TargetRust) {
    $generation = Select-CompleteGeneration $Records $SourceCode $TargetCode
    $prefix = "$SourceCode-$TargetCode"
    $model = Select-OneFile $generation 'model'
    $lex = Select-OneFile $generation 'lex'
    $sharedVocab = @($generation | Where-Object fileType -eq 'vocab')
    if ($sharedVocab.Count -eq 1) {
        $sourceVocab = $sharedVocab[0]
        $targetVocab = $sharedVocab[0]
        $sourceVocabPath = "$prefix/vocab.spm"
        $targetVocabPath = $sourceVocabPath
        $vocabArtifacts = @((New-Artifact $sourceVocab $sourceVocabPath))
    } else {
        $sourceVocab = Select-OneFile $generation 'srcvocab'
        $targetVocab = Select-OneFile $generation 'trgvocab'
        $sourceVocabPath = "$prefix/source-vocab.spm"
        $targetVocabPath = "$prefix/target-vocab.spm"
        $vocabArtifacts = @(
            (New-Artifact $sourceVocab $sourceVocabPath),
            (New-Artifact $targetVocab $targetVocabPath)
        )
    }
    return [ordered]@{
        Descriptor = [ordered]@{
            source = $SourceRust
            target = $TargetRust
            model_file = "$prefix/model.bin"
            source_vocab_file = $sourceVocabPath
            target_vocab_file = $targetVocabPath
            shortlist_file = "$prefix/lex.bin"
        }
        Artifacts = @(
            (New-Artifact $model "$prefix/model.bin")
        ) + $vocabArtifacts + @(
            (New-Artifact $lex "$prefix/lex.bin")
        )
        Upstream = [ordered]@{
            version = [string]$generation[0].version
            architecture = [string]$generation[0].architecture
        }
    }
}

if ($InputPath) {
    $source = Get-Content -LiteralPath $InputPath -Raw | ConvertFrom-Json
} else {
    $source = Invoke-RestMethod -Uri $SourceUri -TimeoutSec 60
}
$records = if ($source.data) { @($source.data) } else { @($source) }
if ($records.Count -lt 1) {
    throw 'Firefox translation catalog returned no records'
}

$existing = Get-Content -LiteralPath $OutputPath -Raw | ConvertFrom-Json
$qwen = @($existing.packs | Where-Object tier -ne 'compact')
$compact = foreach ($language in $languages) {
    $forward = New-Direction $records 'en' $language.Code 'english' $language.Rust
    $reverse = New-Direction $records $language.Code 'en' $language.Rust 'english'
    [ordered]@{
        id = $language.Id
        display_name = "$($language.Name) ↔ English"
        description = 'Small CPU-only language package for fast everyday translation.'
        tier = 'compact'
        license = 'MPL-2.0'
        recommended_locales = @($language.Locale)
        backend = [ordered]@{
            kind = 'firefox'
            directions = @($forward.Descriptor, $reverse.Descriptor)
        }
        artifacts = @($forward.Artifacts) + @($reverse.Artifacts)
        languages = @('english', $language.Rust)
    }
    Write-Host ("{0}: {1}->{2} {3}/{4}; {2}->{1} {5}/{6}" -f
        $language.Id, 'en', $language.Code,
        $forward.Upstream.version, $forward.Upstream.architecture,
        $reverse.Upstream.version, $reverse.Upstream.architecture)
}

$catalog = [ordered]@{
    schema_version = 4
    packs = @($compact) + $qwen
}
$json = $catalog | ConvertTo-Json -Depth 20
$temporary = "$OutputPath.tmp"
[System.IO.File]::WriteAllText($temporary, $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
Move-Item -LiteralPath $temporary -Destination $OutputPath -Force
Write-Host "Wrote $($compact.Count) compact packs and $($qwen.Count) contextual packs to $OutputPath"
