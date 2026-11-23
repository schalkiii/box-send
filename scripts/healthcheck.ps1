<#
.SYNOPSIS
  只读健康巡检 BoxSend：配置/账本可解析、qB 连通、未完成种子、近期审计失败率、退避到期数。

.DESCRIPTION
  纯只读：不新建/删除/修改任何文件，不改 qB 状态，不访问 PT 站点；对 qB 只发
  auth/login 与只读 GET。输出不含用户名/密码/cookie/SID/passkey/token。
  单项异常不中断其它项；仅配置不可解析时直接 FAIL 退出（后续检查都依赖它）。

.PARAMETER Config
  boxsend.json 路径。缺省 %LOCALAPPDATA%\BoxSend\boxsend.json。

.PARAMETER Days
  回看 auto-audit.jsonl 的天数（北京时间日界），默认 1。

.PARAMETER Json
  额外在 stdout 末尾输出机器可读 JSON 对象（overall / checks[]）。

.EXAMPLE
  pwsh scripts/healthcheck.ps1
  pwsh scripts/healthcheck.ps1 -Days 3 -Json
#>
[CmdletBinding()]
param(
    [string]$Config,
    [ValidateRange(1, 90)][int]$Days = 1,
    [switch]$Json
)

$ErrorActionPreference = 'Stop'

# ---------- helpers ----------

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

function Remove-Credential([string]$Text) {
    if (-not $Text) { return '' }
    $t = $Text -replace '(?i)(passkey|torrent_pass|authkey|token|cookie|password|sid)=([^&\s]+)', '$1=***'
    $t = $t -replace '(?i)(passkey|torrent_pass|authkey|token)/([A-Za-z0-9]+)', '$1/***'
    return $t
}

$checks = [System.Collections.Generic.List[object]]::new()
$icon = @{ ok = '✅'; warn = '⚠️'; fail = '❌'; info = 'ℹ️' }

function Add-Check([string]$Id, [string]$Level, [string]$Message, $Value = $null) {
    $checks.Add([pscustomobject]@{
            id      = $Id
            level   = $Level
            message = $Message
            value   = $Value
        })
}

# ---------- resolve config path ----------

if (-not $Config) {
    $Config = Join-Path $env:LOCALAPPDATA 'BoxSend\boxsend.json'
}

$nowUtc = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()

# ---------- 1. 配置可解析 ----------

$cfg = $null
$configPathResolved = $Config
try {
    if (-not (Test-Path -LiteralPath $Config)) {
        throw "找不到配置文件：$Config"
    }
    $configPathResolved = (Resolve-Path -LiteralPath $Config).Path
    $cfg = Get-Content -Raw -LiteralPath $configPathResolved | ConvertFrom-Json

    $missingSections = [System.Collections.Generic.List[string]]::new()
    if ($null -eq (Get-Field $cfg 'downloader' $null)) { $missingSections.Add('downloader') }
    $siteBag = Get-Field $cfg 'sites' $null
    if ($null -eq $siteBag) { $siteBag = Get-Field $cfg 'sourceSites' $null }
    if ($null -eq $siteBag) { $siteBag = Get-Field $cfg 'targetSites' $null }
    if ($null -eq $siteBag) { $missingSections.Add('sites/sourceSites') }
    if ($null -eq (Get-Field $cfg 'assist' $null)) { $missingSections.Add('assist') }

    if ($missingSections.Count -gt 0) {
        throw "配置缺少关键段：$($missingSections -join '、')"
    }

    $sourceCount = @(Get-Field $cfg 'sourceSites' @()).Count
    $targetCount = @(Get-Field $cfg 'targetSites' @()).Count
    Add-Check 'config' 'ok' "配置可解析：downloader / 站点清单 / assist 齐全（源站 $sourceCount 个，目标站 $targetCount 个）" `
        ([ordered]@{ sourceSites = $sourceCount; targetSites = $targetCount })
} catch {
    Add-Check 'config' 'fail' "配置不可解析：$(Remove-Credential $_.Exception.Message)" $null
    # 后续检查都依赖配置，直接收尾退出
    $overall = 'FAIL'
    $exitCode = 2
    foreach ($c in $checks) {
        Write-Output ("{0} [{1}] {2}" -f $icon[$c.level], $c.id, $c.message)
    }
    if ($Json) {
        $report = [ordered]@{
            overall = $overall
            summary = [ordered]@{ pass = 0; warn = 0; fail = 1; total = $checks.Count }
            checks  = @($checks)
        }
        Write-Output ''
        Write-Output ($report | ConvertTo-Json -Depth 8)
    }
    Write-Output "健康检查：FAIL（配置不可解析，退出码 2）"
    exit $exitCode
}

# dataDir：配置优先，缺省退到配置文件所在目录
$dataDirRaw = [string](Get-Field $cfg 'dataDir' '')
$dataDir = if ($dataDirRaw) { $dataDirRaw } else { Split-Path -Parent $configPathResolved }
$dataDir = [IO.Path]::GetFullPath(($dataDir -replace '/', [IO.Path]::DirectorySeparatorChar))
$coveragePath = Join-Path $dataDir 'coverage.json'
$auditPath = Join-Path $dataDir 'auto-audit.jsonl'

# ---------- 2. 账本可解析 + corrupt 残留 ----------

$contentsCount = 0
try {
    if (-not (Test-Path -LiteralPath $coveragePath)) {
        Add-Check 'coverage' 'warn' "账本缺失：找不到 coverage.json（$dataDir）" $null
    } else {
        $coverage = Get-Content -Raw -LiteralPath $coveragePath | ConvertFrom-Json
        $contents = Get-Field $coverage 'contents' $null
        if ($null -eq $contents) {
            Add-Check 'coverage' 'fail' '账本解析成功但 contents 段为空' $null
        } else {
            $contentsCount = @($contents.PSObject.Properties).Count
            if ($contentsCount -le 0) {
                Add-Check 'coverage' 'warn' "账本可解析，但 contents 计数为 0" $contentsCount
            } else {
                $corruptFiles = @(Get-ChildItem -LiteralPath $dataDir -File -Filter '*.corrupt*' -ErrorAction SilentlyContinue)
                if ($corruptFiles.Count -gt 0) {
                    $parts = foreach ($f in $corruptFiles) {
                        "$($f.Name)（$($f.LastWriteTime.ToString('yyyy-MM-dd HH:mm'))）"
                    }
                    Add-Check 'coverage' 'warn' "账本可解析：contents $contentsCount 条；发现 $($corruptFiles.Count) 个损坏残留：$($parts -join '；')" `
                        ([ordered]@{ contents = $contentsCount; corrupt = @($corruptFiles | ForEach-Object { $_.Name }) })
                } else {
                    Add-Check 'coverage' 'ok' "账本可解析：contents $contentsCount 条，无 .corrupt 残留" $contentsCount
                }
            }
        }
    }
} catch {
    Add-Check 'coverage' 'fail' "账本不可解析：$(Remove-Credential $_.Exception.Message)" $null
}

# ---------- 3. qB 连通 ----------

$dl = Get-Field $cfg 'downloader' $null
$qbUrl = [string](Get-Field $dl 'url' '')
$qbSession = $null
$qbConnected = $false
$qbNote = ''

if (-not $qbUrl) {
    $qbNote = '配置里没有 downloader.url，跳过 qB 检查'
    Add-Check 'qb-connect' 'warn' $qbNote $null
} else {
    try {
        $base = $qbUrl.TrimEnd('/')
        $qbSession = New-Object Microsoft.PowerShell.Commands.WebRequestSession
        $loginBody = @{
            username = [string](Get-Field $dl 'username' '')
            password = [string](Get-Field $dl 'password' '')
        }
        $login = Invoke-RestMethod -Method Post -Uri "$base/api/v2/auth/login" `
            -Body $loginBody -WebSession $qbSession -TimeoutSec 8
        if ($null -ne $login -and [string]$login -match 'Fails') {
            throw '登录被拒绝（用户名/密码或本地 IP 白名单）'
        }
        $version = Invoke-RestMethod -Method Get -Uri "$base/api/v2/app/version" `
            -WebSession $qbSession -TimeoutSec 8
        $qbConnected = $true
        Add-Check 'qb-connect' 'ok' "qB 连通正常，版本 $([string]$version)" $([string]$version)
    } catch {
        $qbNote = "qB 连通失败，跳过后续下载器项：$(Remove-Credential $_.Exception.Message)"
        Add-Check 'qb-connect' 'warn' $qbNote $null
    }
}

# ---------- 4. 低于 100% 种子 ----------

if ($qbConnected) {
    try {
        $list = Invoke-RestMethod -Method Get -Uri "$base/api/v2/torrents/info?filter=incomplete" `
            -WebSession $qbSession -TimeoutSec 20
        $incomplete = @($list)
        # 双保险：接口口径偏差时按 progress<1 自行统计
        $progressIncomplete = @($incomplete | Where-Object { [double](Get-Field $_ 'progress' 1) -lt 1 })
        $n = $progressIncomplete.Count
        if ($n -gt 0) {
            Add-Check 'qb-incomplete' 'warn' "未完成 $n 条（progress<1）" $n
        } else {
            Add-Check 'qb-incomplete' 'ok' '未完成 0 条，全部达到 100%' 0
        }
    } catch {
        Add-Check 'qb-incomplete' 'warn' "未完成种子查询失败：$(Remove-Credential $_.Exception.Message)" $null
    }
} else {
    Add-Check 'qb-incomplete' 'warn' 'qB 未连通，跳过未完成种子检查' $null
}

# ---------- 5. 最近审计失败率 ----------

# 北京时间日界：now + 8h 后按天取整，再回推 (Days-1) 天
$utc8 = 8 * 3600
$todayStart = $nowUtc - (($nowUtc + $utc8) % 86400)
$cutoff = $todayStart - ($Days - 1) * 86400

$auditCounts = [ordered]@{ posted = 0; blocked = 0; failed = 0; duplicate = 0; other = 0 }
$auditTotal = 0
try {
    if (-not (Test-Path -LiteralPath $auditPath)) {
        Add-Check 'audit-rate' 'info' "无近期审计：找不到 auto-audit.jsonl" $null
    } else {
        foreach ($line in Get-Content -LiteralPath $auditPath -ErrorAction SilentlyContinue) {
            if (-not $line.Trim()) { continue }
            try { $rec = $line | ConvertFrom-Json -AsHashtable } catch { continue }
            $at = 0L
            if (-not [long]::TryParse([string](Get-Field $rec 'at' 0), [ref]$at)) { continue }
            if ($at -lt $cutoff) { continue }
            $auditTotal++
            $result = [string](Get-Field $rec 'result' '')
            switch ($result) {
                'posted'           { $auditCounts.posted++ }
                'blocked'          { $auditCounts.blocked++ }
                'failed'           { $auditCounts.failed++ }
                'review_failed'    { $auditCounts.failed++ }
                'skipped_duplicate' { $auditCounts.duplicate++ }
                default            { $auditCounts.other++ }
            }
        }

        if ($auditTotal -eq 0) {
            Add-Check 'audit-rate' 'info' "无近期审计：最近 $Days 天（北京时间日界）无记录" $auditCounts
        } else {
            $failRate = [math]::Round(100.0 * $auditCounts.failed / $auditTotal, 1)
            $msg = "最近 $Days 天审计 $auditTotal 条：posted $($auditCounts.posted) / 拦截 $($auditCounts.blocked) / 失败 $($auditCounts.failed) / 重复 $($auditCounts.duplicate) / 其它 $($auditCounts.other)，失败率 $failRate%"
            if ($failRate -gt 20) {
                Add-Check 'audit-rate' 'warn' "$msg（>20% 阈值）" ([ordered]@{ total = $auditTotal; failed = $auditCounts.failed; failRatePct = $failRate; counts = $auditCounts })
            } else {
                Add-Check 'audit-rate' 'ok' $msg ([ordered]@{ total = $auditTotal; failed = $auditCounts.failed; failRatePct = $failRate; counts = $auditCounts })
            }
        }
    }
} catch {
    Add-Check 'audit-rate' 'warn' "审计统计失败：$(Remove-Credential $_.Exception.Message)" $null
}

# ---------- 6. 退避到期数 ----------
# 口径对齐 autopost.rs::in_backoff：同 (key, site) 取最新一条 bad 记录；
# blocked 退避 24h（86400s），failed / review_failed 退避 6h（21600s）；
# now - at >= 窗口即视为已过退避期、可重试。

$backoffLatest = @{}   # "key|site" -> @{ at = long; blocked = bool }
try {
    if (Test-Path -LiteralPath $auditPath) {
        foreach ($line in Get-Content -LiteralPath $auditPath -ErrorAction SilentlyContinue) {
            if (-not $line.Trim()) { continue }
            try { $rec = $line | ConvertFrom-Json -AsHashtable } catch { continue }
            $result = [string](Get-Field $rec 'result' '')
            if ($result -notin @('blocked', 'failed', 'review_failed')) { continue }
            $k = [string](Get-Field $rec 'key' '')
            $s = [string](Get-Field $rec 'site' '')
            if (-not $k -or -not $s) { continue }
            $at = 0L
            if (-not [long]::TryParse([string](Get-Field $rec 'at' 0), [ref]$at)) { continue }
            $slotKey = "$k|$s"
            if (-not $backoffLatest.ContainsKey($slotKey) -or $at -gt $backoffLatest[$slotKey].at) {
                $backoffLatest[$slotKey] = @{ at = $at; blocked = ($result -eq 'blocked') }
            }
        }
    }

    $expired = 0
    $inBackoff = 0
    foreach ($slot in $backoffLatest.Values) {
        $window = if ($slot.blocked) { 86400 } else { 21600 }
        if ($nowUtc - $slot.at -ge $window) { $expired++ } else { $inBackoff++ }
    }

    if ($backoffLatest.Count -eq 0) {
        Add-Check 'backoff' 'info' '无拦截/失败记录，退避表为空' ([ordered]@{ expired = 0; inBackoff = 0 })
    } elseif ($expired -gt 0) {
        Add-Check 'backoff' 'info' "退避期内 $inBackoff 组；已过退避、可重试 $expired 组（草稿+站点，未与当前草稿清单交叉核对）" `
            ([ordered]@{ expired = $expired; inBackoff = $inBackoff; total = $backoffLatest.Count })
    } else {
        Add-Check 'backoff' 'ok' "退避期内 $inBackoff 组，暂无到期可重试" ([ordered]@{ expired = 0; inBackoff = $inBackoff })
    }
} catch {
    Add-Check 'backoff' 'warn' "退避统计失败：$(Remove-Credential $_.Exception.Message)" $null
}

# ---------- 汇总 ----------

$failCount = @($checks | Where-Object { $_.level -eq 'fail' }).Count
$warnCount = @($checks | Where-Object { $_.level -eq 'warn' }).Count
$okCount = @($checks | Where-Object { $_.level -eq 'ok' }).Count

if ($failCount -gt 0) {
    $overall = 'FAIL'; $exitCode = 2
} elseif ($warnCount -gt 0) {
    $overall = 'WARN'; $exitCode = 1
} else {
    $overall = 'OK'; $exitCode = 0
}

foreach ($c in $checks) {
    Write-Output ("{0} [{1}] {2}" -f $icon[$c.level], $c.id, $c.message)
}

if ($Json) {
    $report = [ordered]@{
        overall = $overall
        summary = [ordered]@{ pass = $okCount; warn = $warnCount; fail = $failCount; total = $checks.Count }
        checks  = @($checks)
    }
    Write-Output ''
    Write-Output ($report | ConvertTo-Json -Depth 8)
}

switch ($overall) {
    'FAIL' { Write-Output "健康检查：FAIL（$failCount 项失败，$warnCount 项提醒）" }
    'WARN' { Write-Output "健康检查：WARN（$okCount 项通过，$warnCount 项提醒）" }
    default { Write-Output "健康检查：OK（$okCount 项通过，$warnCount 项提醒）" }
}

exit $exitCode
