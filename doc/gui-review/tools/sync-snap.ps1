# 把当前工作树的 GUI 源码同步进 guisnap（HEAD 快照）并做两处快照专用补丁，然后由调用方 build
# 用途：主工作树的 crates/bsend 正被其他 Agent 重构（会短时编译不过），
#       用 HEAD 版引擎 + 当前 GUI 源码做视觉验证，主树恢复后再做一次正式构建。
param([switch]$Build)

$repo = "C:\Users\Schal\WorkBuddy\2026-11-17-20-52-34"
$snap = "C:\Users\Schal\WorkBuddy\guisnap"

robocopy "$repo\rust\crates\bsend-gui" "$snap\rust\crates\bsend-gui" /MIR /NFL /NDL /NJH /NJS | Out-Null
Copy-Item "$repo\rust\Cargo.toml" "$snap\rust\Cargo.toml" -Force

# 补丁 1：HEAD 的 AutopostOptions 没有 Default，去掉 ..Default::default()
$main = "$snap\rust\crates\bsend-gui\src\main.rs"
$t = (Get-Content -LiteralPath $main -Raw) -replace "`r`n", "`n"
$t = $t -replace "(?s)(sites: if self\.autopost_sites\.is_empty\(\) \{\n(?:.*\n)*?\s*\},\n)\s*\.\.Default::default\(\)\n", '$1'
Set-Content -LiteralPath $main -Value $t -Encoding utf8NoBOM

if ($Build) {
    Set-Location "$snap\rust"
    cargo build --profile gui-release -p bsend-gui 2>&1 | Select-String -Pattern "error|Finished" | Select-Object -Last 12
}
Write-Output "snapshot synced: $snap"
