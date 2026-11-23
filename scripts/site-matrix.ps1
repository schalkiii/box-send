<#
.SYNOPSIS
  只读生成站点适配矩阵：读 `bsend sites --json`，汇总启用/暂停/实测/分片/cookie 现状。

.DESCRIPTION
  只运行 `bsend sites --json`（该命令只读配置与 cookie 元数据，不发站点请求），
  不改配置、不登录站点、不上传。输出 Markdown 站点矩阵，供逐站适配与排障。
  输出不含 cookie/passkey/密码。

.PARAMETER Binary
  bsend.exe 路径。缺省优先 release，其次 debug。

.PARAMETER ConfigPath
  boxsend.json 路径。缺省 %LOCALAPPDATA%\BoxSend\boxsend.json，其次仓库根 boxsend.json。

.PARAMETER OutFile
  输出 Markdown 路径。缺省打印到 stdout（不再默认覆盖 doc/ 下的文件）。

.PARAMETER Print
  同时把 Markdown 打到 stdout（默认写文件后只打印一行摘要）。

.EXAMPLE
  pwsh scripts/site-matrix.ps1
  pwsh scripts/site-matrix.ps1 -ConfigPath C:\path\boxsend.json -OutFile matrix.md
#>
[CmdletBinding()]
param(
    [string]$Binary,
    [string]$ConfigPath,
    [string]$OutFile,
    [switch]$Print
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Remove-Credential([string]$Text) {
    if (-not $Text) { return '' }
    $t = $Text -replace '(?i)(passkey|torrent_pass|authkey|token|cookie|password)=([^&\s]+)', '$1=***'
    $t = $t -replace '(?i)(passkey|torrent_pass|authkey|token)/([A-Za-z0-9]+)', '$1/***'
    return $t
}

function YesNo([object]$Value) {
    if ([bool]$Value) { return '是' }
    return '—'
}

function Escape-Cell([string]$Text) {
    if (-not $Text) { return '' }
    return ($Text -replace '\|', '\|' -replace "`r?`n", ' ')
}

function Shorten([string]$Text, [int]$Max = 220) {
    if (-not $Text) { return '' }
    # 错误里偶尔夹整页 HTML：先去标签、压空白，再截断，保证矩阵可读
    $t = $Text -replace '<[^>]{1,160}>', ' '
    $t = ($t -replace '\s+', ' ').Trim()
    if ($t.Length -gt $Max) { $t = $t.Substring(0, $Max) + '…' }
    return $t
}

function Prop([object]$Obj, [string]$Name, $Default = $null) {
    if ($null -eq $Obj) { return $Default }
    $p = $Obj.PSObject.Properties[$Name]
    if ($null -eq $p) { return $Default }
    return $p.Value
}

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Binary) {
    $release = Join-Path $repoRoot 'rust\target\release\bsend.exe'
    $debug = Join-Path $repoRoot 'rust\target\debug\bsend.exe'
    if (Test-Path -LiteralPath $release) { $Binary = $release }
    elseif (Test-Path -LiteralPath $debug) { $Binary = $debug }
    else { throw '找不到 bsend.exe：先在 rust 目录 cargo build --release，或用 -Binary 指定' }
} elseif (-not (Test-Path -LiteralPath $Binary)) {
    throw "找不到 bsend.exe：$Binary"
}

if (-not $ConfigPath) {
    $local = Join-Path $env:LOCALAPPDATA 'BoxSend\boxsend.json'
    $inRepo = Join-Path $repoRoot 'boxsend.json'
    if (Test-Path -LiteralPath $local) { $ConfigPath = $local }
    elseif (Test-Path -LiteralPath $inRepo) { $ConfigPath = $inRepo }
    else { throw '找不到 boxsend.json：用 -ConfigPath 指定（脚本不会回退到空配置）' }
} elseif (-not (Test-Path -LiteralPath $ConfigPath)) {
    throw "找不到配置：$ConfigPath"
}

$toStdout = -not $OutFile

$argv = @('--config', (Resolve-Path -LiteralPath $ConfigPath).Path, 'sites', '--json')
$raw = (& $Binary @argv 2>&1 | Out-String)
if ($LASTEXITCODE -ne 0) {
    throw "bsend sites 失败（exit=$LASTEXITCODE）：$((Remove-Credential $raw).Trim())"
}
try {
    $rows = @($raw | ConvertFrom-Json)
} catch {
    throw "bsend sites 输出不是 JSON：$((Remove-Credential $raw).Trim().Substring(0, [Math]::Min(300, $raw.Trim().Length)))"
}
if ($rows.Count -eq 0) { throw 'bsend sites 返回空列表' }

# formcheck.json：最近一次「拉上传表单但不提交」的字段核对结论（每站取最新一条）
$cfg = Get-Content -Raw -LiteralPath $ConfigPath | ConvertFrom-Json
$dataDirRaw = [string](Prop $cfg 'dataDir' '')
$dataDir = if ($dataDirRaw) { $dataDirRaw } else { Split-Path -Parent (Resolve-Path -LiteralPath $ConfigPath).Path }
$dataDir = [IO.Path]::GetFullPath(($dataDir -replace '/', [IO.Path]::DirectorySeparatorChar))
$formPath = Join-Path $dataDir 'formcheck.json'
$fcMap = @{}
$fcCounts = [ordered]@{ ok = 0; gap = 0; fail = 0; untested = 0 }
$formNote = ''
if (Test-Path -LiteralPath $formPath) {
    try {
        $fcRoot = Get-Content -Raw -LiteralPath $formPath | ConvertFrom-Json
        foreach ($res in @(Prop $fcRoot 'results' @())) {
            $site = [string](Prop $res 'site' '')
            if (-not $site) { continue }
            $at = [long](Prop $res 'at' 0)
            if (-not $fcMap.ContainsKey($site) -or $at -gt [long](Prop $fcMap[$site] 'at' 0)) {
                $fcMap[$site] = $res
            }
        }
    } catch {
        $formNote = "formcheck.json 读取失败：$((Remove-Credential $_.Exception.Message).Trim())"
    }
} else {
    $formNote = "没有 $formPath（先跑一次 `bsend formcheck`）"
}
foreach ($r in $rows) {
    $fc = $fcMap[$r.site]
    $verdict = if ($fc) { [string](Prop $fc 'verdict' '') } else { '' }
    switch ($verdict) {
        'ok' { $fcCounts.ok++ }
        'gap' { $fcCounts.gap++ }
        'fail' { $fcCounts.fail++ }
        default { $fcCounts.untested++ }
    }
}

$drillOk = @($rows | Where-Object { $_.drill -eq 'ok' }).Count
$drillFail = @($rows | Where-Object { $_.drill -eq 'fail' }).Count
$drillUntested = $rows.Count - $drillOk - $drillFail
$enabled = @($rows | Where-Object { $_.enabled }).Count
$paused = @($rows | Where-Object { $_.paused }).Count
$reseedPaused = @($rows | Where-Object { $_.reseedPaused }).Count
$ready = @($rows | Where-Object { $_.enabled -and -not $_.paused -and -not $_.reseedPaused -and $_.drill -eq 'ok' }).Count
$piecesOk = @($rows | Where-Object { $_.pieces -eq 'ok' }).Count

$generated = [DateTimeOffset]::Now.ToString('yyyy-MM-dd HH:mm:ss zzz')
$stage = Resolve-Path -LiteralPath $ConfigPath
$sb = [System.Text.StringBuilder]::new()
[void]$sb.AppendLine('# 站点适配矩阵')
[void]$sb.AppendLine()
[void]$sb.AppendLine('> 只读生成：`scripts/site-matrix.ps1` → `bsend sites --json`。不改配置、不发站点请求；重新生成本文件即可刷新。')
[void]$sb.AppendLine()
[void]$sb.AppendLine("- 生成时间：$generated")
[void]$sb.AppendLine("- 配置：``$($stage.Path)``")
[void]$sb.AppendLine("- 站点数：**$($rows.Count)**｜启用 **$enabled**｜全停 **$paused**｜停转种 **$reseedPaused**")
[void]$sb.AppendLine("- 实测：ok **$drillOk**｜fail **$drillFail**｜未测 **$drillUntested**｜pieces ok **$piecesOk**")
[void]$sb.AppendLine("- formcheck 最新结论：ok **$($fcCounts.ok)**｜gap **$($fcCounts.gap)**｜fail **$($fcCounts.fail)**｜未测 **$($fcCounts.untested)**")
if ($formNote) { [void]$sb.AppendLine("- 表单核对备注：$formNote") }
[void]$sb.AppendLine("- 可发布候选（启用+未全停+未停转种+实测 ok）：**$ready**")
[void]$sb.AppendLine()
[void]$sb.AppendLine('## 1. 全量矩阵')
[void]$sb.AppendLine()
[void]$sb.AppendLine('| 站点 | 名称 | 域名 | 框架 | 启用 | 全停 | 停转种 | 实测 | formcheck | pieces | cookie |')
[void]$sb.AppendLine('| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |')
foreach ($r in ($rows | Sort-Object site)) {
    $fc = $fcMap[$r.site]
    $fcText = if ($fc) {
        $v = [string](Prop $fc 'verdict' '?')
        $fieldsN = [int](Prop $fc 'fields' 0)
        $selectsN = [int](Prop $fc 'selects' 0)
        $coreN = @(Prop $fc 'core' @()).Count
        if ($fieldsN -eq 0 -and $selectsN -eq 0) {
            $v
        } else {
            "{0}·{1}f/{2}s{3}" -f $v, $fieldsN, $selectsN, $(if ($coreN -gt 0) { "·硬缺$coreN" } else { '' })
        }
    } else { '未测' }
    [void]$sb.AppendLine(("| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} | {9} | {10} |" -f `
                (Escape-Cell $r.site), (Escape-Cell $r.name), (Escape-Cell $r.host), (Escape-Cell $r.framework),
                (YesNo $r.enabled), (YesNo $r.paused), (YesNo $r.reseedPaused), [string]$r.drill,
                (Escape-Cell $fcText), [string]$r.pieces, [string]$r.cookies))
}
[void]$sb.AppendLine()
[void]$sb.AppendLine('## 2. 实测失败原因归组')
[void]$sb.AppendLine()
$fails = @($rows | Where-Object { $_.drill -eq 'fail' -and $_.drillReason })
if ($fails.Count -eq 0) {
    [void]$sb.AppendLine('无。')
} else {
    $groups = $fails | ForEach-Object {
        $reason = Remove-Credential ([string]$_.drillReason)
        $reason = $reason -replace '(?i)https?://[^\s）]+', '[url]'
        $reason = $reason.Trim()
        [pscustomobject]@{ Reason = $reason; Site = [string]$_.site }
    } | Group-Object Reason | Sort-Object Count -Descending
    [void]$sb.AppendLine('| 次数 | 站点 | 原因（URL 已折叠） |')
    [void]$sb.AppendLine('| --- | --- | --- |')
    foreach ($g in $groups) {
        $sites = ($g.Group | ForEach-Object { $_.Site }) -join ', '
        [void]$sb.AppendLine("| $($g.Count) | $(Escape-Cell $sites) | $(Escape-Cell (Shorten $g.Name)) |")
    }
}
[void]$sb.AppendLine()
[void]$sb.AppendLine('## 3. formcheck 字段缺口')
[void]$sb.AppendLine()
$fcIssues = [System.Collections.Generic.List[object]]::new()
foreach ($r in $rows) {
    $fc = $fcMap[$r.site]
    if (-not $fc) { continue }
    $verdict = [string](Prop $fc 'verdict' '')
    $core = @(Prop $fc 'core' @())
    $soft = @(Prop $fc 'soft' @())
    $errorText = Remove-Credential ([string](Prop $fc 'error' ''))
    if ($verdict -in @('gap', 'fail') -or $core.Count -gt 0 -or $errorText) {
        $fieldsN = [int](Prop $fc 'fields' 0)
        $selectsN = [int](Prop $fc 'selects' 0)
        $sizeText = if ($fieldsN -eq 0 -and $selectsN -eq 0) { '—' } else { "${fieldsN}f/${selectsN}s" }
        $fcIssues.Add([pscustomobject]@{
                Site    = [string]$r.site
                Name    = [string]$r.name
                Verdict = $verdict
                Size    = $sizeText
                Core    = ($core -join '；')
                Soft    = ($soft -join '；')
                Error   = $errorText
            })
    }
}
if ($fcIssues.Count -eq 0) {
    [void]$sb.AppendLine('无。')
} else {
    [void]$sb.AppendLine('| 站点 | 名称 | 结论 | 字段/下拉 | 硬缺口 core | 软提示 soft | 错误 |')
    [void]$sb.AppendLine('| --- | --- | --- | --- | --- | --- | --- |')
    foreach ($x in ($fcIssues | Sort-Object Site)) {
        $line = "| {0} | {1} | {2} | {3} | {4} | {5} | {6} |" -f `
            (Escape-Cell $x.Site), (Escape-Cell $x.Name), [string]$x.Verdict,
            $x.Size, (Escape-Cell (Shorten $x.Core 160)),
            (Escape-Cell (Shorten $x.Soft 160)), (Escape-Cell (Shorten $x.Error 160))
        [void]$sb.AppendLine($line)
    }
}
[void]$sb.AppendLine()
[void]$sb.AppendLine('## 4. 未测站点（下一步补实测）')
[void]$sb.AppendLine()
$untested = @($rows | Where-Object { $_.drill -ne 'ok' -and $_.drill -ne 'fail' } | Sort-Object site)
if ($untested.Count -eq 0) {
    [void]$sb.AppendLine('无。')
} else {
    [void]$sb.AppendLine('| 站点 | 名称 | 启用 | 停转种 | pieces | cookie |')
    [void]$sb.AppendLine('| --- | --- | --- | --- | --- | --- |')
    foreach ($r in $untested) {
        [void]$sb.AppendLine(("| {0} | {1} | {2} | {3} | {4} | {5} |" -f `
                    (Escape-Cell $r.site), (Escape-Cell $r.name), (YesNo $r.enabled),
                    (YesNo $r.reseedPaused), [string]$r.pieces, [string]$r.cookies))
    }
}
[void]$sb.AppendLine()
[void]$sb.AppendLine('## 5. 列口径')
[void]$sb.AppendLine()
[void]$sb.AppendLine('- **启用**：参与辅种/转种（`enabled`）。')
[void]$sb.AppendLine('- **全停**：`pausedSites`，整站不参与。')
[void]$sb.AppendLine('- **停转种**：`reseedPausedSites`，辅种仍做、只停转种/发布。')
[void]$sb.AppendLine('- **实测**：最近一次 `bsend sites` 记录的上传页演练结论（`ok`/`fail`/未测）；只拉 `upload.php` 核对字段，绝不 POST。')
[void]$sb.AppendLine('- **formcheck**：`formcheck.json` 每站最新一条的字段核对结论与规模：`结论·字段数f/下拉数s·硬缺N`。')
[void]$sb.AppendLine('- **pieces**：站点种子分片格式兼容性（`ok` / `unsupported` / 未测）。')
[void]$sb.AppendLine('- **cookie**：`cookies.json` 中该 host 的 cookie 条数（只显示数量，不显示值）。')
$text = $sb.ToString()

if ($OutFile) {
    $parent = Split-Path -Parent $OutFile
    if ($parent -and -not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    Set-Content -LiteralPath $OutFile -Value $text -Encoding utf8
}
if ($Print -or $toStdout) { Write-Output $text }
$target = if ($OutFile) { $OutFile } else { 'stdout' }
Write-Output ("站点矩阵已生成：{0} 个站点（启用 {1}，实测 ok {2}，fail {3}，未测 {4}）→ {5}" -f `
        $rows.Count, $enabled, $drillOk, $drillFail, $drillUntested, $target)
