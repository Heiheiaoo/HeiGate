param(
    [string]$OutputPath = (Join-Path $PSScriptRoot 'AppIcon.ico')
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

# Icon.FromHandle(...).Save() in Windows PowerShell 5.1 writes a single 4-bit
# (16 colour) image, which strips the anti-aliasing and the soft shadow and
# leaves the tray downscaling a 256px image to 16px. The ICO is therefore
# assembled by hand: 32-bit DIBs for the small sizes the shell uses for the
# tray, taskbar and title bar, and a PNG for 256px, which every consumer of
# the file (rc via ApplicationIcon, NSIS, Explorer) accepts.
$sizes = @(16, 24, 32, 48, 256)

$segments = @(
    @(68, 70, 68, 184),
    @(116, 44, 116, 158),
    @(68, 126, 116, 126),
    @(200, 74, 144, 42),
    @(144, 42, 144, 166),
    @(144, 166, 200, 198),
    @(200, 198, 200, 126),
    @(200, 126, 166, 126)
)

function New-RoundPen([System.Drawing.Color]$color, [single]$width) {
    $pen = [System.Drawing.Pen]::new($color, $width)
    $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    return $pen
}

# The artwork is authored on a 256px grid and scaled, so every size is drawn
# as vectors rather than resampled from the large bitmap.
function New-IconBitmap([int]$size) {
    $bitmap = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $graphics.Clear([System.Drawing.Color]::Transparent)
        $graphics.ScaleTransform($size / 256.0, $size / 256.0)

        $shadow = New-RoundPen ([System.Drawing.Color]::FromArgb(42, 79, 70, 229)) 28
        $hPen = New-RoundPen ([System.Drawing.Color]::FromArgb(255, 59, 130, 246)) 22
        $gPen = New-RoundPen ([System.Drawing.Color]::FromArgb(255, 99, 102, 241)) 22
        try {
            foreach ($line in $segments) {
                $graphics.DrawLine($shadow, $line[0], $line[1] + 4, $line[2], $line[3] + 4)
            }
            for ($i = 0; $i -lt $segments.Count; $i++) {
                $pen = if ($i -lt 3) { $hPen } else { $gPen }
                $line = $segments[$i]
                $graphics.DrawLine($pen, $line[0], $line[1], $line[2], $line[3])
            }
        } finally {
            $shadow.Dispose()
            $hPen.Dispose()
            $gPen.Dispose()
        }
    } finally {
        $graphics.Dispose()
    }
    return $bitmap
}

function Get-PngBytes([System.Drawing.Bitmap]$bitmap) {
    $stream = [System.IO.MemoryStream]::new()
    try {
        $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
        return ,$stream.ToArray()
    } finally {
        $stream.Dispose()
    }
}

# An ICO DIB is a BITMAPINFOHEADER with a doubled height (XOR image + AND
# mask), bottom-up BGRA rows, then a 1bpp AND mask. With 32-bit colour the
# alpha channel carries transparency, so the mask is left all zero.
function Get-DibBytes([System.Drawing.Bitmap]$bitmap) {
    $size = $bitmap.Width
    $rect = [System.Drawing.Rectangle]::new(0, 0, $size, $size)
    $data = $bitmap.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $rowBytes = $size * 4
    $pixels = New-Object byte[] ($rowBytes * $size)
    try {
        for ($y = 0; $y -lt $size; $y++) {
            $source = [IntPtr]::Add($data.Scan0, $y * $data.Stride)
            [System.Runtime.InteropServices.Marshal]::Copy($source, $pixels, ($size - 1 - $y) * $rowBytes, $rowBytes)
        }
    } finally {
        $bitmap.UnlockBits($data)
    }
    $maskRowBytes = [int]([math]::Ceiling($size / 32.0) * 4)
    $maskBytes = $maskRowBytes * $size

    $stream = [System.IO.MemoryStream]::new()
    $writer = [System.IO.BinaryWriter]::new($stream)
    try {
        $writer.Write([uint32]40)
        $writer.Write([int32]$size)
        $writer.Write([int32]($size * 2))
        $writer.Write([uint16]1)
        $writer.Write([uint16]32)
        $writer.Write([uint32]0)
        $writer.Write([uint32]($pixels.Length + $maskBytes))
        $writer.Write([int32]0)
        $writer.Write([int32]0)
        $writer.Write([uint32]0)
        $writer.Write([uint32]0)
        $writer.Write($pixels)
        $writer.Write((New-Object byte[] $maskBytes))
        $writer.Flush()
        return ,$stream.ToArray()
    } finally {
        $writer.Dispose()
        $stream.Dispose()
    }
}

$images = @()
foreach ($size in $sizes) {
    $bitmap = New-IconBitmap $size
    try {
        $bytes = if ($size -ge 256) { Get-PngBytes $bitmap } else { Get-DibBytes $bitmap }
    } finally {
        $bitmap.Dispose()
    }
    $images += ,@{ Size = $size; Bytes = $bytes }
}

$output = [System.IO.File]::Create($OutputPath)
$writer = [System.IO.BinaryWriter]::new($output)
try {
    $writer.Write([uint16]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]$images.Count)
    $offset = 6 + 16 * $images.Count
    foreach ($image in $images) {
        # A dimension of 256 does not fit in a byte; ICO encodes it as 0.
        $dimension = if ($image.Size -ge 256) { 0 } else { $image.Size }
        $writer.Write([byte]$dimension)
        $writer.Write([byte]$dimension)
        $writer.Write([byte]0)
        $writer.Write([byte]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]32)
        $writer.Write([uint32]$image.Bytes.Length)
        $writer.Write([uint32]$offset)
        $offset += $image.Bytes.Length
    }
    foreach ($image in $images) {
        $writer.Write([byte[]]$image.Bytes)
    }
} finally {
    $writer.Dispose()
    $output.Dispose()
}

Write-Host "Generated $OutputPath ($($sizes -join ', ') px, 32-bit)"
