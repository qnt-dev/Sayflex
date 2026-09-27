$ErrorActionPreference = 'Stop'

$module = Join-Path $PSScriptRoot '..\release-validation.psm1'
Import-Module $module -Force

Describe 'SayFlex delayed clipboard image installed fixture' {
    It 'restores the exact delayed image while input-host remains responsive' `
        -Skip:($env:SAYFLEX_RUN_DELAYED_IMAGE_SMOKE -ne '1') {
        $receiptPath = Join-Path $TestDrive 'delayed-image-live.json'
        $inputHost = if ($env:SAYFLEX_DELAYED_IMAGE_INPUT_HOST) {
            $env:SAYFLEX_DELAYED_IMAGE_INPUT_HOST
        } else {
            Join-Path $PSScriptRoot '..\..\target\debug\sayflex-input-host.exe'
        }

        & powershell.exe -NoProfile -STA -ExecutionPolicy Bypass `
            -File (Join-Path $PSScriptRoot '..\test-delayed-clipboard-image.ps1') `
            -InputHostPath $inputHost `
            -Consumers FixtureConsumer `
            -DelaySecondsCsv '0,1,10,60' `
            -ReceiptPath $receiptPath

        $LASTEXITCODE | Should Be 0
        Test-Path -LiteralPath $receiptPath -PathType Leaf | Should Be $true
        $receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
        $failures = @(
            Get-DelayedClipboardImageReceiptFailures `
                -Receipt $receipt `
                -ExpectedDelays @(0, 1, 10, 60) `
                -ExpectedConsumers @('FixtureConsumer')
        )
        $failures | Should BeNullOrEmpty
    }
}
