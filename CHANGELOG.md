# Changelog

版本以 git tag 为准（Rust workspace 当前 `0.1.0`）。只记录用户可见/行为变化；
逐条技术细节见 `doc/ARCHITECTURE.md`、`doc/OPERATIONS.md`、`doc/ROADMAP.md` 与 git 历史。

## [Unreleased]

### Changed

- 发布标题按站点规范重排：完结剧集去掉 `Complete`，技术参数固定“分辨率 → 帧率/HDR → 片源 → 地区 → 视频编码 → 位深 → 音频 → 音频细节 → 压制组”；带 `USA`、`MA`、`5.1` 的标题不再被塞到音频后面。站点改写标题时也按同一顺序生成。
- 4K 60fps 选档不再留空：`standard_sel` 同时有 `2160p/4K` 和 `2160p/4K 60Fps` 时，标题写 60fps 就选 60Fps 档，没写就选普通档（NovaHD 实测）。
- 新增覆盖状态 `pending_offers`（待走候选入口）与审计词 `pending_offers`：站点要求改走候选页（`offers.php`）时不再落 `skipped`，也不当失败反复重试；`bsend status` 单列“待跟进（等站点侧动作，程序不自动重试）”，`bsend audit` 合计带“待走候选”。
- 发种失败冷却默认关闭：`assist.backoffHours` 不填或填 0 时，被拦或失败的条目下一轮直接重试；只有显式配小时数才在窗口内跳过。GUI 诊断页与日志改成“冷却”口径。
- 文档与规则改用白话：不再把命令名当动词/术语，正文先讲做什么再给命令；AGENTS.md 已加这条规则。
- 命令 `bsend reattach` 改名为 `bsend pull`（把站点上已发布的种子下回来挂进 qB）；旧名保留为隐藏别名，老脚本仍可运行，`--help` 里只显示新名。
- `bsend status` 的“待跟进”按类点名：等走候选入口 / 等站点审核 / 站点审核没过 / 站上已找不到 / 站上已发但 qB 未挂回，每类列出条目和站点当时说的话（原来只有数量，或只有候选入口一类点名）。
- `bsend audit` 每天小计和合计都带“待跟进”一行（待走候选 / 等站点审核 / 站点审核没过）。
- GUI 覆盖概览的状态图例补齐：“等走候选”“等站点审核”“审核没过”“站上已没有”各用自己的颜色和名字，不再落进“已跳过”的兜底；图例改为穷举全部状态，以后新增状态编译会报错。
- **audiences 判重改走 Kimi WebBridge**：`NexusPhp` 对 audiences 不再用无头 HTTP 检索，改在用户已登录的真实浏览器里先打开种子列表页、等 Cloudflare Turnstile token，再同源 `fetch` 搜索 URL；token 未就绪时明确报“需要人工完成一次人机验证”，不把验证页当“站内没有”。2026-11-25 实测补上后台标签页诊断：`visibilityState=hidden` 时 Turnstile 不生成 iframe，程序单独报“标签页在后台”，提示切到前台后重试。
- **源站简介没有图时自动补图**：新增 `scripts/seed-images.py`，用本机 ffmpeg 从视频抽帧，也可按豆瓣条目自动取海报（`--douban`）或传海报 URL，上传到 pixhost/imgbox，写入 `screenshot-overrides.json`；发布时程序自动把这些图追加到简介。波洛候选 carpt 532 已补 3 张，tey 预检从“0 图”变为“3 张 ≥ 3”；tey 仍未上传，是站点 `medium_sel` 没有 Bluray 选项，不是图床。
- **图片门禁改成代码硬规则**：剪死图不再读配置，所有站点上传前无条件逐图探测、剪掉死图；要图的站至少 3 张（`gates::image_floor`：藏宝阁/优堡/太乙/麒麟 3 张、熊猫 4 张），配置里的 `pruneDeadImages` / `requireImages` 已删除、写了也会被覆盖。
- **上传成功就立刻挂回 qB 改成硬步骤**：上传后立即下载站点 `.torrent`、核对分片并加入 qB；站点刚建种时的 `torrent not registered` 固定重试 4 轮、每轮 15 秒，仍拿不到才落待人工。所有站点、所有发种入口都走这条，没有配置开关。
- **熊猫暂停发种**：`panda` 从 `assist.autoReseedSites` 摘掉并列入 `assist.reseedPausedSites`；站上 141276 是候选种子 24 小时没做种被系统删的，重发需先“上传→立刻挂回 qB 做种”。
- 2026-11-24 晚把《伪装者》同一内容的另外四站简介死图换掉：crabpt 240242、hdfans 287791、qingwa 208575、luckpt 61102 的 9 张 `i13.tietuku.com` 全部换成 imgbox，复查 0 张旧图 / 9 张新图；sbpt 21684 本来没有图片，不需要换。

### Added

- `scripts/edit-seed.py --clear-technical-info`：清空 `technical_info` 字段，配合 `--descr-file` 处理不收 MediaInfo 的站点（财神 260049/260050 实测）。
- `scripts/seed-images.py`：源站简介没有图时，按内容键从本地视频抽帧（默认 3 张，可多条视频各取一帧），可选 `--douban` 自动取豆瓣条目海报或 `--poster-url` 传图，上传 pixhost/imgbox 后写 `screenshot-overrides.json`；发布时自动追加进简介。
- `scripts/seed-images.py` 默认用 pixhost，避免 UBITS 的 imgbox 黑名单；`--dry-run` 只抽帧不上传。
- “等站点审核”做成机器可跟踪：新增覆盖状态 `pending_review`（等站点审核）、`review_rejected`（站点审核没过）、`gone_from_site`（站上已找不到）和审计词 `pending_review` / `review_rejected`。发种时站点收下了但不给下载（“尚未通过审核”等）或页面标着未审 / 不通过，就落对应的格子，不算失败、不重发；自动发种每轮开头和新命令 `bsend followup` 会回访详情页，审完了自动把站点种子下回来挂进 qB。页面标记只认明写的字样（tey `title="未审"`、麒麟“审核不通过，原因：…”、藏宝阁 `title="通过"`、熊猫“没有该ID的种子”都是实测样本）。
- 发布前门禁新增 `titleNoCjk`、`requireImages`、`descrOrder`：主标题含汉字、最终简介图片不足、简介段落乱序都会在上传前拦下；门禁改为读 `apply_site_rules` 之后真正要提交的简介，不再看源页原文。藏宝阁配 2 张图 + 剪死图，熊猫高清配 4 张图和【转载出处-制作说明-海报-简介-info-截图】顺序。
- `scripts/imgbox-upload.py`：匿名上传 imgbox，返回原图直链；无需 API key，单图 10MB，实测带藏宝阁/熊猫 Referer 均 200。
- `scripts/edit-seed.py`：只改已发布种子的标题/简介，默认演练，`--apply` 才提交；其余表单原样回填。
- `scripts/fetch-descr.py`：读已发布种子的编辑页，把简介字段原样落盘，给换图/改简介做准备；只读不提交。
- `scripts/swap-descr-images.py`：按出现顺序把简介里的图片链接 1:1 换成新直链，BBCode 与 HTML 都支持，张数对不上报错不写文件。
- `scripts/set-cell-note.py`：按内容键 + 站点改覆盖记录里那一格的说明（区间替换、保持原换行、先备份）。
- 2026-11-24 修三条图片类审核退回：ubits 379266 的 9 张图全在已失效的 `i13.tietuku.com`，换成 1 海报 + 8 张截图；tey 623 是同一条内容，同样换掉；麒麟 479362 被判“缺少海报或截图”，7 张图全不可达（豆瓣海报裸请求 418、`img.pterclub.com` TLS 失败），换成 1 海报 + 6 张截图。三条都只改简介、不重发；页面仍挂着“不通过 / 未审”，等管理复审，`bsend followup` 接着盯。
- 2026-11-24 查清熊猫 141276：是《星球大战9：天行者崛起》UHD BluRay 1080p 单文件（11.56 GB，https://pandapt.net/details.php?id=141276 ）。站内信两条：第一次被 NoCaps 判主标题含中文、简介顺序不合规；重提的那版被系统当“候选种子”，24 小时内没做种自动删除。不是审核没过被删。qB 里还挂着那条（tracker 报 `torrent not registered`），删还是重发待定。
- 2026-11-23 修复藏宝阁 43994：豆瓣海报 418/HTML、`imgs.loukky.com` TLS 失败，重传 imgbox 后详情页两张图正常；修复熊猫 141276：主标题去掉 `[50%] 剩余时间` 中文尾巴，简介重排为六段并补海报 + 3 张截图。
- 辅种：pieces-hash 批量通道 + 名字检索（严格 / 宽松 / 最大成员种名兜底）。
- 转种：预检 → 官方预审 → 上传 → 发后自检 → 推种校验全流程；`autopost`
  白名单 + 日限额（北京时间）+ 失败冷却（默认关闭）+ 审计（`auto-audit.jsonl`）。
- GUI（GPUI）：概览 / 转种草稿 / 自动发种 / 配置 / 诊断 / 帮助。
- 引擎新加入下载器的种子统一带 `Boxsend` 标签；已存在的同 hash 种子不改动（不补挂 tracker、不改标签）。
- Rust release 工作流（`v*` tag 触发，CLI + GUI + SHA256SUMS）。
- 搜索结果多候选择优：同片多版本时优先取名称最接近发布名的条目。
- `reseed-log.jsonl`：CLI/GUI/autopost 每次转种落一行动作日志，含 `seeded`
  （是否已确认在 qB 做种），便于排查「站点发了、qB 没辅上」。
- `scripts/seeding-audit.ps1` + `doc/OPERATIONS.md`：只读巡检覆盖率异常记录、
  未成种记录与下载器卡种；不打印 cookie/passkey。
- `scripts/site-matrix.ps1` + `doc/SITES.md`：只读生成 147 站适配矩阵
  （启用/暂停/停转种/实测/formcheck/pieces/cookie 数）。
- `formcheck` 落盘记录补充提交地址（`action`）与每个下拉的全量选项表（`selectOptions`）；
  旧字段保持不变，站点矩阵脚本兼容。
- 覆盖报表新增按 qB 统计的“已有 / 缺口 / 覆盖率”：`qbCovered`、`qbMissing`、`qbRatio`；
  `bsend status` 单站表改为这三列，`bsend report` 同时保留旧列作历史对比。
- 自动发种的待发清单改为每轮从 qB 实时种子列表派生：白名单站点没有 tracker、源种子已定位、
  内容还在 qB、没有禁止转载才生成；同名内容对同站只出一条，按体积排序并按每站额度截断。
- 文档收敛（2026-11-23）：17 份 `doc/` 文档合并为 5 份（`ARCHITECTURE` / `OPERATIONS` /
  `SITES` / `SITE-DEBUG` / `ROADMAP`），批次报告与历史实机记录并入后退役；正文改写为白话，
  先讲原因再讲做法，技术字段首次出现时附一句解释。
- 站点配置新增 `overrides.announceURL`：填写该站含个人 passkey 的完整 tracker 地址。
  第一次向 qB 里还没有该站种子的站点发种时，靠它构造只带目标站 tracker 的上传种子；
  不填则继续从 qB 现有 tracker 里找，两边都没有时中止并提示配置。
- 转种链路改为单种子单站点：从 qB 导出源种后字节级改写 tracker、删 `announce-list`、
  写目标站专属 `info.source` 生成新 hash；上传后从目标站下载 `.torrent` 独立挂回 qB，
  不再调用 `addTrackers`，也不把多个站的 announce 合并进同一个种子。
- 新增 `bsend pull --site <id> --url <详情页>`（当时叫 `reattach`）：把站上已发布、qB 未挂回的种子下回来挂进 qB；
  `--key` 指定内容，`--file` 在站点拒绝下载时给本地构造的 `.torrent`。
- 2026-11-23 继续首次向 `railgun`、`lemonhd` 发种并挂回：两站都发布 `carpt` 的 Poirot 合集，
  站上详情分别为 `60632`、`48970`，站上 hash 与构造 hash 不同（railgun `72889498…` vs 构造 `6debbce8…`，
  lemonhd `3c00cbb2…` vs 构造 `d50dfa19…`）；两站上传回包都没给详情链接，先落 `posted_unresolved`，
  加入 hash 改写白名单后挂回，qB 校验 100%，tracker 分别只有 `bilibili.download`、
  `tracker.lemonhd.net`。两站目标站都已有其他版本，按“同名其他版本照样发”策略继续发布。
  累计首次实发 12 站、13 颗目标种。挂回后读 tracker 列表增加 3 次短重试，
  避免刚 add 的种子在 qB 里还没建好 tracker 列表时被误跳过单站点核对（railgun 实发时出现过一次）。
- 2026-11-23 首次向 `ptba`、`cspt`、`dubhe`、`ptsbao`、`crabpt`、`hdbao`、`tey`、`oshen`、`ptfans`、`itzmx` 发种（hdbao 2 条，其余各 1 条）并挂回：十站都出现站上 hash 与上传构造 hash 不同（ptba 已确认是 `info.source` 被改写，
  cspt/dubhe/ptsbao/crabpt/hdbao/tey/oshen/ptfans/itzmx 具体字段未逐字节核对）；按 pieces + `info.name`
  校验后补挂成功，qB 校验 100%。hdbao 的 `autopost --limit 1` 曾连发两条，已修：
  `posted_unresolved` 也是一次上传，必须计入 limit。tey 的 qB 老种走 `tracker.tey.cc`，
  已补 announce 别名，qB 已有从 0 变 5，新种挂回后为 6；该站 `medium_sel` 没有 Blu-ray 档，
  蓝光/Remux 会被选档门禁拦，HDTV/WEB-DL 可发，新种当前状态“未审”。oshen 前两次站点抖动，
  第三次恢复并发布，详情 `57082`，qB 校验 100%；ptfans 上传页/判重回稳后一次通过，
  详情 `11111`，qB 校验 100%。itzmx 的 torrent 文件上限为 `266,240 bytes`，大合集候选会被拒；
  改用 `ubits` 源的《Liar Game》小种（源种 96,934 bytes）首次发布 `13134` 并挂回，
  qB 校验 100%，详情页无“完结/中字”标签，需人工补。
- 2026-11-24 凌晨向 `pthome`、`nanyang`、`ubits` 发种：三站上传成功、站上 hash 与构造 hash 不同，
  分别是 pthome `284701`（源 hdfans 133877《人民的名义》）、nanyang `226159`（源 soulvoice 54682《知否知否应是绿肥红瘦》）、
  ubits `379266`（源 hdsky 27637《伪装者》）。三站已加入 hash 改写白名单（连同此前十二站共十五站），
  并用 `bsend pull` 把站上的种子下回来挂进 qB：qB 校验 100%、只挂单站 tracker、状态 `seeding`（补挂后覆盖记录一度被写成
  `same_hash_skip`，已用 `bsend prime` 按 qB 真值改回）。同一批的 `hdfans` 回包带 `existed=1` 已核实：
  `287791` 就是同一条《伪装者》，qB 里 11-23 23:49 就在做种、校验 100%，覆盖记录 `reseeded`，没有重发。
- 2026-11-24 `tjupt`：announce 地址改为以站点下载的种子为准（`tracker-public.tjupt.org`，https + passkey），
  写进 `overrides.announceURL`；上传仍撞 `takeupload.php` 超时，上传单步超时已放宽到 120s。
  站上已有 `416154`（FLAC/DDP5.1 版本）与 `236519`（同名 FRDS 合集、pieces 与本地不同），属实不同内容。
- 2026-11-24 `qingwa`：Poirot 合集的“未识别回包”已定性——回包是 `torrent_too_large` 表单自动 submit 到
  `offers.php?add_offer=1`，站点要求改走候选入口；代码已识别并落 `pending_offers`（待走候选入口、不自动重试，后续在 `status` 里跟进）。
  同站《伪装者》发布 `208575`，自动挂回因站上 hash（`d1eeec5c…`）与构造 hash（`c78e9cb2…`）不同被拒，
  改用挂回命令，qB 校验 100%。`audiences` 判重检索被人机验证拦住，且无头 HTTP 复用不了浏览器会话
  （cookie 里没有 `uid/pass` 类鉴权字段、直连被跳 `login.php`），该站只能人工发。

### Changed

- 发后定位优先使用 `takeupload.php` 回包/跳转里的详情 id，再下载站点生成的 `.torrent`；回包没有链接时按发布名 + 本地总大小做站内检索，结果行和详情页大小都要对得上，找不到就落 `posted_unresolved`，不猜。
- 挂回命令只接受站点详情页下载的目标种，移除 `--file` 本地构造种入口：站点审核没通过时，本地构造种挂进 qB 只会得到一条没有 tracker 响应的废种。
- 站内检索新增大小初筛：`/api/pieces-hash` 不支持或按名称检索时，同名不同大小的候选会被跳过；列表里解析不到大小的，仍用详情页大小 + pieces 兜底。

- 发布口径（2026-11-22）：目标站已有同名其他版本（分集/全集、不同压制组）时**照样发布**，
  画质高低交给站点规则裁决，工具不再自行比较 2160p/1080p（`assist.allowOtherVersion`）。
- 自动发种新增标题级内容黑名单 `assist.skipKeywords`：教育/成人等命中即跳过，不占上传预算。
  匹配规则：ASCII 关键词按整词命中（`Adult` 不误伤 `Adulthood`），中文关键词按子串；
  名称先归一小写、非字母数字折成空格再比。
- `existed` 判定收紧为结构性证据（`existed=1` 跳转 / `errorbox` 重复措辞），上传表单原样返回
  不再算重复；定位兜底去掉 IMDb 区，未验证的回包 id 不再写进覆盖表 `url`。
- `other_version` 从「待人工」改为明确的「不同内容，可另行发布」；总大小不同不再进人工队列。
- `bsend status` 新增单站视图（已有 / 缺口 / 覆盖率 / 草稿 / 已跳过）与「白名单站点暂无待发」提示；待找源清单按内容名去重。
- `bsend status` 在覆盖分布下单独显示「其中发后未解决 N」：上传可能成功但 qB 没挂回，提醒按详情页核对、勿重发。
- 自动发种白名单为空时的口径固定：真实发布不发任何站，避免漏填后全站开火；`--dry-run` 才用全部站点做发布检查预演。

- 自动发种审计区分 `posted / skipped_duplicate / other_version / existed_pending / failed`。
- `otherVersion` 纳入终态，不再每 7 天重复入队。
- 限流地板按轮次保留（一次成功不再回到 8 秒节奏）。
- `requireLinks` / `requireMediaInfo` 放宽同时作用于发布检查与预检。
- 同 `info hash` 的目标种不再一律记「同 hash 跳过」：qB 不允许同 hash 两条，
  遇到同 hash 先读原种 tracker 和 progress；只挂目标站 tracker 且 `progress=1` 就是已在该站做种，记 `seeding`，
  不改原种子、也不 add 第二条；挂别站、没有 tracker 或还没下完才记「同 hash 跳过」。
  `info hash` 是 `info` 字典的 SHA1，tracker 不在 `info` 里；pieces 指纹是对整串 `pieces` 再算的 SHA1，
  两者都不等于「站上已有」。hdfans 287874、ptcafe 137241、kufei 142202、sbpt 21764、xdy 8043 已按这条修正。

### Fixed

- 发后定位的大小初筛增加站点显示精度余量：页面写 `274.04 GiB` 时容忍 `0.01` 个显示单位（约 10.7 MiB），小体积按自己的单位缩放；仍只用于「名字 + 大小」挑候选，最终以 pieces 精确比对。
- 详情页大小解析纠偏（`4e3a8b1` 的遗留问题，`21e7589` 修复）：那一版把大小解析放宽成“任意位置的数字都试”，
  又保留了缺单位按 B 算的旧逻辑，结果把“上传于 2026.11.23”“做种 12”这类计数当成字节数，发后定位和源站检索
  会把正确候选以“大小不符”跳过；那一版 changelog 写的“已跳过‘大小：’前缀”也不成立（数字和标签粘成一个词，
  仍然解析失败），该句已撤回。现在只认“数字 + 显式单位”，详情页优先取以“大小 / Size”开头的单元格
  （标准 NexusPHP 把大小和类型、媒介挤在同一格），取不到再退回“整格只有一个带单位大小”的短单元格；
  MediaInfo 的 `Size : 7.99 GiB`、文件列表每行的体积、页眉的“上传量：1.35 TB”都不再被当成种子大小。
  用 7 张真实详情页（crabpt/qilin/hdclone/ubits/hdsky/pter/zmpt）按页面标注的大小做了断言，
  并要求全部已存详情页不得报出小于 1 MiB 的字节数。
- tjupt 剧集分类的三个动态字段补齐：剧集上传页的 `cname` / `ename` / `specificcat` 是
  `showcategorydetail.php?catid=402` 动态插进来的，静态 HTML 里没有；只提交通用字段会被
  `takeupload.php` 以“上传失败！”拒绝。现在中文名取副标题方括号里的名字，英文名用发布名，
  剧集类型按地区推断，推不出就填“其他”。
- ubits 的 `source_sel[4]`（地区）和 `team_sel[4]`（制作组）没有默认值，不补会被 `takeupload.php`
  以“一个或多个质量区域未选择”拒绝；现在按地区选 source，制作组固定填 `Other(5)`，不冒充驻站组。
- 详情页标题比对前先裁掉站点追加的角标（如 `[免费] 剩余时间：23时59分钟`）：这些后缀不是标题正文，
  不裁会把真重复判成“标题不符”。正常标题不会被误裁。
- 上面这条裁角标的实现原来按内置角标表的顺序裁，标题是 `X [2X] [Free]` 时会先裁 `[Free]`，留下 `X [2X]`
  仍带角标；现在裁在标题里最早出现的那个角标上，和表的顺序无关。
- 选目标站 announce 和校验 `overrides.announceURL` 的 host 判断，统一成“已在做种预筛”的口径（`host_matches`）：
  原来是“相等或以 `.host` 结尾”，站点地址带 `www.` 时（如 `www.pthome.net`）永远匹配不上 `t.pthome.net`
  这类 tracker，只能靠内置别名兜底，没别名时会报“不在 announceHosts 里，拒绝上传到错误 tracker”。
  现在 `www.` 不算站点身份、站点自己任意子域都算本站；别的站的子域仍然不算。
- qingwa 补 announce 别名 `tracker.qingwapt.org`：页面在 `qingwapt.com`、qB 里的 tracker 在子域，
  不补会把已在做种的青蛙种子当成缺口，重复走发种。
- CI：修复 `cargo fmt --check` 的格式失败，工作流统一跑 `cargo fmt --all -- --check`；打包流水线固定到 `macos-15` arm64 并加同 ref 并发取消，避免 macOS runner 排队 15 分钟导致整条 run 变红。
- 藏宝阁 43994 审核驳回修复：BDRip+x264 归 `Encode(3)`，标题无分辨率时按 MediaInfo 长边归档
  （1290x696 → `720p(3)`）；源站 HTML 简介提交前默认转 BBCode，避免站点把 `<fieldset>`/`<img>`
  转义成字面标签。已在线编辑 43994，未新发种；2026-11-23 复审又因图片全失效拒绝，
  豆瓣海报和图床截图改用 imgbox 直链后重新提交，等复审。
- 历史已发布种子简介批量转 BBCode：29 条（sbpt/luckpt/lajidui/ptcafe/tangpt/xdy 等）逐个编辑复查，
  只改简介、不换种子文件；`scripts/fix-bbcode-descr.py` 保留为可重复执行的维护脚本。
- GUI 概览页 clippy：`sort_by` 改 `sort_by_key(Reverse(...))`，图例查找去掉多余借用。

- 发后挂回不再把“站上已发、qB 没挂上”记成发种失败：下载目标种失败、hash/pieces 校验
  不过或挂回 qB 失败时，格子落 `review_pending`，note 前缀 `posted_unresolved` /
  `existed_unresolved`，不进失败冷却、不占发布额度。
- 挂回后 tracker 校验从“数量必须 == 1”改为“每条 tracker 的 host 都必须属于目标站”：
  站点自己的种子带多条同站 announce（如 hdvideo 的 `tracker.hdvideo.top` + `hdvideo.top`）
  原样保留，出现别站 host 才中止。
- 挂回前新增目标种校验：新发种下载回来的 `.torrent` 必须等于上传构造的新 hash；
 目标种 pieces 必须与本地源种一致，否则拒绝加入 qB。
- hdvideo 78856、panda 141276、qilin 479362 三条历史“站上已发、qB 未挂回”已补挂，
  qB 校验 100%、tracker 正常（hdvideo 站点种自带双 announce，panda/qilin 各 1 条）；
  qilin 站点审核仍显示“缺图”，不影响做种。
- 发后定位不再取回包里的第一条详情链接：详情和下载必须按 id 配对，最终跳转是详情页时优先用它；
  详情和下载 id 对不上就留空转 `posted_unresolved`，避免把成功页里的相关旧种（hdvideo 14469）挂进 qB。
  `userdetails.php?id=` 也不再被当成种子详情。
- 辅种新加种带限速/异常 note 时不再误落 `assisted` 捷径（会继续校验并启动）。
- 源种预算耗尽不再落 7 天 `miss` 负缓存。
- 审核渲染脱敏 `passkey/token/secure/uid` 等字段值。
- CookieCloud 错误信息不再包含访问 KEY。
- GUI 跑一轮/基线结论落盘（`persist=true`）。
- 跨进程运行锁（`bsend-state.lock`），防 GUI + CLI 并发突破日限额/覆盖记录。
- `autopost --dry-run` 在空白名单时逐条跑发布检查演练（此前直接全部跳过，与提示文案不符）；
  `--limit` 在演练下限制审阅条数（此前不生效，会跑完全部草稿）。
- 站内判重命中（同分片）/ 上传回包 existed 时，下载目标种并推入下载器做种
  （此前只记「已覆盖/待确认」，qB 里没有目标站 tracker）。
- qB 真值匹配补 qilin 的 announce 别名 `tracker.hdkyl.in`：页面在 `www.hdkyl.in`，
  tracker 在子域，之前 664 条已有 qilin tracker 的内容被误算成缺口。
- qB 真值再补 7 个 announce 别名（baozi/novahd/xingyunge/ptlgs/pthome/agsvpt/cspt），
  并让 `host_matches` 把 `www.` 与裸域视为同一站（tjupt）；实机 qB 覆盖率从大面积 0
  恢复到真实值，避免把已有站点误当缺口去发重种。
- 站点改写种子元信息的 hash 例外：ptba、cspt、dubhe、ptsbao、crabpt、hdbao 实机确认会被站点重写，
  目标种 hash 与上传构造不同时，只有 pieces、`info.name` 都一致且站点在白名单内才放行；
  其他站仍严格拒绝，避免把别的版本塞进 qB。
- oshen 这类没 `WEB-DL` 媒介选项的站：标题是 WEB-DL/WEBRip、站点只有 Encode 时，
  `medium_sel` 自动归 `Encode`，不再留空被发布检查拦。
- ptsbao 这类把 `source_sel` 当二级分类的站：先识别「大类-子类」选项，不再按 WEB-DL/Blu-ray
  片源词错填成「电影-WEB-DL」；type 选完后再按父子关系填，实测《知否》选到「剧集-大陆」。
- 新站首次实发暴露的 cspt tracker 别名 `tracker.cspt.cc` 已补；不补时目标种加进 qB 后
  会被“非目标站 tracker”校验中止，补挂失败。
- 其余未打通站：`itzmx` 早期大合集上传被 Kangle 以 `input filter` 403 拒绝，后续确认真正门槛是
  torrent 文件超过 `266,240 bytes`，已换小种打通（见上）；`ptfans` 上传页和 IMDb 判重搜索此前都超时，
  站点恢复后已一次通过，两者都已补挂成功。
- 上传错误文本提取的 UTF-8 切片 panic：错误文本含中文时，`body[p..p+120]` 会切在字符中间导致进程
  崩溃；改为 `floor_char_boundary` 并新增回归测试 `error_extraction_does_not_split_utf8`。
- GUI 双击启动：配置文件默认回退到 `%LOCALAPPDATA%\BoxSend\boxsend.json`；
  加载失败弹可见错误框（此前无控制台，双击后静默退出）。
- **自动发种改为「审阅产物直发」**（BUG-M7）：发布检查通过后直接把 `ReviewReport`
  交给 `run_reseed_reviewed`，提交字段与发布检查结果同一份；不再二次抓源页/上传页、
  不重复调官方预审，消除「审阅字段 ≠ 真正上传字段」的漂移。单发 CLI/GUI 仍走现场构造。
- 新增站点写接口级 e2e：审阅产物直发（提交体字段一致、源页/表单零重抓）与
  `takeupload` 拒收（保留 errorbox 原文、不推种、不改覆盖记录）。
- 上传提交地址防误配：action 只在上传表单块内取；命中搜索/奖励/登录/详情等非上传端点时
  中止提交，不再 POST 到错误接口（站外绝对提交地址照常放行）。
- 部分站点提交后不发跳转：回包正文带唯一新种详情页链接时识别为发布成功；表单重填页
  （multipart）或正文挂多个详情链接不算成功。

## [0.1.0] - 2026-11-22

首个 Rust 版可运行版本（引擎 + CLI + GUI）。能力与配置见 `rust/README.md`，
站点规则/审计结论见 `doc/`。
