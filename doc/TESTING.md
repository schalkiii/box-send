# 测试地图

这份文档回答三个问题：测试分几层、每条命令跑什么、新增用例往哪里放。架构和发种流程看 `ARCHITECTURE.md`，运行数据和排障看 `OPERATIONS.md`。

## 1. 三层测试

| 层级 | 位置 | 跑的什么 | 是否需要网络 |
| --- | --- | --- | --- |
| 单元测试 | 各模块源码里的 `#[cfg(test)] mod tests` | 纯函数、解析、判重、状态映射、格式化 | 不需要 |
| 本地假服务端到端 | `rust/crates/bsend/tests/coverage_e2e.rs`、`other_version_e2e.rs`、`skip_keywords_e2e.rs` | 起本地假站点和假 qB，走完审阅、上传、下载、挂回、写覆盖记录 | 不需要外网 |
| 真实站点实测 | 人工执行 `bsend` 命令 | 登录、上传、审核、挂回的真实链路 | 需要站点账号和网络 |

真实站点实测不放进 `cargo test`：它会改站点状态、消耗上传额度，不能当成可重复的自动测试。

## 2. 常用命令

```powershell
Set-Location rust

# 核心库 + CLI + 集成测试；本机没有 MSVC 链接器时用这一条
cargo test --workspace --exclude bsend-gui

# 全部 crate（含 GPUI GUI）；Windows 需要 Visual Studio C++ 链接器
cargo test --workspace

# 只跑核心库
cargo test -p bsend

# 只跑某个模块
cargo test -p bsend --lib qbittorrent::tests
cargo test -p bsend --lib reseed::tests

# 只跑某一条用例
cargo test -p bsend --lib reseed::tests::exist_marker

# 端到端
cargo test -p bsend --test coverage_e2e
cargo test -p bsend --test other_version_e2e
cargo test -p bsend --test skip_keywords_e2e

# 静态检查与格式
cargo clippy --workspace --all-targets --exclude bsend-gui -- -D warnings
cargo fmt --all -- --check
```

## 3. 现有假服务端

- `coverage_e2e.rs`：假站点提供搜索页、详情页、上传页、`takeupload.php`、下载页；假 qB 提供登录、种子列表、文件清单、tracker、导出和添加。需要指定校验进度时用 `qb_server_for_reseed_with_progress`，默认包装器保持 100%。
- `qbittorrent.rs` 的 `mock_qb`：只处理单条 HTTP 连接，记录请求方法、路径和表单；用于登录、版本、清单、文件参数回退、trackers 回退、409、空回包、导出和启停。
- `other_version_e2e.rs` / `skip_keywords_e2e.rs`：分别验证“同名其他版本照发”和“内容类型黑名单在站点请求前拦掉”。

假服务只监听 `127.0.0.1`，测试结束由进程退出回收；不需要外部端口或账号。

## 4. 新增用例怎么写

每条用例先写清三件事：

1. **目标**：要保护哪个行为，失败时用户会看到什么错误；
2. **输入构造**：索引、配置、假站点回包、qB 进度/状态；
3. **检查点**：返回值、覆盖格状态、`reseed-log.jsonl` 字段、请求次数和请求参数。

写法约定：

- 用例之间不能有顺序依赖；临时目录带测试名，不共用固定路径。
- 需要 qB 进度时用 `qb_server_for_reseed_with_progress`，不要改默认 100% 的包装器。
- 测试发现生产代码 bug 时先报告，不为迁就用例改实现。
- 新增状态或审计词时，先补状态映射的穷举测试，再补端到端。

## 5. 覆盖率的现状

目前没有可用的行覆盖率数字：

- GNU 工具链的 `cargo llvm-cov` 报 `can't find crate for profiler_builtins`；
- MSVC 工具链缺 `llvm-tools-preview` 和 `link.exe`，装不上就跑不了。

现在用“测试数 / 模块行数”和“具体未覆盖路径”衡量缺口，不能把它当成行覆盖率。修好工具链后再把 `cargo llvm-cov --workspace --exclude bsend-gui --summary-only` 接进 CI，并设置不低于当前基线的门槛。

## 6. 当前缺口

- `qbittorrent.rs`：`all_tracker_urls`、`torrent_names`、`torrent_content_path`、`set_download_limit`、`remove_torrent`、`torrent_files_many` 的并发/部分失败。
- `bsend-cli`：`cmd_status`、`cmd_report`、`cmd_audit`、`cmd_autopost`、`cmd_pull`、`cmd_followup` 的命令处理本身；空索引、坏配置、错误退出码。
- `bsend-gui`：`jobs.rs`、`views/*`、`widgets.rs`、`theme.rs`、`types.rs` 基本没有行为断言。
- `kimi.rs`：桥接协议错误、超时、后台标签页 `visibilityState=hidden`。
- `coverage_e2e.rs`：`pending_review`、`review_rejected`、`gone_from_site`、`pending_offers` 的完整链路。
- `scripts/`：参数解析、坏输入、原子写、dry-run。

详细缺口和新增用例记录见 `TEST-REPORT-2026-11-25.md`，审计结论见 `AUDIT-REPORT-2026-11-25.md`。
</content>
