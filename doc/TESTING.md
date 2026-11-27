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

# 全部 crate（含 GPUI GUI）；2026-11-27 实测 GNU 工具链可直接跑，不需要 MSVC 链接器
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

# 静态检查与格式（GUI 也在内）
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## 2.1 故障修复后的回放

改完代码或脚本，先用同一草稿跑一次不提交的回放，再谈上线：

```powershell
python scripts\replay-reseed.py --draft 31 --expect '注入本机视频截图 4 张' --expect '13→4' --reject '13→1'
python scripts\replay-reseed.py --draft 85 --expect '注入本机视频截图 3 张' --expect '11→4' --reject '11→1'
python scripts\replay-reseed.py --draft 56 --expect 'source_sel[4] = 10' --expect 'processing_sel[4] = 11' --expect 'team_sel[4] = 5'
```

回放脚本实际调用 `bsend reseed --draft <N>`，不传 `--yes`，不会提交。`--expect` 必须命中真实输出里的字段、图片张数或标题；`--reject` 用来挡住已经修掉的旧现象（例如“11→1”“source_sel[4] = 0”）。

已发种子只能在紧急纠错时人工介入；改动仍先用 `scripts/edit-seed.py` 演练，确认后再 `--apply`，提交后重新抓编辑页复核。修复后按需要补单测或把现场加入回放用例，下一轮不能只靠人工记忆。

## 3. 现有假服务端

- `coverage_e2e.rs`：假站点提供搜索页、详情页、上传页、`takeupload.php`、下载页；假 qB 提供登录、种子列表、文件清单、tracker、导出和添加。需要指定校验进度时用 `qb_server_for_reseed_with_progress`，默认包装器保持 100%。
- `qbittorrent.rs` 的 `mock_qb`：只处理单条 HTTP 连接，记录请求方法、路径和表单；用于登录、版本、清单、文件参数回退、trackers 回退、409、空回包、导出、启停、tracker 汇总、批量文件清单。
- `kimi.rs` 的 `mock_bridge`：本地假 WebBridge，按请求体返回 `ok/data` 或坏 JSON；用于桥接错误、返回类型、HTTP 500、人机验证文案和标签复用。
- `bsend-cli/tests/cli_smoke.rs`：直接启动 `bsend` 二进制，检查版本号、缺配置和坏配置的退出码/错误提示。
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

**2026-11-27 当前基线**：`cargo test --workspace` 共 513 条全绿（核心库 454、`coverage_e2e` 33、其余 2/2/16/3/3）；`cargo fmt --check` 和 `cargo clippy --workspace --all-targets -- -D warnings` 都通过。GUI 不再需要排除，直接跑 workspace 测试。

## 6. 当前缺口

- `qbittorrent.rs`：`all_tracker_urls` 的 50000 条上限告警只靠日志目测；tracker `status` 已能读，但扫描 4 万条种子的批量路径还没逐条核对 status。
- `bsend-cli`：`cmd_status`、`cmd_report`、`cmd_audit`、`cmd_autopost`、`cmd_pull`、`cmd_followup` 的命令处理本身仍缺；集成冒烟只覆盖版本号、缺配置、坏配置三条退出路径，加 `cli_smoke.rs` 的 3 条一共 19 条。
- `bsend-gui`：`jobs.rs`、`views/*`、`widgets.rs`、`theme.rs`、`types.rs` 基本没有行为断言。
- `kimi.rs`：`prepare_search_page` 的 40 轮等待、后台标签 `visibilityState=hidden` 分支仍缺。
- `coverage_e2e.rs`：审核状态链路已覆盖上传被拦截、候选入口、回访的“未审 / 审核不通过 / 站上已找不到”，以及审核通过后下载站点种子并挂回 qB；仍缺的是其他业务分支，不是审核状态本身。
- `scripts/`：参数解析、坏输入、原子写、dry-run；现在只有 `scripts.yml` 的语法门禁。

详细缺口和新增用例记录见 `TEST-REPORT-2026-11-27.md`；旧 `TEST-REPORT-2026-11-25.md` 只作历史基线，结论以新报告为准。审计结论见 `AUDIT-REPORT-2026-11-27.md`。
