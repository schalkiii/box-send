# 审计报告：Rust 辅种引擎 v1（rust/coverage @ 0a5b103）

审计范围：`rust/` 工作区全部代码与文档（bsend 库 13 模块、bsend-cli、e2e 测试、Cargo 清单、
CI workflow、rust/README.md），基准提交 `0a5b103`（46 单测 + 5 e2e 全绿、clippy -D warnings
零告警、CI Ubuntu+Windows 双矩阵通过）。

## 一、现状描述

- **规模**：2 crate、约 10,000 行（含 148 站识别表约 1,100 行）；`bsend` 库 13 个模块 +
  `bsend-cli` 一个 bin + 1 个集成测试文件（615 行）。
- **结构**：`bencode`/`manifest`/`models`/`matcher` 为纯逻辑层（无 IO，单测最密）；
  `qbittorrent`/`sites::nexusphp` 为 HTTP 适配层；`config` 兼容层（直接读 GUI 版
  boxsend.json + cookies.json）；`index` 持久化；`resolver`/`throttle`/`coverage` 编排层。
- **质量水平**：fmt/clippy 全绿；测试以「移植语义钉住」为主（Swift 行为逐条有断言）；
  e2e 用真实 TCP HTTP mock 跑完整轮次；文档有一份面向使用者的 rust/README.md。
  主要薄弱面：编排层（coverage.rs）无直接单测，qB 客户端的 409/补挂分支零覆盖，
  README 的配置键描述与实现不一致。

## 二、问题清单

| # | 位置 | 问题 | 严重 | 影响 |
|---|------|------|------|------|
| 1 | `bsend/src/index.rs` `is_settled()` | `Verifying` 被算作终态，preskip 跳过它 | **高** | 校验超时的格子**永久卡死**：qB 里那条暂停种子永远无人 resume/回滚，注释「下一轮接着收尾」永不发生（功能错误） |
| 2 | `bsend/src/coverage.rs` `process_item` 四处 `throttle.acquire` 失败分支 | 预算耗尽返回 `Miss` 事件 → 写入索引 → 7 天负缓存 | **高** | 大轮跑爆预算后，剩余格子被错误锁 7 天；「检索已命中、只差下载预算」的格子被记成「站内没有」（事实错误 + 漏辅种 7 天） |
| 3 | `coverage.rs` 已做种预筛；`resolver.rs:93` | announce host 精确相等匹配 | **高** | tracker 常在子域（`tracker.xxx.com` vs 页面 `xxx.com`）→ Seeding 预筛漏判（多花 3–4 请求/格，把已在做种误记 Assisted）、源站优先排序失效 |
| 4 | `coverage.rs` preskip | `ReviewPending`（C 级）与 `Failed`（校验回滚失败）无 TTL，每轮无限重搜重推 | 中 | v1 无人工确认入口 → C 级格子每轮烧 3–4 请求；回滚类 Failed 每轮「重搜→重推→再校验→再删」死循环（预算黑洞） |
| 5 | `qbittorrent.rs` `patch_trackers` setTrackers 回退分支 `.ok()?` | 请求发送失败被当成功（返回 None=成功语义） | 中 | 4.x 回退路径补挂失败却提示「已补挂，跨种已生效」（功能性误报） |
| 6 | 测试布局 | 409 已存在 → `attach_site_tracker` → addTrackers/setTrackers 全链路零测试 | 中 | Swift 移植的核心兼容点无回归保护 |
| 7 | `rust/README.md` | 配置键写成 `coverage.searchBudget/sourceBudget/uploadBudget`；实现读 `assist.roundBudget`（search/source 共用）、upload 固定 10 | 中 | 照 README 配置不生效（误导使用者） |
| 8 | `config.rs` `DownloaderSettings.skip_checking` | 加载但从未使用（push_tail 按方案 §七 硬编码 false） | 低 | 死字段误导（配了不生效） |
| 9 | `bsend/Cargo.toml` | `thiserror`、`humantime` 依赖未被任何代码引用 | 低 | 死依赖：拖慢构建、误导读者 |
| 10 | `index.rs` `new_entry` `let _ = key;`；`coverage.rs` `process_item` `_tx`、`reseed_arm` `_done_url` | 重构残留死参数 | 低 | 可读性 |
| 11 | `coverage.rs` `merge_scan` 调用点（run_round） | hash-only 条目在 `files_of` 返回 None 后，同一次扫描内再调一次（失败路径双请求） | 低 | qB API 波动时请求翻倍 |
| 12 | `bsend-cli/Cargo.toml` | 未用 workspace 继承（version/edition/license 硬编码）；workspace 无 `rust-version` | 低 | 清单不一致；MSRV 未声明 |
| 13 | `coverage.rs` `worker_loop` | adapter 查不到时静默丢整站队列（理论不可达，防御性差） | 低 | 若未来调用方式变化会静默丢工作 |
| 14 | README 状态机段 | waitSource「30 天」表述含糊：30 天只作用于**源种定位**负缓存；格子本身 7 天后重搜目标站 | 低 | 使用者预期偏差 |

**各维度结论**：①文档一致性——问题 7/14；②文档缺漏——Rust-only 旋钮
（maxConcurrentSites/archiveDays/sourceMissTtlDays/uploadBudget 不可由 boxsend.json 配）未说明；③架构——
done/done_url 闭包链 + too_many_arguments 属可接受取舍，登记不改（最小改动）；④代码写法——问题 5/8/9/10/11/13；
⑤测试覆盖率——问题 6 + coverage.rs 编排逻辑（优先级排序、--limit 裁剪、收尾路径）无直接单测；
⑥测试严谨性——e2e 断言到请求计数与索引终态，质量好；「跑通即通过」类无；⑦工程规范——问题 12，其余规范（fmt/clippy/CI/原子写/明文 JSON）良好。

## 三、改进建议（做法 / 文件 / 验证）

### P1-1 Verifying 收尾机制（修问题 1）
- `index.rs`：`SiteCoverage` 增 `#[serde(default)] pub verify_hash: Option<String>`（旧 coverage.json 兼容）。
- `coverage.rs`：`Event::ItemDone` 增 `verify_hash: Option<String>`；push_tail 超时分支直接构造带
  `verify_hash` 的事件（其余状态一律 None）；run_round 事件应用时写进格子。
- `coverage.rs` run_round：Seeding 预筛之后、入队之前，主线程对每个 `Verifying` 且带 `verify_hash`
  的格子直查 `qb.torrent_progress`：≥0.999 → resume → Assisted；非 checking 且 <0.999 → 按
  `removeOnVerifyFail` 删种保文件或保持暂停 → Failed；查不到该种子 → Failed（用户手删）；
  仍在 checking → 留 Verifying（下轮再收尾）。qB 查询不过站点预算。
- 验证：新 e2e `e2e_verifying_collect`（mock 进度可编程：卡 0.5 pausedUP → 收尾 Failed；卡
  checkingDL → 留 Verifying；1.0 → Assisted 且有 resume 请求）+ `cargo test` 全绿。

### P1-2 预算耗尽不再记 Miss（修问题 2）
- `coverage.rs`：新增 `Event::ItemSkipped { site_id, key, reason }`；`process_item` 四处预算失败改返
  ItemSkipped；run_round 事件循环对 ItemSkipped 只 `report.preskipped += 1` 并转发，**不写索引**。
- `main.rs` `print_event` 增分支。
- 验证：e2e `e2e_budget_exhaustion_not_cached`（searchBudget=1 的小预算：第一格正常、第二格
  ItemSkipped；断言第二格索引无 coverage 记录、重跑会再检索）+ 全量 `cargo test`。

### P1-3 announce 子域匹配（修问题 3）
- 新助手（`sites/mod.rs` 或 `manifest.rs`）：`pub fn host_matches(tracker_host: &str, site_host: &str) -> bool`
  ＝ 相等 ∨ `tracker_host.ends_with(".{site_host}")`。
- `coverage.rs` 预筛、`resolver.rs:93` 排序改用之。
- 验证：单测（等/子域/跨域/空 host）+ 现有 e2e 不回归（127.0.0.1 vs localhost 仍不相等，语义不变）。

### P1-4 ReviewPending / Failed 套 miss TTL（修问题 4）
- `coverage.rs` preskip：`Miss | WaitSource | ReviewPending | Failed` 统一 `at > miss_cutoff` 才重试。
- README 状态机段同步。
- 验证：现有 e2e 全绿（单轮场景不触发）；新增单测钉 preskip 语义（构造带 TTL 的格子验证跳过/到期重试——
  若做成纯函数则直接单测）。

### P2-1 setTrackers 发送失败当失败（修问题 5）
- `qbittorrent.rs` `patch_trackers` 回退分支：`.ok()?` 改为显式映射 `Some("补挂请求没通（setTrackers）")`。
- 验证：`cargo test` + 代码走查（无 mock 也行，路径极短）。

### P2-2 删死代码与死依赖（修问题 8/9/10/12）
- 删 `DownloaderSettings.skip_checking`（e2e 构造点同步）；删 `thiserror`/`humantime` 依赖；
  删 `new_entry` 的 key 参数、`process_item` 的 `_tx`、`reseed_arm` 的 `_done_url`；
  `bsend-cli` 改 `version.workspace/edition.workspace/license.workspace` 继承，workspace 增 `rust-version = "1.82"`。
- 验证：`cargo test` + `cargo clippy --all-targets -- -D warnings` + `cargo build --release`。

### P2-3 README 纠偏（修问题 7/14 + 文档缺漏）
- 配置键表改为实际支持的 `assist.*` 键清单；说明 search/source 共用 `roundBudget`、upload 固定 10；
  列出 Rust-only 旋钮（代码常量，v1 不暴露配置）；状态机段补「reviewPending/failed 也按 missTTL 重试」
  「waitSource 格子 7 天重搜目标站、源种定位另享 30 天负缓存」；补 verifying 收尾说明。
- 验证：逐条对照 `config.rs` 与 `coverage.rs` 人工校核。

### P3 测试补强（修问题 6 及覆盖缺口）
- e2e `e2e_already_exists_attaches_tracker`：mock `/torrents/add` 回 409 → 断言 addTrackers
  带目标站 announce、格子 Assisted、无重复添加。
- e2e Verifying 收尾（并入 P1-1 的用例）。
- 单测：cookies.json `{"cookies":{...}}` 外层包裹解析；sourceSites 无 framework 字段被剔除（Custom）。
- 验证：`cargo test` 全绿，计数增长如实登记。

### P4 登记不改（架构取舍，保持最小改动）
- done/done_url 闭包参数链；worker_loop 防御性 Log；merge_scan 双调 files_of；
  coverage.rs 优先级排序/limit 裁剪的直接单测（e2e 已间接覆盖主路径）。

## 四、优先级与并行性

- **P1-1 → P1-2 → P1-3 → P1-4**：全部触碰 `coverage.rs`，**串行执行**（每步后跑全量测试）。
- **P2-1**（仅 qbittorrent.rs）与 **P2-2**（死代码/清单）与 P1 无文件交集，可穿插并行；
  P2-3（README）在 P1 定稿后做，避免返工。
- **P3** 的 e2e 依赖 P1-1/P1-2 的新事件与字段，最后做。
- 全部完成后：fmt → clippy -D warnings → cargo test → 提交 → push → CI 绿。

## 五、修复记录（2026-11-17 处置结果）

P1–P3 全部落地（49 单测 + 9 e2e 全绿，fmt/clippy 零告警）：

- **P1-1 ✅** Verifying 收尾 pass：`SiteCoverage.verify_hash`（`#[serde(default)]` 兼容旧盘）、
  `Event::ItemDone.verify_hash`、run_round 在入队前直查下载器收尾（满 100% → resume →
  Assisted；非 checking 且不满 → 按 `removeOnVerifyFail` 删种保文件/保持暂停 → Failed；
  查无此种 → Failed；仍在 checking → 留待下轮）。e2e：`e2e_verifying_collect_assisted`
  （启动分支）与 `e2e_verifying_collect_rollback`（删种分支），均断言收尾轮零站点请求、
  凭据落盘且用掉即清。
- **P1-2 ✅** `Event::ItemSkipped`：四处预算获取失败不再记 Miss、不写索引；CLI 打印「跳过」。
  e2e：`e2e_budget_exhaustion_not_cached`（预算 1 → 一格 WaitSource、一格跳过且无格子；
  第二轮预算回来两格补齐）。
- **P1-3 ✅** `sites::host_matches`（相等 ∨ `.{site_host}` 子域后缀），预筛与 resolver
  排序改用；单测 `host_matches_exact_and_subdomain`（等值/子域/跨域/反向/空值）。
- **P1-4 ✅** preskip 统一 TTL：`Miss/WaitSource/ReviewPending/Failed` 都歇 missTTLDays。
  语义由 e2e 钉住（wait_source 与 budget 两场景的第二轮 TTL 跳过）；未抽独立单测
  （内联逻辑，抽纯函数属侵入式改动，按 P4 精神登记不改）。
- **P2-1 ✅** setTrackers 回退分支发送失败返回「补挂请求没通（setTrackers）」。
- **P2-2 ✅** 删 `skip_checking`、`thiserror`、`humantime`、`new_entry` 的 key 参数、
  `process_item` 的 `_tx`、`reseed_arm` 的 `_done_url`；bsend-cli 三字段 workspace 继承；
  workspace 增 `rust-version = "1.82"`。
- **P2-3 ✅** README 纠偏：`assist.*` 实际键名、roundBudget 双用、Rust-only 固定值清单、
  统一 TTL 语义、verifying 收尾与子域预筛说明、测试计数 49+9。
- **P3 ✅** e2e `e2e_already_exists_attaches_tracker`（409 → 补挂请求体含本站 announce、
  已存在路径不碰启停）；单测 cookies.json PT-depiler 外层包裹解析、非检索框架剔除
  （用独立临时目录，避开共享 cookies.json 串扰）。

P4 维持登记不改（问题 11/13 防御性/低影响，避免侵入式重构）。
