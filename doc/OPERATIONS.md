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
| `auto-audit.jsonl` | 自动发种审计，日限额和退避都靠它 | 只追加或整行删除，不要改中间字段 |
| `reseed-log.jsonl` | 每次审阅/上传/挂种的流水 | 只追加或整行删除，整个文件删掉也可以 |
| `pieces-cache.json` | 分片比对缓存 | 删单条或整个文件都行，会重新算 |
| `formcheck.json` | 上次表单检查的字段和下拉选项 | 可以删，下次检查会重算 |
| `gatecheck.json` | 发布检查矩阵和官方预审结果 | 可以删，会重算 |
| `screenshot-overrides.json` | 内容 key 对应的公网截图地址 | 只增删地址 |
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
  `seeding / assisted / reseeded / verifying / review_pending / rejected / miss / wait_source / failed / reseed_draft / other_version / same_hash_skip / skipped`。
- `skipped` 是内容黑名单的终态，第二轮不会再排队；`note` 里会写命中了哪个关键词。
- `reseed_draft` 在新流程里只是临时的待发标记，不是永久待办。
- 加白名单不会自动生成待发记录。新流程每轮从 qB 的实时缺口生成；`scan` 只刷新内容索引，不改站点处理记录。
- 改完运行 `bsend status`，能正常解析再继续。

### 2.2 容易误读的结果

| 结果 | 实际意思 |
| --- | --- |
| `skipped_duplicate` | 目标站已经有同分片条目，不是站点拒绝上传 |
| `same_hash_skip` | 目标种子和 qB 已有种子完全同 hash，不改原种子 |
| `other_version` | 站上同内容但分片不同，不是“重复” |
| `existed_pending` | 回包说重复，但回包里没有可核对的详情页/下载链接；回包 id 经常是错的 |
| `posted_unresolved` | 上传很像成功，但回包里没有可核对的详情页/下载链接；待人工确认，不记失败、不进退避、不占发布额度 |
| `wait_source` | 没有任何可用成员种子，导不出源文件 |
| `review_pending` | 站上命中了，但清单或 hash 对不上，等人核对 |

`bsend status` 有这类记录时，会在“覆盖分布”下面单独给出“其中发后未解决 N”，只统计 note 以 `posted_unresolved` 开头的条目；它和普通 `review_pending` 不是一回事，不能拿普通待核对的处置方式去重发。

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

## 4. 审计与退避

`auto-audit.jsonl` 每行是一条自动发种结果：

```
at / site / key / result / url? / error? / note? / gates?
```

`result` 常见值：

`posted / blocked / failed / review_failed / skipped_duplicate / other_version / existed_pending / same_hash_skipped`

用途：

- **日限额**：按北京时间切日，统计 `posted`；
- **退避**：同一条内容 + 同一个站，`blocked` 停 24 小时，`failed` 和 `review_failed` 停 6 小时；
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
6. 有多少条还在退避期。

```powershell
.\scripts\healthcheck.ps1 -Config "$env:LOCALAPPDATA\BoxSend\boxsend.json"
```

退出码：`0` 全正常、`1` 有警告、`2` 配置或覆盖记录坏了。

### 5.3 各工具分工

- `bsend sites` / `scripts/site-matrix.ps1`：看站点档案和表单情况；只有新站第一次接入才跑表单检查。
- `bsend report`：看整体覆盖率。
- `bsend autopost`：找缺口、生成待发、审阅、发布、挂回。
- `seeding-audit.ps1`：看哪里卡住了。
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

## 7. 发布相关配置

| 配置项 | 默认 | 说明 |
| --- | --- | --- |
| `assist.publishEnabled` | `false` | 总开关；关闭时只审不发 |
| `assist.autoReseedSites` | 42 站 | 自动发种白名单。空的时候真实发布不会发出任何站；只在演练时会覆盖全部站点做发布检查预演 |
| `assist.reseedPausedSites` | 空 | 只停发种，辅种照常 |
| `assist.pausedSites` | 空 | 整站暂停 |
| `assist.roundBudget` | 200 | 每轮每站最多处理多少条 |
| `assist.uploadBudget` | 10 | 每轮最多尝试上传多少次；重复结果是否占额度还在修 |
| `assist.allowOtherVersion` | `true` | 站上有同名其他版本时照样发 |
| `assist.skipKeywords` | 16 个词 | 标题黑名单，命中不占额度 |
| `assist.reseedRules` | 35 条 | 站点级字段映射和下拉选项覆盖 |
| `assist.skipPostCheck` | `true` | 跳过发后自检；小批量验证时应关掉 |

`targetSites` 是旧界面留下的字段，引擎不读。

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
