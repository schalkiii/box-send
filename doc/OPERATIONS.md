# BoxSend 运行与排障手册

这份文档讲运行数据放在哪、每个文件哪些能改哪些不能改、常用命令怎么用、出问题按什么顺序查。程序为什么这样设计看 `ARCHITECTURE.md`。

## 1. 数据目录

数据目录取自配置文件 `boxsend.json` 里的 `dataDir`；没配就放在配置文件同目录。实机是：

```
C:\Users\Schal\AppData\Local\BoxSend
```

程序没有 `~/.boxsend` 这种默认路径，找不到文件先来这里看。

| 文件 | 用途 | 能不能手改 |
| --- | --- | --- |
| `boxsend.json` | 主配置，程序只读 | 改前备份；负数会被改成 0 或下限 |
| `coverage.json` | 内容索引和覆盖记录 | 可以，但要按第 2 节的格式 |
| `auto-audit.jsonl` | 自动发种审计，日限额和失败冷却都靠它 | 只追加或整行删除，不要改中间字段 |
| `reseed-log.jsonl` | 每次审阅/上传/挂种的流水 | 只追加或整行删除，整个文件删掉也可以 |
| `pieces-cache.json` | 分片比对缓存 | 删单条或整个文件都行，会重新算 |
| `formcheck.json` | 上次表单检查的字段和下拉选项 | 可以删，下次检查会重算 |
| `gatecheck.json` | 发布检查矩阵和官方预审结果 | 可以删，会重算 |
| `screenshot-overrides.json` | 内容 key 对应的公网截图地址；由 `scripts/seed-images.py` 生成 | 只增删地址 |
| `cookies.json` | 按站点保存的 Chrome Cookie | 删某个站会触发重新同步 |
| `coverage.json.corrupt` | 解析失败时隔离出来的坏文件 | 保留，程序不会自动覆盖 |
| `coverage.json.bak-*` | 覆盖记录的自动备份 | 留最近几份 |
| `startup.log` | 旧 Windows 界面留下的日志 | 现在的界面不写，可以删 |

## 2. 覆盖记录 `coverage.json`

文件结构是：

```json
{ "contents": { "m:<内容短标识>": { "...": "..." } } }
```

每段内容包括：

```
name / totalSize / fileCount / manifestDigest
torrents{hash → 种子信息}
trackerHosts
firstSeen / goneSince
coverage{站点 id → 处理记录}
source / forbidReseed
```

每个“处理记录”有五个字段，必须齐全：

```
status / at / url / note / verifyHash
```

少一个字段，整个文件就会解析失败。程序会把坏文件改名为 `coverage.json.corrupt`，然后从空记录重建。所以手改前先复制一份。

### 2.1 手改规则

- 想让某站某条内容重新处理：删掉该站那条记录即可。
- 不要写 `status: "missing"`。表示“没有”的枚举值是 `miss`。
- 状态值使用下划线写法：
  `seeding / assisted / reseeded / verifying / review_pending / rejected / miss / wait_source / failed / reseed_draft / other_version / same_hash_skip / skipped / pending_offers / pending_review / review_rejected / gone_from_site`。
- `skipped` 是内容黑名单的终态，第二轮不会再排队；`note` 里会写命中了哪个关键词。
- `pending_offers`（待走候选入口）用在站点要求改走候选页（NexusPHP 的 `offers.php`）时，例如判定种子超过体积上限。这类不是“完事”，也不该当失败反复重试：记录留着给人跟进，`bsend status` 单列“待跟进”，下一轮不会自动重新排队。
- 站点审核的三类：`pending_review`（等站点审核，站上已发布但站点还没放行、拿不到站点种子）、`review_rejected`（站点明写审核不通过，`note` 里是站点给的原因）、`gone_from_site`（回访时详情页说没有这条）。都不算失败、不重发、不进失败冷却；由回访收尾（见第 6 节的 `bsend followup`），`bsend status` 的“待跟进”里逐条点名。想人工了结其中一条：删掉该站那条记录即可。
- `reseed_draft` 在新流程里只是临时的待发标记，不是永久待办。
- 加白名单不会自动生成待发记录。新流程每轮从 qB 的实时缺口生成；`scan` 只刷新内容索引，不改站点处理记录。
- 改完运行 `bsend status`，能正常解析再继续。

### 2.2 容易误读的结果

| 结果 | 实际意思 |
| --- | --- |
| `skipped_duplicate` | 目标站已经有同分片条目，不是站点拒绝上传 |
| `same_hash_skip` | 目标种子和 qB 已有种子完全同 `info hash`，但不满足“只挂目标站 tracker 且 progress=1”（没挂目标站、挂了别站、或还没下完/校验完）；不改原种子、不 add 第二条。满足条件时记 `seeding`，不记这个 |
| `pending_review` | 站上已发布，但站点还没审完或还没放行下载；等回访，不算失败 |
| `other_version` | 站上同内容但分片不同，不是“重复” |
| `existed_pending` | 回包说重复，但回包里没有可核对的详情页/下载链接；回包 id 经常是错的。note 以 `existed_unresolved` 开头时，表示站上已有、下载/挂回 qB 失败 |
| `posted_unresolved` | 站上已发布（新发或 existed），但下载目标种、校验或挂回 qB 失败；待人工补挂，不记失败、不进失败冷却、不占发布额度 |
| `wait_source` | 没有任何可用成员种子，导不出源文件 |
| `review_pending` | 站上命中了，但清单或 hash 对不上，等人核对 |
| `pending_offers` | 站点要求改走候选入口（`offers.php`），程序不自动重试；等人工去候选页提交，`status` 里单列“待跟进” |

`bsend status` 把两类“等站点侧动作”的记录单列成“待跟进（等站点侧动作，程序不自动重试）”：等走候选入口（`pending_offers`）和站上已发、qB 未挂回（note 以 `posted_unresolved` / `existed_unresolved` 开头）。等候选的条目会点名列出（最多 10 条，附站点当时给的原因），超过 10 条提示去 `bsend audit --days 1` 看明细。这两类都不能拿普通 `review_pending` 的处置方式去重发。

## 3. 覆盖率怎么读

`status` 和 `report` 的主口径已经从“判过多少”改成“qB 里有没有”：

- **已有**：qB 里带该站 tracker 的内容数。
- **缺口**：全库内容总数减去“已有”。
- **覆盖率**：已有除以全库内容总数。

这三个数回答“这个站在全部内容里铺开了多少”，适合跨站比较。

旧的字段还保留在 JSON 里：

- `cells`：该站处理过的内容数；
- `covered`：`seeding / assisted / reseeded` 的数量；
- `coveredRatio`：`covered` 除以 `cells`，只看已处理部分；
- `libraryRatio`：旧口径的全库覆盖。

`coveredRatio` 可能显示 100%，但全库只覆盖 4%，因为没处理过的内容根本没进分母。不要拿它当铺满度。现在 `status` 表已经换成“已有 / 缺口 / 覆盖率”三列，`report` 表同时保留旧列供历史对比。

## 4. 审计与失败冷却

`auto-audit.jsonl` 每行是一条自动发种结果：

```
at / site / key / result / url? / error? / note? / gates?
```

`result` 常见值：

`posted / blocked / failed / review_failed / skipped_duplicate / other_version / existed_pending / pending_offers / same_hash_skipped`

用途：

- **日限额**：按北京时间切日，统计 `posted`；
- **失败冷却**：默认关闭（`assist.backoffHours` 不填或填 0）。同一内容 + 同一站点被拦或失败后，下一轮直接重试，由人看审计再决定要不要再跑；只有显式配了小时数（例如 6）才会在窗口内跳过，且拦截和失败共用这一个窗口。`pending_offers` 不进冷却，因为它不是失败；
- **聚合查看**：`bsend audit --days N [--site X] [--json]`，按六类汇总：
  `posted / blocked / failed / duplicate / other_version / other`，另外报告格式坏掉的行数。

`reseed-log.jsonl` 每行记录一次动作：

```
at / started_at / key / name / source_site / source_url
target_site / result / url / note / cell_status / seeded
```

`seeded=false` 表示站上可能已经发布，但本机没挂上种子。优先查目标种下载和添加时的目录布局，不要先怀疑上传。日志写失败只会打印错误，不影响发种结论。

## 5. 只读巡检脚本

### 5.1 `scripts/seeding-audit.ps1`

不改配置、不写覆盖记录、不发站点请求；读 `coverage.json`、`reseed-log.jsonl`，并向 qB 读一次种子列表。列三类问题：

1. 卡住的处理记录：`failed / verifying / reseed_draft / review_pending / other_version / same_hash_skip / wait_source`；
2. 最近 N 天 `seeded=false` 的转种记录；
3. qB 里带 `Boxsend` 标签、但进度不到 100% 的种子。

```powershell
pwsh scripts/seeding-audit.ps1
pwsh scripts/seeding-audit.ps1 -Days 14 -Limit 100 -OutFile audit.md
pwsh scripts/seeding-audit.ps1 -Json -OutFile audit.json
```

参数：

- `-ConfigPath`：配置文件，默认按标准位置找；
- `-Days`：回看天数，1–90，默认 7；
- `-Limit`：每类最多列多少条，默认 50；
- `-Json`：输出 JSON；
- `-OutFile`：写到文件。

退出码：`0` 正常（有问题也是 0，看报告内容）、`1` 脚本自身异常、`2` 参数错误。脚本只调 qB 的登录和种子列表接口，输出前会把 URL 里的 `passkey`、`torrent_pass`、`authkey`、`token` 替换成 `***`。

### 5.2 `scripts/healthcheck.ps1`

六项只读检查：

1. 配置能不能解析；
2. 覆盖记录能不能解析，有没有 `.corrupt` 残留；
3. qB 能不能连上；
4. 有没有没下完的种子；
5. 最近审计的失败率；
6. 有多少条还在失败冷却期（`assist.backoffHours` 为 0 时这一项恒为 0）。

```powershell
.\scripts\healthcheck.ps1 -Config "$env:LOCALAPPDATA\BoxSend\boxsend.json"
```

退出码：`0` 全正常、`1` 有警告、`2` 配置或覆盖记录坏了。

### 5.3 各工具分工

- `bsend sites` / `scripts/site-matrix.ps1`：看站点档案和表单情况；只有新站第一次接入才跑表单检查。
- `bsend report`：看整体覆盖率。
- `bsend autopost`：找缺口、生成待发、审阅、发布、挂回。
- `seeding-audit.ps1`：看哪里卡住了。
- `scripts/seed-images.py`：源站简介没有图时，按内容键从本地视频抽帧（默认 3 张，可多条视频各取一帧），也可用 `--douban <条目id>` 自动取豆瓣海报或传 `--poster-url`；上传 pixhost/imgbox 后写 `screenshot-overrides.json`；`--dry-run` 只抽帧不上传。
- `scripts/imgbox-upload.py`：把本地海报/截图传到 imgbox，输出原图直链；匿名、无需 API key、单图 10MB。
- `scripts/edit-seed.py`：只改已发布种子的标题/简介；默认演练，`--apply` 才提交，其余表单原样回填。
- `scripts/fetch-descr.py`：读已发布种子的编辑页，把简介字段原样落到 `%TEMP%\bsend-review\`，给换图/改简介做准备；只读，不提交。
- `scripts/swap-descr-images.py`：按出现顺序把简介里的图片链接 1:1 换成新直链；BBCode 与 HTML 都支持，张数对不上直接报错不写文件。
- `scripts/set-cell-note.py`：按内容键 + 站点改覆盖记录里那一格的说明（区间替换、保持原换行），改完自动备份并复核。
- `healthcheck.ps1`：看运行健不健康。

## 6. 常用命令

```powershell
Set-Location rust
cargo test -p bsend
cargo test -p bsend --lib reseed::tests::exist_marker
cargo build --release -p bsend-cli

.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" status
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" report --json out.json
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" audit --days 7
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" autopost --dry-run --limit 5
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" autopost --loop --interval-secs 1800
```

`--dry-run` 是试运行：会连站点做审阅和发布检查，但不上传、不写覆盖记录。

站上已经发了、qB 里还没有的记录，用 `bsend pull` 挂回来（不重发、不占发布额度）。原理是把站点详情页上的种子下回来，按分片（pieces）认出本地内容，再挂进 qB：

```powershell
# 详情页可下载：自动拉详情页、下载目标种、按 pieces 找本地内容并挂回
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" pull `
  --site hdvideo --url https://hdvideo.top/details.php?id=78856

# 索引里同一内容有多条、或要指定内容：加 --key
... pull --site qilin --url https://www.hdkyl.in/details.php?id=479362 --key m:5826983b00126e58
```

这个命令原来叫 `reattach`，2026-11-24 改成 `pull`；旧名还能用，只是不出现在 `--help` 里。

这条命令只从站点详情页下载目标种，不再接受 `--file` 本地构造种；站点下载不到就报错，宁可不挂也不塞废种。只核对 hash/pieces 和挂回 qB，不会调用上传接口；目标种 pieces 与本地内容对不上会直接拒绝。

站点还没审完的种子由回访接着管。自动发种每轮开头会自动回访一次（只看记了审核结论的格子），也可以随时手动跑：

```powershell
# 先演练：照样打开详情页看结论，但不改覆盖记录、不往 qB 里加种子
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" followup --dry-run --sites tey,qilin,cangbao,panda

# 正式回访：页面还标着未审 / 不通过 → 改格子并记下站点原话；页面不再有审核标记 → 把站点种子下回来挂进 qB
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" followup

# 连自己发的、已经在做种的也看一遍（找出以前没记下来的“未审 / 不通过”）；--limit 限条数，--json 给脚本用
... followup --all --limit 50
```

回访每条最多两次站点请求（详情页、站点种子），照该站的请求间隔排队，不上传、不发帖。页面打不开（登录失效、限流、网络故障）不算结论，格子不动。审核没过的要人去站上改简介或种子，程序不会重发，也不会替你给管理发站内信。

## 7. 发布相关配置

| 配置项 | 默认 | 说明 |
| --- | --- | --- |
| `assist.publishEnabled` | `false` | 总开关；关闭时只审不发 |
| `assist.autoReseedSites` | 40 站（2026-11-25 实测配置） | 自动发种白名单。空的时候真实发布不会发出任何站；只在演练时会覆盖全部站点做发布检查预演 |
| `assist.reseedPausedSites` | 空 | 只停发种，辅种照常 |
| `assist.pausedSites` | 空 | 整站暂停 |
| `assist.roundBudget` | 200 | 每轮每站最多处理多少条 |
| `assist.uploadBudget` | 10 | 每轮最多尝试上传多少次；重复结果是否占额度还在修 |
| `assist.allowOtherVersion` | `true` | 站上有同名其他版本时照样发 |
| `assist.skipKeywords` | 16 个词 | 标题黑名单，命中不占额度 |
| `assist.reseedRules` | 36 条 | 站点级字段映射和下拉选项覆盖 |
| `assist.skipPostCheck` | `true` | 跳过发后自检；小批量验证时应关掉 |

`targetSites` 是旧界面留下的字段，引擎不读。

`reseedRules.<site>.uploadTimeoutSec` 是上传请求的单独超时，缺省 120 秒。站点服务端解 torrent、查重特别慢时才放宽；2026-11-25 tjupt 的 25GB 单条能正常完成，274GB 合集放到 300 秒仍只回空正文，属于站点后端超时，不是超时值不够。

### 7.1 第一次向一个新站发种：先填 tracker 地址

每个站在 `sourceSites[]` 里可以写一个 `overrides.announceURL`，内容是这个站给本账号的完整 tracker 地址（含 passkey）：

```json
{
  "id": "hdsky",
  "name": "天空",
  "url": "https://hdsky.me/",
  "overrides": {
    "announceURL": "https://tracker.hdsky.me/announce.php?passkey=你的个人密钥"
  }
}
```

为什么要单独填：程序正常情况下会从 qB 里已有该站的种子中抄 tracker 地址。第一次向这个站发种时，qB 里一条该站种子都没有，抄不到，程序会停下不传。填了以后就用配置里的地址；没填仍会先尝试从 qB 抄。

地址必须属于这个站。程序会拿地址里的域名和该站登记的域名比对，对不上直接中止。不要把别的站的 passkey 填进来，也不要只填 tracker 主域名而漏掉你的个人参数。

### 7.2 简介格式：BBCode 站要转格式

NexusPHP 的简介框是 BBCode 编辑器。源站简介里的 `<fieldset>`、`<img>` 原样提交，会被站点转义成字面标签，页面只显示 `<fieldset>…`，审核端也会判“简介未包含图片”。

程序现在**默认把 HTML 简介转成 BBCode**（`[quote]`、`[b]`、`[size]`、`[img]`、`[url]`），不需要逐站配置。只有确认某站支持 HTML、且不希望转换时，才在该站规则里显式关掉：

```json
"reseedRules": {
  "some-html-site": {
    "descrFormat": "html"
  }
}
```

藏宝阁 43994 已经用 BBCode 重新提交，详情页能正常渲染引用块和海报图。2026-11-23 抽查发现 lajidui、ptcafe、sbpt、luckpt、ptlgs、tangpt、longpt、cyanbug、kufei、xingyunge、city13、shuioudao、xdy、baozi、novahd、ptzone 等站的历史种子简介也是字面 HTML，已用 `scripts/fix-bbcode-descr.py` 批量改成 BBCode（只改简介，不换种子文件）；29 条已发布种子复查时编辑框里都不再是 HTML。

### 7.3 图片门禁与图床

站点审核会逐张打开简介图片。图片 418/HTML、TLS 失败、域名失效都算死图；只在浏览器里能看不算通过。发种前按下面执行：

- **剪死图：所有站点无条件执行**（2026-11-24 起，代码硬规则，不再读 `assist.pruneDeadImages`）。带目标站上传页 Referer 逐张 GET，非 2xx 或非图片类型剪掉；豆瓣图床裸请求 418，带 Referer 才出图，所以 Referer 必须带。
- **要图的站有硬性最低图数**：`gates::image_floor` 按站点硬编码——藏宝阁、优堡、太乙、麒麟 3 张；熊猫高清 4 张（站规更严）；ptlgs 4 张（1 张海报 + 3 张截图）。配置里写 `requireImages` 会被覆盖，不用再配。
- `descrOrder`：按标记检查段落顺序，缺段或倒置都拦；只对配置了的站点生效。
- `titleNoCjk`：主标题含汉字直接拦，中文名放副标题。
- **独立海报/截图字段**：上传页把海报、截图单列时，在 `sourceSites[].overrides` 写 `posterField` / `screenshotField`（ptlgs 是 `cover` / `screenshots`）。程序从最终简介按顺序抽图：第一张填海报，其余一行一个填截图；字段不存在不硬塞，简介没图时清空。

藏宝阁 43994 的两张图（豆瓣海报、`imgs.loukky.com` 截图）都不可达；用 `scripts/imgbox-upload.py` 重传后换成 `images4.imgbox.com` 直链，带 Referer 实测 200。熊猫 141276 原来没有海报/截图，重写简介时补了 1 张海报 + 3 张截图，顺序按站点要求。死图不要靠关门禁绕过；先重传，再改简介。

2026-11-24 又碰上三次：ubits 379266 的 9 张图全在已失效的 `i13.tietuku.com`（站内信判“海报与截图均无法访问”），同一条内容的 tey 623 带着同样的死图还没审到；麒麟 479362 被判“缺少海报或截图”，实际 7 张图全不可达（豆瓣海报裸请求 418、`img.pterclub.com` TLS 失败）。三条都按同一套流程修：`fetch-descr.py` 取原简介 → 本地备图（海报带 `Referer: https://movie.douban.com/` 取，截图用 `seed-images.py` 从正片抓）→ `imgbox-upload.py` 传图 → `swap-descr-images.py` 按顺序换链接 → `edit-seed.py` 演练再提交 → 详情页数 imgbox 张数、旧图站点应为 0。

同一内容的其余四站（crabpt 240242、hdfans 287791、qingwa 208575、luckpt 61102）也在 2026-11-24 晚用同一套脚本把 9 张 `i13.tietuku.com` 死图换成了 imgbox，复查四站都是 0 张旧图、9 张新图；sbpt 21684 那份简介本来就没有图片，不需要换。

页面仍显示“不通过”时不用重发：站点没有用户侧“重新提交审核”入口，由 `bsend followup` 接着盯；覆盖记录用 `set-cell-note.py` 写明“已换图、等复审、别重发”。

## 8. 出问题按这个顺序查

1. `healthcheck.ps1`：配置、覆盖记录、qB 连接有没有问题。
2. `bsend status`：各站已有多少、缺多少，有多少待发和待找源。
3. `bsend audit --days 3`：最近失败、拦截、重复的比例。
4. `seeding-audit.ps1`：哪些记录卡住了，哪些发完没挂上。
5. qB 里筛 `tag=Boxsend`：看进度和 tracker 数量；每份种子只能有一个 tracker。
6. 最后才看站点：用标题、上传者、发布时间核对，不要信上传回包里的 id。

## 9. 相关文档

- 判断规则、发种流程、辅种规则：`ARCHITECTURE.md`
- 站点总账、白名单、受阻原因：`SITES.md`、`SITE-DEBUG.md`
- 还没做完的事：`ROADMAP.md`
