[CmdletBinding()]
param(
    [string]$InputHost = (
        Join-Path $PSScriptRoot '..\target\debug\sayflex-input-host.exe'
    ),
    [switch]$InsertionOnly
)

$ErrorActionPreference = 'Stop'
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
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
        ClipboardRestored = $true
    }
}

function Invoke-InsertionFixture {
    param(
        [Parameter(Mandatory)][string]$Text,
        [Parameter(Mandatory)][string]$ClipboardMarker,
        [int]$KeyDownDelayMs = 0,
        [switch]$RequirePreserved
    )

    Set-Clipboard -Value $ClipboardMarker
    $env:SAYFLEX_RUN_CLIPBOARD_LEASE_SMOKE = '1'
    $env:SAYFLEX_INSERTION_SMOKE_HOST_PATH = $inputHostPath
    $env:SAYFLEX_INSERTION_SMOKE_TEXT = $Text
    if ($KeyDownDelayMs -gt 0) {
        $env:SAYFLEX_INSERTION_SMOKE_KEYDOWN_DELAY_MS = [string]$KeyDownDelayMs
    } else {
        Remove-Item Env:SAYFLEX_INSERTION_SMOKE_KEYDOWN_DELAY_MS -ErrorAction SilentlyContinue
    }
    & powershell.exe -NoProfile -STA -ExecutionPolicy Bypass -File (
        Join-Path $rustRoot 'tests\fixtures\insertion-fixture.ps1'
    )
    if ($LASTEXITCODE -ne 0) {
        throw "insertion fixture exited with code $LASTEXITCODE"
    }
    $inserted = Get-Content -LiteralPath (
        Join-Path $rustRoot 'target\insertion-smoke.txt'
    ) -Raw
    if ($inserted -ne $Text) {
        throw "insertion fixture received unexpected text: $inserted"
    }
    $event = (
        Get-Content -LiteralPath (
            Join-Path $rustRoot 'target\insertion-smoke.out'
        ) -Raw
    ) | ConvertFrom-Json
    if ($event.clipboard_restored -eq $event.clipboard_preserved) {
        throw 'insertion service reported an invalid clipboard lease disposition'
    }
    if ($RequirePreserved -and -not $event.clipboard_preserved) {
        throw 'delayed insertion restored the old clipboard without a target read acknowledgement'
    }
    $expectedClipboard = if ($event.clipboard_restored) { $ClipboardMarker } else { $Text }
    if ((Get-Clipboard -Raw) -ne $expectedClipboard) {
        throw 'clipboard contents do not match the reported lease disposition'
    }
    [pscustomobject]@{
        Fixture = if ($KeyDownDelayMs -gt 0) { 'insertion-delayed-read' } else { 'insertion-immediate' }
        Method = 'ClipboardLease'
        Characters = $inserted.Length
        Milliseconds = '-'
        ClipboardDisposition = if ($event.clipboard_restored) { 'RestoredAfterTargetRead' } else { 'PreservedUnverified' }
    }
}

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
        -Text 'SAYFLEX_INSERT_LAST_SMOKE' `
        -ClipboardMarker 'SAYFLEX_IMMEDIATE_CLIPBOARD_SENTINEL'
    $results += Invoke-InsertionFixture `
        -Text 'SAYFLEX_DELAYED_PASTE_SMOKE' `
        -ClipboardMarker 'SAYFLEX_DELAYED_CLIPBOARD_SENTINEL' `
        -KeyDownDelayMs 1200 `
        -RequirePreserved
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
    Remove-Item Env:SAYFLEX_RUN_CLIPBOARD_LEASE_SMOKE -ErrorAction SilentlyContinue
    Remove-Item Env:SAYFLEX_INSERTION_SMOKE_HOST_PATH -ErrorAction SilentlyContinue
    Remove-Item Env:SAYFLEX_INSERTION_SMOKE_TEXT -ErrorAction SilentlyContinue
    Remove-Item Env:SAYFLEX_INSERTION_SMOKE_KEYDOWN_DELAY_MS -ErrorAction SilentlyContinue
}
