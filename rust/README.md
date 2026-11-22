# bsend（Rust 转种引擎 + 覆盖运营）

以「本地库 × 站点覆盖矩阵」为核心的 PT 运营引擎：**辅种是基础能力**（pieces-hash 批量通道
与站内名字检索兜底，已实现），**核心是带站点规范门禁的自动发种**（审阅 → 门禁 → 发布，
已实现并由 `autopost` 白名单 + 日限额 + 退避 + 审计驱动）。判定级别、推送与校验尾巴、
配置格式从 Swift 版逐条移植（方案文档见 `COVERAGE-PLAN.md`）。

## 构建

```bash
cd rust
cargo build --release                              # CLI：target/release/bsend.exe（单文件约 6 MB）
cargo build --profile gui-release -p bsend-gui     # GUI：target/gui-release/bsend-gui.exe
```

## 用法

```bash
# 只扫描 qBittorrent 建索引（不动任何站点）
bsend --config /path/to/boxsend.json scan

# 只做基线：把「已在做种」的格子标好并落盘，零站点请求（先看清家底）
bsend prime

# 预演一轮：检索照跑、结论照出，但绝不碰下载器
bsend run --dry-run   # 结论会落盘（下轮按终态预筛）
bsend plan            # 完全无痕：结论也不落盘

# 正式跑一轮辅种 + 转种草稿（结论落盘 coverage.json）
bsend run
bsend run --sites pter,hdsky     # 只跑指定站
bsend run --limit 10             # 小规模试跑（--limit 0 等价于 prime）
bsend run --only "The.Thirteen"  # 只处理这一段内容（名称包含 / contentKey 前缀）

# 看索引现状：内容数、各站覆盖分布、待发草稿、待找源清单
bsend status

# Cookie：逐站看条目数/过期（不打印值）；从 CookieCloud 现拉，或写回 cookies.json
bsend cookies                                # 只看参与站点
bsend cookies --from cookiecloud             # 从 CookieCloud 拉取并解密（不写盘）
bsend cookies --from cookiecloud --write     # 写回 cookies.json（旧的备份为 cookies.json.bak）
bsend cookies --all                          # 列出全部域名
bsend --cookiecloud run                      # 本轮现拉 cookie，任何子命令都可带

# 覆盖报告：逐站覆盖率、未覆盖最多清单、草稿与待找源；--json 导出给 GUI/daemon
bsend report --top 20
bsend report --json coverage-report.json
```

`--config` 默认读当前目录的 `boxsend.json`。每轮建议用 cron / 计划任务低频驱动；
终态格子会被预筛跳过，增量轮次的花费很小。

## Rust 版 GUI（GPUI）

`bsend-gui` 是 CLI 之上的可视化外壳，与 CLI 共用同一份配置与 `coverage.json`：

```bash
bsend-gui /path/to/boxsend.json     # 缺省读当前目录 boxsend.json
```

> 构建说明：GPUI 0.2.2 在 Windows 的标准 `--release` 下会调用 `fxc.exe`（Windows SDK）
> 预编译 HLSL；没装 SDK 时用专用 profile `gui-release`（走运行时着色器编译），
> 或直接 `cargo build -p bsend-gui` 用 debug 版——两者功能相同。

- **概览**：内容数 / 成员种子 / 各站格子、覆盖率；格子 = 一段内容 × 一个站点的判定单元，
  附状态图例（已在做种/已辅种/已转种/校验中/待人工确认/人工否决/站内没有/待找源/失败/转种草稿）；
- **转种草稿**：源站、目标站、详情页链接；「复制源站链接」「审阅」（只审不发：
  字段清单打进日志）和「确认转种」（原生一键：预检 → 上传 → 发后自检 → 推种 → 校验，
  不再调用 GUI 版）；
- **任务**：扫描 / 基线 / 跑一轮 / 刷新，后台线程执行，日志实时回流（不卡界面）；
- **确认说明**：内置转种全流程说明（审阅 + 五步各自做什么、预检拦哪三类问题）。

技术栈：GPUI 0.2.2（Zed 的 UI 框架，`Application::new().run` + `Render`），
深色主题、mpsc 事件 + 200ms 轮询刷新视图。

## 审阅与确认转种（Rust 原生全流程，v1 已不再依赖 GUI 版）

Rust 版转种臂产出 `reseed_draft`（源站 + 详情页链接 + 目标站）后，先审后发：

```bash
bsend reseed                 # 列出草稿编号
bsend reseed --draft 1       # 审阅第 1 条（只审不发）：预检结论 + 官方预审评分 + 将提交的字段清单
bsend reseed --draft 1 --yes # 确认提交：预检 → 上传 → 发后自检 → 推种 → 校验
bsend formcheck              # 全部启用站发种表格演练（只审不发，绝不 POST）
bsend formcheck --sites luckpt,hdsky   # 只演练指定站
bsend layoutcheck --limit 20           # 落盘核对：抽 20 段内容看推种会用哪个 savepath（只读）
bsend sites [--md|--json]              # 站点总账：启用/暂停/实测/pieces/cookie 一处看全
bsend gatecheck [--sites a,b|--json]  # 全站门禁演练：字段 + 官方预审，不上传种子
bsend autopost --dry-run               # 草稿审阅 + 门禁矩阵（不发布；--limit N 限制审阅条数）
bsend autopost --sites sbpt            # 只自动发布 sbpt 草稿（白名单 + 日限额 + 审计）
```

GUI「转种草稿」页的「审阅」按钮与之等价（字段清单打进任务日志），「确认转种」= 人工
确认，与 `--yes` 等价。随后原生一条龙：

1. **预检（本地三查 + 站点官方预审）**：本地三查——MediaInfo 非空 / 简介带
   IMDb·豆瓣·TMDB 链接（缺则按源页解析出的条目 id 自动补）/ 检测到中文字幕时勾
   「中字」标签、检测到完结剧集（完结/全集/全N集/Complete）时勾「完结」标签；任一阻断项不过就不上传。
   **源站详情页没有 MediaInfo 块时，从本地文件生成**（MediaInfo.dll C 接口，
   取内容目录里最大的视频；luckpt 等站发种硬性要求，缺了官方预审直接报错）。官方预审——上传页带 LuckAudit 面板的站
   （目前 luckpt）把将提交字段送 `POST /api/auto-audit/pre-audit` 打分，审阅里全量
   列出官方 ❌/⚠️/🔍 明细与建议，`--yes` 时 `passed=false` 直接拦下（站点自动审核
   同样会拒，先拦省一次白传）；接口异常只降级提示不拦。预审载荷会剥掉简介里的内联
   事件属性（onload/onclick 一类）：站点 WAF 对表单体里的 `on*=` 按出现次数异常计分
   （实测 2 处放行、4 处即 403「网站防火墙」），剥属性对打分无意义且真实上传一字不动。
2. **上传**：拉目标站 `upload.php` 表单，透传 hidden 字段（passkey 等 token）与 select
   默认值，填标题/副标题/简介/MediaInfo（独立表单字段 technical_info 等优先，否则内嵌
   简介）/IMDb·豆瓣·TMDB 字段/标签复选框，multipart 提交；「请填写必填项目」自动重投一次。
   **选档**：分类/媒介/编码/音频/分辨率按发布名+副标题词元自动选档（`HDTV`/`1080p`/
   `x264`/`AAC`/`国产剧`… 与该站选项标签唯一配对才选，配不上或不唯一只留提示不猜）；
   制作组按名尾 `-GROUP` 对 team_sel 配对。配对规则（实测站点校准）：规范化全等
   （`WEB-DL 高清`≡`webdl`）优先、分隔子串兜底，别名表覆盖简写与中文标签（`HDTV`→
   `TV`/`电视`、`1080p`→`1080` 裸数字、`x264`↔`H.264/AVC`…）；NexusPHP 参数化多组
   下拉（`medium_sel[4]`/`medium_sel[9]` 分类分支）各组独立配对全填；选项与词表不同
   域的同名下拉（`source_sel` 是地区、书站 `audiocodec_sel` 是电子书格式）自动跳过
   不误报；同名重复选项（两个 `AAC`）歧义留空不猜；垂直站（曲艺/有声书/音乐）分类里
   没有影视类时提示「样本类型无对应项」，不算表单缺口；Gazelle 系字段名回退
   （`title`/`subtitle`/`release_desc`）；无标题输入框的站（标题由种子文件名自动
   生成，如 tjupt）不算「标题未落上」。
3. **发后自检**：回读新种详情页，核对标题 / MediaInfo / 条目链接真的落上了，
   漏了在日志里点名（只警告不推翻上传结论）。
4. **推种**：从目标站详情页下载新种子，暂停加入下载器（dlLimit=1 兜底），启动校验。
5. **收尾**：校验到 100% 解除限速、格子记「已转种」；分片对不上按设置停掉/删掉；
   超时落「校验中」下一轮收尾。上传提示「种子已存在」或站内判重命中且分片一致时，
   **仍会下载目标种并挂到本地同分片数据上**：hash 不同 → 新加一个种子（不动原种子）；
   hash 完全相同 → qB 不允许同 hash 两条，按策略**不改动原种子**（不补挂 tracker、
   不打标签），记「同 hash 跳过」；分片不一致才记「其他版本」跳过。

   每次转种（CLI / GUI / autopost）都会在 dataDir 追加一行 `reseed-log.jsonl`：
   开始/结束时间、源/目标站、结果、备注，以及 `seeded`（是否已确认在 qB 做种：
   只有校验到 100%、格子落到 `reseeded/assisted/seeding` 才为 true）。

   **落盘布局（`NoSubfolder`）**：qB 默认把多文件种子放进 `<savepath>/<种子根名>/`。
   如果目标站种子的根名与本地已有种子不同（`Show.S01-GRP` vs `Show S01 GRP`），
   直接沿用旧 savepath 会新建一个空目录、校验 0%。引擎会核对两边「去掉根名后的文件
   清单」：完全一致时把 savepath 指到**既有内容目录**，并带 `contentLayout=NoSubfolder`
   （告诉 qB 不要再建根子目录，直接把文件映射进该目录），从而复用同一份数据校验到
   100%；清单不一致或单文件改名则拒绝推种（`Mismatch`），不做猜测。

解析层的防坑（实测踩过）：单引号 `id='kdescr'` 与双引号同样识别；标题尾部促销角标
（`[50%]`/`[FREE]`/`[2x]`/`[免費]`）自动剥掉但保留 `[中字]` 这类内容标签；角标后还挂
「剩余时间：6天18时」倒计时的（luckpt），先剥倒计时尾巴再剥角标，正文里提到「剩余时间」
但不是倒计时的不动；简介容器找不到时退「简介」表行 → 正文纯文本，绝不把整页 HTML 当简介
提交；源站把简介整体转义一层存储的（luckpt kdescr 里是 `&lt;fieldset&gt;…`，站点只补
nl2br 的裸 `<br />`），按「转义标签占绝对多数 + 富文本标记/无裸标签」判别后单趟解码还原
（`decode_entities_once`，不会把 `&amp;lt;` 二次解成 `<`），混在裸 br 里的 b/i/u 教程文本
不会误解码。检索/详情/下载回包里的 JS 人机挑战页（Cloudflare「Just a moment」等）与限流页
一律冒错退避，绝不当「站内没有」——判重因此 fail-closed，不会放行重复上传。
下载路径同样防坑：NexusPHP 首次下载确认页（`downloadnotice.php?type=firsttime`）
自动 POST 确认表单再重试；pieces 通道命中后从 qB tracker 定向刷新该站 passkey
（缓存会过期——sbpt 实测旧值被 download.php 拒、pieces API 却仍接受）。

## 配置：与 GUI 版同一份

直接读 Swift GUI 版写的 `boxsend.json`，无需另配：

- 参与站点 = `sourceSites` 里 enabled 的站 ∩ 辅种设置（`assist.siteIDs`，空 = 全部）
  − `pausedSites`，且框架为 NexusPHP（v1 只有这套检索启发式）。
  转种/发种另有 `assist.reseedPausedSites`：该表里的站**仍可辅种**，只是不出转种草稿
  （账号无发布权限、上传界面 v1 不支持的站放这里）。
- 下载器 = `assistDownloaderID` 指向的那台（缺省、或找不到指定 id 都退回主下载器）；
  沿用它的 savePath / 分类 / 限速（含 `siteUpLimits` 逐站表）。推送固定「先暂停等校验」，
  不读 skipChecking（方案 §七）。
- Cookie 读同目录 `cookies.json`（PT-depiler 备份格式，支持 `{"cookies":{...}}` 外层包裹）。
- `sourceSites[].statusNote`：站况标注（如「已关站（savept.icu 2026-10-29）：站点已放告别页」），
  配置加载会忽略该字段，仅作记录用；恢复某站时把它删掉、`enabled` 改回 true 即可。

`assist` 段里有几项是 Rust 侧扩展（GUI 不写也能跑，全有缺省）：

| 键 | 缺省 | 说明 |
| --- | --- | --- |
| `scanConcurrency` | 6 | 首扫读文件清单的并发数（qB 一次一条请求，4 万条串行十几分钟） |
| `maxConcurrentSites` | 4 | 同时推进的站数（站内仍串行保节奏） |
| `siteIntervals` | `{}` | 逐站请求间隔（秒）：`{"hdsky": 65}` —— hdsky 实测要求 60 秒/次 |
| `rateLimitFloorSec` | 60 | 站点报限流后该站本轮的最低间隔 |
| `roundBudget` | 200 | 目标站检索预算，**按真实 HTTP 请求计**（一次站内检索要发 3–7 条查询词） |
| `pausedSites` | `[]` | 完全暂停的站：辅种/转种/演练都不去敲（连不上、站点已挂、cookie 失效） |
| `reseedPausedSites` | `[]` | 只停转种/发种的站：无发布权限、上传界面 v1 不支持；辅种照常 |
| `mediaInfoDll` | 自动 | MediaInfo.dll 路径（源站没有 MediaInfo 时从本地文件生成用）；缺省按常见安装路径找 |
| `piecesHash` | `auto` | pieces-hash 批量通道（站点支持就走，失败回退名字检索；`off` 关闭） |
| `autoReseedSites` | `[]` | 自动发种白名单：`autopost` 只发布列出的站；空 = 只做门禁演练 |
| `reseedRules` | `{}` | 逐站发种规范（标题长度/链接/MediaInfo/禁发词/日限额/`selectPicks` 显式选档） |
| `publishEnabled` | `false` | **真实发种总闸**：三处入口都从此过；默认关闭，演练不受影响 |
| `skipPostCheck` / `skipHashCheck` | `false` | 发后跳过标题自检 / qB 添加时跳过哈希校验（测试阶段按需开启） |

### Cookie 来源

- 默认读同目录 `cookies.json`（PT-depiler 备份格式，支持 `{"cookies":{...}}` 外层包裹）。
- **CookieCloud**（easychen/CookieCloud，PT 站 cookie 端对端加密云备份）：读 GUI 设置的
  `cookieCloud` 段（`host` / `key` / `password`），两种用法：
  - `bsend --cookiecloud <命令>`：本轮现拉现用，不写盘；
  - `bsend cookies --from cookiecloud --write`：拉取后写回 `cookies.json`，之后普通命令直接用。
  解密在本地（`aes-128-cbc-fixed` 与 legacy CryptoJS 两种格式都支持），服务器只存密文；
  与 Swift 版 `Sync/CookieCloudSync.swift` 的派生规则逐字节一致。
- 站点没有 cookie 时会点名提示（跑到会 failed），不会静默吞掉。

站点级别名也可以在 `sourceSites[].overrides.announceHosts` 里追加（数组）：
判断「这份文件已经在该站做种」时，页面域名与 announce 域名不一致的站（实机 luckpt / chdbits）
靠它命中；内置别名表在 `sites::extra_announce_hosts`。

## 发种安全闸门

`assist.publishEnabled` **缺省 false**：`reseed --yes` / `autopost` / `run_reseed` 三处入口
在任何站点写请求之前先检查总闸（CLI 早于连 qB/拉源页）。默认只能演练；显式设为 true 后
才允许真实发布（实测已打通 luckpt / sbpt）。演练命令：`formcheck`（表单）、
`gatecheck`（字段 + 官方预审 + 门禁矩阵）、`reseed --draft N`（单草稿审阅）、
`autopost --dry-run`（草稿门禁）。

## 一轮流水线

1. **扫描合并**：qBittorrent 全量种子 → 按 v1/v2 info hash ∪ 文件清单摘要合并进内容索引；
   消失的种子记 `goneSince`，30 天后归档。没下完的种子跳过（文件不全没法比）。
   文件清单按 `scanConcurrency` 并发抓、只抓索引里还没有摘要的种子（二次扫描零请求）。
2. **已做种预筛**：tracker host 命中站点 announce host（含子域与内置别名）的格子直接标
   `seeding`，零请求。qB 5.x 的 `/torrents/info` 只给 `tracker`（当前那条），
   解析层对 `tracker_hosts` / `trackers` / `tracker` 三种形态都认。
3. **辅种臂**（站内检索）：pieces-hash 批量通道先行（rep 的 ph + 其余成员的已缓存 ph
   一起入批——目标站封装可能与任意成员一致而不一定是 rep），命中直接下种判定；
   未命中走名字检索：严格查询 → 未命中且开了宽松重试再来一轮 → 仍未命中且内容有多个
   成员种子时，用**最大成员种名**兜底再查一次（合集/目录名与站点标题不一致的实机漏检，
   `assist.memberNameFallback`，缺省开）→ 命中后下 .torrent 逐文件判定
   （A=hash 一致 / B=重新打包清单全等 / C=只有名字像）。
   A、B 且布局可复用（同根名，或仅根名不同 → NoSubfolder）→ 停止态添加
   （`dlLimit=1` 下载兜底）→ `start` 触发校验 →
   满 100% 解除限速、已在做种（`assisted`）；不满则停住，按 `assist.removeOnVerifyFail`
   删种保文件（`failed`）或保持停止。qB 5.x 起停止态的种子不会自校验，必须 start 一次；
   校验期间的 1 B/s 兜底保证「分片结构不同」时不会开始狂下。
   **落盘（先于添加）**：按成员种子在下载器里的**实时** savePath/根名（不信索引旧值）
   决定——根名一致直接沿用；根名不同但去根后的文件清单全等 → `savepath` 指到既有内容
   目录 + `contentLayout=NoSubfolder`（qB 4.4+，把新种文件直接映射进既有目录，不新建
   文件夹）；清单不一致 → `reviewPending`，绝不碰下载器。`bsend layoutcheck --limit N`
   只读核对一批内容的落盘决策（不推种）。
   轮询等到头还没结论 → 落 `verifying`（记下那条种子的 hash）；下一轮开头的
   收尾 pass 凭它直查下载器收尾，不花站点请求。C 级与布局对不上的 → `reviewPending`，
   绝不碰下载器。
4. **转种臂**（站内没有）：定位源种（已在做种的源站优先，刚查过的目标站不重复问）→
   出 `reseedDraft` 草稿（源站 + 详情页链接 + 目标站清单）。上传由 `autopost`
   （白名单 + 日限额 + 门禁 + 退避）或 `reseed --draft N --yes` 执行；上传前过
   判重 fail-closed、源种分片硬闸与全局上传预算。
   哪都没有 → `waitSource` 并记 30 天负缓存；检索报错标 `failed`，不记负缓存。

**不落负缓存的四种情况**（实机都踩到过）：cookie 失效（401/403、被跳到登录页、
NexusPHP 的 `index.php?returnto=…` 跳转）、站点限流页（HTTP 200 但正文是
「请求过于频繁，限制访问间隔 60 秒/次」的 403 页）、站点 5xx、预算半路用完。

## 下载器标签（Boxsend）

引擎加入下载器的每一条种子（辅种、转种回推）都带 `Boxsend` 标签；若种子早已存在
（409 / 补挂 tracker 路径），用 `torrents/addTags` **追加**，不覆盖原标签。
打标签失败只写进格子备注，不影响推种。下载器里可直接按标签筛选引擎管理的种子。

## 预算与节流（方案第九节）

- 三类独立预算，每轮重置：目标站检索 200 / 源站定位 200（都读 `assist.roundBudget`，
  与 GUI 版同一个旋钮）/ 转种上传 10（`assist.uploadBudget`）。预算按**真实请求**记：
  一次站内检索会走查询词阶梯（严格 3 条 + 宽松至多 4 条），每条都是一发请求、都要领号。
  预算耗尽的格子本轮直接跳过、不记任何结论——「没查成」不能当成「查过没有」。
- 转种上传同样领上传预算：`run_reseed` 在 `submit_upload` 前 `acquire(Upload)`，
  耗尽即 fail-closed；`autopost` 一次运行共用一个节流器，预算/站间隔跨草稿累计。
- 每站串行（缺省 8s 间隔 + 3s 抖动，`assist.perSiteIntervalSec` / `assist.jitterSec`），
  可用 `assist.siteIntervals` 逐站放宽（hdsky 实测 60 秒/次）；全局 worker 上限 4，
  替代 Swift 版的全流水线单线程。
- 站点报限流（HTTP 429/503、或限流提示页）→ 该站间隔翻倍退避，且本轮最低间隔抬到
  `rateLimitFloorSec`（缺省 60s）；一次成功请求恢复。
- 重试节流：`miss` / `waitSource` / `reviewPending` / `failed` 都歇
  `assist.missTTLDays` 天（默认 7）再重查；源种「定位不到」的负缓存
  `assist.sourceMissTTLDays` 天（默认 30）。校验收尾 pass 不占任何预算。
- 每站每轮最多排 `roundBudget` 格（一轮的预算撑不起更多，排进来只会白占内存）。
- 网络一律直连（`no_proxy`）：PT 站 passkey 与 IP 绑定，且 Windows 上「系统代理已停用但
  注册表留着 ProxyServer」会让客户端去连不存在的 SOCKS、每请求卡满超时（实机踩到）。

## 实机规模（2026-11-17，qB 5.3.0）

| 指标 | 实测 |
| --- | --- |
| 下载器种子数 | 41,885（41,882 已完成） |
| 首扫（抓 41,882 份文件清单，并发 6） | 214 s，进程峰值 233 MB |
| 去重后内容数 | **3,385 段**（12.4×：跨站辅种的同一内容合并成一段） |
| `coverage.json` | 15 MB（明文 JSON、原子写） |
| 增量重扫（零文件清单请求） | 14 s |
| 覆盖基线（9 个启用站，零请求 `prime`） | 3,938 格已是「在做种」 |

## coverage.json

落盘在 `dataDir/coverage.json`，原子写（tmp + rename），camelCase；结构：

- 每段内容一条：名字、总大小、文件清单摘要、全部 info hash（多个站同一内容合并成一条）、
  各站覆盖格子（status / at / url / note）、源种记录（siteId + detailUrl，或 missAt 负缓存）。
- 状态机：`seeding / assisted / reseeded / verifying / reviewPending / rejected /
  miss / waitSource / failed / reseedDraft / otherVersion`。`seeding / assisted /
  reseeded / rejected / reseedDraft / otherVersion` 为终态，不会再产生请求；
  `otherVersion`（目标站已有其他版本、pieces 不一致）不自动重传、也不周期性重查；
  `verifying` 由下一轮收尾 pass 接管（凭格子里的 verifyHash 直查下载器）；
  其余四类歇一个 missTTL 再重查。
- 解析失败的文件会被挪成 `coverage.json.corrupt` 留证，不会被下一轮 save 无声覆盖。
  **手改账本必须保持完整字段形状**：格子的 `url`/`note` 无 serde 缺省，漏写会让
  整个文件被判 corrupt、索引从零重建（历史结论全丢）；改前备份、改后校验。

## 与 Swift 版的关系

- 移植：NexusPHP 检索启发式（查询词阶梯 / 归一化互含 / 词元宽容判定）、CookieCloud
  （两种加密格式 + host 归一化 + 合并）、A/B/C 判定、
  qB 5.x 启停端点（`start`/`stop`，旧 `resume`/`pause` 已删，404 回退兼容 4.x）、
  布局门、推送尾巴（暂停添加 → 校验 → 启动/回滚）、qB WebAPI 兼容补丁（参数名两版并记住
  可用的那个、回包两种包装、409 已存在补挂 tracker、登录一次复用）、登录墙检测
  （401/403、登录页跳转、`returnto=`）、站点限流页检测、148 站识别表。
- 已移植：Rust GUI（`bsend-gui`，与 CLI 共用配置/索引）、上传表单填充与发种全流程
  （预检 → 上传 → 发后自检 → 推种校验 → 台账）、自动发种调度、源站全量适配器。
- 未移植：非 NexusPHP 框架的站点适配、Transmission。

> qB 5.3 语义：该版本移除了 `skip_checking`，改用 `seedMode=true`（跳过哈希校验、直接
> 以完整数据入种）。`add_form_fields` 同时发送两者：4.x 认旧键，5.3 认新键。

## 测试

改 CI / 依赖 / workspace 成员后，先在仓库根目录跑本地彩排（与 workflow 完全同一组命令）：

```bash
bash scripts/ci-check.sh
```

```bash
cargo test          # 数量以本次 cargo test 输出为准（单测 + e2e：mock qBittorrent +
                    # mock NexusPHP 走真实 TCP HTTP 跑完整一轮：辅种全流程、已做种预筛
                    # （含 qB 5.x 单 tracker 字段）、源站优先、负缓存、dry-run、
                    # C 级人工确认、409 已存在补挂 tracker、校验中跨轮收尾
                    # （启动/回滚两分支）、预算耗尽不记负缓存、
                    #  登录墙（login.php 与 returnto 两种）、限流页、429 映射失败、
                    #  审阅产物直发（提交字段一致、源页/上传页零重抓）、
                    #  站点 takeupload 拒收（保留 errorbox、不推种））
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

CI 在 `.github/workflows/rust.yml`（Ubuntu + Windows 双矩阵）。
