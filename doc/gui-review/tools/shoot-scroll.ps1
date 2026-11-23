# GUI 截图工具（带滚轮）：启动/复用 bsend-gui，点击后在指定区域滚动，再抓窗口截图
# 用法：
#   .\shoot-scroll.ps1 -Exe <path> -Width 1280 -Height 820 -Out <png> [-Click "x,y"] [-Wheel 30] [-WheelX 450] [-WheelY 450] [-Wait 1500]
param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [Parameter(Mandatory = $true)][int]$Width,
  [Parameter(Mandatory = $true)][int]$Height,
  [Parameter(Mandatory = $true)][string]$Out,
  [string]$Click = "",
  [int]$Wheel = 0,
  [int]$WheelX = 450,
  [int]$WheelY = 450,
  [int]$Wait = 1600,
  [switch]$NoStart
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$sig = @'
using System;
using System.Runtime.InteropServices;
public class Win32Shot2 {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int ht, bool repaint);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern void SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
}
'@
if (-not ("Win32Shot2" -as [type])) { Add-Type -TypeDefinition $sig }

function Get-GuiWindow([string]$exePath) {
  $name = [System.IO.Path]::GetFileName($exePath)
  for ($i = 0; $i -lt 40; $i++) {
    $p = Get-Process | Where-Object { $_.ProcessName -eq [System.IO.Path]::GetFileNameWithoutExtension($exePath) }
    foreach ($proc in $p) {
      if ($proc.MainWindowHandle -ne 0) { return $proc }
    }
    Start-Sleep -Milliseconds 250
  }
  throw "找不到 GUI 窗口: $name"
}

if (-not $NoStart) {
  Get-Process -Name "bsend-gui" -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Milliseconds 600
  Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe) | Out-Null
}

$proc = Get-GuiWindow $Exe
$h = $proc.MainWindowHandle
Start-Sleep -Milliseconds 900

[Win32Shot2]::ShowWindow($h, 9) | Out-Null
[Win32Shot2]::MoveWindow($h, 40, 40, $Width, $Height, $true) | Out-Null
[Win32Shot2]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds $Wait

if ($Click -ne "") {
  $parts = $Click.Split(",")
  $r = New-Object Win32Shot2+RECT
  [Win32Shot2]::GetWindowRect($h, [ref]$r) | Out-Null
  $cx = $r.Left + [int]$parts[0]
  $cy = $r.Top + [int]$parts[1]
  [Win32Shot2]::SetCursorPos($cx, $cy) | Out-Null
  Start-Sleep -Milliseconds 120
  for ($i = 0; $i -lt 2; $i++) {
    [Win32Shot2]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 120
    [Win32Shot2]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 400
  }
  Start-Sleep -Milliseconds 900
}

if ($Wheel -gt 0) {
  $r = New-Object Win32Shot2+RECT
  [Win32Shot2]::GetWindowRect($h, [ref]$r) | Out-Null
  $wx = $r.Left + $WheelX
  $wy = $r.Top + $WheelY
  [Win32Shot2]::SetCursorPos($wx, $wy) | Out-Null
  Start-Sleep -Milliseconds 200
  for ($i = 0; $i -lt $Wheel; $i++) {
    [Win32Shot2]::mouse_event(0x0800, 0, 0, [uint32]4294967168, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 90
  }
  Start-Sleep -Milliseconds 600
}

$r = New-Object Win32Shot2+RECT
[Win32Shot2]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.Right - $r.Left
$ht = $r.Bottom - $r.Top

$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size $w, $ht))
$g.Dispose()
$dir = Split-Path -Parent $Out
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "saved: $Out  (${w}x${ht})"
