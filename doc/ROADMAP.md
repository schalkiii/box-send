# BoxSend 还要做什么

这份文档只记“现在做到哪、下一步做什么、有什么风险”。已完成的历史流水不再展开。判断规则见 `ARCHITECTURE.md`，运行排障见 `OPERATIONS.md`。

## 1. 当前主线

把“逐个站点搜索判断有没有”改成“以 qB 的 tracker 为准”，并保证一个种子只挂一个站点。

要达成四件事：

1. 判断阶段不再请求站点，只读 qB；
2. 待发清单从 qB 的实时缺口生成，不再依赖旧的“没有”记录；
3. 上传前从 qB 导出源种子，改写 tracker 和目标站专属来源标记，让新 hash 唯一；
4. 发布后从目标站下载种子，独立挂回 qB，不和源种子混在一起。

## 2. 已经做完的

### 2.1 判断与报表

- 覆盖报表新增“qB 里有没有”的统计：`qbCovered`、`qbMissing`、`qbRatio`。
- `bsend status` 的单站表改成“已有 / 缺口 / 覆盖率”三列，直接看 qB 里的实际情况。
- `bsend report` 的主表也加了这三列，旧的“已判记录 / 判定覆盖 / 全库覆盖”保留作历史对比。
- 有单元测试覆盖：qB 里有 tracker 就算已有，子域名也算；没有记录也能算出覆盖率；全库为空不会出现除零或 NaN。
- 2026-11-23 补齐 7 个 announce 别名：`baozi`、`novahd`、`xingyunge`、`ptlgs`、`pthome`、`agsvpt`、`cspt`，并把 `www.` 与裸域归一化（`tjupt`）。qB 覆盖率不再把已做种的内容算成缺口；例如 agsvpt 从 0 → 757、baozi 0 → 569、novahd 0 → 601、xingyunge 0 → 566、pthome 0 → 566、ptlgs 0 → 584，cspt、tjupt 也补回真实值。

### 2.2 待发清单

- `autopost` 不再要求覆盖记录里先有 `reseed_draft` 条目。
- 每一轮读 qB 的实时种子列表，找出“白名单站点没有 tracker、源种子已定位、内容还在 qB、没有禁止转载”的组合。
- 同名内容对同一个站只生成一条；按内容体积从大到小排，每站按当天额度截断。
- 内容类型黑名单、白名单、暂停站、失败冷却（默认关闭）、日限额仍然在发布前检查。
- 已经有单元测试覆盖：qB 已有目标站 tracker 时不生成；源站不转自己；下载器里已有的站会跳过。

### 2.3 文档

- 原来 17 份分散文档合并成 5 份：`ARCHITECTURE.md`、`OPERATIONS.md`、`SITES.md`、`SITE-DEBUG.md`、`ROADMAP.md`。
- 文档改成白话叙述，先说“为什么”，再说“怎么做”，最后说“出问题看哪里”。
- 删除 `work/expand/`、`work-site-json.txt` 和空目录 `work/`。

### 2.4 桌面界面

- 概览表改成六列：站点 / 已有 / 缺口 / 覆盖率 / 草稿 / 已跳过；表头和数据行共用同一组列宽，窄窗口不再错位。
- 默认按覆盖率降序；最小窗口 1060×720。
- 改前/改后截图在 `doc/gui-review/compare-*.png`，设计口径在 `doc/gui-review/DESIGN-SPEC.md`。

## 3. 正在做的

### 3.1 单种子单站点上传

代码已经接入了大部分：

- 从 qB 导出源种子；
- 保留文件清单、分片、名称和分片长度；
- 把 tracker 换成目标站地址；
- 第一次向某站发种时，可以在该站配置 `overrides.announceURL` 里填完整 tracker 地址，解决“qB 里还没有该站种子、抄不到 passkey”的问题；填了配置优先，没填才从 qB 现有 tracker 里找；
- 写入目标站专属的 `info.source`；
- 计算新 hash，确认不在 qB 现有集合里；
- 上传后优先从回包正文、最终跳转或 `submit_upload` 已解析的详情 id 取目标种；回包没有链接时，按发布名 + 本地总大小站内检索，并核对详情页大小；
- 从目标站下载种子，只核对 pieces 指纹和文件清单（比较时忽略首段根目录名）；站点改写 hash/标题都放行，pieces 或文件清单对不上才拒绝；不拿本地构造种顶替；
- 站点侧已发布、但下载目标种或挂回 qB 失败时，记 `ReviewPending`、note 前缀 `posted_unresolved` / `existed_unresolved`，不记失败、不进失败冷却、不占发布额度；
- 手工挂回：只拉详情页、下载站点生成的 `.torrent`，按 pieces 找本地内容后走同一条挂回逻辑（命令 `bsend pull --site <id> --url <详情页>`，2026-11-24 由 `reattach` 改名）；已移除本地构造种的 `--file` 入口。

已通过：`coverage_e2e` 27 条全部通过；真实发种 luckpt、sbpt 各一条并挂回校验；hdvideo 78856、panda 141276、qilin 479362 三条历史种子已补挂，qB 校验 100%、tracker 正常。2026-11-23 又首次向 `ptba`、`cspt`、`dubhe`、`ptsbao`、`crabpt`、`hdbao`、`tey`、`oshen`、`ptfans`、`itzmx`、`railgun`、`lemonhd` 发种（hdbao 2 条，其余各 1 条），十二站都出现“站上 hash 与构造 hash 不同”，已按 pieces/标题校验补挂成功，并把十二站加入 hash 改写白名单；ptsbao 的 `source_sel` 二级分类也已按 type 父类 + 标题自动填对；hdbao 首次实发还暴露 `autopost --limit` 只数 Posted、不数 `posted_unresolved` 的问题，已修；itzmx 的 torrent 文件上限为 `266,240 bytes`，改用《Liar Game》小种后才打通。2026-11-23 发布检查新增 `titleNoCjk`、`requireImages`、`descrOrder`，并让门禁读最终提交的简介（剪死图/转 BBCode 之后），藏宝阁、熊猫高清已启用；2026-11-24 ubits、tey、麒麟因审核退回补开 `pruneDeadImages` + `requireImages=2`。

还没完成：

- 用更多真实站点跑完整链路，覆盖“站上已有 / existed / 下载被拒 / 校验失败”等分支；
- **三条“站上已发、qB 未挂回”已补齐**（2026-11-24，详见 `SITES.md` 2.3）：pthome `284701`、nanyang `226159`、ubits `379266` 三站加入 hash 改写白名单后，用 `bsend pull` 把站点种子下回来挂进 qB，qB 校验 100%、只挂单站 tracker、状态 `seeding`。站上 hash 与构造 hash 的不同点（具体改了哪个 info 字段）仍未逐字节核对。
- **hdfans `287791` 已核实**（2026-11-24）：就是《伪装者》同一条，qB 里 `b1e600a6…` 校验 100%，覆盖记录 `reseeded`，没有重发。
- **tjupt 已实发**（2026-11-25）：announce 地址定案 `tracker-public.tjupt.org`（https、带 passkey，已写进 `overrides.announceURL`）。CookieCloud 拉到的 `access_token`（2027-01-15 到期）恢复登录态，`gatecheck` 通过；真发《我们的父辈》25GB 成功，详情页 `https://www.tjupt.org/details.php?id=652794`，站上改写 hash/标题，按 pieces + 文件清单核对后挂回 qB，校验 100%，页面等审核。同批 274GB《波洛全季》合集上传 120 秒超时、放宽到 300 秒仍回 HTTP 200 空正文（站上无新条目），属于 tjupt 后端处理超时，重试仍会空回包，要发得先拆小或换内容。站上已有 `416154`（FLAC/DDP5.1）与 `236519`（同名 FRDS 合集，pieces 与本地不同），属实不同内容，不算重复。
- **audiences**：判重检索被人机验证拦住，程序按“未确认重复不上传”中止。浏览器里过了验证也救不了无头请求：cookie 里没有 `uid/pass` 类鉴权字段，直连会被跳 `login.php`。2026-11-24 晚 Kimi WebBridge 可行性实验通过：驱动本机真实浏览器，audiences 已登录 `Schalkiii`；页面内 `fetch('/torrents.php?search=…')` 返回 200、命中 2 个 `details.php?id=`、无人机验证。代码已接入程序。2026-11-25 实测卡点：Kimi 能选中已打开的 audiences 标签页，但浏览器把页面判为 `document.visibilityState=hidden`，Turnstile 不生成 iframe、token 一直为空；显式 `turnstile.render` 也没有 token。需要把该标签页切到前台让页面可见后重试，程序会明确报“标签页在后台”，不把验证页当站内没有。
- **qingwa 的 Poirot 合集**：不是“回包未识别”，回包是 `torrent_too_large` 表单（自动 submit 到 `offers.php?add_offer=1`），站点要求改走候选入口；程序记 `pending_offers`（待走候选入口），不算失败、不自动重试，`bsend status` 单列“待跟进”，等人去候选页提交（2026-11-24 已真跑复现，覆盖记录与审计都落这个新状态）。
- 站点审核的结论已由回访落进覆盖记录（2026-11-24 晚，`bsend followup --all` 限定五站实跑，14 条详情页全部打开、0 条打不开）：藏宝阁 43994 详情页审核图标已标“通过”，审核已过，格子仍是 `reseeded`，不用动；tey 623 仍标 `title="未审"` → `pending_review`；麒麟 479362 横幅仍写“审核不通过，原因：缺少海报或截图（图片少于 2 张）” → `review_rejected`；熊猫 141276 详情页报“没有该ID的种子” → `gone_from_site`，重提的那版没留在站上，qB 里那条还挂着 tracker。之后不用人工盯：自动发种每轮开头会回访（已用 `autopost --limit 0` 实跑验证，看了 3 条、没发任何东西），审完自动挂回。
- **图片类的审核退回已处理（2026-11-24 深夜）**：ubits 379266 站内信判“海报与截图均无法访问”，简介 9 张图全是已失效的 `i13.tietuku.com`；同一条内容的 tey 623 带着同样的死图、只是还在“未审”；麒麟 479362 判“缺少海报或截图”，实际 7 张图全不可达（豆瓣海报防盗链 418 + `img.pterclub.com` TLS 失败）。三条都已换成 imgbox 新图并提交（ubits 1 海报 + 8 截图、tey 同一条 9 张、麒麟 1 海报 + 6 截图），详情页复查旧图站点 0 处、imgbox 张数对上；ubits/麒麟页面仍挂着“不通过”，站点没有用户侧重提入口，等管理复审，程序不重发、不私信。覆盖记录对应用 `scripts/set-cell-note.py` 写明了“已换图、等复审、别重发”，回访继续盯。
- 死图重传现在是半自动流水：`scripts/fetch-descr.py`（取原简介）→ 本地备图（海报带豆瓣 Referer、截图用 cv2 从正片抓）→ `scripts/imgbox-upload.py`（传图拿直链）→ `scripts/swap-descr-images.py`（按出现顺序 1:1 换链接，BBCode/HTML 都支持）→ `scripts/edit-seed.py`（演练后提交）；发布前门禁会先剪死图/拦残图。尚未接成发布前自动重传（要能自动找海报才行）。
- **图门禁改成代码硬规则（2026-11-24 晚）**：剪死图不再读配置，所有站点上传前无条件逐图探测、剪掉死图；要图的站有硬性最低图数（`gates::image_floor`：藏宝阁/优堡/太乙/麒麟 3 张，熊猫 4 张，2026-11-25 新增 ptlgs 4 张 = 1 张海报 + 3 张截图），配置里写 `requireImages` 也会被覆盖。真实配置里的 `pruneDeadImages` / `requireImages` 键已删。单测改成验证硬编码与配置不可覆盖。
- **熊猫 141276 查清（2026-11-24）**：《星球大战9：天行者崛起》UHD BluRay 1080p 单文件 11.56G（https://pandapt.net/details.php?id=141276 ）。站内信两条：第一次被 NoCaps 判主标题含中文、简介顺序不合规；重提的那版被系统当“候选种子”，24 小时内没做种自动删除——不是审核不过被删。qB 里那条还挂着（tracker 报 `torrent not registered`）。2026-11-24 晚曾暂停熊猫发种；**2026-11-27 已闭环跑通并恢复**：141840 真发一条、挂回 qB 校验 100%，见 7.9。
- **上传后立刻挂回改成硬步骤（2026-11-24 晚）**：上传成功后立即下载站点 `.torrent` / 核对分片 / 挂进 qB，站点刚建种时常见的 `torrent not registered` 固定重试 4 轮、每轮 15 秒，仍失败才落待人工；这条没有配置开关，适用于所有站点。

### 3.2 文档里的站点数据

`SITES.md` 的 147 站表来自配置和上一次表单检查，不是实时状态。站点改版、关站、权限变化后要重新生成，不要拿旧表当现在能不能发的依据。

## 4. 还没做的

### 4.1 发布前再确认一次（已完成 2026-11-25）

真要发某一条之前，程序的 `reseed.rs` 第 2.5 步会先查一次目标站：

- 只对“这一轮真要发”的条目查；
- 每条最多查一次；
- 查到站上已有，不重复上传，改为下载目标种子挂回 qB；
- 搜索本身失败（网络、登录、限流）就中止这条，按失败冷却设置决定要不要下一轮再试，不盲发。

`e2e_duplicate_hit_pushes_target_torrent_to_downloader` 已经覆盖“命中后下载目标种、pieces 比对、挂回 qB”这条路径。2026-11-25 真实命中已出现：sbpt 七条经 `existed` 路径处理，其中 21764 先被旧同 hash 判断误记、后按 qB progress/tracker 复核为做种，其余下载目标种并校验 pieces 后挂回；详情页作者都是本站账号 `Schalkiiii`，不是别人发的可辅种（见 `SITES.md` 2.5.1）。这条“真实命中样本待跑”可以勾掉。

### 4.2 上传额度按结果分开算（已完成 2026-11-23）

原问题：旧口径下“站上已有”“重复”“失败”也可能消耗当轮上传次数，热门站查 10 次全是重复，一条也发不出去。

现在 `autopost` 在 `posted` 和 `posted_verifying` 两种结果上都会 `posted_today += 1`；`blocked`、`duplicate`、`failed`、`pending_offers` 等不计入 `posted`。`parse_audit` 也按这两种结果统计，日限额只算真正发出上传请求的条目。

注意：这里的“额度”是**发布额度**，不是上传请求总预算；全局 `roundBudget/uploadBudget` 仍会拦截所有真正的上传动作，两者不冲突。

### 4.3 判重和发后定位

- 上传回包里的详情页 id 不可信，已经出过“站上已发、本地判失败”。
- 发后定位缺陷已修（2026-11-23）：`extract_posted_links` 不再取第一条链接，而是按 id 配对详情/下载；最终跳转是详情页时优先用它；多个链接对不上时两个都不用，转 `posted_unresolved`；`userdetails.php` 不再被当成种子详情。无链接时再按名称 + 总大小检索，避免同名不同版本抓错。
- 挂回前校验已补（2026-11-23）：新发种下载回来的 `.torrent` 默认必须与上传构造的新 hash 相同，pieces 必须与本地源种一致，否则拒绝挂回；下载/挂回失败不再记失败，落 `posted_unresolved`，判重路径落 `existed_unresolved`。
- 站点改写种子元信息的例外（2026-11-23）：ptba、cspt、dubhe、ptsbao、crabpt、hdbao、tey、oshen、ptfans、itzmx、railgun、lemonhd 实机确认站上 hash 与构造 hash 不同，pieces 和 `info.name` 一致；当时按白名单放行。2026-11-24 又加入 pthome、nanyang、ubits。**现行放行条件已改成 pieces + 文件清单一致，不再要求 hash 或标题相等，也不再看白名单**；白名单只用于日志标注“已实测改写”，补挂命令不设期望 hash。
- announce 别名已补（2026-11-23）：baozi、novahd、xingyunge、ptlgs、pthome、agsvpt、cspt、tey 加了站点别名，`www.` 与裸域也归一化；cspt 的 `tracker.cspt.cc`、tey 的 `tracker.tey.cc` 都是“页面域和 tracker 域后缀不同”的例子，tey 不补别名会被误判成 qB 覆盖 0。2026-11-24 又补了 qingwa 的 `tracker.qingwapt.org`。
- 详情页大小解析纠偏（2026-11-24）：`4e3a8b1` 为了补 0.01 显示余量，把大小解析放宽成“任意位置的数字都试”，又留着缺单位按 B 算的旧逻辑，结果“上传于 2026.11.23”“做种 12”这类计数被当成字节数；标准 NexusPHP 的大小又和类型、媒介挤在同一格（超过 40 字）根本取不到。两条路都指向同一个后果：发后定位、源站检索把正确候选判成“大小不符”跳过。现在只认“数字 + 显式单位”，优先取以“大小 / Size”开头的单元格，再退回“整格只有一个带单位大小”的短单元格，MediaInfo、文件列表、页眉账户统计都跳过；取不到就不参与大小比对。用 `Tests/BoxSendTests/Fixtures/` 里 7 张真实详情页做了断言。
- 详情页标题比对前先裁掉站点角标（2026-11-24）：`[免费] 剩余时间：23时59分钟` 这类后缀不是标题正文，不裁会把真重复判成“标题不符”。同日又修了裁的位置：原来按内置角标表的顺序裁，`X [2X] [Free]` 会留下 `X [2X]`；现在裁在标题里最早出现的那个角标上。
- 选 announce 与校验 `overrides.announceURL` 的 host 口径统一（2026-11-24）：这两处原来写的是“相等或以 `.host` 结尾”，和“已在做种预筛”的 `host_matches` 不是一套；站点地址带 `www.` 时（如 `www.pthome.net`）会把自己的 `t.pthome.net` 当成别站，没内置别名就报“不在 announceHosts 里”。现在三处都走 `host_matches`。
- 新站字段补齐（2026-11-24）：tjupt 剧集分类的 `cname/ename/specificcat`（上传页静态 HTML 里没有，是分类页动态插入的）、ubits 的 `source_sel[4]`（地区）/`team_sel[4]`（`Other`）都能自动填；不补会被 `takeupload.php` 直接拒绝。
- 站点判定“种子过大”有了明确归宿（2026-11-24）：青蛙（qingwa）的 Poirot 合集回包不是未知页面，而是 `torrent_too_large` 表单自动 submit 到 `offers.php?add_offer=1`，即站点要求改走候选入口。`extract_upload_error` 现在识别这类回包并给出中文原因，`submit_upload` 命中时把覆盖记录落 `pending_offers`（待走候选入口）而不是记失败或当已跳过：不入队重试，但 `status` / `audit` 单列出来给人跟进；候选表单不代填。
- 上传步骤单独放宽超时（2026-11-24）：所有站点共用的 30s 超时对 `takeupload.php` 不够（北洋园连挂两次），现在只给上传这一步 120s，列表/检索仍保持 30s，避免把整站超时整体拉长。
- “等站点侧动作”不再当已跳过（2026-11-24）：新增覆盖状态 `pending_offers`（待走候选入口）与审计词 `pending_offers`；`bsend status` 单列“待跟进（等站点侧动作，程序不自动重试）”，`bsend audit` 合计带“待走候选”数量。这类记录不入队重试、也不算失败。
- 发种失败冷却默认关闭（2026-11-24）：`CoverageSettings.backoff_hours` 默认 0，可由 `assist.backoffHours` 显式打开；拦截与失败共用一个窗口，日志与 GUI 诊断页同步改成“冷却”口径。默认关闭的理由：手动跑一轮由人决定重试，机器不该替人压着。
- 文档口径统一（2026-11-24）：正文不再把命令名当动词/术语，先讲做什么再给命令；AGENTS.md 补了这条规则。同日把 `bsend reattach` 改名为 `bsend pull`，旧名留作隐藏别名。
- 手工挂回入口：`bsend pull --site <id> --url <详情页>`，可用 `--key` 指定内容；不再接受本地构造的 `.torrent`，避免挂上站点没通过的废种。
- “等站点审核”做成机器可跟踪（2026-11-24）：新增覆盖状态 `pending_review`（等站点审核）、`review_rejected`（站点审核没过）、`gone_from_site`（站上已找不到）；发种时站点不给下载且提示是审核没放行、或页面标着未审 / 不通过，就落对应格子，不做站内检索兜底、不记失败、不重发。新命令 `bsend followup` 回访详情页，自动发种每轮开头也跑一次：审完了自动把站点种子下回来挂进 qB，没过就记原因，站上没了就记下来。页面标记按真实页面样本写（tey `title="未审"`、麒麟“审核不通过，原因：缺少海报或截图”、藏宝阁 `title="通过"`、熊猫“没有该ID的种子”），五张真实页面离线核对全部识别正确；判断顺序改成“明写的压过含糊的”，导航项“审核中心”不再被当成“审核中”。`bsend status` 的待跟进按类逐条点名，`bsend audit` 带“待跟进”一行，GUI 图例补齐。
- hdvideo 78856、qilin 479362、panda 141276 三条历史结果已补挂完成，qB 校验 100%；qilin 站点仍显示审核不通过，2026-11-24 已把简介里 7 张不可达图换成 imgbox 图并提交，等复审（不影响做种）。

### 4.4 旧“没有”记录不再新增

新流程已经不依赖这些记录，但旧扫描路径还会写 `miss`。要逐步停掉：

- 判断阶段不请求站点；
- 不再为“内容 × 站点”写“没有”记录；
- 旧记录保留作负缓存，不主动清空。

### 4.5 拆分 `reseed.rs`

`reseed.rs` 已超过 9,000 行，改一处容易碰坏别处。计划拆成：

```
reseed/mod.rs          共享类型和入口
reseed/flow.rs         主流程编排
reseed/judge.rs        判重和决策
reseed/upload.rs       表单、上传、发后处理
reseed/coverage_io.rs  覆盖记录读写
```

只移动代码，不改行为。每拆一批都要编译、跑测试，确认对外接口不变。

## 5. 已知问题

### 5.1 站点适配

- **ptlgs**：上传页把海报、截图放在独立字段 `cover` / `screenshots`，页面写明“至少需要 3 张原始截图”。2026-11-25 查看已发布 `115117`、`115313`，详情页 `ktorrentcover` 是 `<img src=''/>`、简介 0 张图，说明旧版 Rust 没填独立字段；已补 `posterField` / `screenshotField`，从最终简介按顺序抽图填字段，并把 ptlgs 最低总图数设为 4 张（1 海报 + 3 截图）。2026-11-25 用《苦月亮》真发一条 `115350`（https://ptlgs.org/details.php?id=115350 ）验证：详情页 `ktorrentcover` 有海报、`ktorrentscreenshots` 有 3 张截图，qB 校验 100%，站点标“未审”；这条是有图源的真实样本，不是只跑单测。
- **sbpt**：2026-11-25 核实 21764、21767、21768、21780、21782、21783、21784 七条详情页作者都是本站账号 `Schalkiiii`。`existed=1` 只证明同 `info hash` 判重，不能说明是谁先发；21764 曾被旧同 hash 判断误记，后按 qB tracker/progress 复核为做种，其余已下载目标种、校验 pieces 后挂回。Aquarela、The Intern 仍缺“中字”标签，需人工补。
- **hdvideo**：必填项已补（`region_sel[4]=2`、`style_sel[4][]=6`），站上已发布 78856；发后定位取错旧种 14469（缺陷已修）。2026-11-23 已手工补挂 78856，qB hash `dd7f6097…` 校验 100%。
- **luckpt**：截图和 BDRip 选档还没完全过。豆瓣图床防盗链补 `Referer` 已解决；`img.pterclub.com` 在本机 TLS 连不上，截图数量仍不足。BDRip 的媒介档已确认是 `Encode(7)`。
- **qingwa**：未覆盖的是成人合集和多片合集，属于内容黑名单范围，不是适配器故障。
- **haitang**：只有曲艺、有声书类目，和影视范围不匹配，已移出白名单。
- **haoxue**：Next.js/Flux 单页应用，静态上传页拿不到，需要按前端接口重做，已移出白名单。2026-11-27 走站点接口真发一条 23943（待审，见 7.9）；长期维护浏览器流程还是每次人工，待决定。
- **agsvpt**：2026-11-23 当时提交域和登录域不一致，返回“你没有该权限”；2026-11-26 复核上传页表单完整、无权限拒绝字样，2026-11-27 已启用并进白名单（`team_sel` 兜底 Other=22）；546963 仍等复审。
- **ptba / cspt / dubhe / ptsbao / crabpt / hdbao / tey / oshen / ptfans / itzmx / railgun / lemonhd**：2026-11-23 首次实发（hdbao 2 条，其余各 1 条）并挂回成功；十二站都会改写种子元信息，站上 hash 与构造 hash 不同，已加入 hash 改写白名单。cspt 另有 `tracker.cspt.cc` 别名，tey 另有 `tracker.tey.cc` 别名；dubhe 判重搜索偶发 30 秒超时；ptsbao 的 `source_sel` 是二级分类，已按 type 父类 + 标题自动填；crabpt 此前候选源站 `hdfans` 502，被失败冷却挡住，改走 `hdsky` 源后成功；hdbao 的 `autopost --limit` 计数缺陷已修；tey 表单没有 Blu-ray 媒介档，蓝光/Remux 源会被门禁拦，HDTV/WEB-DL 可发；oshen 前两次站点抖动，第三次才恢复；ptfans 此前上传页/判重超时，恢复后一次通过；railgun、lemonhd 上传回包都没给详情链接，按详情页补挂成功。
- **itzmx**：站点 torrent 文件上限 `266,240 bytes`，大合集/大剧包会被拒；改用 `ubits` 源的《Liar Game》小种（源种 96,934 bytes）首次实发 `13134` 并挂回，qB 校验 100%。后续给该站选种先看 `.torrent` 文件大小，不要只看内容体积；详情页没有“完结/中字”标签，发布后人工补。
- **pthome / nanyang / ubits**：2026-11-24 首次上传成功（`284701` / `226159` / `379266`），站上 hash 与构造的不同，三站已加入 hash 改写白名单，把站点种子下回来挂进 qB，校验 100%。ubits 的 `source_sel[4]`、`team_sel[4]` 不补会被拒，现已自动填。
- **hdfans**：`287791` 已核实是《伪装者》同一条，qB `b1e600a6…` 校验 100%，覆盖记录 `reseeded`，未重发。
- **tjupt**：动态字段已能填；announce 定案 `tracker-public.tjupt.org`（写进 `overrides.announceURL`）；CookieCloud 拉到的 `access_token` 已恢复登录态（2027-01-15 到期），`gatecheck` 通过。2026-11-25 真发 25GB《我们的父辈》`652794` 成功并挂回 qB（校验 100%，等审核）；274GB《波洛全季》合集 5 分钟仍空回包、站上无条目，重试仍会空回包，要发得先拆小或换内容。
- **audiences**：判重检索被人机验证拦住，且无头 HTTP 复用不了浏览器会话；Kimi WebBridge 复用浏览器登录态已接入程序，但 2026-11-25 实测浏览器把标签页判为 hidden，Turnstile 不渲染 iframe、token 为空。需把标签页切到前台再试，不盲发。
- **qingwa**：announce 别名已补；《知否》回包“已存在！”且 qB 已在做种；Poirot 合集被站点判 `torrent_too_large`（要求走 `offers.php` 候选），记 `pending_offers`（待走候选入口），不自动重试、`status` 单列待跟进；《伪装者》发 `208575` 并挂回，校验 100%。

### 5.2 工程

- 配置测试的临时目录按进程号共用，偶尔互相干扰。
- 部分临时目录只用进程号做前缀，同一进程并发跑测试可能撞名。
- 界面的自动化测试还很少。
- 旧版 `startup.log` 已没有程序写，可以删。

## 6. 风险

- **qB 有 tracker 不代表站上一定还在**：站点删种、账号被禁都会出现。发布前再确认一次是兜底。
- **qB 没 tracker 不代表一定没发过**：站上可能有，只是本地没挂。确认命中后要转人工核对，不能当失败。
- **改写种子可能泄露 passkey**：tracker 只能取目标站配置里的地址，日志要脱敏。
- **旧“没有”记录可能误导**：新流程不读它们；迁移期不要拿旧覆盖率高低下结论。
- **两个 Agent 同时改 `reseed.rs` 会互相覆盖**：单种子单站点改造结束前，其他工作不要碰这个文件。

## 7. 全部站点全自动差距评估（2026-11-26 实测）

### 7.1 口径与证据来源

- 证据：`bsend-batch-2026-11-26.md`（74 次真实上传尝试、覆盖 37 站）、当天 `auto-audit.jsonl` 与 `reseed-log.jsonl`、`boxsend.json` 的白名单/暂停名单、`SITES.md` 历史记录。本轮没有再跑会写覆盖记录的命令，结论只对上述只读材料负责。
- **"能全自动"必须有完整链路证据**：上传成功 → 从站点把种子下载回来 → 按 pieces 指纹和文件清单核对 → 挂进 qB 校验 100%。页面还标"未审"的算 `pending_review`，由 `bsend followup` 机器回访收尾，不算人工兜底。
- 没有本轮或历史完整链路证据的，一律标"未验证"，不按"代码看起来支持"推断。
- 配置现状（2026-11-27）：白名单 `autoReseedSites` 45 站；`sourceSites` 里启用扫描 101 站；其余是源站、暂停站、无权限站、动态表单站、连不通站、类目不符站、关站站和还没接适配器的外文站。

### 7.2 白名单 45 站逐站矩阵

> 下面这张表是 2026-11-26 批次的逐站结论；2026-11-27 新进白名单的 panda、iloli、muxuege、fenghuang 见 7.9，agsvpt 见 7.8。

判定分四档：**能全自动**（链路已打通）、**需补适配**（代码/配置缺口，修了能发）、**站点侧限制**（程序改不了，只能半自动或人工兜底）、**未验证**（没有完整链路证据）。

| 站点 | 本轮实际结果 | 判定 | 判定依据（可核对的证据） |
| --- | --- | --- | --- |
| baozi | 2 条已提交、已挂回，等审核 | 能全自动 | 83868 https://p.t-baozi.cc/details.php?id=83868 ；83869 https://p.t-baozi.cc/details.php?id=83869 ；均 `pending_review`，回访收尾 |
| cangbao | 1 条已挂回等审核；1 条已有条目复核 404 | 能全自动 | 44240 https://cangbao.ge/details.php?id=44240 已挂回；1674 下载种子 HTTP 404，未判其他版本 https://cangbao.ge/details.php?id=1674 |
| dragonhd | 1 条已做种；1 条判重检索超时 | 能全自动（检索稳定性待补） | 49038 https://www.dragonhd.xyz/details.php?id=49038 已做种；War 检索 `torrents.php?search=…` 30 秒超时后中止，未盲发 |
| hdtime | 2 条已做种 | 能全自动 | 239557 https://hdtime.org/details.php?id=239557 ；239558 https://hdtime.org/details.php?id=239558 |
| hdvideo | 1 条已挂回等审核；1 条 `existed=1` 线索未核实；1 条被 G2 拦 | 能全自动 | 79086 https://hdvideo.top/details.php?id=79086 已挂回；74646 线索 https://hdvideo.top/details.php?id=74646&existed=1 未能定位核对 |
| kufei | 2 条已做种；1 条被 G2 拦 | 能全自动 | 142311 https://kufei.org/details.php?id=142311 ；142312 https://kufei.org/details.php?id=142312 ；被拦条目的 `medium_sel` 选档待补 |
| lemonhd | 2 条已做种 | 能全自动 | 49207 https://lemonhd.net/details.php?id=49207 ；49208 https://lemonhd.net/details.php?id=49208 |
| ptcafe | 2 条已做种 | 能全自动 | 137339 https://ptcafe.club/details.php?id=137339 ；137340 https://ptcafe.club/details.php?id=137340 |
| ptlgs | 1 条已挂回等审核 | 能全自动 | 115362 https://ptlgs.org/details.php?id=115362 ；历史 115350 已验证独立 `cover`/`screenshots` 字段填得对 |
| ptsbao | 1 条已做种；1 条被 G2 拦 | 能全自动 | 135848 https://ptsbao.club/details.php?id=135848 ；被拦条目 `medium_sel` 选档待补 |
| pthome | 1 条已有并做种；1 条"已存在！"被记成失败 | 能全自动（回包识别待补） | 272549 https://www.pthome.net/details.php?id=272549 已自己做种；War 回包"已存在！"目前落 `failed`，应判重复 |
| qingwa | 1 条已挂回，后续审核通过 | 能全自动（大合集走候选入口属站点侧限制） | 208804 https://qingwapt.com/details.php?id=208804 ；Poirot 合集 `torrent_too_large`，记 `pending_offers`，候选表单不代填 |
| sbpt | 1 条命中已有 | 能全自动 | 21768 https://sbpt.link/details.php?id=21768 ；历史七条详情页作者均为本站账号，已做种或校验 100%（见 `SITES.md` 2.5.1） |
| shuioudao | 2 条已挂回等审核 | 能全自动 | 72641 https://sewerpt.com/details.php?id=72641 ；72642 https://sewerpt.com/details.php?id=72642 |
| tangpt | 2 条已挂回等审核 | 能全自动 | 74000 https://www.tangpt.top/details.php?id=74000 ；74001 https://www.tangpt.top/details.php?id=74001 |
| xdy | 2 条已做种 | 能全自动 | 8073 https://xdypt.vip/details.php?id=8073 ；8074 https://xdypt.vip/details.php?id=8074 |
| crabpt | 2 条已挂回等审核 | 能全自动 | 241256 https://crabpt.vip/details.php?id=241256 ；241257 https://crabpt.vip/details.php?id=241257 |
| cyanbug | 2 条已挂回等审核；首轮 TLS 断连后重试成功 | 能全自动（偶发连接抖动） | 105724 https://cyanbug.net/details.php?id=105724 ；105725 https://cyanbug.net/details.php?id=105725 |
| dubhe | 2 条已做种 | 能全自动 | 55806 https://dubhe.site/details.php?id=55806 ；55807 https://dubhe.site/details.php?id=55807 |
| hdbao | 1 条已做种；1 条命中已有 | 能全自动 | 54876 https://hdbao.cc/details.php?id=54876 ；54657 https://hdbao.cc/details.php?id=54657&existed=1 |
| nanyang | 2 条已做种 | 能全自动 | 226318 https://nanyangpt.com/details.php?id=226318 ；226319 https://nanyangpt.com/details.php?id=226319 |
| novahd | 2 条已做种 | 能全自动 | 227797 https://pt.novahd.top/details.php?id=227797 ；227798 https://pt.novahd.top/details.php?id=227798 |
| oshen | 2 条已做种 | 能全自动 | 57273 http://www.oshen.win/details.php?id=57273 ；57274 http://www.oshen.win/details.php?id=57274 |
| railgun | 2 条已做种 | 能全自动 | 60808 https://bilibili.download/details.php?id=60808 ；60809 https://bilibili.download/details.php?id=60809 |
| ubits | 1 条已做种 | 能全自动 | 379934 https://ubits.club/details.php?id=379934 ；图床黑名单（imgbox 拒、必须 pixhost）规则已落地 |
| xingyunge | 2 条已挂回等审核 | 能全自动 | 192758 https://pt.xingyungept.org/details.php?id=192758 ；192759 https://pt.xingyungept.org/details.php?id=192759 |
| ptba | 本轮无缺口条目，没有可发内容 | 能全自动（历史证据） | 历史 106718 https://1ptba.com/details.php?id=106718 已发布并挂回、校验 100%；本轮源站本身，缺口已被覆盖 |
| audiences | 2 次上传失败，通用"上传失败！" | 需补适配（叠加站点侧人工依赖） | 上传页是自定义 JS 表单，`type` 以隐藏值 `0` 提交，程序没有对应选档逻辑；判重检索还依赖真实浏览器前台渲染 Turnstile，后台标签页 token 为空（`kimi.rs` 已报"标签页在后台"） |
| longpt | 4 条中止：表单 action 落到 `bonusrain.php` | 需补适配 | 审计 error："上传表单 action 不是上传处理页：https://longpt.org/bonusrain.php（命中 bonusrain.php），已中止提交"；历史 51065 曾发成，入口识别现在仍会走错 |
| ptfans | 本轮未发：分类归到 H 漫区；源站详情页超时 | 需补适配 | Coraline 被自动归到"H漫(动漫、漫画)"区，分类映射错误；另一条源站 `carpt.net/details.php?id=198553` 请求超时 |
| ptzone | 1 条被 G2 拦（`audiocodec_sel` 未选）；1 条详情页复核超时 | 需补适配 | 112597 https://ptzone.xyz/details.php?id=112597 复核请求超时；历史 190916 五项检查全过、发成过 |
| city13 | 2 条 HTTP 200 未识别失败；2 条被 G2 拦 | 需补适配 | 回包既不是详情页也不是错误框，程序认不出；历史 88256 https://13city.org/details.php?id=88256 是靠站内搜索人工兜底确认已发 |
| qilin | 1 条已发布但挂回失败；1 条同分片跳过 | 需补适配 | 480763 https://www.hdkyl.in/details.php?id=480763 站点种子带 `hdkylin.top` 非目标 tracker，按单站策略拒绝补挂，落 `posted_unresolved`；418978 https://www.hdkyl.in/details.php?id=418978 同分片跳过 |
| lajidui | 2 条已挂回等审核，但属安全集外误发 | 需补适配（候选范围失守） | 85084 https://pt.lajidui.top/details.php?id=85084 （人民的名义）、85085 https://pt.lajidui.top/details.php?id=85085 （伪装者）；批次备注：不在安全集内，未做人工核对 |
| luckpt | 1 条命中已有；2 条官方预审未过被拦 | 站点侧限制（半自动） | 61704 https://pt.luckpt.de/details.php?id=61704 ；G5 LuckAudit 官方预审未清零不发，程序代过不了预审，只能等条目自身过审 |
| cspt | 4 条中止：账号无发布权限 | 站点侧限制（权限缺失） | 审计 error："该账号无发布权限（页面提示含「没有发布种子的权限」）：https://cspt.top/upload.php" |
| itzmx | 2 次失败：通用上传失败；HTTP 403 | 站点侧限制（文件大小上限，403 待查） | 历史 13134 https://pt.itzmx.com/details.php?id=13134 小种发成；站点 `.torrent` 上限 266,240 bytes，大合集普遍被拒；本轮 Coraline 回 HTTP 403 是新现象 |
| tey | 1 条已挂回等审核；3 条被 G2 拦（蓝光/Remux） | 站点侧限制（HDTV/WEB-DL 可自动，蓝光不行） | 624 https://pt.tey.cc/details.php?id=624 已挂回；`medium_sel` 只有 WEB-DL/HDTV/DVDR/CD/Track，没有 Blu-ray，不能硬填 HDTV |
| tjupt | 2 次失败：大种子上传空回包 | 站点侧限制（体积门槛） | 历史 652794 https://www.tjupt.org/details.php?id=652794 25GB 单条发成并挂回；274GB 合集上传 5 分钟回 HTTP 200 空正文、站上无条目，重试仍空回包 |
| hdfans | 1 条被 G2 拦（`medium_sel` 未选） | 未验证 | 本轮没有新上传成功证据；287791、287874 是原种挂目标站 tracker、progress=1，不算程序发布；选档缺口待补后再验证 |

**矩阵小结（2026-11-26 批次口径）**：当时白名单 40 站里，26 站本轮链路打通、1 站有历史完整链路证据（ptba）；7 站需补适配；5 站受站点侧限制；1 站未验证。2026-11-27 新增四站结论见 7.9。

### 7.3 白名单之外的站点（107 站，按没启用的原因分组）

| 分组 | 站点 | 为什么没启用 / 能不能自动 |
| --- | --- | --- |
| 账号没有发布权限（站点侧，程序改不了） | btschool、carpt、chdbits、cmct、daxiangjiao、discfan、dstudio、ggpt、hdarea、hdhome、hdsky、hhanclub、hitpt、kamept、keepfrds、march、njtupt、okpt、playlet、pt52、pter、ptskit、siqi、tccf、tokyo、ultrahd、wtsakura、xingwan、zmpt | 上传页明示"没有发布种子的权限"；只有账号升级权限后才谈得上适配 |
| 权限待实证（旧"无权限"结论可能过时） | ~~agsvpt~~ | 2026-11-26 GET `upload.php` 返回完整上传表单、没有权限拒绝字样；2026-11-27 已启用并进白名单，见 7.8 |
| 动态/JS 上传界面，当前程序不支持 | hddolby、mypt、ptt、rousi、ttg、haoxue、hdcity | 拿不到可解析的静态表单（自定义 JS、SPA、隐藏字段）；haoxue 是 Next.js/Flux 单页应用，要按前端接口重做。iloli、muxuege、fenghuang 2026-11-27 已落地并实发，见 7.9；aling 源站 500、hdu 全站过载、xingtan 类目不符，见下两组 |
| 连不通 / 超时 / 403（网络或登录态） | freefarm、hdclone、hudbt、u2、shadow、sunny、piggo、tu88 | 域名、TLS、连接超时或跳登录页；整站暂停，不写"站上没有"的记录。gtk 已配按站代理、链路可达，尚未排队，见 7.9 |
| 类目不符（不转影视） | haitang、kelu、vclib、nicept、ziran、soulvoice、musopia、dicmusic、xingtan | 只有曲艺、有声书、音乐、电子书类目；xingtan 是学术教材方向，影视无处归档 |
| 已关站或换形态 | momentpt、ptlao、movie52、yinghua | 告别页、404 或域名解析失败 |
| 其他站点程序，适配器未接入、未验证 | aither、alpharatio、animez、anthelion、azusa、beyondhd、bitporn、blutopia、brokenstones、byr、clearjav、empornium、exoticaz、fappaizuri、filelist、generationfree、gpw、haidan、happyfappy、hdspace、hdtorrents、huno、iptorrents、jpopsuki、lst、milkie、monika、mteam、myanonamouse、nebulance、oldtoons、opencd、orpheus、ourbits、ptneko、sjtu、sportscult、tlf、torrentleech、yemapt、yzyy、zhuque | Gazelle / Unit3D / Blu / XBTIT / custom / Discuz / TNode / YemaPT / Haidan 等程序，当前只有 NexusPHP 上传链路；未测过，不推断能不能发 |
| 源站故障/过载 | aling、hdu | aling 全 500 空包（源站报错，非 WAF）；hdu 全站过载回 262 字节负载壳；待站点恢复后复测 |

### 7.4 "需补适配"站按缺口类型归类

| 缺口类型 | 涉及站点 | 本轮证据 | 修一次能覆盖几站 |
| --- | --- | --- | --- |
| 上传表单结构不支持（自定义 JS 表单、隐藏字段、action 落到别处） | audiences、longpt | audiences：`type` 隐藏值 0，无选档逻辑；longpt：action 误判到 `bonusrain.php` 中止 | 白名单 2 站；同一套表单解析改造还能给 hddolby、mypt、ptt、rousi、ttg 等动态站打底 |
| 判重/详情页检索超时，或回包认不出来 | dragonhd、ptzone、pthome、city13、hdvideo、cyanbug | dragonhd 检索超时；ptzone 详情页超时；pthome"已存在！"落 failed；city13 HTTP 200 未识别；hdvideo `existed=1` 线索定位不到；cyanbug 首轮 TLS EOF | 6 站（统一加检索重试 + 错误文案/回包分类 + 站内搜索兜底确认） |
| G2 选档下拉匹配不上（`medium_sel`/`audiocodec_sel` 未选） | hdfans、kufei、hdvideo、ptsbao、city13、ptzone | 今日被 G2 拦下的条目，片源/音频 token 对不上站点真实选项；同站另有条目发成，说明是偶发或单类型映射缺口 | 6 站（补选档映射和 `selectPicks`；匹配不上仍落人工，不硬填） |
| 分类映射错误 | ptfans | Coraline 被归到"H漫(动漫、漫画)"区 | 1 站；分类文案映射表是全站共用，修正后其他站同类问题一并受益 |
| 挂回时 tracker 别名识别不全 | qilin | 480763 站点种子带 `hdkylin.top` announce，和页面域 `hdkyl.in` 是同站别名，被单站策略当非目标 tracker 拒绝 | 1 站；别名归一逻辑可复用给其他多 announce 站 |
| 候选范围/安全集校验失守 | lajidui | 恢复禁转标记后按全量候选生成草稿，把安全集外的人民的名义、伪装者发了上去 | 1 站；编排层校验，白名单/安全集范围对所有站生效 |
| 发布前自动补图没接进流水 | sbpt、hdbao、hdfans（续测预检发现） | 白色巨塔在 sbpt 7 张图全死、剪完 0 张；伪装者在 hdbao 9 张全死；Liar Game 在 hdfans 只剩 1 张海报；目前补图是半自动脚本流水，发布前不会自动找海报/抽帧 | 所有要图站（藏宝阁/优堡/太乙/麒麟/熊猫/ptlgs 等）共性问题 |

### 7.5 下一步最值得做的 5 件事（按影响面排序）

1. **修回包识别和检索超时重试**（解锁 dragonhd、ptzone、pthome、city13、hdvideo、cyanbug 6 站）
   - 把"已存在！"这类站点明文判重归入重复，不再记失败；检索/详情页 30 秒超时允许重试 1–2 次；HTTP 200 但页面认不出来时，先站内搜索或看账号上传列表确认到底发没发，再决定重试。
   - 验收：city13 一条演练回包能正确归类；dragonhd、ptzone 检索超时自动重试后拿到结果；pthome"已存在！"落重复而不是失败；有单测覆盖这几类回包，一轮 dry-run 没有未识别回包。
2. **补 G2 选档映射**（解锁 hdfans、kufei、hdvideo、ptsbao、city13、ptzone 6 站，部分与第 1 件重叠）
   - 把今日被拦条目的片源、音频 token 对到站点真实选项；站点列表里没有的选项落人工，不硬填。
   - 验收：今日被拦的 7 条 dry-run 演练 G2 全过，或明确落人工；tey 的蓝光源仍被正确拦下（站点无 Blu-ray 档，不允许硬填 HDTV）。
3. **修表单入口识别和隐藏/JS 字段解析**（解锁 longpt、audiences 2 站，并为动态表单站打底）
   - longpt 正确识别 `takeupload.php`，不再走到 `bonusrain.php`；audiences 隐藏 `type=0` 字段要有选档逻辑；提交链路仍标注需要浏览器前台 Turnstile。
   - 验收：longpt 演练 action 指向上传处理页；audiences 表单能构造出 `type` 选值；audiences 判重仍走真实浏览器，后台标签页明确报"标签页在后台"。
4. **修分类映射和候选安全集范围**（解锁 ptfans、lajidui 2 站）
   - ptfans 影视条目不再落到 H 漫/动漫区；候选生成只走白名单和安全集，恢复禁转标记后不扩大范围。
   - 验收：Coraline→ptfans 分类落到电影/剧集；lajidui 候选里不再出现安全集外的人民的名义、伪装者；用回放脚本复现同两条问题条目。
5. **挂回前归一 tracker 别名 + 发布前自动补图**（解锁 qilin 1 站，并消除要图站的死图退回）
   - 挂回前把同站 announce 别名归一（如 `hdkylin.top` ↔ `hdkyl.in`），只有真的别的站 tracker 才拒绝；发布前死图剪完不足时，自动从正片抽帧或取海报、传 pixhost 再写回简介，接进自动流水而不是靠手工脚本。
   - 验收：qilin 480763 按别名核对后能挂回；sbpt/hdbao/hdfans 死图条目 dry-run 能补足图数，补不足的明确落人工；补图动作在自动发种流程里自动触发。

### 7.6 只能半自动 + 人工兜底的站及原因

| 站点 | 原因 | 程序现在能做到哪一步 |
| --- | --- | --- |
| cspt 及其余无权限站（名单见 7.3 第一组） | 账号没有发布权限，站点侧限制 | 只能辅种；发布要等账号权限变化 |
| luckpt | 站点官方 LuckAudit 预审，预审不过不发 | 预审通过的条目能自动发；预审本身程序代过不了 |
| tey（蓝光/Remux 部分） | 媒介下拉没有 Blu-ray 档 | HDTV/WEB-DL 能自动发；蓝光/Remux 只能人工或不发 |
| itzmx（大种部分） | `.torrent` 文件上限 266,240 bytes | 小种能自动发；大合集被业务错误框拒绝，本轮 Coraline 还回了 403，原因待查 |
| tjupt（超大合集部分） | 站点后端处理大种子超时，空回包 | 25GB 左右单条能自动发；几百 GB 合集要拆小或换内容 |
| qingwa（大合集） | `torrent_too_large`，站点要求走 `offers.php` 候选入口 | 记 `pending_offers`，候选表单不代填，等人去候选页提交 |
| audiences | 判重依赖真实浏览器前台渲染 Turnstile；上传页是自定义 JS 表单 | 检索走 Kimi WebBridge；后台标签页明确报错，不盲发；表单选档未接 |
| panda | 2026-11-27 闭环已跑通并恢复发种 | 141840 上传后立刻挂回 qB 校验 100%、只挂单站 tracker，等站点审核；旧账 141276 已删，残留 `3f8ed774…` 待人工清理 |
| hddolby、mypt、ptt、rousi、ttg、haoxue 等动态表单站 | 静态表单拿不到，适配器形态不支持 | 只能记录站点形态，要按前端接口重做；haoxue 2026-11-27 已走站点接口发一条，长期是否维护浏览器流程见 7.9 待办 |
| 连不通站（freefarm、hdclone、hudbt、u2、shadow、sunny、piggo、tu88） | 网络/TLS/登录态不通 | 整站暂停，连通后再测；freefarm 迁 `farmm.cc`，是否切 URL 待人工（见 7.9） |
| 类目不符站（haitang、kelu、vclib、nicept、ziran、soulvoice、musopia、dicmusic、xingtan） | 没有影视类目 | 只做提示，不转种 |
| 已关站（momentpt、ptlao、movie52、yinghua） | 告别页/404/域名失效 | 暂停并记 `statusNote` |
| 外文程序站（Gazelle/Unit3D/Blu/XBTIT/Discuz/TNode 等） | 适配器未接入 | 未验证，不在自动发种范围 |

### 7.7 与 `SITES.md` 实测对不上的地方（待另一处统一修订，本节不改 `SITES.md`）

1. **cspt**：`SITES.md` 2.1、2.5、第 3 节表标"可发，259870 已挂回"；今日审计 4 条都是"该账号无发布权限"。权限疑似被收回，状态需要改。
2. **ptfans**：`SITES.md` 2.1 标"可发 11111"，2.5 行只记了 `medium_sel=8` 选档；今日分类被归到 H 漫/动漫区，源详情页还超时；配置 note 写着 `publishEnabled 仍 false`，文档没反映分类映射问题。
3. **longpt**：`SITES.md` 2.2 表标"可发 51065"，2.5 行备注 action 可能误配但读起来像已修；今日 4 条仍落到 `bonusrain.php` 中止。
4. **audiences**：`SITES.md` 2.5 行只记了 Turnstile/后台标签页；今日两次失败在上传表单结构（自定义 JS、`type` 隐藏值 0，程序不支持），这个表单缺口文档没记。
5. **city13**：`SITES.md` 2.4 记 88256 靠站内搜索确认已发；今日 2 条仍 HTTP 200 未识别，回包缺口还在。
6. **itzmx**：`SITES.md` 2.1 标"可发（限小种）"；今日出现 HTTP 403（Coraline）和通用上传失败（War），403 是新现象，文档未记。
7. **hdfans**：`SITES.md` 2.3 只记 287791、287874 已核实做种；今日 A Touch 被 G2 `medium_sel` 拦，选档缺口未记。
8. **ptzone**：`SITES.md` 2.2 标"可发 190916"；今日详情页复核超时、`audiocodec_sel` 未选被拦，稳定性和选档缺口未记。
9. **扫描站数**：2026-11-27 配置实测 `enabled=true` 是 101 个，`SITES.md` 第 1 节已同步为 101。
10. **cangbao 43994**：`SITES.md` 记审核图标已标"通过"；今日批次备注页面仍标"未审"，qB 已校验 100%。
11. **lajidui**：`SITES.md` 2.4 记 4 条等审核、三项下拉已核对；今日 85084、85085 是安全集外误发，候选范围失守问题文档未记。
12. **qilin**：`SITES.md` 2.5 行只记缺图/换图等复审；今日 480763 挂回被非目标 tracker 拒（`posted_unresolved`），挂回 tracker 别名识别缺口未记。

### 7.8 2026-11-26 复核后的修正（本节覆盖 7.3、7.6 里过时的说法）

上一轮差距评估之后，把 47 个未启用站、13 个暂停站、6 个动态界面站、7 个表单缺口站全部实测了一遍，结论有变动：

- **未启用 47 站里只有 agsvpt 真能启用，已经启用**：`sourceSites.enabled=true` + 进 `autoReseedSites` + `team_sel` 兜底 Other=22；`gatecheck` 全绿（17 字段 / 9 下拉 / 7 项检查）；qB 里 806 条该站 tracker 种子可抄 passkey（tracker 主机 `tracker.agsvpt.cn`）。它不再是"权限待实证"，而是"已启用、等 546963 复审"。
- **剩下 46 个未启用站的原因收窄成四类**：差登录 3（azusa、tlf、generationfree）；校外被拦 1（byr，CERNET IPv6 + wengine 网关 403）；已关站 1（yinghua）；其余是**框架不支持**——Unit3D 13 / custom 8 / Gazelle 6 / XBTIT 5 / Blu 2 / Haidan 1 / YemaPT 1 / Discuz 1 / TNode 1。代码 `search_capable()` 只对 NexusPHP 为真，配置加载阶段就过滤，改 `enabled` 没用。要解锁这批等于新增框架适配器，属新能力，不是配置活。
- **整站暂停从 13 降到 8**：保留 hudbt、hdclone（无发布权限）、freefarm（迁 farmm.cc，域名是否切、cookie 怎么补待人工，见 7.9）、u2（登录墙 + 两步验证）、shadow（cookie 失效）、sunny（改版 SPA 走 JSON 接口）、piggo（WAF + 会话失效）、tu88（cookie 失效 + 图形验证码）；摘掉 gtk 与四个已关站（momentpt、movie52、ptlao、yinghua）。
- **gtk 的卡点已用按站代理解决（2026-11-27）**：站点实测可用（登录态有效、共享 tracker `t.myaltbox.com` 有 170 条可抄 passkey，`overrides.announceURL` 已填）。配置 `overrides.proxy=http://127.0.0.1:7890` 后走代理链路可达，`gatecheck` 只剩内容侧阻断（MediaInfo 取不到、0 张可用图）；尚未排进自动发种队列。
- **6 个"动态界面"站里只有 3 个真动态**：hddolby 其实标准静态表单（17 字段 / 9 下拉，只是要过 2FA）；hdcity 两段式（首段 5 字段，二段编辑页要真实上传后才拿得到）；mypt 时段禁传 + Cloudflare；rousi 是 SPA 需逆建种接口；ptt、ttg 是账号/板块没权限。
- **7 个表单缺口站里 3 个已落地配置并实发**：iloli（team Other=5）、muxuege（team Other=52 + 普通 AAC=6）、fenghuang（team other=1）；代码侧补了通用地区匹配、同名候选取首个、年代档匹配、分类联动行不计缺口。2026-11-27 复核：xingtan 无影视类目移出；haoxue 单页应用走站点接口发一条；hdu 全站过载；aling 源站 500。
- **熊猫已闭环并恢复（2026-11-27）**：141840《Liar Game S01 2007 2160p WEB-DL》上传后立刻挂回 qB 校验 100%、只挂单站 tracker，7 图逐张 200、简介六段齐全，`pending_review` 等审核；已从 `reseedPausedSites` 移出并留在白名单。旧账 141276 已删，残留 `3f8ed774…` 待人工清理。

下一步优先级（在前面的 5 件之后）：

1. ~~**扩失败分类，别把"站侧阻塞"当失败**~~（2026-11-27 已完成）：`no_permission` / `two_fa` / `challenge_gate` / `spa_api` 单列审计分类，不进失败冷却，覆盖记录落 `skipped` 中性终态，`bsend audit` 合计单列"站侧阻塞"行。
2. ~~**按站代理开关**~~（2026-11-27 已完成）：`sourceSites[].overrides.proxy`，空/缺省直连行为不变，填了只有该站客户端走代理；gtk 已用上。
3. **hdcity 二段编辑页字段 + rousi 建种接口**：都要真实浏览器，前者真发一条后核对，后者从懒加载包逆。
4. ~~**xingtan 两级分类驱动**~~（2026-11-27 复核取消）：该站无电影/剧集类目，影视无处归档，已移出白名单，不再投入适配。
5. ~~**熊猫闭环**~~（2026-11-27 已完成）：141840 真发一条并挂回校验 100%，等站点审核收尾。

### 7.9 2026-11-27 实测结论与新增待办

**本轮实发四条**（详情页与 hash 见 `SITES.md` 2.8）：panda 141840、muxuege 59990、fenghuang 39168 由引擎发并挂回 qB 100%；haoxue 23943 走站点接口发，待审期下载接口 404，没挂回，审核通过后由回访挂回。

**代码能力与口径变化**：

- 发种前按 `descrOrder` 自动装配简介分段（只在配了该规则的站生效；源简介六段齐就逐字节不动；装配后自查段序、图数下限、MediaInfo，缺任一项阻断）。
- 按站代理 `overrides.proxy`；站侧阻塞四类从失败口径拆出；`announceURL` 主机自动并入允许清单；上传被拒又解析不出原因时把回包正文落临时目录（已落地并有单测，还没有真实拒绝回包做实样核对，见 `OPERATIONS.md` 4.1）。

**待办（还没决定怎么做）**：

1. **手动发布要不要进审计**：手动 `bsend reseed --yes` 只往 `reseed-log.jsonl` 记，不往 `auto-audit.jsonl` 追加；按日限额、失败冷却的判定因此看不到这批手动发布。需要决定是否统一写进审计。
2. **iloli 的 AC3 同名候选**：audiocodec_sel 里 AC3 有多个同名候选（`DD/AC3`、`DD/AC-3`、`AC3` 这类），引擎判歧义留空不猜，被 G2 拦。先读表单真实 value，在 `reseedRules.selectPicks` 钉住普通 AC3 档，或在代码里补音频选档映射。
3. **望雪这类"引擎走不了、只能走站点接口/浏览器"的站**：要长期维护一套浏览器提交流程，还是每次人工处理，待决定。
4. **freefarm 域名切换**：站点已迁 `farmm.cc`，新旧域名都能登录；是否把配置里的站点 URL 从 `pt.0ff.cc` 切到 `farmm.cc` 并补该域 cookie，待人工看两域名是否共享会话后决定。

## 8. 相关文档

- 为什么这样判断、怎么发一条种子：`ARCHITECTURE.md`
- 数据文件、命令、巡检、排障：`OPERATIONS.md`
- 站点清单和受阻原因：`SITES.md`、`SITE-DEBUG.md`
