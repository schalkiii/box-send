# 测试改进报告（2026-11-27）

本次走 `software-improvement` 的**测试分支**：只改测试和测试辅助代码，不改生产行为。
基线 `db9e16b`；改动后待提交。目标不是把覆盖率刷成好看数字，而是补上本轮审计里最容易产生假绿/假阳性的边界。

## 一、基线

- 改动后 `cargo test --workspace`：513 条全绿（核心库 454、coverage_e2e 33、其余 2/2/16/3/3）。
- `cargo fmt --check` 通过；`cargo clippy --workspace --all-targets -- -D warnings` 通过（只有 `proc-macro-error2` 的 future-incompat 提示，不是本项目代码）。
- 覆盖率工具：GNU 工具链缺 `profiler_builtins`，`cargo llvm-cov` 失败；MSVC + `rust-lld` 方案需要 `llvm-tools-preview`，下载组件长时间无响应，本轮停止，未产生行覆盖率数字。下面用“测试/函数比”和缺口清单量化，不伪造百分比。

## 二、逐模块缺口（量化）

| 模块 | 生产行数（近似） | 函数/测试 | 测试/函数比 | 主要缺口 |
|---|---:|---:|---:|---|
| `reseed.rs` | 11,857 | 345 / 168 | 0.49 | 标题/别名/判重分支多，上传回包与状态迁移覆盖不均；tracker 真值没有测试（生产也缺状态方法） |
| `sites/nexusphp.rs` | 3,091 | — / 36 | — | 名称判定的年份/分辨率冲突缺显式用例；审核页面变体未穷举 |
| `coverage.rs` | 2,164 | — / 26 | — | `collect_verifying` tracker 状态、并发超时、旧 miss 路径 |
| `bsend-cli/main.rs` | 2,380 | — / 16 | — | 命令参数组合；另有 `tests/cli_smoke.rs` 3 条，合计 19 |
| `bsend-gui` | 3,298 | — / 3 | — | 视图状态、按钮可用性、错误提示 |
| `qbittorrent.rs` | 1,627 | — / 22 | — | `trackers()` 只有 URL，没有 status；50,000 条上限只告警 |
| `autopost.rs` | 1,804 | — / 25 | — | 手动审计只测过 `posted`，未测重复/失败 |
| `config.rs` | 1,167 | — / 18 | — | 测试临时目录按进程号共用（本次修） |
| `gates.rs` | 962 | — / 20 | — | 真实站点组合仍靠 e2e |
| `report.rs` | 1,501 | — / 27 | — | 大索引、空索引、异常状态聚合 |

缺口优先级：`reseed` 的 tracker 终态、`coverage` 的收尾终态、`autopost` 的手动审计异常路径、`config` 的测试隔离、`nexusphp` 的判别词冲突。GUI 和 CLI 命令组合放到下一轮，本轮不硬凑。

## 三、测试计划（新增 6 条）

| 用例 | 测试目标 | 输入构造 | 预期检查点 |
|---|---|---|---|
| `temp_dirs_are_isolated_between_test_threads` | 配置测试不再共用临时目录 | 当前测试线程取一次目录，另起命名线程再取一次 | 两个路径不相等 |
| `all_site_title_badges_are_stripped_idempotently` | 16 种站点角标都能裁掉且幂等 | 固定核心标题 + 审计报告列出的 16 个角标逐个拼接 | 裁完等于核心；再裁一次不变 |
| `first_imdb_hit_ignores_anchor_without_title_attribute` | 没 `title` 的 details 锚点不能判重 | 只有链接文字的 IMDb 搜索页 | 返回 `None` |
| `manual_duplicate_audit_is_manual_but_not_counted_as_posted` | 手动重复命中写审计但不占额度 | 临时配置 + 写一条 `skipped_duplicate` | `manual=true`、`result=skipped_duplicate`、当日 posted 为 0 |
| `search_names_match_rejects_conflicting_discriminating_tokens` | 年份/分辨率冲突在宽容模式也不能判同一条 | 2023 vs 2024、1080p vs 720p、分隔符不同但词元一致 | 前两个 false；第三个 true |

## 四、实施与调试

### 4.1 测试临时目录隔离（`config.rs`）

原来 `write_temp` 用 `bsend-cfg-{pid}`，一个测试进程内所有测试共用一个目录；`cookie_header_matching` 写 `cookies.json` 会影响别的测试。现在改成 `test_dir()`：

- `thread_local!` 每个测试线程一个稳定目录；
- 目录名带进程号、测试线程名和原子序号，避免同名；
- 同一测试内多次 `write_temp` 仍共享目录，不破坏 `boxsend2.json` + `cookies.json` 的配套关系；
- `write_temp_isolated` 改为 `test_dir().join(tag)`。

验证：`cargo test -p bsend config::tests` 18 条全绿，连续跑无互相覆盖。

### 4.2 标题角标全量枚举（`reseed.rs`）

用固定核心标题逐一拼接 16 个角标，包括 `[2X免费]`、`[2x免費]`、`[50%]`、`剩余时间`。每个都断言裁完等于核心，并再裁一次验证幂等。这个用例等于对有限角标空间做穷举，不引入随机种子。

验证：单测通过；失败信息会打印具体角标。

### 4.3 IMDb 无 title 锚点（`reseed.rs`）

补了一个只有链接文字、没有 `title` 属性的 details 锚点，断言 `first_imdb_hit` 返回 `None`。这是 13city 判重误命中的同类边界：没有发布名就不能当重复。

验证：单测通过。

### 4.4 手动重复审计（`autopost.rs`）

补 `skipped_duplicate` 手动记录：断言 `manual=true`，且 `parse_audit` 不把它算进当日 `posted`。覆盖“手动补发不占额度”的真实分支，不再只测成功发布。

验证：单测通过。

### 4.5 名称判别词冲突（`sites/nexusphp.rs`）

补年份、分辨率、分隔符三组输入。前两组必须 false，第三组必须 true，避免“宽容模式把 2023/2024 或 1080p/720p 当同一条”。

验证：单测通过。

### 4.6 tracker 状态不能只看 progress（`qbittorrent.rs`）

补 `parse_tracker_entries` + `tracker_status_note` 单测：`status=2` 翻成“tracker 已回包（做种 3 / 下载 14）”，`status=1` 翻成“tracker 还没回包”，`status=4` 带报错原文。生产侧同时把 `trackers()` 拆出 `tracker_info()`，并在 reseed/coverage 的 100% 收尾里写 tracker 状态；`same_hash_on_target` 增加 `status=2` 条件，tracker 没回包时不再记 `seeding`。

验证：单测通过；全 workspace 513 条通过（`artifacts/workspace-test-20261127-final3.log`）；clippy 通过。真实站点侧 pthome 16:32 变成 `status=2`，city13 长时间 `status=1`、恢复 qB 诊断配置后变 `status=4` 且手动 announce TLS reset，见 `SITES.md` 2.10。

## 五、新增用例结果

| 用例 | 结果 |
|---|---|
| `config::tests::temp_dirs_are_isolated_between_test_threads` | 通过 |
| `reseed::tests::all_site_title_badges_are_stripped_idempotently` | 通过 |
| `reseed::tests::first_imdb_hit_ignores_anchor_without_title_attribute` | 通过 |
| `autopost::tests::manual_duplicate_audit_is_manual_but_not_counted_as_posted` | 通过 |
| `sites::nexusphp::tests::search_names_match_rejects_conflicting_discriminating_tokens` | 通过 |
| `qbittorrent::tests::tracker_info_keeps_status_and_turns_it_into_an_honest_note` | 通过 |

## 六、覆盖率提升与限制

- 测试总数 507 → 513，新增 6 条；`reseed` 166 → 168，`config` 17 → 18，`autopost` 24 → 25，`nexusphp` 35 → 36，`qbittorrent` 22 → 23。
- 新增覆盖：16 种角标的全量分支、IMDb 无 title 锚点、手动重复审计、名称判别词冲突、配置测试线程隔离、tracker status 1/2/4 的文案与同 hash 条件。
- 没有行覆盖率数字：GNU 工具链缺 profiler runtime，MSVC 组件下载无响应。下一轮按审计 S8 在 CI 里预装 `llvm-tools-preview`，再补真实百分比。
- 没有引入随机 fuzzing：本轮缺口都是有限枚举或固定输入，随机化不会增加判别力；标题角标已做 16 种穷举。后续若要 fuzzing，优先对 `strip_site_title_badges` 和 `search_names_match` 做约束输入生成，并固定可打印种子。

## 七、完成判据自查

- [x] 逐模块缺口分析有量化表（测试/函数比、已知缺口）。
- [x] 测试计划每条有目标、输入构造、检查点。
- [x] 关键模块补了边界/异常用例：`config`、`reseed`、`autopost`、`nexusphp`。
- [x] 复杂数值计算 golden：本项目本轮无复杂数值算法；用固定输入/期望表作为 golden，逐项断言。
- [x] 没有随机 fuzzing；有限空间已穷举，理由见第六节。
- [x] 用例可独立运行，无顺序依赖。
- [x] 新增用例全绿，并已独立调试收敛。
- [x] 报告已输出。

## 八、后续建议

1. ~~先做审计 S1（tracker 真值），再补 `qbittorrent` status=1/2/4 的 mock 测试~~（本轮已完成读取和单测；整状态机仍只做到“记录真值”，没有新增独立状态）。
2. 下一轮测试分支再补 CLI 有效配置路径、GUI 纯状态函数、`all_tracker_urls` 50,000 条上限。
3. 覆盖率接入 CI 后再更新本报告，不在本地反复尝试工具链。
