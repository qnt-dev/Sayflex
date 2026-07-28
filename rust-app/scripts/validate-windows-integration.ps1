[CmdletBinding()]
param(
    [string]$InputHost = (
        Join-Path $PSScriptRoot '..\target\debug\rust-input-host.exe'
    )
)

$ErrorActionPreference = 'Stop'
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$inputHostPath = [System.IO.Path]::GetFullPath($InputHost)
if (-not (Test-Path -LiteralPath $inputHostPath)) {
    throw "Input host was not found: $inputHostPath"
}

$originalClipboard = Get-Clipboard -Raw -ErrorAction SilentlyContinue
$env:QNTFLEX_RUN_SELECTION_SMOKE = '1'
$env:QNTFLEX_SELECTION_SMOKE_HOST_PATH = $inputHostPath

function Invoke-SelectionFixture {
    param(
        [Parameter(Mandatory)][string]$Script,
        [Parameter(Mandatory)][string]$Output,
        [Parameter(Mandatory)][string]$ExpectedText,
        [Parameter(Mandatory)][string]$ExpectedMethod,
        [Parameter(Mandatory)][string]$ClipboardMarker
    )

    $env:QNTFLEX_SELECTION_SMOKE_CLIPBOARD_MARKER = $ClipboardMarker
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

try {
    $results = @(
        Invoke-SelectionFixture `
            -Script 'tests\fixtures\selection-uia-fixture.ps1' `
            -Output 'target\uia-smoke.out' `
            -ExpectedText 'QNTFLEX_UIA_SELECTION_TEXT' `
            -ExpectedMethod 'UiAutomation' `
            -ClipboardMarker 'QNTFLEX_UIA_CLIPBOARD_SENTINEL'
        Invoke-SelectionFixture `
            -Script 'tests\fixtures\selection-fixture.ps1' `
            -Output 'target\window-message-smoke.out' `
            -ExpectedText 'QNTFLEX_SELECTION_FIXTURE_TEXT' `
            -ExpectedMethod 'WindowMessage' `
            -ClipboardMarker 'QNTFLEX_WINDOW_MESSAGE_CLIPBOARD_SENTINEL'
        Invoke-SelectionFixture `
            -Script 'tests\fixtures\selection-shortcut-fixture.ps1' `
            -Output 'target\shortcut-smoke.out' `
            -ExpectedText 'QNTFLEX_SHORTCUT_SELECTION_TEXT' `
            -ExpectedMethod 'Shortcut(ControlC)' `
            -ClipboardMarker 'QNTFLEX_SHORTCUT_CLIPBOARD_SENTINEL'
    )
    $insertionClipboard = Get-Clipboard -Raw
    $env:QNTFLEX_RUN_INSERTION_SMOKE = '1'
    $env:QNTFLEX_INSERTION_SMOKE_HOST_PATH = $inputHostPath
    $env:QNTFLEX_INSERTION_SMOKE_TEXT = 'QNTFLEX_INSERT_LAST_SMOKE'
    & powershell.exe -NoProfile -STA -ExecutionPolicy Bypass -File (
        Join-Path $rustRoot 'tests\fixtures\insertion-fixture.ps1'
    )
    if ($LASTEXITCODE -ne 0) {
        throw "insertion fixture exited with code $LASTEXITCODE"
    }
    $inserted = Get-Content -LiteralPath (
        Join-Path $rustRoot 'target\insertion-smoke.txt'
    ) -Raw
    if ($inserted -ne $env:QNTFLEX_INSERTION_SMOKE_TEXT) {
        throw "insertion fixture received unexpected text: $inserted"
    }
    $insertionEvent = (
        Get-Content -LiteralPath (
            Join-Path $rustRoot 'target\insertion-smoke.out'
        ) -Raw
    ) | ConvertFrom-Json
    if (-not $insertionEvent.clipboard_restored) {
        throw 'insertion service did not report clipboard restoration'
    }
    if ((Get-Clipboard -Raw) -ne $insertionClipboard) {
        throw 'insertion fixture did not restore the clipboard'
    }
    $results += [pscustomobject]@{
        Fixture = 'insertion-fixture.ps1'
        Method = $insertionEvent.method
        Characters = $inserted.Length
        Milliseconds = '-'
        ClipboardRestored = $true
    }
    $results | Format-Table -AutoSize
    Write-Host 'Windows selected-text integration validation passed.'
}
finally {
    if ($null -ne $originalClipboard) {
        Set-Clipboard -Value $originalClipboard
    }
    Remove-Item Env:QNTFLEX_RUN_SELECTION_SMOKE -ErrorAction SilentlyContinue
    Remove-Item Env:QNTFLEX_SELECTION_SMOKE_HOST_PATH -ErrorAction SilentlyContinue
    Remove-Item Env:QNTFLEX_SELECTION_SMOKE_CLIPBOARD_MARKER -ErrorAction SilentlyContinue
    Remove-Item Env:QNTFLEX_RUN_INSERTION_SMOKE -ErrorAction SilentlyContinue
    Remove-Item Env:QNTFLEX_INSERTION_SMOKE_HOST_PATH -ErrorAction SilentlyContinue
    Remove-Item Env:QNTFLEX_INSERTION_SMOKE_TEXT -ErrorAction SilentlyContinue
}
