<#
.SYNOPSIS
  只读真站巡检：跑 status/report/followup --dry-run，并核对指定已发种子的详情页、
  qB 上传量和 tracker 回包状态。

.DESCRIPTION
  不发布、不挂种、不改覆盖率；followup 只加 --dry-run。qB 只调用 auth/login、
  torrents/info、torrents/trackers 三个只读接口。输出中的 passkey/token 会脱敏。
  站点不可达或 followup 报错时退出码非零，但不会写 coverage.json。

.PARAMETER ConfigPath
  boxsend.json 路径。缺省 %LOCALAPPDATA%\BoxSend\boxsend.json。

.PARAMETER BsendPath
  bsend.exe 路径。缺省取仓库 rust\target\release\bsend.exe，再退回 PATH。

.PARAMETER Sites
  followup 默认巡检的站点，逗号分隔。默认 city13,hdvideo,pthome。

.PARAMETER FollowupLimit
  followup 最多回访条数，默认 5。

.PARAMETER SkipFollowup
  只跑 status/report 和本地详情核对，不打开站点详情页。

.PARAMETER OutDir
  输出目录。缺省 artifacts\live-smoke-<时间戳>。

.EXAMPLE
  pwsh scripts/live-smoke.ps1
  pwsh scripts/live-smoke.ps1 -Sites city13,hdvideo -FollowupLimit 2
#>
[CmdletBinding()]
param(
    [string]$ConfigPath,
    [string]$BsendPath,
    [string]$Sites = 'city13,hdvideo,pthome',
    [ValidateRange(1, 50)][int]$FollowupLimit = 5,
    [switch]$SkipFollowup,
    [string]$OutDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Remove-Credential([string]$Text) {
    if (-not $Text) { return '' }
    $t = $Text -replace '(?i)(passkey|torrent_pass|authkey|token|cookie|password)=([^&\s]+)', '$1=***'
    $t = $t -replace '(?i)(passkey|torrent_pass|authkey|token)/([A-Za-z0-9]+)', '$1/***'
    return $t
}

function Get-Field([object]$Obj, [string]$Name, $Default = $null) {
    if ($null -eq $Obj) { return $Default }
    if ($Obj -is [System.Collections.IDictionary]) {
        if ($Obj.Contains($Name)) { return $Obj[$Name] }
        return $Default
    }
    $prop = $Obj.PSObject.Properties[$Name]
    if ($null -eq $prop) { return $Default }
    return $prop.Value
}

function Format-Bytes([object]$Value) {
    $n = 0L
    if (-not [long]::TryParse([string]$Value, [ref]$n)) { return '—' }
    if ($n -lt 1024) { return "$n B" }
    $units = @('KiB', 'MiB', 'GiB', 'TiB')
    $v = [double]$n
    foreach ($u in $units) {
        $v = $v / 1024
        if ($v -lt 1024 -or $u -eq 'TiB') { return ('{0:F2} {1}' -f $v, $u) }
    }
}

function Format-Unix([object]$Value) {
    $n = 0L
    if (-not [long]::TryParse([string]$Value, [ref]$n)) { return '—' }
    if ($n -le 0) { return '—' }
    return [DateTimeOffset]::FromUnixTimeSeconds($n).ToLocalTime().ToString('yyyy-MM-dd HH:mm')
}

function Find-CoverageEntry($Contents, [string]$Site, [string]$TorrentId) {
    foreach ($key in $Contents.Keys) {
        $entry = $Contents[$key]
        $cells = Get-Field $entry 'coverage' @{}
        $cell = Get-Field $cells $Site $null
        if ($null -eq $cell) { continue }
        $url = [string](Get-Field $cell 'url' '')
        if ($url -notmatch "id=$TorrentId(?:&|\z)") { continue }
        return [pscustomobject]@{ Key = [string]$key; Entry = $entry; Cell = $cell; Url = $url }
    }
    return $null
}

function Find-PostedHash($Entry, [string]$TrackerHost) {
    $torrents = Get-Field $Entry 'torrents' @{}
    $best = $null
    $bestAt = -1L
    foreach ($hash in $torrents.Keys) {
        $t = $torrents[$hash]
        $hosts = @(Get-Field $t 'trackerHosts' @())
        if (-not ($hosts | Where-Object { $_ -match [regex]::Escape($TrackerHost) })) { continue }
        $at = 0L
        [void][long]::TryParse([string](Get-Field $t 'addedOn' 0), [ref]$at)
        if ($at -gt $bestAt) { $bestAt = $at; $best = [string]$hash }
    }
    return $best
}

function Invoke-QbRead($Base, $Session, [string]$Hash) {
    $info = Invoke-RestMethod -Method Get -Uri "$Base/api/v2/torrents/info?hashes=$Hash" `
        -WebSession $Session -TimeoutSec 15
    $trackers = Invoke-RestMethod -Method Get -Uri "$Base/api/v2/torrents/trackers?hash=$Hash" `
        -WebSession $Session -TimeoutSec 15
    $row = @($info) | Select-Object -First 1
    if ($null -eq $row) { return $null }
    return [pscustomobject]@{
        Uploaded      = [long](Get-Field $row 'uploaded' 0)
        Downloaded    = [long](Get-Field $row 'downloaded' 0)
        State         = [string](Get-Field $row 'state' '')
        Progress      = [double](Get-Field $row 'progress' 0)
        NumComplete   = [long](Get-Field $row 'num_complete' 0)
        NumIncomplete = [long](Get-Field $row 'num_incomplete' 0)
        Trackers      = @($trackers)
    }
}

if (-not $ConfigPath) {
    $ConfigPath = Join-Path $env:LOCALAPPDATA 'BoxSend\boxsend.json'
}
if (-not (Test-Path -LiteralPath $ConfigPath)) {
    throw "找不到配置：$ConfigPath（先让 GUI 保存一次，或用 -ConfigPath 指定）"
}
$ConfigPath = (Resolve-Path -LiteralPath $ConfigPath).Path
$cfg = Get-Content -Raw -LiteralPath $ConfigPath | ConvertFrom-Json

if (-not $BsendPath) {
    $repoRoot = Split-Path -Parent $PSScriptRoot
    $candidate = Join-Path $repoRoot 'rust\target\release\bsend.exe'
    if (Test-Path -LiteralPath $candidate) {
        $BsendPath = $candidate
    } else {
        $BsendPath = (Get-Command bsend -ErrorAction Stop).Source
    }
}
if (-not (Test-Path -LiteralPath $BsendPath)) { throw "找不到 bsend：$BsendPath" }

$dataDirRaw = [string](Get-Field $cfg 'dataDir' '')
$dataDir = if ($dataDirRaw) { $dataDirRaw } else { Split-Path -Parent $ConfigPath }
$dataDir = [IO.Path]::GetFullPath(($dataDir -replace '/', [IO.Path]::DirectorySeparatorChar))
$coveragePath = Join-Path $dataDir 'coverage.json'

if (-not $OutDir) {
    $OutDir = Join-Path (Split-Path -Parent $PSScriptRoot) ('artifacts\live-smoke-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$failures = [System.Collections.Generic.List[string]]::new()

# 1) status / report：只读，不落覆盖率
$statusOut = Join-Path $OutDir 'status.txt'
$reportOut = Join-Path $OutDir 'report.txt'
& $BsendPath --config $ConfigPath status *> $statusOut
if ($LASTEXITCODE -ne 0) { $failures.Add("bsend status 退出码 $LASTEXITCODE") }
& $BsendPath --config $ConfigPath report --top 5 *> $reportOut
if ($LASTEXITCODE -ne 0) { $failures.Add("bsend report 退出码 $LASTEXITCODE") }

# 2) followup --dry-run：会打开详情页，但不改格子、不挂种
$followupOut = Join-Path $OutDir 'followup.json'
$followupErr = Join-Path $OutDir 'followup.log'
$followupSummary = $null
if (-not $SkipFollowup) {
    & $BsendPath --config $ConfigPath followup --dry-run --sites $Sites --limit $FollowupLimit --json 1> $followupOut 2> $followupErr
    if ($LASTEXITCODE -ne 0) { $failures.Add("bsend followup 退出码 $LASTEXITCODE") }
    try {
        $raw = Get-Content -Raw -LiteralPath $followupOut
        $start = $raw.IndexOf('{')
        $end = $raw.LastIndexOf('}')
        if ($start -lt 0 -or $end -lt $start) { throw '没有 JSON 对象' }
        $followupSummary = $raw.Substring($start, $end - $start + 1) | ConvertFrom-Json
    } catch {
        $failures.Add('followup 没输出可解析的 JSON')
        $followupSummary = $null
    }
    if ($followupSummary -and [int](Get-Field $followupSummary 'errors' 0) -gt 0) {
        $failures.Add("followup 有 $($followupSummary.errors) 条打不开")
    }
}

# 3) 指定已发种子的详情页 + qB 上传量 + tracker 状态
$targets = @(
    @{ Site = 'city13'; Id = '88467'; TrackerHost = 'tracker.13city.org'; TrackerPattern = 'tracker\.13city\.org' },
    @{ Site = 'hdvideo'; Id = '79168'; TrackerHost = 'hdvideo.top'; TrackerPattern = 'tracker\.hdvideo\.top|hdvideo\.top' },
    @{ Site = 'pthome'; Id = '285040'; TrackerHost = 't.pthome.org'; TrackerPattern = 't\.pthome\.org|tracker\.pthome\.org' }
)

$qbBase = ''
$qbSession = $null
$dl = Get-Field $cfg 'downloader' $null
$qbUrl = [string](Get-Field $dl 'url' '')
if ($qbUrl) {
    $qbBase = $qbUrl.TrimEnd('/')
    $qbSession = New-Object Microsoft.PowerShell.Commands.WebRequestSession
    $body = @{
        username = [string](Get-Field $dl 'username' '')
        password = [string](Get-Field $dl 'password' '')
    }
    $login = Invoke-RestMethod -Method Post -Uri "$qbBase/api/v2/auth/login" `
        -Body $body -WebSession $qbSession -TimeoutSec 8
    if ($null -ne $login -and [string]$login -match 'Fails') {
        $failures.Add('qB 登录被拒绝')
        $qbSession = $null
    }
}

$rows = [System.Collections.Generic.List[object]]::new()
if (Test-Path -LiteralPath $coveragePath) {
    $coverage = Get-Content -Raw -LiteralPath $coveragePath | ConvertFrom-Json -AsHashtable
    $contents = Get-Field $coverage 'contents' @{}
    foreach ($t in $targets) {
        $found = Find-CoverageEntry $contents $t.Site $t.Id
        if ($null -eq $found) {
            $rows.Add([pscustomobject]@{
                    Site = $t.Site; Id = $t.Id; Name = '（覆盖表里没找到）'; Status = '—'
                    Url = '—'; Hash = '—'; Uploaded = '—'; Ratio = '—'; Peers = '—'
                    TrackerStatus = '—'; Note = '—'
                })
            continue
        }
        $hash = Find-PostedHash $found.Entry $t.TrackerHost
        $uploaded = '—'; $ratio = '—'; $peers = '—'; $trackerStatus = '—'; $trMsg = ''
        if ($qbSession -and $hash) {
            try {
                $qb = Invoke-QbRead $qbBase $qbSession $hash
                if ($qb) {
                    $uploaded = Format-Bytes $qb.Uploaded
                    $ratio = if ($qb.Downloaded -gt 0) { '{0:F2}' -f ($qb.Uploaded / $qb.Downloaded) } else { '∞' }
                    $peers = "$($qb.NumComplete) 做种 / $($qb.NumIncomplete) 下载"
                    $matching = @($qb.Trackers | Where-Object {
                            $u = [string](Get-Field $_ 'url' '')
                            $u -match $t.TrackerPattern
                        })
                    if ($matching.Count -gt 0) {
                        $trackerStatus = (@($matching | ForEach-Object {
                                [string](Get-Field $_ 'status' '?')
                            } | Sort-Object -Unique) -join '/')
                        $msgCandidates = @($matching | ForEach-Object {
                                Remove-Credential ([string](Get-Field $_ 'msg' ''))
                            } | Where-Object { $_ })
                        $trMsg = if ($msgCandidates.Count -gt 0) { [string]$msgCandidates[0] } else { '' }
                    }
                }
            } catch {
                $failures.Add("$($t.Site) $($t.Id) qB 查询失败：$(Remove-Credential $_.Exception.Message)")
            }
        }
        $note = Remove-Credential ([string](Get-Field $found.Cell 'note' ''))
        if ($trMsg) { $note = "$note；tracker: $trMsg" }
        $rows.Add([pscustomobject]@{
                Site = $t.Site
                Id = $t.Id
                Name = [string](Get-Field $found.Entry 'name' '')
                Status = [string](Get-Field $found.Cell 'status' '')
                Url = $found.Url
                Hash = if ($hash) { $hash.Substring(0, [Math]::Min(12, $hash.Length)) } else { '—' }
                Uploaded = $uploaded
                Ratio = $ratio
                Peers = $peers
                TrackerStatus = $trackerStatus
                Note = $note
            })
    }
} else {
    $failures.Add("没有覆盖表：$coveragePath")
}

$summary = [ordered]@{
    generatedAt = [DateTimeOffset]::Now.ToString('yyyy-MM-dd HH:mm:ss zzz')
    config = $ConfigPath
    bsend = $BsendPath
    coverage = $coveragePath
    followup = $followupSummary
    seeds = @($rows)
    failures = @($failures)
}
$summaryJson = Join-Path $OutDir 'summary.json'
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryJson -Encoding utf8

$md = [System.Text.StringBuilder]::new()
[void]$md.AppendLine('# BoxSend 真站只读巡检')
[void]$md.AppendLine('')
[void]$md.AppendLine("- 时间：$($summary.generatedAt)")
[void]$md.AppendLine("- 配置：$ConfigPath")
[void]$md.AppendLine("- bsend：$BsendPath")
if ($followupSummary) {
    [void]$md.AppendLine("- followup：选出 $($followupSummary.selected)、打开 $($followupSummary.checked)、打不开 $($followupSummary.errors)")
} else {
    [void]$md.AppendLine('- followup：已跳过')
}
[void]$md.AppendLine('')
[void]$md.AppendLine('| 站点 | 种子 | 覆盖状态 | 上传量 | 比率 | 同伴 | tracker |')
[void]$md.AppendLine('|---|---|---|---|---|---|---|')
foreach ($r in $rows) {
    [void]$md.AppendLine("| $($r.Site) | [$($r.Id)]($($r.Url)) | $($r.Status) | $($r.Uploaded) | $($r.Ratio) | $($r.Peers) | $($r.TrackerStatus) |")
}
[void]$md.AppendLine('')
foreach ($r in $rows) {
    [void]$md.AppendLine("## $($r.Site) $($r.Id)")
    [void]$md.AppendLine('')
    [void]$md.AppendLine("名称：$($r.Name)")
    [void]$md.AppendLine('')
    [void]$md.AppendLine("详情页：$($r.Url)")
    [void]$md.AppendLine('')
    [void]$md.AppendLine("qB hash：$($r.Hash)　上传：$($r.Uploaded)　同伴：$($r.Peers)　tracker：$($r.TrackerStatus)")
    [void]$md.AppendLine('')
    [void]$md.AppendLine("备注：$($r.Note)")
    [void]$md.AppendLine('')
}
if ($failures.Count -gt 0) {
    [void]$md.AppendLine('## 失败项')
    [void]$md.AppendLine('')
    foreach ($f in $failures) { [void]$md.AppendLine("- $f") }
} else {
    [void]$md.AppendLine('## 失败项')
    [void]$md.AppendLine('')
    [void]$md.AppendLine('无')
}
$mdPath = Join-Path $OutDir 'summary.md'
$md.ToString() | Set-Content -LiteralPath $mdPath -Encoding utf8

Write-Output "输出目录：$OutDir"
$rows | Format-Table Site, Id, Status, Uploaded, Ratio, Peers, TrackerStatus -AutoSize
if ($failures.Count -gt 0) {
    Write-Output "失败 $($failures.Count) 项："
    $failures | ForEach-Object { Write-Output "  - $_" }
    exit 2
}
exit 0
