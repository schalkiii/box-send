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
- `skipped` 是内容黑名单的终态，第二轮不会再排队；`note` 里会写命中了哪个关键词。站侧阻塞（账号无发布权限、要过 2FA、被人机验证拦截、上传界面是纯前端）也落 `skipped` 这个中性终态，`note` 写明"站侧阻塞"和对应分类；这类不是发错了，程序不自动重发，要人工处理后删掉该站记录才会重新排队。
- `pending_offers`（待走候选入口）用在站点要求改走候选页（NexusPHP 的 `offers.php`）时，例如判定种子超过体积上限。这类不是“完事”，也不该当失败反复重试：记录留着给人跟进，`bsend status` 单列“待跟进”，下一轮不会自动重新排队。
- 站点审核的三类：`pending_review`（等站点审核，站上已发布但站点还没放行、拿不到站点种子）、`review_rejected`（站点明写审核不通过，`note` 里是站点给的原因）、`gone_from_site`（回访时详情页说没有这条）。都不算失败、不重发、不进失败冷却；由回访收尾（见第 6 节的 `bsend followup`），`bsend status` 的“待跟进”里逐条点名。想人工了结其中一条：删掉该站那条记录即可。
- 审核被拒的标记认两种写法：详情页标题旁标 `(禁止)` / `（禁止）`，或明写“审核不通过”，一律判审核被拒、落 `review_rejected`，不能再记成“已在做种”。
- 覆盖记录里的 `url` 必须是目标站自己的详情页链接：写入前程序校验域名，把源站链接写进目标站格子会被拒绝写入并报“链接与站点不符”；草稿阶段存源种链接不受影响。2026-11-26 纠正一条留痕：内容键 `m:5826983b00126e58` 的 agsvpt 格子原来写成 `seeding` + `https://cyanbug.net/details.php?id=5586`（状态错、链接是源站），已改为 `review_rejected` + `https://www.agsvpt.com/details.php?id=546963`，`note` 写明拒因、已补标签 52 和 4 张图、等复审、本地旧副本不能当已做种；同批内容键 `m:0996dd966965cbb1` 的 tjupt 格子保持 `pending_review`，`note` 改成“2026-11-26 已补海报+截图和 `[mediainfo]`、重新进待审、还差中文名拆分”。
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
| `posted_verifying` | 上传成功、种子已加入 qB，但校验没跑完；界面不写“转种完成”，下一轮接着收尾。算一次发布额度，但还不算已做种 |
| `wait_source` | 没有任何可用成员种子，导不出源文件 |
| `review_pending` | 站上命中了，但清单或 hash 对不上，等人核对 |
| `pending_offers` | 站点要求改走候选入口（`offers.php`），程序不自动重试；等人工去候选页提交，`status` 里单列“待跟进” |

`bsend status` 把“等站点侧动作”的记录单列成“待跟进（等站点侧动作，程序不自动重试）”，共五类：待走候选入口（`pending_offers`）、等站点审核（`pending_review`）、站点审核没过（`review_rejected`）、站上已找不到（`gone_from_site`）、站上已发但 qB 未挂回（note 以 `posted_unresolved` 或 `existed_unresolved` 开头）。等候选的条目会点名列出（最多 10 条，附站点当时给的原因），超过 10 条提示去 `bsend audit --days 1` 看明细。这几类都不能拿普通 `review_pending` 的处置方式去重发。

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
at / site / key / result / url? / error? / note? / gates? / manual?
```

`result` 常见值：

`posted / posted_verifying / blocked / failed / review_failed / skipped_duplicate / other_version / existed_pending / posted_unresolved / pending_offers / pending_review / review_rejected / same_hash_skipped / no_permission / two_fa / challenge_gate / spa_api`

用途：

- **日限额**：按北京时间切日，`posted` 和 `posted_verifying` 都算一次上传；后者只是做种还没确认；
- **失败冷却**：默认关闭（`assist.backoffHours` 不填或填 0）。同一内容 + 同一站点被拦或失败后，下一轮直接重试，由人看审计再决定要不要再跑；只有显式配了小时数（例如 6）才会在窗口内跳过，且拦截和失败共用这一个窗口。`pending_offers` 不进冷却，因为它不是失败；
- **站侧阻塞四类不进冷却、不算失败**：`no_permission`（账号无发布权限）、`two_fa`（站点要求先过二次验证）、`challenge_gate`（被人机验证/区域时段拦截）、`spa_api`（上传界面是纯前端 SPA/JSON 接口，程序拿不到表单）。这四类是站点侧条件，不是我们发错了：审计单列这四个分类，不进失败冷却，覆盖记录落 `skipped` 中性终态、不落 `failed`，程序不自动重发。`bsend audit` 合计里有一行"站侧阻塞"单独列出这四类条数，不混进失败率；
- **聚合查看**：`bsend audit --days N [--site X] [--json]`，主表按六类汇总：
  `posted / blocked / failed / duplicate / other_version / other`，另外报告格式坏掉的行数。待跟进单列，不混进六类：`pending_offers`、`pending_review`、`review_rejected`、`posted_unresolved`。

`reseed-log.jsonl` 每行记录一次动作：

```
at / started_at / key / name / source_site / source_url
target_site / result / url / note / cell_status / seeded
```

`seeded=false` 表示当前不能算“已在做种”：可能是站上已发布、但本机没挂上种子；也可能是 `posted_verifying`，种子已经加入 qB，但校验还没跑完。先看同一条记录的 `cell_status` 和 `result` 区分：`verifying` / `posted_verifying` 去等校验，其他情况再查目标种下载和添加时的目录布局，不要先怀疑上传。日志写失败只会打印错误，不影响发种结论。**注意 `seeded=true` 只证明“已经挂回 qB”，不证明站点 tracker 已回包**：还要看覆盖格 note 里的 tracker 状态，或直接查 qB `torrents/trackers` 的 `status`。`status=2` 才是站点已看见，`status=1` 只是还没联系上，`status=4` 是 tracker 报错。

当前口径：手动跑 `bsend reseed --yes` 成功后也会往 `auto-audit.jsonl` 追加一条 `manual=true` 的记录，并计入当天 `posted` 额度；重复、被拦、失败同样落审计，方便和自动队列按同一套日限额/失败冷却口径排查。`manual` 字段就是用来区分人工补发和自动发种的。

### 4.0 qB 重启后的 announce 队列

qB 里有 4 万多条种子时，重启后会把所有种子重新排队 announce。默认 `max_concurrent_http_announces=200`，新挂回的种子可能排在几十分钟之后，`torrents/trackers` 长期显示 `status=1`（未联系）且 `updating=true`，这不是发种代码失败。

- 先看目标种的 `last_activity` 是否还等于 `added_on`；是的话说明一次 announce 都还没发出去。
- 再看 qB 日志和 tracker 域名本身；`status=1` 同时手动请求也 TLS reset/超时的，是网络或站点 tracker 侧问题，不是队列。
- 队列拥塞可以临时把 `max_concurrent_http_announces` 调到 1000，观察目标种是否变成 `status=2`；诊断完恢复 200，别长期高压。
- 2026-11-27 实测：pthome 在 200 并发下等了约 50 分钟仍是 `status=1`，调到 1000 后 90 秒内变成 `status=2`；city13 调到 1000 仍不联系，手动 announce 被 TLS reset，属于站点 tracker 链路阻塞。

### 4.1 上传回包怎么分六类

原先所有失败都笼统显示“上传失败”。现在按回包特征分六类报：

1. **请求层超时/断连** → “上传请求没通”；网络层会同一字节重投一次，第二次仍不通才报失败。
2. **后端空回包**（正文 0 字节，或只剩 HTML 标签没有可见文本）→ 明确写“HTTP 200 空回包，通常是站点后端处理超时或崩溃，本次未确认发布”。
3. **缺必填项拒绝**（正文出现“有项目没有填写 / 请填写必填”一类）→ “站点提示缺少必填项”并带片段。
4. **种子过大**（`torrent_too_large` / `add_offer=1`）→ 落“待走候选入口”（`pending_offers`），不自动重试。
5. **其他站点拒绝** → 取错误框原文，兜底改成“站点拒绝（回包未展开具体原因）”，不再假装是“上传失败”。
6. **登录失效、回到上传表单**：各自保留原来的判定；**HTTP 200 但页面认不出来**先不判失败，走下面的“未确认发布”核对。

**HTTP 200 未识别回包先核对（2026-11-27 起）**：回包是 `HTTP 200 未识别的返回`（含“回包已存 …”变体）时，按发布名 + 本地总大小做站内检索；找到就把站点种子下回来、核对 pieces 并挂进 qB，找不到落 `posted_unresolved` 待人工，不记 `failed`、不进失败冷却。403/404、空回包、明确的站点拒绝仍按原失败口径。

**重复回包不按失败报**（2026-11-27 起）：最终跳转带 `existed=1`、错误框里有明确重复措辞，或重填页/超过 8KB 的长页面里出现“该种子已存在！”/“该种子已经存在！”/“已存在！”精确提示时，走重复分支，不落 `failed`；上传页帮助文案里的泛化“提示种子已存在”不算，避免误判。

**回包快照要先开开关**：跑批前设环境变量 `BSEND_DUMP_UPLOAD=1` 才会把上传回包快照存到数据目录的 `upload-debug/`；不设就没有这份带请求头和字段名的快照。2026-11-26 北洋园两条非空正文的“上传失败！”就是没留快照，离线只能记原因待查。

**没解析出具体拒因时自动留一份临时回包（2026-11-27 新增能力，已落地并有单测；当天 iloli 真实拒绝回包已留档实测，`artifacts\iloli-upload-1795736380253.html`）**：站点没给明确原因的通用拒绝（“站点拒绝（回包未展开具体原因）”或“HTTP … 未识别的返回”）会把回包正文原样写到系统临时目录 `bsend-upload-<站点id>-<时间戳>.html`，错误信息里附上绝对路径和剥掉标签后的可见文字；HTTP 200 未识别回包在转成“未确认发布”前也先留一份。已经能解析出具体原因的失败（缺必填项、重复、种子过大、会话失效等）不写临时文件、文案不变。临时目录这份不需要开环境变量；写盘失败静默忽略，不影响原错误上报。

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
- `scripts/seed-images.py`：源站简介没有图时，按内容键从本地视频抽帧（默认 3 张，可多条视频各取一帧），也可用 `--douban <条目id>` 自动取豆瓣海报或传 `--poster-url`；上传 pixhost 后写 `screenshot-overrides.json`；`--dry-run` 只抽帧不上传。同一部剧有多个内容键（如 HDTV 和 WEB-DL）时，用 `--also-content <内容键>` 逐个带齐，或 `--for-name <名称片段>` 在覆盖记录里找齐同名内容键；不要只补一个键。
- `scripts/imgbox-upload.py`：把本地海报/截图传到 imgbox，输出原图直链；匿名、无需 API key、单图 10MB。当前常规图床是 pixhost，imgbox 只作历史兼容，UBits 会把 imgbox 当死图。
- `scripts/edit-seed.py`：只改已发布种子的标题/简介；默认演练，`--apply` 才提交，其余表单原样回填。`--processing` 改文件容器（MP4/MKV）下拉，`--team` 改制作组下拉，`--append-image <url>` 在简介末尾追加一张图（已有同链接就跳过）。
- `scripts/replay-reseed.py`：按草稿号跑一次 `bsend reseed --draft <N>`（不提交），用 `--expect` 断言必须出现的字段/图片数，用 `--reject` 断言旧问题不能回来；修完问题先跑这个，再谈上线。
- `scripts/fetch-descr.py`：读已发布种子的编辑页，把简介字段原样落到 `%TEMP%\bsend-review\`，给换图/改简介做准备；只读，不提交。
- `scripts/swap-descr-images.py`：按出现顺序把简介里的图片链接 1:1 换成新直链；BBCode 与 HTML 都支持，张数对不上直接报错不写文件。
- `scripts/set-cell-note.py`：按内容键 + 站点改覆盖记录里那一格的说明（区间替换、保持原换行），改完自动备份并复核。
- `healthcheck.ps1`：看运行健不健康。

### 5.4 修复回放（2026-11-26 起）

发现站点字段、图片、标题或挂回问题时，先改代码或仓库脚本。人工只做只读检查，以及已经发到站上的种子紧急纠错；常规修复不靠手点表单。

改完后用同一个草稿做回放，不提交：

```powershell
python scripts\replay-reseed.py --draft 56 `
  --expect 'source_sel[4] = 10' `
  --expect 'processing_sel[4] = 11' `
  --expect 'team_sel[4] = 5' `
  --expect '注入本机视频截图 3 张' `
  --reject 'source_sel[4] = 0'

python scripts\replay-reseed.py --draft 85 --expect '注入本机视频截图 3 张' --expect '11→4' --reject '11→1'
```

回放只看“预检通过”不够，要看到会提交的具体字段、图片张数和标题。涉及已发种子时，再跑 `edit-seed.py` 的默认演练（不带 `--apply`），核对打印出的改动；确认后才加 `--apply`，提交后重新抓编辑页复核字段和图片数。

修复要留两层证据：纯逻辑补单测（例如 lajidui 的 `lajidui_quality_fields_fill_region_container_and_team`、`lajidui_unknown_team_uses_other_and_mkv`），全链路用上面的草稿回放。下一轮遇到相同现场，先跑回放，不靠记忆判断。

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
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" autopost --loop-mode --interval-secs 1800
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

这条命令只从站点详情页下载目标种，不再接受 `--file` 本地构造种；站点下载不到就报错，宁可不挂也不塞废种。只按 pieces 和文件清单核对，允许站点改写 `info hash` 或标题；核对一致就挂回 qB，不会调用上传接口。目标种 pieces 或文件清单与本地内容对不上会直接拒绝。

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

回访只跑配置里有的站：不在配置里的站直接跳过，不会被回访到。AGSVPT 不在发布目标里就一直被跳过，这是它被拒后没人管、覆盖记录还能错写成 `seeding` 的原因之一（2026-11-26 核实）。

## 7. 发布相关配置

| 配置项 | 默认 | 说明 |
| --- | --- | --- |
| `assist.publishEnabled` | `false` | 总开关；关闭时只审不发 |
| `assist.autoReseedSites` | 46 站（2026-11-27 加 panda、iloli、muxuege、fenghuang；agsvpt 11-26 已加；gtk 当天复核后临时加入，等站点发布窗口） | 自动发种白名单。空的时候真实发布不会发出任何站；只在演练时会覆盖全部站点做发布检查预演 |
| `assist.reseedPausedSites` | 35 站 | 只停发种，辅种照常；熊猫已撤销发种暂停、移出本名单并留在白名单 |
| `assist.pausedSites` | 8 站（hudbt、freefarm、hdclone、u2、shadow、sunny、piggo、tu88） | 整站暂停 |
| `assist.roundBudget` | 200 | 每轮每站最多处理多少条 |
| `assist.uploadBudget` | 10 | 每轮最多真正发出多少次上传请求；发出请求就算一次，站点回“重复/已存在”也一样。日限额只管 `posted` 和 `posted_verifying`，两者分开算 |
| `assist.allowOtherVersion` | `true` | 站上有同名其他版本时照样发 |
| `assist.skipKeywords` | 16 个词 | 标题黑名单，命中不占额度 |
| `assist.reseedRules` | 42 条 | 站点级字段映射和下拉选项覆盖 |
| `assist.skipPostCheck` | `true` | 跳过发后自检；小批量验证时应关掉 |

`targetSites` 是旧界面留下的字段，引擎不读。

**让一个站真正参与自动发种，要同时做两件事**（2026-11-26 明确）：`sourceSites[]` 里把 `enabled` 改成 `true`，**并且**把站 id 加进 `assist.autoReseedSites`。只改前者只是进扫描范围，不进发种队列；只加后者而不启用也没用（配置加载阶段就过滤掉未启用站）。另外 `sourceSites[].managed` 是界面字段，引擎不读，改它不影响行为。

`reseedRules.<site>.selectPicks` 是**兜底**不是强制：只有当那个字段当前为空/0 时才写入，已经匹配到合适选项的不会覆盖。所以给 `team_sel` 配 Other 不会把本来能匹配驻站组的种子改成 Other；反过来，同名多候选按页面文档顺序取首个（muxuege 两个 AAC），拿不准就留空，不硬填。

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

填了 `announceURL` 后，该地址的主机会自动并入允许的 tracker 主机清单（和页面域名、内置别名、手写的 `overrides.announceHosts` 合并去重），通常不需要再手写 `announceHosts`；一个站有多个 announce 主机时，仍可用 `announceHosts` 追加。

地址必须属于这个站。程序会拿地址里的域名和该站登记的域名比对，对不上直接中止。不要把别的站的 passkey 填进来，也不要只填 tracker 主域名而漏掉你的个人参数。

### 7.1.1 个别站只能过本机代理：`overrides.proxy`

程序默认对站点请求直连（不读系统代理）。如果某个站本机直连不通、但浏览器走本机代理能打开（例如系统代理 `127.0.0.1:7890`，直连回 `os error 10060/10054`），可以在这个站的配置里单独填代理：

```json
{
  "id": "gtk",
  "name": "GTK",
  "url": "https://pt.gtkpw.xyz/",
  "overrides": {
    "proxy": "http://127.0.0.1:7890"
  }
}
```

- 不填或留空：维持默认直连，行为不变。
- 填了以后：只有这一个站的 HTTP 客户端走这个代理；qB 下载器、CookieCloud、其他站的客户端都仍然直连，不走代理。
- 日志里只会记代理的 `host:port`（如 `本站走代理 127.0.0.1:7890`），不回显带账号口令的代理地址。
- 代理地址填成不合法的 URL，构造站点客户端时会直接报错，不会静默退回直连。

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
- `descrOrder`：按标记检查段落顺序，缺段或倒置都拦；只对配置了的站点生效。2026-11-27 起，配了该规则的站还会在发种前自动装配分段（源简介六段齐就逐字节不动），装配后自查段序、图数下限、MediaInfo，缺任一项阻断不提交；详见 `ARCHITECTURE.md` 5.5.1。
- `titleNoCjk`：主标题含汉字直接拦，中文名放副标题。
- **独立海报/截图字段**：上传页把海报、截图单列时，在 `sourceSites[].overrides` 写 `posterField` / `screenshotField`（ptlgs 是 `cover` / `screenshots`）。程序从最终简介按顺序抽图：第一张填海报，其余一行一个填截图；字段不存在不硬塞，简介没图时清空。
- **海报/截图分开算底线（2026-11-26 起）**：除总图数底线（藏宝阁、优堡、太乙、麒麟、熊猫、ptlgs），另有两条独立口径：AGSVPT 至少 1 张海报 + 1 张截图；北洋园（tjupt）至少 1 张图。有独立海报/截图字段的站按字段算，没有独立字段的按“首图算海报、其余算截图”。
- **必需标签门禁**：站点配置可以声明上传必须勾选的标签（字段名 + 值 + 文案）；构造完成后没勾上，提交前拦下并说明缺哪个。代码不预设任何站点“一律勾某标签”，AGSVPT 在配置里声明 `tags[4][]` + `52`。注意标签值：方舟计划是 52，13 是杜比，51 是合并，不要混用。

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

**"请求没通"先分清三种情况（2026-11-26 补）**：

- 站点真挂了：首页也连不上，浏览器同样打不开。
- 只能过代理：这台机器系统代理开着（`127.0.0.1:7890`），浏览器走代理能开，而 `bsend` 默认直连（PT 站 passkey 与账号 IP 绑定，默认不走代理既避免连不上也避免算异常登录），所以报 `os error 10060 / 10054`。这类站可以在该站配置 `overrides.proxy` 显式开代理（见 7.1.1）；没配之前不算站点问题，也不算配置问题。gtk 已配该站代理、链路可达，临时加进白名单，等站点发布窗口再发。
- 人机验证拦住了：页面是 Cloudflare 挑战页或「Just a moment」，**不能当成站点关闭或没有表单**，要换真实浏览器、且标签页在前台。

表单演练的结果现在会带失败类别（`formcheck.json` 的 `errorKind`）：无权限、登录失效、2FA 中间页、人机验证、单页应用走接口、无表单。看这个字段比看笼统的"没有可解析的静态上传表单"准确。`formcheck` 和 `gatecheck` 现在用同一套逐站规则，两边结论应该一致；不一致就是 bug。

## 9. 相关文档

- 判断规则、发种流程、辅种规则：`ARCHITECTURE.md`
- 站点总账、白名单、受阻原因：`SITES.md`、`SITE-DEBUG.md`
- 还没做完的事：`ROADMAP.md`
