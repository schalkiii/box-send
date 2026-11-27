# 项目审计（2026-11-27 r3）

> 审计范围：`C:\Users\Schal\WorkBuddy\2026-11-17-20-52-34` 当前 `master` 分支。
> 代码基线：`a5cd535`（生产路径 `unwrap/expect` 清理与文档同步已提交）。审计过程只读，不改代码。
> 本报告取代 `AUDIT-REPORT-2026-11-27-r2.md`；r2 保留为上一轮证据，前部快照已过时。

## 一、现状

- **规模**：Rust 57 个 `.rs` 文件、43,318 行。按职责拆开：生产代码 26,915 行，内联测试 11,562 行，`tests/` 目录 4,841 行；测试代码占 37.9%。项目 Markdown 33 个。
- **结构**：`bsend`（核心引擎）、`bsend-cli`（命令行）、`bsend-gui`（GPUI 桌面界面）三个 crate。核心按“配置 / 站点适配 / 审阅上传 / 覆盖记录 / 自动编排 / qB 接口”分层；`reseed` 与 `sites/nexusphp` 已完成按职责拆分，`coverage.rs`、`commands.rs` 仍是单文件大模块。
- **质量基线**：`cargo test --workspace` 535 条全绿（核心 457、`coverage_e2e` 35、`other_version` 2、`skip_keywords` 2、CLI 23、`cli_smoke` 3、GUI 13）。`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- **覆盖率**：CI 行覆盖率 16,842/21,087 = **79.87%**；`coverage` job 用 `--fail-under-lines 77` 设了 77% 硬门槛。本地 Windows GNU 缺 profiler runtime，本地不重复跑 `cargo llvm-cov`。
- **CI**：`0f1ff35` 的 run #113 四个 job 全部 success；`a5cd535` 的 run #114 也已四个 job 全部 success（2 分 35 秒）。
- **真实链路**：city13 [88467](https://13city.org/details.php?id=88467) 已发布、挂回、tracker `status=2`，复查上传 227.16 MB、2 个下载者；hdvideo [79168](https://hdvideo.top/details.php?id=79168) 已发布、挂回，主 announce `status=2`，`tracker.hdvideo.top` `status=4`（stream truncated），0 peers、0 上传；pthome 285040 回“该种子已存在！”，未重复挂种。两条站点页仍标“未审”。

## 二、七个维度

### 1. 文档一致性

- 当前基线文档已同步到 535 条、79.87%、CI 77% 门槛；`README.md` 原先“旧报告不再单独保留”与 `doc/AUDIT-REPORT-*`、`doc/TEST-REPORT-*` 实际存在矛盾，已改成“报告保留作证据、结论以最新一份为准”。
- `AGENTS.md` 已补 `sites/nexusphp/` 子模块，删掉“历史报告不再保留”的死引用；`doc/ROADMAP.md` 第 6 节旧的“两个 Agent 改 `reseed.rs`”风险已改成当前拆分后的写集。
- `doc/SITES.md` 的 hdvideo 行已从 `assisted` 纠正为 `reseeded`，tracker 状态更新为 `status=4`；city13 复查上传量 227.16 MB。
- **剩余问题**：`AUDIT-REPORT-2026-11-27-r2.md` 前部仍是 53 文件 / 40,770 行 / 533 条 / 覆盖率无阈值的旧快照；该报告已由本报告取代，但文件本身未标“历史版”。

### 2. 文档缺漏

- 常驻文档覆盖了架构、运维、站点、测试、路线图和变更记录；`README.md` 已补审计/测试报告入口。
- **缺口**：没有一份“真实站点验收清单”把 city13、hdvideo、pthome 的发布、挂回、tracker 回包、站点审核状态固化成可重复检查的步骤；现在证据散在 `SITES.md`、`TEST-REPORT` 和 `artifacts/`。建议补 `scripts/live-smoke.ps1` 并在 `OPERATIONS.md` 引用，见 S2。
- **缺口**：qB 接口的 50,000 条上限没有写进运维文档；一旦种子数接近或超过上限，排障人员无法从文档判断“为什么少了一批种子”。

### 3. 架构优化点

- `reseed.rs` 仍有 3,740 行（含内联测试），但表单、预审、上传、发后写回已拆走；继续硬拆的收益低于回归成本，暂不作为本轮建议。
- `coverage.rs` 2,294 行（含测试）同时承担索引结构、分页预算、worker 调度、tracker 复核和落盘；`commands.rs` 2,100 行承担全部 CLI 命令。两者都能再拆，但当前测试覆盖足够，优先级低于可靠性问题。
- **真正的架构风险**在 qB 列表读取：`qbittorrent.rs` 三处直接用 `/api/v2/torrents/info?limit=50000`，没有分页；现役 42,473 条已到上限的 85%。超过 50,000 条后列表会被截断，覆盖判断可能把“站上已有”看成“站上没有”，进而重复发种。见 S1。

### 4. 代码写法

- 本轮已清掉生产路径 6 处可直接避免的 `unwrap/expect`：`coverage.rs` 的锁与分页裁剪、`reseed.rs` 的 UTF-8 取字符和标签栈、NexusPHP 详情链接边界；行为不变。
- 剩余生产路径 `unwrap/expect` 10 处，均有前置不变量：`index.rs:385`、`report.rs:172`、`reseed/upload.rs:507`、`sites/nexusphp/detail.rs` 三处固定选择器、`sites/nexusphp/upload.rs:40`、`commands.rs:521`、`bsend-gui/src/main.rs:361` 与 `:673`。其中 GUI 的 `rx.lock().unwrap()` 仍是 poison 风险，其他多为“上面已过滤/固定选择器”的断言。
- 没有发现新的死代码或明显重复逻辑；`clippy -D warnings` 为零告警。

### 5. 测试覆盖率

- 535 条测试、行覆盖率 79.87%，高于 77% 门槛；核心库 457 条覆盖解析、判重、表单、覆盖状态和 qB 假服务端。
- **缺口一**：qB 超过 50,000 条的分页/截断行为没有用例；现有假服务端只模拟小列表。
- **缺口二**：GUI 13 条是纯状态测试，按钮可用性、真实窗口渲染、配置路径选择仍靠手工。
- **缺口三**：真实站点链路只有 city13、hdvideo 两条人工证据；qingwa、dragonhd、ptzone 等仍被外部链路挡住，不能算代码已覆盖。

### 6. 测试严谨性

- `coverage_e2e` 35 条使用本地假 qB / 假 HTTP，断言覆盖状态、`verifyHash`、tracker 文案和新发种/补挂来源，属于“状态 + 落盘”双重比对，不是只跑通。
- **不足**：真实站点验收没有自动化，靠人工打开详情页和查 qB tracker；同一结论没有脚本化的“详情页 + tracker + 上传量”三元组断言。S2 的只读巡检脚本可以补这一层，但不能替代真发。
- **不足**：GUI 用例只断言文案和配色，不断言点击后的状态迁移；这部分风险低，暂不优先。

### 7. 工程规范

- `.gitignore` 已挡住 `artifacts/`、`rust/target/`、根目录一次性证据；`git status` 干净（本报告写完后待提交）。
- `artifacts/` 当前 281 个文件、28.42 MB；其中 8 个 `.torrent` 含站点 `passkey`，虽然被 ignore，打包/备份/误拷贝仍会泄露。
- 分支 `main`、`rust/coverage` 已完全合入 `master`，但远端正地保留；worktree 只有当前一个。
- GitHub push 在 schannel 下会被 reset，现用 `-c http.sslBackend=openssl` 绕过；这个写法没有写进 `AGENTS.md` 或脚本，换机器会重复踩坑。

## 三、问题清单

| ID | 位置 | 问题 | 严重程度 | 影响范围 |
|---|---|---|---|---|
| A1 | `doc/AUDIT-REPORT-2026-11-27-r2.md` | 前部规模/测试/覆盖率/工作区快照已过时，未标历史版 | 低 | 接手者按旧数字判断现状 |
| A2 | `README.md`、`AGENTS.md` | 原先称“旧报告不再保留”，与磁盘上的审计/测试报告矛盾（已修） | 低 | 文档检索与交接 |
| A3 | `rust/crates/bsend/src/qbittorrent.rs` | `torrents/info?limit=50000` 硬上限，无分页；现役 42,473 条 | 高 | 超过 50,000 条后列表截断，覆盖误判、重复发种 |
| A4 | `rust/crates/bsend/tests/coverage_e2e.rs`、`doc/OPERATIONS.md` | 真站验收只有人工证据，无只读巡检脚本 | 中 | 回归时无法快速复核站点状态 |
| A5 | `rust/crates/bsend/src/coverage.rs`、`index.rs` | `coverage.json` 单文件 23 MB，整文件序列化落盘 | 中 | 启动/保存变慢，手工合并易冲突 |
| A6 | `index.rs`、`report.rs`、`reseed/upload.rs`、`detail.rs`、`sites/nexusphp/upload.rs`、`commands.rs`、`bsend-gui/src/main.rs` | 生产路径仍有 10 处 `unwrap/expect`，GUI 锁未做 poison 恢复 | 低 | 极端 panic 会中断单轮任务 |
| A7 | `artifacts/` | 281 文件 / 28.42 MB，8 个 torrent 含 passkey | 中 | 备份、打包、误拷贝泄露站点凭据 |
| A8 | Git 分支 | `main`、`rust/coverage` 已合入未删除 | 低 | 分支列表噪音 |
| A9 | `bsend-gui/src/main.rs` | 13 条 GUI 测试只覆盖纯状态，不覆盖窗口交互 | 低 | 界面回归靠手工 |
| A10 | `AGENTS.md`、推送流程 | schannel TLS 被 reset 的绕过写法未固化 | 低 | 换机器重复排障 |
| A11 | `doc/SITES.md`、运行态 | city13 88467 仍 `pending_review`，hdvideo 79168 仍“未审”、0 上传 | 中 | 业务状态待站点审核，不是代码 bug |
| A12 | `reseed.rs` | 3,740 行（含测试）仍是核心大文件 | 低 | 维护成本，暂不再拆 |

## 四、改进建议

### S1（P1，串行）给 qB 种子列表加分页/上限保护

- **具体做法**：在 `qbittorrent.rs` 抽一个“取全量列表”的函数：先按现有 `limit=50000` 取第一页；返回条数正好等于上限时，继续用 `offset` 取下一页，直到返回空或不足一页；三处调用统一走这个函数。超过 50,000 条时打一条明确日志，不再只警告“可能不完整”。
- **修改涉及文件**：`rust/crates/bsend/src/qbittorrent.rs`（`torrents()`、`tracker_hosts()`、`tracker_map` 三处调用及内联测试）；必要时 `rust/crates/bsend/src/models.rs`（若分页需要新的返回类型）。
- **验证方式**：假 qB 服务端模拟 50,001 条，断言分两页取回且总数正确；再跑 `cargo test -p bsend --lib qbittorrent::`、`cargo test --workspace`、fmt、clippy。真实环境用 `bsend status` 对比索引成员数与 qB 列表条数。
- **并行性**：可与 S2、S4 并行；`affected_files` 与它们无交集。

### S2（P2，可并行）补只读真站巡检脚本

- **具体做法**：新增 `scripts/live-smoke.ps1`，只读执行：`bsend status`、`bsend report`、`bsend followup --dry-run`（若支持）或按站点读取详情页；输出每条种子的详情页、tracker 状态、上传量、审核标记，不发布、不挂种、不改覆盖。默认只查 city13、hdvideo、pthome 三条，参数可扩展。
- **修改涉及文件**：新增 `scripts/live-smoke.ps1`；`doc/OPERATIONS.md` 增加“真站只读巡检”一节；`README.md` 文档表可选加一行。
- **验证方式**：在真实配置上跑一次，输出与 `bsend status` / qB tracker 手工查询一致；脚本退出码在站点不可达时为非零，但不写任何状态文件。
- **并行性**：可与 S1 并行；不碰 Rust 源码。

### S3（P2，串行）评估 `coverage.json` 分片或增量落盘

- **具体做法**：先做基准：记录当前 23 MB 索引的加载、整文件保存耗时和峰值内存；若保存占一轮总时长超过 5%，按站点或 hash 前缀分片，或只写脏站点文件。不要在未测基准前直接改存储格式。
- **修改涉及文件**：`rust/crates/bsend/src/coverage.rs`（`CoverageIndex::load/save`）、`rust/crates/bsend/src/index.rs`、`rust/crates/bsend/src/config.rs`（若新增路径配置）。
- **验证方式**：同一份 23 MB 数据做前后基准对比；跑 `cargo test -p bsend --lib coverage::`、全量 535 条；额外用例覆盖“分片缺失/旧单文件迁移”。
- **并行性**：与 S1/S2 文件无交集，可并行；但与任何改 `coverage.rs` 的任务串行。

### S4（P3，可并行）文档收口：标 r2 历史版、同步当前数字

- **具体做法**：在 `AUDIT-REPORT-2026-11-27-r2.md` 顶部加“历史版，现役结论见 r3”；把 r2 前部旧快照保留但标注日期；确认 `README.md`、`AGENTS.md`、`ROADMAP.md`、`TESTING.md` 不再出现“533/40,770/53 文件/覆盖率无阈值”的现役表述。
- **修改涉及文件**：`doc/AUDIT-REPORT-2026-11-27-r2.md`、`README.md`、`AGENTS.md`、`doc/ROADMAP.md`、`doc/TESTING.md`。
- **验证方式**：全仓 `rg -n "533|40,770|53 个|覆盖率 job 无阈值"`，只允许出现在带日期的历史段落；人工通读 r3 与 r2 的替代关系。
- **并行性**：可与 S1/S2 并行；只改文档。

### S5（P3，串行）清掉剩余生产路径 `unwrap/expect`

- **具体做法**：逐处判断不变量：`index.rs:385` 改 `get_mut` 的 `let-else`；`report.rs:172` 已有 `expect` 文案，保留；`reseed/upload.rs:507` 改 `pop()` 的 `let-else`；`sites/nexusphp/upload.rs:40` 的 `last` 改 `ok_or_else`；`commands.rs:521` 的 `draft` 用 `match`；GUI `main.rs:361` 改用局部 `lock_unpoisoned` 或 `match`，`:673` 保留带文案的 `expect`。一次只改一类，逐点跑测试。
- **修改涉及文件**：`rust/crates/bsend/src/index.rs`、`report.rs`、`reseed/upload.rs`、`sites/nexusphp/upload.rs`、`bsend-cli/src/commands.rs`、`bsend-gui/src/main.rs`。
- **验证方式**：每改一处跑对应模块测试；最后全量 535 条、fmt、clippy。
- **并行性**：与 S1/S3 有代码交集风险，必须串行。

### S6（P3，串行，需用户确认）清场 `artifacts/` 与根目录一次性证据

- **具体做法**：先只读列出，不删除；优先删 `artifacts/coverage-before-ghiblies.json`（22.9 MB 旧备份）、三个 `reseed.rs.bak-before-*`、8 个含 passkey 的 `.torrent`、根目录 47 个 ignored 一次性 txt/html（另有 `.workbuddy/`、`rust/target/` 不删）。保留 `artifacts/` 中最近的门禁日志和真实站点证据截图。
- **修改涉及文件（删除候选，未确认前不动）**：`artifacts/coverage-before-ghiblies.json`、`artifacts/reseed.rs.bak-before-form-split`、`artifacts/reseed.rs.bak-before-audit-split`、`artifacts/reseed.rs.bak-before-upload-post-split`、`artifacts/iloli-56513.torrent`、`artifacts/railgun-58652.torrent`、`artifacts/railgun-59010.torrent`、`artifacts/u2-66255.torrent`、`artifacts/gtk-evidence-20261127/gtk-7696.torrent`、`artifacts/gtk-evidence-20261127/local-poirot.torrent`、`artifacts/staging-xingtan-20261127/site-456304.torrent`、`artifacts/staging-xingtan-20261127/xingtan-upload-hu8a.torrent`；根目录 47 个一次性 ignored 文件见 `artifacts/neat-inventory-20261127-r4.txt`。
- **验证方式**：删除前 `git status --ignored` 快照；删除后重跑 `git status`、`bsend status`，确认没有动到唯一证据。
- **并行性**：破坏性操作，必须等用户看完汇报明确确认，且与其它写任务串行。

### S7（P4，可并行）删除已合并分支

- **具体做法**：确认 `main`、`rust/coverage` 的 tip 已在 `master` 历史中，删除本地和远端分支；保留 tag/提交。
- **修改涉及文件**：无文件；只动 Git refs（`refs/heads/main`、`refs/heads/rust/coverage`、`refs/remotes/origin/main`、`refs/remotes/origin/rust/coverage`）。
- **验证方式**：`git branch --merged master` 为空；`git log master..main`、`git log master..rust/coverage` 为空。
- **并行性**：可与文档/代码任务并行；但需用户确认后再删远端。

## 五、优先级与并行

| 顺序 | 建议 | 优先级 | 并行性 | 依赖 |
|---|---|---|---|---|
| 1 | S1 qB 分页 | P1 | 可并行（与 S2/S4） | 无 |
| 2 | S2 真站只读巡检 | P2 | 可并行（与 S1/S4） | 无 |
| 3 | S3 索引落盘基准 | P2 | 可并行（与 S1/S2，但不碰 `coverage.rs` 的其他任务） | 先测基准 |
| 4 | S4 文档收口 | P3 | 可并行（与 S1/S2） | 无 |
| 5 | S5 剩余 unwrap | P3 | 串行 | 与 S1/S3 代码写集互斥 |
| 6 | S6 清场 | P3 | 串行、需确认 | 用户明确确认 |
| 7 | S7 删分支 | P4 | 可并行 | 用户确认 |

## 六、本轮已关闭

- S6（r2 的 NexusPHP 拆分）已完成：根文件 2,886 → 1,834 行，子模块 `text/search/detail/upload` 已提交 `8087341`。
- r2 的 S2（锁 poison）、S4（覆盖率门槛）、S5（CLI 空路径测试）均已落地；本轮又补了 `coverage.rs` 的锁与分页裁剪。
- 新发种收尾语义已修：`collect_verifying` 区分“本站新发种”和“补挂种子”，hdvideo 79168 的旧格人工纠正为 `reseeded`；全量 535 条。
- 文档当前数字已同步：`AGENTS.md` 535、`TESTING.md` 79.87%/77%、`TEST-REPORT` 535、`ARCHITECTURE` 535、`SITES` hdvideo `reseeded`。

## 七、遗留与待确认

- **待用户确认**：S6 清场（`artifacts/` 与根目录 ignored 证据）、S7 删除已合并分支。未确认前一个都没删。
- **待站点侧**：city13 88467、hdvideo 79168 仍“未审”；hdvideo `tracker.hdvideo.top` `status=4`（stream truncated），0 上传；city13 已上传 227.16 MB，但覆盖格仍是 `pending_review`。
- **外部阻塞**：qingwa 真实回包、dragonhd/ptzone 真站复测仍被站点链路挡住，不是代码回包识别 bug。
- **CI 已确认**：`a5cd535` 的 run #114 四 job success；本轮文档改动只动 Markdown，提交后的 CI 仍按同一条 `rust.yml` 跑。
