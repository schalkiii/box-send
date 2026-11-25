# BoxSend 回归测试增补报告（2026-11-25）

执行分支：`software-improvement` 的**测试分支**（一次只走一个分支）  
代码范围：只改测试和测试辅助，不改生产行为  
基线提交：`6ce56e8`；本轮改动 4 个 Rust 文件，测试代码单独提交，报告随文档提交  
审计依据：`doc/AUDIT-REPORT-2026-11-25.md`

## 1. 本轮目标

上一轮刚修掉“发布成功但没做种被报成 `posted`”的问题，新增了 `SameHashSkip`、`PostedVerifying`、上传超时/空回包处理。测试分支的目标是：

1. 把刚修的链路用回归测试钉住，尤其是“校验没完不能算发布完成”和“同 hash 不能碰原种”。
2. 给 qB 客户端补 HTTP 契约测试，覆盖旧版 qB、参数回退、409、空回包、非 torrent 回包这些真实遇到过的兼容问题。
3. 给 CLI 补参数解析和格式化边界，先把“文档写 `--loop` 实际参数是 `--loop-mode`”这类问题变成可发现的测试。
4. 不为了迁就测试而改实现；发现真实 bug 只记录，不在测试分支静默修。

## 2. 增补前后量化

| 指标 | 增补前 | 增补后 | 变化 |
| --- | ---: | ---: | ---: |
| 源码单测（`bsend` lib） | 356 | 377 | +21 |
| `coverage_e2e` | 27 | 29 | +2 |
| `other_version_e2e` | 2 | 2 | 0 |
| `skip_keywords_e2e` | 2 | 2 | 0 |
| `bsend-cli` 单测 | 6 | 16 | +10 |
| `bsend-cli` 集成冒烟 | 0 | 3 | +3 |
| `bsend-gui` 单测 | 3 | 3 | 0 |
| **workspace 合计** | **396** | **432** | **+36** |

本机实际执行 `cargo test --workspace --exclude bsend-gui`：**429 个通过，0 失败**（不含 GUI 3 个）。  
`cargo test --workspace` 在本机不绿，原因是 GUI 链接工具链缺失，不是测试断言失败：GNU 缺 `-lktmw32`，MSVC 缺 `link.exe`。因此本报告的“429 通过”是核心库 + CLI + 集成测试的口径，不能冒充完整 workspace。

## 3. 新增用例：目标、输入、检查点

### 3.1 qB 客户端（9 个，`qbittorrent.rs`）

这些测试起一个本地 HTTP 假服务，记录每次请求的方法、路径、表单和请求体，再按预设返回。**不连真实 qB**。

| 用例 | 目标 | 输入构造 | 检查点 |
| --- | --- | --- | --- |
| `login_accepts_204_and_rejects_wrong_password` | 登录成功和密码错误要分开 | 第一次 `204 No Content`，第二次 `200 "Fails."` | 204 被当作登录成功，`test_connection` 返回版本串；错误密码让 `test_connection` 返回 `Err`，错误含“登录失败”和“HTTP 200”；两次都只发一次 `/api/v2/auth/login` |
| `version_accepts_json_and_rejects_empty_body` | 版本接口容忍正常 JSON，拒绝空回包 | 一次 `{"version":"v4.6.2"}`，一次空正文 | 正常 JSON 返回版本串；空正文报错；请求路径是 `/api/v2/app/version` |
| `list_torrents_skips_records_without_hash_and_keeps_rich_fields` | 清单缺 `hash` 的记录不能进结果，字段要保留 | 两条记录，一条缺 `hash`，一条有 `hash/name/size/progress/state/tracker` | 只返回 1 条；字段值逐个保留；请求含 `filter=all` |
| `files_falls_back_to_hash_and_remembers_the_working_param` | 旧版 qB 不认 `hashes` 时要回退 `hash`，并记住可用参数 | 第一次 `/api/v2/torrents/files` 对 `hashes` 返回 400，第二次对 `hash` 返回文件数组 | 第一次用 `hashes`，第二次用 `hash`；后续调用优先用 `hash`；文件路径/大小保留 |
| `trackers_falls_back_to_infohash_and_filters_non_http_entries` | trackers 参数回退，并过滤 DHT/udp 伪 tracker | 第一次 `hash` 返回 400，第二次 `infohash` 返回 `http`、`udp`、`** [DHT] **` 混合 | 回退到 `infohash`；只保留 `http` 项；udp/DHT 不出现 |
| `add_conflict_marks_already_without_touching_existing_torrent` | 409 表示“已存在”，不能删原种、不能重试写 | `add` 返回 409，预置一条旧种 | 返回 `Already`；没有调用 `delete`；只探测一次；原种仍在 |
| `add_zero_success_count_is_an_error_not_a_silent_accept` | qB 明确说 0 条成功时必须报错 | `{"success_count":0,"failure_count":1}` | 返回 `Err`，错误信息包含 `success_count`，不静默成功 |
| `export_rejects_non_bittorrent_body` | 导出接口返回 HTML/空内容时不能当种子 | `200` 正文 `<html>not a torrent</html>` | 返回 `Err`；不会把非 bencode 内容写进种子 |
| `stop_falls_back_to_legacy_pause_on_404` | 新版 `stop` 不存在时回退旧 `pause` | 第一次 `/stop` 404，第二次 `/pause` 200 | 返回 `Ok`；`pause` 收到相同 hash 表单；不重复调 `stop` |

### 3.2 CLI（9 个，`bsend-cli/src/main.rs`）

| 用例 | 目标 | 输入构造 | 检查点 |
| --- | --- | --- | --- |
| `status_defaults_to_boxsend_json_and_no_cookiecloud` | 默认配置路径和默认参数 | 只给 `status` | 配置默认指向 `%LOCALAPPDATA%\BoxSend\boxsend.json`；`cookiecloud` 为空 |
| `global_cookiecloud_is_accepted_after_subcommand` | 全局参数放在子命令后也要认 | `status --cookiecloud autopost` | 解析成功；`cookiecloud=Some("autopost")` |
| `report_parses_site_list_and_top` | `report` 的站点列表和 `--top` | `report --sites sbpt,luckpt --top 5` | 站点拆成 2 个；`top=5` |
| `audit_defaults_to_seven_days_and_text_output` | `audit` 默认 7 天、默认文本输出 | 只给 `audit` | `days=7`；`json=false` |
| `autopost_loop_mode_flag_is_parsed` | 实际参数名 `--loop-mode` 能被解析 | `autopost --loop-mode` | 解析成功；`loop_mode=true`；同时说明文档里的 `--loop` 不是当前参数名 |
| `commas_handles_zero_negatives_and_max` | 千分位格式的 0、负数、最大值 | `0`、`-1`、`1_000`、`-1_000`、`i64::MAX` | 输出 `0`、`-1`、`1,000`、`-1,000`、`9,223,372,036,854,775,807`；负数符号保留 |
| `size_text_switches_units_at_expected_boundaries` | 大小单位切换 | B/KB/MB/GB/TB 的边界值 | 单位在边界处切换，不出现 `1024.0 KB` 这类未进位结果 |
| `display_width_and_padding_handle_cjk_and_emoji` | 中文/emoji 的显示宽度 | 中文、英文、emoji、混合串 | 宽度按显示列计算；补齐/截断后列数符合预期 |
| `follow_up_item_strips_raw_reply_and_truncates` | 回访条目去掉原始回复并截断 | 带 `回复：...` 的原始文本、超长文本 | 不保留原始回复前缀；超长时按字符边界截断，不切坏中文 |

### 3.3 自动发种审计（1 个，`autopost.rs`）

| 用例 | 目标 | 输入构造 | 检查点 |
| --- | --- | --- | --- |
| `parse_audit_counts_posted_verifying_but_not_pending_review` | `posted_verifying` 是真实上传，要占日限额；`pending_review` 不占 | 同一天写一条 `posted_verifying`、一条 `pending_review` | `posted_today=1`；`pending_review` 不增加；北京时间日界仍按原有逻辑 |

同时扩展了 `duplicate_outcomes_map_to_distinct_audit_results` 的断言，确认 `PostUnresolved`、`PendingReview`、`ReviewRejected`、`GoneFromSite` 不会都映射成同一个审计词。

### 3.4 端到端（1 个，`coverage_e2e.rs`）

| 用例 | 目标 | 输入构造 | 检查点 |
| --- | --- | --- | --- |
| `e2e_unfinished_verification_is_posted_verifying` | 上传成功、种子已加 qB、校验没完成时，不能报 `posted`，要报 `posted_verifying` | 假站点接受上传并返回详情；假 qB 的 progress=0.42、state=`checkingUP`；校验等待设为 0 秒，轮询在未完成时结束 | `ReseedOutcome::PostedVerifying`；格子 `Verifying`；动作日志 `result=posted_verifying`、`cell_status=verifying`、`seeded=false`；保留非空 `verify_hash`；日志写明“校验还没跑完”；确实调用过一次 `torrents/add` |

为支持这条用例，`qb_server_for_reseed` 被拆成默认 100% 的包装器和可指定 `progress/state` 的 `qb_server_for_reseed_with_progress`。既有调用保持默认 100%，不改行为。

## 4. 验证命令与结果

### 4.1 已通过

```powershell
# 核心库 + CLI + 集成测试
cargo test --workspace --exclude bsend-gui
# 结果：429 passed; 0 failed

# 静态检查
cargo clippy --workspace --all-targets --exclude bsend-gui -- -D warnings
# 结果：通过

# 格式
cargo fmt --all -- --check
# 结果：通过
```

`cargo test` 分项：

| 测试目标 | 通过 |
| --- | ---: |
| `bsend` lib | 377 |
| `coverage_e2e` | 29 |
| `other_version_e2e` | 2 |
| `skip_keywords_e2e` | 2 |
| `bsend-cli` | 16 |
| `bsend-cli` 集成冒烟 | 3 |
| doc-tests | 0 |
| **合计（不含 GUI）** | **429** |

### 4.2 本机未通过 / 未测

| 项目 | 结果 | 原因 |
| --- | --- | --- |
| `cargo test --workspace` | 失败 | GUI 链接：GNU 工具链缺 `-lktmw32` |
| `cargo +stable-x86_64-pc-windows-msvc test -p bsend-gui` | 失败 | `linker link.exe not found`；本机没有 Visual Studio C++ 链接器 |
| `cargo llvm-cov`（GNU） | 失败 | `can't find crate for profiler_builtins` |
| `cargo llvm-cov`（MSVC） | 未得到结果 | 缺 `llvm-tools-preview`，安装长时间无进展，后台任务已停止；日志 `C:\Users\Schal\AppData\Local\Temp\bsend-cov-msvc.log` |

因此本报告没有行覆盖率百分比。下面的“覆盖缺口”用测试数、模块行数和具体未覆盖路径描述，不能把它当成行覆盖率。

### 4.3 远端 CI

2026-11-25 12:54 推送后，GitHub Actions 的 `rust` 工作流**通过**：

- Linux：`fmt`、`clippy`（核心库 + CLI）、`test`（核心库 + CLI）全部通过。
- Windows：`clippy`（全部 crate，含 GPUI GUI）通过，`test`（全部 crate）通过；这补上了本机缺 MSVC 链接器、跑不了 GUI 3 个测试的缺口。

旧的 `打包` 工作流也在同一提交上触发并处于运行中；这是审计问题 E3 描述的遗留 Swift/.NET 流水线，和 Rust 测试结果无关。

## 5. 覆盖缺口前后对比

| 模块 | 行数 | 之前测试 | 之后测试 | 仍缺什么 |
| --- | ---: | ---: | ---: | --- |
| `qbittorrent.rs` | 1,421 | 6 | 21 | `torrent_files_many` 的部分失败已覆盖；`all_tracker_urls` 的 50000 条上限告警仍只靠日志目测 |
| `bsend-cli/src/main.rs` | 2,277 | 6 | 16 | `cmd_status`、`cmd_report`、`cmd_audit`、`cmd_autopost`、`cmd_pull`、`cmd_followup` 的命令处理本身仍缺；集成冒烟只覆盖版本号、缺配置、坏配置三条退出路径 |
| `bsend-cli` 集成 | — | 0 | 3 | 成功路径（空索引/空审计）还没有稳定配置夹具 |
| `autopost.rs` | 1,549 | 19 | 20 | `posted_unresolved` 的日限额/审计周报桶已补上；多站点并发限额 |
| `coverage_e2e.rs` | — | 27 | 29 | `review_rejected` 已补；`pending_review` / `gone_from_site` / `pending_offers` 的完整链路仍缺 |
| `bsend-gui/src/` | 3,282 | 3 | 3 | 12 个文件仍 0 测试；`jobs.rs`、`views/*`、`widgets.rs`、`theme.rs`、`types.rs` 基本没有行为断言 |
| `kimi.rs` | 203 | 1 | 6 | 桥接错误、非 JSON、返回类型、HTTP 500、人机验证文案、标签复用已覆盖；`prepare_search_page` 的 40 轮等待和后台标签分支仍缺 |
| `scripts/` | 13 个脚本 | 0 | 0 | 参数解析、坏输入、原子写、dry-run |

## 6. 测试严谨性说明

- 新增 qB 测试全部通过本地假 HTTP 服务驱动，检查了请求方法、路径、查询参数和表单字段；不是只断言“返回 Ok”。
- 409、0 成功、空正文、非 torrent 回包这些用例都检查了“没有副作用”：没有删除原种、没有把非种子内容写盘、没有重复请求。
- `posted_verifying` e2e 检查了四个独立输出：返回枚举、覆盖格状态、动作日志字段、`seeded` 布尔值。这样即使其中一个输出被改错，测试也会失败。
- `parse_audit_counts_posted_verifying_but_not_pending_review` 检查的是日限额计数，不是只检查解析函数不 panic。
- 已知弱项：`outcome_seeded` 一度与“已挂回 qB”这句文案耦合；代码质量分支已改成 `DuplicateOutcome.mounted` 显式标记，测试按标记断言，不再跟文案走。

## 7. 后续修复（代码质量分支）

1. **`outcome_seeded` 依赖文案**：已改成 `DuplicateOutcome.mounted` 显式标记；`PendingReview` / `ReviewRejected` 在“已挂回 qB”时为 true，测试按标记断言。
2. **`run_reseed` 的 `Ok(other) => Posted`**：已拆成显式 `Seeding` 分支；其他 `Ok` 状态落 `posted_unresolved`，不再静默当发布成功。
3. **审计映射兜底**：`audit_result` 已穷举 `CoverageStatus`，新增状态会编译报错；`report::audit_bucket` 已单列 `posted_unresolved`。
4. **`bsend audit` 看不到待补挂**：日/总计都加了 `posted_unresolved`，`status` 同时认 `posted_unresolved` 和 `existed_unresolved` 前缀。
5. **`commas(i64::MIN)`**：改用 `unsigned_abs()`，并补 `i64::MIN` 断言。
6. **qB add 缺 `success_count`**：现在要求 200 回包是 `Ok.` 或带 `success_count` 的 JSON；空正文/HTML/缺字段都报错，并补了两个契约用例。
7. **`autopost --loop`**：文档已统一成 `--loop-mode`；是否再加 `--loop` 别名仍是产品决定，没有擅自改 CLI。

## 8. 第二批回归（14 个，2026-11-25 后续）

| 模块 | 用例 | 检查点 |
| --- | --- | --- |
| qB | `all_tracker_urls_collects_non_empty_trackers` | 过滤空 tracker，路径带 `limit=50000` |
| qB | `torrent_names_handles_empty_bad_json_and_missing_size` | 空输入不发请求；坏 JSON 返回空；缺 size 记 0 |
| qB | `torrent_content_path_filters_empty_values` | 正常路径返回；空串返回 None |
| qB | `set_download_limit_and_remove_torrent_use_expected_endpoints` | 两个写接口路径正确，不碰 add |
| qB | `torrent_files_many_skips_failures_and_reports_progress` | 失败 hash 不进结果；每条都回调；进度收尾 |
| Kimi | `command_rejects_bridge_error_and_non_json` | `ok:false` 和坏 JSON 都报明确错误 |
| Kimi | `evaluate_string_rejects_non_string_value` | 返回类型不是字符串时报错 |
| Kimi | `fetch_search_html_rejects_non_200_and_captcha_text` | HTTP 500 和人机验证未通过都报错 |
| Kimi | `fetch_search_html_returns_body_on_success` | 200 时原样返回正文 |
| Kimi | `active_tab_prefers_same_site_active_tab` | 只复用同站且 active 的标签页 |
| e2e | `e2e_review_rejected_keeps_mounted_and_records_review` | 审核不通过仍 `seeded=true`，格子 `review_rejected` |
| CLI 集成 | `version_prints_and_exits_zero` | 版本号输出、退出码 0 |
| CLI 集成 | `status_with_missing_config_exits_nonzero` | 缺配置非零退出，错误可读 |
| CLI 集成 | `status_with_bad_config_json_exits_nonzero` | 坏配置非零退出，错误可读 |

## 9. 结论

本轮把“上传成功但校验没完”“同 hash 不碰原种”“旧版 qB 参数回退”“409/0 成功/空回包/非 torrent 回包”这些真实踩过的坑补成了回归测试；陆续补到核心库 + CLI + 集成测试 429 个全绿，静态检查和格式检查通过。GUI 3 个测试和完整 workspace 仍依赖远端 Windows MSVC CI；行覆盖率仍没有数字，下一轮应先解决覆盖率工具链，再谈覆盖率门禁。
</content>
