# 核图：扫描概览表头行与首数据行的文字像素簇，输出各簇左右边缘
param(
  [Parameter(Mandatory = $true)][string]$Img,
  [int]$HeaderY1 = 507,
  [int]$HeaderY2 = 518,
  [int]$RowY1 = 543,
  [int]$RowY2 = 554
)
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap($Img)

function Cluster-Edges($y1, $y2, $label) {
  $cols = New-Object 'bool[]' $bmp.Width
  for ($y = $y1; $y -le $y2; $y++) {
    for ($x = 0; $x -lt $bmp.Width; $x++) {
      $c = $bmp.GetPixel($x, $y)
      if ($c.R -gt 85 -or $c.G -gt 90 -or $c.B -gt 95) { $cols[$x] = $true }
    }
  }
  $starts = New-Object System.Collections.Generic.List[int]
  $ends = New-Object System.Collections.Generic.List[int]
  $start = -1
  for ($x = 0; $x -lt $bmp.Width; $x++) {
    if ($cols[$x]) { if ($start -lt 0) { $start = $x } }
    else { if ($start -ge 0) { $starts.Add($start); $ends.Add($x - 1); $start = -1 } }
  }
  if ($start -ge 0) { $starts.Add($start); $ends.Add($bmp.Width - 1) }
  # 合并间距 <=6px
  $mS = New-Object System.Collections.Generic.List[int]
  $mE = New-Object System.Collections.Generic.List[int]
  for ($i = 0; $i -lt $starts.Count; $i++) {
    if ($mS.Count -gt 0 -and $starts[$i] - $mE[$mE.Count - 1] -le 6) {
      $mE[$mE.Count - 1] = $ends[$i]
    } else { $mS.Add($starts[$i]); $mE.Add($ends[$i]) }
  }
  Write-Output "== $label =="
  for ($i = 0; $i -lt $mS.Count; $i++) {
    Write-Output ("  x {0,4}-{1,4}  w={2}" -f $mS[$i], $mE[$i], ($mE[$i]-$mS[$i]+1))
  }
}

Cluster-Edges $HeaderY1 $HeaderY2 "HEADER"
Cluster-Edges $RowY1 $RowY2 "ROW1"
$bmp.Dispose()
