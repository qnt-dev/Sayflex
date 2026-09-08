[CmdletBinding()]
param(
    [string]$AppDirectory,
    # Compare the installed copy against this staged one. Only meaningful after
    # installing: during a publish the installed copy is still the previous
    # release, so this would always fail.
    [switch]$Installed
)

$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($AppDirectory)) {
    $AppDirectory = Join-Path $PSScriptRoot '..\dist\app'
}
$appRoot = [System.IO.Path]::GetFullPath($AppDirectory)
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$required = @(
    'SayFlex.exe',
    'SayFlex.InputHost.exe',
    'SayFlex.Speech.Cpu.exe',
    'SayFlex.Speech.Cuda.exe',
    'SayFlex.Speech.Vulkan.exe',
    'SayFlex.Translation.exe',
    'SayFlex.Companion.exe',
    'speech-backends.json',
    'runtimes\cuda\cublas64_13.dll',
    'runtimes\cuda\cublasLt64_13.dll'
)

$missing = $required | Where-Object {
    -not (Test-Path -LiteralPath (Join-Path $appRoot $_) -PathType Leaf)
}
if (@($missing).Count -gt 0) {
    throw "Release is incomplete. Missing: $($missing -join ', ')"
}

$manifest = Get-Content -LiteralPath (Join-Path $appRoot 'speech-backends.json') -Raw |
    ConvertFrom-Json
if ($manifest.schema_version -ne 1) {
    throw "Unsupported speech backend manifest schema: $($manifest.schema_version)"
}
$backendIds = @($manifest.backends | ForEach-Object id)
foreach ($backend in @('cuda', 'vulkan', 'cpu')) {
    if ($backendIds -notcontains $backend) {
        throw "Release manifest is missing the $backend backend."
    }
}
$cudaArchitectures = @(
    $manifest.backends |
        Where-Object id -eq 'cuda' |
        ForEach-Object cuda_architectures
)
foreach ($architecture in @('86-real', '89-real', '120a-real')) {
    if ($cudaArchitectures -notcontains $architecture) {
        throw "CUDA release manifest is missing architecture $architecture."
    }
}

$files = foreach ($name in $required) {
    $path = Join-Path $appRoot $name
    $item = Get-Item -LiteralPath $path
    $hash = Get-FileHash -LiteralPath $path -Algorithm SHA256
    [pscustomobject]@{
        File = $name
        Bytes = $item.Length
        SHA256 = $hash.Hash.ToLowerInvariant()
    }
}

$files | Format-Table -AutoSize

$failures = New-Object System.Collections.Generic.List[string]

# 1. Every executable carries the version this release claims.
$expectedVersion = (
    Select-String -Path (Join-Path $rustRoot 'Cargo.toml') -Pattern '^version = "(.+)"' |
        Select-Object -First 1
).Matches[0].Groups[1].Value
foreach ($name in $required) {
    if (-not $name.EndsWith('.exe')) { continue }
    $info = (Get-Item -LiteralPath (Join-Path $appRoot $name)).VersionInfo
    if ($info.ProductVersion -ne $expectedVersion) {
        $failures.Add("$name reports version $($info.ProductVersion), not $expectedVersion")
    }
}

# 2. The installed set matches the staged set, hash for hash.
#
#    Only when asked for with -Installed. Comparing them during a publish is a
#    guaranteed failure — the installed copy is still the previous release at
#    that point — and a gate that always fails is a gate nobody reads. Run this
#    again after installing, which is when the question means something.
if ($Installed) {
    $installedRoot = Join-Path $env:LOCALAPPDATA 'Programs\SayFlex'
    if (-not (Test-Path -LiteralPath $installedRoot)) {
        $failures.Add('-Installed was asked for, and there is no installed copy')
    } else {
        foreach ($row in $files) {
            $there = Join-Path $installedRoot $row.File
            if (-not (Test-Path -LiteralPath $there)) {
                $failures.Add("$($row.File) is staged but not installed")
                continue
            }
            $hash = (Get-FileHash -LiteralPath $there -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($hash -ne $row.SHA256) {
                $failures.Add("$($row.File) installed is not the staged build")
            }
        }
    }
}

# 3. Every worker agrees with the desktop about the protocol. A handshake that
#    only happens at run time is a handshake nobody checks before shipping.
$protocol = (
    Select-String -Path (Join-Path $rustRoot 'crates\protocol\src\lib.rs') `
        -Pattern 'PROTOCOL_VERSION: u32 = (\d+)' |
        Select-Object -First 1
).Matches[0].Groups[1].Value
foreach ($worker in @('SayFlex.Speech.Cpu.exe', 'SayFlex.Translation.exe')) {
    $path = Join-Path $appRoot $worker
    # Started rather than called, so its output is read from a file: a native
    # process's stdout captured through the pipeline came back empty here, and
    # an empty answer read as a mismatch.
    $out = [System.IO.Path]::GetTempFileName()
    try {
        $run = Start-Process -FilePath $path -ArgumentList '--protocol-version' `
            -NoNewWindow -Wait -PassThru -RedirectStandardOutput $out
        $answer = (Get-Content -LiteralPath $out -Raw -ErrorAction SilentlyContinue)
        $answer = "$answer".Trim()
        if ($run.ExitCode -ne 0 -or $answer -ne $protocol) {
            $failures.Add("$worker answers '$answer' for the protocol, not $protocol")
        }
    } finally {
        Remove-Item -LiteralPath $out -Force -ErrorAction SilentlyContinue
    }
}

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Host "FAILED: $_" }
    throw "Release validation failed with $($failures.Count) problem(s)."
}

Write-Host "SayFlex $expectedVersion validated: file set, backend manifest, versions, installed hashes and protocol $protocol."
Write-Host 'This does not replace the smoke test: dictate and translate with the installed build before shipping it.'
