# BoxSend 项目审计报告（2026-11-27）

审计对象：`C:\Users\Schal\WorkBuddy\2026-11-17-20-52-34`，Git 基线 `a81b568`（`master`，在 `1c666a7` 审计后合入 `7046f4d` 测试分支和 `a81b568` tracker 真值修复），2026-11-27 16:30 开始、17:00 复核。
审计范围：Rust workspace、发布流程、站点适配、测试、文档、脚本和当前运行态。审计只记录问题与建议，不改代码。

## 结论摘要

项目当前可以构建、测试和真实发种，但“发种成功”的判定存在一个会误导人的缺口：**qB 校验到 100% 就写“已在做种”，没有核对 tracker 是否真的联系过站点**。本轮城市 13 的 `88464` 就是典型：程序记了 `reseeded`，qB 进度 100%，但站点详情页显示 `0 个做种者 | 2 个下载者`，qB tracker 状态先是 1（未联系）、恢复诊断配置后变 4（报错）。这会把“已挂回”误报成“站点已看见我在做种”。覆盖格已人工改成 `review_pending`（待人工确认）。

本轮已经走通的真实链路是 iloli `56513`：站点详情正常、qB tracker 状态 2、目标种已有 131,072 字节上传，站点页 3 个做种者 / 14 个下载者。pthome `285040` 也完成了发布和挂回，16:32 tracker 变成 `status=2`，站点已看见 announce，但 `num_seeds=0`、`uploaded=0`，页面被 Cloudflare 挡住，不能写“页面已确认做种”。city13 `88464` 完成了发布和挂回，但 tracker 长时间 `status=1`、恢复诊断配置后变 `status=4`，手动带 passkey announce 被 TLS 连接重置，站点页 0 做种者，不能按“已做种”汇报。

优先级建议：

1. **P0** 修 tracker 真值判定，再修测试和文档口径。
2. **P1** 把 `reseed.rs` 和命令行入口拆分，降低继续加站点规则的回归风险。
3. **P1** 把 `artifacts/` 和根目录调试材料纳入忽略规则，里面有 7 个含 passkey 的 torrent 文件。
4. **P2** 补 GUI、CLI、qB tracker 状态和 MSRV/覆盖率的持续校验。

## 一、现状描述

### 1.1 规模与结构

- Git：`master`，审计开始基线 `1c666a7`；最终收尾在 `a81b568`（含 `7046f4d` 的 5 条测试分支和 tracker 真值修复），远端最新仍是 `db9e16b`，`7046f4d`、`a81b568` 尚未推送。
- Rust 源文件 45 个，合计 42,353 行；按 `#[cfg(test)]` 估算生产代码约 31,237 行。
- 测试属性 513 个（本报告初稿写 507，后续补了 6 条：5 条测试分支 + 1 条 tracker 状态）。`cargo test --workspace` 实测：核心库 454、`coverage_e2e` 33、其余 2/2/16/3/3，全部通过。
- 最大模块：`reseed.rs` 11,857 行、345 个函数、49 个公开函数、168 个测试；`coverage_e2e.rs` 3,582 行、33 个测试；`sites/nexusphp.rs` 3,091 行、36 个测试；`bsend-cli/main.rs` 2,380 行、16 个内联测试，加 `tests/cli_smoke.rs` 3 条共 19 条。
- GUI：`bsend-gui/src` 约 3,298 行，只有 3 个测试。
- 配置：`sourceSites` 147 个，启用 101 个；`autoReseedSites` 46 个；`reseedPausedSites` 35 个；暂停站 8 个。
- 运行态：qB 5.3.0 有 42,469 条种子；`max_concurrent_http_announces=200`；重启后载入 4 万多条种子约 9 分钟，期间新种 tracker 状态长期为 1。

### 1.2 文档与测试基线

- `doc/ARCHITECTURE.md` 约 38 KB，`ROADMAP.md` 约 58 KB，`SITES.md` 约 103 KB，`OPERATIONS.md` 约 34 KB。
- 历史审计 `doc/AUDIT-REPORT-2026-11-25.md` 有 47 条，26 条已解决、5 条部分完成、16 条未完成。本次抽查的未完成项仍集中在 `reseed.rs` 拆分、CLI 测试、GUI 测试、qB 边界测试、文档过期和工程规范。
- 覆盖率工具：GNU 工具链没有 `profiler_builtins`，`cargo llvm-cov` 失败；已尝试用 MSVC + `rust-lld`，但需要先装 `llvm-tools-preview`，本轮没有拿到行覆盖率数字。不能把“测试全绿”写成“覆盖率已量化”。

### 1.3 本轮真实发种证据

| 站点 | 详情页 | 当前结论 | 不能夸大的地方 |
|---|---|---|---|
| iloli | https://mua.xloli.cc/details.php?id=56513 | 已发布；qB tracker 状态 2（16:34 有一次 4: stream truncated）、上传 131,072 字节；站点页 3 个做种者 / 14 个下载者 | 唯一完整闭环；单次 announce 抖动不影响前一次成功回包 |
| pthome | https://www.pthome.net/details.php?id=285040 | 已发布、已挂回；tracker 16:32 变成 `status=2`，站点已看见 announce | `num_seeds=0`、`uploaded=0`；页面被 Cloudflare 拦截，不能写“页面已确认做种” |
| city13 | https://13city.org/details.php?id=88464 | 已发布、已挂回、qB 校验 100%、程序曾记 `reseeded` | 长时间 `status=1`，恢复诊断配置后变 `status=4`；手动带 passkey announce 和代理都 TLS reset；详情页 `0 个做种者 / 2 个下载者`；**不能算站点已看见做种** |

## 二、七个维度排查

### 维度 1：文档一致性

**结论：存在 4 处会直接误导执行的过期描述。**

| 编号 | 位置 | 问题 | 严重程度 | 影响范围 |
|---|---|---|---|---|
| D1 | `AGENTS.md:27` | 写 `reseed.rs` “单文件 5k+ 行”，实际 11,857 行 | 中 | 违反“文档描述当前实现”的规则；后续拆分评估会低估规模 |
| D2 | `README.md:55`、`doc/SITES.md:8-9` | 写自动发种白名单 45 站；`SITES.md:15` 又写 46 站；配置实际 46 站 | 中 | 运维按 45 站核对会漏掉一个站 |
| D3 | `doc/OPERATIONS.md:137` | 仍写“手动 `bsend reseed --yes` 只写 reseed-log，不写 auto-audit，日限额看不到” | 高 | 与 `1c666a7` 的新行为相反；按文档排查会误判手动发布不占额度 |
| D4 | `doc/TESTING.md:89-92` | 初稿时仍写全 workspace 测试会卡在 GUI 链接，建议 `--exclude bsend-gui`；收尾已改成 `cargo test --workspace`，最终 513 条全绿 | 中 | 新环境按旧命令会少跑 GUI；也会误以为所有测试已覆盖 |
| D5 | `doc/TEST-REPORT-2026-11-25.md:30-31` | 仍写 396/432 条基线，和当前 513 条不一致；没有记录本轮新增的手动审计、AC3、IMDb 侧栏和标题角标测试 | 中 | 测试报告不能作为当前质量证据 |
| D6 | `doc/ARCHITECTURE.md:59,202,220,248` | 把“qB progress=1”直接等价为“已在做种”，没有区分 tracker 是否已联系站点 | 高 | 支撑了本轮 city13 的假阳性结论 |

> 上表是 16:30 审计时的现场。D1–D5 已在同日 `7046f4d`/`a81b568` 的文档收尾里改掉（D4 最终按 workspace 全量跑）、D6 已在 `ARCHITECTURE.md` 改成“progress=1 只代表本地校验，tracker status=2 才是站点已回包”；保留原发现是为了对照审计过程。G1–G3 也已在 `ARCHITECTURE.md` 5.7 和 `OPERATIONS.md` 4.0/手动审计节补齐。

### 维度 2：文档缺漏

**结论：核心流程有文档，但缺“如何证明站点真的看见做种”和“qB 拥塞时怎么处理”的运维说明。**

| 编号 | 位置 | 问题 | 严重程度 | 影响范围 |
|---|---|---|---|---|
| G1 | `doc/ARCHITECTURE.md`、`doc/OPERATIONS.md` | 没有定义 `progress=100`、`tracker_status=1/2`、站点种子数三者的判定关系 | 高 | 会把“挂回成功”误写成“已做种”；审核跟进也会误判 |
| G2 | `doc/OPERATIONS.md` | 没有记录 qB 4 万条种子、200 并发 announce 导致新种排队的时间量级和处置办法 | 中 | 新种长时间 status=1 会被误当成配置错误 |
| G3 | `doc/OPERATIONS.md` | 没有记录手动 `reseed --yes` 写 `auto-audit.jsonl` 的字段、额度和失败语义 | 中 | 人工补发和自动队列的额度口径容易冲突 |

### 维度 3：架构优化点

**结论：最大的架构问题是单文件职责过载，第二位是 CLI 入口和验证逻辑重复。**

| 编号 | 位置 | 问题 | 严重程度 | 影响范围 |
|---|---|---|---|---|
| A1 | `rust/crates/bsend/src/reseed.rs` | 11,857 行、345 个函数、49 个公开函数；选档、判重、上传、回包解析、挂回、审计都在一个文件 | 高 | 任何站点规则改动都可能触发无关回归；审查和并行开发几乎不可行 |
| A2 | `rust/crates/bsend-cli/src/main.rs` | 2,380 行，命令解析和业务执行混在一起；命令处理测试只有 16 个 | 中 | 新增命令继续堆积；命令行为缺少集成测试 |
| A3 | `reseed.rs:8136-8152`、`coverage.rs:1470-1478` | 两处各自判断“progress 100 = 已在做种”，没有共享一个可测试的终态判定函数 | 高 | 两条路径会一起产生假阳性；修一处漏一处 |
| A4 | `qbittorrent.rs:372-405` | `trackers()` 只返回 URL，不返回状态；调用方无法判断 tracker 是否真的联系过站点 | 高 | 直接造成 city13/pthome 的误报 |
| A5 | `sites/nexusphp.rs` | 3,091 行，搜索、详情解析、审核标记、上传字段都在一个适配器；`ReviewPending` 仍靠字符串前缀/文案判断 | 中 | 站点页面小改就会误判；新增站点测试成本高 |

### 维度 4：代码写法

**结论：本轮修复方向正确，但“成功”语义仍有一个高严重度缺口，另有 3 个低风险代码味道。**

| 编号 | 位置 | 问题 | 严重程度 | 影响范围 |
|---|---|---|---|---|
| C1 | `reseed.rs:8136-8152` | 校验到 100% 立即写 `CoverageStatus::Reseeded` 和“已在做种”，不检查 `qb.trackers()` 的状态，也不看站点是否已有该 peer | 高 | 真实站点显示 0 做种者时程序仍报成功；后续自动审计和覆盖统计都不可信 |
| C2 | `coverage.rs:1470-1478` | 上一轮 `Verifying` 收尾同样只看 progress，100% 就写 `Assisted` + “已在做种” | 高 | 同一假阳性会在下一轮扫描中再次出现 |
| C3 | `reseed.rs:6550`、`bsend-cli/main.rs:861-868` | `ReseedOutcome::note()` 已实现但 CLI 传的是 `outcome.display()`；审计里的 `note` 是展示文案，不是结构化备注 | 低 | 审计字段语义漂移；以后改展示文案会连带改变审计含义 |
| C4 | `autopost.rs:667` | `append_manual_audit` 对手动 `reseed` 的每种结果都写一条，包括重复/未发布结果；没有 run id | 低 | `auto-audit.jsonl` 会累积重复记录；排查时需要靠 url/result 去重 |
| C5 | `reseed.rs` 的 `first_imdb_hit` 修复 | 只按标题匹配第一条带 title 的 details 链接；如果侧栏标题恰好和发布名相同仍可能误命中 | 低 | 概率低，但判重误命中会直接阻断发布 |

### 维度 5：测试覆盖率

**结论：核心纯函数覆盖较好，但“高风险路径”和“用户入口”覆盖不足；没有行覆盖率数字，不能宣称覆盖充分。**

| 模块 | 行数 | 测试数 | 每千行测试数 | 主要缺口 |
|---|---:|---:|---:|---|
| `reseed.rs` | 11,857 | 168 | 14.2 | 345 个函数只对应 168 个测试；上传回包、tracker 终态、失败分支覆盖不均衡 |
| `coverage.rs` | 2,164 | 26 | 12.0 | `collect_verifying` 的 tracker 状态分支、并发/超时、旧 miss 写入路径 |
| `sites/nexusphp.rs` | 3,091 | 35 | 11.3 | 页面结构变体、审核标记、搜索空结果/侧栏噪声 |
| `bsend-cli/main.rs` | 2,380 | 16 | 6.7 | 命令参数组合、真实配置加载、审计/报告输出 |
| `bsend-gui` | 3,298 | 3 | 0.9 | 视图状态、任务按钮、错误提示、无头测试 |
| `qbittorrent.rs` | 1,627 | 22 | 13.5 | tracker 状态、4 万条种子的分页/上限、登录失效 |
| `autopost.rs` | 1,804 | 24 | 13.3 | 手动审计、失败冷却、重复记录 |
| `config.rs` | 1,167 | 17 | 14.6 | 临时目录隔离、配置迁移、默认值 |
| `gates.rs` | 962 | 20 | 20.8 | 真实站点字段组合仍靠 e2e |
| `report.rs` | 1,501 | 27 | 18.0 | 大索引、空索引、异常状态聚合 |

补充事实：

- `cargo test --workspace` 513 条全绿，但这只是“现有断言通过”，不是覆盖率证明。
- `cargo llvm-cov` 在 GNU 工具链因缺 `profiler_builtins` 失败；MSVC 方案还在装 `llvm-tools-preview`，本轮没有拿到数字。
- GUI 只有 3 个测试，且没有真实窗口/事件循环的自动化。
- `scripts/` 只有语法检查，没有行为测试；`kimi.rs` 的可见性等待和 40 轮重试没有 mock 覆盖。

### 维度 6：测试严谨性

**结论：大部分测试是真实断言，不是“跑通即通过”；但有 3 处隔离/边界问题会制造假绿。**

| 编号 | 位置 | 问题 | 严重程度 | 影响范围 |
|---|---|---|---|---|
| R1 | `config.rs:719-720` | `write_temp` 按进程号共用 `bsend-cfg-{pid}`，多个配置测试并行时可能互写 `boxsend.json`/`cookies.json` | 中 | 偶发失败会被误判为产品 bug；`write_temp_isolated` 只覆盖了部分测试 |
| R2 | `coverage_e2e.rs` 多个 mock server | 测试结束没有统一关闭监听端口，连续跑多轮可能占用端口/留下线程 | 低 | 本地重复跑 e2e 偶发端口冲突 |
| R3 | 多处断言用中文 note 文本 | 文案改动会让状态断言跟着改，测试保护的是文案而不是行为 | 中 | 类似 C3；重构时容易把测试改成“迁就实现” |
| R4 | `autopost.rs` 手动审计测试 | 只覆盖 `posted` 一种结果，没有覆盖 duplicate/blocked/审计写失败 | 中 | 新行为在真实失败路径上没有保护 |

### 维度 7：工程规范

**结论：构建和 CI 基本规范，但敏感证据管理、MSRV、覆盖率和大规模 qB 运行态没有形成制度。**

| 编号 | 位置 | 问题 | 严重程度 | 影响范围 |
|---|---|---|---|---|
| E1 | `.gitignore`、根目录 `artifacts/` | `artifacts/` 未忽略；实测 125 个文件、约 36.5 MB，其中 7 个 `.torrent` 含 `passkey=`；根目录还有 48 个未跟踪证据文件 | 高 | 一次误 `git add -A` 就可能把站点 passkey 推到公开仓库 |
| E2 | 运行态 qB | 42,469 条种子、200 并发 announce、重启约 9 分钟；新种 tracker status=1 可持续很久 | 高 | 发布成功但站点看不到做种；用户会以为代码坏了 |
| E3 | `.github/workflows/rust.yml` | CI 只跑 fmt/clippy/test，没有 MSRV 任务，也没有覆盖率或 `scripts` 行为测试 | 中 | MSRV 声明和实际依赖漂移不会被发现；覆盖率无法持续 |
| E4 | 仓库根目录 | 48 个未跟踪调试文件与源码混在一起，没有归档目录/清理规则 | 中 | 审查时噪声大；敏感材料容易误提交 |

## 三、改进建议

### S1（P0）把“已在做种”改成 tracker 真值判定

**2026-11-27 落地状态：部分完成。** `qbittorrent::tracker_info()` 已能返回 `url/status/msg/num_seeds/num_leeches/num_downloaded`，reseed 和 coverage 的 100% 收尾都会把 tracker 状态写进 note；`same_hash_on_target` 已增加 `status=2` 条件。还没有做的是“状态机”：tracker 没回包时仍然落 `Reseeded/Assisted`（只是 note 不再说“已在做种”），没有新增独立状态，也没有在 100% 后轮询 2-3 次。下面第 2/3/4 条的完整状态改造仍待做。本轮已把 city13 `88464` 的历史格人工改成 `review_pending`，pthome `285040` 的 note 改成 tracker 16:32 已回包。

**具体做法：**

1. 在 `qbittorrent.rs` 增加 `tracker_statuses(hash) -> Result<Vec<TrackerState>>`，解析 qB `torrents/trackers` 的 `url/status/msg/num_peers`；保留现有 `trackers()` 兼容调用。
2. 在 `reseed.rs` 和 `coverage.rs` 共用一个纯函数 `seeding_verdict(progress, tracker_statuses) -> SeedingVerdict`：`progress >= 0.999` 且目标 tracker 状态为 `2` 才算 `Reseeded/Assisted`；状态 `1`/`4` 返回 `Verifying`，note 写明“已校验 100%，tracker 还没联系站点/报错”。
3. `push_target_torrent` 在 progress 100 后轮询 2-3 次 tracker 状态；仍为 1 就落 `Verifying` + `verify_hash`，不要写“已在做种”。
4. `collect_verifying` 用同一函数收尾；tracker 仍为 1 时返回 `None`，保持 `Verifying`，不要清掉凭据。
5. 给 qB mock 增加 status=1/2/4 三种响应；补“progress 100 + status 1 不得写 Reseeded”的回归测试。

**affected_files：**

- `rust/crates/bsend/src/qbittorrent.rs`（新增状态结构与方法）
- `rust/crates/bsend/src/reseed.rs`（`push_target_torrent` 终态判定）
- `rust/crates/bsend/src/coverage.rs`（`collect_verifying` 终态判定）
- `rust/crates/bsend/src/index.rs`（如需新增 `Verifying` 原因字段，可只改注释）
- `rust/crates/bsend/tests/coverage_e2e.rs`（三种 tracker 响应）
- `rust/crates/bsend/src/qbittorrent.rs` 末尾测试模块（解析单测）

**验证方式：**

- `cargo test --workspace` 全绿，新增用例在 status=1 时断言 `Verifying`、status=2 时断言 `Reseeded`。
- 用 city13 真实种复跑：站点详情仍为 0 做种者时，程序不得打印“已在做种”。
- 用 iloli 真实种复跑：tracker 状态 2 时仍能正常记 `Reseeded`。

**优先级：** P0。**并行性：** 可与 S2、S5、S6、S7、S8 并行；不要与 S4（拆 `reseed.rs`）同时做。

### S2（P0）把调试证据移出 Git 风险区

**具体做法：**

1. `.gitignore` 增加 `artifacts/`、`/audit-*.txt`、`/gatecheck-*.json`、`/drafts-*.txt`、`/review*.txt`、`/formcheck-*.txt` 等本轮证据模式。
2. 保留 `artifacts/` 作为本地证据目录，但在 README/OPERATIONS 写明“禁止 `git add -A`；torrent 含 passkey”。
3. `git check-ignore` 验证 7 个含 passkey 的 torrent 全部被忽略。

**affected_files：** `.gitignore`、`README.md`、`doc/OPERATIONS.md`。

**验证方式：** `git status --short` 不再显示 `artifacts/` 和根目录证据；`git check-ignore artifacts/iloli-56513.torrent` 返回路径；`git grep -n 'passkey='` 不在已跟踪文件里出现。

**优先级：** P0。**并行性：** 可与 S1、S5、S6、S7、S8 并行。

### S3（P1）统一文档口径（代码改完后一次做）

**具体做法：**

- 把 `AGENTS.md` 的 `reseed.rs` 行数改成 11,857，并写“待拆分”。
- `SITES.md` 白名单数量统一成配置实际值 46；README 同步。
- `OPERATIONS.md` 删掉“手动 reseed 不写 auto-audit”的旧句，补手动审计字段和额度规则。
- `TESTING.md` 改成当前命令 `cargo test --workspace`，注明 GUI 已纳入；把覆盖率限制写成“GNU 不支持，需 MSVC + llvm-tools”。
- 新增 `doc/TEST-REPORT-2026-11-27.md`，记录 513 条测试、三条真实发种和 tracker 状态证据。
- `ARCHITECTURE.md` 把 progress=1 的表述改成“progress=1 且目标 tracker 状态 2 才算已在做种”。

**affected_files：** `AGENTS.md`、`README.md`、`doc/SITES.md`、`doc/OPERATIONS.md`、`doc/TESTING.md`、`doc/TEST-REPORT-2026-11-27.md`、`doc/ARCHITECTURE.md`、`doc/ROADMAP.md`。

**验证方式：** `rg -n '5k\+|45 站|不写 auto-audit|--exclude bsend-gui'` 不再命中过期句；配置计数脚本输出 46；文档中的链接与详情页一致。

**优先级：** P1。**并行性：** 与 S1/S4 串行（都改 `reseed`/状态语义）；与 S5、S6、S7、S8 可并行。

### S4（P1）拆分 `reseed.rs`（分阶段，不要一次大重写）

**具体做法：**

1. 先抽纯函数模块：`reseed/naming.rs`（标题/别名）、`reseed/dedupe.rs`（IMDb/搜索复核）、`reseed/fields.rs`（选档/字段映射）、`reseed/parse.rs`（回包/详情解析）。
2. 保留 `reseed.rs` 作为流程编排层，公开 API 不变；每抽一个模块先跑该模块测试和 `cargo test -p bsend`。
3. 把 `push_target_torrent` 的终态判定交给 S1 的共享函数，避免拆分时复制语义。
4. 最后再评估是否拆 `upload`/`pull` 子模块；一轮只做一层。

**affected_files：** `rust/crates/bsend/src/reseed.rs`、新增 `rust/crates/bsend/src/reseed/naming.rs`、`dedupe.rs`、`fields.rs`、`parse.rs`（以及 `lib.rs` 的 `mod` 声明）。

**验证方式：** 每个阶段 `cargo test -p bsend` 全绿；`cargo clippy` 无新 warning；`bsend reseed --site city13 --only Poirot` 的审阅输出与拆分前逐字一致（用 golden 文本比对）。

**优先级：** P1。**并行性：** 必须与 S1 串行；不要与 S5 同时改 `main.rs`。

### S5（P1）拆 CLI 并补命令级测试

**具体做法：**

1. 把 `main.rs` 的 `status/report/audit/reseed/gatecheck` 处理拆到 `cli/commands/*.rs`，`main.rs` 只保留参数定义和分发。
2. 增加 `tests/cli_smoke.rs`：用临时配置和 mock qB 跑 `status`、`audit --days 1`、`reseed --help`，断言退出码和关键输出；不访问真实站点。
3. 把 `run_reseed` 的审计写入抽成可注入的 trait/closure，测试 duplicate/blocked/写失败三种结果。

**affected_files：** `rust/crates/bsend-cli/src/main.rs`、新增 `rust/crates/bsend-cli/src/commands/`、新增 `rust/crates/bsend-cli/tests/cli_smoke.rs`、`rust/crates/bsend-cli/Cargo.toml`（dev-dependencies 如需要）。

**验证方式：** `cargo test -p bsend-cli --tests` 全绿；`bsend --help`、`bsend status`、`bsend audit` 输出与拆分前一致；CI 的 CLI job 通过。

**优先级：** P1。**并行性：** 与 S1/S4 可并行（文件不重叠）；与 S3 串行（文档最后改）。

### S6（P1）修测试临时目录隔离

**具体做法：**

1. `config.rs` 的 `write_temp` 改成按测试线程名生成目录：`bsend-cfg-{pid}-{test_name}`；保留同一测试内多次调用共用一个目录。
2. `write_temp_isolated` 复用同一 helper，避免两套命名规则。
3. 补一个并行测试：两个线程各自写 `boxsend.json`，断言互不覆盖。

**affected_files：** `rust/crates/bsend/src/config.rs`（测试模块）、可新增 `rust/crates/bsend/src/test_support.rs` 或在 `config.rs` 内私有 helper。

**验证方式：** `cargo test -p bsend config::tests -- --test-threads=8` 连续跑 10 次不失败；临时目录名唯一。

**优先级：** P1。**并行性：** 可与 S1、S5、S7、S8 并行。

### S7（P1）给 GUI 加可自动化的纯状态测试

**具体做法：**

1. 从 `bsend-gui/src/views/jobs.rs`、`report.rs`、`config.rs` 抽出不依赖 GPUI 窗口的纯函数（筛选、排序、状态文案、按钮可用性）。
2. 对这些纯函数补 10-15 个测试；真实窗口渲染仍可留手工验收。
3. CI 增加 `cargo test -p bsend-gui --lib`，不再把 GUI 从 workspace 测试排除。

**affected_files：** `rust/crates/bsend-gui/src/views/jobs.rs`、`report.rs`、`config.rs`、`rust/crates/bsend-gui/src/lib.rs`（导出测试辅助）、`.github/workflows/rust.yml`。

**验证方式：** `cargo test -p bsend-gui --lib` 全绿；GUI 至少 13 个测试；手工打开窗口确认状态文案未变。

**优先级：** P1。**并行性：** 与 S1/S5/S6 可并行；与 S8 共享 workflow 文件，需串行改 workflow。

### S8（P2）补 MSRV、覆盖率和脚本行为检查

**具体做法：**

1. CI 增加 `msrv` job：按 `Cargo.toml` 的 `rust-version` 安装固定工具链，跑 `cargo check --workspace`。
2. 覆盖率任务改到 MSVC + `llvm-tools-preview`，只对 `bsend` 库生成 `llvm-cov --summary-only`；GNU job 不再跑覆盖率。
3. `scripts` job 增加 `python -m py_compile` 之外的 `--help`/dry-run smoke；至少覆盖 `replay_audit.py` 和 `bsend` 包装脚本。
4. 把覆盖率结果写入 `doc/TEST-REPORT-2026-11-27.md`，低于阈值只警告不阻断，先建立趋势。

**affected_files：** `.github/workflows/rust.yml`、`.github/workflows/scripts.yml`、`rust/Cargo.toml`、`doc/TEST-REPORT-2026-11-27.md`。

**验证方式：** 新 workflow 在 PR 上跑绿；MSRV job 失败能指出具体依赖；覆盖率 job 产出 `llvm-cov` 摘要。

**优先级：** P2。**并行性：** 与 S1/S5/S6/S7 可并行（只改 workflow/manifest）；与 S3 串行（报告最后写）。

### S9（P2）把 qB 运行态写进运维手册并给出可执行处置

**具体做法：**

1. `OPERATIONS.md` 增加“qB 4 万条种子 / announce 队列”章节：如何看 `torrents/trackers` 的 status、如何判断是队列而不是配置错、重启后预期等待时间。
2. 给出两个处置方案：长期方案是给 BoxSend 单独一个轻量 qB 实例/分类；临时方案是清掉不需要的旧种或在非发种时段重启。
3. 不建议直接调高 `max_concurrent_http_announces` 作为常规操作，避免被站点限流；如要调，先记录基线并观察 24 小时。

**affected_files：** `doc/OPERATIONS.md`、`doc/ROADMAP.md`（7.5/7.9 遗留项）、可选 `Config/boxsend.example.json` 注释。

**验证方式：** 按文档在 qB 上复现一次 status=1 判定；按处置方案把 city13 目标种从 status=1 推到 status=2，站点显示 ≥1 做种者。

**优先级：** P2。**并行性：** 与 S1/S3 串行（都改运维/状态文档）；与 S5/S6/S7/S8 可并行。

## 四、优先级与并行执行表

| 顺序 | 条目 | 优先级 | 并行组 | 依赖/串行原因 |
|---|---|---|---|---|
| 1 | S1 tracker 真值 | P0 | G1 | 先修语义，后续文档和拆分都依赖它 |
| 2 | S2 证据忽略 | P0 | G1 | 文件不重叠，可立即做 |
| 3 | S6 测试隔离 | P1 | G1 | 只改 `config.rs` 测试 |
| 4 | S5 CLI 拆分 | P1 | G1 | 只改 `bsend-cli` |
| 5 | S7 GUI 测试 | P1 | G1 | 只改 GUI；workflow 改动让给 S8 |
| 6 | S8 CI/MSRV/覆盖率 | P2 | G1 | 只改 workflow/manifest |
| 7 | S4 拆 `reseed.rs` | P1 | G2 | 必须在 S1 后，避免复制终态逻辑 |
| 8 | S3 文档统一 | P1 | G3 | 等 S1/S4 的最终行为定稿后再改 |
| 9 | S9 qB 运维手册 | P2 | G3 | 与 S3 同改 `OPERATIONS.md`，串行 |

G1 内文件交集为空：S1（`qbittorrent/reseed/coverage/tests`）、S2（`.gitignore/README/OPERATIONS`）、S5（`bsend-cli`）、S6（`config.rs`）、S7（`bsend-gui`）、S8（`.github/workflows`）。S2 虽然改 `OPERATIONS.md`，但与 S9 不在同一组；S9 放 G3，避免与 S3 争用同一文档。

## 五、存量审计与路线图遗留

- `AUDIT-REPORT-2026-11-25.md` 的 16 条未完成项中，A1/A2/A3/A5/A6、T2/T3/T6/T7、R2/R3/R5、E1/E2/E7 仍可在本轮报告中找到对应问题或建议；T5 已由现有审核 e2e 覆盖，T9 仍缺属性测试。
- `ROADMAP.md` 4.4（旧 `miss` 停写）尚未执行；4.5（`reseed.rs` 拆分）对应 S4；5.2（测试临时目录/GUI/旧 `startup.log`）对应 S6/S7；7.5 剩余站点真实回包仍受网络和 qB 队列影响；7.9 手动审计已落地、iloli AC3 已修，haoxue/gtk 仍需站点窗口。
- 不建议把 4.5 拆成一次巨大提交；按 S4 分阶段，每阶段保持公开 API 和 golden 输出不变。

## 六、给执行 Agent 的开工顺序

1. 先做 S1，拿到 qB tracker status 的真实语义，再复跑 city13/iloli。
2. 做 S2，确保任何证据文件不会被误提交。
3. 并行做 S6/S5/S7/S8 中文件不重叠的部分。
4. S1 稳定后再做 S4；S3 和 S9 最后统一改文档。
5. 每步完成后更新 `doc/TEST-REPORT-2026-11-27.md`，不要只写“已修复”。

## 七、本次审计的证据入口

- 代码基线：`1c666a7`；审计时 `artifacts/` 和 48 个根目录证据文件未跟踪。2026-11-27 收尾已把 `/artifacts/` 加进 `.gitignore`，并用 `git check-ignore` 验证含 passkey 的 torrent 被忽略。
- 测试：`cargo test --workspace` 513 条全绿；`cargo fmt --check` 通过；`cargo clippy --workspace --all-targets -- -D warnings` 通过（只有 future-incompat 提示）。
- 真实发种：`artifacts/pthome-zen-upload-20261127.log`、`artifacts/city13-poirot-upload3-20261127.log`、`artifacts/iloli-56513.torrent`。
- 站点页面：`artifacts/13city_org-details.txt` 显示 `0 个做种者 | 2 个下载者`；iloli 详情页 `https://mua.xloli.cc/details.php?id=56513`。
- qB：`http://127.0.0.1:9091`；city13 详情 `https://13city.org/details.php?id=88464` 对应的 qB 目标种 progress=1、tracker 先 status=1、恢复诊断配置后 status=4；pthome 目标种 hash `8085a487...` 为 status=2、iloli 目标种 hash `5d8dff96...` 本轮见过 status=2。
- 覆盖率：`artifacts/coverage-20261127.log`（GNU 失败）、`artifacts/coverage-msvc-rustlld-20261127b.log`（MSVC 方案，待完成）。
