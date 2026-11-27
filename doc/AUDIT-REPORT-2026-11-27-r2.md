# 项目审计（2026-11-27 重构后复审）

本报告是 `AUDIT-REPORT-2026-11-27.md` 的复审版。旧报告记录的是大拆分之前的状态，行数和 S4/S5 结论已经过时；本报告以当前工作区为准。审计只做静态检查、构建/测试和运行态取证，不改代码。

## 一、现状

- 规模：Rust workspace 53 个 `.rs` 文件、40,770 行；`bsend` 30,783 行，`bsend-cli` 2,368 行，`bsend-gui` 3,210 行。
- 主要模块：`reseed.rs` 3,585 行（主流程编排）；`reseed/form.rs` 2,485；`reseed/tests.rs` 3,433；`reseed/audit.rs` 753；`reseed/upload.rs` 615；`reseed/post.rs` 658；`sites/nexusphp.rs` 2,941；`coverage.rs` 2,106；`bsend-cli/commands.rs` 1,998；`bsend-cli/main.rs` 129。
- 测试：全量 **528 条**全绿（核心库 455、coverage_e2e 33、other_version 2、skip_keywords 2、CLI 20、cli_smoke 3、GUI 13）。`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- 构建：`cargo build --release -p bsend-cli` 成功；`bsend status` 用真实配置输出 3,388 段内容、42,465 条成员种子；`bsend audit --days 1` 可读。
- 工程：`rust/Cargo.toml` 声明 `rust-version = "1.88"`；本地 GNU 1.88 `cargo check -p bsend` 通过。GitHub Actions 已有 Linux/Windows 测试矩阵、`MSRV 1.88` job、`coverage` job（Ubuntu + `cargo-llvm-cov`，上传 `lcov.info`）。
- 真实链路：2026-11-27 晚用重构后的 release 二进制实发 city13 `88467`、hdvideo `79168`，两条都完成上传 → 站内定位 → 下载站点种子 → qB 校验 100%；tracker 复查仍 `status=1`（42,469 条种子的 announce 队列 + 站点待审），只能记“已发布、已挂回、未确认做种”。pthome《Black》上传回包“该种子已存在！”，重复分支走通，没有重复挂种。证据见 `doc/SITES.md` 2.11。

## 二、七个维度

### 1. 文档一致性

已同步：`AGENTS.md` 的模块行数、`ROADMAP.md` 4.4/4.5/4.6、`ARCHITECTURE.md` 的代码位置表、`SITES.md` 2.10/2.11、`CHANGELOG.md` 的拆分/MSRV/实发记录、`TESTING.md` 与 `TEST-REPORT-2026-11-27.md` 的 528/MSRV/覆盖率说明。

仍不一致：

- 旧 `doc/AUDIT-REPORT-2026-11-27.md` 仍写 `reseed.rs` 11,889 行、S4/S5“第一批”、S8 未做；本报告已取代它，但旧文没有标注“历史版”，容易被后来的人当成现状。
- `doc/ROADMAP.md` 6 节的风险条目还写“两个 Agent 同时改 `reseed.rs` 会互相覆盖”，拆分后主要风险已转到 `reseed/form.rs` 和 `sites/nexusphp.rs`，这条描述过时。

### 2. 文档缺漏

- `OPERATIONS.md` 已写 qB 4 万条种子的 announce 队列处置，但没有写“新种 tracker 一直 `status=1` 时，先看队列还是先看站点审核”的判断顺序；这次 city13/hdvideo 两条都卡在 `status=1`，现场靠人记忆判断。
- `TESTING.md` 已写覆盖率 job，但没有写“CI 的 `lcov.info` 怎么读、当前没有阈值”的基线说明；后来的人容易把“有 artifact”当成“覆盖率达标”。

### 3. 架构优化点

大拆分已完成，`reseed.rs` 从 11,889 降到 3,585，CLI `main.rs` 从 2,380 降到 129。当前最大的三个单文件是：

- `sites/nexusphp.rs` 2,941 行：详情页解析、表单解析、上传、审核标记、别名归一都在一个文件，站点改版时容易互相碰。
- `reseed/form.rs` 2,485 行：字段构造、分类/图片/区域匹配、站点规则都在一个文件，规则继续长会回到“大文件”问题。
- `bsend-cli/commands.rs` 1,998 行：所有命令处理和格式化辅助集中，命令继续加会重复。

这三处是下一轮架构候选，不是本轮必须拆；当前拆分已把最痛的 `reseed.rs` 主流程拆开，继续硬拆的回归成本高于收益。

### 4. 代码写法

- **生产路径的互斥锁 `unwrap`**：`coverage.rs` 14 处、`throttle.rs` 8 处、`qbittorrent.rs` 3 处，都是 `mutex.lock().unwrap()`。任意线程持锁 panic 后，后续所有请求会继续 panic，整轮扫描/发种直接中断。计数器、队列、缓存这三类数据没有跨字段不变量，适合用“忽略 poison、取回内部值”的写法。
- **根目录 ignore 过宽**：`.gitignore` 用 `/*.txt`、`/*.html`、`/bsend-batch-*.md`、`/gatecheck-*.json`、`/rust/*.txt` 挡证据文件，未来根目录放正式说明或报告会被静默忽略。应只挡明确的证据文件名或统一放 `artifacts/`。
- `reseed.rs` 仍有 1 处 `name[i..].chars().next().unwrap()`，`upload.rs` 有 1 处 `found.pop().unwrap()`；两处都在已判非空/已判字符边界后执行，当前不是 bug，但属于可读性风险。
- `nexusphp.rs` 有 4 处生产 unwrap（`href[..i].chars().next_back().unwrap()`、`Selector::parse("a").unwrap()` 等），`Selector::parse` 对固定字符串是常量，失败只可能是选择器写错；保留可接受，建议加注释说明。

### 5. 测试覆盖率

- CI 已加覆盖率 job，但还没设阈值；本地 Windows GNU 缺 profiler runtime，不能产生行覆盖率数字。当前只有“528 条测试”这个数量基线，没有分支/行覆盖率基线。
- 核心库测试集中在 `reseed/tests.rs`（168 条）和 `coverage_e2e`（33 条）；`sites/nexusphp.rs` 只有 36 条，详情页/审核页面变体仍是缺口。
- CLI 20 条单测 + 3 条冒烟；`cmd_status`、`cmd_report`、`cmd_audit`、`cmd_autopost`、`cmd_pull`、`cmd_followup` 的真实配置加载和网络路径没有自动化。
- GUI 13 条纯状态测试；按钮可用性、真实窗口渲染仍靠手工。

### 6. 测试严谨性

- `coverage_e2e` 的 qB mock 已补 `status:2`，tracker 终态断言是自动比对，不是“跑通即通过”。
- `other_version`、`skip_keywords` 有独立断言；CLI 冒烟检查退出码和错误文案；GUI 测试只断言纯状态，不依赖窗口，边界清楚。
- 缺口：没有模拟“42,469 条种子导致 announce 队列堵塞”的时序；真实发种的 `status=1` 只能靠人复查。`gatecheck` 的通用样本仍会出现“0 张图可用”，不能代表真实候选；qingwa/dragonhd/ptzone 的真实候选复测被源站超时/连接重置挡住，属于外部链路，不是断言缺失。

### 7. 工程规范

- CI 三块（测试矩阵、MSRV、覆盖率）已齐；本地和 CI 的工具链差异已写进 `TESTING.md`。
- 工作区当前还有未提交的重构、`Cargo.toml`/`Cargo.lock`、coverage miss 改动，`origin/master` 仍是 `e86a616`；在提交前不能说“远端已验证”。
- `artifacts/` 186 个文件、36.6 MB，其中 7 个 `.torrent` 含 `passkey=`；已被 `.gitignore` 挡住，但仍在磁盘上，打包/备份/误拷贝会泄露。Neat Freak 只列了候选，未删除，等用户确认。
- `Cargo.lock` 为配合 1.82 尝试把 `rust-embed` 从 8.14 降到 8.7，并把 lock 改成 `version = 3`；现在 MSRV 已定 1.88，这个降级没有注释说明，后续 `cargo update` 可能无意升回去。

## 三、问题清单

| ID | 位置 | 问题 | 严重度 | 影响 |
|---|---|---|---|---|
| A1 | `doc/AUDIT-REPORT-2026-11-27.md` | 旧报告仍写 11,889 行、S4/S5 第一批、S8 未做 | 中 | 后来的人按旧报告开工，会重复做已完成的拆分 |
| A2 | `doc/ROADMAP.md` 第 6 节 | 风险条目仍说“两个 Agent 改 reseed.rs 会互相覆盖”，与拆分后结构不符 | 低 | 误导并行分工 |
| B1 | `doc/OPERATIONS.md` | 没写“新种 tracker status=1 时先查 announce 队列还是先查站点审核” | 中 | 现场容易把队列拥塞误判成发种失败 |
| C1 | `sites/nexusphp.rs` | 2,941 行，详情/表单/上传/审核混在一起 | 中 | 站点改版时改动面大，回归成本高 |
| C2 | `reseed/form.rs` | 2,485 行，字段构造和站点规则继续增长 | 低 | 下一轮规则增加会回到大文件 |
| C3 | `bsend-cli/commands.rs` | 1,998 行，所有命令处理集中 | 低 | 新命令继续加会重复格式化逻辑 |
| D1 | `coverage.rs`、`throttle.rs`、`qbittorrent.rs` | 生产路径 25 处 `mutex.lock().unwrap()` | 高 | 任一线程持锁 panic 后，整轮扫描/发种会连锁 panic；**本轮已修** |
| D2 | `.gitignore` | 根目录 `/*.txt`、`/*.html` 等通配过宽 | 中 | 未来正式根目录文档会被静默忽略，证据文件也可能漏挡；**本轮已收窄** |
| D3 | `reseed.rs`、`upload.rs`、`nexusphp.rs` | 少量“已判非空后 unwrap”和固定选择器 unwrap | 低 | 当前不是 bug，但读代码时难判断安全性 |
| E1 | `.github/workflows/rust.yml` | 覆盖率 job 只有 artifact，没有阈值/基线 | 中 | 覆盖率下降不会让 CI 变红 |
| E2 | `bsend-cli/commands.rs` | 命令处理只有 8 条单测，真实配置/网络路径未覆盖 | 中 | `status`/`audit`/`pull` 回归只能靠手工 |
| E3 | `sites/nexusphp.rs` | 36 条测试，详情页/审核页面变体覆盖不足 | 中 | 站点改版时容易漏解析分支 |
| F1 | 工作区 | 重构和 Cargo 改动未提交，远端 CI 未跑 | 高 | 不能说“已完成”，也无法在远端复现 |
| F2 | `artifacts/` | 186 文件 / 36.6 MB，7 个 torrent 含 passkey | 高 | 打包、备份、误拷贝会泄露站点凭据 |
| F3 | `rust/Cargo.lock` | `rust-embed` 8.14→8.7、lock v3 无说明 | 低 | 后续升级可能无意改变依赖基线 |
| G1 | `doc/ROADMAP.md` 5.2 | “旧 startup.log 可以删”仍留着，但文件已不存在 | 低 | 清场清单有噪音 |

## 四、改进建议

### S1（P0，串行，已执行）先提交并跑远端 CI

- 做法：把本轮拆分、`Cargo.toml`/`Cargo.lock`、coverage miss、文档改动分组提交；推送后等 `test`（Linux/Windows）、`MSRV 1.88`、`coverage` 三个 job 全绿，再写“已通过”。
- 文件：`rust/crates/bsend/src/reseed.rs`、`reseed/{form,audit,upload,post,tests}.rs`、`bsend-cli/src/{main,commands}.rs`、`coverage.rs`、`rust/Cargo.toml`、`rust/Cargo.lock`、`.github/workflows/rust.yml`、`doc/*`、`AGENTS.md`、`CHANGELOG.md`。
- 验证：`git status` 干净；GitHub Actions 三个 job 成功；`git rev-parse HEAD` 与 `origin/master` 一致。
- 执行结果：提交 `025b712` 已推送，run [45993230747](https://github.com/schalkiii/box-send/actions/runs/45993230747) 的覆盖率和 MSRV、Ubuntu、Windows 四个 job 全部 success。
- 并行性：串行，提交是所有其它改动的收口。

### S2（P1，可与 S1 外的文档工作并行）修生产路径锁 poison

- 做法：在 `lib.rs` 增加 `lock_unpoisoned<T>(&Mutex<T>) -> MutexGuard<'_, T>`，用 `unwrap_or_else(|e| e.into_inner())` 取回内部值；把 `coverage.rs`、`throttle.rs`、`qbittorrent.rs` 生产路径的 `.lock().unwrap()` 换成该函数；加一条“线程 panic 后还能继续加计数”的单测。
- 文件：`rust/crates/bsend/src/lib.rs`、`coverage.rs`、`throttle.rs`、`qbittorrent.rs`。
- 验证：新增 poison 用例通过；`cargo test --workspace` 528+ 全绿；`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- 并行性：与 S4、S5 文件不重叠，可并行；不要和 `reseed.rs` 拆分同时改。

### S3（P1，串行，已执行）收窄 `.gitignore`

- 做法：保留 `/artifacts/`；把根目录 `/*.txt`、`/*.html`、`/bsend-batch-*.md`、`/gatecheck-*.json`、`/rust/*.txt` 换成明确的证据文件名或统一移进 `artifacts/`，不要用会吃掉正式文档的通配。
- 文件：`.gitignore`。
- 验证：`git check-ignore -v artifacts/<含 passkey 的 torrent>` 仍命中；在根目录临时建 `README-test.md`、`notes.txt` 确认不被忽略；`git status --ignored` 里证据文件仍在。
- 并行性：与 S2、S4 文件不重叠，可并行。

### S4（P2，串行）给覆盖率加基线门槛

- 做法：等 CI 第一次 `lcov.info` 产出后，记录行覆盖率基线；在 `coverage` job 加 `--fail-under-lines <基线-2>` 或先只生成 summary 并人工对比，避免一上来就红。
- 文件：`.github/workflows/rust.yml`。
- 验证：本地用 CI artifact 读出的百分比与 job 日志一致；故意降一行覆盖不会误报。
- 并行性：与 S2、S3 文件不重叠，可并行。

### S5（P2，串行）补 CLI 命令处理测试

- 做法：用临时目录生成最小 `boxsend.json`，覆盖 `cmd_status` 的空索引、`cmd_audit` 的空审计、`cmd_report` 的缺配置三条路径；不碰网络和 qB。
- 文件：`rust/crates/bsend-cli/src/commands.rs`（测试模块）、`rust/crates/bsend-cli/tests/cli_smoke.rs`。
- 验证：新增用例在 `cargo test -p bsend-cli` 通过；不改变现有 20+3 条。
- 并行性：与 S2/S3/S4 文件不重叠；但和 S1 提交串行。

### S6（P2，串行）给 `nexusphp.rs` 做下一轮拆分

- 做法：按 `sites/nexusphp/{mod,detail,search,upload}.rs` 拆，`mod.rs` 只留站点定义和公共类型；只移动代码，不改行为；每拆一个文件跑 `cargo test -p bsend --lib sites::`。
- 文件：`rust/crates/bsend/src/sites/nexusphp.rs` + 新文件。
- 验证：`cargo test -p bsend --lib` 455 条全绿；`cargo clippy` 无新警告；`git diff` 只有移动。
- 并行性：串行，单独分支；不要与 S2 同改 `nexusphp.rs`。

### S7（P3，串行）清掉过时工程说明

- 做法：`ROADMAP` 5.2 的 `startup.log` 删除项标注“已不存在”；旧 `AUDIT-REPORT-2026-11-27.md` 顶部加“历史版，现状见 r2”；`Cargo.lock` 的 rust-embed 降级要么恢复 8.14，要么在 `Cargo.toml`/文档写一句“为兼容旧工具链临时降级，MSRV 1.88 后可评估恢复”。
- 文件：`doc/ROADMAP.md`、`doc/AUDIT-REPORT-2026-11-27.md`、`rust/Cargo.toml`、`rust/Cargo.lock`。
- 验证：文档搜索不再出现 11,889/第一批；`cargo check` 仍通过。
- 并行性：与 S2/S3/S4 文件不重叠，可并行；但改 lock 要和 S1 提交串行。

### S8（P3，串行，需用户确认）清场 `artifacts/`

- 做法：按 `neat-freak` 候选清单，把 277 个文件 / 38.34 MB 中的含 passkey torrent 和一次性日志移出仓库或删除；删除前等用户明确确认。
- 文件：`artifacts/`、根目录被忽略证据文件。
- 验证：`git status --ignored` 不再显示敏感 torrent；仓库体积下降；保留的正式证据仍有索引。
- 并行性：串行，破坏性动作，必须用户确认。

## 五、优先级与并行

- P0：S1（提交 + 远端 CI）。
- P1：S2（锁 poison）、S3（ignore 收窄）。
- P2：S4（覆盖率门槛）、S5（CLI 测试）、S6（nexusphp 拆分）。
- P3：S7（文档/依赖说明）、S8（清场，需确认）。

可并行组：`S2`、`S3`、`S4` 的 `affected_files` 分别为代码、`.gitignore`、workflow，交集为空；`S5` 与它们也不重叠。`S1` 是所有改动的提交收口，必须串行。`S6` 单独分支，不要与 S2 同改 `nexusphp.rs`。

## 六、本轮已关闭

- `reseed.rs` 11,889 → 3,585，CLI `main.rs` 2,380 → 129。
- `coverage.rs` 的转种臂关闭/暂停/禁转三种跳过场景改 `Event::ItemSkipped`，不再写新的 `CoverageStatus::Miss`；`ResolveOutcome::Miss` 仍写源种负缓存，口径已写进 ROADMAP 4.4。
- MSRV 1.88 与 CI `msrv`/`coverage` job 已落地；本地 GNU 1.88 `cargo check` 通过。
- 527 条测试、fmt、clippy 在最新重构上复跑通过；软件改进后增到 528 条。
- 真实发种 city13 88467 / hdvideo 79168 已发布并挂回 100%；tracker `status=1` 与站点未审已如实记录，没有写成“已做种”。

## 七、本轮执行结果

- S1 已执行：改动提交为 `025b712` 并推送 `origin/master`；GitHub Actions run [45993230747](https://github.com/schalkiii/box-send/actions/runs/45993230747) 的 `MSRV 1.88`、覆盖率、Ubuntu、Windows 四个 job 全部 success。
- S2 已执行：新增 `lib.rs::lock_unpoisoned`，`coverage.rs`、`throttle.rs`、`qbittorrent.rs` 的生产路径锁改为 poison 恢复；新增 `lock_unpoisoned_recovers_after_holder_panics` 用例。全量从 527 增到 **528 条**，核心库 455、e2e 33、CLI 23、GUI 13 全绿；fmt/clippy 通过。
- S3 已执行：删掉根目录 `/*.txt`、`/*.html` 两条总通配，改成 `/audit-final*`、`/review*`、`/status-*`、`/test-*`、`/iloli-*` 等已知证据前缀；`artifacts/` 整目录仍忽略。`git check-ignore` 实测 `review-54.txt` 仍命中、`notes.txt`/`README.md`/`report.html` 不再被吞。
- S4/S5/S6/S7/S8 未在本轮执行，作为后续可并行/串行项保留；其中 S8 清场必须等用户明确确认。
- 旧 `doc/AUDIT-REPORT-2026-11-27.md` 保留为拆分前历史版，现状以本报告为准。


