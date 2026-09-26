param(
    [string]$OutputPath = (Join-Path $PSScriptRoot 'AppIcon.ico')
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$size = 256
$bitmap = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$graphics.Clear([System.Drawing.Color]::Transparent)

$shadow = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(42, 79, 70, 229), 28)
$shadow.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$shadow.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
$hPen = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(255, 59, 130, 246), 22)
$hPen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$hPen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
$gPen = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(255, 99, 102, 241), 22)
$gPen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$gPen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
$gPen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round

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

foreach ($line in $segments) {
    $graphics.DrawLine($shadow, $line[0], $line[1] + 4, $line[2], $line[3] + 4)
}
for ($i = 0; $i -lt 3; $i++) {
    $line = $segments[$i]
    $graphics.DrawLine($hPen, $line[0], $line[1], $line[2], $line[3])
}
for ($i = 3; $i -lt $segments.Count; $i++) {
    $line = $segments[$i]
    $graphics.DrawLine($gPen, $line[0], $line[1], $line[2], $line[3])
}

$iconHandle = $bitmap.GetHicon()
$icon = [System.Drawing.Icon]::FromHandle($iconHandle)
$stream = [System.IO.File]::Create($OutputPath)
try {
    $icon.Save($stream)
} finally {
    $stream.Dispose()
    $icon.Dispose()
    $graphics.Dispose()
    $bitmap.Dispose()
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class HeiGateNativeIcon {
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool DestroyIcon(IntPtr handle);
}
'@
    [HeiGateNativeIcon]::DestroyIcon($iconHandle) | Out-Null
}

Write-Host "Generated $OutputPath"
