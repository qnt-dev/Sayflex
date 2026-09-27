[CmdletBinding()]
param(
    [string]$ResultPath,
    [switch]$PipePressure,
    [int]$ExpectedInputBytes = 0
)

if ($PipePressure) {
    $payload = 'X' * (256 * 1024)
    [Console]::Out.Write($payload)
    [Console]::Out.Flush()
    [Console]::Error.Write($payload)
    [Console]::Error.Flush()
    $received = [System.IO.MemoryStream]::new()
    [Console]::OpenStandardInput().CopyTo($received)
    if ($received.Length -ne $ExpectedInputBytes) {
        exit 23
    }
    exit 0
}

if ([string]::IsNullOrWhiteSpace($ResultPath)) {
    throw 'ResultPath is required outside PipePressure mode.'
}

$Host.UI.RawUI.WindowTitle = 'SayFlex WezTerm Insertion E2E'
Clear-Host
Write-Host 'Waiting for Ctrl+Alt+V insertion...'

$buffer = [System.Text.StringBuilder]::new()
$deadline = [DateTime]::UtcNow.AddSeconds(20)
$lastInput = $null
while ([DateTime]::UtcNow -lt $deadline) {
    while ([Console]::KeyAvailable) {
        $key = [Console]::ReadKey($true)
        if ($key.KeyChar -ne [char]0) {
            [void]$buffer.Append($key.KeyChar)
            $lastInput = [DateTime]::UtcNow
        }
    }
    if (
        $buffer.Length -gt 0 -and
        $null -ne $lastInput -and
        [DateTime]::UtcNow -ge $lastInput.AddMilliseconds(750)
    ) {
        break
    }
    Start-Sleep -Milliseconds 10
}

[System.IO.File]::WriteAllText($ResultPath, $buffer.ToString())
