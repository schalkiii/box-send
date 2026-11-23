# AGENTS.md — BoxSend（本仓库）

## 这是什么
PT 站自动发种/跨站辅种工具：读 qB 里的内容组和 tracker，找出目标站缺口 → 审阅 + 发布前检查 → 上传 → 从目标站下载种子挂回 qB 做种。Rust workspace 在 `rust/`，桌面界面用 GPUI；另有 Swift 原版（`Sources/`）与 Windows 打包脚本（`windows/`）。

## 常用命令
```powershell
Set-Location rust
cargo test -p bsend                                  # 单测 + e2e
cargo test -p bsend --lib reseed::tests::exist_marker # 跑单条用例
cargo build --release -p bsend-cli                   # 产物 rust/target/release/bsend.exe

# 试运行：拉上传页构造字段 + 跑发布前检查，不上传
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" --cookiecloud autopost --dry-run --sites sbpt --limit 5
# 真实发布（白名单 + 日限额 + 退避 + 发布前检查）
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" --cookiecloud autopost --sites luckpt --limit 2
```

## 运行数据
配置与数据目录：`C:\Users\Schal\AppData\Local\BoxSend`（`boxsend.json` 是真实配置；`coverage.json` 是覆盖记录；`auto-audit.jsonl` 是发种审计；`reseed-log.jsonl` 是转种动作日志）。字段格式与手改边界见 `doc/OPERATIONS.md`，改 `coverage.json` 前必读。

## 技术栈
Rust 2021；crate：`bsend`（引擎）、`bsend-cli`、`bsend-gui`。错误用 `anyhow`，HTTP 用 `reqwest`（默认跟随重定向），种子文件格式（Bencode）自实现于 `bencode.rs`。

## 目录与约定
- `rust/crates/bsend/src/`：`reseed.rs`（审阅/上传/挂回主流程，单文件 5k+ 行）、`autopost.rs`（自动发种编排）、`coverage.rs` / `report.rs`（覆盖记录与统计）、`matcher.rs`（判重）、`sites/nexusphp.rs`（站点适配）、`config.rs`、`gates.rs`。
- 代码注释用英文；日志与文档用中文。

## 不能违反的规则
- **判断目标站有没有，以 qB 为准（2026-11-22 起）**：qB 里有目标站 tracker 就视为站上已有，没有就视为大概率没有；不再逐站搜索。只有真要发某条之前，才对目标站再确认一次。详见 `doc/ARCHITECTURE.md`。
- **一个种子只挂一个站点**：禁止 `addTrackers`，禁止把多个站的 announce 合并。上传种子时写目标站专属 `info.source`，让新 info hash 和源种子不同；发布后从目标站下载 `.torrent`，作为独立种子挂回 qB。
- 发布不看画质：站上有同名其他版本照样发（`assist.allowOtherVersion`）；只有站点明确返回 `existed` 才不发。
- 第一次向 qB 里还没有该站种子的站点发种时，抄不到该站 passkey，必须在 `sourceSites[].overrides.announceURL` 填该站完整 tracker 地址；没填会直接中止，不会伪造无 passkey 的种子。
- 判重以 info hash 和分片（pieces）为准；只有名称相同且总大小相等时才进人工核对，总大小不同就是不同内容。
- `existed` 只认最终跳转的 `existed=1`，或错误框里的明确重复措辞；上传表单原样返回不算重复。
- 退避：同一内容 + 同一站点，发布检查拦截停 24 小时，失败停 6 小时，依据写在 `auto-audit.jsonl`；`--dry-run` 同样受限。
- 待发条数从 qB 的实时缺口生成：`autopost` 读下载器种子列表，`--dry-run` 读索引里最后一次扫描结果。旧 `reseed_draft` 记录只作过渡期兼容。加白名单不会凭空产生待发条目。
- 内容范围由 `assist.skipKeywords` 拦教育/成人，命中不占上传额度。
- 详细规则见 `doc/ARCHITECTURE.md`；运行数据与巡检见 `doc/OPERATIONS.md`；站点清单见 `doc/SITES.md`；未完成项见 `doc/ROADMAP.md`。历史审计与批次报告已并入这些文档，不再单独保留。
