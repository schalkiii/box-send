# GUI 截图工具：启动 bsend-gui、按指定尺寸摆放窗口、截图窗口区域
# 用法：
#   .\shoot.ps1 -Exe <path> -Width 1200 -Height 760 -Out <png> [-Click "x,y"] [-Wait 1500]
param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [Parameter(Mandatory = $true)][int]$Width,
  [Parameter(Mandatory = $true)][int]$Height,
  [Parameter(Mandatory = $true)][string]$Out,
  [string]$Click = "",
  [int]$Wait = 1600,
  [switch]$NoStart
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$sig = @'
using System;
using System.Runtime.InteropServices;
public class Win32Shot {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int ht, bool repaint);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
}
'@
if (-not ("Win32Shot" -as [type])) { Add-Type -TypeDefinition $sig }

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
  # 先清掉同名的旧实例，避免截到旧窗口
  Get-Process -Name "bsend-gui" -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Milliseconds 600
  Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe) | Out-Null
}

$proc = Get-GuiWindow $Exe
$h = $proc.MainWindowHandle
Start-Sleep -Milliseconds 900

# 摆放窗口：屏幕左上角起，留 40px 边距
[Win32Shot]::ShowWindow($h, 9) | Out-Null   # SW_RESTORE
[Win32Shot]::MoveWindow($h, 40, 40, $Width, $Height, $true) | Out-Null
[Win32Shot]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds $Wait

if ($Click -ne "") {
  $parts = $Click.Split(",")
  $r = New-Object Win32Shot+RECT
  [Win32Shot]::GetWindowRect($h, [ref]$r) | Out-Null
  $cx = $r.Left + [int]$parts[0]
  $cy = $r.Top + [int]$parts[1]
  [Win32Shot]::SetCursorPos($cx, $cy) | Out-Null
  Start-Sleep -Milliseconds 120
  # 点两次：第一次激活窗口，第二次才落在控件上；down/up 之间留一帧，GPUI 才认这次点击
  for ($i = 0; $i -lt 2; $i++) {
    [Win32Shot]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)   # LEFTDOWN
    Start-Sleep -Milliseconds 120
    [Win32Shot]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)   # LEFTUP
    Start-Sleep -Milliseconds 400
  }
  Start-Sleep -Milliseconds 900
}

$r = New-Object Win32Shot+RECT
[Win32Shot]::GetWindowRect($h, [ref]$r) | Out-Null
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
