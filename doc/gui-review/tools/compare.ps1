# 把改造前/改造后截图拼成一张对比图（左 before、右 after，顶部标题条）
# 用法：
#   .\compare.ps1 -Before <png> -After <png> -Out <png> -Title "概览：字体重叠 + 长列表溢出"
param(
  [Parameter(Mandatory = $true)][string]$Before,
  [Parameter(Mandatory = $true)][string]$After,
  [Parameter(Mandatory = $true)][string]$Out,
  [Parameter(Mandatory = $true)][string]$Title
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$b = [System.Drawing.Image]::FromFile($Before)
$a = [System.Drawing.Image]::FromFile($After)

$pad = 16
$barH = 56
$labelH = 30
$w = [Math]::Max($b.Width, $a.Width)
$h = [Math]::Max($b.Height, $a.Height)

$canvasW = $pad * 3 + $w * 2
$canvasH = $barH + $labelH + $h + $pad

$bmp = New-Object System.Drawing.Bitmap $canvasW, $canvasH
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::ClearTypeGridFit
$g.Clear([System.Drawing.Color]::FromArgb(13, 17, 23))

$sans = New-Object System.Drawing.Font("Microsoft YaHei UI", 15, [System.Drawing.FontStyle]::Bold)
$sansS = New-Object System.Drawing.Font("Microsoft YaHei UI", 11, [System.Drawing.FontStyle]::Bold)
$white = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(230, 237, 243))
$dim = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(154, 164, 175))
$red = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(248, 81, 73))
$green = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(63, 185, 80))

$g.DrawString($Title, $sans, $white, $pad, 14)

$x1 = $pad
$x2 = $pad * 2 + $w
$labelY = $barH

$g.DrawString("BEFORE · 改造前", $sansS, $red, $x1, $labelY)
$g.DrawString("AFTER · 改造后", $sansS, $green, $x2, $labelY)

$imgY = $barH + $labelH
$g.DrawImage($b, $x1, $imgY, $b.Width, $b.Height)
$g.DrawImage($a, $x2, $imgY, $a.Width, $a.Height)

# 两张图的边框
$penB = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(90, 50, 56, 62), 2)
$g.DrawRectangle($penB, $x1, $imgY, $b.Width, $b.Height)
$g.DrawRectangle($penB, $x2, $imgY, $a.Width, $a.Height)

$g.Dispose()
$dir = Split-Path -Parent $Out
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
$b.Dispose()
$a.Dispose()
Write-Output "saved: $Out (${canvasW}x${canvasH})"
