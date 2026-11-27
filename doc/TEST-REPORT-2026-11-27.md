# 测试改进报告（2026-11-27）

本次走 `software-improvement` 的**测试分支**：先只改测试和测试辅助代码，不改生产行为。
基线 `db9e16b`；6 条测试随 `7046f4d`、`a81b568` 提交并推送。之后在同一分支上补了 S1 的 tracker 终态（生产行为改动）和 S7 的 10 条 GUI 纯状态测试，再把 CLI 参数定义拆到 `cli.rs` 并新增 4 条解析测试，全量基线更新为 **527 条**。目标不是把覆盖率刷成好看数字，而是补上本轮审计里最容易产生假绿/假阳性的边界。

## 一、基线

- 改动后 `cargo test --workspace`：**535 条全绿**（核心库 457、coverage_e2e 35、其余 2/2/23/3/13）。
- `cargo fmt --check` 通过；`cargo clippy --workspace --all-targets -- -D warnings` 通过（只有 `proc-macro-error2` 的 future-incompat 提示，不是本项目代码）。
- GitHub Actions `61cf4d2` run `45979346634` success（Linux + Windows 全量测试、fmt、clippy）；本地同一提交 527 条全绿。
- 覆盖率：已接入 CI（`coverage` job 用 Ubuntu + `cargo-llvm-cov` 生成 `lcov.info` artifact），并用 `--fail-under-lines 77` 设了 77% 的硬门槛；当前行覆盖率 16,842/21,087 = **79.87%**。本地 Windows GNU 工具链缺 profiler runtime，仍不能跑 `cargo llvm-cov`，本地用“测试/函数比”和缺口清单量化。
- 最新重构后复跑：核心库 457、CLI 23 + 冒烟 3、coverage_e2e 35、GUI 13，全量 **535 条全绿**；`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings` 通过。MSRV 1.88 的 `cargo check` 在本机 GNU 工具链通过。
- 推送后远端 CI：提交 `025b712`，run [45993230747](https://github.com/schalkiii/box-send/actions/runs/45993230747) 的 `MSRV 1.88`、覆盖率、Ubuntu、Windows 四个 job 全部 success；覆盖率 `lcov.info` 以 artifact `coverage-lcov` 保存。

## 二、逐模块缺口（量化）

| 模块 | 生产行数（近似） | 函数/测试 | 测试/函数比 | 主要缺口 |
|---|---:|---:|---:|---|
| `reseed.rs` | 3,585 | 主流程编排 | — | 表单/预审/上传/发后写回已拆到子模块；剩下的标题/简介解析和主流程仍靠站点真实回包验证 |
| `reseed/form.rs` | 2,485 | — / 子模块测试 | — | 真实站点字段组合靠 gatecheck / e2e |
| `reseed/tests.rs` | 3,433 | — / 168 | — | 原 reseed 单测整体迁出，测试本身未减 |
| `sites/nexusphp.rs` | 2,941 | — / 36 | — | 名称判定的年份/分辨率冲突缺显式用例；审核页面变体未穷举 |
| `coverage.rs` | 2,106 | — / 26 | — | `collect_verifying` 已接 tracker 真值；并发超时、旧 miss 路径仍缺 |
| `bsend-cli/commands.rs` | 1,998 | — / 8 | — | 命令处理已从 `main.rs` 迁出；真实配置加载和网络命令仍靠手工/e2e |
| `bsend-cli/main.rs` | 129 | — / 0 | — | 只剩入口、qB 连接、适配器构建 |
| `bsend-gui` | 3,298 | — / 13 | — | 纯状态文案/配色/路径已覆盖；按钮可用性、真实窗口仍靠手工 |
| `qbittorrent.rs` | 1,652 | — / 23 | — | `tracker_info()` 已能返回 status；50,000 条上限只告警，队列拥塞仍是运维问题 |
| `autopost.rs` | 1,776 | — / 25 | — | 手动审计已补 duplicate/blocked；长期 run id 未定 |
| `config.rs` | 1,133 | — / 18 | — | 测试临时目录按测试名隔离（本次修） |
| `gates.rs` | 905 | — / 20 | — | 真实站点组合仍靠 e2e |
| `report.rs` | 1,425 | — / 27 | — | 大索引、空索引、异常状态聚合 |

缺口优先级：`reseed` 的 tracker 终态已落共享判定（复用 `Verifying` 继续复核），轮询窗口仍只有一轮；`coverage` 的收尾已接同一判定；`autopost` 的手动审计异常路径、`config` 的测试隔离、`nexusphp` 的判别词冲突已补；GUI 纯状态已补 10 条，CLI 命令组合和真实窗口放到下一轮。

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

验证：单测通过；当期 workspace 513 条通过（`artifacts/workspace-test-20261127-final3.log`）；补 GUI 测试后 523 条（`final5.log`）；clippy 通过。真实站点侧 pthome 16:32 变成 `status=2`；city13 长时间 `status=1`、恢复 qB 诊断配置后变 `status=4`（stream truncated），但 17:50 站点页显示自己 1 个做种者、上传 70.08 MB、45.33 KB/s、连接 30:24，人工按站点页证据改成 `reseeded`，见 `SITES.md` 2.10。

### 4.7 tracker 终态不再只看 progress（S1，生产行为）

`tracker_confirms_seeding()` 成为 reseed 和 coverage 共用的唯一判定：`progress >= 0.999` 且至少一条 tracker `status=2` 才写 `Reseeded/Assisted`；`status=1/4` 或读不到时写 `Verifying`，note 带“已发布并挂回 100%，但 tracker 还没确认，下一轮复核”。这比新增独立状态改动小，且复用现有收尾循环：下一轮 `collect_verifying` 会再问 tracker，直到 `status=2` 或用户手工处理。

验证：`cargo test --workspace` 533 条全绿（当时基线；后续新增 2 条后为 535，见 4.9）；`coverage_e2e` 34 条；CLI 23 + 冒烟 3；`clippy` 通过（`artifacts/clippy-20261127-final17.log`）。

### 4.8 GUI 纯状态测试（S7）

`bsend-gui` 补 10 条不依赖 GPUI 窗口的测试：全部 17 个 `CoverageStatus` 的文案唯一性、审核三态配色、状态计数、`short_path` 只留末两段、配置路径优先级。GUI 测试从 3 条增到 13 条，达到审计 S7 的“至少 13 个测试”门槛。

验证：`cargo test -p bsend-gui` 13 条全绿（`artifacts/gui-test-20261127-final2.log`）；Windows CI 的 `cargo test --workspace` 会跑。

### 4.9 新发种收尾语义（S9，生产行为）

`collect_verifying` 原先对“校验 100% 且 tracker 已回包”的格子一律写 `Assisted`，导致本站新发种被误计成“已辅种”。现在按格子备注区分来源：带“本站新发种”的写 `Reseeded`，只有“补挂种子”才写 `Assisted`；`push_target_torrent` 在 tracker 未确认时按 `expectedHash` 是否为空写对应前缀，下一轮仍能分辨。hdvideo [79168](https://hdvideo.top/details.php?id=79168) 的旧格已按此语义人工纠正为 `reseeded`。

验证：单测 `verifying_origin_is_new_post_only_for_new_post_notes`、e2e `e2e_verifying_collect_reseeded_for_new_post` 通过；`cargo test --workspace` **535 条全绿**（`artifacts/workspace-full-test-20261127-final18.log`，核心 457、coverage_e2e 35、其余 2/2/23/3/13）；fmt/clippy 通过。

## 五、新增用例结果

| 用例 | 结果 |
|---|---|
| `config::tests::temp_dirs_are_isolated_between_test_threads` | 通过 |
| `reseed::tests::all_site_title_badges_are_stripped_idempotently` | 通过 |
| `reseed::tests::first_imdb_hit_ignores_anchor_without_title_attribute` | 通过 |
| `autopost::tests::manual_duplicate_audit_is_manual_but_not_counted_as_posted` | 通过 |
| `sites::nexusphp::tests::search_names_match_rejects_conflicting_discriminating_tokens` | 通过 |
| `qbittorrent::tests::tracker_info_keeps_status_and_turns_it_into_an_honest_note` | 通过 |
| `bsend-gui::tests::status_chip_labels_*`（8 组状态文案/配色） | 通过 |
| `bsend-gui::tests::status_chip_labels_are_unique`、`status_chip_keeps_review_colors_distinct` | 通过 |
| `bsend-gui::tests::short_path_keeps_only_the_last_two_components` | 通过 |
| `coverage::tests::verifying_origin_is_new_post_only_for_new_post_notes` | 通过 |
| `coverage_e2e::e2e_verifying_collect_reseeded_for_new_post` | 通过 |

## 六、覆盖率提升与限制

- 测试总数 507 → 523，新增 16 条：5 条测试分支 + tracker 真值 1 条 + GUI 纯状态 10 条。后续拆分/修复再增：NexusPHP 拆分不增删用例，verifyHash 回填 2 条、CLI 空路径 3 条、新发种收尾语义 2 条，当前 **535 条**。
- 新增覆盖：16 种角标的全量分支、IMDb 无 title 锚点、手动重复审计、名称判别词冲突、配置测试线程隔离、tracker status 1/2/4 的文案与同 hash 条件、tracker 终态进 `Verifying`、GUI 全状态文案和配置路径、新发种/补挂的来源区分。
- 行覆盖率已由 CI 产出：16,842/21,087 = **79.87%**，`--fail-under-lines 77` 作为硬门槛；本地 Windows GNU 仍缺 profiler runtime，不重复尝试。
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

1. ~~先做审计 S1（tracker 真值），再补 `qbittorrent` status=1/2/4 的 mock 测试~~（本轮已完成：共享 `tracker_confirms_seeding`，reseed/coverage 100% 后 tracker 未回包落 `Verifying`；status 1/2/4 断言已进 `qbittorrent` 单测）。
2. 下一轮测试分支补 CLI 有效配置路径、`reseed.rs` 的真实轮询窗口（2-3 次 announce）、`all_tracker_urls` 50,000 条上限。
3. 覆盖率接入 CI 后再更新本报告，不在本地反复尝试工具链。

