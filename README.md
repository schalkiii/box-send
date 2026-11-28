# BoxSend

PT 站自动发种和跨站辅种工具，适合有盒子或长期做种的用户。现在的开发主线是 Rust 版：

- `bsend`：核心引擎；
- `bsend-cli`：命令行 `bsend.exe`；
- `bsend-gui`：桌面界面。

根目录的 `Sources/`、`Tests/` 是早期 Swift 版存档，`windows/` 是打包脚本，都不再作为开发主线。

## 它能做什么

- 从 qBittorrent 里已有内容找出目标站的缺口，自动审阅、检查、上传、挂回做种。
- 判断目标站有没有，以 qB 里的 tracker 为准，不再逐个站点搜索。
- 一个种子只挂一个站点，禁止把多个站的 tracker 合并；上传时写入目标站专属来源标记，让新种子有独立的 hash。
- 发布后从目标站下载种子，作为独立种子加回 qB，不和源种子混在一起。
- 按白名单、每日限额、失败冷却（默认关闭）运行，所有发布动作写审计日志。
- 支持 CookieCloud、PT-depiler Gist 同步 Cookie，多主题桌面界面；上传限速默认跟随 qB 全局（`defaultUpLimit=0`，不单独限速）。
- 只发影视、剧集、纪录片；教育、成人等类型由标题关键词黑名单拦掉。

**当前基线（2026-11-28）**：配置 147 个站，自动发种白名单 46 个；发种统计 44 个站、157 条；Rust 全量测试 565 条。精确的运行分布以 `bsend status` 和 `bsend stats` 为准。

## 快速开始

```powershell
Set-Location rust
cargo test -p bsend
cargo build --release -p bsend-cli

# 试运行：会审阅和检查，但不上传
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" autopost --dry-run --limit 5

# 自动循环：每 30 分钟一轮
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" autopost --loop-mode --interval-secs 1800
```

`cargo test -p bsend` 只测核心库；要跑全部 Rust 测试用 `cargo test --workspace`。Windows 上需要 Visual Studio 的 C++ 链接器；没有时用 `cargo test --workspace --exclude bsend-gui`，GUI 那部分由远端 Windows CI 覆盖。

第一次使用按这个顺序：

1. 配置 qBittorrent 地址和账号；
2. 同步要发种站点的 Cookie；
3. 如果 qB 里还没有某个站的种子，第一次向该站发种前，在该站配置的 `overrides.announceURL` 填上该站完整 tracker 地址（含个人 passkey）；
4. 配好自动发种白名单和每日限额；
5. 先跑 `status` 和 `report`，确认各站已有多少、缺多少；
6. 再用 `autopost --dry-run --limit 2` 看会做什么；
7. 最后小批量真发，确认站上详情页和 qB 做种都正常。

## 文档

| 文档 | 讲什么 |
| --- | --- |
| `AGENTS.md` | 开发命令、目录约定、不能违反的规则 |
| `doc/ARCHITECTURE.md` | 为什么以 qB 为准、一条种子怎么发、辅种怎么判 |
| `doc/OPERATIONS.md` | 数据文件、命令、巡检、排障顺序 |
| `doc/TESTING.md` | 测试分层、跑法、假服务端和新用例放哪里；当前测试数看这里 |
| `doc/SITES.md` | 147 个站的清单、46 站自动发种白名单、卡住的站 |
| `doc/SITE-DEBUG.md` | 新站怎么接、上传被拒怎么查、单站结论 |
| `doc/SITE-REVIEW-2026-11-28.md` | 最近一次站点消息、审核拒绝和收件箱结论 |
| `doc/REVIEW-RECURRENCE-2026-11-28.md` | 审核问题复盘：哪些已进代码门禁、哪些还没收 |
| `doc/ROADMAP.md` | 已经做完什么、还在做什么、有什么风险 |
| `doc/NEAT-FREAK-2026-11-28.md` | 本次知识收尾、事实矩阵和待确认清场清单 |
| `doc/AUDIT-REPORT-2026-11-27-r3.md` | 2026-11-27 审计基线；历史证据，不是当前待办 |
| `doc/TEST-REPORT-2026-11-27.md` | 2026-11-27 测试基线；当前测试数见 `doc/TESTING.md` |
| `CHANGELOG.md` | 用户能感觉到的变化 |

按日期命名的审计报告、测试报告和洁癖报告保留作证据，结论以最新一份为准；日常维护只改上面的常驻文档和最新一份收尾报告，不再新开零散手册。

## 反馈

QQ 群：951690884（加群密码：鱼佬）。
