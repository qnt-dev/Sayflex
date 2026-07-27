[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release',
    [string]$Version = '0.1.0'
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $PSScriptRoot '..'))
$distributionRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $repositoryRoot 'dist'))
$applicationRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $distributionRoot 'app'))

if (-not $distributionRoot.StartsWith(
    $repositoryRoot + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Distribution path escaped the repository: $distributionRoot"
}

if (Test-Path -LiteralPath $applicationRoot) {
    Remove-Item -LiteralPath $applicationRoot -Recurse -Force
}

New-Item -ItemType Directory -Path $applicationRoot -Force | Out-Null

$projects = @(
    'src\Keyboop.UI\Keyboop.UI.csproj',
    'src\Keyboop.InputHost\Keyboop.InputHost.csproj',
    'src\Keyboop.SpeechWorker\Keyboop.SpeechWorker.csproj'
)

foreach ($project in $projects) {
    dotnet publish (Join-Path $repositoryRoot $project) `
        --configuration $Configuration `
        --runtime win-x64 `
        --self-contained true `
        --output $applicationRoot `
        -p:Version=$Version `
        -p:PublishReadyToRun=true `
        --nologo
    if ($LASTEXITCODE -ne 0) {
        throw "dotnet publish failed for $project"
    }
}

$excludedRuntimeDirectories = @(
    'runtimes\cuda\linux-x64',
    'runtimes\win-arm64',
    'runtimes\win-x86',
    'cs',
    'de',
    'es',
    'fr',
    'it',
    'ja',
    'ko',
    'pl',
    'pt-BR',
    'tr',
    'zh-Hans',
    'zh-Hant'
)

foreach ($relativePath in $excludedRuntimeDirectories) {
    $target = [System.IO.Path]::GetFullPath(
        (Join-Path $applicationRoot $relativePath))
    if (-not $target.StartsWith(
        $applicationRoot + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Publish cleanup path escaped the application root: $target"
    }

    if (Test-Path -LiteralPath $target) {
        Remove-Item -LiteralPath $target -Recurse -Force
    }
}

Copy-Item -LiteralPath (Join-Path $repositoryRoot 'LICENSE') `
    -Destination $applicationRoot
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md') `
    -Destination $applicationRoot

$dotnetRoot = Split-Path (Get-Command dotnet).Source -Parent
Copy-Item -LiteralPath (Join-Path $dotnetRoot 'LICENSE.txt') `
    -Destination (Join-Path $applicationRoot 'DOTNET_LICENSE.txt')
Copy-Item -LiteralPath (Join-Path $dotnetRoot 'ThirdPartyNotices.txt') `
    -Destination (Join-Path $applicationRoot 'DOTNET_THIRD_PARTY_NOTICES.txt')

$manifest = [ordered]@{
    version = $Version
    configuration = $Configuration
    runtime = 'win-x64'
    createdAt = [DateTimeOffset]::UtcNow.ToString('O')
    files = @(
        Get-ChildItem -LiteralPath $applicationRoot -Recurse -File |
            Sort-Object FullName |
            ForEach-Object {
                [ordered]@{
                    path = [System.IO.Path]::GetRelativePath(
                        $applicationRoot,
                        $_.FullName).Replace('\', '/')
                    bytes = $_.Length
                    sha256 = (Get-FileHash -LiteralPath $_.FullName `
                        -Algorithm SHA256).Hash.ToLowerInvariant()
                }
            }
    )
}

$manifest |
    ConvertTo-Json -Depth 5 |
    Set-Content -LiteralPath (Join-Path $distributionRoot 'manifest.json') `
        -Encoding utf8NoBOM

Write-Host "Published Keyboop $Version to $applicationRoot"
