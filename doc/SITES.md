# 站点清单与现状

这张表回答四个问题：配置里有哪些站、哪些参与自动发种、哪些站卡住了、卡在什么地方。它不回答“某个站已经覆盖了多少内容”，那要看 `bsend status`。逐站适配经验和门禁总表见 2.5，单站排查过程见 `SITE-DEBUG.md`，发种流程见 `ARCHITECTURE.md`。

## 1. 当前范围

- 配置里一共 **147** 个站；
- 参与扫描的 **101** 个（2026-11-27 按配置 `enabled=true` 实测）；
- 允许自动发种的 **45** 个（`assist.autoReseedSites`）。

白名单只是“允许发”，不等于“一定发得出去”。实际发布还要过站点权限、字段检查、当天额度和失败冷却（默认关闭）。

2026-11-22 起，不再为了判断“站上有没有”逐个搜索，也不再日常批量跑表单检查。表单检查只在新站第一次接入时做一次，用来拿上传页字段和下拉选项。以前的检查结论只作参考，站点改版后可能失效。

当前白名单（46 站）：

```
audiences, baozi, cangbao, dragonhd, hdfans, hdtime, hdvideo, kufei, lajidui,
lemonhd, longpt, luckpt, ptba, ptcafe, ptfans, pthome, ptlgs, ptsbao, ptzone,
qingwa, sbpt, shuioudao, tangpt, xdy, city13, crabpt, cspt, cyanbug, dubhe,
hdbao, itzmx, nanyang, novahd, oshen, qilin, railgun, tey, tjupt, ubits,
xingyunge, agsvpt, panda, iloli, muxuege, fenghuang, gtk
```

2026-11-27 新进白名单的是 agsvpt、panda、iloli、muxuege、fenghuang；gtk 当天复核后临时加入，等站点发布窗口，见 2.9。hdu、aling、xingtan、haoxue 复核后移出。望雪（haoxue）是单页应用，引擎的静态表单走不了，不进自动名单，发布只能走站点接口或浏览器。

## 2. 2026-11-22 夜间接入的 12 个站

“发布并挂回成功”要同时满足三件事：详情页标题和本地一致、上传者是本站自己的账号（sbpt 实测为 `Schalkiiii`）、qB 里带 `Boxsend` 标签的种子进度到 100%。

| 站点 | 结论 | 实测结果 | 详情页 / 新种 hash | 备注 |
| --- | --- | --- | --- | --- |
| ptzone | 可发 | 已发布，五项发布检查全过 | https://ptzone.xyz/details.php?id=190916 ｜ `10d23b0e…` | 发布并挂回成功 |
| kufei | 可发 | 已发布，五项发布检查全过 | https://kufei.org/details.php?id=141933 ｜ `266fbb06…` | 没有独立的 MediaInfo/IMDb 字段，内容并入简介 |
| longpt | 上传入口未定位（2026-11-27 复核） | 2026-11-22 已发布 51065；11-27 复核 `upload.php` 只有 `bonusrain.php` 捐款表单（hidden + number，0 个 file input），`takeupload.php` GET 只回“上传失败！”页 | https://longpt.org/details.php?id=51065 ｜ `91882f95…` | 程序现在按 `no-form` 中止，不再把种子 POST 到捐款接口；真实上传入口仍未定位，v1 暂不支持 |
| tangpt | 可发 | 已发布，五项发布检查全过 | https://www.tangpt.top/details.php?id=72943 ｜ `2c7da189…` | 第一次抓到的是搜索表单；TLS 需要跳过校验 |
| ptlgs | 可发 | 已发布，五组下拉全填 | https://ptlgs.org/details.php?id=115117 ｜ `aa85a3ad…` | 硬性要求海报、至少 3 张截图（总图 ≥4）、MediaInfo；没有 DDP/E-AC3 |
| dragonhd | 可发 | 已发布并挂回 | https://www.dragonhd.xyz/details.php?id=48745 ｜ `bac2dd51…` | 中字、国语、HDR10 标签要人工补 |
| hdtime | 可发 | 已发布并挂回 | https://hdtime.org/details.php?id=238600 ｜ `77b02a63…` | 上传表单里没有分辨率下拉 |
| hdvideo | 已补挂 | 必填项修好后发布成功，新种 78856（11.56 GB） | https://hdvideo.top/details.php?id=78856 ｜ qB hash `dd7f6097…` | 必填的地区 `region_sel[4]=2`、题材 `style_sel[4][]=6` 已补；发后定位取错旧种 id（缺陷已修）。2026-11-23 已从详情页下载目标种补挂，qB 校验 100%、tracker 正常 |
| luckpt | 缺口待修 | 站点自带发布检查 95/100 | — | 豆瓣防盗链已修（补 `Referer` 后 418 变 200）；`img.pterclub.com` 在本机 TLS 连不上，截图数量不足 |
| qingwa | 无主流缺口 | 665/667 条有记录，约 99.7% | — | 没覆盖的是成人合集（被内容黑名单拦）和多片合集，不是适配器故障 |
| haitang | 类目不符 | 没有发布 | — | 只有曲艺、有声书类目；已移出白名单，规则留档 |
| haoxue | 需要重做 | 单页应用 | — | Next.js/Flux 单页应用，引擎静态表单走不了，已移出白名单；2026-11-27 走站点接口实发 23943，见 2.8 |

扩容候选里，city13、novahd、xingyunge、cyanbug 这 4 个站已经实测发布并挂回成功；hdvideo（78856）、qilin（479362）站上已经发布，2026-11-23 已全部补挂到 qB 并校验 100%；hdbao、tey、oshen、ptfans、itzmx、railgun、lemonhd 也在 2026-11-23 首次实发（见 2.1、2.2）；agsvpt 2026-11-27 已启用并进白名单（见 2.5 与 `SITE-DEBUG.md` 6.19）；panda 的 141276 后来被系统删除，2026-11-27 新实发 141840，见 2.8；qingwa、ubits 不是“无缺口”，最新 qB 口径分别是 402/3,388、656/3,388，缺口 2,986、2,732；其余没测或类目不符。证据见 `SITE-DEBUG.md`。

### 2.1 2026-11-23 新实发的 10 个站

这十站都是第一次向目标站上传，发布后站上 hash 和本地构造的 hash 不同；ptba 已确认是 `info.source` 被站点改写，cspt/dubhe/ptsbao/crabpt/hdbao/tey/oshen/ptfans/itzmx 的分片和标题未变但具体改写字段没有逐字节核对。当时程序按“该站白名单 + pieces 一致 + `info.name` 一致”放行，现场补挂成功；现行规则已改为只核对 pieces + 文件清单，白名单不再参与放行。

| 站点 | 结论 | 实测结果 | 详情页 / 新种 hash | 备注 |
| --- | --- | --- | --- | --- |
| ptba | 可发 | 已发布并挂回，qB 校验 100% | https://1ptba.com/details.php?id=106718 ｜ `abf41c82…` | 站上把 `info.source` 写成 `[1ptba.com] 1PTBA.COM`；已加入 hash 改写白名单，后续同站不用再手工补挂 |
| cspt | 权限疑似被收回（2026-11-26） | 2026-11-23 已发布并挂回，qB 校验 100%；11-26 四条审计均“该账号无发布权限” | https://cspt.top/details.php?id=259870 ｜ `cf568e1a…` | 目标种 tracker 在 `tracker.cspt.cc`，页面域是 `cspt.top`；已补 announce 别名，站点种自带两条同站 announce。11-23 的“可发”结论已被 11-26 的权限报错取代，能否再发待确认 |
| dubhe | 可发 | 已发布并挂回，qB 校验 100% | https://dubhe.site/details.php?id=55492 ｜ `b5ebc539…` | 第一次判重搜索 30 秒超时，重试通过；站上已有同片 1080p 其他版本，按策略继续发布；表单没有“完结”标签，发布后手动补 |
| ptsbao | 可发 | 已发布并挂回，qB 校验 100% | https://ptsbao.club/details.php?id=135787 ｜ `0b0087f7…` | `source_sel[1]` 是二级分类，已按 type 父类 + 标题自动填 `剧集-大陆`；上传也出现 hash 差异，已加入白名单；表单没有“完结”标签，发布后手动补 |
| crabpt | 可发 | 已发布并挂回，qB 校验 100% | https://crabpt.vip/details.php?id=240242 ｜ `732e2c99…` | 此前候选源站 `hdfans` 502，被失败冷却挡住，改选 `hdsky` 源后成功；站上已有同片 2160p 其他版本，按策略继续发布；上传出现 hash 差异，已加入白名单 |
| hdbao | 可发 | 已发布 2 条并挂回，qB 校验 100% | https://hdbao.cc/details.php?id=54402 ｜ `95eacf24…`；https://hdbao.cc/details.php?id=54403 ｜ `51ededb2…` | `autopost --limit 1` 实测连发两条（`posted_unresolved` 没计入 limit，已修）；站上同样改写 hash，两条按 pieces/标题校验补挂成功，已加入白名单 |
| tey | 可发（限非蓝光） | 已发布 1 条并挂回，qB 校验 100% | https://pt.tey.cc/details.php?id=623 ｜ `c9570904…` | 表单 `medium_sel[4]` 只有 WEB-DL/HDTV/DVDR/CD/Track，没有 Blu-ray；蓝光源会被选档门禁拦住，HDTV/WEB-DL 正常。qB 里 5 条老种全走 `tracker.tey.cc`，已补 announce 别名；站上改写 hash，已加入白名单；新种状态“未审”，2026-11-24 回访仍标 `title="未审"`，覆盖记录已落 `pending_review`（等站点审核），由回访跟进 |
| oshen | 可发 | 已发布 1 条并挂回，qB 校验 100% | http://www.oshen.win/details.php?id=57082 ｜ `22e3dd63…` | 两次失败后第三次恢复：判重搜索正常、上传成功；回包没给详情链接，落 `posted_unresolved`，按详情页补挂成功；站上 hash 与构造不同（构造 `0f73ae73…`，站上 `22e3dd63…`），已加入白名单；表单没有“完结”标签，发布后需人工补 |
| ptfans | 2026-11-26 受限（类目/源站超时） | 2026-11-23 已发布 1 条并挂回，qB 校验 100% | https://ptfans.cc/details.php?id=11111 ｜ `fdf27b29…` | 此前上传页和 IMDb 判重都超时；2026-11-23 恢复后一次通过。站上已有 720p 其他版本 6095，按策略继续发布；上传同样改写 hash（构造 `4fd4392f…`，站上 `fdf27b29…`），已加入白名单；表单无“完结”标签，人工补。2026-11-26：Coraline 被归到 H 漫/动漫区、源详情页超时；配置 note 仍写 `publishEnabled false` |
| itzmx | 可发（限小种） | 已发布 1 条并挂回，qB 校验 100% | https://pt.itzmx.com/details.php?id=13134 ｜ `88e3103b…` | 站点 torrent 文件上限 `266,240 bytes`，大合集/大剧包普遍超限；改用 `ubits` 源的《Liar Game》小种（源种 96,934 bytes）一次通过。上传后 hash 被站点改写（构造 `7744200c…`，站上 `88e3103b…`），已加入白名单；详情页没找到“中字/完结”标签，发布后手动补 |

### 2.2 2026-11-23 继续实发：railgun、lemonhd

这两站也是第一次实发，上传成功、站上 hash 与构造 hash 不同；按 pieces + `info.name` 校验后补挂，qB 进度 100%。

| 站点 | 结论 | 实测结果 | 详情页 / 新种 hash | 备注 |
| --- | --- | --- | --- | --- |
| railgun | 可发 | 已发布 1 条并挂回，qB 校验 100% | https://bilibili.download/details.php?id=60632 ｜ `72889498…` | 源 `carpt` 的 Poirot 合集；上传回包未给详情链接，落 `posted_unresolved`；补入 hash 改写白名单后把站点种子下回来挂进 qB，qB 校验 100%，tracker 为 `bilibili.download` |
| lemonhd | 可发 | 已发布 1 条并挂回，qB 校验 100% | https://lemonhd.net/details.php?id=48970 ｜ `3c00cbb2…` | 源 `carpt` 的 Poirot 合集；站上已有不同版本 13452，按策略继续发布；上传回包未给详情链接，落 `posted_unresolved`；补入 hash 改写白名单后把站点种子下回来挂进 qB，qB 校验 100%，tracker 为 `tracker.lemonhd.net` |

### 2.3 2026-11-24 凌晨实发：pthome、nanyang、ubits（已全部补挂）

三站都是第一次上传成功，但下载回来的目标种 hash 和上传时构造的不同（站点具体改了哪个字段仍未逐字节核对）。三站已加入 hash 改写白名单，用 `bsend pull` 把站点种子下回来挂进 qB，完成：qB 校验 100%、只挂单站 tracker、状态 `seeding`。补挂后覆盖记录曾被写成 `same_hash_skip`，已用 `bsend prime` 按 qB 真值改回 `seeding`。**站上已经发了，不要重发**。

| 站点 | 结论 | 实测结果 | 详情页 / hash | 备注 |
| --- | --- | --- | --- | --- |
| pthome | 已补挂 | qB 校验 100%，状态 `seeding` | https://www.pthome.net/details.php?id=284701 ｜ 构造 `b31d8eaf…`，站上 `ebaa6dd9…` | 源 hdfans 133877《人民的名义》2160p WEB-DL；qB 里 11-23 23:56 就在做种，本次补挂当时报“同 hash 跳过”，但没有查 tracker；后续按 qB 真值核对为只挂 pthome tracker、`bsend prime` 改回 `seeding` |
| nanyang | 已补挂 | qB 校验 100%，状态 `seeding` | https://nanyangpt.com/details.php?id=226159 ｜ 构造 `187d1487…`，站上 `8a9b9ab2…` | 源 soulvoice 54682《知否知否应是绿肥红瘦》4K WEB-DL；表单没有“中字”类标签复选框，需人工补 |
| ubits | 已补挂；审核没过已换图 | qB 校验 100%，做种中；覆盖记录 `review_rejected` | https://ubits.club/details.php?id=379266 ｜ 构造 `d9970bce…`，站上 `2a57b787…` | 源 hdsky 27637《伪装者》1080p HDTV；前两次报“上传失败！”，补 `source_sel[4]`（地区）和 `team_sel[4]`（`Other`）后通过。11-24 站内信判“海报与截图均无法访问（图床域名已失效）”：简介 9 张图全是已失效的 `i13.tietuku.com`；已换 imgbox 新图（1 海报 + 8 截图）并提交，页面仍显示不通过，等管理复审（详见 `SITE-DEBUG.md` 6.12） |
| hdfans | 已确认并补挂 | 287791 就是同一条《伪装者》，qB `b1e600a6…` 校验 100% | http://hdfans.org/details.php?id=287791 | 覆盖记录已是 `reseeded`（11-23 23:49 转种成功），qB 里 11-23 23:49 就在上传，无需重发 |
| tjupt | 已实发 25GB 单条；274GB 合集仍卡后端超时 | 652794 已挂回 qB、校验 100%，等站点审核 | https://www.tjupt.org/details.php?id=652794 | announce 地址按站点下载的种子定案为 `tracker-public.tjupt.org`（https、带 passkey，已写进 `overrides.announceURL`）。2026-11-25 真发《我们的父辈》25GB 成功，站上改写 hash/标题，按 pieces + 文件清单核对后挂回 qB，校验 100%；页面还标“待审核”。同内容的 274GB《波洛全季》合集上传 5 分钟仍回 HTTP 200、正文 0 字节，站点没有留下条目，重试仍会空回包；要发得先拆小或换内容。站上已有 `416154`（FLAC/DDP5.1）与 `236519`（同名 FRDS 合集、pieces 与本地不同），属实不同内容 |
| qingwa | 分区：Poirot 不走普通上传；《伪装者》已发并挂回 | Poirot = 站点判定过大，记为待走候选入口；《伪装者》`208575` 校验 100% | https://qingwapt.com/details.php?id=208575 ｜ 构造 `c78e9cb2…`，站上 `d1eeec5c…` | Poirot 回包其实是 `torrent_too_large` 表单（自动 submit 到 `offers.php?add_offer=1`）：站点要求改走候选入口，不是未知回包；程序记 `pending_offers`（待走候选入口），不算失败、不自动重试，`bsend status` 单列“待跟进”等人工去候选页提交（2026-11-24 11:33 真跑复现一次，覆盖记录落 `pending_offers`、审计 result=`pending_offers`、`status` 显示“待走候选入口 1”）。同站《伪装者》发布后自动挂回被 hash 校验拒绝，改用 `bsend pull` 把站点种子下回来挂进 qB，成功 |
| audiences | 判重已接 Kimi WebBridge，当前卡在验证框不渲染 | — | 程序按“没确认重复就不上传”中止，没有盲发；无头 HTTP 复用不了浏览器会话。2026-11-24 晚可行性实验通过：Kimi WebBridge 驱动本机真实浏览器，audiences 已登录 `Schalkiii`；页面内 `fetch('/torrents.php?search=…')` 返回 200、未跳登录、命中 2 个 `details.php?id=`、无人机验证。2026-11-25 已接入程序，但 Kimi 选中的标签页是后台（`visibilityState=hidden`），Turnstile 不生成 iframe、token 为空；需要把标签页切到前台再试，程序明确报“标签页在后台”，不盲发 |

**audiences 的后续方案（2026-11-24 更新）**：不再折腾无头 HTTP 复用 cookie，改走 Kimi WebBridge 接管用户已登录的浏览器：判重检索交给人机已经登录的窗口做，拿到结果再回到程序决定发不发。可行性实验已通过（登录态复用 + 站内检索都成功），代码已接入程序。

2026-11-25 实测卡点：Kimi 能找到用户已打开的 audiences 标签页（`borrowed=true`），但浏览器把页面判为 `document.visibilityState=hidden`，Turnstile 不生成 iframe，`cf-turnstile-response` 一直为空；显式调用 `turnstile.render` 返回 widget id 也没有 token。这不是“人机验证点一下”能解决的状态——页面必须真的可见。程序现在会把“标签页在后台”单独报出来，不把验证页当“站内没有”，也不盲发。把该标签页切到前台后重试即可。

### 2.4 2026-11-25 调试期小批量回归

这一轮对白名单站点逐站再发，原则是“先确认上一次到底发没发出去，再决定重不重发”。有代表性的结果：

| 站点 | 结果 | 详情页 | 备注 |
| --- | --- | --- | --- |
| baozi | 2 条，等审核 | https://p.t-baozi.cc/details.php?id=83738 | 波洛 294GB、知否；已挂回 qB |
| longpt | 1 条，等审核 | https://longpt.org/details.php?id=51248 | 知否；第二条上传页只有 `bonusrain.php` 捐款表单，程序按 `no-form` 中止，未发 |
| ptcafe | 1 条新发 + 1 条同 hash 已做种 | https://ptcafe.club/details.php?id=137250 | 伪装者 137250；人民的名义 https://ptcafe.club/details.php?id=137241 是原种已挂 ptcafe tracker，qB progress=1，按 `bsend pull` 复核记为 `seeding` |
| ptfans | 2 条已发布（11-26 复核受限，见 2.1/第 3 节） | https://ptfans.cc/details.php?id=11125 | 知否、人民的名义；2026-11-26 Coraline 被归到 H 漫/动漫区、源详情页超时，配置 note 仍写 `publishEnabled false` |
| pthome | 1 条已发布 + 1 条已存在补挂 | https://www.pthome.net/details.php?id=284810 | 金枝欲孽 78566 已补挂 |
| ptlgs | 2 条，等审核 | https://ptlgs.org/details.php?id=115313 | 伪装者、Coraline |
| ptlgs（独立图字段复测） | 1 条，等审核 | https://ptlgs.org/details.php?id=115350 | 《苦月亮》Bitter Moon；`cover` / `screenshots` 已填，详情页 `ktorrentcover` 有海报、`ktorrentscreenshots` 有 3 张截图，qB 校验 100%；站点仍标“未审” |
| ptsbao | 2 条已发布 | https://ptsbao.club/details.php?id=135824 | 广告狂人、伪装者 |
| shuioudao | 2 条，1 条补图后复审 | https://sewerpt.com/details.php?id=72463 | 波洛补 4 张 imgbox |
| tangpt | 2 条，等审核 | https://www.tangpt.top/details.php?id=73679 | 波洛、广告狂人 |
| xdy | 2 条已发布 | https://xdypt.vip/details.php?id=8049 | 知否、人民的名义 |
| nanyang | 2 条已发布 | https://nanyangpt.com/details.php?id=226221 | 广告狂人、伪装者 |
| oshen | 2 条已发布 | http://www.oshen.win/details.php?id=57209 | 人民的名义、伪装者 |
| xingyunge | 2 条，等审核 | https://pt.xingyungept.org/details.php?id=192560 | 知否、人民的名义 |
| hdvideo | 1 条，等审核 + 1 条同 hash | https://hdvideo.top/details.php?id=79023 | 知否 |
| ptba | 2 条已发布 | https://1ptba.com/details.php?id=106938 | 伪装者、绝世天劫 |
| railgun | 2 条已发布 | https://bilibili.download/details.php?id=60734 | 知否、广告狂人；完结标签缺复选框 |
| crabpt | 2 条，等审核 | https://crabpt.vip/details.php?id=240795 | 人民的名义、Liar Game |
| dragonhd | 2 条已发布 | https://www.dragonhd.xyz/details.php?id=48987 | 知否、广告狂人；完结/中字标签缺复选框 |
| dubhe | 2 条已发布 | https://dubhe.site/details.php?id=55718 | 波洛、广告狂人；新种页面已标“通过” |
| hdbao | 2 条已发布 | https://hdbao.cc/details.php?id=54657 | 伪装者、Coraline；Coraline 标题已手工修正 |
| lajidui | 4 条，等审核；其中 85084、85085 为安全集外误发 | https://pt.lajidui.top/details.php?id=85084 | 知否 84999、广告狂人 85000、人民的名义 85084、伪装者 85085；三项下拉和图片已核对。2026-11-26 复核：85084、85085 落在安全集之外，属于候选范围失守，此前文档只记了下拉和图片核对 |
| lemonhd | 2 条已发布 | https://lemonhd.net/details.php?id=49077 | 知否、广告狂人 |
| city13 | 1 条已发布；11-26 两条仍 200 未识别 | https://13city.org/details.php?id=88256 | 88256 上传回 200 未识别，站内搜索确认已发出后补挂；2026-11-26 另有 2 条仍是 HTTP 200 未识别回包。2026-11-27 起程序不再直接把 200 未识别记失败，改按名称 + 总大小核对，核对不到落 `posted_unresolved`；这 2 条仍待真实复测 |
| novahd | 2 条已发布 | https://pt.novahd.top/details.php?id=227655 | 伪装者、人民的名义；4K 60fps 选档已修 |
| qingwa | 1 条新发，等审核 | https://qingwapt.com/details.php?id=208692 | Bitter Moon；伪装者同内容已存在 |
| cspt | 2 条已发布，补清 MediaInfo；11-26 权限疑似被收回 | https://cspt.top/details.php?id=260050 | 站点不收 MediaInfo 和图片外 URL 标签，已清 `technical_info`；2026-11-26 四条审计均“该账号无发布权限”，能否再发待确认 |
| luckpt | 6 条已做种 | https://pt.luckpt.de/details.php?id=61704 | A Touch of Zen 61704、绿里奇迹 61705、The Intern 61706、Black 61707 已审核结束、qB progress=1、只挂 luckpt tracker；波洛 61665、广告狂人 61666 已做种；伪装者 61102 已补 MediaInfo |
| hdfans | 已做种；11-26 A Touch 卡在媒介选档 | http://hdfans.org/details.php?id=287874 | 同 `info hash` 原种已挂目标站 tracker；qB progress=1、tracker 只有 `hdfans.org`。`info hash` `d8c48453…`，pieces 指纹 `7fcdb513…`。Liar Game；历史 `same_hash_skip` 只表示当时没 add 第二条，判断是否做种必须查 qB tracker。2026-11-26：A Touch 被 `medium_sel` 未选拦下，媒介选档缺口此前没记 |
| sbpt | 七条都是本站账号的条目 | https://sbpt.link/details.php?id=21767 | 广告狂人 21767、A Touch of Zen 21768、知否 21764 已做种；绿里奇迹 21780、Aquarela 21782、我们的父辈 21783、The Intern 21784 已校验 100%。详情页作者均为 `Schalkiiii`，不是别人发的可辅种；七条链接和判断见 2.5.1。Aquarela、The Intern 缺“中字”标签需手动补 |
| cyanbug | 判重复核超时；已加重试待实测 | https://cyanbug.net/details.php?id=32829 | 未上传；2026-11-27 已加检索/详情页超时重试，单测过，真站未复测；不盲目重发 |
| ubits | 上传被图床黑名单拦下 | https://ubits.club/details.php?id=379266 | 简介含 imgbox，站点要求换 pixhost 等图床 |
| qilin | 审核未过，简介已转 BBCode | https://www.hdkyl.in/details.php?id=479362 | 等站点重审 |
| tey | 已补 3 张截图，质量档没有 Bluray 选项 | https://carpt.net/details.php?id=532 | 不是 tey 图床坏；tey 623《伪装者》9 张图正常。carpt 532 简介 0 个 [img]、编辑页无 takeedit 表单，已用 `scripts/seed-images.py` 补 3 张 pixhost 截图，tey 预检“3 张 ≥ 3”通过；卡在 `medium_sel` 只有 WEB-DL/HDTV/DVDR/CD/Track，没有 Bluray，不能硬填 HDTV，未上传 |
| tjupt | 274GB 合集仍超时；25GB 单条已发并挂回 | https://www.tjupt.org/details.php?id=652794 | 652794 校验 100%；2026-11-26 补 1 海报 + 1 截图和 `[mediainfo]` 段后重新进待审（见 2.6）；剧集分类字段历史上已实现，本轮改成规则驱动 + 提交前硬拦，11-26 只读演练载荷带齐三个字段、无必填缺失；274GB 合集本站没留下条目，不自动重试 |
| itzmx | 大种 403 / 超 266,240 字节；11-26 再出 403 | https://pt.itzmx.com/details.php?id=13134 | 2026-11-26 Coraline 403、War 通用上传失败；403 是新现象，根因待查，选种先看种子文件大小 |
| audiences | 判重已接 Kimi WebBridge；当前卡在验证框不渲染 + 上传表单结构 | — | 2026-11-25 重试：Kimi 能选中已打开的 audiences 标签页，但浏览器把页面判为 `visibilityState=hidden`（窗口最小化或不在前台），Turnstile 一直不生成 iframe、token 为空；显式 `turnstile.render` 也没有 token。同源 `fetch` 搜索仍返回“搜索人机验证未通过”。需要把该标签页切到前台、让页面可见后再试；程序明确报“标签页在后台”，不把验证页当站内没有，也不盲发。2026-11-26 另记一条缺口：上传表单是自定义 JS 表单，`type` 是隐藏值 0，静态解析不到可提交结构，这条之前没记 |

### 2.5 逐站适配经验与门禁（46 个自动发种站）

2026-11-25 汇总。先说清三层，不要把“代码能拦”“配置要填”“站点就是不给”混成一件事。

**全站硬规则（代码写死，不读配置）**

- 一份种子只带一个站的 tracker；上传前必须有该站完整 passkey 地址；上传后立刻从站点下载种子、核对分片、挂回 qB。
- 简介里的图片逐张探测，打不开的当场剪掉；不是“配了才剪”。
- 要图站有最低图数：藏宝阁、优堡、太乙、麒麟各 3 张，熊猫 4 张，ptlgs 4 张（1 张海报 + 3 张截图）；`gates::image_floor` 是硬编码，配置改不动。另有两条独立口径：AGSVPT 至少 1 张海报 + 1 张截图，北洋园（tjupt）至少 1 张图；有独立海报/截图字段的站按字段算，没有的按“首图算海报、其余算截图”。
- 标题按“分辨率 → 帧率/HDR → 片源 → 地区 → 视频编码 → 位深 → 音频 → 音频细节 → 压制组”重排，完结剧集去掉 `Complete`。
- 判重只认 `info hash` 和 pieces 指纹；同 hash 不等于已在目标站做种，必须查 qB 的 tracker 和 progress。
- 上传回包里的 `details.php?id=` 不能单独当证据；详情链接和下载链接的 id 必须对得上，拿不到就落待人工核对。
- `existed=1` 只说明站点上已经有同 `info hash` 的条目，不说明是谁先发的；要看详情页的“由 … 发布”和发布时间。

**按站配置（`assist.reseedRules`，只在配了的站生效）**

- `titleNoCjk`：主标题出现汉字就拦。当前是藏宝阁、熊猫。
- `requireLinks` / `requireMediainfo`：条目链接、MediaInfo 默认都要求；只有站点规则显式写 `false` 才放宽。
- `descrFormat`：简介默认转 BBCode；只有站点规则显式写 `html` 才保留原 HTML。
- `descrOrder`：简介必须按指定段落顺序。当前只有熊猫。
- `requiredFields`：核心必填字段没有值就拦。当前是 hdvideo 的 `region_sel` / `style_sel`。
- 必填标签门禁：站点配置可以声明上传必须勾选的标签（字段名 + 值 + 文案），构造完成后没勾上就在提交前拦下并说明缺哪个；代码层不预设任何站点“一律勾某标签”。AGSVPT 要在配置里声明 `tags[4][]` + `52`（方舟计划；13 是杜比，51 是合并，不要混用）。
- `selectPicks` / `multiPicks`：把有歧义的下拉或复选框定死。当前有 luckpt 的 BDRip、ptfans 的同名 Encode、hdvideo 的地区/剧情。
- 日限额：默认 2 条；luckpt、sbpt 各 20 条。`posted` 和 `posted_verifying` 都算一次上传；重复/失败/待跟进不占额度。
- `posterField` / `screenshotField`：上传页把海报、截图放在独立字段时，从简介图片里抽出来填进去；不靠简介图冒充独立字段。ptlgs 的 `cover` / `screenshots` 就是这种站。

**站点侧限制（程序不绕过，也不该硬填）**

- tey 的媒介下拉只有 WEB-DL / HDTV / DVDR / CD / Track，没有 Blu-ray；蓝光/Remux 不能硬填 HDTV。
- itzmx 的 `.torrent` 文件上限 266,240 bytes，大合集会被业务错误框拒绝；选种先看种子文件大小。
- ubits 拒 imgbox，简介图要换 pixhost 等站允许的图床；不是网络故障。
- audiences 的判重检索必须走 Kimi WebBridge 的真实浏览器；Cloudflare Turnstile 在后台标签页不会渲染，页面 `visibilityState=hidden` 时要先切前台。
- luckpt 有官方 LuckAudit 预审，预审不过不发；藏宝阁/优堡/太乙/麒麟/熊猫的图会被逐张审。
- 有些站会把 `info.source` 或标题改写，导致站上 hash 与上传构造的 hash 不同；挂回前只核对 pieces 指纹和文件清单（忽略首段根目录名），对得上就放行，不看 hash、也不要求标题一字不差。`allows_target_hash_rewrite` 白名单只用于日志标注“已实测改写”，不是放行条件。

**逐站表**。表里的“默认”指上面的全站硬规则 + 简介链接/MediaInfo + BBCode + 每日 2 条；只列额外项和站点侧差异。标题长度按 `titleMaxLen` 检查：本批 32 个配了 200；audiences、baozi、lajidui、lemonhd、ptcafe、pthome、shuioudao、xdy 这 8 个没配规则，长度不单独拦。

| 站点 | 额外门禁 / 发前必查 | 适配经验（站点侧差异） |
| --- | --- | --- |
| audiences | 默认；判重必须走 Kimi WebBridge | Cloudflare Turnstile 只在页面可见时生成 iframe；后台标签页要报“标签页在后台”，不盲发，也不把验证页当“站内没有” |
| baozi | 默认 | 常规 NexusPHP；tracker 别名已补，按 qB 覆盖真值核对 |
| cangbao | 主标题禁中文；简介 BBCode；≥3 张可达图；链接 + MediaInfo | 死图先剪再计数；死图用 imgbox 重传，不要关图门禁 |
| dragonhd | 默认 | 上传表单没有“中字/国语/HDR10”复选框时，发布后人工补标签 |
| hdfans | 默认 | 同 hash 必须查 qB：只挂 hdfans tracker、progress=1 才算做种，不能只看历史日志 |
| hdtime | 默认 | 上传表单没有分辨率下拉，不要硬找分辨率字段 |
| hdvideo | `region_sel=美国(2)`、`style_sel=剧情` 必填；标题 ≤200 | 不填会被通用“上传失败！”拒绝；发后定位按 id 配对，不能抓回包第一条旧种 |
| kufei | 默认 | 没有独立 MediaInfo/IMDb 字段，内容并进简介；上传回包正文可能才带唯一详情链接 |
| lajidui | `processing_sel`=文件容器（MP4/MKV）；`source_sel`=地区；`team_sel`=制作组 | 2026-11-26 起程序按容器、地区、名尾制作组自动填；制作组不在站点列表时选 Other，不冒充驻站组 |
| lemonhd | 默认（无规则） | 站上会改写元信息，按 pieces + 文件清单核对（白名单仅日志）；回包可能不给详情链接，必要时下载目标种补挂 |
| longpt | 默认（无静态上传表单） | 2026-11-27 复核 `upload.php` 只有 `bonusrain.php` 捐款表单，`takeupload.php` GET 只回上传失败页；程序按 `no-form` 中止，不提交到捐款接口，真实入口待定位 |
| luckpt | LuckAudit 官方预审；BDRip → `medium_sel=Encode(7)`；每日 20；链接 + MediaInfo | 预审 100/100 才发；截图不足先换图，不靠关图门禁；表单没有 BDRip 档 |
| ptba | 默认 | 站上改写 `info.source`，按 pieces + 文件清单核对（白名单仅日志）；下载目标种后核对 pieces 和文件清单，不要求标题一字不差 |
| ptcafe | 默认（无规则） | 同 hash 先查 tracker/progress；不要把“同 hash”直接当已做种 |
| ptfans | `medium_sel=8`（表单有两个同名 Encode，取 8） | 站上改写 hash，按 pieces + 文件清单核对（白名单仅日志）；表单无“完结”复选框，发布后人工补 |
| pthome | 默认（无规则） | 站上改写 hash，按 pieces + 文件清单核对（白名单仅日志）；页面域带 `www.`，tracker host 要按别名归一化；2026-11-27 已补“该种子已存在！”/“已存在！”精确提示识别，真实回包未复测 |
| ptlgs | 海报 + ≥3 张截图（总图 ≥4）；链接 + MediaInfo；无 DDP/E-AC3 | 海报和截图是独立字段 `cover` / `screenshots`，不是只放进简介；总图不足 4 张不发；2026-11-25 真发 https://ptlgs.org/details.php?id=115350 已确认详情页两个区块都有图 |
| ptsbao | 默认 | `source_sel` 是二级分类，按 type 父类 + 标题填；按 pieces + 文件清单核对（白名单仅日志）；无“完结”复选框 |
| ptzone | 默认 | 常规 NexusPHP，五项发布检查全过；2026-11-27 已加检索/详情页超时重试，单测过，真站复测未做 |
| qingwa | Remux 标题片源写 `BluRay`，不要带横杠；媒介为 Remux 时勾「Remux」标签；完结标题不带 `Complete`，技术顺序固定 | Poirot 合集被 `torrent_too_large` 要求走 `offers.php`，记 `pending_offers`；标题顺序/片源拼写/Remux 标签不对会被拒；2026-11-27 已补“已存在！”精确提示识别，真实回包未复测 |
| sbpt | 链接 + MediaInfo；每日 20；无官方预审 | `existed=1` 不能推断作者；本次七条详情页作者都是本站账号，见 2.5.1；缺“中字”复选框时只能发布后人工补 |
| shuioudao | 默认（无规则） | 简介图片可能需补图；改已发布种子只动简介，不换种子文件 |
| tangpt | 默认 | 第一次抓到的可能是搜索表单；TLS 校验按站点实际情况处理，不能把搜索页当上传页 |
| xdy | 默认（无规则） | 同 hash 按 qB tracker/progress 复核；不要凭日志判定 |
| city13 | 默认 | 上传可能回 HTTP 200 但页面不可识别；2026-11-27 起程序先按名称 + 总大小站内核对，找到就挂回，找不到落 `posted_unresolved`，不再直接判失败 |
| crabpt | 默认 | 站上改写 hash，按 pieces + 文件清单核对（白名单仅日志）；源站 hdfans 502 时换源，不重复发 |
| cspt | 默认 | 简介不收 MediaInfo 和图片外的 URL；`technical_info` 要单独清空；按 pieces + 文件清单核对（白名单仅日志） + `tracker.cspt.cc` 别名 |
| cyanbug | 默认 | 判重检索可能超时；2026-11-27 已加自动重试（真站未复测）；未确认站上没有之前不得上传 |
| dubhe | 默认 | 判重搜索偶发 30 秒超时，重试一次；按 pieces + 文件清单核对（白名单仅日志）；无“完结”复选框 |
| hdbao | 默认 | 站上改写 hash，按 pieces + 文件清单核对（白名单仅日志）；`--limit` 曾把待人工条目算进额度，已修 |
| itzmx | 默认 | `.torrent` ≤ 266,240 bytes；按 pieces + 文件清单核对（白名单仅日志）；无“完结/中字”标签时发布后人工补 |
| nanyang | 默认 | 站上改写 hash，按 pieces + 文件清单核对（白名单仅日志）；没有“中字”复选框时发布后人工补 |
| novahd | 默认 | 4K 60fps 要选 `2160p/4K 60Fps`，不能选普通 2160p；站上已有其他版本仍可发 |
| oshen | 默认 | 站上改写 hash，按 pieces + 文件清单核对（白名单仅日志）；无“完结”复选框；站点偶发抖动，重试前先核实上一轮结果 |
| qilin | ≥3 张可达图；简介 BBCode | 海报防盗链、旧图床失效会被判“缺少海报或截图”；先把图换到 imgbox 再提交 |
| railgun | 默认 | 站上改写 hash，按 pieces + 文件清单核对（白名单仅日志）；无“完结”复选框；回包可能没详情链接，需下载目标种补挂 |
| tey | ≥3 张可达图；媒介只收 WEB-DL/HDTV/DVDR/CD/Track | 无 Blu-ray 档，蓝光/Remux 不能硬填 HDTV；按 pieces + 文件清单核对（白名单仅日志） + `tracker.tey.cc` 别名；已发但未审要记等审核 |
| tjupt | 默认；剧集分类字段 `cname`/`ename`/`specificcat` 规则驱动 + 提交前硬拦 | announce 用 `tracker-public.tjupt.org`；上传超时秒数可逐站配（`uploadTimeoutSec`，缺省 120）；274GB 大合集先别自动重试，25GB 单条已实测成功并挂回。分类字段历史上是写死的 tjupt 特判，本轮改成站点规则抽象（`CategoryRule`，配置 `categoryRules` 可覆盖、缺省走内置兜底表），提交前拉 `showcategorydetail.php?catid=402` 片段按真实控件名核对，控件不在记缺口、不臆造字段；派生不出中/英文名时记本地缺口、真提交前硬拦点名缺哪个。2026-11-26 只读演练（`gatecheck --sites tjupt`，只 GET 不 POST）载荷带 `cname`/`ename`/`specificcat`，必填缺失清单为空 |
| ubits | ≥3 张可达图；`source_sel` 地区、`team_sel=Other` 必填；图床黑名单 | imgbox 会被直接拒，改用 pixhost；按 pieces + 文件清单核对（白名单仅日志） |
| xingyunge | 默认 | tracker 域名别名已补；按 qB 覆盖真值核对，不凭页面域名猜 |
| panda | 主标题禁中文；简介按六段顺序装配；≥4 张可达图；链接 + MediaInfo | 站规简介按【转载出处】【制作说明】【海报】【简介】【info】【截图】六段排列；2026-11-27 闭环跑通，141840 等审核，见 2.8 |
| iloli | `team_sel` 兜底 Other=5；地区下拉按标题/简介地区词配对 | announce 主机是独立域 `a.x-anime.cc`，不是站点同域；填了 `overrides.announceURL` 后该主机自动并入允许清单；AC3 有多个同名候选，引擎留空不猜，见 2.9 |
| muxuege | `team_sel` 兜底 Other=52；普通 AAC 取 6；年代档抽不到年份留空 | 源简介只剩 1 张豆瓣海报时，用抽帧脚本补 pixhost 截图满足门禁 |
| fenghuang | `team_sel` 只有 other=1；无地区、无音频码下拉 | 详情页编号从站点下载种子的评论字段取到 |

熊猫高清已恢复自动发种（2026-11-27 撤销暂停，见 2.8 第 1 条）；门禁仍是主标题禁中文、简介六段顺序、至少 4 张图。

#### 2.5.1 sbpt 的 `existed` 到底是谁发的（2026-11-25 核实）

这七条的详情页“由 … 发布”全部是本站账号 `Schalkiiii`，不是别人发的可辅种；`existed=1` 只说明站点按同 `info hash` 判重，不能单凭这个字段推断作者。逐条详情页：

- 21764 知否：https://sbpt.link/details.php?id=21764
- 21767 广告狂人：https://sbpt.link/details.php?id=21767
- 21768 A Touch of Zen：https://sbpt.link/details.php?id=21768
- 21780 绿里奇迹：https://sbpt.link/details.php?id=21780
- 21782 Aquarela：https://sbpt.link/details.php?id=21782
- 21783 我们的父辈：https://sbpt.link/details.php?id=21783
- 21784 The Intern：https://sbpt.link/details.php?id=21784

21764 当天早上先被旧的同 hash 判断记成 `same_hash_skipped`，后来按 qB 的 progress/tracker 复核为已在做种；其余条目经 `existed` 路径下载目标种、校验 pieces 后挂回 qB。准确说法是“都是自己账号的条目，这次按站点判重/补挂流程挂回”，不是“别人发的、我们捡来辅种”，也不能统一说成“以前发过但都没辅上”。

### 2.6 2026-11-26 站内信与回访结论

| 站点 | 种子 | 结论 | 说明 |
| --- | --- | --- | --- |
| tjupt（北洋园） | [652794](https://www.tjupt.org/details.php?id=652794) | 等站点审核（`pending_review`） | 站内信是“收到新评论”，不是审核结果。管理要求补海报、补 MediaInfo：已补 1 张 pixhost 海报 + 1 张 pixhost 截图，简介加了 `[mediainfo]` 段（站点渲染成“Release Info 发布信息”独立块），简介从 2691 字符变成 3475 字符，标题和分类 402 未变，页面回到“待审核”。2026-11-27 复核：海报已置顶、`[mediainfo]` 已置尾，页面仍标“待审核”；管理评论 `#315005` 仍在。中文名这次仍保持原样未改，等复审结论；`coverage.json` 的备注已同步为 11-27 现状。 |
| cangbao（藏宝阁） | [44189](https://cangbao.ge/details.php?id=44189) | 已解决（`reseeded`，已做种） | 审核通过，页面已无审核标记，回访判定干净，覆盖记录 `reseeded`。 |
| ubits（优堡） | [379934](https://ubits.club/details.php?id=379934) | 已解决（`reseeded`） | 候选被允许并审核通过，页面无审核标记，回访判定干净，覆盖记录 `reseeded`。 |
| qingwa（青蛙） | [208804](https://qingwapt.com/details.php?id=208804) | 已解决（`reseeded`） | 站内信里“标题片源拼写错误、要勾 Remux 标签”的拒绝是修复前的旧状态；当前标题已是 `BluRay`、Remux 标签已勾、页面无审核标记，回访判定干净，覆盖记录 `reseeded`，没有重复提交。同站 [208692](https://qingwapt.com/details.php?id=208692)、[208575](https://qingwapt.com/details.php?id=208575) 仍是“审核没过、等站点重审”，属于历史待跟进。 |
| agsvpt（末日） | [546963](https://www.agsvpt.com/details.php?id=546963) | 审核没过（`review_rejected`），等复审 | 今天被审核拒绝（方舟计划种子未选方舟标签、缺少海报或截图）。已修好：补勾方舟标签 52、简介从 11 张图（1 张豆瓣 + 6 张 pterclub + 4 张 pixhost）剪到 4 张 pixhost 活图（1 海报 + 3 截图），其他字段原样保留，只提交一次。页面仍挂着管理复审前的旧“审核不通过 / (禁止)”横幅（站点没有用户侧重提入口），要等管理重新看过才会消；不重发、不私信。 |

### 2.7 2026-11-26 站点可用性复核（未启用 / 暂停 / 动态界面 / 表单缺口）

这一轮把 147 个站逐个过了一遍：47 个没启用的站、13 个整站暂停的站、6 个动态上传界面的站、7 个表单有缺口的站。结论都是现场实测（浏览器带本机会话 / 只读 HTTP / `formcheck` / `gatecheck`），不是照抄旧结论。

**① 47 个未启用站：只有 1 个能启用**

| 结论 | 数量 | 站点 |
| --- | --- | --- |
| 可启用 | 1 | agsvpt（已启用，见第 3 节该行与 2.6） |
| 差登录 | 3 | azusa、tlf、generationfree（CookieCloud 里没有可用 cookie；generationfree 首页跳 `/bienvenue`、`upload.php` 404，登录后还要再确认上传路径） |
| 校外被拦 | 1 | byr（DNS 解析到 CERNET IPv6，首页和 `upload.php` 都回 403 wengine 网关「访问出错」鉴权页，本机无法适配） |
| 已关站 | 1 | yinghua（域名解析失败 `getaddrinfo 11001`） |
| 框架不支持 | 38 | Unit3D 13、custom 8、Gazelle 6、XBTIT 5、Blu 2、Haidan 1、YemaPT 1、Discuz 1、TNode 1 |
| 暂停复查（见②） | 3 | momentpt、movie52、ptlao |

这 38 个非 NexusPHP 站**不是开关问题**：代码里 `search_capable()` 只对 NexusPHP 为真，配置加载阶段就把它们过滤掉，把 `enabled` 改成 true 也不会进发种队列。要支持得新增对应框架的上传适配器，那是新能力，另立任务。

**② 整站暂停：13 个变 8 个**

保留 8 个，原因都不是"连不上"这么笼统：

| 站点 | 保留原因（实测） |
| --- | --- |
| hudbt、hdclone | 登录成立，但上传页都回「你没有发布种子的权限」，要走候选区或升等级 |
| freefarm | 旧域 `pt.0ff.cc` 和新域 `farmm.cc` 是同一站点的镜像；配置维持 `pt.0ff.cc`，不切 `farmm.cc`。两域都已登录（Schalkiii / UID 5481，LV1 User）；上传页主表单 POST `takeupload.php`，38 input + 18 select + 2 textarea，全页 62 控件 |
| u2 | 门户登录墙没建立会话，需要账号密码 + 两步验证/验证码 |
| shadow | 服务端判 cookie 失效（`error=cookie`），要重登 |
| sunny | 前端改版成单页应用，旧 `upload.php` 404；新上传路由 `/torrent-new` 表单完整但走 JSON 接口，当前版本没适配 |
| piggo | WAF 层 403，真实浏览器里根域跳 `/login`、`/upload` 跳登录，站点活着但会话失效 |
| tu88 | cookie 失效，登录页带图形验证码 |

摘掉 5 个：

- **gtk**：实测可恢复（登录态有效、上传页 37 字段、共享 tracker `t.myaltbox.com` 有 170 条可抄 passkey），已从暂停名单摘掉；2026-11-27 配了该站按站代理后链路可达，已临时加进 `autoReseedSites`，等站点发布窗口，原因见 2.9。
- **momentpt、movie52、ptlao、yinghua**：已确认关站（见第 5 节），不再占暂停名单；它们本来就是 `enabled:false`，留在未启用一栏即可。

**③ 6 个动态上传界面：真正"动态"的只有 3 个**

| 站点 | 实测形态 | 现在能不能自动发 |
| --- | --- | --- |
| hdcity | 两段式：首段只有种子文件框 + 5 个隐藏字段（`citeid`/`vf20261126`/`domainname`/`uid`/`name`），提交到 `upload_receiver.php?spm=<日令牌>`；标题/简介/MediaInfo 在上传后的二段编辑页填 | 首段可构造（5 字段），二段要真实上传后才能核对 |
| hddolby | 其实是标准静态表单（17 字段 / 9 下拉），只是会话带 2FA，失效时页面脚本跳 `take2fa.php` 中间页 | 可自动发；2FA 过期需浏览器过一次 |
| mypt | 区域/时段限制页（18:00–07:00 自行禁传）+ Cloudflare 挑战页，门后没有可读表单 | 当前时段不可判定，需允许时段用真实浏览器核对 |
| ptt | 已登录但账号/板块没有发布权限（新号先走 `offers.php` 候选区） | 不能，属权限问题不是表单问题 |
| rousi | React Router 单页应用（`ssr:false`），静态 HTML 无控件；API 在 `/api/v1`，建种接口在懒加载路由包里 | 纯 HTTP 不可行，需浏览器逆向接口 |
| ttg | 已登录但账号没有上传权限（页面写「您没有上传种子的权限」） | 不能，属权限问题 |

**④ 表单缺口与复核站：3 个配置落地、1 个类目不符、1 个单页应用、1 个过载、1 个源站故障**

| 站点 | 结论 |
| --- | --- |
| iloli | `team_sel` 配 Other=5 已落地；`source_sel` 是**地区**下拉，代码这轮补了通用地区匹配（国产→大陆 1 / 日剧→日本 6 / 无线索→其它 13），不再按站写死。2026-11-27《知否》被账号浓度门槛拦下（数量 ≥50%、体积 ≥40%），账号级站侧阻塞，见 2.9。 |
| muxuege | `team_sel` 配 Other=52、`audiocodec_sel` 取普通组首个 AAC=6 已落地；同名 AAC 消歧与年代档（`processing_sel`，2015=19）已进代码，抽不到年份就留空。2026-11-27 实发 59990，见 2.8。 |
| fenghuang | `team_sel` 下拉只有 other=1，已落地。2026-11-27 实发 39168，见 2.8。 |
| xingtan | 页面按分类渲染 328 个 `mode_<分类id>` 隐藏行，只有当前分类那行可见；隐藏行的空下拉不再误报缺口（实测 326 条不再算缺口）。两级分类是页面里点树切换的，静态 HTTP 驱动不了。2026-11-27 浏览器复核：无电影/剧集类目，影视无处归档，移出白名单；书籍/PDF 类目手动发成一条 456304，见 2.8、2.9。 |
| haoxue | Next.js 单页应用，静态页只服务端渲染分类一个下拉，其余控件和提交都在前端 JS 里，v1 静态表单发不了。2026-11-27 走站点接口真发 23943，见 2.8；不进自动名单。 |
| hdu | `/`、`/torrents.php`、`/upload.php`、`/login.php` 都回 200，但都是 253–264 字节自动刷新壳，原文「服务器负载过高，正在重试，请稍后...」，壳里没有真实表单；页面上那个 `[EASY-UPLOAD]` 按钮是本机 PT 助手浏览器扩展注入的，不是站点组件，不能当成动态上传界面的证据。2026-11-27 两次浏览器复核同一状态，移出白名单，见 2.9。 |
| aling | 首页、登录页、上传页浏览器侧都回 HTTP ERROR 500；`cfOrigin;dur` 241/238/234 ms、`cfEdge;dur` 3–8 ms，源站自己两百多毫秒就报错（不是边缘排队超时，也不是 WAF）。2026-11-27 两次复核后移出白名单，待站点恢复后复测，见 2.9。 |

**⑤ 熊猫（panda）闭环已跑通，撤销暂停**

- 2026-11-27 实发 141840《Liar Game S01 2007 2160p WEB-DL》：上传成功后立刻从站点下载种子、按分片和文件清单核对，挂回 qB 校验 100%、只挂熊猫一个 tracker；7 张图（1 海报 + 6 截图）逐张 200，简介六段齐全，当前 `pending_review` 等站点审核。详情页 https://pandapt.net/details.php?id=141840 ；qB hash `d60643d0b6eb0d1759ae0f6c318d72e3647a1ada`（站点改写后；本地构造 hash `2d562e251808618560714b9955986951cf407605`）。
- 这一条触发的新能力：发种前按站规 `descrOrder` 自动装配简介六段（只在配了该规则的站生效；源简介六段齐就逐字节不动）。历史上 141276 那版是发到站上之后手工重排的。
- 旧账：站上 141276 已删（详情页回「错误 没有该ID的种子」）；qB 里另有 1 条残留 `3f8ed774…` 挂失效 tracker，属待人工清理，不能算"在做种"。
- 熊猫已从 `reseedPausedSites` 移出并留在 `autoReseedSites`；站上那条 141276 是候选种子 24 小时没做种被系统删的，重发必须满足"上传→立刻挂回 qB 做种"。

### 2.8 2026-11-27 实发五条

熊猫、慕雪阁、凤凰、星潭四条都已把站点种子挂回 qB，校验 100%、只挂目标站一个 tracker；望雪待审期拿不到站点种子，当次没挂回。熊猫、慕雪阁、望雪仍是 `pending_review` 等审核；星潭进候选区待审核；凤凰已做种（`reseeded`）。

| 站点 | 详情页 | 内容 | 源站 · 体积 / 文件数 | qB hash | 图与简介 | 状态 |
| --- | --- | --- | --- | --- | --- | --- |
| 熊猫 panda | https://pandapt.net/details.php?id=141840 | 《Liar Game S01 2007 2160p WEB-DL》 | ubits 187730 · 40,063,028,807 字节 / 13 文件 | 站点改写后 `d60643d0b6eb0d1759ae0f6c318d72e3647a1ada`；本地构造 hash `2d562e251808618560714b9955986951cf407605` | 7 张图（1 海报 + 6 截图）逐张 200；简介六段齐全 | `pending_review`，页面角标 `title="未审"`，progress 1，单 tracker `tracker.pandapt.net`，已挂回 qB 100% |
| 慕雪阁 muxuege | https://pt.muxuege.org/details.php?id=59990 | 《知否知否》4K WEB-DL H265 60fps | soulvoice 54682 · 128,401,739,986 字节 / 73 文件 | `76692871e8c0ed233f0d7828e1f6d56a7566ae54` | 源简介只剩 1 张豆瓣海报，用抽帧脚本补 3 张 pixhost 截图后满足门禁 | `pending_review`，页面角标 `title="未审"`，progress 1，单 tracker `pt.muxuege.org`，已挂回 qB 100% |
| 凤凰 fenghuang | https://pt.521.best/details.php?id=39168 | 《广告狂人 Mad Men S01-S07》 | ptba 5565 · 95.10GB / 107 文件 | 前缀 `f8a8abe66511` | 简介 10 张图 | 已挂回 qB 做种（`reseeded`）；详情页编号从站点下载种子的评论字段取到 |
| 望雪 haoxue | https://www.hxpt.org/torrent/23943 | 《知否知否》 | — · 73 个 mp4 / 128401739986 字节 | 站点 info_hash `85edbcb1738078e8e9b71c7fe9728e3b53d6be5a`；pieces 指纹 `5804349c90ee57d3886f0c01d7b27f96bb6c9001`（与本地一致） | 5 张图（海报 1、九宫格 1、截图 3）HEAD/Range 全部 206 存活；MediaInfo 2323 字符；标签 国语/中字/完结 | `pending_review`，**当次没挂回 qB**，见下 |
| 星潭 xingtan | https://xingtan.one/details.php?id=456304 | 「初中英语八年级上册【电子课本】」 | 源 hxpt 原种原样保留 · 7,830,090 字节 / 1 文件 | 上传用 info hash `7e028f0e3eaf9daf5ee0ea8db4888ed17cf182cf`；pieces 指纹 `608379ab68ae0c441483c264a30f37934f57783f`；`info.source` 是 `[xingtan.one] 杏坛` | 归到 课程与教学论 / 书籍 / PDF，标签「确认」 | 候选区待审核（候选区保留约 1 天 23 小时）；已作为独立种子挂回 qB，progress 100%、只挂星潭一个 announce |

望雪这条是走站点接口真发的，不是引擎发的。回包 `code=0`、`approval_status=0`，页面标「审核中」；上午又查了两次仍是 `approval_status=0`、页面「审核中」，收件箱没有驳回留言。待审期间下载接口 `/api/v1/torrents/23943/download` 回 404 `{"code":1004,...,"message":"资源不存在"}`（已审的 23941 / 23928 同接口 200），所以站点种子拿不到、当次没挂回；审核通过后由 `bsend followup` 下载核对再挂回。望雪站规明令禁 FRDS 作品，引擎给该站的两条候选都是 MNHD-FRDS 压制，已排除。

星潭这条也是手动发的，走的是书籍/PDF 类目，不是影视。站点回传种子和上传种子在 announce、name、source、长度、pieces 指纹、文件清单、info hash 七项上完全一致。查重双证据：qB 里原种只挂 hxpt、没有星潭 announce；站内检索沪教8A、8A英语、英语课本 八年级、舒白梅、八年级 英语、沪教版 英语 等关键词均 0 条。覆盖账本：内容键 `m:a733ec64fdbb4fbd`（沪教8A英语课本.pdf，7,830,090 字节）的星潭格子落 `pending_review` + 上面这个详情页链接。

### 2.9 2026-11-27 复核结论：iloli、三个硬阻塞站、gtk、freefarm

**iloli（爱萝莉）**

- **tracker 真相**：announce 主机是独立域 `a.x-anime.cc`，不是站点同域；完整 announce 从站内两条种子（56406、56211）的 .torrent bencode 里取出，两条一致，74 字符；qB 里本来就有 163 条挂这个主机的种子（passkey 一致）。
- **由此暴露的 bug，已修**：引擎按站点域名匹配 tracker，会把这个站的已有做种认不出来，于是把两条本来 100% 在做种的内容（Poirot、周星驰合集）当成缺口；上传守卫又因为配置的 announce 主机不在允许清单里而拒绝上传，客观上挡住了重复上传。修法：配置里填了 `overrides.announceURL` 就自动把该主机并入允许清单（和页面 host、内置别名、手写 `announceHosts` 去重合并），识别已有做种和上传守卫两处消费同一份清单；没填 announceURL 的站行为不变。修完后这两条假缺口从草稿里消失。
- **修完后出现两条真缺口，拒因已查清**：《知否》两次提交都被站点拒绝、未建种、未挂回，`reseed-log.jsonl` 记 failed；第二次回包正文被留档抓到，错误框原文：`发布者二次元浓度不足，无法发布该类型资源。当前数量浓度 0.00%（须不低于 50%），体积浓度 0.00%（须不低于 40%）。` 站点上传页规则原文是：在种子区发布非二次元时，二次元条数须 ≥50%、体积须 ≥40%；二次元只算漫画/CG杂图/动漫杂志和动画（Animation），非二次元算电视剧、综艺、纪录片、电影，来源写日本（JPN）的不计入。这里的比例按账号的**发布记录**算，不是当前做种：这个号当前做种 163 条（其中动画 15 条、232.94GB），发布记录仍是 0 条，所以数量/体积都是 0.00%。这是账号级站侧阻塞，不是表单问题，也不是同剧重复（站上虽有同剧另一版本 31565 UBWEB 2160p，但回包没说重复）。回包留档 `artifacts\iloli-upload-1795736380253.html`。要在这站发非动画内容，得先从这个号发出二次元素材（动画/漫画/CG/动漫杂志），把发布记录里的二次元条数和体积养到门槛以上；是否这么做待定，见 ROADMAP 待办。《广告狂人》没提交：iloli 的 audiocodec_sel 里 AC3 有多个同名候选（`DD/AC3`、`DD/AC-3`、`AC3` 这类），引擎判歧义留空不猜，被 G2 质量档门禁拦住。这两条不是"站点不可用"。

**三个站点侧硬阻塞（都做过真实浏览器复核，不是静态解析失败就放弃）**

- **星潭 xingtan：影视确认不收，书籍/PDF 类目手动发成一条**。影视不收的证据：站内真实种子列表前 30 条全是教材/专著/文档，没有电影/剧集/纪录片；分类树里最接近的「戏剧影视」叶子点开只有类型、格式两个下拉，影视内容无处归档。站规原帖这次 404 打不开，不引用没复核过的站规原话。书籍类目实发一条 456304，见 2.8。
- **HDU hdu**：`/`、`/torrents.php`、`/upload.php`、`/login.php` 都回 200，但都是 253–264 字节的自动刷新壳，原文「服务器负载过高，正在重试，请稍后...」，壳里没有真实表单。页面上那个 `[EASY-UPLOAD]` 按钮是本机 PT 助手浏览器扩展注入的，不是站点组件，别把它当成动态上传界面的证据。
- **阿灵 aling**：首页/登录页/上传页浏览器侧都是 HTTP ERROR 500；只读抓响应头 `cfOrigin;dur` 分别 241 / 238 / 234 ms、`cfEdge;dur` 3–8 ms，源站两百多毫秒就报错，不是边缘排队超时，也不是 WAF。

**gtk**

- 两条候选都停在提交前，没有发布。《大侦探波洛合集》站内真重复：站上 7696 与本地 pieces 指纹 `c058abaab5de65186b26418911db9d77af1af93e`、155 文件、294,270,271,759 字节三项全一致，2024-05-18 匿名发布、8 人做种；本地 63 条同名副本都 100% 但没挂 `t.myaltbox.com`。不重发。
- 《知否知否》字段缺口已用配置补上：`team_sel` 落 Other=5，标签勾「国语」=5、「中字」=6；表单没有「完结」复选框，只能靠副标题写全 73 集；MediaInfo 只取到单集 E16，2851 字节。回放演练（草稿 17、未提交）字段数 13→15，简介 8663 字节、4 张图逐张 200，预检无阻断。
- 站点账号是 User 等级，站规写明 User 只能周六 12:00–周日 23:59 发布，本次周五早上不能发，等窗口。gtk 已临时加进 `autoReseedSites`（现 46 站），`overrides.proxy=http://127.0.0.1:7890` 与 `overrides.announceURL` 已配；走代理链路可达。站内已有 5 个其他版本《知否》（18185 UBWEB、13068 DBTV、9015 QHstudIo、8611 HHWEB、5230 OurTV，都 78 集），本条 73 集 119.58GB，存在审核判重风险，见 ROADMAP 待办。
- gtk 的 announce 主机是共享的 `t.myaltbox.com`（76 字符，qB 里 170 条同 passkey），站点页面不印完整 announce，要从 .torrent 或 qB 里抄。

**freefarm（农场）**

- 旧域 `pt.0ff.cc` 和新域 `farmm.cc` 是同一站点的镜像，配置维持 `pt.0ff.cc`，不切 `farmm.cc`。两域都已登录 Schalkiii / UID 5481（LV1 User）。上传页两域一致：主表单 POST `takeupload.php`，38 input + 18 select + 2 textarea；另两个搜索表单各 1 input + 1 select，全页 62 个控件；无权限拒绝字样，页面 tracker 提示 `tracker.0ff.cc/announce.php`。它仍在 `pausedSites` 里，本轮没解除。

## 3. 全部站点（147 个）

> 这张表由配置和 `formcheck.json` 生成，不是实时状态。`bsend sites --md` 和 `scripts/site-matrix.ps1` 都可以重新生成；手改只改“备注”列。

先说明表里的缩写和取值：

- **站点程序**：站点用的论坛/种子站程序，当前只有 NexusPHP 参与上传；
- **参与扫描**：`sourceSites[].enabled`，是否进入扫描范围；
- **整站暂停**：`pausedSites`，连辅种也停；
- **只停发种**：`reseedPausedSites`，辅种照常；
- **表单检查**：最近一次 `formcheck` 的结论，`ok` 表示能拿到表单，`fail` 表示拿不到，`gap` 表示表单有缺口，`未测` 表示没查过；
- **检查结论**：那次检查的具体原因；
- **pieces 反查**：`ok` 表示站点实现了 `POST /api/pieces-hash`，可用分片 hash 精确定位源种；`unsupported` 表示站点没有这个接口（404/405），只是退回名字检索；`未测` 表示还没验证。它不决定能否直接发种，挂回前仍会下载目标种核对 pieces 与 `info.name`；
- **Cookie 数**：本地保存的该站 Cookie 条数，不是登录状态。

| 站点 | 名称 | 域名 | 站点程序 | 参与扫描 | 整站暂停 | 只停发种 | 表单检查 | 检查结论 | pieces 反查 | Cookie 数 | 备注 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| btschool | 学校 | pt.btschool.club | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://pt.btschool.club/upload.php | unsupported | 6 |  |
| carpt | 车站 | carpt.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://carpt.net/upload.php | ok | 5 |  |
| chdbits | 彩虹岛 | ptchdbits.co | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://ptchdbits.co/upload.php | unsupported | 5 |  |
| cmct | 春天 | springsunday.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://springsunday.net/upload.php | unsupported | 1 |  |
| daxiangjiao | 大香蕉 | pt.daxiangjiao.org | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://pt.daxiangjiao.org/upload.php | ok | 1 |  |
| discfan | 蝶粉 | discfan.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「沒有發布種子的許可權」）：https://discfan.net/upload.php | ok | 2 |  |
| dstudio | Depth Studio | dstudio.me | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://dstudio.me/upload.php | 未测 | 1 |  |
| freefarm | 农场 | pt.0ff.cc | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 请求没通：https://pt.0ff.cc/upload.php: error sending request for url (https://pt.0ff.cc/upload.php): operation timed out | 未测 | 1 | 2026-11-27 复核：`pt.0ff.cc` 和 `farmm.cc` 是同一站点的镜像，配置维持 `pt.0ff.cc`，不切 `farmm.cc`。两域都已登录 Schalkiii / UID 5481（LV1 User）；上传页两域一致，主表单 POST `/takeupload.php`（38 input + 18 select + 2 textarea，另两个搜索表单各 1 input + 1 select，全页 62 控件），无权限拒绝字样，页面 tracker 提示 `tracker.0ff.cc/announce.php`。仍在 `pausedSites` |
| ggpt | GGPT | gamegamept.com | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://www.gamegamept.com/upload.php | ok | 1 |  |
| gtk | GTK | pt.gtkpw.xyz | NexusPHP | 是 | — | — | fail | 抓目标站上传表单失败: 请求没通：https://pt.gtkpw.xyz/upload.php: error sending request for url (https://pt.gtkpw.xyz/upload.php): operation timed out | 未测 | 2 | 2026-11-27：已配 `overrides.proxy=http://127.0.0.1:7890` 与 `overrides.announceURL`，走代理链路可达，已临时加进 `autoReseedSites`。两条候选都停在提交前：波洛合集站内真重复（7696，pieces `c058abaab5de65186b26418911db9d77af1af93e`、155 文件、294,270,271,759 字节，2024-05-18 匿名、8 人做种），不重发；知否字段已补齐（草稿 17 未提交，13→15 字段，简介 8663 字节、4 图 200，预检无阻断），但账号是 User 等级，站规限周六 12:00–周日 23:59 发布，周五早上不能发，等窗口；站内已有 5 个其他版本知否（78 集），本条 73 集 119.58GB，有审核判重风险。announce 是共享主机 `t.myaltbox.com`（76 字符，qB 里 170 条同 passkey），页面不印完整 announce，要从 .torrent 或 qB 抄。见 2.9 |
| hdarea | 高清视界 | hdarea.club | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://hdarea.club/upload.php | 未测 | 6 |  |
| hdclone | 独自 | pt.hdclone.top | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 请求没通：https://pt.hdclone.top/upload.php: error sending request for url (https://pt.hdclone.top/upload.php): operation timed out | 未测 | 5 |  |
| hddolby | 杜比 | uhddolby.com | NexusPHP | 是 | — | 是 | ok | team_sel 未选 | unsupported | 5 |  |
| hdhome | 家园 | hdhome.org | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://hdhome.org/upload.php | 未测 | 6 |  |
| hdsky | 天空 | hdsky.me | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://hdsky.me/upload.php | unsupported | 6 |  |
| hhanclub | 憨憨 | hhanclub.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布」）：https://hhanclub.net/upload.php | unsupported | 5 |  |
| hitpt | 百川 | hitpt.com | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://www.hitpt.com/upload.php | ok | 1 |  |
| hudbt | 蝴蝶 | hudbt.hust.edu.cn | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 请求没通：https://hudbt.hust.edu.cn/upload.php: error sending request for url (https://hudbt.hust.edu.cn/upload.php): client error (Connect): tcp connect error: 由于连接方在一段时间后没有正确答复或连接的主机没有反应，连接尝试失败。 (os error 10060) | 未测 | 5 |  |
| kamept | 龟站 | kamept.com | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://kamept.com/upload.php | 未测 | 2 |  |
| keepfrds | 朋友 | pt.keepfrds.com | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://pt.keepfrds.com/upload.php | unsupported | 6 |  |
| march | 三月 | duckboobee.org | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://duckboobee.org/upload.php | ok | 1 |  |
| momentpt | 瞬间 | momentpt.top | NexusPHP | — | — | — | fail | 目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：https://www.momentpt.top/upload.php | 未测 | 2 | 已关站（savept.icu 2026-10-29）：站点已放告别页「Moment · 山水有相逢」 |
| movie52 | 52MOVIE | 52movie.top | NexusPHP | — | — | — | fail | 目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：https://www.52movie.top/upload.php | 未测 | 6 | 已关站（savept.icu 2026-10-20）：域名 52movie.top 已无法解析 |
| mypt | 我的PT | pt.mypt.cc | NexusPHP | 是 | — | 是 | fail | [challenge] 站点被人机验证/区域时段拦截：请在前台真实浏览器打开 https://pt.mypt.cc/upload.php 人工过一次验证（标签页必须在前台，不能最小化或切到后台）；纯 HTTP 复现不了验证结果，当前不自动发种。 | unsupported | 0 |  |
| njtupt | 蒲园 | njtupt.top | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://njtupt.top/upload.php | ok | 5 |  |
| okpt | OK | okpt.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://www.okpt.net/upload.php | ok | 5 |  |
| piggo | 猪猪 | piggo.me | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 站点回 HTTP 403（多半是 cookie 失效）：https://piggo.me/upload.php | 未测 | 1 |  |
| playlet | PlayLet | playlet.cc | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://playlet.cc/upload.php | ok | 1 |  |
| pt52 | 52PT | 52pt.site | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://52pt.site/upload.php | 未测 | 5 |  |
| pter | 猫 | pterclub.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://pterclub.net/upload.php | unsupported | 7 |  |
| ptlao | 忘年桥 | ptlao.top | NexusPHP | — | — | — | fail | 抓目标站上传表单失败: 请求没通：https://ptlao.top/upload.php: error sending request for url (https://ptlao.top/upload.php): operation timed out | 未测 | 1 | 已关站（savept.icu 2026-10-21）：原 PT 站已停，域名现为「忘年桥资源站」，upload.php 404 |
| ptskit | 拾刻 | ptskit.org | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://www.ptskit.org/upload.php | ok | 2 |  |
| ptt | 时间 | pttime.org | NexusPHP | 是 | — | 是 | fail | [no-permission] 该账号在 ptt 无发布权限（页面提示含「在该板块发布种子的权限」）；本站条目自动发种中止，待人工确认权限后再处理：https://www.pttime.org/upload.php | 未测 | 1 |  |
| rousi | 肉丝 | rousi.pro | NexusPHP | 是 | — | 是 | fail | [spa] 站点上传界面是纯前端 SPA/JSON API（https://rousi.pro/upload.php）：表单控件由网页 JS 渲染，纯 HTTP 拿不到字段名与提交接口；自动发种暂不支持。缺：上传表单控件定义、提交接口地址、浏览器会话鉴权。 | unsupported | 1 |  |
| shadow | 影 | star-space.net | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: HTTP 404 抓页面 https://star-space.net/upload.php：File not found. | 未测 | 1 |  |
| siqi | 思齐 | si-qi.xyz | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://si-qi.xyz/upload.php | ok | 1 |  |
| sunny | 阳光 | sunnypt.top | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: HTTP 404 抓页面 https://sunnypt.top/upload.php：<!DOCTYPE html><html class="scrollbar-none" lang="zh-CN"><head><meta charSet="utf-8"/><meta name="viewport" content="wid | 未测 | 2 |  |
| tccf | TCCF | et8.org | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://et8.org/upload.php | unsupported | 5 |  |
| tokyo | Tokyo | tokyo-manga.top | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://www.tokyo-manga.top/upload.php | 未测 | 1 |  |
| ttg | 套 | totheglory.im | NexusPHP | 是 | — | 是 | fail | [no-permission] 该账号在 ttg 无发布权限（页面提示含「没有上传种子的权限」）；本站条目自动发种中止，待人工确认权限后再处理：https://totheglory.im/upload.php | unsupported | 6 |  |
| tu88 | TU88 | pt.tu88.men | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 被跳到登录页/首页（cookie 可能已失效，或站内有待确认的公告/考核）：https://pt.tu88.men/upload.php | 未测 | 1 |  |
| u2 | 幼儿圈 | u2.dmhy.org | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 请求没通：http://u2.dmhy.org/upload.php: error sending request for url (http://u2.dmhy.org/upload.php): client error (Connect): tcp connect error: 由于连接方在一段时间后没有正确答复或连接的主机没有反应，连接尝试失败。 (os error 10060) | 未测 | 2 |  |
| ultrahd | UltraHD | ultrahd.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://ultrahd.net/upload.php | ok | 1 |  |
| wtsakura | 冬樱 | wintersakura.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://wintersakura.net/upload.php | ok | 2 |  |
| xingwan | 星湾 | xingwan.cc | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://xingwan.cc/upload.php | ok | 1 |  |
| zmpt | 织梦 | zmpt.cc | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://zmpt.cc/upload.php | ok | 2 |  |
| hdcity | 城市 | hdcity.city | NexusPHP | 是 | — | 是 | gap | 简介为空 | unsupported | 5 |  |
| musopia | 音乐乌托邦 | musopia.vip | NexusPHP | 是 | — | — | gap | medium_sel 未选 | 未测 | 1 |  |
| muxuege | 慕雪阁 | pt.muxuege.org | NexusPHP | 是 | — | — | gap | audiocodec_sel 未选 | ok | 1 | 2026-11-27 实发 59990（《知否》4K WEB-DL H265 60fps，源 soulvoice 54682，128,401,739,986 字节 / 73 文件），源简介只剩 1 张豆瓣海报，抽帧补 3 张 pixhost 截图后过门禁；qB hash `76692871e8c0ed233f0d7828e1f6d56a7566ae54`，progress 1、单 tracker `pt.muxuege.org`，校验 100%，`pending_review`，见 2.8 |
| ptfans | PTFans | ptfans.cc | NexusPHP | 是 | — | — | gap | medium_sel 未选 | ok | 1 | 2026-11-23 已实发 `11111`，`fdf27b29…`，补挂 qB 校验 100%；站上改写 hash，已入白名单；站上已有 720p 其他版本，按策略继续发；表单无“完结”标签，人工补。2026-11-26：Coraline 被归到 H 漫/动漫区、源详情页超时；配置 note 仍写 `publishEnabled false`，“可发 11111”只是 11-23 单次实测 |
| agsvpt | 末日 | agsvpt.com | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | 未测 | 1 | 2026-11-26 启用：`sourceSites` 里 `enabled:true` 且已加进 `autoReseedSites`（只改 JSON 开关不会进队列，两件事要一起做）。浏览器带本机会话实测上传页 200、登录身份 Schalkiii、表单完整无权限拒绝字样；`gatecheck` 全绿（17 字段 / 9 下拉 / 7 项检查全过）。passkey 不用手填：qB 里 806 条该站 tracker 种子全部 100%，可抄 announce（tracker 主机是 `tracker.agsvpt.cn`，浏览是 `www.agsvpt.com`）。`team_sel[4]` 值表：BeiTai=30、AGSVPT=6、AGSVWEB=21、AGSVMUS=20、GodDramas=23、RL=31、DYZ=28、Hares=29、CatEDU=24、Pack=16、Other=22；配不上驻站组时落 Other=22（`selectPicks` 只在字段为空时兜底，不会覆盖已匹配的组）。[546963](https://www.agsvpt.com/details.php?id=546963) 仍等复审，见 2.6。「方舟计划」不是每条都该勾 52，入选条件还没摸清，没写进必勾标签 |
| aling | 爱玲 | pt.aling.de | NexusPHP | 是 | — | — | fail | 抓目标站上传表单失败: HTTP 500 抓页面 https://pt.aling.de/upload.php： | 未测 | 1 | 2026-11-27 两次真实浏览器复核：首页/登录页/上传页全 HTTP ERROR 500；`cfOrigin;dur` 241/238/234 ms、`cfEdge;dur` 3–8 ms，源站两百多毫秒就报错，不是边缘排队超时也不是 WAF；已移出白名单，待站点恢复后复测，见 2.9 |
| audiences | 观众 | audiences.me | NexusPHP | 是 | — | — | ok |  | unsupported | 6 | 2026-11-23 判重检索被人机验证拦住，按“未确认重复不上传”中止，未发布；无头 HTTP 复用不了浏览器会话。2026-11-24 晚 Kimi WebBridge 可行性实验通过：真实 Chrome 已登录 `Schalkiii`，页面内 `fetch('/torrents.php?search=…')` 返回 200、命中 2 个详情页、无验证码；只做只读检索，尚未接进程序，暂仍按只人工发处理（见 2.3 末尾）。**2026-11-26 复核：除 Turnstile 和后台标签页问题外，上传表单结构本身也是缺口——自定义 JS 表单、`type` 为隐藏值 0，当前静态解析交不出可提交字段，此前文档没记这条** |
| baozi | 包子 | p.t-baozi.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| cangbao | 藏宝阁 | cangbao.ge | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 3 | 43994 图片死链已修。两次核对说法不一致，分别记录：2026-11-24 回访时详情页审核图标标 `title="通过"`；2026-11-26 复核页面仍标“未审”，qB 已 100% 做种。当前以 11-26 页面标记为准，11-24 的“通过”不作定论，待下次回访再确认。另：[44189](https://cangbao.ge/details.php?id=44189) 是另一条，11-26 回访判定干净、`reseeded` 已做种，见 2.6 |
| city13 | 13City | 13city.org | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 1 | 2026-11-26：2 条上传仍回 HTTP 200 未识别回包；11-27 起不再直接判失败，改按名称 + 总大小核对，核对不到落 `posted_unresolved`；真实下拉 `medium_sel[4]=3` 是 Remux，草稿选档已覆盖 |
| crabpt | 蟹黄堡 | crabpt.vip | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 已实发 240242，`732e…`；此前候选源站 hdfans 502，被失败冷却挡住，改选 hdsky 源后成功；站点改写 hash，已加入白名单，挂回校验 100%；2026-11-24 晚《伪装者》简介 9 张 `i13.tietuku.com` 死图已换 imgbox（复查 0 旧图 / 9 新图） |
| cspt | 财神 | cspt.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 | 2026-11-23 已实发 259870，`cf56…`；tracker 在 `tracker.cspt.cc`，已补别名；站点改写 hash，挂回校验 100%。**2026-11-26 复核：当天 4 条审计都是“该账号无发布权限”，发布权限疑似已被收回**；259870 的“可发”是 11-23 旧结论，能否再发待权限确认 |
| cyanbug | 大青虫 | cyanbug.net | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 |  |
| dicmusic | 海豚 | dicmusic.com | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 3 |  |
| dragonhd | 龙之家 | dragonhd.xyz | NexusPHP | 是 | — | — | ok | team_sel 未选 | unsupported | 5 |  |
| dubhe | 天枢 | dubhe.site | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 已实发 55492，`b5eb…`；判重搜索偶发超时；站点改写 hash，挂回校验 100% |
| fenghuang | 凤凰 | pt.521.best | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-27 实发 39168（《广告狂人 Mad Men S01-S07》，源 ptba 5565，95.10GB / 107 文件），简介 10 张图；qB hash 前缀 `f8a8abe66511`，校验 100%；详情页编号从站点下载种子的评论字段取到，见 2.8 |
| haitang | 海棠 | htpt.cc | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 7 |  |
| haoxue | 好学 | hxpt.org | NexusPHP | 是 | — | — | ok | category_id 未选 | unsupported | 4 | 单页应用，引擎静态表单走不了，不进 `autoReseedSites`。2026-11-27 走站点接口真发 23943（《知否》），pieces 指纹与本地一致；上午两次复查仍 `approval_status=0`、页面「审核中」，收件箱无驳回留言，5 张图（海报 1、九宫格 1、截图 3）HEAD/Range 全 206 存活；待审期下载接口回 404 不给下载，当次没挂回 qB，审核通过后由回访挂回；站规禁 FRDS，两条 MNHD-FRDS 候选已排除，见 2.8 |
| hdbao | 海德堡 | hdbao.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 | 2026-11-23 已实发 54402/54403，`95ea…`/`51ed…`；站上改写 hash，已加入白名单，两条均补挂校验 100%；`autopost --limit 1` 曾连发两条，已修 limit 计数 |
| hdfans | 红豆饭 | hdfans.org | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 5 | 2026-11-23 发《伪装者》时回包带 `existed=1`（指向 287791）；2026-11-24 核对 287791 就是同一条，qB 里 `b1e600a6…` 校验 100%，覆盖记录 `reseeded`，未重发；2026-11-24 晚该条简介 9 张 `i13.tietuku.com` 死图已换 imgbox（复查 0 旧图 / 9 新图）；2026-11-25 核对 Liar Game `287874`：站点种同 `info hash`，qB progress=1、只挂 `hdfans.org`，记 `seeding`；当源站时常见 502 / 超时。2026-11-26：A Touch 被 `medium_sel` 未选拦下。2026-11-27：真实下拉 `medium_sel[4]=23` 是 BD Remux，草稿 78 已落 23 |
| hdtime | 时光 | hdtime.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 6 |  |
| hdu | 好多油 | pt.upxin.net | NexusPHP | 是 | — | — | fail | 目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：https://pt.upxin.net/upload.php | unsupported | 5 | 2026-11-27 两次真实浏览器复核：`/`、`/torrents.php`、`/upload.php`、`/login.php` 都回 200，但都是 253–264 字节自动刷新壳，原文「服务器负载过高，正在重试，请稍后...」，壳里没有真实表单；页面上 `[EASY-UPLOAD]` 按钮是本机 PT 助手浏览器扩展注入，不是站点组件。已移出白名单，站点恢复后复测，见 2.9 |
| hdvideo | HDVideo | hdvideo.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 检查结论是上一次表单结果；`region_sel[4]`、`style_sel[4][]` 已补，78856 已补挂 qB |
| iloli | 爱萝莉 | mua.xloli.cc | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | 未测 | 1 | announce 主机是独立域 `a.x-anime.cc`（不是站点同域，74 字符，qB 里 163 条同 passkey）；填 `overrides.announceURL` 后该主机自动并入允许清单。2026-11-27：《知否》两次提交都被拒、未建种；回包抓到真实拒因——账号二次元浓度不足（数量浓度 0.00% 须 ≥50%，体积浓度 0.00% 须 ≥40%），账号级站侧阻塞，不是表单问题也不是同剧重复；回包留档 `artifacts\iloli-upload-1795736380253.html`。《广告狂人》AC3 多个同名候选被 G2 拦、未提交。见 2.9 |
| itzmx | Itz | pt.itzmx.com | NexusPHP | 是 | — | — | ok | team_sel 未选 | 未测 | 5 | 2026-11-23 首次实发 `13134`（`88e3103b…`）并补挂校验 100%；站点 torrent 文件上限 `266,240 bytes`，大合集/大剧包会被拒，改用小种《Liar Game》通过；站上改写 hash，已加入白名单。2026-11-26：Coraline 又出现 403、War 通用上传失败；403 是本轮新现象（早期 403 曾归为超大请求体前置拦截），根因待查 |
| kelu | Kelu | our.kelu.one | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 1 |  |
| kufei | 库非 | kufei.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-25 核对《人民的名义》`142202`：站点种同 `info hash`，qB progress=1、只挂 kufei tracker，按 `bsend pull` 复核记 `seeding`；2026-11-27 真实下拉 `medium_sel[4]=6` 是 BD Remux，草稿 79 已落 6 |
| lajidui | 垃圾堆 | pt.lajidui.top | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 1 | 2026-11-26 程序已自动填 MP4/MKV、大陆/日韩/欧美、制作组或 Other；表单检查显示“未选”是检查工具只读原表单，不是发种时留空。**候选范围失守：85084、85085 是安全集（允许转载内容范围）之外误发**，这条此前没记；后续选种要先核对内容是否在安全集内，再谈下拉和图片 |
| lemonhd | 柠檬不甜 | lemonhd.net | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 首次实发 48970（`3c00cbb2…`）并补挂校验 100%；站上改写 hash，已加入白名单；savept.icu 同名 LemonHD（lemonhd.club）标注已关站 2026-03-13，本配置域名 lemonhd.net 实测存活 |
| longpt | 龙 | longpt.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 3 | 2026-11-26：4 条上传表单 action 仍落 `bonusrain.php` 后中止。2026-11-27 复核：`upload.php` 只有 `bonusrain.php` 捐款表单（hidden + number，0 个 file input），`takeupload.php` GET 只回“上传失败！”页；程序按 `no-form` 中止，不提交到捐款接口，真实入口仍未定位，51065 的“可发”是 11-22 旧结论 |
| luckpt | 幸运 | pt.luckpt.de | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 4 | 2026-11-24 晚《伪装者》`61102` 简介 9 张 `i13.tietuku.com` 死图已换 imgbox（复查 0 旧图 / 9 新图），回包 `edited=1&audit_pending=1`；2026-11-25 新发 A Touch of Zen `61704`、绿里奇迹 `61705`、The Intern `61706`、Black `61707`，均已审核结束、qB progress=1、只挂 luckpt tracker |
| nanyang | 南洋 | nanyangpt.com | NexusPHP | 是 | — | — | ok | [nanyang] 表单里没找到「中字/完结/国语」类标签复选框，请发布后手动补 | unsupported | 6 | 2026-11-24 首次实发 `226159`（《知否》，源 soulvoice）；站上 hash 与构造不同（构造 `187d1487…`，站上 `8a9b9ab2…`），已加入 hash 改写白名单，把站点种子下回来挂进 qB，校验 100%、单站 tracker（见 2.3） |
| nicept | 老师 | nicept.net | NexusPHP | 是 | — | — | ok | type 未选 | ok | 1 |  |
| novahd | nova | pt.novahd.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 |  |
| oshen | 奥申 | oshen.win | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 已实发 `57082`，`22e3dd63…`，补挂 qB 校验 100%；站上改写 hash，已入白名单；表单没有“完结”标签，人工补 |
| panda | 熊猫 | pandapt.net | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | 未测 | 5 | 2026-11-27 闭环跑通：141840《Liar Game S01 2007 2160p WEB-DL》上传后立刻挂回 qB 校验 100%、只挂熊猫一个 tracker，7 图逐张 200、简介六段齐全，`pending_review` 等审核（见 2.8、2.7⑤）。已撤销发种暂停，留在 `autoReseedSites`。历史 141276 是候选种子 24 小时没做种被系统删的；qB 里残留 `3f8ed774…` 挂失效 tracker，待人工清理 |
| ptba | 1PT | 1ptba.com | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 2 | 2026-11-23 已实发 106718，`abf4…`；站点改写 `info.source`，已加入 hash 改写白名单，挂回校验 100% |
| ptcafe | 咖啡 | ptcafe.club | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | ok | 1 | 2026-11-25 核对《人民的名义》`137241`：站点种 `info hash` 与 qB 原种相同，qB progress=1、只挂 `tracker.ptcafe.club`，按 `bsend pull` 复核记 `seeding`；不是没挂上 |
| pthome | 铂金家 | pthome.net | NexusPHP | 是 | — | — | ok | team_sel 未选 | unsupported | 6 | 2026-11-24 首次实发 `284701`（《人民的名义》，源 hdfans）；站上 hash 与构造不同（构造 `b31d8eaf…`，站上 `ebaa6dd9…`），已加入白名单，补挂时确认 qB 里 11-23 23:56 就在做种、校验 100%（见 2.3）。2026-11-27 已补“该种子已存在！”/“已存在！”精确识别，真实回包未复测 |
| ptlgs | 劳改所 | ptlgs.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | 未测 | 6 |  |
| ptsbao | 烧包 | ptsbao.club | NexusPHP | 是 | — | — | ok | processing_sel[1] 未选 | ok | 1 | 2026-11-23 已实发 135787，`0b0087…`；`source_sel[1]` 二级分类已自动填 `剧集-大陆`；站点改写 hash，已加入白名单，挂回校验 100%；表单无“完结”标签需人工补；2026-11-27 草稿 82 已落 `medium_sel[1]=1`（BluRay）+ processing=Remux + source=电影-Remux |
| ptzone | 葡萄汁 | ptzone.xyz | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-26：详情页访问超时、`audiocodec_sel` 未选被门禁拦下；2026-11-27 已加检索/详情页超时重试，单测过，真站复测未做；真实下拉 `medium_sel[4]=3` 是 Remux，选档已覆盖；早期 190916 的“五项检查全过”是 11-22 实测，选档缺口此前没记 |
| qilin | 麒麟 | hdkyl.in | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 8 | 479362 已补挂 qB；站点审核不通过（缺图），不影响做种；2026-11-24 横幅仍写“审核不通过，原因：缺少海报或截图（图片少于 2 张）”，覆盖记录落 `review_rejected`。旧简介 7 张图全不可达（豆瓣海报防盗链 418、`img.pterclub.com` TLS 失败），已全部换成 imgbox 图（1 海报 + 6 截图）并提交，等复审；门禁 `剪死图+至少3张`。2026-11-26：480763 首次挂回被“非目标站 tracker”拒绝、落“已发但未挂回”，原因是 tracker 别名识别缺口——页面域 `www.hdkyl.in`，站点种子 tracker 在 `www.hdkylin.top`；补别名后才挂回（详见 `SITE-DEBUG.md` 6.16） |
| qingwa | 青蛙 | qingwapt.com | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 | announce 别名 `tracker.qingwapt.org` 已补；2026-11-24 Poirot 合集被站点判 `torrent_too_large`（要求改走 `offers.php` 候选），程序记 `pending_offers`（待走候选入口），不算失败、不自动重试，`status` 单列待跟进；同站《伪装者》发 `208575` 并挂回，校验 100%（构造 `c78e9cb2…`，站上 `d1eeec5c…`），当晚 9 张 `i13.tietuku.com` 死图已换 imgbox（复查 0 旧图 / 9 新图）；《知否》回包“已存在！”且 qB 已在做种（见 2.3）。2026-11-26：[208804](https://qingwapt.com/details.php?id=208804) 复核标题已是 `BluRay`、Remux 标签已勾、页面无审核标记，回访干净、`reseeded`，没有重复提交；[208692](https://qingwapt.com/details.php?id=208692)、[208575](https://qingwapt.com/details.php?id=208575) 仍审核没过、等站点重审，是历史待跟进；2026-11-27 已补“已存在！”精确识别，真实回包未复测 |
| railgun | Railgun | bilibili.download | NexusPHP | 是 | — | — | ok |  | ok | 2 | 2026-11-23 首次实发 60632（`72889498…`）并补挂校验 100%；站上改写 hash，已加入白名单 |
| sbpt | SBPT | sbpt.link | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 | 2026-11-25 核对：广告狂人 `21767`、A Touch of Zen `21768`、知否 `21764` 已做种；绿里奇迹 `21780`、Aquarela `21782`、我们的父辈 `21783`、The Intern `21784` 已校验 100%；详情页作者均为本站账号，不是别人发的可辅种（见 2.5.1）；Aquarela、The Intern 缺“中字”标签需手动补 |
| shuioudao | 下水道 | sewerpt.com | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 |  |
| soulvoice | 聆音 | pt.soulvoice.club | NexusPHP | 是 | — | — | ok | type 未选 | ok | 1 |  |
| tangpt | 躺平 | tangpt.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| tey | 太乙 | pt.tey.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 已实发 `623`，`c9570904…`，补挂 qB 校验 100%；`medium_sel` 仅 WEB-DL/HDTV/DVDR/CD/Track，蓝光源会被门禁拦；qB 老种走 `tracker.tey.cc`，已补别名；站上改写 hash，已入白名单；新种状态“未审”（2026-11-24 回访仍未审，覆盖记录落 `pending_review`）。同一条简介曾带 9 张失效的 `i13.tietuku.com` 图，已换 imgbox 图并提交；门禁 `剪死图+至少3张` |
| tjupt | 北洋园 | tjupt.org | NexusPHP | 是 | — | — | ok | [tjupt] 表单里没找到「中字/完结/国语」类标签复选框，请发布后手动补 | unsupported | 1 | 剧集分类的中文名 `cname`、英文名 `ename`、剧集类型 `specificcat`，还有地区复选框，来自 `showcategorydetail.php?catid=402`，静态 HTML 里不存在。历史上由写死的 tjupt 特判填这三个字段；本轮改成规则驱动（`CategoryRule`，配置 `categoryRules` 可覆盖、缺省走内置兜底表），提交前拉分类片段按真实控件名核对，控件不在记缺口、不臆造字段；派生不出中/英文名时记本地缺口、真提交前硬拦点名缺哪个；旧特判和旧单测已删除，没有两份重复实现。2026-11-26 只读演练（`gatecheck --sites tjupt`，只 GET 上传页和分类片段、未 POST）载荷带 `cname=无台标完整版 全41集1080P`、`ename=The.Disguiser.Complete.1080p.HDTV.x264.AAC-JING`、`specificcat=其他`，必填缺失清单为空；当时唯二阻断项都是图片底线（简介 0 张可用图、站点要求 1 张），与分类字段无关。announce 以站点下载的种子定案为 `tracker-public.tjupt.org`（已写进 `overrides.announceURL`）。652794 已于 11-26 补 1 海报 + 1 截图和 `[mediainfo]` 段，重新进待审，见 2.6；同日 274GB《波洛全季》合集上传 5 分钟仍回 HTTP 200 空正文、站上无条目，重试仍会空回包，要发得先拆小或换内容 |
| ubits | 优堡 | ubits.club | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | ok | 8 | `source_sel[4]`（地区）和 `team_sel[4]`（`Other`）已能自动填，上面的检查结论是旧结果；2026-11-24 首次实发 `379266`（《伪装者》，源 hdsky），站上 hash 与构造不同（构造 `d9970bce…`，站上 `2a57b787…`），已加入白名单，把站点种子下回来挂进 qB，校验 100%、单站 tracker；该条简介 9 张 `i13.tietuku.com` 死图已换 imgbox，页面仍标不通过，等复审（见 2.3）。2026-11-26：[379934](https://ubits.club/details.php?id=379934) 候选被允许并审核通过，页面无审核标记，回访判定干净，覆盖记录 `reseeded`，见 2.6 |
| vclib | VC-Lib | pt.vclib.online | NexusPHP | 是 | — | — | ok | type 未选 | ok | 2 |  |
| xdy | 修道院 | xdypt.vip | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-25 核对知否 `8043`：站点种与 qB 原种同 `info hash`，qB progress=1、只挂 xdy tracker，按 `bsend pull` 复核记 `seeding` |
| xingtan | 杏坛 | xingtan.one | NexusPHP | 是 | — | — | ok | source_sel[16] 未选 | 未测 | 5 | 2026-11-27 浏览器复核：学术特色站，站内列表前 30 条全是教材/专著/文档，无电影/剧集/纪录片；最接近的「戏剧影视」94/192 是教材方向，实点开只有类型和格式两个下拉，影视无处归档，已移出白名单。书籍/PDF 类目手动发成一条 456304（候选区待审核），见 2.8、2.9 |
| xingyunge | 星陨阁 | pt.xingyungept.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| ziran | 自然 | naturept.top | NexusPHP | 是 | — | — | ok | type 未选 | ok | 2 |  |
| aither | Aither | aither.cc | custom | — | — | — | 未测 |  | 未测 | 0 |  |
| alpharatio | AlphaRatio | alpharatio.cc | Gazelle | — | — | — | 未测 |  | 未测 | 0 |  |
| animez | AnimeZ | animez.to | Gazelle | — | — | — | 未测 |  | 未测 | 0 |  |
| anthelion | Anthelion | anthelion.me | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| azusa | 梓喵 | azusa.wiki | NexusPHP | — | — | — | 未测 |  | 未测 | 0 |  |
| beyondhd | BeyondHD | beyond-hd.me | XBTIT | — | — | — | 未测 |  | 未测 | 4 |  |
| bitporn | BitPorn | bitporn.eu | custom | — | — | — | 未测 |  | 未测 | 1 |  |
| blutopia | Blutopia | blutopia.cc | Blu | — | — | — | 未测 |  | 未测 | 3 |  |
| brokenstones | BrokenStones | brokenstones.is | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| byr | 北邮 | byr.pt | NexusPHP | — | — | — | 未测 |  | 未测 | 0 |  |
| clearjav | ClearJAV | clearjav.com | XBTIT | — | — | — | 未测 |  | 未测 | 1 |  |
| empornium | 峨眉派 | empornium.sx | Unit3D | — | — | — | 未测 |  | 未测 | 1 |  |
| exoticaz | ExoticaZ | exoticaz.to | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| fappaizuri | Fappaizuri | fappaizuri.me | XBTIT | — | — | — | 未测 |  | 未测 | 0 |  |
| filelist | FileList | filelist.io | Unit3D | — | — | — | 未测 |  | 未测 | 1 |  |
| generationfree | Generation-Free | generation-free.org | NexusPHP | — | — | — | 未测 |  | 未测 | 0 |  |
| gpw | 海豹 | greatposterwall.com | custom | — | — | — | 未测 |  | 未测 | 3 |  |
| haidan | 海胆 | haidan.cc | Haidan | — | — | — | 未测 |  | 未测 | 5 |  |
| happyfappy | HappyFappy | happyfappy.net | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| hdspace | HD-Space | hd-space.org | Gazelle | — | — | — | 未测 |  | 未测 | 5 |  |
| hdtorrents | HD-Torrents | hd-torrents.org | XBTIT | — | — | — | 未测 |  | 未测 | 0 |  |
| huno | HUNO | hawke.uno | XBTIT | — | — | — | 未测 |  | 未测 | 5 |  |
| iptorrents | IPT | iptorrents.com | Gazelle | — | — | — | 未测 |  | 未测 | 0 |  |
| jpopsuki | Jpopsuki | jpopsuki.eu | Unit3D | — | — | — | 未测 |  | 未测 | 2 |  |
| lst | LST | lst.gg | custom | — | — | — | 未测 |  | 未测 | 0 |  |
| milkie | 奶昔 | milkie.cc | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| monika | 莫妮卡 | monikadesign.uk | Blu | — | — | — | 未测 |  | 未测 | 3 |  |
| mteam | 馒头 | kp.m-team.cc | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| myanonamouse | MyAnonamouse | myanonamouse.net | custom | — | — | — | 未测 |  | 未测 | 0 |  |
| nebulance | Nebulance | nebulance.io | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| oldtoons | Oldtoons | oldtoons.world | Unit3D | — | — | — | 未测 |  | 未测 | 1 |  |
| opencd | 皇后 | open.cd | Gazelle | — | — | — | 未测 |  | 未测 | 7 |  |
| orpheus | Orpheus | orpheus.network | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| ourbits | 我堡 | ourbits.club | custom | — | — | — | 未测 |  | 未测 | 1 |  |
| ptneko | 超科学PT喵 | ptneko.com | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| sjtu | 葡萄 | pt.sjtu.edu.cn | custom | — | — | — | 未测 |  | 未测 | 0 |  |
| sportscult | SportsCult | sportscult.org | Gazelle | — | — | — | 未测 |  | 未测 | 3 |  |
| tlf | 吐鲁番 | pt.eastgame.org | NexusPHP | — | — | — | 未测 |  | 未测 | 0 |  |
| torrentleech | TorrentLeech | torrentleech.cc | custom | — | — | — | 未测 |  | 未测 | 0 |  |
| yemapt | 野马 | yemapt.org | YemaPT | — | — | — | 未测 |  | 未测 | 2 |  |
| yinghua | 樱花 | pt.ying.us.kg | NexusPHP | — | — | — | 未测 |  | 未测 | 0 | 已关站（savept.icu 2026-09-20）：域名 pt.ying.us.kg 已无法解析 |
| yzyy | YZYY | yzyy.org | Discuz | — | — | — | 未测 |  | 未测 | 0 |  |
| zhuque | 朱雀 | zhuque.in | TNode | — | — | — | 未测 |  | 未测 | 1 |  |

## 4. 卡住的站和怎么处理

| 类型 | 站点 | 结论 | 处理 |
| --- | --- | --- | --- |
| 账号没有发布权限 | hdsky、pter、hdarea、hhanclub、btschool 等 29 站，另有 ttg、ptt、hudbt、hdclone | 页面提示“没有发布种子的权限”（ttg、ptt 的写法都收进权限词表，ptt 是板块级） | 只停发种，辅种保留；权限不是代码能解决的 |
| 动态上传界面 | hdcity、mypt、rousi、haoxue | 两段式表单 / 时段限制 + 人机验证 / 单页应用走 JSON 接口 | 当前版本不支持，记录站点形态；二段字段与建种接口需真实浏览器核对；haoxue 2026-11-27 已走站点接口真发一条，是否长期维护浏览器提交流程见 ROADMAP |
| 连接不通 | hdclone、hudbt、u2、shadow、piggo、tu88 | 权限 / 登录墙 / 会话失效 / 需验证码 | 留在整站暂停；freefarm 的镜像入口都活、都已登录，单列下行 |
| 镜像双入口 | freefarm | `pt.0ff.cc` 和 `farmm.cc` 是同一站点的镜像，配置维持 `pt.0ff.cc`、不切 `farmm.cc`；两域都已登录 Schalkiii / UID 5481（LV1 User），62 控件静态表单 | 仍在 `pausedSites`，本轮不改 URL |
| 类目不符 | haitang、kelu、vclib、nicept、ziran、soulvoice、musopia、dicmusic、xingtan | 曲艺、有声书、音乐、电子书；xingtan 是学术教材方向，无电影/剧集类目（书籍/PDF 类目 2026-11-27 手动发成一条 456304，见 2.8） | 影视只做提示，不算表单缺口 |
| 关站或换形态 | momentpt、ptlao、movie52、yinghua | 告别页、404、域名解析失败 | 已从暂停名单摘除，保持 `enabled:false`，备注写 `statusNote` |
| 源站故障/过载 | aling、hdu | aling 首页/登录页/上传页全 HTTP 500（`cfOrigin;dur` 241/238/234 ms、`cfEdge;dur` 3–8 ms，源站报错，不是 WAF）；hdu 四个页面都回 200 但都是 253–264 字节「服务器负载过高，正在重试，请稍后...」刷新壳，页面上的 `[EASY-UPLOAD]` 是本机浏览器扩展注入，不是站点组件 | 待恢复后复测，不伪造字段 |
| 代理可达、等发布窗口 | gtk | 配该站 `overrides.proxy` 后走代理链路可达；波洛合集站内真重复不重发，知否字段补齐但 User 等级限周六 12:00–周日 23:59 发布，等窗口；站内已有 5 个其他版本知否，有判重风险 | 已临时加进 `autoReseedSites`，窗口内再发，见 2.9 |

## 5. 已确认关站的站

对照 savept.icu 的关站记录，本轮又逐一复测了一遍：

| 站点 | 关站日期 | 实测情况 |
| --- | --- | --- |
| momentpt | 2026-10-29 | 全站告别页「山水有相逢 / A quiet farewell」 |
| ptlao | 2026-10-21 | 公告原文「2026年10月20日，忘年桥PT站将永久关停」，现存页面只是交流论坛，旧 NexusPHP 路径全 404 |
| movie52 | 2026-10-20 | 告别页「很抱歉，和大家说再见了」。注意它和 `ourbits` 都会被 Cloudflare 挑战页拦，别把「Just a moment」当关站证据 |
| yinghua | 2026-09-20 | 域名解析失败（`getaddrinfo 11001`），经代理也是 502 |

这四个站 2026-11-26 已从 `assist.pausedSites` 摘除，保持 `enabled:false`。

## 6. 重新生成这张表

```powershell
# 只有新站第一次接入时才跑；日常不要批量探测
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" formcheck --sites <site>

# 生成表格到临时文件，核对后再替换本文第 3 节的表；不要直接覆盖整份文件
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" sites --md > $env:TEMP\sites.md
pwsh scripts/site-matrix.ps1   # 默认打印到屏幕；要写文件加 -OutFile
```

表单检查结果写进 `dataDir/formcheck.json`，包含提交地址和所有下拉选项。`bsend sites --md` 只读配置和这份缓存，不发站点请求。

## 7. 相关文档

- 新站怎么接、上传失败怎么查：`SITE-DEBUG.md`
- 判断规则和发种流程：`ARCHITECTURE.md`
- 数据文件与命令：`OPERATIONS.md`
- 还没做完的事：`ROADMAP.md`
