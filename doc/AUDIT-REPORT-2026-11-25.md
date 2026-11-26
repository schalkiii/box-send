# BoxSend 项目审计报告（2026-11-25）

审计对象：`C:\Users\Schal\WorkBuddy\2026-11-17-20-52-34`  
审计基线：`master` 分支，提交 `6ce56e8`，审计开始时工作树干净  
审计范围：`rust/` 三个 crate、`doc/` 五份文档、根目录说明、CI 与维护脚本  
审计纪律：本报告只记录发现和建议；审计阶段没有修改任何代码。后续测试分支的改动另见 `doc/TEST-REPORT-2026-11-25.md`。

## 0. 先统一几个词

- **`info hash`**：整个 `info` 字典的 SHA1 指纹。改标题、改 `info.source` 都会让它变，但文件内容和分片没有变。
- **pieces**：种子文件里每个分片的 SHA1 串。**pieces 指纹**是把这整串再算一次 SHA1，用来证明分片布局一致。
- **qB**：qBittorrent 下载器。
- **e2e**：从入口跑到出口的端到端测试，这里指 `rust/crates/bsend/tests/coverage_e2e.rs` 里起假站点、假 qB 的测试。
- **MSRV**：项目声明支持的最低 Rust 版本，本仓库是 `1.82`。
- **P1 / P2 / P3**：优先级。P1 是先修，P2 是排期修，P3 是顺手清理。

## 0.1 处理状态（2026-11-25 收尾）

47 条按 `software-improvement` 的分支纪律分批处理：文档、代码质量、工程规范的一部分、测试的一部分。最新测试和提交见 `doc/TEST-REPORT-2026-11-25.md`。

| 状态 | 编号 | 说明 |
| --- | --- | --- |
| 已解决（26 条） | D1–D10、G1–G3、C1–C6、T1、R1、R4、E3–E6 | 文档口径、`mounted` 显式标记、审计桶穷举、qB 剩余契约测试、CI 重复打包/本地检查/checkout 版本 |
| 部分完成（5 条） | A4、T2、T4、T5、E8 | A4 只把 `seeded` 改成显式 `mounted`，状态/标签单一真源未做；T2 只补了 CLI 入口冒烟；T4 补了桥接错误，等待/后台标签分支未测；T5 只补了 `review_rejected` e2e；E8 只有 shell/Python 语法门禁 |
| 未完成（16 条） | A1、A2、A3、A5、A6、T3、T6、T7、T8、T9、R2、R3、R5、E1、E2、E7 | 大文件拆分、GUI 可测化、`ReviewPending` 分状态、覆盖率门禁、MSRV 作业、依赖审计、脚本行为测试等，需要单独排期 |


## 1. 现状描述

### 1.1 规模

| 项目 | 数量 |
| --- | --- |
| Rust crate | 3 个：`bsend`（核心库）、`bsend-cli`、`bsend-gui` |
| Rust 源文件 | 40 个（`bsend` 25、`bsend-cli` 1、`bsend-gui` 14） |
| Rust 源码总行数 | 33,215 行（含源码文件里的测试） |
| 测试函数 | 416 个（其中本机实际跑 413 个，GUI 3 个未在本机编译） |
| 文档 | `ARCHITECTURE` 318 行、`OPERATIONS` 323 行、`SITES` 433 行、`SITE-DEBUG` 308 行、`ROADMAP` 186 行 |
| 站点配置 | 147 个站点；自动发种白名单 40 个（2026-11-25 实测 `boxsend.json`） |

### 1.2 结构

- `bsend` 是核心：索引、判重、源种定位、站点适配、发种、挂回、审计都在这里。
- `bsend-cli` 是主要人工入口；`bsend-gui` 是 GPUI 桌面界面。
- `sites/nexusphp.rs` 承担 NexusPHP 站点的搜索、详情页、表单和上传；`reseed.rs` 是发种主流程。
- 运行数据独立于仓库：`%LOCALAPPDATA%\BoxSend` 下的 `boxsend.json`、`coverage.json`、`auto-audit.jsonl`、`reseed-log.jsonl`。

### 1.3 质量水平

核心链路测试比较扎实：`reseed.rs` 有 124 个单测，`coverage_e2e.rs` 有 28 个端到端用例，`bencode`、`matcher`、`gates`、`report` 都有针对性用例。CI 已在 Linux/Windows 上跑 `fmt`、`clippy -D warnings`、测试，2026-11-25 最新远端 rust 与打包任务均为 success。审计后测试分支把 `coverage_e2e` 补到 29 个，全 workspace 到 432 个。

主要短板是**结构头和测试厚度不匹配**：`reseed.rs` 9,262 行，`bsend-cli/src/main.rs` 2,277 行，`bsend-gui` 3,282 行；而 CLI 审计时只有 15 个测试、GUI 只有 3 个测试，qB 客户端虽然有 15 个测试，但仍有多个公开方法没有契约测试；测试分支后 CLI 单测 16 个 + 集成冒烟 3 个，qB 21 个。文档整体详细，但有几处关键口径已经落后于代码。

## 2. 七个维度结论

### 2.1 文档一致性

结论：**有 10 处需要改，其中 2 处会直接误导操作**。最严重的是文档里的 `autopost --loop` 实际不被 CLI 接受。

| 编号 | 位置 | 问题 | 严重度 | 影响 |
| --- | --- | --- | --- | --- |
| D1 | `doc/ARCHITECTURE.md:23` | 还写“发布前再确认目前还没有完全接到新流程里”，但 `ROADMAP.md` 4.1 已标完成，代码也会在发种前做目标站确认 | 高 | 读者会以为核心流程缺一块，或反过来手工重复确认 |
| D2 | `README.md:18` | 写“上传限速默认 10 MB/s”，代码 `defaultUpLimit` 默认是 `0`（不单独限速，跟随 qB 全局），实际配置也是 0 | 中 | 用户以为有 10 MB/s 保护，可能把上行打满 |
| D3 | `README.md:52` | 写“42 站白名单”，实际是 40 站；`doc/SITES.md:15` 和 `OPERATIONS.md:243` 都写 40 | 低 | 新人按错数字核对配置 |
| D4 | `doc/ARCHITECTURE.md:43-56` | 状态表缺 `rejected`、`miss`、`pending_review`、`review_rejected`、`gone_from_site`；`review_pending` 的解释写成“站上看起来已有但 qB 没有 tracker”，实际是“站内命中了但清单/hash 对不上，等人核对” | 中 | 运维查状态时对不上号，容易把“等审核”和“待人工核对”混为一谈 |
| D5 | `doc/OPERATIONS.md:85` | 写“两类待跟进：pending_offers 和 posted_unresolved”，实际 `bsend status` 列四类：`pending_offers`、`pending_review`、`review_rejected`、`gone_from_site`，再加 `posted_unresolved` 共五类 | 中 | 审核类待跟进会被漏看 |
| D6 | `doc/OPERATIONS.md:221` | 写 `bsend pull`“只核对 hash/pieces”，实际现在只要求 pieces 和文件清单一致，站点改写 hash/标题时允许 hash 不同 | 中 | 操作员可能把正常的站点改写 hash 当成失败，拒绝挂回 |
| D7 | `doc/ARCHITECTURE.md:256`、`doc/ROADMAP.md:101`、`doc/OPERATIONS.md:247` | 三处日限额口径过期：`ARCHITECTURE` 还说“重复额度问题还要继续修”，`ROADMAP` 说只算 `Posted`，`OPERATIONS` 说“重复结果是否占额度还在修”；实际现在 `posted` 和 `posted_verifying` 都算一次上传 | 中 | 对日限额的预期错误，可能多发或少发 |
| D8 | `doc/OPERATIONS.md:132` | 写 `seeded=false` 等于“站上可能已发布、但本机没挂上种子”；`posted_verifying` 时种子其实已加进 qB，只是校验没完，也会 `seeded=false` | 低 | 会把“校验中”误判成“没挂上”，重复挂种 |
| D9 | `doc/OPERATIONS.md:116` | 审计结果词表没列 `posted_unresolved`、`pending_review`、`review_rejected`；这些词现在会写进 `auto-audit.jsonl` | 低 | 看审计日志时不知道这些词是什么意思 |
| D10 | `README.md:32`、`doc/ARCHITECTURE.md:260`、`doc/OPERATIONS.md:203`、`rust/README.md:36` | 文档写 `bsend autopost --loop`，但 CLI 实际参数名是 `--loop-mode`（`main.rs` 的字段 `loop_mode` 自动生成 kebab-case）。用文档命令会直接报未知参数 | **高** | 主要的长期运行命令照文档跑不起来 |

补充：`doc/SITES.md:156` 的“限额只算真正上传成功的条目”本身不算错，但没有说明 `posted_verifying` 也算上传成功，容易和 D7 一起误读；建议在改 D7 时一并补一句。

### 2.2 文档缺漏

结论：**架构文档的模块地图不完整，测试策略没有单独文档**；不是缺使用说明，而是缺“接手的人怎么定位代码、怎么补测试”。

| 编号 | 位置 | 问题 | 严重度 | 影响 |
| --- | --- | --- | --- | --- |
| G1 | `doc/ARCHITECTURE.md:285-297` | “代码在哪里”只列了 8 个模块，缺 `review.rs`、`kimi.rs`、`layout.rs`、`manifest.rs`、`torrent_rewrite.rs`、`throttle.rs`、`lock.rs`、`index.rs`、`models.rs`、`piecesapi.rs`、`cookiecloud.rs`、`resolver.rs`、`mediainfo.rs` | 中 | 新维护者无法从文档建立完整模块图，排障时容易漏掉相关文件 |
| G2 | `doc/` 根目录 | 没有测试策略/测试地图文档；现有测试信息散落在 `ROADMAP`、`ARCHITECTURE` 和代码注释里 | 中 | 补测试靠口口相传，不知道哪些路径已有 e2e、哪些只有单测 |
| G3 | `README.md`、`rust/README.md` | 没有写本地跑全量测试需要 MSVC 工具链或 `--exclude bsend-gui`；本机默认 GNU 工具链会在 GUI 链接阶段失败 | 中 | 新人照 README 跑 `cargo test --workspace` 会得到与 CI 不一致的失败 |

### 2.3 架构优化点

结论：**核心逻辑高度集中，单文件已经成为主要维护风险**；状态/审计词没有单一真源，新增状态要改很多处。

| 编号 | 位置 | 问题 | 严重度 | 影响 |
| --- | --- | --- | --- | --- |
| A1 | `rust/crates/bsend/src/reseed.rs` | 单文件 9,262 行，占核心源码约四分之一；审阅、上传、挂回、状态写入、动作日志都在里面 | 高 | 改一处容易碰坏别处；两人同时改这个文件容易冲突；review 成本高 |
| A2 | `rust/crates/bsend-cli/src/main.rs` | 2,277 行，命令解析、业务编排、输出格式、文件读写混在一起 | 中 | 命令层难做单测，新增命令会继续堆大文件 |
| A3 | `rust/crates/bsend-gui/src/` | 3,282 行，14 个文件里 12 个没有测试；`jobs.rs` 的异步任务也没有独立可测入口 | 中 | 界面/任务调度回归只能靠手工点 |
| A4 | `reseed.rs`、`autopost.rs`、`report.rs`、`bsend-cli`、`bsend-gui`、文档 | 状态词、审计词、界面标签、报表桶分散在多处；新增 `posted_verifying` 时已经在多个文件同步 | 中 | 新状态容易漏改一处，出现“日志有、报表没有”或“界面不显示” |
| A5 | `rust/crates/bsend/src/index.rs`、`reseed.rs`、`review.rs` | `ReviewPending` 同时表示“站内命中但清单对不上”和“站上已发布但没挂回”（靠 note 前缀 `posted_unresolved` 区分）；`ReviewPending` 与 `PendingReview` 名字又很像 | 中 | 运维容易按错处置方式；代码分支靠字符串前缀，后续扩展易漏 |
| A6 | `rust/crates/bsend/src/sites/nexusphp.rs` | 2,900 行，搜索、详情页、表单解析、上传、审核标记都在一个适配器里 | 低 | 单站适配改动影响面大，测试定位慢 |

### 2.4 代码写法

结论：**没有发现当前会立即导致错误的功能 bug，但有三处“未来会静默误分类”的写法**，和刚修掉的 `SameHashSkip` 属于同一类风险。

| 编号 | 位置 | 问题 | 严重度 | 影响 |
| --- | --- | --- | --- | --- |
| C1 | `reseed.rs` 的 `outcome_seeded` | 用 `note.contains("已挂回 qB")` 这种给人看的文案判断“是否已做种” | 中 | 文案一改，`seeded` 字段会静默翻转；测试也跟着文案走，保护力弱 |
| C2 | `reseed.rs` 的 `run_reseed` | 仍有 `Ok(other) => Posted` 兜底；当前只会命中 `Seeding`，但以后新增 `Ok` 状态会被误包成“发布成功” | 中 | 重演 `SameHashSkip` 被报成 `posted` 的问题 |
| C3 | `reseed.rs` 的 `audit_result`、`report.rs` 的 `audit_bucket` | 都有 `_ =>` 兜底：前者未知状态归 `failed`，后者未知结果归 `other` | 中 | 新增状态可能被静默记成失败或“其他”，而且不会编译报错提醒补映射 |
| C4 | `report.rs:614-624` | `audit_bucket` 没有映射 `posted_unresolved`，它会掉进 `other` | 中 | “站上已发、qB 未挂回”在审计周报里不可见，只剩 status 能看到 |
| C5 | `bsend-cli/src/main.rs` 的 `commas` | 用 `n.abs()`；`i64::MIN` 在 debug 下会 panic | 低 | 当前调用方都是计数，实际不可达；作为通用格式化函数仍应加保护 |
| C6 | `qbittorrent.rs` 的 `add_torrent_ex` | qB 返回 JSON 但缺 `success_count` 时默认按 1 条成功；只有明确 `success_count=0 && failure_count>0` 才报错 | 低 | qB 回包格式变化时可能把“没加进去”当成功；当前已有回查缓解 |

### 2.5 测试覆盖率

结论：**核心库覆盖较好，接口层和界面层覆盖明显偏薄**。因为本机没有可用的行覆盖率工具，下面用“测试数/千行”作为量化代理，并列出具体未覆盖路径；这不等于行覆盖率。

| 模块 | 行数 | 测试数 | 每千行测试数 | 判断 |
| --- | ---: | ---: | ---: | --- |
| `reseed.rs` | 9,262 | 124 | 13.4 | 主流程覆盖较好，但 9k 行单文件仍难保证分支全覆盖 |
| `sites/nexusphp.rs` | 2,900 | 31 | 10.7 | 解析/审核标记覆盖尚可，HTTP 失败路径偏少 |
| `bsend-cli/src/main.rs` | 2,277 | 15 | 6.6 | 只覆盖参数解析和少量格式化，命令处理函数未测 |
| `qbittorrent.rs` | 1,421 | 15 | 10.6 | 本轮补了 9 个契约测试，但公开方法仍有缺口 |
| `bsend-gui/src/` | 3,282 | 3 | 0.9 | 12 个视图/任务/控件文件 0 测试 |
| `kimi.rs` | 203 | 1 | 4.9 | 只测会话名，协议/超时/后台标签页未测 |
| `sites/mod.rs` | 1,357 | 6 | 4.4 | host 匹配有单测，147 站目录映射未穷举 |
| `reseed.rs` 之外的核心里 | — | — | — | `bencode`、`matcher`、`gates`、`report` 覆盖相对好 |

具体缺口：

| 编号 | 位置 | 缺口 | 严重度 | 影响 |
| --- | --- | --- | --- | --- |
| T1 | `qbittorrent.rs` | 本轮已补登录/版本/清单/文件参数回退/trackers 回退/409/空回包/导出/启停；仍缺 `all_tracker_urls`、`torrent_names`、`torrent_content_path`、`set_download_limit`、`remove_torrent`、`torrent_files_many` 的并发/部分失败 | 中 | qB 兼容性回归主要靠实测发现 |
| T2 | `bsend-cli/src/main.rs` | 只测 `Cli::try_parse_from` 和格式化；`cmd_status`、`cmd_report`、`cmd_audit`、`cmd_autopost`、`cmd_pull`、`cmd_followup` 都没直接测 | 中 | 输出格式、空数据、错误退出码容易回归 |
| T3 | `bsend-gui` | `jobs.rs`、`views/*`、`widgets.rs`、`theme.rs`、`types.rs` 全 0 测试 | 中 | 界面状态、任务切换、按钮行为没有自动检查 |
| T4 | `kimi.rs` | 只有 `browser_session_name_is_stable`；没有桥接协议错误、超时、`visibilityState=hidden` 分支 | 中 | audiences 判重路径出问题只能靠人工点验证 |
| T5 | `coverage_e2e.rs` | 已补 `posted_verifying`；仍缺 `pending_review` / `review_rejected` / `gone_from_site` / `pending_offers` 的完整 e2e | 中 | 审核回访链路的端到端回归不足 |
| T6 | CI / 工具链 | 没有覆盖率门禁；`cargo llvm-cov` 在本机 GNU 上失败（`profiler_builtins`），MSVC 缺 `llvm-tools-preview` 和 `link.exe` | 中 | 无法用数字判断覆盖率升降，只能靠测试数 |
| T7 | `scripts/` | 7 个 Python、3 个 PowerShell、3 个 shell 脚本没有自动测试 | 低 | 维护脚本改错只能人工发现 |
| T8 | `sites/mod.rs` | 147 站的目录映射/别名只做了部分样例 | 低 | 新站接入可能漏别名，影响 qB 覆盖判断 |
| T9 | `bencode` / `matcher` | 有 golden 和边界用例，但没有属性测试/模糊测试 | 低 | 极端畸形种子只能靠现有样例 |

### 2.6 测试严谨性

结论：**大多数断言是具体的，e2e 也真的检查了请求/落盘结果**；主要问题是少量测试和“给人看的文案/临时目录”耦合。

| 编号 | 位置 | 问题 | 严重度 | 影响 |
| --- | --- | --- | --- | --- |
| R1 | `reseed.rs` 的 `outcome_seeded` 测试 | 断言用的是固定 note 文案；文案和逻辑一起改，测试也会一起改，无法独立保护行为 | 中 | 文案漂移不会被测出来 |
| R2 | `config.rs:673`、`index.rs:693` 等 | 临时目录只用进程号做前缀，同进程并发测试如果共用同名目录可能互相踩 | 低 | 偶发测试污染/失败，难复现 |
| R3 | `coverage_e2e.rs` 的 mock server | 每个连接起线程、测试结束不显式关闭监听；大量 e2e 并行时会留下短命线程 | 低 | 测试资源泄漏，影响稳定性 |
| R4 | `report.rs` 的 `audit_buckets_map_known_results` | 只覆盖部分结果词；`pending_offers`、`pending_review`、`review_rejected`、`posted_unresolved` 没进表 | 低 | 新增审计词时回归保护不足 |
| R5 | GUI 测试 | 只有 3 个测试，且没有视图渲染/交互断言 | 中 | 界面回归无法自动发现 |

### 2.7 工程规范

结论：**CI 基本规范，版本管理和格式检查到位；但本地复现、覆盖率门禁、旧打包流水线三处需要整理**。

| 编号 | 位置 | 问题 | 严重度 | 影响 |
| --- | --- | --- | --- | --- |
| E1 | `.github/workflows/rust.yml` | 没有覆盖率步骤/门槛；测试通过只证明用例通过，不证明覆盖没下降 | 中 | 新增代码可能长期无测试而不报警 |
| E2 | `rust/Cargo.toml:rust-version = "1.82"` | 没有 `rust-toolchain.toml`，CI 也没有 MSRV 作业 | 中 | 代码可能悄悄用了高于 1.82 的特性，MSRV 声明失真 |
| E3 | `.github/workflows/release.yml` | Swift/.NET 遗留打包流水线仍在 `master` push 和所有 tag 上触发；Rust 出包另有 `rust-release.yml` | 中 | 每次推送多跑一套旧打包，tag 发版可能产生两个来源的安装包 |
| E4 | `scripts/ci-check.sh` | Windows 分支声称跑全 workspace，但本机默认 GNU 工具链缺 `ktmw32`，MSVC 又缺 `link.exe`；实际只能跑 `--exclude bsend-gui` | 中 | 本地彩排结果和 CI 不一致 |
| E5 | `scripts/ci-check.sh:9` vs `rust.yml` | 本地用 `cargo fmt --check`，CI 用 `cargo fmt --all -- --check` | 低 | 本地可能放过非默认成员的格式问题 |
| E6 | `.github/workflows/rust.yml` / `rust-release.yml` | `actions/checkout` 版本不一致（v7 与 v4） | 低 | 升级维护不统一 |
| E7 | 依赖安全 | 没有 `cargo audit` / `cargo deny` 步骤 | 低 | 依赖漏洞只能靠人工关注 |
| E8 | `scripts/` | 13 个维护脚本没有语法/行为测试 | 低 | 脚本坏掉只能运行时发现 |

## 3. 问题清单汇总

按严重度从高到低：

**高**

1. D10：文档里的 `--loop` 与 CLI 实际的 `--loop-mode` 不一致，照文档跑不起来。
2. D1：`ARCHITECTURE` 还写核心“发种前再确认”没接完，和 `ROADMAP`/代码冲突。
3. A1：`reseed.rs` 9,262 行，单文件承载主流程，维护和并行开发风险高。

**中**

4. D2：README 的上传限速默认值写错。
5. D4：状态表缺状态，`review_pending` 解释不准。
6. D5：待跟进类别文档只写两类，实际五类。
7. D6：`bsend pull` 的比对口径过期。
8. D7：三处日限额口径过期。
9. G1：架构模块表不完整。
10. G2：没有测试策略文档。
11. G3：本地全量测试工具链要求没写。
12. A2：CLI 单文件 2,277 行。
13. A3：GUI 12 个文件 0 测试。
14. A4：状态/审计词没有单一真源。
15. A5：`ReviewPending` / `PendingReview` 语义重叠。
16. C1：`outcome_seeded` 依赖文案。
17. C2：`run_reseed` 的 `Ok(other) => Posted` 兜底。
18. C3：审计映射的 `_ =>` 兜底。
19. C4：`posted_unresolved` 在审计周报掉进 `other`。
20. T1：qB 客户端仍有公开方法没有契约测试。
21. T2：CLI 命令处理函数没有直接测试。
22. T3：GUI 测试几乎为空。
23. T4：Kimi 桥接没有异常路径测试。
24. T5：审核类状态的 e2e 不足。
25. T6：没有行覆盖率数字/门禁。
26. R1：`outcome_seeded` 测试与文案耦合。
27. R5：GUI 没有行为断言。
28. E1：没有覆盖率门禁。
29. E2：MSRV 没有 CI 守护。
30. E3：旧打包流水线仍自动触发。
31. E4：本地全量测试跑不起来。

**低**

32. D3：README 白名单数字 42 对 40。
33. D8：`seeded=false` 说明不够准确。
34. D9：审计词表缺几个词。
35. A6：`sites/nexusphp.rs` 2,900 行。
36. C5：`commas(i64::MIN)` 潜在 panic。
37. C6：qB add 缺 `success_count` 时默认成功。
38. T7：维护脚本无测试。
39. T8：147 站映射未穷举。
40. T9：bencode/matcher 缺属性/模糊测试。
41. R2：临时目录只用进程号。
42. R3：mock server 线程不关闭。
43. R4：审计桶测试不全。
44. E5：本地 fmt 与 CI fmt 参数不一致。
45. E6：action 版本不一致。
46. E7：没有依赖漏洞扫描。
47. E8：脚本没有语法/行为测试。

## 4. 改进建议

### R1. 让 `--loop` 和文档一致（P1，可并行 A）

- 对应问题：D10。
- 做法：把 `bsend-cli/src/main.rs` 的 `Autopost.loop_mode` 参数改成 `#[arg(long = "loop", alias = "loop-mode")]`；`--loop` 成为主名字，旧脚本里的 `--loop-mode` 仍可用。补一条测试，两个写法都能解析。
- affected_files：`rust/crates/bsend-cli/src/main.rs`
- 验证：`cargo test -p bsend-cli`；执行 `bsend autopost --loop --dry-run --limit 1`，确认不再报未知参数。

### R2. 校正核心文档口径（P1，可并行 A，三份文档分开做）

- 对应问题：D1、D2、D3、D4、D5、D6、D7、D8、D9、D10 的文档侧，以及 `SITES.md:156`。
- 做法：
  - `ARCHITECTURE`：改第 23 行“再确认已接完”；第 256 行改成“`posted` 与 `posted_verifying` 都算一次上传”；状态表补齐 5 个状态并把 `review_pending` 改成“站内命中但清单/hash 对不上”；第 299 行按实际 9,262 行更新拆分说明。
  - `OPERATIONS`：第 85 行改成五类待跟进；第 221 行改成“pieces 和文件清单一致即可，允许站点改 hash/标题”；第 247 行改成“重复/失败/待跟进不占；`posted` 和 `posted_verifying` 占”；第 132 行补“`posted_verifying` 也会 `seeded=false`”；第 116 行审计词表补 `posted_unresolved`、`pending_review`、`review_rejected`。
  - `README` / `SITES` / `ROADMAP`：README 18 行改成“上传限速默认跟随 qB 全局（`defaultUpLimit=0`）”；README 52 行 42 改 40；SITES 156 行补 `posted_verifying`；ROADMAP 101 行把“只算 Posted”改成“`posted` 和 `posted_verifying` 都算”，135 行按 9,262 行更新。
- affected_files：`doc/ARCHITECTURE.md`、`doc/OPERATIONS.md`、`README.md`、`doc/SITES.md`、`doc/ROADMAP.md`
- 验证：逐条搜索旧措辞应当为 0 命中；`bsend status` / `bsend audit` 输出与文档词表逐项对照；让一个没有项目背景的人按文档跑 `autopost --loop` 能跑通。

### R3. 把状态/审计词收成单一真源（P1，需串行 B）

- 对应问题：A4、A5、C1、C2、C3、C4、R1、R4。
- 做法：
  1. 把 `ReseedOutcome::audit_result` 的字符串改成枚举（例如 `AuditWord`），`as_str` 只在一处定义；`report::audit_bucket` 对枚举做**穷举**匹配，去掉 `_ =>`，新增状态编译期就会报错。
  2. `outcome_seeded` 不再读 note，改成由调用方传 `mounted: bool` 或直接读 `CoverageStatus`；`Verifying` 明确按“已加入但未确认做种”处理。
  3. `run_reseed` 去掉 `Ok(other) => Posted`，显式处理 `Seeding`，其余 `Ok` 状态返回错误或 `Duplicate`，不允许静默成功。
  4. `ReviewPending` 拆成 `MismatchPending`（清单/hash 对不上）和 `PostUnresolved`（站上已发、qB 没挂回），或至少在状态枚举层保留独立值，不再靠 note 前缀。
  5. `report::audit_bucket` 增加 `posted_unresolved` 桶（可归“待人工”），并在 `bsend audit` 输出里显示。
- affected_files：`rust/crates/bsend/src/reseed.rs`、`rust/crates/bsend/src/autopost.rs`、`rust/crates/bsend/src/report.rs`、`rust/crates/bsend/src/index.rs`、`rust/crates/bsend-cli/src/main.rs`、`rust/crates/bsend-gui/src/views/*.rs`、`doc/ARCHITECTURE.md`、`doc/OPERATIONS.md`
- 验证：`cargo fmt --all -- --check`；`cargo clippy -p bsend -p bsend-cli --all-targets -- -D warnings`；`cargo test -p bsend --lib` + `coverage_e2e`；构造 `posted_verifying`、`pending_review`、`review_rejected`、`gone_from_site`、`posted_unresolved` 五种审计输入，确认周报桶和状态输出都正确。

### R4. 拆分 `reseed.rs`（P1，需串行 B，放在 R3 之后）

- 对应问题：A1、A6。
- 做法：先按“共享入口 / 审阅 / 上传 / 挂回 / 记录读写”切模块，只移动代码不改行为；第一步把纯函数和记录读写搬出去，保留兼容 re-export；每搬一批跑一次全量测试。`sites/nexusphp.rs` 可作为第二步，把表单解析、上传、审核标记拆开。
- affected_files：`rust/crates/bsend/src/reseed.rs`、新建 `rust/crates/bsend/src/reseed/*.rs`、`rust/crates/bsend/src/lib.rs`、`rust/crates/bsend/src/sites/nexusphp.rs`、`doc/ARCHITECTURE.md`、`doc/ROADMAP.md`
- 验证：每批 `cargo test -p bsend` + `cargo test -p bsend --test coverage_e2e` 全绿；`cargo clippy`；对比拆分前后的公开函数签名和 e2e 行为。

### R5. 补齐 qB 客户端的剩余契约测试（P2，可并行 A）

- 对应问题：T1。
- 做法：用本轮已有的本地 mock 服务，补 `all_tracker_urls`（50000 条上限告警/普通解析）、`torrent_names`（空输入/非 200/坏 JSON）、`torrent_content_path`（空串过滤）、`set_download_limit`/`remove_torrent`（只记录表单，不抛错）、`torrent_files_many`（空列表、部分失败、并发回调顺序）。这些只测客户端契约，不连真实 qB。
- affected_files：`rust/crates/bsend/src/qbittorrent.rs`
- 验证：`cargo test -p bsend --lib qbittorrent::tests`；断言的请求路径和表单字段与 qB Web API v2 文档一致。

### R6. 拆 CLI 并补命令层测试（P2，需串行 B，放在 R1/R3 之后）

- 对应问题：A2、T2。
- 做法：把 `main.rs` 拆成 `cli/args.rs`、`cli/commands/{status,report,audit,autopost,pull,followup}.rs`、`cli/ui.rs`；纯格式化函数进 `ui.rs`；命令处理函数改为接受可注入的 `Config`/索引/输出 writer，测试用临时目录和假报告。先补空索引、无白名单、坏配置三条错误路径。
- affected_files：`rust/crates/bsend-cli/src/main.rs`、新建 `rust/crates/bsend-cli/src/cli/*.rs`、`rust/crates/bsend-cli/Cargo.toml`
- 验证：`cargo test -p bsend-cli`；`bsend status` / `report` / `audit` / `autopost --dry-run` 在临时数据目录下能输出稳定文本/JSON；错误退出码符合预期。

### R7. 给 GUI 建立可测的 view-model（P2，需串行 B，放在 R3 之后）

- 对应问题：A3、T3、R5。
- 做法：把 `jobs.rs` 的状态转换、`views/*` 的纯展示数据、`widgets.rs` 的格式化逻辑抽成不依赖 GPUI 窗口的结构；对渲染函数只测输入输出，对任务用假通道驱动。先在 Windows MSVC 环境跑，Linux 继续排除 GUI。
- affected_files：`rust/crates/bsend-gui/src/jobs.rs`、`rust/crates/bsend-gui/src/views/*.rs`、`rust/crates/bsend-gui/src/widgets.rs`、`rust/crates/bsend-gui/src/theme.rs`、`rust/crates/bsend-gui/src/types.rs`、`rust/crates/bsend-gui/Cargo.toml`
- 验证：`cargo test -p bsend-gui`（Windows MSVC）；至少覆盖空状态、加载中、错误、成功四种视图；`cargo clippy -p bsend-gui --all-targets -- -D warnings`。

### R8. 补审核类状态的 e2e（P2，可并行 A）

- 对应问题：T5。
- 做法：在 `coverage_e2e.rs` 增加四条：站上已发布但详情页标“未审”→ `PendingReview`；标“审核不通过”→ `ReviewRejected`；详情页 404→ `GoneFromSite`；上传回包 `torrent_too_large`/`offers.php` → `PendingOffers` 且不重试。每条断言状态、审计词、无第二次上传请求。
- affected_files：`rust/crates/bsend/tests/coverage_e2e.rs`
- 验证：`cargo test -p bsend --test coverage_e2e`；确认四条都不会进入失败冷却，`bsend followup` 能看到。

### R9. 加覆盖率与 MSRV 门禁（P2，可并行 A）

- 对应问题：T6、E1、E2。
- 做法：在 `rust.yml` 增加一个 Ubuntu 作业安装 `llvm-tools-preview`，跑 `cargo llvm-cov --workspace --exclude bsend-gui --summary-only` 并上传 lcov；先设“不低于当前基线”的软门槛，稳定后再提高。另加一个 `cargo +1.82 check` 或 `rust-toolchain.toml` 固定 MSRV，避免代码用到更高版本特性。
- affected_files：`.github/workflows/rust.yml`、`rust/Cargo.toml`、新建 `rust/rust-toolchain.toml`、`rust/README.md`
- 验证：PR 上覆盖率作业能出数字；故意删一个测试应当低于基线；MSRV 作业在 1.82 上能编译核心库和 CLI。

### R10. 收敛打包流水线（P2，可并行 A）

- 对应问题：E3、E6。
- 做法：`release.yml` 的旧 Swift/.NET 流水线改成只 `workflow_dispatch`（或归档到 `legacy/` 不自动触发）；`rust-release.yml` 作为唯一 tag 发版入口。两个 workflow 的 `actions/checkout` 统一到同一版本。
- affected_files：`.github/workflows/release.yml`、`.github/workflows/rust-release.yml`
- 验证：推一个测试 tag，确认只产生一套 Rust 安装包；`master` push 不再触发旧流水线。

### R11. 补测试地图和模块地图（P2，需串行 B，放在 R2 之后）

- 对应问题：G1、G2、G3。
- 做法：新增 `doc/TESTING.md`，写清测试分层（纯函数 / mock e2e / 真实站点实测）、已有夹具、如何跑单条、如何加新用例、覆盖率工具限制；`ARCHITECTURE` 第 11 节补全缺失模块；README 写明本机 GUI 需要 MSVC 或使用 `--exclude bsend-gui`。
- affected_files：新建 `doc/TESTING.md`、`doc/ARCHITECTURE.md`、`README.md`、`rust/README.md`
- 验证：按文档从零跑 `cargo test -p bsend --lib`、`cargo test -p bsend --test coverage_e2e`、`cargo test --workspace --exclude bsend-gui`；一个新 Agent 只看这三份文档能定位测试和模块。

### R12. 维护脚本测试与模糊测试（P3，可并行 A/B）

- 对应问题：T7、T8、T9、E8。
- 做法：给 `scripts/` 的 Python 脚本加最小 `pytest`（参数解析、坏输入、原子写）；PowerShell 用 `PSScriptAnalyzer` 或 dry-run；bencode/matcher 加确定性随机种子生成器（不引入随机依赖）做 100 组 round-trip/边界，失败时打印种子。不要用真实站点数据做随机测试。
- affected_files：`scripts/*.py`、`scripts/*.ps1`、新建 `scripts/tests/`、`rust/crates/bsend/src/bencode.rs`、`rust/crates/bsend/src/matcher.rs`
- 验证：`pytest scripts/tests`；`cargo test -p bsend`；随机用例固定种子可复现。

## 5. 优先级与并行性

### 5.1 第一批（P1）

| 建议 | 优先级 | 并行性 | 冲突说明 |
| --- | --- | --- | --- |
| R1 `--loop` 修正 | P1 | 可并行 A | 只碰 `bsend-cli/src/main.rs` |
| R2 文档校正 | P1 | 可并行 A（三份文档分开） | `ARCHITECTURE`、`OPERATIONS`、`README/SITES/ROADMAP` 互不重叠 |
| R3 状态/审计单一真源 | P1 | 需串行 B | 会碰 `reseed`/`report`/CLI/GUI/文档，和 R1 有 CLI 重叠 |
| R4 `reseed.rs` 拆分 | P1 | 需串行 B | 会碰 `reseed.rs`，必须排在 R3 之后 |

### 5.2 第二批（P2）

| 建议 | 优先级 | 并行性 | 冲突说明 |
| --- | --- | --- | --- |
| R5 qB 剩余契约测试 | P2 | 可并行 A | 只碰 `qbittorrent.rs` |
| R8 审核类 e2e | P2 | 可并行 A | 只碰 `coverage_e2e.rs` |
| R9 覆盖率/MSRV | P2 | 可并行 A | 只碰 CI/Cargo/README |
| R10 打包收敛 | P2 | 可并行 A | 只碰两个 workflow |
| R6 CLI 拆分 | P2 | 需串行 B | 和 R1/R3 都碰 CLI |
| R7 GUI 可测化 | P2 | 需串行 B | 和 R3 都碰 GUI/文档 |
| R11 测试/模块地图 | P2 | 需串行 B | 和 R2 都碰 README/ARCHITECTURE |

### 5.3 第三批（P3）

| 建议 | 优先级 | 并行性 | 冲突说明 |
| --- | --- | --- | --- |
| R12 脚本测试/模糊测试 | P3 | 可并行 A/B | 只碰 scripts 和两个核心算法文件；如同时做 bencode/matcher 则与 R4 分开排期 |

### 5.4 并行门禁检查

- A 组（R1、R2 的三份文档、R5、R8、R9、R10、R12）：文件清单交集为空，可同时开工。
- B 组（R3、R4、R6、R7、R11）：都依赖或修改核心状态/文档，必须串行；建议顺序 R3 → R4 → R6 → R7 → R11。
- 任何并行任务开始前，先用 `git status -sb` 确认工作树干净；同一文件不要两个 Agent 同时写。

## 6. 本轮测试分支的后续动作（不计入审计结论）

审计之后按 `software-improvement` 的测试分支执行了第一批补测，详细用例见 `doc/TEST-REPORT-2026-11-25.md`。概述：

- 新增 20 个测试：qB 客户端 9 个、CLI 9 个、自动发种审计 1 个、`posted_verifying` e2e 1 个。
- 全量测试从 396 个增加到 416 个；本机实际跑 413 个（GUI 3 个未在本机编译）。
- `cargo test --workspace --exclude bsend-gui` 全绿；`cargo clippy --workspace --all-targets --exclude bsend-gui -- -D warnings` 全绿；`cargo fmt --all` 通过。
- 本机 `cargo test --workspace` 失败在 GUI 链接（GNU 缺 `ktmw32`；MSVC 缺 `link.exe`），不是本次测试逻辑失败；2026-11-25 12:54 推送后，远端 `rust` 工作流 success：Linux 核心库 + CLI 全绿，Windows 全 workspace（含 GUI）全绿。
- 行覆盖率没有测出来：GNU 的 `cargo llvm-cov` 报 `profiler_builtins` 缺失；MSVC 缺 `llvm-tools-preview` 和链接器。报告里用“测试数/千行”代替，不能当行覆盖率。

## 7. 自查

- 七个维度均有结论：文档一致性、文档缺漏、架构、代码写法、测试覆盖率、测试严谨性、工程规范。
- 每条问题都有位置、描述、严重度、影响范围。
- 每条建议都有具体做法、文件清单、验证方式。
- 并行组内已检查 `affected_files` 无重叠；跨组冲突已标“需串行”。
- 本报告只描述发现，没有在审计阶段改代码。

如果要按这份报告开工，建议先做 R1（一个小改动让文档命令真正可用），再做 R2（纯文档校正，风险最低），然后按 B 组顺序处理 R3/R4。R5/R8 已经在本轮补了一部分，剩余部分可作为下一轮并行任务。

## 8. 2026-11-26 进展

- R8/T5 的审核状态 e2e 已补：上传被审核拦截落 `pending_review`、站点回 `torrent_too_large` 落 `pending_offers`、回访分别识别“未审 / 审核不通过 / 站上已找不到”，以及审核通过后下载站点种子并挂回 qB。四条用例在 `coverage_e2e.rs`，`cargo test -p bsend` 现在 423 个通过。
- R2 只在 `coverage_e2e.rs` 的测试临时目录上补了“进程号 + 递增序号”；`config.rs`、`index.rs` 里仍只用进程号的临时目录还没改。
- 其余未完成项按本报告第 2 节状态表保持不变；本轮没有动生产代码。
