# bsend（Rust 铺种引擎）

Swift GUI 版 box-send 的铺种引擎重写：把「已在做种的种子自动去重建索引 → 去各目标站辅种 →
站内没有的转种（v1 为草稿模式）」做成一条可单独运行的 CLI 流水线。检索启发式、判定级别、
推送与校验尾巴、配置格式全部从 Swift 版逐条移植（见 `COVERAGE-PLAN.md` 方案文档）。

## 构建

```bash
cd rust
cargo build --release
# 产物：target/release/bsend.exe（或 Linux 下的 bsend）
```

## 用法

```bash
# 只扫描 qBittorrent 建索引（不动任何站点）
bsend --config /path/to/boxsend.json scan

# 预演一轮：检索照跑、结论照出，但绝不碰下载器
bsend run --dry-run   # 结论会落盘（下轮按终态预筛）
bsend plan            # 完全无痕：结论也不落盘

# 正式跑一轮辅种 + 转种草稿（结论落盘 coverage.json）
bsend run
bsend run --sites pter,hdsky     # 只跑指定站
bsend run --limit 10             # 小规模试跑

# 看索引现状：内容数、各站覆盖分布、待发草稿、待找源清单
bsend status
```

`--config` 默认读当前目录的 `boxsend.json`。每轮建议用 cron / 计划任务低频驱动；
终态格子会被预筛跳过，增量轮次的花费很小。

## 配置：与 GUI 版同一份

直接读 Swift GUI 版写的 `boxsend.json`，无需另配：

- 参与站点 = `sourceSites` 里 enabled 的站 ∩ 辅种设置（`assist.siteIDs`，空 = 全部）
  − `pausedSites`，且框架为 NexusPHP（v1 只有这套检索启发式）。
- 下载器 = `assistDownloaderID` 指向的那台（缺省、或找不到指定 id 都退回主下载器）；
  沿用它的 savePath / 分类 / 限速（含 `siteUpLimits` 逐站表）。推送固定「先暂停等校验」，
  不读 skipChecking（方案 §七）。
- Cookie 读同目录 `cookies.json`（PT-depiler 备份格式，支持 `{"cookies":{...}}` 外层包裹）。

## 一轮流水线

1. **扫描合并**：qBittorrent 全量种子 → 按 v1/v2 info hash ∪ 文件清单摘要合并进内容索引；
   消失的种子记 `goneSince`，30 天后归档。没下完的种子跳过（文件不全没法比）。
2. **已做种预筛**：tracker host 命中站点 announce host（含子域，tracker.xxx.com 对
   xxx.com 也算）的格子直接标 `seeding`，零请求。
3. **辅种臂**（站内检索）：严格查询 → 未命中且开了宽松重试再来一轮 → 命中后下
   .torrent 逐文件判定（A=hash 一致 / B=重新打包清单全等 / C=只有名字像）。
   A、B 且目录布局一致 → 暂停添加 → 轮询校验 → 满 100% 才启动（`assisted`）；
   不满则按 `assist.removeOnVerifyFail` 删种保文件（`failed`）或保持暂停。
   轮询等到头还没结论 → 落 `verifying`（记下那条种子的 hash）；下一轮开头的
   收尾 pass 凭它直查下载器收尾，不花站点请求。C 级与布局对不上的 → `reviewPending`，
   绝不碰下载器。
4. **转种臂**（站内没有）：定位源种（已在做种的源站优先）→ v1 只出草稿
   （`reseedDraft`：源站 + 详情页链接 + 目标站清单），上传是后续版本的事。
   哪都没有 → `waitSource` 并记 30 天负缓存；检索报错（cookie 失效——含 401/403 与
   被跳到登录页——站点 5xx）标 `failed`，不记负缓存。

## 预算与节流（方案第九节）

- 三类独立预算，每轮重置：目标站检索 200 / 源站定位 200（两个都读 `assist.roundBudget`，
  与 GUI 版同一个旋钮）/ 转种上传 10（Rust 侧固定）。预算耗尽的格子本轮直接跳过、
  不记任何结论——「没查成」不能当成「查过没有」，下轮预算回来从头查。
- 每站串行（默认 8s 间隔 + 3s 抖动，`assist.perSiteIntervalSec` / `assist.jitterSec`），
  全局 worker 上限 4，替代 Swift 版的全流水线单线程。
- 站点报限流（HTTP 429/503 自动识别）→ 该站间隔翻倍退避（封顶 240s），
  一次成功请求恢复。
- 重试节流：`miss` / `waitSource` / `reviewPending` / `failed` 都歇
  `assist.missTTLDays` 天（默认 7）再重查；源种「定位不到」的负缓存 30 天
  （Rust 侧固定）。校验收尾 pass 不占任何预算。
- 源种负缓存 30 天、归档 30 天、每轮并发 4、转种草稿模式：均为 Rust 侧固定值，
  v1 不读配置。

## coverage.json

落盘在 `dataDir/coverage.json`，原子写（tmp + rename），camelCase；结构：

- 每段内容一条：名字、总大小、文件清单摘要、全部 info hash（多个站同一内容合并成一条）、
  各站覆盖格子（status / at / url / note）、源种记录（siteId + detailUrl，或 missAt 负缓存）。
- 状态机：`seeding / assisted / reseeded / verifying / reviewPending / rejected /
  miss / waitSource / failed / reseedDraft`。`seeding / assisted / reseeded /
  rejected / reseedDraft` 为终态，不会再产生请求；`verifying` 由下一轮收尾 pass
  接管（凭格子里的 verifyHash 直查下载器）；其余四类歇一个 missTTL 再重查。

## 与 Swift 版的关系

- 移植：NexusPHP 检索启发式（查询词阶梯 / 归一化互含 / 词元宽容判定）、A/B/C 判定、
  布局门、推送尾巴（暂停添加 → 校验 → 启动/回滚）、qB WebAPI 兼容补丁（参数名两版、
  回包两种包装、409 已存在补挂 tracker、登录一次复用）、登录墙检测（401/403/被跳到
  登录页 → 按 cookie 失效报错，绝不落「站内没有」负缓存）、148 站识别表。
- 未移植（v1 范围外）：GUI、上传表单填充（转种只出草稿）、非 NexusPHP 框架的站点适配、
  Transmission。

## 测试

```bash
cargo test          # 54 个单测（判定/分词/节流/索引/配置与 cookie 兼容/限流判定）+ 11 个端到端
                    #（mock qBittorrent + mock NexusPHP 走真实 TCP HTTP 跑完整一轮：
                    #  辅种全流程、已做种预筛、源站优先、负缓存、dry-run、C 级人工确认、
                    #  409 已存在补挂 tracker、校验中跨轮收尾（启动/回滚两分支）、
                    #  预算耗尽不记负缓存、登录墙不落负缓存、429 限流映射失败）
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

CI 在 `.github/workflows/rust.yml`（Ubuntu + Windows 双矩阵）。
