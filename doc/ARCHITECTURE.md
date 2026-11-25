# BoxSend 是怎么工作的

这份文档讲清楚三件事：BoxSend 为什么这样判断、一条种子怎么发出去、出问题时先看哪里。运行数据和命令在 `OPERATIONS.md`，站点情况在 `SITES.md`，还没做完的事在 `ROADMAP.md`。

## 1. 它解决什么问题

BoxSend 做两件事：

- **发种**：qBittorrent（下称 qB）里已经下好的影视内容，上传到另一个 PT 站。
- **辅种**：目标站已经有同一套文件内容，把站点的种子下载回来，挂进 qB 一起做种。

只处理影视、剧集、纪录片。教育、成人等类型由标题关键词黑名单拦掉，命中后不占当天的发布额度。

代码主体在 `rust/`：`bsend` 是引擎，`bsend-cli` 是命令行 `bsend.exe`，`bsend-gui` 是桌面界面。根目录的 `Sources/` 和 `Tests/` 是早期 Swift 版本存档，`windows/` 是打包脚本，都不是现在开发主线。

## 2. 判断目标站有没有：看 qB，不逐站搜索

这是现在最核心的规则。

- qB 里某段内容带着目标站的 tracker，就认为目标站已经有这段内容。
- qB 里没有目标站的 tracker，就认为目标站大概率没有。
- 平时不再为了“有没有”去逐个站点搜索。搜索很慢、容易被限流，站点改版后结果也不稳定。
- 只有真要发某一条之前，才对目标站再确认一次。这个“再确认”目前还没有完全接到新流程里，见 `ROADMAP.md`。

tracker 是种子文件里记录的站点服务器地址。判断时会把域名大小写、端口、路径、`user:pass@` 前缀、域名末尾的点这些差异归一化，也会认站点配置里的别名和子域名；页面带 `www.` 与裸域按同一站处理。否则同一个站换个写法就会被误判成“没有”。

### 2.1 这个判断的边界

qB 有 tracker，不等于站上一定还有这个种子。站点可能删种、账号可能被禁用，所以真正上传前还要再确认。

qB 没有 tracker，也不等于站上一定没发过。可能是站上已经有，只是本地没挂那份种子，或者换了域名。因此新流程在“确认时发现站上已有”的情况下，会把这条记成待人工核对，而不是直接当失败。

### 2.2 为什么不再写“没有”的记录

旧逻辑会给“内容 × 站点”的每个组合写一条“没有”的记录。147 个站、几千条内容，记录会越滚越大，而且大部分永远不会再读。

现在只记录真正发生过动作的组合：发过、辅过、失败过、跳过过。没有发过的组合不落记录，需要时从 qB 实时算。

## 3. 覆盖记录里还留什么

覆盖记录文件是 `coverage.json`，按内容分组，每组下面记各个站点的处理结果。字段名和手改边界见 `OPERATIONS.md`，这里只说每个状态是什么意思。

| 状态 | 白话解释 |
| --- | --- |
| `seeding` | 这个站在 qB 里已经有种子在做种 |
| `assisted` | 旧辅种流程挂上的种子；新流程不再产生带多个站点 tracker 的记录 |
| `reseeded` | 已经发布，并从目标站下载种子挂回 qB，校验完成 |
| `verifying` | 已挂回 qB，还在校验文件 |
| `failed` | 这次失败，过一段时间才能重试 |
| `review_pending` | 站上看起来已有，但 qB 没有对应 tracker，等人核对 |
| `wait_source` | 本地没有可用的源种子，导不出文件 |
| `reseed_draft` | 临时的待发队列；一轮结束后应该变成成功、失败或跳过 |
| `skipped` | 终态，不再自动发。只用于命中内容类型黑名单（`assist.skipKeywords`） |
| `pending_offers` | 站点明确要求改走候选入口（例如青蛙的 `torrent_too_large` 只肯走 `offers.php`）。不入队重试，但也不算完事：`status` 单列“待跟进”，等人去候选页提交，`bsend audit` 单独数 `pending_offers` |
| `other_version` | 站上已有同片的其他版本，分片不一样 |
| `same_hash_skip` | 目标种子和 qB 已有种子完全同一个 `info hash`，qB 不允许重复添加；查原种 tracker 和 progress，只挂目标站且 progress=1 时记 `seeding`（已做种），挂别站、没有 tracker 或还没下完才记本状态 |

从 2026-11-23 起，`autopost` 不再要求覆盖记录里先有 `reseed_draft` 条目。它每一轮直接读 qB 的实时种子列表，找出“白名单站点没有 tracker、源种子已定位、内容还在 qB”的组合，现场生成待发清单。`reseed_draft` 只作为过渡期的旧记录保留。

## 4. 覆盖率看哪个数字

命令行 `status` 和 `report` 现在把“qB 里有没有”作为主口径：

- **已有**：qB 里带该站 tracker 的内容数。
- **缺口**：全库内容总数减去“已有”。
- **覆盖率**：已有除以全库内容总数。

这三个数回答的是“这个站在我全部内容里铺开了多少”，不会因为“只挑容易判的内容去判”而虚高。

旧的记录口径还在 JSON 里：`cells`（判过的记录数）、`covered`（判过且已覆盖的记录数）、`coveredRatio`（已覆盖除以判过）供历史对比；另有一个 `libraryRatio`（已覆盖记录除以全库内容数）也从记录出发，只作旧视角参照。`status` 的新表已经不再把它们当主列。

## 5. 发一条新种的完整过程

```
从 qB 找缺口 → 选源种子 → 导出 .torrent → 改 tracker 和来源标记
→ 算新 hash 并确认不重复 → 上传 → 从目标站下载种子 → 独立挂回 qB → 校验
```

### 5.1 找缺口

每一轮从 qB 拉一次全量种子列表，按内容归组。对白名单里的每个站，如果这组内容没有任何一个成员种子带着该站 tracker，就算缺口。

还要同时满足：

- 源种子已经定位，记录里有详情页地址；
- 内容还在 qB 里，不是已消失的归档；
- 源种子没有“禁止转载”标记；
- 标题不命中内容类型黑名单；
- 站点没有暂停、没有在失败冷却期（默认冷却关闭）、今天没到限额。

满足后生成一条待发记录。同一个内容对同一个站只会出现一次。每站最多保留当天额度那么多条，避免一次生成几万条。

### 5.2 选源种子并导出

从这段内容的成员种子里选一个已经下完的，优先选能识别出 tracker 的私有站种子。用 qB 的 `torrents/export` 接口拿到原始 `.torrent` 字节，不重新做种。

### 5.3 改种子

只改两处：

1. 把 tracker 地址换成目标站的唯一地址；
2. 在种子信息里写入目标站专属的 `info.source` 值。

`info.files`、`info.pieces`、`info.name`、`piece length` 这些决定“这是什么文件”的内容原样保留。这样本地已有的文件可以直接复用，不需要重新下载。

`info.source` 是种子信息块里的来源标记，PT 站通常用它区分不同站的同内容种子。写入目标站的值以后，新种子的 info hash 会和源种子不同，qB 才能把两份种子同时挂住。

少数站点在 `takeupload.php` 阶段会自己改写种子元信息（1ptba 已逐字节确认是把 `info.source` 写成 `[1ptba.com] 1PTBA.COM`；cspt、dubhe、ptsbao、crabpt、hdbao、tey、oshen、ptfans、itzmx、railgun、lemonhd 实测也出现 hash 差异，pieces 和 `info.name` 未变，但具体改写字段没有逐字节核对），此时站上返回的 hash 和本地构造的 hash 必然不同。程序默认仍要求 hash 相等；只有实测确认的站点白名单、且 pieces 与 `info.name` 都一致时才放行，并在日志里写明原因。

个别站另有上传文件本身的大小上限。`itzmx` 的 `takeupload.php` 只收不超过 `266,240 bytes` 的 `.torrent`，大合集/大剧包会在上传后被业务错误框拒绝；给这类站选种时先看源种导出的 `.torrent` 大小，不要只看内容体积。

### 5.4 目标站的 tracker 地址从哪来

上传的种子必须带目标站自己的 tracker 地址，而且这个地址里要有你的个人 passkey（站点用来认人的一串字符）。程序按下面的顺序取：

1. 先看这个站在 `boxsend.json` 里有没有配 `overrides.announceURL`；
2. 没配，就从 qB 现有种子中找一条属于该站的 tracker，把整条地址抄过来；
3. 两边都找不到就中止，不会伪造一个没有 passkey 的地址去上传。

第一次向某个站发种时，qB 里本来就没有这个站的种子，第 2 步一定拿不到。所以新站接入时要在配置里补一条：

```json
{
  "id": "hdsky",
  "overrides": {
    "announceURL": "https://tracker.example.com/announce.php?passkey=你的个人密钥"
  }
}
```

地址从站点自己的“做种/下载”页面复制，必须是这个站、这个账号的完整 tracker 地址。程序上传前会核对地址里的域名确实属于该站，填错会直接中止，不会把种子发到别的站。

### 5.5 上传前必须检查

- 新种子的 info hash 不能已经存在于 qB；
- 上传构造的新种子只能有一个 tracker，不能把多个站合并；
- 目标站地址必须属于该站：优先用配置文件里的完整地址，没配才从 qB 现有 tracker 抄；不能拿别的站地址凑数。

任何一条不满足就中止，不带着疑问上传。

### 5.5.1 简介门禁（2026-11-23 起）

`gates.rs` 读的是**最终要提交的简介**（`apply_site_rules` 剪死图/转 BBCode 之后），不是源页原文。图相关的两条是**代码硬规则，不读配置**：

| 规则 | 作用 |
| --- | --- |
| 剪死图（无条件） | 每个站、每次提交前逐张 GET 简介图片，非 2xx/非图片类型就剪掉（带目标站上传页 Referer） |
| 要图站至少 3 张 | `gates::image_floor` 按站点硬编码：藏宝阁/优堡/太乙/麒麟 3 张，熊猫 4 张；死图被剪掉后数量不够就拦 |

站点规范里仍按站配 `titleNoCjk`（主标题出现汉字即拦）、`descrOrder`（按给定标题标记检查顺序）、`requireLinks` / `requireMediainfo`。熊猫高清配 `titleNoCjk=true`，图数由硬规则给 4 张，并要求 `【转载出处】→【制作说明】→【海报】→【简介】→【info】→【截图】`；配置文件里的 `requireImages` 一律被覆盖。图片全部失效时不要绕过门禁，先把原图/海报/截图重传到可用图床，再改简介。

图床选择：`imgbox.com` 匿名可用、无需 API key、单图 10MB，实测带藏宝阁/熊猫 Referer 都是 `200 image/jpeg`。仓库脚本 `scripts/imgbox-upload.py` 负责批量上传并输出原图直链；`scripts/edit-seed.py` 负责只改已发布种子的标题/简介，不动种子文件。imgbox 不承诺永久，重要图仍应自建或另存。

`scripts/edit-seed.py` 默认只读演练，`--apply` 才 POST；除指定字段外其余表单原样回填，避免误改分类/媒介/标签。编辑已发布种子后，站点会重新进审核队列，是否通过以站点状态为准。

### 5.5.2 站内判重：audiences 走 Kimi WebBridge（2026-11-25 起）

audiences 的 `torrents.php` 有 Cloudflare Turnstile，无头 HTTP 拿不到搜索结果，还可能把挑战页当成“站内没有”。这条路固定走 Kimi WebBridge：

- 用用户已登录的真实浏览器打开 `{base}/torrents.php`，轮询 `input[name=cf-turnstile-response]`，最多 40 次、每 500ms；没有挑战区或 token 非空才算页面就绪。状态里同时读 `document.visibilityState` 和 `hasFocus`。
- 就绪后用同源 `fetch(url, {credentials:'include'})` 抓带查询的搜索页，HTTP 非 200 或回包含“搜索人机验证未通过”直接报错。
- IMDb 区判重也走同一个浏览器抓取入口，不再单独发无头 HTTP 请求；否则挑战页会被误读成“没有 IMDb 命中”。
- token 为空且页面 `visibilityState=hidden` 时，明确报“标签页在后台，Turnstile 不会生成验证框”，提示把标签页切到前台；不退回无头 HTTP、不盲发。2026-11-25 实测：Kimi 能选中已打开的 audiences 标签页，但 Edge 窗口最小化时页面一直是 hidden，显式 `turnstile.render` 也没有 token；把页面切到前台后重试。

代码在 `kimi.rs` 和 `sites/nexusphp.rs` 的 `uses_kimi_search` / `fetch_search_html` / `search_sized_browser`；`KimiBrowser` 默认 `http://127.0.0.1:10086/command`，session 固定 `bsend-audiences`，可用 `KIMI_WEBBRIDGE_URL` 覆盖地址。

### 5.6 上传与挂回

上传成功后，从目标站下载刚发布的 `.torrent`，作为一份**独立种子**加进 qB。这是**硬步骤**：下载失败不是“发完就算”，程序会在 4 轮、每轮 15 秒内重试（站点刚建种时常先回 `torrent not registered`），仍拿不到才记 `posted_unresolved` 待人工，回访用 `bsend pull` 补；站点要先审才给下载的落 `pending_review`，审完由回访自动挂回。上传回包里的 `details.php?id=` 不能单独当证据：成功页常混着侧边栏的相关旧种，实测直接取第一条会挂错。程序现在按 id 配对详情和下载链接，最终跳转是详情页时优先用它；只有一类链接时用唯一那条；详情和下载的 id 对不上就两个都不用。拿不到链接时会再按「发布名 + 本地总大小」做站内检索，结果行或详情页大小对不上就不采；仍然定位不到才记 `posted_unresolved` 待人工确认，不会拿本地构造种顶替。

大小初筛的容差是 `max(0.5%, min(2 MiB, 1%))`，再额外带上站点文本的显示精度：页面写 `274.04 GiB` 时，`0.01` 个显示单位（约 10.7 MiB）也算合法余量；`1.00 MB` 的量子只有 10 KiB 级，不会套用 GB 的绝对余量。这个余量只用于「名字 + 大小」挑候选，最终仍靠下载回来的 `pieces` 精确比对，不能拿显示大小当同一内容的证据。

详情页上的总大小不是“看到数字就算”，同一个页面里有好几种带数字的东西都不是种子总大小：做种数、日期这类纯数字，简介里 MediaInfo 的单个文件体积（`Size : 7.99 GiB`），文件列表每一行的体积，页眉的账户统计（`上传量：1.35 TB`）。标准 NexusPHP 还把大小和类型、媒介写在同一格里（`大小：77.43 GB 类型: 动漫 …`）。程序只认“数字 + 显式单位”，先找以「大小 / Size」开头的单元格并取标签后的第一个大小，找不到再退回“整格只有一个带单位大小”的短单元格；MediaInfo、文件列表、页眉统计一律跳过。取不到就不参与大小比对，不会拿计数凑一个字节数，也不会因为一个假大小把正确候选判成“大小不符”。

挂回前还要过两道校验：下载回来的目标种 `info hash` 默认必须等于上传时构造的新 hash（判重/existed 路径没有“新 hash”，只核对 pieces 指纹）；目标种 pieces 指纹必须与 qB 里某个完整本地副本一致（不一定是上传时的源种，分片长度可能不同）。已知会改写 `info.source` 的站（ptba、cspt、dubhe、ptsbao、crabpt、hdbao、tey、oshen、ptfans、itzmx、railgun、lemonhd）只有在 pieces 指纹和 `info.name` 都一致时才放行 hash 差异，其他站仍拒绝；任何一条不满足就拒绝挂回，宁可人工处理，不往 qB 塞错种。

站点上其实已经发布（新发成功或 existed），但下载目标种、校验或挂回 qB 失败时，不记发种失败：格子落 `ReviewPending`，note 前缀 `posted_unresolved` 或 `existed_unresolved`，不进失败冷却、不占发布额度，下一轮或人工按详情页补挂。

历史欠账可以手工补：把站点详情页上的种子下回来、按 pieces 找到本地内容后挂进 qB，不重新上传；命令是 `bsend pull --site <站点id> --url <详情页>`（旧名 `reattach` 仍可用）。它只拉详情页、下载站点生成的 `.torrent`。**不再接受本地构造的 `.torrent`**：站点审核没通过时挂本地构造种只会得到一条永远没有 tracker 响应的废种。

站点审核是另一条线：站点收下了种子但还没放行时，拿不到站点种子，上面的挂回流走不通。这时格子落三个专门状态之一——`PendingReview`（等站点审核）、`ReviewRejected`（站点明写审核不通过）、`GoneFromSite`（回访时详情页说没有这条）——都不算失败、不重发。页面标记由 `sites/nexusphp.rs` 的 `review_mark` 识别，只认明写的字样，且只在详情页“标题到下载链接”之间那一段找（免得把导航栏的“审核中心”算进来）：等审核（`title="未审"`、“尚未通过审核”等）→ 不通过（“审核不通过，原因：…”、`title="不通过"`）→ 通过（只认 `title="通过"`），明写的压过“请稍后再试”这类含糊说法。详情页说“没有该ID的种子”或 HTTP 404 时抛 `NoSuchTorrent`，和登录失效、限流区分开。收尾在 `review.rs`：`decide`（纯函数）看“页面现在怎么说 × 格子现在是什么”决定不改 / 改结论 / 收尾；`recheck_reviews` 负责打站点、写格子，审完的就走 `pull_with_data` 把站点种子挂进 qB（下载器里本来就在做种的只改格子）。自动发种每轮开头调一次，`bsend followup` 可手动跑；`--all` 才带上我们自己发出去的条目（已转种 / 校验中，或带详情页链接的“已在做种”；扫描按下载器 tracker 写的“已在做种”没有链接，那是站上别人的种子，不看）。覆盖扫描的“已在做种”标记不覆盖这三个状态——qB 里挂着该站 tracker，不代表站上还在审、没删这条（实测熊猫 141276：qB 还挂着，站上详情页已报没有这条）。

挂回 qB 时沿用源内容的保存目录，并处理“多文件种子是否带根目录”的差异，否则 qB 会新建一个空目录，校验一直是 0%。这部分见 7.2。

挂回后核对 tracker：全部 tracker 的 host 必须属于目标站。站点自己的种子偶尔带两条同站 announce（例如 hdvideo 的 `tracker.hdvideo.top` 和 `hdvideo.top`），这不是多站合并，保留站点原样；只要出现别的站 host 就中止，不补挂、不改原种子。`host_matches` 会把 `www.` 剥掉再比，并认站点配置里的 announce 别名；例如 cspt 页面在 `cspt.top`、种子 tracker 在 `tracker.cspt.cc`，tey 页面在 `pt.tey.cc`、种子 tracker 在 `tracker.tey.cc`，qingwa 页面在 `qingwapt.com`、种子 tracker 在 `tracker.qingwapt.org`，不补别名会被误判成别站或 qB 覆盖 0。

选 announce 也是同一套口径：从 qB 现有种子里取该站 announce、校验 `overrides.announceURL` 的 host、挂回后核对 tracker，三处都走 `host_matches`（选 announce 和校验经 `qbittorrent::announce_host_matches`）。2026-11-24 之前前两处写的是另一套“相等或以 `.host` 结尾”，站点地址带 `www.` 时（如 `www.pthome.net`）会把自己的 `t.pthome.net` 当成别站，没内置别名就报“不在 announceHosts 里”。

### 5.7 校验

种子加进 qB 后等它校验到 100%。到了就记 `reseeded`；没到就记 `verifying`，下一轮继续查，不重复发。

## 6. 一个种子只挂一个站点

这条规则管的是“一个种子只向一个站汇报”，不是机械地只许一条 URL：

- 上传构造的种子只保留目标站一个 tracker；
- 禁止调用 qB 的 `addTrackers`；
- 禁止把源站和目标站的 tracker 合进同一个种子；
- 发布后从目标站下载种子，独立添加，不和源种子混在一起；
- 目标站自己的种子若含多条同站 announce（同一站的不同域名），允许原样保留；挂回后程序会核对每条 tracker 的 host，出现别站就中止。

原因很直接：PT 站通常禁止一个种子同时向多个站汇报；混在一起还容易出现 passkey 泄露、站点判定作弊、两边都停止做种。独立种子各有各的 hash，互不影响。

## 7. 辅种怎么做

### 7.1 先比 pieces 指纹，不比标题

先把几个容易混的叫法定死：

- **`info hash`**：整个 `info` 字典的 SHA1，是种子信息块的身份。tracker/announce 不在 `info` 里，所以同一份内容只换 tracker 仍可能得到同一个 `info hash`；qB 因此不能同时存两份同 hash、不同 tracker 的种子。
- **`pieces`**：`info` 里按每个分片算出的 20 字节 SHA1 串；分片长度不同，`pieces` 就不同。
- **pieces 指纹**：代码里的 `pieces_hash`，是对 `pieces` 整串再算一次 SHA1，只用于快速比对内容。它既不是单个分片的 SHA1，也不是 `info hash`；指纹相同通常说明分片串一致，但仍要核对文件清单和分片长度才算同一份内容。
- **分片长度**：`piece length`，例如 8 MiB、16 MiB；文件内容一样但分片长度不同，`pieces` 和 pieces 指纹都会不同（hdvideo 7607 的 8 MiB 目标种和 sbpt 的 16 MiB 种就是这种情况，只能拿 qB 里同是 8 MiB 的本地副本去对）。

| 情况 | 判断 | 做法 |
| --- | --- | --- |
| 目标种 `info hash` 和 qB 不同，但 pieces 指纹与某个本地完整副本一致 | 可以跨站辅种 | 新加一份种子，复用本地文件，等校验到 100% |
| 目标种 `info hash` 和 qB 完全相同 | qB 不允许同 `info hash` 两份 | 先查原种 tracker 和 progress：只挂目标站且 progress=1 → 记 `seeding`（已做种），不再 add；挂别的站、没有 tracker 或还没下完 → 记 `same_hash_skip`，不能给原种补挂，也不能新增第二条 |
| pieces 指纹和所有本地完整副本都不一致 | 站上是另一个版本，或分片边界不同 | 不能拿本地文件辅种；但可以按“允许其他版本”继续发自己这条 |

判断以 `info hash` 和 pieces 指纹为准。标题一样不代表文件一样，标题不同也不代表不能发。

### 7.2 多文件种子的根目录

qB 加多文件种子时，默认会放到 `<保存目录>/<种子根目录>/`。如果目标站的根目录名和本地不一样，qB 会新建一个空目录，校验结果就是 0%。

程序会先比较“去掉根目录后的文件清单”：

- 完全一致：把保存目录指到已有内容，并告诉 qB 不要再套一层根目录（`contentLayout=NoSubfolder`）；
- 对不上：拒绝添加，记 `Mismatch`，不猜路径。

### 7.3 什么算“站上已有”

只认两种证据：

- 上传后最终跳转地址带 `existed=1`；
- 页面错误框里出现明确的重复措辞，例如“已经上传”“已存在”“被人上传”“already exists”。

上传表单被原样返回不算重复，页面里随便出现“存在”两个字也不算。旧逻辑就因为整页搜“已存在”，把校验失败页误判成重复。

比对标题之前会先裁掉站点追加的角标，例如 `[免费] 剩余时间：23时59分钟`、`[50%]`：它们不是标题正文，不裁的话真重复会因为“标题不符”被放过，或者反过来把不同的条目当成同一条。裁的位置是标题里最早出现的那个角标，和内置角标表的顺序无关；正常不带角标的标题不会被误裁。

## 8. 失败、冷却和限额

自动发种默认不做冷却：被拦或失败后，下一轮直接重试，由人看审计（`auto-audit.jsonl`）决定要不要再跑。`assist.backoffHours` 不填或填 0 是默认；只有显式配了小时数才按“同一条内容 + 同一个站”在窗口内跳过，拦截和失败共用一个窗口，试运行同样受限。`pending_offers`（等候选入口）不算失败，不进冷却。

日限额按北京时间切日，只统计真正发布成功的条数。被拦截、重复、失败不占发布额度；旧代码里重复结果也会消耗额度的问题还要继续修。

## 9. 自动运行

`bsend autopost --loop --interval-secs 1800` 会串行循环：

```
找缺口 → 生成待发清单 → 源种子审阅 → 发布检查 → 上传 → 下载挂回 → 记审计
```

任何一步失败只影响当前这条，不跳过审计。审计写不进去时会停止后续发布，避免当天限额失控。

Windows 计划任务只给命令示例，不自动创建。需要常驻时用任务计划程序调用上面的命令。

## 10. 发布范围怎么定

现在有两套开关：

- `sourceSites[].enabled`：参与扫描和生成待发的站点范围；
- `assist.autoReseedSites`：自动发种白名单。空的时候真实发布一条也不发，防止漏填后把全部站点都发一遍；只有演练会拿全部站点做发布检查预演。

实际发布时还要排除：

- 整站暂停 `assist.pausedSites`；
- 只停发种 `assist.reseedPausedSites`；
- 当天额度已满、还在失败冷却期的站点（冷却默认关闭）。

`targetSites` 是旧界面留下的字段，引擎不读，可以忽略。

## 11. 代码在哪里

| 文件 | 负责什么 |
| --- | --- |
| `reseed.rs` | 单条内容的审阅、上传、发后挂回，主流程，文件很大 |
| `qbittorrent.rs` | qB 登录、种子列表、导出、添加、校验 |
| `coverage.rs` | 内容索引、tracker 归一化、覆盖记录 |
| `report.rs` | 报表、从 qB 缺口生成待发清单、对账、审计汇总 |
| `autopost.rs` | 自动发种编排、限额、失败冷却、审计 |
| `matcher.rs` | `info hash`、pieces 指纹、名称判重 |
| `sites/nexusphp.rs` | NexusPHP 站点的表单解析与上传 |
| `config.rs` / `gates.rs` | 配置结构、发布检查规则 |
| `bsend-cli/src/main.rs` | `status`、`report`、`autopost`、`audit` 等命令 |

`reseed.rs` 已经超过 5,000 行，计划按“共享入口、主流程、判重、上传、记录读写”拆开。拆分只移动代码，不改行为，每拆一批都要能编译和跑测试。

## 12. 已经踩过的坑

- qB 5.x 的种子列表不再返回完整的 tracker 集合；在一个种子只挂一个站的前提下，单个 tracker 字段就够判断。
- 上传回包的 `details.php?id=` 不可信；实测多条新种的初始 id 都打不开，曾把旧种当成新种。
- 整页搜“已存在”会误判；必须看错误框的结构。
- `<meta name="description">` 不是上传字段，只认 `input`、`textarea`、`select`。
- Cloudflare 会对表单里的内联 `on*=` 属性计数，4 处就可能 403；提交前要清掉事件属性。
- MediaInfo 的图形界面版不能无界面运行，必须调 `MediaInfo.dll` 的 C 接口。
- 搜索表单可能污染上传字段的类型；定位上传块时按 `type=file`、`takeupload`、`upload` 打分。
- 早期每个请求慢 30 秒是系统代理残留；qB 的版本接口也不再返回 JSON，连通性要看种子列表接口。
- 30 秒全局超时对**上传**这一步不够：站点要在服务端解 bencode、算 hash、查重再落库，北洋园实测回不来，客户端会先断开（断开后站上也不会有记录，不用担心中途落库重复）。所以只有 `post_upload` 放宽到 120 秒，列表和检索仍是 30 秒。
- 有些站点的“拒绝”长得像成功页：青蛙的 Poirot 合集回包是一个自动 submit 到 `offers.php?add_offer=1` 的 `torrent_too_large` 表单。只看“有没有报错字样”会把它当成功，得按表单 id 识别。

## 13. 相关文档

- 运行数据、命令、巡检、排障：`OPERATIONS.md`
- 站点清单、白名单、受阻原因：`SITES.md`、`SITE-DEBUG.md`
- 还没做完的事：`ROADMAP.md`
