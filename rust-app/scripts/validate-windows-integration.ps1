[CmdletBinding()]
param(
    [string]$InputHost,
    [switch]$InsertionOnly
)

$ErrorActionPreference = 'Stop'
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$InputHost = if ([string]::IsNullOrWhiteSpace($InputHost)) {
    Join-Path $rustRoot 'target\debug\sayflex-input-host.exe'
} else {
    $InputHost
}
$inputHostPath = [System.IO.Path]::GetFullPath($InputHost)
if (-not (Test-Path -LiteralPath $inputHostPath)) {
    throw "Input host was not found: $inputHostPath"
}

$originalClipboard = Get-Clipboard -Raw -ErrorAction SilentlyContinue
$env:SAYFLEX_RUN_SELECTION_SMOKE = '1'
$env:SAYFLEX_SELECTION_SMOKE_HOST_PATH = $inputHostPath

function Invoke-SelectionFixture {
    param(
        [Parameter(Mandatory)][string]$Script,
        [Parameter(Mandatory)][string]$Output,
        [Parameter(Mandatory)][string]$ExpectedText,
        [Parameter(Mandatory)][string]$ExpectedMethod,
        [Parameter(Mandatory)][string]$ClipboardMarker
    )

    $env:SAYFLEX_SELECTION_SMOKE_CLIPBOARD_MARKER = $ClipboardMarker
    & powershell.exe -NoProfile -STA -ExecutionPolicy Bypass -File (
        Join-Path $rustRoot $Script
    )
    if ($LASTEXITCODE -ne 0) {
        throw "$Script exited with code $LASTEXITCODE"
    }

    $outputPath = Join-Path $rustRoot $Output
    if (-not (Test-Path -LiteralPath $outputPath)) {
        throw "$Script did not produce $outputPath"
    }
    $raw = Get-Content -LiteralPath $outputPath -Raw
    if ([string]::IsNullOrWhiteSpace($raw)) {
        throw "$Script did not capture selected text"
    }
    $result = $raw | ConvertFrom-Json
    if ($result.text -ne $ExpectedText) {
        throw "$Script returned unexpected text: $($result.text)"
    }
    if ($result.method -ne $ExpectedMethod) {
        throw "$Script used $($result.method), expected $ExpectedMethod"
    }
    if ((Get-Clipboard -Raw) -ne $ClipboardMarker) {
        throw "$Script did not restore the original clipboard text"
    }
    [pscustomobject]@{
        Fixture = [System.IO.Path]::GetFileName($Script)
        Method = $result.method
        Characters = $result.text.Length
        Milliseconds = $result.elapsed_ms
        ClipboardDisposition = 'Restored'
    }
}

function Invoke-InsertionFixture {
    param(
        [Parameter(Mandatory)][string]$Text,
        [Parameter(Mandatory)][string]$ClipboardMarker,
        [int]$InputDelayMs = 0,
        [switch]$ForceUnicode,
        [switch]$ObserveClipboard
    )

    Set-Clipboard -Value $ClipboardMarker
    $env:SAYFLEX_RUN_INSERTION_SMOKE = '1'
    $env:SAYFLEX_INSERTION_SMOKE_HOST_PATH = $inputHostPath
    $env:SAYFLEX_INSERTION_SMOKE_TEXT = $Text
    if ($InputDelayMs -gt 0) {
        $env:SAYFLEX_INSERTION_SMOKE_INPUT_DELAY_MS = [string]$InputDelayMs
    } else {
        Remove-Item Env:SAYFLEX_INSERTION_SMOKE_INPUT_DELAY_MS -ErrorAction SilentlyContinue
    }
    if ($ForceUnicode) { $env:SAYFLEX_INSERTION_SMOKE_FORCE_UNICODE = '1' }
    else { Remove-Item Env:SAYFLEX_INSERTION_SMOKE_FORCE_UNICODE -ErrorAction SilentlyContinue }
    if ($ObserveClipboard) { $env:SAYFLEX_INSERTION_SMOKE_OBSERVE_CLIPBOARD = '1' }
    else { Remove-Item Env:SAYFLEX_INSERTION_SMOKE_OBSERVE_CLIPBOARD -ErrorAction SilentlyContinue }
    & powershell.exe -NoProfile -STA -ExecutionPolicy Bypass -File (
        Join-Path $rustRoot 'tests\fixtures\insertion-fixture.ps1'
    )
    if ($LASTEXITCODE -ne 0) {
        throw "insertion fixture exited with code $LASTEXITCODE"
    }
    $inserted = [System.IO.File]::ReadAllText(
        (Join-Path $rustRoot 'target\insertion-smoke.txt'),
        [Text.Encoding]::UTF8
    )
    if ($inserted -ne $Text) {
        throw "insertion fixture received unexpected text: $inserted"
    }
    $event = (
        Get-Content -LiteralPath (
            Join-Path $rustRoot 'target\insertion-smoke.out'
        ) -Raw
    ) | ConvertFrom-Json
    if (-not $event.clipboard_untouched) {
        throw 'automatic insertion reported that it borrowed the clipboard'
    }
    if ((Get-Clipboard -Raw) -ne $ClipboardMarker) {
        throw 'automatic insertion changed the user clipboard'
    }
    [pscustomobject]@{
        Fixture = if ($InputDelayMs -gt 0) { 'insertion-delayed-target' } else { 'insertion-immediate' }
        Method = $event.method
        Characters = $inserted.Length
        Milliseconds = $event.dispatch_duration_ms
        ClipboardDisposition = 'Untouched'
    }
}

$regressionClipboardMarker = [Text.Encoding]::UTF8.GetString(
    [Convert]::FromBase64String('0JfQsNC60LDQtyA20JEtMDE3Njk4ODMx0L7RgiAyOS4wNy4yMDI2')
)
$multilingualInsertionText = [Text.Encoding]::UTF8.GetString(
    [Convert]::FromBase64String('U0FZRkxFWF9JTlNFUlRfTEFTVF9TTU9LRQ0K0J/RgNGP0LzQsNGPINCy0YHRgtCw0LLQutCw')
)
$longUnicodeInsertionText = ('SayFlex long Unicode input 0123456789. ' * 26).TrimEnd()

try {
    $results = @()
    if (-not $InsertionOnly) {
        $results += Invoke-SelectionFixture `
                -Script 'tests\fixtures\selection-uia-fixture.ps1' `
                -Output 'target\uia-smoke.out' `
                -ExpectedText 'SAYFLEX_UIA_SELECTION_TEXT' `
                -ExpectedMethod 'UiAutomation' `
                -ClipboardMarker 'SAYFLEX_UIA_CLIPBOARD_SENTINEL'
        $results += Invoke-SelectionFixture `
                -Script 'tests\fixtures\selection-fixture.ps1' `
                -Output 'target\window-message-smoke.out' `
                -ExpectedText 'SAYFLEX_SELECTION_FIXTURE_TEXT' `
                -ExpectedMethod 'WindowMessage' `
                -ClipboardMarker 'SAYFLEX_WINDOW_MESSAGE_CLIPBOARD_SENTINEL'
        $results += Invoke-SelectionFixture `
                -Script 'tests\fixtures\selection-shortcut-fixture.ps1' `
                -Output 'target\shortcut-smoke.out' `
                -ExpectedText 'SAYFLEX_SHORTCUT_SELECTION_TEXT' `
                -ExpectedMethod 'Shortcut(ControlC)' `
                -ClipboardMarker 'SAYFLEX_SHORTCUT_CLIPBOARD_SENTINEL'
    }
    $results += Invoke-InsertionFixture `
        -Text $multilingualInsertionText `
        -ClipboardMarker $regressionClipboardMarker `
        -ObserveClipboard
    $results += Invoke-InsertionFixture `
        -Text 'SAYFLEX_DELAYED_UNICODE_INPUT' `
        -ClipboardMarker 'SAYFLEX_DELAYED_CLIPBOARD_SENTINEL' `
        -InputDelayMs 1200 `
        -ForceUnicode `
        -ObserveClipboard
    $results += Invoke-InsertionFixture `
        -Text $longUnicodeInsertionText `
        -ClipboardMarker 'SAYFLEX_LONG_INPUT_CLIPBOARD_SENTINEL' `
        -ForceUnicode `
        -ObserveClipboard
    $results | Format-Table -AutoSize
    Write-Host 'Windows selected-text integration validation passed.'
}
finally {
    if ($null -ne $originalClipboard) {
        Set-Clipboard -Value $originalClipboard
    }
    Remove-Item Env:SAYFLEX_RUN_SELECTION_SMOKE -ErrorAction SilentlyContinue
    Remove-Item Env:SAYFLEX_SELECTION_SMOKE_HOST_PATH -ErrorAction SilentlyContinue
    Remove-Item Env:SAYFLEX_SELECTION_SMOKE_CLIPBOARD_MARKER -ErrorAction SilentlyContinue
    Remove-Item Env:SAYFLEX_RUN_INSERTION_SMOKE -ErrorAction SilentlyContinue
    Remove-Item Env:SAYFLEX_INSERTION_SMOKE_HOST_PATH -ErrorAction SilentlyContinue
    Remove-Item Env:SAYFLEX_INSERTION_SMOKE_TEXT -ErrorAction SilentlyContinue
    Remove-Item Env:SAYFLEX_INSERTION_SMOKE_INPUT_DELAY_MS -ErrorAction SilentlyContinue
    Remove-Item Env:SAYFLEX_INSERTION_SMOKE_FORCE_UNICODE -ErrorAction SilentlyContinue
    Remove-Item Env:SAYFLEX_INSERTION_SMOKE_OBSERVE_CLIPBOARD -ErrorAction SilentlyContinue
}
