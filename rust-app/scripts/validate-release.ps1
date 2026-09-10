[CmdletBinding()]
param(
    [string]$AppDirectory,
    # The installer to check, by path. Never "the newest file in dist": that
    # picked up the *previous* release — publish runs this before Inno Setup has
    # produced anything — verified its old sidecar, and reported the installer
    # checksum as checked while the one about to be built went unexamined.
    [string]$InstallerPath,
    # Compare the installed copy against this staged one. Only meaningful after
    # installing: during a publish the installed copy is still the previous
    # release, so this would always fail.
    [switch]$Installed,
    # Public artifacts must prove Authenticode on every executable and the
    # exact installer named by -InstallerPath. Developer builds may omit it.
    [switch]$RequireSignature
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'release-validation.psm1') -Force
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
    'build-manifest.json',
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

function Invoke-ReleaseProbe([string]$Path, [string]$Argument) {
    $out = [System.IO.Path]::GetTempFileName()
    try {
        $run = Start-Process -FilePath $Path -ArgumentList $Argument `
            -NoNewWindow -PassThru -RedirectStandardOutput $out
        $null = $run.Handle
        if (-not $run.WaitForExit(20000)) {
            $run.Kill()
            throw "$([System.IO.Path]::GetFileName($Path)) did not answer $Argument within 20 seconds"
        }
        $answer = "$(Get-Content -LiteralPath $out -Raw -ErrorAction SilentlyContinue)".Trim()
        if ($run.ExitCode -ne 0 -or [string]::IsNullOrWhiteSpace($answer)) {
            throw "$([System.IO.Path]::GetFileName($Path)) failed to answer $Argument"
        }
        return $answer
    } finally {
        Remove-Item -LiteralPath $out -Force -ErrorAction SilentlyContinue
    }
}

# 0. The manifest binds this directory to one commit, one dependency lock and
# one exact set of bytes. It is the authority used by every later comparison.
$buildManifestPath = Join-Path $appRoot 'build-manifest.json'
$buildManifest = Get-Content -LiteralPath $buildManifestPath -Raw | ConvertFrom-Json
if ($buildManifest.schema_version -ne 1 -or $buildManifest.product -ne 'SayFlex') {
    $failures.Add('build-manifest.json has an unsupported identity or schema')
}
if ($buildManifest.git_commit -notmatch '^[0-9a-f]{40}$') {
    $failures.Add("build manifest has an invalid commit '$($buildManifest.git_commit)'")
}
$manifestFiles = @($buildManifest.files)
$actualPayload = @(
    Get-ChildItem -LiteralPath $appRoot -File -Recurse |
        Where-Object Name -ne 'build-manifest.json' |
        ForEach-Object { $_.FullName.Substring($appRoot.Length + 1).Replace('\', '/') } |
        Sort-Object
)
$declaredPayload = @($manifestFiles | ForEach-Object path | Sort-Object)
if (Compare-Object $declaredPayload $actualPayload) {
    $failures.Add('the staged payload file set does not match build-manifest.json')
}
foreach ($entry in $manifestFiles) {
    $path = Join-Path $appRoot ($entry.path.Replace('/', '\'))
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { continue }
    $item = Get-Item -LiteralPath $path
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($item.Length -ne $entry.bytes -or $hash -ne $entry.sha256) {
        $failures.Add("$($entry.path) does not match build-manifest.json")
    }
}
$head = (& git -C $rustRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -eq 0 -and $head -ne $buildManifest.git_commit) {
    $failures.Add("staged build is commit $($buildManifest.git_commit), not current HEAD $head")
}
$lockHash = (Get-FileHash -LiteralPath (Join-Path $rustRoot 'Cargo.lock') -Algorithm SHA256).Hash.ToLowerInvariant()
if ($lockHash -ne $buildManifest.cargo_lock_sha256) {
    $failures.Add('Cargo.lock does not match the dependency lock recorded by the build')
}


# 1. Every executable carries the version this release claims.
# From the built desktop, not from Cargo.toml: the question is what this
# package says it is, and a source file cannot answer that about a binary that
# may have been built before the number moved.
$expectedVersion = (Get-Item -LiteralPath (Join-Path $appRoot 'SayFlex.exe')).VersionInfo.ProductVersion
if ([string]::IsNullOrWhiteSpace($expectedVersion)) {
    throw 'SayFlex.exe carries no version; the package cannot be identified.'
}
$sourceVersion = (
    Select-String -Path (Join-Path $rustRoot 'Cargo.toml') -Pattern '^version = "(.+)"' |
        Select-Object -First 1
).Matches[0].Groups[1].Value
if ($expectedVersion -ne $sourceVersion) {
    $failures.Add("SayFlex.exe is $expectedVersion but the workspace says $sourceVersion")
}
if ($expectedVersion -ne $buildManifest.version) {
    $failures.Add("SayFlex.exe is $expectedVersion but the build manifest says $($buildManifest.version)")
}
foreach ($name in $required) {
    if (-not $name.EndsWith('.exe')) { continue }
    $info = (Get-Item -LiteralPath (Join-Path $appRoot $name)).VersionInfo
    if ($info.ProductVersion -ne $expectedVersion) {
        $failures.Add("$name reports version $($info.ProductVersion), not $expectedVersion")
    }
}

if ($RequireSignature) {
    foreach ($name in $required | Where-Object { $_.EndsWith('.exe') }) {
        $path = Join-Path $appRoot $name
        $signature = Get-AuthenticodeSignature -LiteralPath $path
        if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
            $failures.Add(
                "$name does not carry a valid Authenticode signature (status: $($signature.Status))"
            )
        }
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
        # Compare every byte that the build manifest owns, rather than only
        # the small `$required` bootability subset. Otherwise a stale model
        # runtime DLL or asset could survive installation while this check
        # still called the installed product identical to staging.
        $installedPayload = @($manifestFiles) + [pscustomobject]@{
            path = 'build-manifest.json'
            bytes = (Get-Item -LiteralPath $buildManifestPath).Length
            sha256 = (Get-FileHash -LiteralPath $buildManifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        $expectedInstalledFiles = @($installedPayload | ForEach-Object path | Sort-Object)
        $actualInstalledFiles = @(
            Get-ChildItem -LiteralPath $installedRoot -File -Recurse |
                Where-Object { $_.Name -notmatch '^unins\d+\.(exe|dat|msg)$' } |
                ForEach-Object { $_.FullName.Substring($installedRoot.Length + 1).Replace('\', '/') } |
                Sort-Object
        )
        if (Compare-Object $expectedInstalledFiles $actualInstalledFiles) {
            $failures.Add('the installed payload file set does not match the staged release')
        }
        foreach ($row in $installedPayload) {
            $relativePath = $row.path.Replace('/', '\')
            $there = Join-Path $installedRoot $relativePath
            if (-not (Test-Path -LiteralPath $there)) {
                $failures.Add("$($row.path) is staged but not installed")
                continue
            }
            $item = Get-Item -LiteralPath $there
            $hash = (Get-FileHash -LiteralPath $there -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($item.Length -ne $row.bytes -or $hash -ne $row.sha256) {
                $failures.Add("$($row.path) installed is not the staged build")
            }
        }
        # Inno's stable AppId is the registry identity. Its default display
        # name is versioned ("SayFlex version 0.6.1"), so discovering the key
        # through a presentation string made a correct installation look
        # absent. Address the exact product key and read its values explicitly.
        $uninstallPath = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{BE541A7B-AF41-4F29-8D3F-D9F75AB68F66}_is1'
        $uninstall = Get-ItemProperty -LiteralPath $uninstallPath -ErrorAction SilentlyContinue
        if (-not $uninstall) {
            $failures.Add('the installed copy has no SayFlex uninstall registration')
        } else {
            if ($uninstall.DisplayVersion -ne $expectedVersion) {
                $failures.Add("uninstall registration reports $($uninstall.DisplayVersion), not $expectedVersion")
            }
            $registeredLocation = "$($uninstall.InstallLocation)".TrimEnd('\')
            if ($registeredLocation -and
                -not $registeredLocation.Equals($installedRoot.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)) {
                $failures.Add("uninstall registration points at '$registeredLocation', not '$installedRoot'")
            }
        }
    }
}

# 3. Every worker agrees with the desktop about the protocol. A handshake that
#    only happens at run time is a handshake nobody checks before shipping.
$protocol = Invoke-ReleaseProbe (Join-Path $appRoot 'SayFlex.exe') '--protocol-version'
if ($protocol -ne "$($buildManifest.protocol_version)") {
    $failures.Add("SayFlex.exe reports protocol $protocol but the build manifest says $($buildManifest.protocol_version)")
}
# Every process that speaks the protocol, not two of them: a CUDA worker that
# disagrees is exactly as broken as a CPU one, and it is the one that runs.
foreach ($worker in @(
    'SayFlex.Speech.Cpu.exe',
    'SayFlex.Speech.Cuda.exe',
    'SayFlex.Speech.Vulkan.exe',
    'SayFlex.Translation.exe',
    'SayFlex.InputHost.exe',
    'SayFlex.Companion.exe'
)) {
    $path = Join-Path $appRoot $worker
    try {
        $answer = Invoke-ReleaseProbe $path '--protocol-version'
        if ($answer -ne $protocol) {
            $failures.Add("$worker answers '$answer' for the protocol, not $protocol")
        }
        $buildId = Invoke-ReleaseProbe $path '--build-id'
        if ($buildId -ne $buildManifest.git_commit) {
            $failures.Add("$worker belongs to commit '$buildId', not $($buildManifest.git_commit)")
        }
    } catch {
        $failures.Add($_.Exception.Message)
    }
}
$desktopBuildId = Invoke-ReleaseProbe (Join-Path $appRoot 'SayFlex.exe') '--build-id'
if ($desktopBuildId -ne $buildManifest.git_commit) {
    $failures.Add("SayFlex.exe belongs to commit '$desktopBuildId', not $($buildManifest.git_commit)")
}

# 4. The installer's own checksum matches the sidecar shipped beside it. A
#    sidecar nobody compares is a sidecar that proves nothing.
$installerChecked = $false
if (-not [string]::IsNullOrWhiteSpace($InstallerPath)) {
    $installerChecked = $true
    Get-InstallerArtifactFailures `
        -InstallerPath $InstallerPath `
        -ExpectedVersion $expectedVersion `
        -RequireSignature:$RequireSignature |
        ForEach-Object { $failures.Add($_) }
} elseif ($RequireSignature) {
    $failures.Add('-RequireSignature also requires the exact -InstallerPath')
}

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Host "FAILED: $_" }
    throw "Release validation failed with $($failures.Count) problem(s)."
}

$checked = @('file set', 'build manifest', 'binary build identity', 'Cargo.lock', 'backend manifest', 'versions', "protocol $protocol")
if ($installerChecked) { $checked += 'installer checksum' }
if ($RequireSignature) { $checked += 'Authenticode signatures' }
if ($Installed) { $checked += 'installed hashes'; $checked += 'uninstall registration' }
Write-Host "SayFlex $expectedVersion checked: $($checked -join ', ')."
if (-not $installerChecked) {
    Write-Host 'No installer was named, so none was checked: pass -InstallerPath after building one.'
}
if (-not $Installed) {
    Write-Host 'The installed copy was not compared: install the package and run this again with -Installed.'
}
Write-Host 'None of this replaces the smoke test: dictate and translate with the installed build before shipping it.'
