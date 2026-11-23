# bsend（Rust 引擎）

BoxSend 的核心程序：读 qBittorrent 里的内容，找出目标 PT 站的缺口，审阅、检查、上传，再把目标站的种子挂回 qB 做种。完整的产品说明、判断规则和排障看仓库根目录的文档，这里只留开发与命令入口。

## 构建

```bash
cd rust
cargo build --release                              # 命令行：target/release/bsend.exe
cargo build --profile gui-release -p bsend-gui     # 桌面界面：target/gui-release/bsend-gui.exe
```

GPUI 0.2.2 在 Windows 的标准 `--release` 下会调用 Windows SDK 的 `fxc.exe` 编译着色器；没装 SDK 时用 `gui-release`，或直接用 debug 版，功能相同。

## 常用命令

```bash
# 建/刷新 qB 内容索引，不发站点请求
bsend --config /path/to/boxsend.json scan

# 只看现状：各站已有多少、缺多少、待发、待找源
bsend status

# 覆盖率报表，可导出 JSON
bsend report --top 20
bsend report --json coverage-report.json

# 审阅一条待发，不发布
bsend reseed                 # 列出待发编号
bsend reseed --draft 1       # 审阅第 1 条
bsend reseed --draft 1 --yes # 真发

# 自动发种：试运行 / 真发 / 循环
bsend autopost --dry-run --limit 5
bsend autopost --sites sbpt --limit 1
bsend autopost --loop --interval-secs 1800

# 站点表单检查（只读，新站首次接入才跑）
bsend formcheck --sites <site>
bsend sites --md

# 审计与巡检
bsend audit --days 7
pwsh ../scripts/healthcheck.ps1 -Config "$env:LOCALAPPDATA\BoxSend\boxsend.json"
```

`--config` 默认读当前目录的 `boxsend.json`。`--dry-run` 会连站点审阅和检查，但不上传、不写覆盖记录。

## 目录结构

| 路径 | 职责 |
| --- | --- |
| `crates/bsend/` | 引擎：内容索引、覆盖记录、报表、发种主流程、qB 接口、站点适配、发布检查 |
| `crates/bsend-cli/` | `bsend.exe` 的命令行入口 |
| `crates/bsend-gui/` | GPUI 桌面界面，共用同一份配置和覆盖记录 |
| `crates/bsend/tests/` | 端到端测试 |

更细的模块说明在 `../doc/ARCHITECTURE.md` 第 11 节。

## 开发检查

```bash
cargo test -p bsend
cargo test -p bsend --lib reseed::tests::exist_marker
cargo check -p bsend -p bsend-cli
```

发布前至少跑一次完整 `cargo test -p bsend`。单测和端到端测试的数量会随功能变化，以命令输出为准。

## 不能违反的规则

- 判断目标站有没有以 qB 里的 tracker 为准，不再逐站搜索。
- 一个种子只挂一个站点；禁止 `addTrackers`，禁止合并多个站的 announce。
- 第一次向一个新站发种前，在该站配置 `overrides.announceURL` 填完整 tracker 地址（含 passkey）；否则程序无法构造上传种子，会直接中止。
- 上传种子写目标站专属 `info.source`，让新 info hash 和源种子不同。
- 发布后从目标站下载 `.torrent`，独立加回 qB。
- 只发影视、剧集、纪录片；教育、成人由标题关键词黑名单拦。
- 判重以 info hash 和分片为准，不靠标题猜。

## 相关文档

- `../README.md`：产品简介和快速开始
- `../doc/ARCHITECTURE.md`：判断规则、发种流程、辅种规则
- `../doc/OPERATIONS.md`：数据文件、命令、巡检、排障
- `../doc/SITES.md`：站点清单和白名单
- `../doc/SITE-DEBUG.md`：新站接入和上传失败排查
- `../doc/ROADMAP.md`：进度和未完成项
