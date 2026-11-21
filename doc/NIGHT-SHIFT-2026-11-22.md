# 夜间收尾与明日真发交接（2026-11-22 深夜）

本文档给明早起床后的「真发」用：先花 2 分钟过 §1，再按 §2 执行。
今晚**未真实发种**，只做了只读演练、代码修复与文档收尾。

## 1. 明早前置检查（2 分钟）

| 项 | 期望 | 怎么查 |
| --- | --- | --- |
| qBittorrent 在线 | `http://127.0.0.1:9091` 可登录 | 浏览器打开 WebUI |
| 发种总闸 | `assist.publishEnabled = true` | 已确认（当前就是 true） |
| 自动发种白名单 | `assist.autoReseedSites = ["luckpt","sbpt"]` | 已确认 |
| sbpt 日限额 | `assist.reseedRules.sbpt.dailyLimit = 20`（北京时间日界） | 已确认 |
| luckpt 日限额 | `20`（官方预审站） | 已确认 |
| 暂停站不含目标站 | `reseedPausedSites` 含 hdcity 等 36 站，**不含 sbpt/luckpt** | 已确认 |
| 无后台发种任务 | 无计划任务、无 bsend 进程 | 已确认（见 §6） |
| Cookie 新鲜 | 建议启动前拉一次 CookieCloud | 命令带 `--cookiecloud` 即可 |

## 2. 真发命令（建议先 10 条）

```powershell
cd C:\Users\Schal\WorkBuddy\2026-11-17-20-52-34\rust
cargo build --release -p bsend-cli

$bsend = ".\target\release\bsend.exe"
$cfg = "C:\Users\Schal\AppData\Local\BoxSend\boxsend.json"

# 2.1 先空跑复核（不连站点上传，只回头读）
& $bsend --config $cfg --cookiecloud autopost --dry-run --sites sbpt --limit 10

# 2.2 确认后真发 10 条（门禁 → 上传 → 发后自检 → 推种校验 → 审计）
& $bsend --config $cfg --cookiecloud autopost --sites sbpt --limit 10

# 2.3 看结果
Get-Content "C:\Users\Schal\AppData\Local\BoxSend\auto-audit.jsonl" -Tail 20
```

- 每发一条都会写 `auto-audit.jsonl`（`posted / skipped_duplicate / other_version /
  existed_pending / failed`）；审计写失败会**中止本轮**，防日限额失守。
- 加入 qB 的种子自动带 `Boxsend` 标签；已存在的种子走 `torrents/addTags` 追加。
- 稳定后可扩大到 `--limit 20`（= sbpt 日限额）；luckpt 同理把 `--sites` 换掉。
- 发完建议再跑一轮辅种：`& $bsend --config $cfg --cookiecloud run --sites sbpt`。

## 3. 昨晚 dry-run 结论（sbpt，limit 20）

- 通过门禁、可发：**20 条**。
- 拦截 5 项，原因均为**素材缺口**（非代码缺陷），需人工补：

| 条目 | 拦截原因 | 处理建议 |
| --- | --- | --- |
| 战斗民族养成记 | medium 未选 | 手工在表单选媒介后重跑 |
| 不白吃 / 宝宝巴士 | 缺条目链接（豆瓣/IMDb） | 补链接到源站后重跑 |
| 儿童画教材 | 分类/质量/MediaInfo 不符 | 人工确认分类与 MediaInfo |
| 夺宝奇兵 2 / 3 | MediaInfo 为空、本地文件打不开 | 检查本地文件完整性后重新生成 MediaInfo |

- 另有 1 条目标站上传页瞬断（白色巨塔，50x/10054）→ 直接重试即可；
  1 条因站点退避被跳过 → 等退避窗口过后重跑。

## 4. 安全闸一览（都已生效）

- 总闸 `publishEnabled`（缺省 false，当前显式 true）。
- 白名单 `autoReseedSites`（空 = 只演练）。
- 日限额按**北京时间**日界；审计 `auto-audit.jsonl` fail-closed。
- 跨进程锁 `bsend-state.lock`（GUI 与 CLI 不会同时发；崩溃残留 2 小时后可接管）。
- 回滚：把 `assist.publishEnabled` 改 false，或把站点加入 `reseedPausedSites`；
  GUI 配置页改完会热重载并备份 `.bak-gui`。

## 5. 今晚的提交（均已推送）

| 提交 | 内容 |
| --- | --- |
| `aa5a404` | 审计遗留收口（编排 e2e / GUI 拆分 / 成员名兜底 / release 流水线） |
| `7b0233e` | 审阅脱敏 + 结构化转种结果 + 预检/节流/终态修正 |
| `64dd5c4` | 跨进程运行锁 + 排除站点点名 + CookieCloud 脱敏 + 原子写统一 |
| `ffceca7` | GUI 配置写回抽出纯函数并补单测 |
| `9d1a581` | 夜间审计执行记录 + MIT LICENSE + CHANGELOG |

验证基线：`cargo fmt` 通过；`clippy --workspace --all-targets -D warnings` 零告警；
`cargo test --workspace` = **191 单测 + 19 e2e + 2 CLI + 2 GUI** 全绿。

## 6. 今晚「不真发」的保障

- 未执行任何真实 `autopost`（只跑 `--dry-run`）。
- 查过计划任务：无 `auto-sbpt.ps1` / `bsend` 注册项（`auto-sbpt.ps1` 只是遗留脚本，
  没有被调度）。
- 无 bsend 进程常驻；GUI 只在人工点「自动发种」时才发。

## 7. 遗留项（代码侧，影响不大）

- **BUG-M7**：审阅产物直传发布（复用审阅表单字段，避免门禁字段与提交字段二次漂移）。
  改动面较大，建议白天串行做。
- **S-16 余项**：站点写接口级的 posted/blocked mock 断言（编排层记账已由 TEST-L4 覆盖，
  真实上传路径留给明早真发时验证）。
- 已补完：BUG-L11（多候选择优）、TEST-L4（posted/blocked 注入式 e2e）、
  GAP-4（`--dry-run` 空白名单逐条演练）。

## 8. 等你确认的清理候选（未删除）

| 文件 | 大小 | 说明 |
| --- | --- | --- |
| `README_副本.md` | 49 KB | 旧 README 副本，已被 `README.md` 取代 |
| `%LOCALAPPDATA%\BoxSend\startup.log` | 184 MB | 旧 GUI 运行日志，可清 |
| `%LOCALAPPDATA%\BoxSend\coverage.json.bak-*`（19 份） | 350.5 MB | 旧备份，只保留最近 2 份即可 |

注：`1.webp` **不能删**——`scripts/make-app.sh` 用它生成 macOS 应用图标
（`1.webp → AppIcon.icns`）。此前审计报告的「待确认删除」结论已更正。

确认后我再删，并同步 `.gitignore` / 文档引用。
