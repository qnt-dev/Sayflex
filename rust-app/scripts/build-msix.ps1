[CmdletBinding(DefaultParameterSetName = 'PartnerCenter')]
param(
    [string]$AppDirectory,
    [string]$OutputPath,
    [Parameter(Mandatory, ParameterSetName = 'PartnerCenter')]
    [string]$IdentityName,
    [Parameter(Mandatory, ParameterSetName = 'PartnerCenter')]
    [string]$Publisher,
    [string]$PublisherDisplayName = 'SayFlex',
    [Parameter(Mandatory, ParameterSetName = 'Development')]
    [switch]$DevelopmentIdentity
)

$ErrorActionPreference = 'Stop'
$rustRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ([string]::IsNullOrWhiteSpace($AppDirectory)) {
    $AppDirectory = Join-Path $rustRoot 'dist\app'
}
$appRoot = [System.IO.Path]::GetFullPath($AppDirectory)
$buildManifestPath = Join-Path $appRoot 'build-manifest.json'
if (-not (Test-Path -LiteralPath $buildManifestPath -PathType Leaf)) {
    throw "The staged build has no build-manifest.json: $buildManifestPath"
}
$build = Get-Content -LiteralPath $buildManifestPath -Raw | ConvertFrom-Json
if ($build.schema_version -ne 1 -or $build.product -ne 'SayFlex' -or
    "$($build.version)" -notmatch '^\d+\.\d+\.\d+$') {
    throw 'The staged build manifest has an unsupported identity or version.'
}

if ($DevelopmentIdentity) {
    $IdentityName = 'qnt-dev.SayFlex.Development'
    $Publisher = 'CN=SayFlex Development'
}
$packageVersion = "$($build.version).0"
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $suffix = if ($DevelopmentIdentity) { 'development-unsigned' } else { 'store-unsigned' }
    $OutputPath = Join-Path $rustRoot "dist\SayFlex-$($build.version)-win-x64-$suffix.msix"
}
$packagePath = [System.IO.Path]::GetFullPath($OutputPath)
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $packagePath) | Out-Null

# Validate the staged payload before copying it. This is deliberately not a
# substitute for the post-pack layout check below.
& (Join-Path $PSScriptRoot 'validate-release.ps1') -AppDirectory $appRoot
if ($LASTEXITCODE -ne 0) {
    throw 'The staged application failed release validation.'
}

$layoutParent = Join-Path $rustRoot 'target\msix-layouts'
New-Item -ItemType Directory -Force -Path $layoutParent | Out-Null
$layout = Join-Path $layoutParent ("$PID-" + [Guid]::NewGuid().ToString('N'))
$layout = [System.IO.Path]::GetFullPath($layout)
if (-not $layout.StartsWith(
    [System.IO.Path]::GetFullPath($layoutParent) + [System.IO.Path]::DirectorySeparatorChar,
    [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to create an MSIX layout outside $layoutParent"
}

function ConvertTo-XmlAttribute([string]$Value) {
    return [System.Security.SecurityElement]::Escape($Value)
}

function Write-Logo([string]$Source, [string]$Destination, [int]$Size) {
    Add-Type -AssemblyName System.Drawing
    $sourceImage = [System.Drawing.Image]::FromFile($Source)
    try {
        $target = [System.Drawing.Bitmap]::new(
            $Size,
            $Size,
            [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
        )
        try {
            $graphics = [System.Drawing.Graphics]::FromImage($target)
            try {
                $graphics.Clear([System.Drawing.Color]::Transparent)
                $graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
                $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
                $graphics.DrawImage($sourceImage, 0, 0, $Size, $Size)
            } finally {
                $graphics.Dispose()
            }
            $target.Save($Destination, [System.Drawing.Imaging.ImageFormat]::Png)
        } finally {
            $target.Dispose()
        }
    } finally {
        $sourceImage.Dispose()
    }
}

try {
    New-Item -ItemType Directory -Path $layout | Out-Null
    Copy-Item -Path (Join-Path $appRoot '*') -Destination $layout -Recurse -Force
    $assets = Join-Path $layout 'Assets'
    New-Item -ItemType Directory -Path $assets | Out-Null
    $logoSource = Join-Path $rustRoot '..\resources\icons\sayflex-256.png'
    if (-not (Test-Path -LiteralPath $logoSource -PathType Leaf)) {
        throw "The canonical SayFlex PNG is missing: $logoSource"
    }
    Write-Logo $logoSource (Join-Path $assets 'StoreLogo.png') 50
    Write-Logo $logoSource (Join-Path $assets 'Square44x44Logo.png') 44
    Write-Logo $logoSource (Join-Path $assets 'Square150x150Logo.png') 150

    $templatePath = Join-Path $rustRoot 'installer\msix\AppxManifest.xml'
    $manifestText = Get-Content -LiteralPath $templatePath -Raw
    $manifestText = $manifestText.Replace('__IDENTITY_NAME__', (ConvertTo-XmlAttribute $IdentityName))
    $manifestText = $manifestText.Replace('__PUBLISHER__', (ConvertTo-XmlAttribute $Publisher))
    $manifestText = $manifestText.Replace('__PUBLISHER_DISPLAY_NAME__', (ConvertTo-XmlAttribute $PublisherDisplayName))
    $manifestText = $manifestText.Replace('__PACKAGE_VERSION__', $packageVersion)
    Set-Content -LiteralPath (Join-Path $layout 'AppxManifest.xml') -Value $manifestText -Encoding utf8

    Import-Module (Join-Path $PSScriptRoot 'release-validation.psm1') -Force
    $layoutFailures = @(Get-MsixLayoutFailures `
        -LayoutPath $layout `
        -BuildManifestPath $buildManifestPath `
        -ExpectedIdentityName $IdentityName `
        -ExpectedPublisher $Publisher)
    if ($layoutFailures.Count -gt 0) {
        throw "MSIX layout validation failed:`n$($layoutFailures -join "`n")"
    }

    $kitsRoot = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $makeAppx = Get-ChildItem -LiteralPath $kitsRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object Name -Match '^\d+\.\d+\.\d+\.\d+$' |
        Sort-Object { [Version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64\makeappx.exe' } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
        Select-Object -First 1
    if (-not $makeAppx) {
        throw 'makeappx.exe was not found in the Windows SDK.'
    }
    & $makeAppx pack /d $layout /p $packagePath /o
    if ($LASTEXITCODE -ne 0) {
        throw 'MakeAppx failed to create the package.'
    }
    $checksum = (Get-FileHash -LiteralPath $packagePath -Algorithm SHA256).Hash
    Set-Content -LiteralPath "$packagePath.sha256" -Encoding ascii -NoNewline -Value (
        "$checksum  $([System.IO.Path]::GetFileName($packagePath))"
    )
    & (Join-Path $PSScriptRoot 'validate-release.ps1') `
        -AppDirectory $appRoot `
        -MsixPath $packagePath `
        -MsixIdentityName $IdentityName `
        -MsixPublisher $Publisher
    if ($LASTEXITCODE -ne 0) {
        throw 'The packed MSIX failed release validation.'
    }
    Write-Host "Unsigned MSIX created: $packagePath"
    if ($DevelopmentIdentity) {
        Write-Host 'This development-identity package is not a Partner Center submission artifact.'
    } else {
        Write-Host 'Partner Center must certify and sign this Store package before distribution.'
    }
} finally {
    $resolvedParent = [System.IO.Path]::GetFullPath($layoutParent) + [System.IO.Path]::DirectorySeparatorChar
    if ($layout.StartsWith($resolvedParent, [StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $layout)) {
        Remove-Item -LiteralPath $layout -Recurse -Force
    }
}
