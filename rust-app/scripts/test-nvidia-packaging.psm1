Set-StrictMode -Version Latest

function Get-TestNvidiaSourceSnapshot {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$RepositoryPath)

    $repositoryRoot = (& git -C $RepositoryPath rev-parse --show-toplevel).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify the test build source repository.' }
    $commit = (& git -C $repositoryRoot rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify the test build base commit.' }
    $status = @(& git -C $repositoryRoot status --porcelain --untracked-files=all)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect the test build source changes.' }
    $paths = @(& git -C $repositoryRoot -c core.quotepath=false ls-files --cached --others --exclude-standard)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot enumerate the test build sources.' }
    $files = @(
        foreach ($relative in @($paths | Sort-Object -Unique)) {
            $path = Join-Path $repositoryRoot $relative
            if (Test-Path -LiteralPath $path -PathType Leaf) {
                [ordered]@{
                    path = $relative.Replace('\', '/')
                    bytes = (Get-Item -LiteralPath $path).Length
                    sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
                }
            } else {
                [ordered]@{ path = $relative.Replace('\', '/'); deleted = $true }
            }
        }
    )
    $snapshot = [ordered]@{
        base_commit = $commit
        dirty = $status.Count -gt 0
        git_status = $status
        files = $files
    }
    $serialized = $snapshot | ConvertTo-Json -Depth 6 -Compress
    $hasher = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hash = $hasher.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($serialized))
        $snapshot.sha256 = [BitConverter]::ToString($hash).Replace('-', '').ToLowerInvariant()
    } finally { $hasher.Dispose() }
    return [pscustomobject]$snapshot
}

function Assert-TestNvidiaSourceUnchanged {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$RepositoryPath,
        [Parameter(Mandatory)][psobject]$Snapshot
    )
    $current = Get-TestNvidiaSourceSnapshot -RepositoryPath $RepositoryPath
    if ($current.sha256 -ne $Snapshot.sha256) {
        throw 'The test build sources changed during or after compilation. Rebuild the final sources before packaging.'
    }
}

function Assert-TestNvidiaPayload {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$AppDirectory,
        [Parameter(Mandatory)][psobject]$Manifest
    )
    $marker = Join-Path $AppDirectory 'sayflex-test-channel'
    if (-not (Test-Path -LiteralPath $marker -PathType Leaf) -or
        (Get-Content -LiteralPath $marker -Raw).Trim() -ne 'test-nvidia') {
        throw 'The test payload requires its sayflex-test-channel isolation marker.'
    }
    if ($Manifest.PSObject.Properties.Name -notcontains 'official_release' -or
        $Manifest.official_release -isnot [bool] -or $Manifest.official_release -ne $false -or
        $Manifest.PSObject.Properties.Name -notcontains 'channel' -or $Manifest.channel -ne 'test-nvidia') {
        throw 'The test manifest must explicitly identify this as an unofficial test-nvidia build.'
    }
    if ($Manifest.PSObject.Properties.Name -notcontains 'source_snapshot' -or
        $Manifest.source_snapshot.sha256 -notmatch '^[0-9a-f]{64}$' -or
        @($Manifest.source_snapshot.files).Count -eq 0) {
        throw 'The test manifest must include its exact source snapshot.'
    }
    $bundledNvidia = @(Get-ChildItem -LiteralPath $AppDirectory -File -Recurse | Where-Object {
        $_.Name -match '(?i)^(cublas|cudart|cufft|curand|cusolver|cusparse|nvrtc|nvjitlink|nvfatbin|nvcuda|nvcudart).*\.dll$'
    })
    if ($bundledNvidia.Count -gt 0) {
        throw "The test payload must not bundle NVIDIA runtime DLLs: $($bundledNvidia.Name -join ', ')"
    }
}

Export-ModuleMember -Function Get-TestNvidiaSourceSnapshot, Assert-TestNvidiaSourceUnchanged, Assert-TestNvidiaPayload
