[CmdletBinding()]
param(
    [string]$OutputDirectory = (
        Join-Path $PSScriptRoot '..\resources\icons')
)

$ErrorActionPreference = 'Stop'
# PowerShell 7 ships the assembly as System.Drawing.Common; Windows PowerShell
# 5.1 has it as System.Drawing. Either will do, so take whichever is there.
try { Add-Type -AssemblyName System.Drawing.Common -ErrorAction Stop }
catch { Add-Type -AssemblyName System.Drawing }

$outputRoot = [IO.Path]::GetFullPath($OutputDirectory)
[IO.Directory]::CreateDirectory($outputRoot) | Out-Null

function New-RoundedRectanglePath {
    param(
        [float]$X,
        [float]$Y,
        [float]$Width,
        [float]$Height,
        [float]$Radius
    )

    $diameter = $Radius * 2
    $path = [Drawing.Drawing2D.GraphicsPath]::new()
    $path.AddArc($X, $Y, $diameter, $diameter, 180, 90)
    $path.AddArc(
        $X + $Width - $diameter,
        $Y,
        $diameter,
        $diameter,
        270,
        90)
    $path.AddArc(
        $X + $Width - $diameter,
        $Y + $Height - $diameter,
        $diameter,
        $diameter,
        0,
        90)
    $path.AddArc(
        $X,
        $Y + $Height - $diameter,
        $diameter,
        $diameter,
        90,
        90)
    $path.CloseFigure()
    return $path
}

function New-IconFrame {
    param([int]$Size)

    $bitmap = [Drawing.Bitmap]::new(
        $Size,
        $Size,
        [Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.Clear([Drawing.Color]::Transparent)
        $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.CompositingQuality =
            [Drawing.Drawing2D.CompositingQuality]::HighQuality
        $graphics.InterpolationMode =
            [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.PixelOffsetMode =
            [Drawing.Drawing2D.PixelOffsetMode]::HighQuality

        $scale = [float]($Size / 256.0)
        # No tile behind the mark. Every capsule is the SayFlex red requested
        # for the waveform; the graphite halo keeps the transparent icon
        # legible on both light and dark Windows surfaces.
        $outlineBrush = [Drawing.SolidBrush]::new(
            [Drawing.ColorTranslator]::FromHtml('#111317'))
        $accentBrush = [Drawing.SolidBrush]::new(
            [Drawing.ColorTranslator]::FromHtml('#EF5048'))
        $centres = @(40, 84, 128, 172, 216)
        $heights = @(62, 112, 158, 112, 62)
        try {
            for ($index = 0; $index -lt $centres.Count; $index++) {
                $outlineWidth = [float](32 * $scale)
                $outlineHeight = [float](($heights[$index] + 18) * $scale)
                $outlineX = [float](($centres[$index] * $scale) - ($outlineWidth / 2))
                $outlineY = [float](($Size - $outlineHeight) / 2)
                $outline = New-RoundedRectanglePath `
                    -X $outlineX `
                    -Y $outlineY `
                    -Width $outlineWidth `
                    -Height $outlineHeight `
                    -Radius ($outlineWidth / 2)
                try {
                    $graphics.FillPath($outlineBrush, $outline)
                }
                finally {
                    $outline.Dispose()
                }

                $width = [float](22 * $scale)
                $height = [float]($heights[$index] * $scale)
                $x = [float](($centres[$index] * $scale) - ($width / 2))
                $y = [float](($Size - $height) / 2)
                $bar = New-RoundedRectanglePath `
                    -X $x `
                    -Y $y `
                    -Width $width `
                    -Height $height `
                    -Radius ($width / 2)
                try {
                    $graphics.FillPath($accentBrush, $bar)
                }
                finally {
                    $bar.Dispose()
                }
            }
        }
        finally {
            $outlineBrush.Dispose()
            $accentBrush.Dispose()
        }

        $stream = [IO.MemoryStream]::new()
        try {
            $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
            return $stream.ToArray()
        }
        finally {
            $stream.Dispose()
        }
    }
    finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

$sizes = @(16, 20, 24, 32, 40, 48, 64, 128, 256)
$frames = foreach ($size in $sizes) {
    [pscustomobject]@{
        Size = $size
        Bytes = New-IconFrame -Size $size
    }
}

$iconPath = Join-Path $outputRoot 'sayflex.ico'
$file = [IO.File]::Create($iconPath)
$writer = [IO.BinaryWriter]::new($file)
try {
    $writer.Write([uint16]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]$frames.Count)

    $offset = 6 + (16 * $frames.Count)
    foreach ($frame in $frames) {
        $writer.Write([byte]$(if ($frame.Size -ge 256) { 0 } else { $frame.Size }))
        $writer.Write([byte]$(if ($frame.Size -ge 256) { 0 } else { $frame.Size }))
        $writer.Write([byte]0)
        $writer.Write([byte]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]32)
        $writer.Write([uint32]$frame.Bytes.Length)
        $writer.Write([uint32]$offset)
        $offset += $frame.Bytes.Length
    }

    foreach ($frame in $frames) {
        $writer.Write([byte[]]$frame.Bytes)
    }
}
finally {
    $writer.Dispose()
    $file.Dispose()
}

[IO.File]::WriteAllBytes(
    (Join-Path $outputRoot 'sayflex-256.png'),
    [byte[]]$frames[-1].Bytes)

Write-Host "Generated $iconPath"
