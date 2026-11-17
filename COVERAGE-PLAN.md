# 辅种方案：内容索引 → 源种定位 → 转种/辅种一体化（v1 草案）

目标一句话：**下载器里已有的文件，自动去重建索引；逐站判断「站内有就辅种、站内没有就转种」，
把每一段文件铺满所有参与站点。** 不再需要手动贴源站链接——源种由软件自己去源站找回。

与现有两条线的关系：

- **转种（`ReseedPipeline`）**：输入是人工粘贴的源站详情页链接。辅种要把它变成「自动定位源种」。
- **辅种（`Assist/`，M0–M2 已落地、1.3 起 `AssistFeature.enabled = false` 收起）**：
  扫描下载器 → 站内检索 → 判定 → 暂停添加 → 校验复核。辅种复用它的全部判定与推送机制，
  但把「一条种子」升级成「一段内容」，并在「站内没有」时不再收工，而是接着转种。

## 一、现状差距

| 想要的 | 现状 | 差距 |
| --- | --- | --- |
| qB 做种清单自动去重建索引 | `AssistPipeline.dedupe` 每次运行临时按 hash+名归一化去重；`assistScan` 只记摘要 | 没有持久的内容级索引：同一内容多条种子各自记账、换代表种子后历史结论对不上 |
| 去源站找对应源种 | 无。源站详情页靠人贴链接 | 需要新的「源种定位」步骤：站内检索 + A/B 判定 + 取回 `ReleaseInfo` |
| 站内没有就转种 | 转种与辅种是两条独立链路，互不衔接 | 需要统一决策：同一格子里先判辅种、判不上再走转种 |
| 转种安全 | `ReseedPipeline` 人工触发，天然有人把关 | 自动转种必须补闸门（草稿确认、站点策略、禁转标记、壳页拦截） |

## 二、三个概念

- **内容（Content）**：一段具体的文件集合。身份不依赖任何站点：v1 hash、v2 hash、
  逐文件清单摘要（manifest），三者任一相同即同一内容（并查集合并）。
  不同压制（同片不同 encode）manifest 不同 → 天然是两个内容，不会被误合并。
- **覆盖（Coverage）**：内容 × 站点 的一个格子，结论是下面第五节决策表之一。
- **源种（Source）**：某个源站上该内容的那条种子：详情页 URL + 解析出的 `ReleaseInfo` + .torrent。
  转种的上传材料（简介/MediaInfo/截图/IMDb/豆瓣/标签）只能从源站详情页拿到，
  本地 .torrent 文件本身不够——这就是「必须去源站找」的原因。

## 三、主流程

```
qBittorrent（已完成种子）
 │ 1 扫描 → 内容索引（新 torrent 才读文件清单，增量合并）
 │
 ├─ 2 已覆盖标记：tracker hosts ∩ 参与站点 → 这些格子直接判「已在做种」
 │
 ├─ 3 逐格决策（内容 × 站点，站点各自异步推进，站内保持间隔）：
 │     ├─ 预筛命中（已做种/已辅/已转/负缓存/人工否决/筛选）→ 跳过，零请求
 │     ├─ 站内检索（复用转种的多级查询词 + 宽松兜底）
 │     │    ├─ 命中且 A/B 级 → 辅种臂：下载站点 .torrent → 统一推送尾巴
 │     │    ├─ 命中但 C 级 → 待人工确认（沿用现队列）
 │     │    └─ 未命中 → 转种臂（需站点策略允许）：
 │     │         ├─ 源种已定位 → 上传 → 取回新种 → 统一推送尾巴
 │     │         ├─ 源种未定位 → 先走源种定位（第四节），成功再上传
 │     │         └─ 源站也没有 → 记「无源可转」，进入待找源清单
 │     └─ 校验复核 / 回查（沿用辅种那套：满 100% 启动，不满按策略回滚）
```

## 四、内容索引（去重的核心）

### 身份与合并

`ContentKey` 的构成（并入查集）：

1. v1 info hash（qB `hash`）；
2. v2 info hash（qB `infohash_v2`，混合种子两者都有）；
3. manifest 摘要：对 `AssistMatcher.comparableKey` 归一化后的逐文件
   `"路径|字节数"` 键排序后做 FNV-1a（复用 `AssistPipeline.digest` 的稳定哈希写法，
   不能用 `hashValue`）。各站重新打包（改 source/private 标记）hash 不同、manifest 全等 → 合并。

现有 `dedupe` 的「名归一化 + 体积容差」只用于**展示聚类**，不参与内容身份：
同名不同压制（1080p vs 2160p）文件清单必不同，靠 manifest 分开；名字被站点改写但内容相同，
靠 manifest 合上——两条路都不需要名字。

### 扫描与增量

- `listTorrents` 全量拉（一次请求）；对照索引：新出现的 hash 才调 `torrentFiles(hash:)`
  算 manifest（首扫描全量较贵，可后台分批；之后只有新种子要算）；
- 种子消失：从所属内容条目里摘除；条目空了不立刻删——保留 30 天（可配）再归档，
  保住「已辅/已转」记录（同内容重新入下载器时不用从头铺）；
- `savePath`/分类变化：更新条目。

### 落盘

独立 `coverage.json`（与 `state.json` 分开：索引条目多，不该把每几秒重写一次的
state.json 再撑大）。原子写、后台节流落盘。条目结构：

```jsonc
{
  "contents": {
    "<contentKey>": {
      "name": "…", "totalSize": 0, "fileCount": 0,
      "manifestDigest": "fnv1a…",            // 不存逐文件清单，判同比对时现算
      "torrents": { "<v1hash>": {"savePath": "…", "category": "…", "lastSeen": 0} },
      "trackerHosts": ["…"],                  // 各条种子的并集
      "firstSeen": 0,
      "coverage": { "<siteID>": {"status": "assisted|reseeded|seeding|miss|…", "at": 0, "url": "…"} },
      "source": { "siteID": "…", "detailURL": "…", "resolvedAt": 0, "missAt": 0 }
    }
  }
}
```

`coverage.status` 的格子即「已在做种/已辅/已转/站内没有/无源可转/待人工/失败」，
是对现有 `assisted`/`assistMiss`/`assistFail`/`uploaded`/`existing`/`targetURL`
六张表的**内容级收拢**：旧表按 `站点|本地hash` 记账，本地代表种子一换就对不上号；
新表按 `站点|contentKey` 记，换哪条种子做代表都成立。旧辅种表保留不动（那条线还收着），
辅种是新表新账。

## 五、源种定位（SourceResolver）

对一段内容，在**源站名单**（`coverage.sourceSiteIDs`，默认取转种源站分组）里找回一条
可用于转种的源种：

1. **优先级 1 —— 已在做种的源站**：`trackerHosts` 里已有某源站的 announce host，
   说明本地就挂着那站的种子。只在那一个站检索（1–3 次请求），命中后 `fetchDetail`。
2. **优先级 2 —— 源站名单按序检索**：复用 `searchExists` 的多级查询词与宽松兜底，
   逐站尝试（受源站预算与间隔约束）。
3. **判定复用 `AssistMatcher.judge`**：下载候选 .torrent，A 级（同 hash）或 B 级
   （manifest 全等）才算定位成功；C 级名字像但清单对不上 → 不采，换下一站。
4. 成功 → `content.source = {siteID, detailURL}`，并 `fetchDetail` 缓存 `ReleaseInfo`
  （含 `isForbidReseed` 检查）；失败 → 记 `source.missAt` 负缓存（TTL 更长，默认 30 天：
  源站新发种比目标站慢，没必要常回去问）。

**转种不成立时的硬规则**：源种命中 `forbidReseedMarkers`（禁转标记）→ 该内容永远
不进转种臂，辅种臂不受影响（辅种不算转载发布）。

## 六、统一决策表（内容 × 站点）

| 判定 | 依据 | 动作 |
| --- | --- | --- |
| 已在做种 | tracker hosts 含该站 | 零请求跳过 |
| 已辅过 / 已转种过 | `coverage` 格子有终态 | 零请求跳过，重跑不重复 |
| 待复核 | 上一轮推送还没等到校验结果 | 零请求跳过，走「回查校验」 |
| 人工否决 | 人工判过不是同一内容 | 零请求跳过（可撤销） |
| 站内检索命中 A/B | `judge` 同 hash 或 manifest 全等 | **辅种臂**：现有推送流程原样 |
| 站内检索命中 C | 名字像、清单对不上 | 待人工确认（现有队列，按内容分组展示） |
| 站内没有 + 站点策略允许转 + 有源种 | 检索未命中、`source` 已定位 | **转种臂**：上传 → 取回新种 → 统一推送尾巴 |
| 站内没有 + 不允许转 | 站点策略 assistOnly | 记 `miss` 负缓存（默认 7 天） |
| 站内没有 + 允许转但无源种 | `source` 未定位 | 先源种定位；仍无 → 记「无源可转」，进待找源清单 |

顺序上**先检索后转种**：同一次站内检索的结果两用——命中走辅种，未命中才触发转种臂
（连带可能触发源种定位）。站点预算分类记账：`sourceSearch` / `targetSearch` / `upload`
各一份，转种预算默认远紧于检索（见第八节）。

## 七、统一推送尾巴（转种与辅种收尾相同）

现有转种推送假设「盒子是空的，推了就下」（`reseedDownloader.savePath` + `skipChecking`）。
辅种的前提是**文件已经在辅种那台下载器上**，所以两条臂共用辅种那套保险丝：

- `addTorrent(…, savePath: 本地内容的 savePath, paused: true)`（不是转种下载器的默认路径）；
- 轮询校验 → 满 100% 才 `resume`；不满按 `removeOnVerifyFail` 回滚（保留文件）；
- `AssistMatcher.layout` 检查同样作用于转种取回的 .torrent：内层目录名对不上就不硬加；
- 大小检测（`SizeGuard`）对辅种**不适用**——文件已在盘上，不存在「下不下得动」的问题；
- 站点刚发种、下载入口未就绪（实测 hdtime 要等几秒）：沿用 `pushRetryWaits`（15s/60s），
  再取不到就记「待取回」，下一轮只补取回，不重发种。

转种成功同时在格子记 `reseeded` + `targetURL`（新种详情页），与 `ReseedPipeline`
的 `markUploaded`/`markExisting` 语义一致；站方回「已存在」（`alreadyExists`）不失败，
直接转辅种臂处理那条已有种子。

## 八、转种安全闸门（与辅种最大的不同，宁可慢不可错）

自动发种的风险是账号级的（错分类、违禁转、垃圾简介都会吃审查），所以：

1. **草稿模式默认开**：转种臂只生成「待发清单」——源 `ReleaseInfo` 摘要 + 目标站
   字段预览（`previewUploadFields`，适配器已实现）+ 预计简介，人逐条或批量确认后才真上传。
   全自动需要逐站在策略里显式开。
2. **站点策略三级**：`coverage.sitePolicies[siteID] = assistOnly | draft | auto`，
   默认 `draft`；没把握的站（强审核、字段怪）标 `assistOnly`。
3. **硬拦截**（任何模式下都不发）：源种 `isForbidReseed`；`sourceCompletenessWarning`
   非空（壳页转出去就是垃圾简介）；`judge` 只有 C 级（清单都对不上，没有转的资格）。
4. **上传频率**：转种预算独立（默认每轮 ≤ 10 次上传），且逐站上传间隔 ≥ 检索间隔。
5. **首转保护**：一个站前 N 条（默认 5）自动转种失败 ≥ 2 次时，该站自动降级回草稿模式，
   并在界面上说明——适配器字段映射对该站可能有坑，别硬撞。

## 九、成本控制与并发

**请求数**：内容索引把「同一内容 N 条种子」收敛成一份格子，是最大的一刀。此外：

- 三类预算（源站检索 / 目标站检索 / 上传）独立记账，单轮上限默认 200 / 200 / 10；
- 负缓存双层：目标站 miss 7 天（可配），源站 miss 30 天（可配）；
- 命中即停：A 级命中后不再跑剩余查询词（现有逻辑）。

**墙钟时间（当前最大的结构性问题）**：`AssistPipeline.run` 是全串行
（站点外层 × 种子内层）。1,000 个内容 × 20 站、每站间隔 8s+抖动 ≈ 每站 2.5 小时、
串行 20 站 ≈ 50 小时。辅种必须改成**站点 worker 模型**：每站一个串行队列（保持本站节奏），
全局并发上限（默认 4 站同时），墙钟时间降到最慢那一站。这个改造在 Swift 里用
`DispatchQueue` + 信号量即可完成，不需要换语言。

**优先级**：默认「未覆盖站多的内容优先、新入库优先」，可配「体积倒序」（沿用
`roundCandidates` 的取法）。

**可选加速（默认关，隐私自担）**：接入 IYUU 一类哈希索引服务——拿 infohash 问
「哪些站有同 hash」，直接省掉目标站检索、只需取回验证。它把你的种子清单交给第三方云端，
必须 opt-in 且在界面写明。

## 十、状态与配置

| 位置 | 加什么 |
| --- | --- |
| `StateStore` / 新 `CoverageStore` | `coverage.json` 读写、原子落盘、`coverage.json` 与 `state.json` 的并发写互不干扰（各自锁） |
| `AppConfig.coverage` | `CoverageConfig`：`sourceSiteIDs`、`siteIDs`、`sitePolicies`、三类预算、两个负缓存 TTL、草稿默认开、索引归档天数、并发站数、`autoRunMinutes`、优先级策略、IYUU 开关 |
| 旧辅种表 | 原样保留；`AssistFeature.enabled` 何时翻回 true 独立决策（辅种 C2 上线后辅种页可由辅种页收编） |

## 十一、代码接缝

| 位置 | 加什么 |
| --- | --- |
| `Assist/ContentIndex.swift`（新） | `ContentKey`、并查集合并、扫描增量（新 hash 才读文件清单）、`coverage.json` 落盘、条目生命周期。纯逻辑，表驱动单测 |
| `Assist/SourceResolver.swift`（新） | 优先级定位（trackerHost 站优先）、A/B 判定、`ReleaseInfo` 缓存、源站负缓存。适配器注入，假站单测 |
| `Assist/CoveragePipeline.swift`（新） | 编排：索引 → 预筛 → 站点 worker 池 → 检索 → 辅种臂/转种臂 → 统一尾巴 → 回查。复用 `SiteThrottle`（加预算分类）、`AssistMatcher`、`AssistQueue`、`AssistRowStatus`（新增 `waitSource`/`reseedDraft`/`reseeded` 三态） |
| `Pipeline/ReseedPipeline.swift` | 抽出「单站上传」段为 `uploadOne(target:release:torrentData:) -> UploadOutcome`（现 `run` 内联在循环里），辅种与手动转种共用；`run` 行为不变 |
| `Downloader` 协议 | 不动（`listTorrents`/`torrentFiles`/`addTorrent(paused:)` 都已齐） |
| `Sites/*` | 不动。`searchExists`/`fetchDetail`/`downloadTorrentFile`/`upload`/`previewUploadFields` 正是这条链需要的全部 |
| `Bridge/AppService.swift` | `coverage.scan` / `coverage.index` / `coverage.plan` / `coverage.start` / `coverage.confirm`（草稿批量确认）/ `coverage.recheck` / `coverage.stop` / `coverage.tick`，照 `assist.*` 的写法：立即返回、后台跑、`snapshot` 带回 `coverage` 段 |
| `BoxSendCLI/main.swift` | `box-send coverage scan\|plan\|run [--sites a,b] [--limit N] [--dry-run] [--auto-reseed]` |
| mac 界面 | 「辅种」新标签（或收编辅种页）：索引统计、待找源清单、草稿确认列表、逐站进度 |
| Windows 界面 | `Views/CoverageView.xaml`，同一套信息结构，桥方法同名 |
| Web 控制台 | `ServeApp` 加 `/api/coverage/*`（只读统计 + 草稿确认），盒子远程可看 |

## 十二、界面（信息结构，两端一致）

- 顶部：扫描时间、内容数/种子数（去重前后）、三预算余量、「约 N 分钟后自动跑下一轮」
- 内容表：名称、体积、覆盖 N/参与站 M、已在做种站、源种（站名 + 链接）；按未覆盖数/体积/新入库排序
- 逐站卡片：已判定 / 还要检索 / 待发草稿 / 失败，点开是格子列表
- 草稿确认列表：源种摘要 + 目标站字段预览 + 差异说明，单条/批量「发」或「弃」
- 待找源清单：源站都检索不到的内容，注明「等源站新发种，30 天后自动再问」
- 待人工确认（C 级）：按内容分组，沿用辅种页那套「采纳/否决」

## 十三、分期

| 期 | 内容 | 验收 |
| --- | --- | --- |
| C0 | `ContentIndex`：扫描、并查集、增量、落盘；CLI `coverage scan` | 万级种子首次建索引 < 10 分钟，二次扫描增量只碰新种子；单测覆盖合并/摘除/归档 |
| C1 | `SourceResolver` + 决策表 dry-run（只出报告，零上传零推送） | 20 个内容 × 5 源站实测：trackerHost 优先路径平均 ≤ 2 请求定位；报告与人工判断一致 |
| C2 | 辅种臂上线（格子记到 `coverage`）+ 站点 worker 并发 + 回查 | 真实辅种 50 条无一重下；20 站并发下墙钟 ≈ 最慢站；重跑零重复请求 |
| C3 | 转种臂草稿模式（预览 + 人工确认）+ 全部硬拦截 | 10 条草稿实测：字段预览与手动转种一致；禁转/壳页/C 级全部拦下并说明 |
| C4 | 白名单站全自动 + 首转保护 + 定时增量 + IYUU 可选开关 | 全自动站连发 20 条无审查问题；失败自动降级回草稿 |
| C5 | Web 控制台 + 统计页（覆盖率曲线、请求花费） | 盒子上无头跑一周，远程可看可确认 |

每期都不破坏现有转种链路；C2 之前 `AssistFeature` 保持关闭。

## 十四、测试

- `ContentIndex`：同内容多源种子合并（v1 相同 / 仅 manifest 相同 / v2 混合）、
  摘除后 30 天保留、归档后重建、`savePath` 变化
- `SourceResolver`：trackerHost 优先只搜一站、C 级不采、禁转标记硬拦、负缓存
- 决策表：九行全覆盖的表驱动用例（假适配器，零网络）
- 转种臂：草稿生成（含字段预览）、确认后走 `uploadOne`、`alreadyExists` 转辅种路径
- 并发：站点 worker 下逐站间隔不被全局并发破坏（用假时钟）
- 回归：`ReseedPipeline.run` 重构抽出 `uploadOne` 后，现有 646 测试全绿

## 十五、需要拍板的五件事（附建议）

1. **转种默认模式**：草稿确认（建议）还是全自动？
2. **源站名单**：独立 `coverage.sourceSiteIDs`（建议，语义清晰）还是复用转种分组？
3. **内容身份宽容度**：manifest/v1/v2 严格合并（建议）；名归一化只做展示不做身份
4. **索引落盘**：独立 `coverage.json`（建议）还是进 `state.json`
5. **IYUU 接入**：v1 先不做（建议，隐私与依赖都重），C4 再评估
