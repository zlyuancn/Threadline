# Generate the app icon (1024x1024 PNG): a "thread line" running through two endpoints.
# ASCII-only (Windows PowerShell 5.1 reads .ps1 as ANSI).
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$size = 1024
$out = Join-Path (Split-Path -Parent $PSScriptRoot) 'app-icon.png'

$bmp = New-Object System.Drawing.Bitmap $size, $size
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias

# Rounded dark background
$bg = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 23, 32, 48))
$g.FillRectangle($bg, 0, 0, $size, $size)

$accent = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 91, 141, 239)), 46
$accent.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$accent.EndCap = [System.Drawing.Drawing2D.LineCap]::Round

# The "thread" (an S-curve) from lower-left to upper-right
$path = New-Object System.Drawing.Drawing2D.GraphicsPath
$path.AddBezier(250, 780, 430, 780, 470, 640, 512, 512)
$path.AddBezier(512, 512, 554, 384, 594, 244, 774, 244)
$g.DrawPath($accent, $path)

# Two endpoints (local port, remote port)
$dot = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 236, 244, 255))
$g.FillEllipse($dot, 250 - 62, 780 - 62, 124, 124)
$g.FillEllipse($dot, 774 - 62, 244 - 62, 124, 124)

$g.Dispose()
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

Write-Host "icon written: $out"
