<#
.SYNOPSIS
  只读巡检 BoxSend：覆盖率异常格、最近未成种记录、下载器里未做种的 Boxsend 种子。

.DESCRIPTION
  只调用 qBittorrent 的 auth/login 与 torrents/info 两个接口，不向任何站点 POST，
  不修改覆盖率/配置/cookie。默认输出 Markdown；-Json 输出机器可读 JSON。
  输出中不含 cookie、passkey、密码等凭据（URL 查询参数会脱敏）。

.PARAMETER ConfigPath
  boxsend.json 路径。缺省 %LOCALAPPDATA%\BoxSend\boxsend.json。

.PARAMETER Days
  reseed-log.jsonl 回看天数（1-90，默认 7）。

.PARAMETER Limit
  每类明细最多列多少条（默认 50，汇总计数不受限）。

.PARAMETER Json
  输出 JSON 而不是 Markdown。

.PARAMETER OutFile
  写入文件；缺省输出到 stdout。

.EXAMPLE
  pwsh scripts/seeding-audit.ps1
  pwsh scripts/seeding-audit.ps1 -Days 14 -Limit 100 -OutFile audit.md
  pwsh scripts/seeding-audit.ps1 -Json -OutFile audit.json
#>
[CmdletBinding()]
param(
    [string]$ConfigPath,
    [ValidateRange(1, 90)][int]$Days = 7,
    [ValidateRange(1, 500)][int]$Limit = 50,
    [switch]$Json,
    [string]$OutFile
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Remove-Credential([string]$Text) {
    if (-not $Text) { return '' }
    $t = $Text -replace '(?i)(passkey|torrent_pass|authkey|token|cookie|password)=([^&\s]+)', '$1=***'
    $t = $t -replace '(?i)(passkey|torrent_pass|authkey|token)/([A-Za-z0-9]+)', '$1/***'
    return $t
}

function Format-Unix([object]$Value) {
    if ($null -eq $Value) { return '—' }
    $n = 0L
    if (-not [long]::TryParse([string]$Value, [ref]$n)) { return '—' }
    if ($n -le 0) { return '—' }
    return [DateTimeOffset]::FromUnixTimeSeconds($n).ToLocalTime().ToString('yyyy-MM-dd HH:mm')
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

if (-not $ConfigPath) {
    $ConfigPath = Join-Path $env:LOCALAPPDATA 'BoxSend\boxsend.json'
}
if (-not (Test-Path -LiteralPath $ConfigPath)) {
    throw "找不到配置：$ConfigPath（先让 GUI 保存一次，或用 -ConfigPath 指定）"
}

$cfg = Get-Content -Raw -LiteralPath $ConfigPath | ConvertFrom-Json
$dataDirRaw = [string](Get-Field $cfg 'dataDir' '')
$dataDir = if ($dataDirRaw) { $dataDirRaw } else { Split-Path -Parent (Resolve-Path -LiteralPath $ConfigPath).Path }
$dataDir = [IO.Path]::GetFullPath(($dataDir -replace '/', [IO.Path]::DirectorySeparatorChar))
$coveragePath = Join-Path $dataDir 'coverage.json'
$reseedLogPath = Join-Path $dataDir 'reseed-log.jsonl'

# 状态 → 人话；只收「需要人看一眼」的状态，seeding/assisted/reseeded/miss 不算问题
$statusLabels = [ordered]@{
    'failed'         = '推送/上传失败'
    'verifying'      = '推送后未出校验结果'
    'reseed_draft'   = '转种草稿待发'
    'review_pending' = 'A/B/C 级命中待人工'
    'other_version'  = '同片异版本，无法跨种'
    'same_hash_skip' = '同 info hash，按策略跳过'
    'wait_source'    = '缺源，待找源'
}

$coverageIssues = [System.Collections.Generic.List[object]]::new()
$statusCounts = [ordered]@{}
$contentCount = 0

if (Test-Path -LiteralPath $coveragePath) {
    $coverage = Get-Content -Raw -LiteralPath $coveragePath | ConvertFrom-Json -AsHashtable
    $contents = Get-Field $coverage 'contents' @{}
    $contentCount = @($contents.Keys).Count
    foreach ($key in $contents.Keys) {
        $entry = $contents[$key]
        $cells = Get-Field $entry 'coverage' @{}
        foreach ($site in $cells.Keys) {
            $cell = $cells[$site]
            $status = [string](Get-Field $cell 'status' '')
            if (-not $status) { continue }
            if ($statusLabels.Contains($status)) {
                if (-not $statusCounts.Contains($status)) { $statusCounts[$status] = 0 }
                $statusCounts[$status]++
                $coverageIssues.Add([pscustomobject]@{
                        Key    = [string]$key
                        Name   = [string](Get-Field $entry 'name' '')
                        Site   = [string]$site
                        Status = $status
                        Label  = $statusLabels[$status]
                        At     = Format-Unix (Get-Field $cell 'at' $null)
                        Note   = Remove-Credential ([string](Get-Field $cell 'note' ''))
                    })
            }
        }
    }
} else {
    Write-Warning "没有 $coveragePath，跳过覆盖率巡检。"
}

# reseed-log.jsonl：只看 seeded=false（发布了但没在本机做成种 / 推种失败）
$seededFalse = [System.Collections.Generic.List[object]]::new()
$bySite = [ordered]@{}
if (Test-Path -LiteralPath $reseedLogPath) {
    $cutoff = [DateTimeOffset]::UtcNow.AddDays(-$Days).ToUnixTimeSeconds()
    foreach ($line in Get-Content -LiteralPath $reseedLogPath -ErrorAction SilentlyContinue) {
        if (-not $line.Trim()) { continue }
        try { $rec = $line | ConvertFrom-Json -AsHashtable } catch { continue }
        $at = 0L
        if (-not [long]::TryParse([string](Get-Field $rec 'at' 0), [ref]$at)) { continue }
        if ($at -lt $cutoff) { continue }
        if ([bool](Get-Field $rec 'seeded' $true)) { continue }
        $site = [string](Get-Field $rec 'target_site' '?')
        if (-not $bySite.Contains($site)) { $bySite[$site] = 0 }
        $bySite[$site]++
        $seededFalse.Add([pscustomobject]@{
                At     = Format-Unix $at
                Site   = $site
                Name   = [string](Get-Field $rec 'name' '')
                Result = [string](Get-Field $rec 'result' '')
                Note   = Remove-Credential ([string](Get-Field $rec 'note' ''))
            })
    }
} else {
    Write-Warning "没有 $reseedLogPath，跳过未成种记录巡检。"
}

# qBittorrent：只读登录 + 列表，挑带 Boxsend 标签且 progress<1 的种子
$qbNotSeeding = [System.Collections.Generic.List[object]]::new()
$qbNote = ''
$dl = Get-Field $cfg 'downloader' $null
$qbUrl = [string](Get-Field $dl 'url' '')
if ($qbUrl) {
    try {
        $base = $qbUrl.TrimEnd('/')
        $session = New-Object Microsoft.PowerShell.Commands.WebRequestSession
        $body = @{
            username = [string](Get-Field $dl 'username' '')
            password = [string](Get-Field $dl 'password' '')
        }
        $login = Invoke-RestMethod -Method Post -Uri "$base/api/v2/auth/login" `
            -Body $body -WebSession $session -TimeoutSec 8
        # qB 5.3 在本地免密/已登录时直接返回 204 空体；是否成功以列表接口为准
        if ($null -ne $login -and [string]$login -match 'Fails') {
            throw '登录被拒绝（检查用户名/密码/本地 IP 白名单）'
        }
        $list = Invoke-RestMethod -Method Get -Uri "$base/api/v2/torrents/info" `
            -WebSession $session -TimeoutSec 20
        foreach ($t in @($list)) {
            $tags = [string](Get-Field $t 'tags' '')
            if ($tags -notmatch '(^|,)\s*Boxsend\s*(,|$)') { continue }
            $progress = [double](Get-Field $t 'progress' 0)
            if ($progress -ge 1) { continue }
            $qbNotSeeding.Add([pscustomobject]@{
                    Name     = [string](Get-Field $t 'name' '')
                    Progress = [math]::Round($progress * 100, 1)
                    State    = [string](Get-Field $t 'state' '')
                    Hash     = ([string](Get-Field $t 'hash' '')).Substring(0, [Math]::Min(8, ([string](Get-Field $t 'hash' '')).Length))
                })
        }
    } catch {
        $qbNote = "下载器巡检跳过：$(Remove-Credential $_.Exception.Message)"
    }
} else {
    $qbNote = '配置里没有 downloader.url，跳过下载器巡检'
}

$totalSeededFalse = $seededFalse.Count
$totalQb = $qbNotSeeding.Count
$report = [ordered]@{
    generatedAt = [DateTimeOffset]::Now.ToString('yyyy-MM-dd HH:mm:ss zzz')
    config      = (Resolve-Path -LiteralPath $ConfigPath).Path
    dataDir     = $dataDir
    coverage    = [ordered]@{
        contents = $contentCount
        issues   = @($coverageIssues | Select-Object -First $Limit)
        counts   = $statusCounts
    }
    reseedLog   = [ordered]@{
        days        = $Days
        seededFalse = @($seededFalse | Select-Object -First $Limit)
        total       = $totalSeededFalse
        bySite      = $bySite
    }
    downloader  = [ordered]@{
        note       = $qbNote
        notSeeding = @($qbNotSeeding | Select-Object -First $Limit)
        total      = $totalQb
    }
    summary     = [ordered]@{
        coverageIssues = $coverageIssues.Count
        seededFalse    = $totalSeededFalse
        qbNotSeeding   = $totalQb
    }
}

if ($Json) {
    $text = $report | ConvertTo-Json -Depth 8
} else {
    $sb = [System.Text.StringBuilder]::new()
    [void]$sb.AppendLine('# BoxSend 只读巡检')
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("- 生成时间：$($report.generatedAt)")
    [void]$sb.AppendLine("- 配置：``$($report.config)``")
    [void]$sb.AppendLine("- 数据目录：``$dataDir``")
    [void]$sb.AppendLine("- 汇总：覆盖率异常 **$($coverageIssues.Count)** 格｜最近 $Days 天未成种 **$totalSeededFalse** 条｜下载器未做种 **$totalQb** 个")
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("## 1. 覆盖率异常格")
    [void]$sb.AppendLine()
    if ($coverageIssues.Count -eq 0) {
        [void]$sb.AppendLine('无。')
    } else {
        [void]$sb.AppendLine('| 站点 | 状态 | 内容 | 最近变更 | 备注 |')
        [void]$sb.AppendLine('| --- | --- | --- | --- | --- |')
        $shown = $coverageIssues | Select-Object -First $Limit
        foreach ($r in $shown) {
            $name = ($r.Name -replace '\|', '\|')
            $note = ($r.Note -replace '\|', '\|')
            [void]$sb.AppendLine("| $($r.Site) | $($r.Label) | $name | $($r.At) | $note |")
        }
        if ($coverageIssues.Count -gt $Limit) {
            [void]$sb.AppendLine()
            [void]$sb.AppendLine("（仅列前 $Limit 条，共 $($coverageIssues.Count) 条）")
        }
    }
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("## 2. 最近 $Days 天未成种记录")
    [void]$sb.AppendLine()
    if ($totalSeededFalse -eq 0) {
        [void]$sb.AppendLine('无。')
    } else {
        [void]$sb.AppendLine('| 时间 | 目标站 | 内容 | 结果 | 备注 |')
        [void]$sb.AppendLine('| --- | --- | --- | --- | --- |')
        foreach ($r in ($seededFalse | Select-Object -First $Limit)) {
            $name = ($r.Name -replace '\|', '\|')
            $note = ($r.Note -replace '\|', '\|')
            [void]$sb.AppendLine("| $($r.At) | $($r.Site) | $name | $($r.Result) | $note |")
        }
        if ($bySite.Count -gt 0) {
            [void]$sb.AppendLine()
            $parts = foreach ($k in $bySite.Keys) { "$k=$($bySite[$k])" }
            [void]$sb.AppendLine("按站：$($parts -join '，')")
        }
    }
    [void]$sb.AppendLine()
    [void]$sb.AppendLine('## 3. 下载器里未做种的 Boxsend 种子')
    [void]$sb.AppendLine()
    if ($qbNote) {
        [void]$sb.AppendLine($qbNote)
    } elseif ($totalQb -eq 0) {
        [void]$sb.AppendLine('无。')
    } else {
        [void]$sb.AppendLine('| 种子 | 进度 | 状态 | hash 前 8 位 |')
        [void]$sb.AppendLine('| --- | --- | --- | --- |')
        foreach ($r in ($qbNotSeeding | Select-Object -First $Limit)) {
            $name = ($r.Name -replace '\|', '\|')
            [void]$sb.AppendLine("| $name | $($r.Progress)% | $($r.State) | $($r.Hash) |")
        }
    }
    $text = $sb.ToString()
}

if ($OutFile) {
    $parent = Split-Path -Parent $OutFile
    if ($parent -and -not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    Set-Content -LiteralPath $OutFile -Value $text -Encoding utf8
    Write-Output ("只读巡检完成：覆盖率异常 {0} 格，未成种 {1} 条，下载器未做种 {2} 个 → {3}" -f `
            $coverageIssues.Count, $totalSeededFalse, $totalQb, $OutFile)
} else {
    Write-Output $text
}
