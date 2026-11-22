# bsend（Rust）审计报告 · 2026-11-22

> 面向对象：接手修复的执行 Agent。本文只做审计与改进建议，不改任何代码。
> 阅读顺序建议：第 1 节看现状 → 第 3 节领问题 → 第 4 节照做 → 第 5 节看并行边界。

## 0. 审计范围、基线与声明

- **基线**：仓库 `C:\Users\Schal\WorkBuddy\2026-11-17-20-52-34`。审计开始于 `ee4dd4a`；
  审计收敛时主 Agent 已提交 **`aa5a404`（审计遗留收口）**，工作区另有未提交的 `reseed.rs`
  （已含 SEC-1 的脱敏修复）。本文问题清单以 **`aa5a404` + 工作区现状**为准。
- **只读**：本次未运行 `cargo build / test / clippy`，避免与主 Agent 争用 `rust/target` 锁、
  也避免读到编译中间态。所有验证方式写在建议里，由执行者后续运行。
- **`aa5a404` 已收口**（不再是待办，详见第 6 节）：
  - TEST-H1（部分）：`autopost` 抽出 `run_autopost_with`，加 5 条零网络编排测试；
  - BUG-L8：`cmd_sites` 改用 `Config` 视图 + `bsend::url_host`；
  - ARC-2（部分）：GUI 拆出 `types.rs` / `jobs.rs`（行为不变）；
  - 名字检索成员名兜底：`assist.memberNameFallback`（缺省开，受预算/站间隔约束）+ 单测；
  - ENG-M2：新增 `.github/workflows/rust-release.yml`；
  - BUG-M11：`append_audit` 改为写入时刻盖章 + 单测。
- **工作区在途**：`reseed.rs`、`autopost.rs`、`throttle.rs`、`index.rs`、CLI/GUI、`rust/README.md`、
  `doc/DATA-FILES.md` 均有未提交改动（主 Agent 正在按本报告实施修复）。**以 `git diff` 为准**：
  已修条目见第 6.1 节，未修条目才是待办。
- **问题总数**：本报告登记 **20 条发现**（高 1 / 中 11 / 低 10，按发现时口径）。**截至落盘，
  已在工作区修复 7 条**（SEC-1、BUG-M9/M11/M12/M13、ARC-3、DOC-M3，见第 6.1 节），
  **仍待修 15 条（高 0 / 中 5 / 低 10）**。§14.5 其余遗留的现状也在第 6 节。
- **Lint**：本报告经 `markdownlint` 检查，结构类规则 0 error；MD013 行宽未启用（仓库既有 `doc/`
  文档同为长行且无 `.markdownlint` 配置，按同一口径处理）。
- **行号会漂移**：上述文件仍在被改，本文定位以**函数名/结构体名**为准，行号只作参考。
  执行前先 `git status`，不要与在途改动并发编辑同一文件。
- **未做真实发种**：本文不提供任何需要向真实站点写请求的验证步骤；演练一律用
  `--dry-run`、`formcheck`、`gatecheck`、mock e2e。
- **凭据**：本文不记录任何 cookie、passkey、token、密码的值；只描述其流向与风险。

## 1. 现状描述

### 1.1 规模（可复现事实）

| 项 | 数值 |
| --- | --- |
| Rust workspace 成员 | 3 个：`bsend`（库）、`bsend-cli`、`bsend-gui` |
| Rust 源文件（含 tests） | 26 个，约 **20,900 行**（含测试；PowerShell 行统计，可能有几行偏差） |
| 库核心 | `bsend` 约 17,700 行；最大单文件 `reseed.rs` 约 4,900 行，`autopost.rs` 约 750 行 |
| GUI | `main.rs` 约 1,500 行 + `jobs.rs` 216 行 + `types.rs` 52 行（`aa5a404` 拆分后） |
| CLI | `bsend-cli/src/main.rs` 约 1,450 行 |
| e2e | `tests/coverage_e2e.rs` 1,453 行 |
| 测试（`aa5a404` + 工作区） | 约 **200**（单测 + 19 e2e + 2 CLI；主 Agent 仍在加，准确数以 `cargo test` 为准） |
| 站点内置表 | 148 条（`sites/mod.rs::SITE_TABLE`，代码测试钉住条数与 id 唯一） |
| 示例配置 | `Config/boxsend.example.json` 700 行 |
| 文档 | `doc/` 8 篇 + 根目录 6 篇 Markdown |
| CI | `rust.yml`（Ubuntu + Windows 矩阵）；`aa5a404` 新增 `rust-release.yml`（tag 出包）；`release.yml` 仍为 Swift 遗留 |

### 1.2 结构

- **分层清晰**：CLI / GUI 只做参数解析与事件渲染；`autopost` / `reseed` / `coverage` /
  `resolver` 是编排层；`sites::nexusphp`、`qbittorrent`、`bencode`、`matcher`、`layout`、
  `manifest`、`piecesapi` 是能力层；`config` / `index` / `models` 是数据与配置。
- **数据流**：`bsend scan` → `coverage.json`（内容 × 站点格子）→ `bsend run`（prefilter →
  辅种臂 → 转种臂出 `reseedDraft`）→ `autopost`（白名单 → 日限额 → 退避 → 审阅 → 门禁）
  → `run_reseed`（预检 → 判重 → 官方预审 → 上传 → 发后自检 → 推种校验）→ 格子落账。
- **共享入口**：`sites::build_adapters` 已下沉给 CLI/GUI 共用（ARC-1 成果），
  `autopost::run_autopost` 也已 CLI/GUI 共用（`aa5a404` 进一步抽出 `run_autopost_with`
  供测试注入）。

### 1.3 质量水平（结论性事实）

- **正面**：安全闸门意识强——`assist.publishEnabled` 缺省 false 且三处入口都过；
  判重/预算耗尽一律 fail-closed；审计写失败中止本轮；`coverage.json` 原子写且损坏隔离；
  限流页/登录墙/WAF 挑战页都有专门识别；e2e 用真实 TCP mock 跑完整一轮。
- **短板**：
  1. **凭据输出卫生**：转种审阅曾把上传表单 hidden 字段（含 passkey）原样打印；工作区已加
     字段名脱敏（SEC-1），但尚未提交、也缺 masking 单测。
  2. **审阅与发布两份字段**：门禁校验的字段与实际提交的字段是两次独立构造（BUG-M7）。
  3. **审阅阶段绕过节流**：`review_reseed` 不收 `SiteThrottle`，且每条草稿把源页/表单/预审抓两遍（BUG-M8）。
  4. **账本一致性**：日限额/审计无跨进程锁（BUG-M10）。
  5. **GUI 零测试**；编排层记账路径仍缺自动化（TEST-M1/TEST-L4）。
  6. **文档口径**：dataDir 缺省、plan 是否落盘、日界 UTC/北京、147/148 站口径有漂移（DOC-M3/DOC-3）。

## 2. 七维结论

| # | 维度 | 结论 | 对应条目 |
| --- | --- | --- | --- |
| 1 | 文档一致性 | **发现问题**：4 条编号问题 + README 自动标签说明不全（并入 S-13） | DOC-M3、DOC-3、DOC-M4、ARC-3 |
| 2 | 文档缺漏 | **发现问题**：4 类关键行为/约束没写进文档 | DOC-M4、GAP-3、ENG-L3、MediaInfo 环境变量 |
| 3 | 架构优化 | **发现问题**：4 处 | BUG-M7、BUG-M8、ARC-4、`builtin_sites` 生产未用（并入 GAP-3）；GUI 单文件已由 `aa5a404` 拆开（残余测试 → TEST-M1） |
| 4 | 代码写法/潜在 bug | **发现问题**：8 处（其中 4 处已在工作区修复） | SEC-2、BUG-M9/M10/M12/M13、BUG-L10/L11 |
| 5 | 测试覆盖率 | **发现问题**：编排记账/GUI/原子写等仍空 | TEST-M1、TEST-L4、TEST-L3、run_reseed e2e（TEST-H1 残余） |
| 6 | 测试严谨性 | **发现问题**：2 处「mock/断言没覆盖真实分支」 | TEST-L3、TEST-L4 |
| 7 | 工程规范 | **发现问题**：ENG-L3 + 2 处已知残余 | ENG-L3（LICENSE/CHANGELOG）、release 已补 `rust-release.yml`（待验证）、无 MSRV job（已知 ENG-M1 残余） |

**落盘时状态**：上表 20 条发现中 7 条已在工作区修复（SEC-1、BUG-M9/M11/M12/M13、ARC-3、
DOC-M3），**仍待修 15 条（高 0 / 中 5 / 低 10）**。已修条目在 §3 保留原文并标注「已修」，
便于回溯与验收；执行请以「未修」条目为准。

七个维度**没有一维是完全干净的**；但也没有发现「必须回滚」级别的破坏性问题。
当前最需要先处理的是第 3 维的审阅/发布一致性（BUG-M7/M8）与跨进程账本锁（BUG-M10）。

## 3. 问题清单

> 每条四要素：位置 / 描述 / 严重程度 / 影响范围。
> 已知遗留（§14.5：TEST-H1、BUG-L8、ENG-M2、ARC-2、名字检索追加成员种名）不重复登记；
> 对它们的补充证据见第 6 节。
>
> **状态标记**：标题带「【已修，见 §6.1】」= 工作区已修复（未提交）；未标注 = 仍待修。
> 落盘时仍待修 **15 条（高 0 / 中 5 / 低 10）**。

### 3.1 高

按**发现时**口径有 1 条高项 **SEC-1**（审阅打印 passkey），现已在工作区修复
（`reseed.rs::mask_field_value`，按字段名脱敏 `render`/`dump_fields`，并补了单测），
详见第 6.1 节；**当前无待修高项**。在提交前不要把它当作已交付。

### 3.2 中

#### BUG-M7｜门禁校验的字段与实际提交的字段是两份（中）

- **位置**：
  - `autopost.rs::run_autopost_with`：`review_reseed` → `analyze_prepared` → `gates::evaluate`；
  - `reseed.rs::run_reseed`：再次 `prepare_released` + `prepare_upload` + `apply_site_rules`。
- **描述**：自动发种先用「审阅」构造一份 `PreparedUpload`，门禁在这份字段上判定；
  通过后又调用 `run_reseed`，后者从源页/上传页**重新**构造一份字段并提交。两份之间没有任何
  一致性校验。`apply_site_rules` 里的 `prune_dead_images` 依赖实时网络探测，站点表单也可能
  在两次请求之间变化，因此「门禁通过的那份」与「真正上传的那份」可能不同。
- **严重程度**：中（触发概率取决于网络探测与站点表单稳定性，但「门禁承诺」层面是缺口）。
- **影响范围**：`bsend autopost`、GUI「开始自动发种」。门禁的核心承诺是「不该发的一律发不出去」；
  两份字段意味着这条承诺只对「审阅时的那份」成立，属于典型的 TOCTOU 缺口。

#### BUG-M8｜autopost 的审阅阶段绕过站点节流，且每条草稿重复抓取（中）

- **位置**：
  - `reseed.rs::review_reseed(cfg, req, log)`：签名没有 `SiteThrottle`；
    内部 `prepare_released`（拉源站详情页、可能生成 MediaInfo）、`prepare_upload`（GET 上传页）、
    `run_pre_audit`（POST 官方预审）都是真实站点请求，全部不领号。
  - `autopost.rs::run_autopost_with`：先 `review_reseed`，再 `run_reseed(..., Some(&throttle))`；
    后者把上面三类请求**再做一遍**。
- **描述**：`autopost` 为一次运行建了共享 `SiteThrottle`，但它只覆盖 `run_reseed` 内部；
  审阅阶段的请求既不计预算、也不遵守站间隔（hdsky 这类 60 秒/次的站会被连敲）。
  同时每条草稿的源页、上传页、官方预审各抓两次，请求量与耗时翻倍。
- **严重程度**：中。
- **影响范围**：`bsend autopost`、GUI 自动发种；直接削弱 §14.3「autopost 一次运行共用一个节流器」
  的修复效果，并放大被站点限流/WAF 拦截的概率。

#### BUG-M9｜autopost 把所有空 URL 结果都记成 skipped_duplicate（中）【已修，见 §6.1】

- **位置**：
  - `reseed.rs::run_reseed` 判重分支：写 `Assisted` / `OtherVersion` / `Failed` 后
    一律 `return Ok(String::new())`。
  - `autopost.rs::run_autopost_with`：`Ok(url) if url.is_empty()` →
    审计记 `result: "skipped_duplicate"`，不计日限额、不进退避。
- **描述**：空 URL 实际上混合了四种结果：同分片已覆盖（Assisted）、站点已有其他版本
  （OtherVersion）、判重复核失败（Failed）、上传回包标记 existed（ReviewPending）。
  自动发种把它们全部记成「重复跳过」：前三者的账目失真，`Failed` 本应走失败退避、
  `ReviewPending` 本应人工确认，却都失去了对应审计语义。
- **严重程度**：中。
- **影响范围**：`auto-audit.jsonl` 的可信度、日限额与退避逻辑、运维对「为什么没发出去」的判断。

#### BUG-M10｜日限额与账本没有跨进程互斥（中）

- **位置**：`autopost.rs`（读 `auto-audit.jsonl` → 判定 → 发布 → 追加审计）；
  `index.rs::ContentIndex::save`（整文件 tmp+rename，无锁）。
- **描述**：两个进程（例如计划任务里的 CLI + 打开着的 GUI，或两次并发 CLI）会各自读到
  相同的 `posted_today`，都能把当日剩余额度用完，突破 `dailyLimit`；`coverage.json`
  并发写则是后写覆盖先写，丢格子结论。进程内没有锁，文件系统也没有 lock 文件。
- **严重程度**：中。
- **影响范围**：无人值守场景（本项目的目标用法）下，日限额这一「防封号」保险可能失效。

#### BUG-M12｜requireLinks / requireMediaInfo 放宽只在门禁生效，发布仍被拦（中）【已修，见 §6.1】

- **位置**：`gates.rs::SiteRules.require_links / require_mediainfo`（门禁按规则放宽）；
  `reseed.rs::preflight`（无条件把「MediaInfo 为空」「无条目链接且补不上」列为阻断）。
- **描述**：站点规则里把这两项设为 `false` 后，门禁会通过，但 `run_reseed` 的预检仍会
  `bail!("预检未通过")`，实际永远发不出去。配置项语义与行为不一致。
- **严重程度**：中。
- **影响范围**：`assist.reseedRules.<site>.requireLinks/requireMediaInfo` 的配置者；
  会表现为「门禁全绿但自动发种一直 failed」。

#### BUG-M13｜限流退避被「一次成功」清零，会在限流站反复撞墙（中）【已修，见 §6.1】

- **位置**：`throttle.rs::note_ok`（`backoff = 1; min_interval = ZERO`）；
  调用点 `coverage.rs::process_item` 成功检索/下载后、`resolver.rs::resolve_source` 成功/未命中后。
- **描述**：站点通过限流页把该站 `min_interval` 抬到 60 秒（缺省 `rateLimitFloorSec`）后，
  只要有一次请求成功，`note_ok` 立刻把地板清零，下一次请求回到全局 8 秒节奏 → 再次触发限流。
  若站点没有在 `assist.siteIntervals` 里显式配置（例如新站/临时站），就会形成「撞限流 →
  等 60 秒 → 成功一次 → 8 秒后再撞」的循环，每次多消耗 1 次请求并留下违规痕迹。
- **严重程度**：中（有条件：仅对「靠限流页发现节奏要求、且未写 siteIntervals」的站）。
- **影响范围**：hdsky 这类严格站点；PT 站对无视限流的行为会警告甚至封号。

#### GAP-3｜非 NexusPHP / 缺 url 的启用站被静默排除，日志不点名（中）

- **位置**：`config.rs::Config::load` 站点过滤循环：
  `if !framework.search_capable() { continue; }`、`let Some(url) = ... else { continue; }`；
  `sites::build_adapters` 只对「缺 cookie」回调点名，不报告被框架/字段排除的站。
- **描述**：用户在 `boxsend.json` 里启用了一个 Unit3D/Blu 站或漏写 url，引擎直接跳过，
  `bsend run` 只打印「源站适配器 N 个」，没有任何「X 站因框架不支持被排除」的提示。
  GUI 同样沉默。这是「站点适配缺口」在体验层最直接的体现。
  附带一点：`sites/mod.rs` 的 148 站内置表（`builtin_sites` / `site_def_by_host`）在生产路径
  没有被调用（只在测试里用），README「148 站识别表」容易被读成运行时会自动认站；
  实际仍完全依赖 `boxsend.json` 的 `sourceSites[].framework`。
- **严重程度**：中。
- **影响范围**：所有含非 NexusPHP 站的配置；用户会误以为站点参与了辅种/转种。

#### DOC-M3｜DATA-FILES 的 dataDir 缺省与写入者清单与实现不符（中）【已修，见 §6.1】

- **位置**：`doc/DATA-FILES.md` 开头「全部位于 dataDir（缺省 `~/.boxsend`，Windows 实机为
  `%LOCALAPPDATA%\BoxSend`）」；`coverage.json` 的「写入者」里列了 `plan`。
- **描述**：`config.rs::coverage_state_path` 只认配置里的 `dataDir`，没有时退到**配置文件所在目录**，
  代码里不存在 `~/.boxsend` / `%LOCALAPPDATA%\BoxSend` 的缺省逻辑（GUI 也没写）。
  另外 CLI `plan` 传 `persist=false`，不写 `coverage.json`（只写 `pieces-cache.json`）。
- **严重程度**：中。
- **影响范围**：排障时找错目录；运维以为 `plan` 会留下账本。

#### TEST-M1｜GUI crate 零自动化测试（中）

- **位置**：`rust/crates/bsend-gui/src/`（`aa5a404` 已拆出 `jobs.rs`/`types.rs`，`main.rs` 仍约 1,500 行）。
- **描述**：GUI 承担配置写入（`mutate_config` 保留未知键 + 备份 + 原子替换）、后台任务调度、
  200ms 事件泵、审阅/转种/自动发种三入口，但三个文件合计 0 个 `#[test]`。ARC-2 拆分后
  没有回归护栏。
- **严重程度**：中。
- **影响范围**：GUI 改动只能靠手点验证；配置写坏会直接影响 CLI（共用同一份 `boxsend.json`）。

#### ARC-3｜OtherVersion 不在终态集合，7 天后会被重新入队（中）【已修，见 §6.1】

- **位置**：`index.rs::CoverageStatus::is_settled`（不含 `OtherVersion`）；
  `coverage.rs::run_round` 预筛 `Some(_) => cell.at > miss_cutoff`；
  `rust/README.md`「otherVersion（…）不自动重传」。
- **描述**：`OtherVersion` 是程序化判定「目标站已有不同版本」，但 7 天后会被当作可重查格子
  再排进队列，重新走检索/判重。README 的「不自动重传」容易被读成「永久不再处理」，
  实际是「每个 missTTL 周期白查一次」。
- **严重程度**：中。
- **影响范围**：所有「目标站有同片其他版本」的格子，长期占用检索预算；文档与行为不一致。

### 3.3 低

#### SEC-2｜CookieCloud 访问键出现在错误消息里（低）

- **位置**：`cookiecloud.rs::fetch_envelope`：`format!("请求 CookieCloud 没通：{url}")`、
  `format!("CookieCloud 回包不是 JSON：{url}")`，其中 `url = {host}/get/{key}`；
  `bsend-cli/src/main.rs::apply_cookiecloud` 打印 `e.root_cause()`（注释称「不打含访问键的完整 URL」，
  但 reqwest 错误的 Display 本身含 URL）；GUI 任务用 `{e:#}` 打印完整 anyhow 链。
- **描述**：CookieCloud KEY 会随错误信息进入终端/GUI 日志。KEY 本身不能直接解密（还需配置里的
  password），但它是一个访问凭据，且项目其它地方明确避免打印凭据。
- **严重程度**：低（但属安全卫生问题，建议与 SEC-1 同批修）。
- **影响范围**：`bsend --cookiecloud`、`bsend cookies --from cookiecloud`、GUI「拉取 CookieCloud」。

#### DOC-3｜日界描述仍有 UTC 残留（低）

- **位置**：`autopost.rs` 的 `PostedToday` 类型注释「各站今日 posted 计数（UTC 日界）」（唯一残留）；
  `bsend-gui/src/main.rs` 两处文案：「每站每日限额按 UTC 日计」「今日已自动发 N 条（UTC 日界）」。
  实现与 `doc/DATA-FILES.md` 都是北京时间（UTC+8）。
- **描述**：同一个概念三种说法；用户会按 UTC 00:00（北京 08:00）理解重置时刻。
- **严重程度**：低。
- **影响范围**：文档/界面认知，不改变运行行为。

#### DOC-M4｜qB `tags=Boxsend` 新行为没有进用户文档（低）

- **位置**：`qbittorrent.rs::BOXSEND_TAG` / `add_form_fields` / `add_tags`（实现）；
  `rust/README.md`、`doc/DATA-FILES.md`（均未提）。
- **描述**：引擎加种统一打 `Boxsend` 标签、409 接管时追加标签，这是用户可见的行为
  （下载器里可筛选），但只出现在 2026-11-21 审计报告 §14.7，没有进 README/数据文件契约。
- **严重程度**：低。
- **影响范围**：用户不知道标签来源与含义；也不清楚标签失败只进备注、不影响推种。

#### DOC-4｜根 README 没有 Rust 导航，且存在已追踪的旧副本（低）

- **位置**：`README.md`（1.8 KB，Swift 版旧说明，未提 `rust/`）；
  `README_副本.md`（约 49 KB，已入库的 README 副本）。
- **描述**：第一次进入仓库的人会落在旧 README；`README_副本.md` 与 `README.md` 并存，
  无法判断哪份是现行说明。`rust/README.md` 才是当前引擎说明。
- **严重程度**：低。
- **影响范围**：新接手者/外部读者的第一印象与文档可信度。

#### ARC-4｜原子写有四套实现，临时文件后缀各不相同（低）

- **位置**：`lib.rs::atomic_write`（`.json.tmp`）、`index.rs::ContentIndex::save`（`.json.tmp`）、
  `report.rs::CoverageReport::write_json`（`.json.tmp`）、`config.rs::save_cookies`（`.json.tmp` + `.bak`）、
  `bsend-gui/src/main.rs::mutate_config`（`.tmp-gui`）。
- **描述**：语义一致但实现分散；新增落盘点容易忘记原子写，或漏清理各自的 tmp 文件。
  `atomic_write` 已经存在，却没有被索引/报告/配置复用。
- **严重程度**：低。
- **影响范围**：可维护性；DATA-FILES「原子写」承诺依赖各实现不漂移。

#### TEST-L3｜e2e mock 的 add 前缀匹配吞掉 addTags，409 补标签成功路径无覆盖（低）

- **位置**：`tests/coverage_e2e.rs::qb_server_409`：
  先判 `path.starts_with("/api/v2/torrents/addTrackers")`，再判 `starts_with("/api/v2/torrents/add")`；
  `qBittorrent::add_tags` 请求的 `/api/v2/torrents/addTags` 会命中后者并返回 409。
- **描述**：`e2e_already_exists_attaches_tracker` 只断言 tracker 补挂与 note 含「补挂」，
  没有断言标签；实际上 mock 让 `add_tags` 失败并被吞进 note。409 接管时「补标签成功」
  这条真实分支没有任何测试证明。
- **严重程度**：低。
- **影响范围**：`tags` 功能在接管路径的回归无护栏。

#### TEST-L4｜autopost 记账路径仍未被注入式测试覆盖（低）

- **位置**：`autopost.rs` 的 `#[cfg(test)] mod tests`（`aa5a404` 已加 6 条：
  白名单、日限额预筛、退避、站点过滤/limit 0、审计路径不可写、审计写入时刻）。
- **描述**：仍缺三类最关键的记账分支：
  ① 门禁拦截 → 写 `blocked` 审计 + Outcome；② 发布成功 → `posted` 审计 + `posted_today` 递增；
  ③ 空 URL → `skipped_duplicate` 审计。这三条正是日限额/退避赖以生效的路径。
- **严重程度**：低（但属 TEST-H1 的核心残余，建议与 BUG-M9 一起改）。
- **影响范围**：自动发种「账记错了」类回归无法被自动发现。

#### BUG-L10｜dry-run 仍强依赖下载器在线（低）

- **位置**：`autopost.rs::run_autopost` 开头无条件 `QBittorrent::new` + `test_connection()`
  后才进入 `run_autopost_with`。
- **描述**：`--dry-run` 只需要审阅 + 门禁（`run_reseed` 不执行），却因下载器不可达而整体失败。
- **严重程度**：低。
- **影响范围**：下载器停机/换机时无法做门禁演练；与「演练不受 publishEnabled 影响」的设计意图不符。

#### BUG-L11｜检索命中首个候选即返回，未在多候选间择优（低）

- **位置**：`sites/nexusphp.rs::search`（`search_name_in_results` 返回第一个命中即 `Ok(Hit)`）；
  `coverage.rs::process_item` 命中后直接下载判定。
- **描述**：同标题多版本（不同分辨率/组）的站，若第一个结果不是本地这份，会落到
  `ReviewPending`/`OtherVersion`，而不会继续比较后续候选的文件清单。属于召回策略的保守损失，
  不是数据破坏。
- **严重程度**：低。
- **影响范围**：多版本同名内容的辅种成功率。

#### ENG-L3｜缺 LICENSE 与 CHANGELOG（低）

- **位置**：仓库根目录（无 `LICENSE`、无 `CHANGELOG.md`）；`rust/Cargo.toml` 声明 `license = "MIT"`。
- **描述**：声明与文件不一致；版本号 `0.1.0` 从未在 CHANGELOG 留痕，ENG-M2 做 release 时
  没有版本事实来源。
- **严重程度**：低。
- **影响范围**：分发合规与发版可追溯性。

## 4. 改进建议

> 每条：具体做法 / affected_files（精确到文件）/ 验证方式。
> `affected_files` 直接作为并行门禁的写集输入；执行者按第 5 节分组，不要跨组并发写同一文件。

### 建议 S-01（对应 SEC-1，已完成待提交）：字段值脱敏渲染

- **当前状态**：工作区 `reseed.rs` 已新增 `mask_field_value` 并接入 `ReviewReport::render`
  与 `dump_fields`，同时补了单测 `review_render_masks_secret_field_values`。
  **剩余工作只有提交**（随主 Agent 当前批次）。
- **具体做法**（验收口径）：
  1. `mask_field_value` 按**字段名**判定（不按值猜）：命中 `passkey` / `token` / `auth` /
     `cookie` / `secure` / `uid` / `secret` / `password` 时输出 `<已隐藏 len=N>`；其余原样。
  2. `ReviewReport::render` 与 `dump_fields` 统一走它。
  3. 保留「字段名 + 是否存在 + 长度」的可见性，审阅仍能判断「passkey 有没有透传成功」。
- **affected_files**：
  - `rust/crates/bsend/src/reseed.rs`（`mask_field_value`、`ReviewReport::render`、`dump_fields`、tests）
- **验证方式**：
  - 新增单测：构造 `fields = [("passkey", "SECRET-PLACEHOLDER"), ("name", "Show")]`，
    断言 `render` 输出不含 `SECRET-PLACEHOLDER`、含 `passkey` 字段名与 `<已隐藏`；
  - `cargo test -p bsend review_render`（执行者运行）；
  - 只读人工核对：`bsend reseed --draft N`（不带 `--yes`，不发种）目视终端输出。

### 建议 S-02（对应 SEC-2）：错误消息不回显 CookieCloud KEY

- **具体做法**：
  1. `cookiecloud.rs::fetch_envelope` 的 `with_context` 只输出 `host`（可加 key 前 4 位指纹），
     不拼接 `/get/{key}` 完整 URL；`fetch_plain` 同理。
  2. CLI `apply_cookiecloud` 改为打印固定的「CookieCloud 拉取失败（见配置 host/key）」
     若 `e.root_cause()` 仍含 URL，则不再打印完整链；简单做法是统一用
     新增的 `cookiecloud::redact_error(&e)`。
  3. GUI 任务打印处保持 `{e:#}`，但底层错误已脱敏即可。
- **affected_files**：
  - `rust/crates/bsend/src/cookiecloud.rs`
  - `rust/crates/bsend-cli/src/main.rs`（`apply_cookiecloud`）
  - `rust/crates/bsend-gui/src/jobs.rs`（或 HEAD 的 `main.rs::pull_cookies_job`）
- **验证方式**：
  - 单测：用不可达 host 构造 `fetch_envelope` 错误，断言 `format!("{e:#}")` 不含 `key` 字符串；
  - `cargo test -p bsend cookiecloud`。

### 建议 S-03（对应 BUG-M7 + BUG-M8）：审阅产物直传发布，并给审阅接节流

- **具体做法**（两件事一起做，因为改的是同一段调用链）：
  1. `reseed.rs` 定义 `pub struct ReviewedRelease { pub info: ReleaseInfo, pub prepared: PreparedUpload, pub preflight: Preflight }`
     （或直接复用 `ReviewReport`，去掉 audit 字段的可选性）。
  2. `run_reseed` 增加可选入参 `Option<&ReviewedRelease>`：有则跳过 `prepare_released` /
     `prepare_upload` / `apply_site_rules`，直接用审阅产物提交；无则保持现有行为
     （CLI `--yes`、GUI 确认按钮仍可独立跑）。
  3. `review_reseed` 增加 `throttle: Option<&SiteThrottle>` 参数，在
     `prepare_released` 的源页抓取、`prepare_upload` 的上传页 GET、`run_pre_audit` 前
     分别 `throttle.acquire(site_id, BudgetKind::Search)`；预算耗尽即返回错误，不静默继续。
  4. `autopost` 把审阅自己的 `throttle` 传进 `review_reseed`，并把审阅产物传给 `run_reseed`。
- **affected_files**：
  - `rust/crates/bsend/src/reseed.rs`（`ReviewedRelease`、`run_reseed`、`review_reseed`、`prepare_released`、`prepare_upload` 调用点）
  - `rust/crates/bsend/src/autopost.rs`（`run_autopost_with` 调用处）
  - `rust/crates/bsend-cli/src/main.rs`（`cmd_reseed` 调 `run_reseed` 传 `None`）
  - `rust/crates/bsend-gui/src/main.rs`（`start_reseed` 传 `None`）
  - `rust/crates/bsend/tests/coverage_e2e.rs`（签名变化后的编译修正）
- **验证方式**：
  - 单测（注入式）：假 runner 收到 `ReviewedRelease`，断言与门禁输入同一份字段；
  - e2e：mock NexusPHP 站统计请求次数，断言每条草稿的源页/上传页/预审各只请求一次；
  - `cargo test --workspace`（执行者运行）。

### 建议 S-04（对应 BUG-M9，已完成）：run_reseed 返回结构化结果

- **当前状态**：工作区已实现 `ReseedOutcome`（`Posted` / `Duplicate(DuplicateOutcome)`），
  带 `audit_result()`（`posted/skipped_duplicate/other_version/existed_pending/failed`）与 `display()`，
  autopost/CLI/GUI 已改接；并有单测 `duplicate_outcomes_map_to_distinct_audit_results`。
  以下为验收口径，保留备查。

- **具体做法**：
  1. 新增 `pub enum ReseedOutcome { Posted { url: String }, Duplicate { status: CoverageStatus, url: Option<String>, note: String }, Uploaded { url: String } }`
     （`Uploaded` 与 `Posted` 可合一，按现有返回值语义取舍；关键是区分 duplicate 的三种 status）。
  2. `run_reseed` 的判重分支按 `Assisted` / `OtherVersion` / `Failed` 分别返回；
     `autopost` 将其映射为 `skipped_duplicate` / `other_version` / `failed` 三种审计 result，
     `failed` 走 6 小时退避、`blocked` 走 24 小时退避。
  3. CLI `cmd_reseed`/GUI `start_reseed` 只做展示，不改变语义。
- **affected_files**：
  - `rust/crates/bsend/src/reseed.rs`
  - `rust/crates/bsend/src/autopost.rs`
  - `rust/crates/bsend-cli/src/main.rs`
  - `rust/crates/bsend-gui/src/main.rs`
  - `rust/crates/bsend/src/autopost.rs` tests
- **验证方式**：
  - 注入式单测：runner 分别返回三种结果，断言审计 result 与 `posted_today` 不变；
  - `doc/DATA-FILES.md` 的 `result` 枚举同步补 `other_version`。

### 建议 S-05（对应 BUG-M10）：加跨进程运行锁

- **具体做法**：
  1. 新增 `bsend::lock::RunLock::acquire(dir) -> Result<RunLock>`：在 `dataDir` 建
     `bsend.lock`，用 `OpenOptions::new().create_new(true)`（或 `fs2::FileExt::try_lock`，
     若接受新增依赖 `fs2 = "0.4"`）；`Drop` 删除/解锁。
  2. `run_autopost` 全流程持锁；拿不到锁直接 `bail!("已有自动发种在跑（锁 {}）")`。
  3. `run_round`/`scan` 的写盘也可持同一把锁（可选，至少 `run` 与 `autopost` 互斥）。
- **affected_files**：
  - `rust/crates/bsend/src/lock.rs`（新增）或 `lib.rs`（若不想新增模块）
  - `rust/crates/bsend/src/lib.rs`（`pub mod lock;`）
  - `rust/crates/bsend/src/autopost.rs`
  - `rust/crates/bsend/Cargo.toml`（仅当用 fs2）
- **验证方式**：
  - 单测：同一目录两次 `RunLock::acquire`，第二次 Err；drop 后可再取；
  - `cargo test -p bsend lock`。

### 建议 S-06（对应 BUG-M11，已完成，无需执行）

- **当前状态**：`aa5a404` 已完成——`append_audit` 内部用 `now_unix()` 覆盖 `at`，
  并有单测 `append_audit_stamps_write_time_not_stale_now`。保留本节只为编号连续与审计追溯。

### 建议 S-07（对应 BUG-M12，已完成）：规则放宽贯穿预检

- **当前状态**：工作区 `preflight(info, tags, rules)` 已按 `SiteRules` 放宽 MediaInfo/链接，
  有单测 `preflight_relaxes_mediainfo_when_rules_allow`。以下为验收口径。

- **具体做法**：
  1. `preflight` 增加 `rules: &SiteRules` 参数：`require_mediainfo=false` 时把「MediaInfo 为空」
     从 `errors` 降为 `warnings`；`require_links=false` 同理。
  2. `prepare_released` 从 `cfg.coverage.reseed_rules.get(target_site)` 取规则传入。
  3. 若不想改语义，则删除这两个配置项并在 README 标明「预检不可放宽」，避免误导。
- **affected_files**：
  - `rust/crates/bsend/src/reseed.rs`（`preflight`、`prepare_released`、tests）
  - `rust/crates/bsend/src/gates.rs`（文档注释）
  - `rust/README.md`（`reseedRules` 说明）
- **验证方式**：单测 `preflight` 带 `require_mediainfo=false` 时 `ok()==true` 且 `warnings` 非空。

### 建议 S-08（对应 BUG-M13，已完成）：限流地板本轮内不清零

- **当前状态**：工作区 `note_ok` 只清 `backoff`、保留 `min_interval`，
  并新增单测 `rate_limit_floor_survives_a_success`；原测试期望改为 59 秒。以下为验收口径。

- **具体做法**：
  1. `note_ok` 只清 `backoff`，**不清** `min_interval`；或引入「连续成功 N 次（如 5 次）且
     距上次限流超过 10 分钟」才清零地板。
  2. 保持 `siteIntervals` 显式配置优先（现有 `base = site_intervals.get(...)` 已满足）。
- **affected_files**：
  - `rust/crates/bsend/src/throttle.rs`（`SitePace`、`note_ok`、tests）
  - `rust/crates/bsend/src/coverage.rs`、`resolver.rs`（仅注释更新，若签名不变可不动）
- **验证方式**：
  - 单测：`note_rate_limited` → `note_ok` → 断言下次 `acquire_dur` 仍 ≥ `rate_limit_floor`；
  - 现有 `rate_limit_backoff_doubles_with_cap` 需按新语义调整。

### 建议 S-09（对应 GAP-3）：把「被排除的站」点名

- **具体做法**：
  1. `Config` 增加 `pub skipped_sites: Vec<(String, String)>`（site id + 原因：
     `framework=Unit3D` / `missing url` / `not in siteIDs` / `pausedSites`）。
  2. `build_adapters` 的回调从「只报缺 cookie」扩为
     `on_notice(&[String] /*missing cookie*/, &[(String, String)] /*skipped*/)`，
     CLI 打 stderr、GUI 进日志。
  3. `bsend sites` 增加一列/一节「未参与原因」。
  4. 同时定夺内置 148 站表：要么在 `framework` 缺失时用 `site_def_by_host` 兜底认框架
     （并在文档写明规则），要么在 README 标明「内置表只是底册/测试用，运行只认配置」。
- **affected_files**：
  - `rust/crates/bsend/src/config.rs`
  - `rust/crates/bsend/src/sites/mod.rs`
  - `rust/crates/bsend-cli/src/main.rs`（`build_adapters` 包装、`cmd_sites`）
  - `rust/crates/bsend-gui/src/main.rs`（`build_adapters` 调用处）
  - `rust/README.md`（内置表口径，**文档部分交 Agent-B**）
- **验证方式**：单测配置含 Unit3D + 缺 url + `siteIDs` 外站点，断言 `skipped_sites` 三条原因准确。

### 建议 S-10（对应 DOC-M3，已完成）：对齐 dataDir 与写入者

- **当前状态**：工作区 `doc/DATA-FILES.md` 已改为「dataDir 未配置时退到配置文件所在目录」，
  并从 `coverage.json` 写入者里删掉 `plan`、注明 `scan` 只写索引。以下为验收口径。

- **具体做法**（二选一，先定产品口径）：
  - **A（推荐，改动小）**：不改代码，改文档——写明「dataDir 缺省 = 配置文件所在目录；
    GUI 版配置通常写 `%LOCALAPPDATA%\BoxSend`」；`plan` 从 `coverage.json` 写入者里删除，
    注明「只写 pieces-cache.json、不写索引」。
  - **B**：在 `config.rs` 增加缺省 dataDir（Windows `%LOCALAPPDATA%\BoxSend`、其它 `~/.boxsend`），
    并补单测；同时保留文档。
- **affected_files**：
  - `doc/DATA-FILES.md`
  - （仅 B）`rust/crates/bsend/src/config.rs` + tests
- **验证方式**：A 靠文档核对；B 用单测断言无 `dataDir` 时的 `coverage_state_path()`。

### 建议 S-11（对应 TEST-M1）：GUI 纯逻辑单测

- **具体做法**：在 ARC-2 拆分完成后，为以下纯函数补测试（不测 GPUI 渲染）：
  1. `snapshot(&Config)`：给定内存索引，断言 stats/sites/drafts 数字；
  2. 配置变换：把 `mutate_config` 里的 JSON 闭包抽成
     `fn toggle_publish(root: &mut Value)` / `fn toggle_site(root, id, enabled)` 等纯函数；
     断言未知键保留、备份名 `.bak-gui`；
  3. 事件归并：`GuiEvent::Snapshot/Log/Busy` 的批处理函数。
- **affected_files**：
  - `rust/crates/bsend-gui/src/jobs.rs`（在途新文件）
  - `rust/crates/bsend-gui/src/types.rs`（在途新文件）
  - `rust/crates/bsend-gui/src/main.rs`
- **验证方式**：`cargo test -p bsend-gui`（Windows；Linux CI 排除 GUI）。

### 建议 S-12（对应 ARC-3，已完成）：给 OtherVersion 明确终态语义

- **当前状态**：工作区已把 `OtherVersion` 加入 `is_settled()`（并有单测
  `other_version_is_terminal`），`rust/README.md` 同步改为「终态、不自动重传、不周期性重查」。
  以下为验收口径。

- **具体做法**：二选一并写进 README：
  - **A**：把 `OtherVersion` 加入 `is_settled()`（永久跳过，需人工删格子才会重查）；
  - **B**：保留周期重查，但 README 改成「OtherVersion 每个 missTTL 会重查一次，
    用于发现站点补种；不自动上传」。
  建议 A（省预算），并在 `doc/DATA-FILES.md` 手改边界注明「删格子 = 重查」。
- **affected_files**：
  - `rust/crates/bsend/src/index.rs`（`is_settled`、tests）
  - `rust/crates/bsend/src/coverage.rs`（预筛注释）
  - `rust/README.md`（**文档部分交 Agent-B**）
- **验证方式**：单测 `is_settled` 列表；e2e 预筛计数（现有 preskip 断言）。

### 建议 S-13（对应 DOC-3 / DOC-M4）：一次改齐文档口径

- **具体做法**：
  1. `autopost.rs` 的 `PostedToday` 注释改成「北京时间（UTC+8）日界」；GUI 两处文案同步。
  2. `rust/README.md` 增加「下载器标签」小节：加种带 `tags=Boxsend`、409 接管追加、
     失败只进备注；`doc/DATA-FILES.md` 在 coverage/审计旁注明标签不落账本。
  3. `rust/README.md` 的自动标签说明补 `国语/粤语/英语/HDR10`（`wanted_tags` 已实现）。
  4. `rust/README.md` 补 `BSEND_MEDIAINFO_DLL` 环境变量（现在只写在 `mediainfo.rs` 注释里）。
- **affected_files**：
  - `rust/crates/bsend/src/autopost.rs`
  - `rust/crates/bsend-gui/src/main.rs`
  - `rust/README.md`
  - `doc/DATA-FILES.md`
- **验证方式**：`rg -n "UTC" rust/ doc/` 仅剩「UTC+8/UTC 日界已改」类说明；文档检索 `Boxsend` 有命中。

### 建议 S-14（对应 DOC-4 / ENG-L3）：根目录导航与合规文件

- **具体做法**：
  1. `README.md` 顶部加三行导航：「当前引擎与构建请看 `rust/README.md`；Swift 版仅存档；
     数据文件契约见 `doc/DATA-FILES.md`」。
  2. `README_副本.md` 建议 `git rm`（需用户确认；它是已入库的旧副本，内容与历史重复）。
  3. 新增 `LICENSE`（MIT 全文）与 `CHANGELOG.md`（从 2026-11-21/22 两批开始记）。
- **affected_files**：
  - `README.md`
  - `README_副本.md`（删除，需用户确认）
  - `LICENSE`（新增）
  - `CHANGELOG.md`（新增）
- **验证方式**：`git ls-files` 检查合规文件；`markdownlint`（结构类规则）。

### 建议 S-15（对应 ARC-4）：原子写统一

- **具体做法**：
  1. `lib::atomic_write` 增加「保留原扩展名」的临时名（`format!("{}.{}.tmp", file_name, pid)`），
     避开多进程同名 tmp 冲突。
  2. `index.save` / `report.write_json` / `config.save_cookies` / GUI `mutate_config` 改调它；
     备份（`.bak`）逻辑留在调用方。
- **affected_files**：
  - `rust/crates/bsend/src/lib.rs`
  - `rust/crates/bsend/src/index.rs`
  - `rust/crates/bsend/src/report.rs`
  - `rust/crates/bsend/src/config.rs`
  - `rust/crates/bsend-gui/src/main.rs`
- **验证方式**：新增 `atomic_write` 单测（父目录不存在、目标存在时覆盖、失败不动旧文件）；
  现有 e2e 全绿。

### 建议 S-16（对应 TEST-L3 / TEST-L4）：补编排与 mock 断言

- **具体做法**：
  1. `qb_server_409` 把 `addTags` 判断放在 `add` 之前并返回 200；e2e 断言：
     `captured` 含 addTags、body 含 `Boxsend`、note 不含「打 Boxsend 标签失败」。
  2. `autopost` 注入式测试补三条：`blocked`、`posted`（断言 `posted_today` 递增与审计行）、
     `skipped_duplicate`（runner 返回空 URL）。
  3. 每条测试用独立 temp 目录（现有 `temp_dir(name)` 已够），避免审计文件互相污染。
- **affected_files**：
  - `rust/crates/bsend/tests/coverage_e2e.rs`
  - `rust/crates/bsend/src/autopost.rs`（tests）
- **验证方式**：`cargo test --test coverage_e2e`、`cargo test -p bsend autopost_`（执行者运行）。

### 建议 S-17（对应 BUG-L10 / BUG-L11）：两处小修

- **具体做法**：
  1. `run_autopost`：`dry_run` 分支不建 `QBittorrent`，或连接失败只记日志不中止
     （dry-run 不需要 qB）。
  2. `search_name_in_results`：返回**全部**候选（或前 3 个），由 `process_item` 逐个下载判定，
     首个 A/B 级即停；预算按候选逐个领号。
- **affected_files**：
  - `rust/crates/bsend/src/autopost.rs`
  - `rust/crates/bsend/src/sites/nexusphp.rs`
  - `rust/crates/bsend/src/coverage.rs`
  - `rust/crates/bsend/tests/coverage_e2e.rs`（若加多候选 e2e）
- **验证方式**：单测 `search_name_in_results` 多候选返回；dry-run 单测用不可达 qB 断言能跑完。

## 5. 优先级与并行性

### 5.1 优先级

| 优先级 | 条目 | 理由 |
| --- | --- | --- |
| P0 | BUG-M7+M8（合并做） | 门禁/发布字段不一致 + 审阅绕过节流，直接破坏安全承诺 |
| P1 | BUG-M10 | 跨进程账本/日限额锁，无人值守的核心保险 |
| P2 | GAP-3、TEST-L4、TEST-L3 | 体验、回归护栏、mock 严谨性 |
| P2 | DOC-3、DOC-M4、DOC-4 | 文档与实现对齐，成本低 |
| P3 | TEST-M1、ARC-4、SEC-2、BUG-L10、BUG-L11、ENG-L3 | 卫生与长期可维护性 |

> 已修条目（SEC-1、BUG-M9/M11/M12/M13、ARC-3、DOC-M3）不再排期，见 §6.1。

### 5.2 写集与并行组

先列写集，再判交并（`并行协作规则`：交集为空才能并行）：

| 组 | 内容 | 写入文件集（affected_files 并集） |
| --- | --- | --- |
| **A（串行链）** | BUG-M7+M8 → BUG-M10 → BUG-L10 → TEST-L4（SEC-1 仅待提交） | `bsend/src/reseed.rs`、`bsend/src/autopost.rs`、`bsend-cli/src/main.rs`、`bsend-gui/src/main.rs`、`bsend/src/lib.rs`/`lock.rs`、`bsend/Cargo.toml`、tests in autopost.rs |
| **B（文档）** | DOC-3、DOC-M4、DOC-4、ENG-L3（+ GAP-3 的 README 部分） | `README.md`、`rust/README.md`、`doc/DATA-FILES.md`、`README_副本.md`（需确认）、`LICENSE`、`CHANGELOG.md` |
| **C（站点过滤核心）** | GAP-3 的 `config.rs` + `sites/mod.rs` 部分 | `bsend/src/config.rs`、`bsend/src/sites/mod.rs` |
| **D（凭据核心）** | SEC-2 的 `cookiecloud.rs` 部分 | `bsend/src/cookiecloud.rs` |
| **E（e2e）** | TEST-L3 | `bsend/tests/coverage_e2e.rs` |
| **F（收尾）** | ARC-4、TEST-M1、GAP-3 的 CLI/GUI 接线、SEC-2 的 CLI/GUI 接线 | `lib.rs`、`index.rs`、`report.rs`、`config.rs`、`bsend-cli/src/main.rs`、`bsend-gui/src/*` |

交集判定：

- A∩B = ∅、A∩C = ∅、A∩D = ∅、A∩E = ∅ → **A 可与 B/C/D/E 并行**。
- C∩F = `config.rs`；D∩F = `cookiecloud.rs` 不冲突但接线在 F；A∩F = GUI/CLI；
  E∩F = ∅ 但 F 会改 `report.rs` → **F 必须最后做**（等 A/C/D 完成），或把 F 拆到各自主人收尾。
- B∩其它 = ∅（只动 Markdown/新文件）→ 可全程并行。

> 文档归属约定：所有 `*.md`（`rust/README.md`、`doc/DATA-FILES.md`、`README.md`、
> `CHANGELOG.md`）的最终落笔统一归 **Agent-B**；涉及**代码注释/GUI 文案**的部分
> （`autopost.rs` 的 UTC 注释、`bsend-gui/src/main.rs` 的限额说明）归 **Agent-A**。
> 因此 S-09 / S-13 / S-14 里标注「文档部分」的文件不构成与 A 的写集冲突，
> 执行时 B 先收齐所有文档改动再统一提交。

### 5.3 推荐派发

1. **Agent-A（编排核心，串行）**：P0 → P1 顺序执行；每完成一条跑一次相关单测。
   不要与主 Agent 当前的未提交改动并发编辑同一文件。
2. **Agent-B（文档）**：改 rust/README + DATA-FILES + 根 README；`README_副本.md` 删除需
   用户确认后再动。DOC-3 的 GUI 文案属于 `bsend-gui/src/main.rs`，**归 Agent-A**。
3. **Agent-C（站点过滤）**：GAP-3 的 `config.rs`/`sites/mod.rs` 核心（产生 `skipped_sites`），
   CLI/GUI 接线留到 F。
4. **Agent-D（凭据核心）**：SEC-2 只改 `cookiecloud.rs` + 单测；CLI/GUI 打印接线留到 F。
5. **Agent-E（e2e）**：TEST-L3 独立。
6. **收尾 F**：ARC-4 + TEST-M1（ARC-2 已拆完，可开始）+ 各接线；由主 Agent 串行统一提交，
   避免同分支并发 push 竞态。

> Git 协同：多 Agent 同仓库改动时，主 Agent 统一串行提交/推送；禁止并行 Agent 各自 push。

## 6. §14.5 遗留与在途修复现状（不计入问题总数）

### 6.1 已修复（审计期间落地，不计入待修数）

> 下列条目均在**工作区**（未提交）或 `aa5a404` 中完成；提交前请以 `git diff` 复核。

- **SEC-1（审阅打印 passkey）**：`reseed.rs` 已加 `mask_field_value`，按字段名接入
  `ReviewReport::render` 与 `dump_fields`；单测 `review_render_masks_secret_field_values` 已补。
- **BUG-M9（空 URL 混淆）**：`ReseedOutcome { Posted, Duplicate(DuplicateOutcome) }` +
  `audit_result()`（posted/skipped_duplicate/other_version/existed_pending/failed）+ `display()`；
  autopost/CLI/GUI 已改接；单测 `duplicate_outcomes_map_to_distinct_audit_results`。
- **BUG-M11（posted 审计时间）**：`append_audit` 用 `now_unix()` 覆盖 `at`，
  单测 `append_audit_stamps_write_time_not_stale_now`。
- **BUG-M12（规则放宽不生效）**：`preflight(info, tags, rules)` 按 `SiteRules` 放宽
  MediaInfo/链接；单测 `preflight_relaxes_mediainfo_when_rules_allow`。
- **BUG-M13（限流地板被清零）**：`note_ok` 只清 `backoff`、保留 `min_interval`；
  单测 `rate_limit_floor_survives_a_success`，原测试期望改为 59 秒。
- **ARC-3（OtherVersion 非终态）**：`is_settled()` 纳入 `OtherVersion`，单测
  `other_version_is_terminal`；`rust/README.md` 同步为「终态、不周期性重查」。
- **DOC-M3（DATA-FILES 失实）**：`doc/DATA-FILES.md` 已改 dataDir 口径、删掉 `plan` 写入者、
  给 auto-audit 的 result 词表补 `other_version` / `existed_pending` / `note`。

### 6.2 `aa5a404` 已收口的 §14.5 遗留（残余另列）

- **TEST-H1（编排层 e2e）**：已抽出 `run_autopost_with` 并加 5 条零网络测试（白名单、日限额、
  退避、过滤+limit、审计不可写）。**残余**：`posted/blocked/skipped_duplicate` 三条记账路径
  （见 TEST-L4）、`run_reseed` 全链路（判重→上传→发后自检→推种→校验）仍无 e2e。
- **BUG-L8（cmd_sites 直读 JSON）**：已改为 `Config` 合并视图 + `bsend::url_host`。
  建议补一条断言「cookie 计数与 `--cookiecloud` 合并视图一致」的单测（当前无）。
- **ENG-M2（release 流水线）**：已新增 `.github/workflows/rust-release.yml`（tag 触发，
  CLI+GUI+校验和）。**残余**：版本号事实来源与 CHANGELOG（见 ENG-L3），以及首次 tag 实跑未验证。
- **ARC-2（GUI 单文件）**：已拆出 `types.rs`（视图状态）与 `jobs.rs`（后台任务），
  `main.rs` 仍约 1,500 行、`render_*` 未按 Tab 再拆。**残余**：GUI 测试（TEST-M1）。
- **名字检索追加成员种名**：已实现 `assist.memberNameFallback`（缺省开，只在多成员内容上
  用「最大且与内容名不同」的成员名再查一次，照常领号）+ 单测 `pick_member_fallback`。
  它**不解决** BUG-L11（检索命中首个候选即返回、不在多候选间择优），两者互补。

## 7. 执行者开工卡

1. `git status --short` + `git diff` 确认主 Agent 当前写集与已修条目（§6.1）；
   **不要**与未提交改动并发编辑同一文件。
2. 先做 P0：S-03（BUG-M7+M8，一组文件）。
3. 再做 P1：S-05（M10）。P2：S-09（GAP-3）、S-16（TEST-L3/L4）、S-13（DOC-3/DOC-M4）。
4. 每条改完跑对应单测；全量验证由主 Agent 在写集空闲时统一跑
   `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`。
5. 全程不真发种：用 `--dry-run`、`formcheck`、`gatecheck`、mock e2e；不打印任何凭据值。
6. 收尾时同步 `rust/README.md` / `doc/DATA-FILES.md` / `CHANGELOG.md`，并做一次
   `markdownlint`（结构类规则）检查。

## 8. 夜间执行记录（2026-11-22 01:00–）

> 本节由执行 Agent 追加，记录本报告的销号情况；未列出的条目仍未处理。

### 8.1 已修复（含验证）

| 条目 | 修复要点 | 验证 |
| --- | --- | --- |
| SEC-1 | 审阅 `render`/`dump_fields` 按字段名脱敏（passkey/token/auth/cookie/secure/uid/secret/password） | 单测 `review_render_masks_secret_field_values` |
| SEC-2 | CookieCloud 错误只报 host + 失败类型，不拼 `/get/{key}` | 单测 `fetch_error_does_not_leak_key` |
| BUG-M8（半） | `review_reseed` 接 `SiteThrottle`：源页/上传页/官方预审都领号 | 编译 + 全量测试 |
| BUG-M9 | `ReseedOutcome` 结构化；审计区分 posted/skipped_duplicate/other_version/existed_pending/failed | 单测 `duplicate_outcomes_map_to_distinct_audit_results` |
| BUG-M10 | `bsend::lock::RunLock`（create_new + 2h 陈旧接管）；`run_round`/`run_autopost` 共用 `bsend-state.lock` | 单测 `second_acquire_fails_until_drop` / `different_names_do_not_collide` |
| BUG-M11 | 审计 `at` 取写入时刻 | 单测 `append_audit_stamps_write_time_not_stale_now` |
| BUG-M12 | `preflight` 接受 `SiteRules`，requireLinks/requireMediaInfo 放宽降为警告 | 单测 `preflight_relaxes_mediainfo_when_rules_allow` |
| BUG-M13 | `note_ok` 只清退避倍数，不清限流地板 | 单测 `rate_limit_floor_survives_a_success` |
| GAP-3 | `Config.skipped_sites` + `skipped_notice()`，CLI/GUI 点名被排除的启用站 | 单测 `skipped_sites_are_reported_with_reason` |
| DOC-M3 | DATA-FILES 的 dataDir 缺省/写入者修正（plan 不写账本） | 人工核对 |
| DOC-3 | 日界文案统一北京时间（代码注释 + GUI 两处） | grep |
| DOC-M4 | README 增加 `Boxsend` 标签说明 | 人工核对 |
| TEST-M1 | GUI 配置写回抽 `config_io::save_config_with_backup` | 单测：保留未知键/备份/坏 JSON 不落盘 |
| ARC-3 | `otherVersion` 纳入终态 | 单测 `other_version_is_terminal` |
| ARC-4 | index/report/config 统一复用 `lib::atomic_write` | 全量测试 |
| BUG-L10 | `autopost --dry-run` 不再要求下载器在线 | 单测 `dry_run_does_not_require_downloader` |
| TEST-L3 | e2e mock 补 `addTags` 分支；409 接管断言补挂 tracker + 补 Boxsend 标签 | `e2e_already_exists_attaches_tracker` |
| BUG-L11 | 多候选择优：`search_name_in_results` 按「精确 > 候选含发布名 > 发布名含候选 > 词元容忍」打分，同档比覆盖比例 | 单测 `search_prefers_closest_candidate_among_matches` |
| TEST-L4 | 编排层注入 reviewer：posted/blocked 记账分支可零网络验证 | 单测 `autopost_writes_posted_and_blocked_audit` |
| GAP-4 | `autopost --dry-run` 空白名单时逐条门禁演练（此前直接全跳过，与提示文案不符）；演练下 `--limit` 改为限制审阅条数 | 单测 `dry_run_with_empty_whitelist_drills_all_drafts` / `dry_run_with_configured_whitelist_skips_unlisted_site` / `dry_run_limit_caps_reviewed_drafts` |
| BUG-M14（新发现） | 站内判重命中（同分片）/ 上传回包 existed 时，也必须下载目标种推入下载器（补挂 tracker 或 NoSubfolder 复用目录）并校验；此前只写 Assisted/待确认，qB 里没有目标站 tracker | e2e `e2e_duplicate_hit_pushes_target_torrent_to_downloader` / `e2e_upload_existed_pushes_target_torrent_to_downloader` |
| BUG-M15（策略） | 同 hash 绝不改动原种子：qB 不允许同 hash 两条，改为提前判掉、不发 add、不补挂 tracker/标签，格子记 `same_hash_skip` | e2e `e2e_same_hash_skips_without_touching_existing_torrent` |
| ENG-M4（新发现） | 新增 `reseed-log.jsonl`：CLI/GUI/autopost 每次转种都落一行（时间/源/目标/结果/`seeded`），解决「站点发了、qB 没辅上」无据可查 | e2e 路径实跑 + DATA-FILES 契约 |
| BUG-M16（新发现） | GUI 双击无窗口：默认配置路径改为「当前目录 → `%LOCALAPPDATA%\BoxSend\boxsend.json`」，加载失败弹 Windows 错误框 | GUI 单测 `config_path_prefers_arg_then_cwd_then_local_appdata` + 双击模拟（可见窗口进程存活） |
| ENG-L3 | 新增根 `LICENSE`（MIT）与 `CHANGELOG.md` | markdownlint |

### 8.2 仍未处理（已于 11-22 下午补齐，见 8.6）

- ~~**S-03 的 BUG-M7**：审阅产物直传发布（门禁字段与提交字段一致性）。~~
  已实现 `ReviewReport.carry` + `run_reseed_reviewed`，autopost 门禁通过后直发。
- ~~**S-16** 其余项：mock 级 HTTP 的 posted/blocked 断言。~~
  已补 `e2e_reviewed_release_reuses_fields_without_refetching`（posted）与
  `e2e_site_rejected_upload_marks_failed_and_skips_downloader`（站点写接口 blocked）。

### 8.3 验证基线

- `cargo fmt` 通过；`cargo clippy --workspace --all-targets -D warnings` 零告警；
  `cargo test --workspace`：**191 单测 + 22 e2e + 2 CLI + 3 GUI** 全绿
  （11-22 下午更新为 **196 单测 + 24 e2e + 2 CLI + 3 GUI**，见 8.6）。
- 实机只读演练：`bsend autopost --dry-run --sites sbpt --limit 20`
  → 20 条门禁通过、5 条拦截、1 条目标站上传页瞬断（50x/10054，可重试）、
  1 条退避跳过；未真实发种。
- `formcheck` 覆盖 101 个启用站（104 条历史结论）；4 条 gap 均为**站点限制或样本不适配**：
  hdcity（JS 驱动上传表单，已加入 `reseedPausedSites`）、
  ptfans/musopia（站点无 HDTV 媒介选项或音乐站样本不适配）、
  muxuege（AAC 同名多选项，留空不猜）。

### 8.4 收尾更正与清理待确认

- `1.webp` **不是**无用资源：`scripts/make-app.sh` 用它生成 macOS 应用图标，
  2026-11-21 报告中的「待确认删除」结论作废。
- 清理候选（未动，等用户确认）：根 `README_副本.md`（49 KB）、
  `%LOCALAPPDATA%\BoxSend\startup.log`（184 MB）、
  `coverage.json.bak-*` 19 份（350.5 MB，建议只留最近 2 份）。
- 明日真发交接见 `doc/NIGHT-SHIFT-2026-11-22.md`。

### 8.5 站点适配复测（11-22 中午，只读）

- `bsend sites --json`：147 站｜启用 101｜暂停 13｜转种暂停 35｜有 cookie 120；
  drill 结论：ok 54 / fail 46 / gap 4 / 未测 43。
- 4 个 gap：hdcity（简介为空，JS 表单，已暂停转种）、musopia（`medium_sel` 未选）、
  muxuege（`audiocodec_sel` 未选）、ptfans（`medium_sel` 未选）。
- 本轮抽测 3 站 `formcheck`（只读、不 POST）：audiences / baozi / crabpt
  **全部表单齐（3/3）**，无硬缺口；仅有 `team_sel`、`processing_sel`、`codec_sel`
  等同名下拉的软提示。
- GUI 修复：双击启动默认回退 `%LOCALAPPDATA%\BoxSend\boxsend.json`，加载失败弹
  可见错误框；模拟双击（工作目录 `C:\Windows\Temp`）进程存活且有可见窗口。

### 8.6 当轮补齐（11-22 下午，串行）

- **BUG-M7 审阅产物直发**：`ReviewReport` 携带审阅时的源/目标适配器与源页（`carry`），
  autopost 门禁通过后调用 `run_reseed_reviewed`；提交字段与门禁判定同一份，源页/上传页/
  官方预审均不再二次请求。CLI 单发与 GUI 确认仍走 `run_reseed` 现场构造，两条路径共用
  `publishEnabled` 总闸。守卫：站点不一致/缺载荷直接拒绝；审阅官方预审未过直接拦。
- **S-16 站点写接口级断言**：
  - `e2e_reviewed_release_reuses_fields_without_refetching`：审阅后直发只 POST 一次
    `takeupload`，multipart 含审阅的全部非空字段；源站详情页与上传表单请求数不增；
    发布后格子 `reseeded`。
  - `e2e_site_rejected_upload_marks_failed_and_skips_downloader`：`takeupload` 返回
    errorbox → 发布报错保留原文、不重投、不推种、不擅改格子（失败由编排层记审计）。
- **P1 只读巡检**：`scripts/seeding-audit.ps1` + `doc/SEEDING-AUDIT.md`。实跑本机：
  覆盖率异常 144 格（failed 41 / wait_source 54 / reseed_draft 29 等），
  qB 5.3 本地免密登录（204 空体）已兼容；无凭据输出。
- **P2 站点矩阵**：`scripts/site-matrix.ps1` + `doc/SITE-ADAPTATION-MATRIX.md`。
  147 站｜启用 101｜全停 13｜停转种 35｜drill ok 54 / fail 46 / 未测 47；
  formcheck 最新 ok 54 / gap 4 / fail 46 / 未测 43。最大同类失败：
  27 站账号无发布权限，7 站无可解析静态上传表单。
- **P3 纯函数单测**：`models.rs` +3（hash 归一/目录推导/JudgeLevel 序列化），
  `manifest.rs` +2（Unicode/空清单/大小写），`bencode.rs` +1（异常长度/空 files）。
  单测 191 → 196。
- **验证基线（本轮）**：`cargo fmt` 通过；`cargo clippy --workspace --all-targets
  -- -D warnings` 零告警；`cargo test --workspace`：**196 单测 + 24 e2e + 2 CLI + 3 GUI**
  全绿。未真实发种、未改用户配置、未 push。
