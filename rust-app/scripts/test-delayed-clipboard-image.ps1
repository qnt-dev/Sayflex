[CmdletBinding()]
param(
    [string]$InputHostPath = (Join-Path $env:LOCALAPPDATA 'Programs\SayFlex\SayFlex.InputHost.exe'),
    [string[]]$Consumers = @('FixtureConsumer'),
    [int[]]$DelaySeconds = @(0, 1, 10, 60),
    [string]$DelaySecondsCsv,
    [string]$ReceiptPath,
    [string]$ValidateReceiptPath
)

$ErrorActionPreference = 'Stop'
$validationModule = Join-Path $PSScriptRoot 'release-validation.psm1'
Import-Module $validationModule -Force

if (-not [string]::IsNullOrWhiteSpace($DelaySecondsCsv)) {
    $DelaySeconds = @($DelaySecondsCsv.Split(',') | ForEach-Object {
        $parsed = 0
        if (-not [int]::TryParse($_.Trim(), [ref]$parsed)) {
            throw "DelaySecondsCsv contains an invalid integer: '$_'"
        }
        $parsed
    })
}

if (-not [string]::IsNullOrWhiteSpace($ValidateReceiptPath)) {
    $resolved = [System.IO.Path]::GetFullPath($ValidateReceiptPath)
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        throw "Delayed-image receipt was not found: $resolved"
    }
    $receipt = Get-Content -LiteralPath $resolved -Raw | ConvertFrom-Json
    $failures = @(
        Get-DelayedClipboardImageReceiptFailures `
            -Receipt $receipt `
            -ExpectedDelays $DelaySeconds `
            -ExpectedConsumers $Consumers
    )
    if ($failures.Count -gt 0) {
        throw "Delayed-image receipt is invalid: $($failures -join '; ')"
    }
    Write-Host "Delayed-image receipt is valid: $resolved"
    exit 0
}

if ([Threading.Thread]::CurrentThread.GetApartmentState() -ne [Threading.ApartmentState]::STA) {
    throw 'The live delayed-image smoke must run in powershell.exe -STA.'
}
if ($Consumers.Count -ne 1 -or $Consumers[0] -cne 'FixtureConsumer') {
    throw 'Only the isolated FixtureConsumer is automated. Firefox and Telegram remain witnessed interactive checks.'
}
if (@($DelaySeconds | Where-Object { $_ -lt 0 }).Count -gt 0) {
    throw 'DelaySeconds cannot contain a negative value.'
}

$inputHost = [System.IO.Path]::GetFullPath($InputHostPath)
if (-not (Test-Path -LiteralPath $inputHost -PathType Leaf)) {
    throw "Input host was not found: $inputHost"
}
if ([string]::IsNullOrWhiteSpace($ReceiptPath)) {
    $ReceiptPath = Join-Path ([System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))) 'dist\delayed-clipboard-image-receipt.json'
}
$receiptFile = [System.IO.Path]::GetFullPath($ReceiptPath)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class SayFlexDelayedImageNative
{
    private const uint WM_NULL = 0;
    private const uint SMTO_ABORTIFHUNG = 0x0002;

    [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    [DllImport("kernel32.dll")] private static extern uint GetCurrentThreadId();
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);
    [DllImport("user32.dll")] private static extern bool AttachThreadInput(uint source, uint target, bool attach);
    [DllImport("user32.dll")] private static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] private static extern IntPtr GetKeyboardLayout(uint threadId);
    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr SendMessageTimeout(
        IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam,
        uint flags, uint timeout, out IntPtr result);

    public static uint WindowThreadId(IntPtr hwnd, out uint processId)
    {
        return GetWindowThreadProcessId(hwnd, out processId);
    }

    public static ulong KeyboardLayout(uint threadId)
    {
        return unchecked((ulong)GetKeyboardLayout(threadId).ToInt64());
    }

    public static void Focus(IntPtr hwnd)
    {
        uint ignored;
        uint current = GetCurrentThreadId();
        uint foreground = GetWindowThreadProcessId(GetForegroundWindow(), out ignored);
        bool attached = foreground != 0 && foreground != current && AttachThreadInput(current, foreground, true);
        try { SetForegroundWindow(hwnd); }
        finally { if (attached) AttachThreadInput(current, foreground, false); }
    }

    public static bool IsResponsive(IntPtr hwnd, uint timeoutMilliseconds)
    {
        IntPtr result;
        return SendMessageTimeout(
            hwnd, WM_NULL, IntPtr.Zero, IntPtr.Zero,
            SMTO_ABORTIFHUNG, timeoutMilliseconds, out result) != IntPtr.Zero;
    }

    public static int ExitCode(IntPtr process)
    {
        uint exitCode;
        if (!GetExitCodeProcess(process, out exitCode)) {
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        }
        return unchecked((int)exitCode);
    }
}
'@

function Wait-ForFile {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][System.Diagnostics.Process]$Process,
        [int]$TimeoutSeconds = 20,
        [int64]$MinimumBytes = 1,
        [scriptblock]$WhileWaiting
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ((Test-Path -LiteralPath $Path -PathType Leaf) -and
            (Get-Item -LiteralPath $Path).Length -ge $MinimumBytes) { return }
        if ($Process.HasExited) {
            throw "Process $($Process.Id) exited with code $($Process.ExitCode) before producing $Path"
        }
        if ($null -ne $WhileWaiting) { & $WhileWaiting }
        Start-Sleep -Milliseconds 25
    }
    throw "Timed out waiting for $Path"
}

function New-KnownImage {
    param([Parameter(Mandatory)][string]$Path)
    $bitmap = [System.Drawing.Bitmap]::new(64, 64, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.Clear([System.Drawing.Color]::FromArgb(255, 17, 24, 32))
        $graphics.FillRectangle([System.Drawing.Brushes]::Crimson, 4, 4, 24, 24)
        $graphics.FillEllipse([System.Drawing.Brushes]::Cyan, 30, 6, 28, 28)
        $graphics.FillRectangle([System.Drawing.Brushes]::Gold, 12, 38, 44, 18)
        $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Bmp)
    } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

function New-CompetitiveImage {
    param([Parameter(Mandatory)][string]$Path)
    $bitmap = [System.Drawing.Bitmap]::new(64, 64, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.Clear([System.Drawing.Color]::FromArgb(255, 245, 245, 245))
        $graphics.FillRectangle([System.Drawing.Brushes]::DarkViolet, 3, 3, 58, 18)
        $graphics.FillEllipse([System.Drawing.Brushes]::LimeGreen, 7, 25, 24, 31)
        $graphics.FillRectangle([System.Drawing.Brushes]::Navy, 35, 27, 25, 29)
        $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Bmp)
    } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

function Set-CompetitiveClipboard {
    param(
        [Parameter(Mandatory)][string]$ImagePath,
        [Parameter(Mandatory)][string]$Text
    )
    $image = [System.Drawing.Bitmap]::FromFile([System.IO.Path]::GetFullPath($ImagePath))
    try {
        $data = [System.Windows.Forms.DataObject]::new()
        $data.SetData([System.Windows.Forms.DataFormats]::UnicodeText, $true, $Text)
        $data.SetData([System.Windows.Forms.DataFormats]::Bitmap, $false, $image)
        [System.Windows.Forms.Clipboard]::SetDataObject($data, $true, 10, 100)
    } finally {
        $image.Dispose()
    }
}

function Copy-ClipboardDataObject {
    $current = [System.Windows.Forms.Clipboard]::GetDataObject()
    if ($null -eq $current) { return $null }
    $copy = [System.Windows.Forms.DataObject]::new()
    foreach ($format in @($current.GetFormats($false))) {
        try {
            $value = $current.GetData($format, $false)
            if ($null -ne $value) { $copy.SetData($format, $false, $value) }
        } catch {
            Write-Warning "Could not preserve clipboard format '$format': $($_.Exception.Message)"
        }
    }
    return $copy
}

function Invoke-DelayedImageCase {
    param(
        [Parameter(Mandatory)][int]$Delay,
        [Parameter(Mandatory)][string]$CaseRoot,
        [Parameter(Mandatory)][string]$KnownImage,
        [Parameter(Mandatory)][string]$KnownHash
    )

    $ready = Join-Path $CaseRoot 'provider.ready'
    $providerStop = Join-Path $CaseRoot 'provider.stop'
    $providerError = Join-Path $CaseRoot 'provider.err'
    $providerLaunchError = Join-Path $CaseRoot 'provider-launch.err'
    $hostRelease = Join-Path $CaseRoot 'input-host.release'
    $hostOutput = Join-Path $CaseRoot 'input-host.out'
    $hostError = Join-Path $CaseRoot 'input-host.err'
    $consumerImage = Join-Path $CaseRoot 'received.bmp'
    $consumerResult = Join-Path $CaseRoot 'consumer.json'
    $consumerError = Join-Path $CaseRoot 'consumer.err'
    $provider = $null
    $hostProcess = $null
    $consumer = $null
    $form = $null

    try {
        $provider = Start-Process -FilePath powershell.exe -ArgumentList @(
            '-NoProfile', '-STA', '-ExecutionPolicy', 'Bypass',
            '-File', (Join-Path $PSScriptRoot '..\tests\fixtures\delayed-clipboard-image-owner.ps1'),
            '-ImagePath', $KnownImage,
            '-ReadyPath', $ready,
            '-StopPath', $providerStop,
            '-ErrorPath', $providerError
        ) -RedirectStandardError $providerLaunchError -WindowStyle Hidden -PassThru
        $providerHandle = $provider.Handle
        Wait-ForFile -Path $ready -Process $provider

        $form = [System.Windows.Forms.Form]::new()
        $form.Text = 'SayFlex delayed clipboard image fixture'
        $form.StartPosition = [System.Windows.Forms.FormStartPosition]::Manual
        $form.Location = [System.Drawing.Point]::new(80, 80)
        $form.Size = [System.Drawing.Size]::new(640, 220)
        $form.TopMost = $true
        $editor = [System.Windows.Forms.TextBox]::new()
        $editor.Multiline = $true
        $editor.Dock = [System.Windows.Forms.DockStyle]::Fill
        $form.Controls.Add($editor)
        $form.Show() | Out-Null
        $form.BringToFront() | Out-Null
        $form.Activate() | Out-Null
        $editor.Focus() | Out-Null
        [System.Windows.Forms.Application]::DoEvents()
        [SayFlexDelayedImageNative]::Focus($form.Handle)
        Start-Sleep -Milliseconds 250

        $savedEnvironment = @{}
        foreach ($name in @(
            'SAYFLEX_RUN_INSERTION_SMOKE',
            'SAYFLEX_INSERTION_SMOKE_HWND',
            'SAYFLEX_INSERTION_SMOKE_TEXT',
            'SAYFLEX_INSERTION_SMOKE_STABLE_FOCUS_MS',
            'SAYFLEX_INSERTION_SMOKE_FORCE_CLIPBOARD_TRANSACTION',
            'SAYFLEX_INSERTION_SMOKE_RELEASE_FILE',
            'SAYFLEX_INSERTION_SMOKE_RELEASE_TIMEOUT_MS'
        )) {
            $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        }
        try {
            $env:SAYFLEX_RUN_INSERTION_SMOKE = '1'
            $env:SAYFLEX_INSERTION_SMOKE_HWND = [string]$form.Handle.ToInt64()
            $env:SAYFLEX_INSERTION_SMOKE_TEXT = 'SAYFLEX_DELAYED_IMAGE_PROBE'
            $env:SAYFLEX_INSERTION_SMOKE_STABLE_FOCUS_MS = '150'
            $env:SAYFLEX_INSERTION_SMOKE_FORCE_CLIPBOARD_TRANSACTION = '1'
            $env:SAYFLEX_INSERTION_SMOKE_RELEASE_FILE = $hostRelease
            $env:SAYFLEX_INSERTION_SMOKE_RELEASE_TIMEOUT_MS = '120000'
            $hostProcess = Start-Process -FilePath $inputHost -ArgumentList '--insertion-smoke' `
                -RedirectStandardOutput $hostOutput -RedirectStandardError $hostError `
                -WindowStyle Hidden -PassThru
            $hostHandle = $hostProcess.Handle
        } finally {
            foreach ($name in $savedEnvironment.Keys) {
                [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
            }
        }

        Wait-ForFile -Path $hostOutput -Process $hostProcess -TimeoutSeconds 30 -WhileWaiting {
            [SayFlexDelayedImageNative]::Focus($form.Handle)
            $form.BringToFront() | Out-Null
            $form.Activate() | Out-Null
            $editor.Focus() | Out-Null
            [System.Windows.Forms.Application]::DoEvents()
        }
        $hostEvent = Get-Content -LiteralPath $hostOutput -Raw | ConvertFrom-Json
        if ($hostEvent.method -ne 'ClipboardTransaction' -or
            $hostEvent.clipboard_policy -ne 'transaction_restored') {
            throw "InputHost did not use and restore a clipboard transaction: $($hostEvent | ConvertTo-Json -Compress)"
        }
        if ($editor.Text -ne 'SAYFLEX_DELAYED_IMAGE_PROBE') {
            throw "The insertion target received unexpected text: '$($editor.Text)'"
        }

        $delayDeadline = [DateTime]::UtcNow.AddSeconds($Delay)
        while ([DateTime]::UtcNow -lt $delayDeadline) {
            [System.Windows.Forms.Application]::DoEvents()
            Start-Sleep -Milliseconds 25
        }
        $responsive = [SayFlexDelayedImageNative]::IsResponsive($form.Handle, 1000)

        $consumer = Start-Process -FilePath powershell.exe -ArgumentList @(
            '-NoProfile', '-STA', '-ExecutionPolicy', 'Bypass',
            '-File', (Join-Path $PSScriptRoot '..\tests\fixtures\delayed-clipboard-image-consumer.ps1'),
            '-OutputImagePath', $consumerImage,
            '-ResultPath', $consumerResult,
            '-ErrorPath', $consumerError
        ) -WindowStyle Hidden -PassThru
        $consumerHandle = $consumer.Handle
        if (-not $consumer.WaitForExit(10000)) {
            $responsive = $false
            $consumer.Kill()
            $consumer.WaitForExit()
            throw 'The delayed-image consumer did not complete within 10 seconds.'
        }
        $consumer.WaitForExit()
        $consumer.Refresh()
        $consumerExitCode = [SayFlexDelayedImageNative]::ExitCode($consumerHandle)
        if ($consumerExitCode -ne 0) {
            $detail = if (Test-Path -LiteralPath $consumerError) {
                Get-Content -LiteralPath $consumerError -Raw
            } else { 'no consumer error was recorded' }
            throw "The delayed-image consumer failed: $detail"
        }
        $consumerEvidence = Get-Content -LiteralPath $consumerResult -Raw | ConvertFrom-Json

        [System.IO.File]::WriteAllText($hostRelease, 'release')
        if (-not $hostProcess.WaitForExit(10000)) {
            $hostProcess.Kill()
            $hostProcess.WaitForExit()
            throw 'InputHost did not exit after the release signal.'
        }
        $hostProcess.WaitForExit()
        $hostProcess.Refresh()
        $hostExitCode = [SayFlexDelayedImageNative]::ExitCode($hostHandle)
        [System.IO.File]::WriteAllText($providerStop, 'stop')
        if (-not $provider.WaitForExit(10000)) {
            $provider.Kill()
            $provider.WaitForExit()
            throw 'The delayed-image provider did not exit after the stop signal.'
        }
        $provider.WaitForExit()
        $provider.Refresh()
        $providerExitCode = [SayFlexDelayedImageNative]::ExitCode($providerHandle)

        return [pscustomobject][ordered]@{
            provider_process = "powershell:$($provider.Id)"
            consumer_process = 'FixtureConsumer'
            delay_seconds = $Delay
            source_sha256 = $KnownHash
            received_sha256 = "$($consumerEvidence.sha256)"
            responsive = $responsive
            provider_probe_exit_code = $providerExitCode
            input_host_exit_code = $hostExitCode
            consumer_probe_exit_code = $consumerExitCode
            status = if (
                $responsive -and
                $KnownHash -eq "$($consumerEvidence.sha256)" -and
                $providerExitCode -eq 0 -and
                $hostExitCode -eq 0 -and
                $consumerExitCode -eq 0
            ) { 'PASS' } else { 'FAIL' }
        }
    } finally {
        if ($null -ne $form) { $form.Close(); $form.Dispose() }
        if ($null -ne $hostProcess -and -not $hostProcess.HasExited) {
            [System.IO.File]::WriteAllText($hostRelease, 'release')
            if (-not $hostProcess.WaitForExit(3000)) { $hostProcess.Kill(); $hostProcess.WaitForExit() }
        }
        if ($null -ne $provider -and -not $provider.HasExited) {
            [System.IO.File]::WriteAllText($providerStop, 'stop')
            if (-not $provider.WaitForExit(3000)) { $provider.Kill(); $provider.WaitForExit() }
        }
        if ($null -ne $consumer -and -not $consumer.HasExited) {
            $consumer.Kill()
            $consumer.WaitForExit()
        }
    }
}

function Invoke-CompetitiveCopyCase {
    param(
        [Parameter(Mandatory)][string]$CaseRoot,
        [Parameter(Mandatory)][string]$CompetitiveImage,
        [Parameter(Mandatory)][string]$CompetitiveHash
    )

    $payload = 'SAYFLEX_MATERIALIZATION_PROBE'
    $foreignText = 'USER_COPY_MUST_WIN'
    $hostOutput = Join-Path $CaseRoot 'competitive-host.out'
    $hostError = Join-Path $CaseRoot 'competitive-host.err'
    $consumerImage = Join-Path $CaseRoot 'competitive-received.bmp'
    $consumerResult = Join-Path $CaseRoot 'competitive-consumer.json'
    $consumerError = Join-Path $CaseRoot 'competitive-consumer.err'
    $hostProcess = $null
    $consumer = $null
    $form = $null

    try {
        $form = [System.Windows.Forms.Form]::new()
        $form.Text = 'SayFlex competitive clipboard fixture'
        $form.StartPosition = [System.Windows.Forms.FormStartPosition]::Manual
        $form.Location = [System.Drawing.Point]::new(80, 80)
        $form.Size = [System.Drawing.Size]::new(640, 220)
        $form.TopMost = $true
        $editor = [System.Windows.Forms.TextBox]::new()
        $editor.Multiline = $true
        $editor.Dock = [System.Windows.Forms.DockStyle]::Fill
        $form.Controls.Add($editor)
        $form.Show() | Out-Null
        $form.BringToFront() | Out-Null
        $form.Activate() | Out-Null
        $editor.Focus() | Out-Null
        [System.Windows.Forms.Application]::DoEvents()
        [SayFlexDelayedImageNative]::Focus($form.Handle)
        Start-Sleep -Milliseconds 250

        $savedEnvironment = @{}
        foreach ($name in @(
            'SAYFLEX_RUN_INSERTION_SMOKE',
            'SAYFLEX_INSERTION_SMOKE_HWND',
            'SAYFLEX_INSERTION_SMOKE_TEXT',
            'SAYFLEX_INSERTION_SMOKE_STABLE_FOCUS_MS',
            'SAYFLEX_INSERTION_SMOKE_FORCE_CLIPBOARD_TRANSACTION',
            'SAYFLEX_INSERTION_SMOKE_DISABLE_UIA_VERIFY',
            'SAYFLEX_INSERTION_SMOKE_RELEASE_FILE'
        )) {
            $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        }
        try {
            $env:SAYFLEX_RUN_INSERTION_SMOKE = '1'
            $env:SAYFLEX_INSERTION_SMOKE_HWND = [string]$form.Handle.ToInt64()
            $env:SAYFLEX_INSERTION_SMOKE_TEXT = $payload
            $env:SAYFLEX_INSERTION_SMOKE_STABLE_FOCUS_MS = '150'
            $env:SAYFLEX_INSERTION_SMOKE_FORCE_CLIPBOARD_TRANSACTION = '1'
            $env:SAYFLEX_INSERTION_SMOKE_DISABLE_UIA_VERIFY = '1'
            Remove-Item Env:SAYFLEX_INSERTION_SMOKE_RELEASE_FILE -ErrorAction SilentlyContinue
            $hostProcess = Start-Process -FilePath $inputHost -ArgumentList '--insertion-smoke' `
                -RedirectStandardOutput $hostOutput -RedirectStandardError $hostError `
                -WindowStyle Hidden -PassThru
            $hostHandle = $hostProcess.Handle
        } finally {
            foreach ($name in $savedEnvironment.Keys) {
                [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
            }
        }

        # Do not pump the target UI while polling. The target's queued paste
        # cannot consume the delayed payload before this independent process
        # materializes it and performs a newer Copy.
        $materialized = $false
        $deadline = [DateTime]::UtcNow.AddSeconds(10)
        while ([DateTime]::UtcNow -lt $deadline -and -not $hostProcess.HasExited) {
            try {
                if ([System.Windows.Forms.Clipboard]::GetText() -ceq $payload) {
                    $materialized = $true
                    break
                }
            } catch {
                # Clipboard contention is expected while the input host publishes.
            }
            Start-Sleep -Milliseconds 5
        }
        if (-not $materialized) {
            throw 'The competitive-copy fixture did not observe delayed materialization.'
        }

        Set-CompetitiveClipboard -ImagePath $CompetitiveImage -Text $foreignText
        if (-not $hostProcess.WaitForExit(10000)) {
            $hostProcess.Kill()
            $hostProcess.WaitForExit()
            throw 'InputHost did not terminate after its publication was superseded.'
        }
        $hostProcess.WaitForExit()
        $hostProcess.Refresh()
        $hostExitCode = [SayFlexDelayedImageNative]::ExitCode($hostHandle)

        $consumer = Start-Process -FilePath powershell.exe -ArgumentList @(
            '-NoProfile', '-STA', '-ExecutionPolicy', 'Bypass',
            '-File', (Join-Path $PSScriptRoot '..\tests\fixtures\delayed-clipboard-image-consumer.ps1'),
            '-OutputImagePath', $consumerImage,
            '-ResultPath', $consumerResult,
            '-ErrorPath', $consumerError
        ) -WindowStyle Hidden -PassThru
        $consumerHandle = $consumer.Handle
        if (-not $consumer.WaitForExit(10000)) {
            $consumer.Kill()
            $consumer.WaitForExit()
            throw 'The competitive-copy consumer did not complete within 10 seconds.'
        }
        $consumer.WaitForExit()
        $consumer.Refresh()
        $consumerExitCode = [SayFlexDelayedImageNative]::ExitCode($consumerHandle)
        if ($consumerExitCode -ne 0) {
            $detail = if (Test-Path -LiteralPath $consumerError) {
                Get-Content -LiteralPath $consumerError -Raw
            } else { 'no consumer error was recorded' }
            throw "The competitive-copy consumer failed: $detail"
        }
        $consumerEvidence = Get-Content -LiteralPath $consumerResult -Raw | ConvertFrom-Json

        return [pscustomobject][ordered]@{
            materialized_by_non_target = $materialized
            expected_text = $foreignText
            received_text = "$($consumerEvidence.text)"
            source_sha256 = $CompetitiveHash
            received_sha256 = "$($consumerEvidence.sha256)"
            input_host_exit_code = $hostExitCode
            consumer_probe_exit_code = $consumerExitCode
            status = if (
                $materialized -and
                $hostExitCode -ne 0 -and
                "$($consumerEvidence.text)" -ceq $foreignText -and
                $CompetitiveHash -eq "$($consumerEvidence.sha256)" -and
                $consumerExitCode -eq 0
            ) { 'PASS' } else { 'FAIL' }
        }
    } finally {
        if ($null -ne $form) { $form.Close(); $form.Dispose() }
        if ($null -ne $hostProcess -and -not $hostProcess.HasExited) {
            $hostProcess.Kill()
            $hostProcess.WaitForExit()
        }
        if ($null -ne $consumer -and -not $consumer.HasExited) {
            $consumer.Kill()
            $consumer.WaitForExit()
        }
    }
}

$originalClipboard = Copy-ClipboardDataObject
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("sayflex-delayed-image-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
$completed = $false
try {
    $knownImage = Join-Path $temporaryRoot 'known.bmp'
    New-KnownImage -Path $knownImage
    $knownHash = (Get-FileHash -LiteralPath $knownImage -Algorithm SHA256).Hash
    $competitiveImage = Join-Path $temporaryRoot 'competitive.bmp'
    New-CompetitiveImage -Path $competitiveImage
    $competitiveHash = (Get-FileHash -LiteralPath $competitiveImage -Algorithm SHA256).Hash
    $cases = @()
    foreach ($delay in $DelaySeconds) {
        $caseRoot = Join-Path $temporaryRoot ("delay-$delay")
        New-Item -ItemType Directory -Path $caseRoot | Out-Null
        $cases += Invoke-DelayedImageCase `
            -Delay $delay `
            -CaseRoot $caseRoot `
            -KnownImage $knownImage `
            -KnownHash $knownHash
    }
    $competitiveRoot = Join-Path $temporaryRoot 'competitive-copy'
    New-Item -ItemType Directory -Path $competitiveRoot | Out-Null
    $competitiveCopy = Invoke-CompetitiveCopyCase `
        -CaseRoot $competitiveRoot `
        -CompetitiveImage $competitiveImage `
        -CompetitiveHash $competitiveHash
    $receipt = [pscustomobject][ordered]@{
        schema_version = 2
        product = 'SayFlex'
        input_host_sha256 = (Get-FileHash -LiteralPath $inputHost -Algorithm SHA256).Hash
        witnessed_utc = [DateTimeOffset]::UtcNow.ToString('o')
        cases = $cases
        competitive_copy = $competitiveCopy
    }
    $receiptDirectory = Split-Path -Parent $receiptFile
    New-Item -ItemType Directory -Path $receiptDirectory -Force | Out-Null
    $receipt | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $receiptFile -Encoding utf8
    $failures = @(
        Get-DelayedClipboardImageReceiptFailures `
            -Receipt ([pscustomobject]$receipt) `
            -ExpectedDelays $DelaySeconds `
            -ExpectedConsumers $Consumers
    )
    if ($failures.Count -gt 0) {
        throw "Delayed-image smoke failed: $($failures -join '; ')"
    }
    $completed = $true
    Write-Host "Delayed-image smoke receipt: $receiptFile"
} finally {
    try {
        if ($null -eq $originalClipboard) {
            [System.Windows.Forms.Clipboard]::Clear()
        } else {
            [System.Windows.Forms.Clipboard]::SetDataObject($originalClipboard, $true, 10, 100)
        }
    } catch {
        Write-Warning "Could not restore the original clipboard: $($_.Exception.Message)"
    }
    $resolvedTemp = [System.IO.Path]::GetFullPath($temporaryRoot)
    $systemTemp = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    if ($completed -and
        $resolvedTemp.StartsWith($systemTemp, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedTemp).StartsWith('sayflex-delayed-image-', [StringComparison]::Ordinal)) {
        Remove-Item -LiteralPath $resolvedTemp -Recurse -Force -ErrorAction SilentlyContinue
    } elseif (-not $completed) {
        Write-Warning "Delayed-image smoke diagnostics were kept at $resolvedTemp"
    }
}
