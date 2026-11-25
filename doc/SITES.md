# 站点清单与现状

这张表回答四个问题：配置里有哪些站、哪些参与自动发种、哪些站卡住了、卡在什么地方。它不回答“某个站已经覆盖了多少内容”，那要看 `bsend status`。适配方法和单站排查过程见 `SITE-DEBUG.md`，发种规则见 `ARCHITECTURE.md`。

## 1. 当前范围

- 配置里一共 **147** 个站；
- 参与扫描的 **101** 个；
- 允许自动发种的 **40** 个（`assist.autoReseedSites`）。

白名单只是“允许发”，不等于“一定发得出去”。实际发布还要过站点权限、字段检查、当天额度和失败冷却（默认关闭）。

2026-11-22 起，不再为了判断“站上有没有”逐个搜索，也不再日常批量跑表单检查。表单检查只在新站第一次接入时做一次，用来拿上传页字段和下拉选项。以前的检查结论只作参考，站点改版后可能失效。

当前白名单（40 站）：

```
audiences, baozi, cangbao, dragonhd, hdfans, hdtime, hdvideo, kufei, lajidui,
lemonhd, longpt, luckpt, ptba, ptcafe, ptfans, pthome, ptlgs, ptsbao, ptzone,
qingwa, sbpt, shuioudao, tangpt, xdy, city13, crabpt, cspt, cyanbug, dubhe,
hdbao, itzmx, nanyang, novahd, oshen, qilin, railgun, tey, tjupt, ubits,
xingyunge
```

## 2. 2026-11-22 夜间接入的 12 个站

“发布并挂回成功”要同时满足三件事：详情页标题和本地一致、上传者是 `Schalkiii`、qB 里带 `Boxsend` 标签的种子进度到 100%。

| 站点 | 结论 | 实测结果 | 详情页 / 新种 hash | 备注 |
| --- | --- | --- | --- | --- |
| ptzone | 可发 | 已发布，五项发布检查全过 | https://ptzone.xyz/details.php?id=190916 ｜ `10d23b0e…` | 发布并挂回成功 |
| kufei | 可发 | 已发布，五项发布检查全过 | https://kufei.org/details.php?id=141933 ｜ `266fbb06…` | 没有独立的 MediaInfo/IMDb 字段，内容并入简介 |
| longpt | 可发 | 已发布，五项发布检查全过 | https://longpt.org/details.php?id=51065 ｜ `91882f95…` | 第一次抓到的表单是 `bonusrain.php`，提交地址已修 |
| tangpt | 可发 | 已发布，五项发布检查全过 | https://www.tangpt.top/details.php?id=72943 ｜ `2c7da189…` | 第一次抓到的是搜索表单；TLS 需要跳过校验 |
| ptlgs | 可发 | 已发布，五组下拉全填 | https://ptlgs.org/details.php?id=115117 ｜ `aa85a3ad…` | 硬性要求海报、至少 3 张截图、MediaInfo；没有 DDP/E-AC3 |
| dragonhd | 可发 | 已发布并挂回 | https://www.dragonhd.xyz/details.php?id=48745 ｜ `bac2dd51…` | 中字、国语、HDR10 标签要人工补 |
| hdtime | 可发 | 已发布并挂回 | https://hdtime.org/details.php?id=238600 ｜ `77b02a63…` | 上传表单里没有分辨率下拉 |
| hdvideo | 已补挂 | 必填项修好后发布成功，新种 78856（11.56 GB） | https://hdvideo.top/details.php?id=78856 ｜ qB hash `dd7f6097…` | 必填的地区 `region_sel[4]=2`、题材 `style_sel[4][]=6` 已补；发后定位取错旧种 id（缺陷已修）。2026-11-23 已从详情页下载目标种补挂，qB 校验 100%、tracker 正常 |
| luckpt | 缺口待修 | 站点自带发布检查 95/100 | — | 豆瓣防盗链已修（补 `Referer` 后 418 变 200）；`img.pterclub.com` 在本机 TLS 连不上，截图数量不足 |
| qingwa | 无主流缺口 | 665/667 条有记录，约 99.7% | — | 没覆盖的是成人合集（被内容黑名单拦）和多片合集，不是适配器故障 |
| haitang | 类目不符 | 没有发布 | — | 只有曲艺、有声书类目；已移出白名单，规则留档 |
| haoxue | 需要重做 | 检索路由 404 | — | Next.js/Flux 单页应用；已移出白名单，需要按前端接口重做 |

扩容候选里，city13、novahd、xingyunge、cyanbug 这 4 个站已经实测发布并挂回成功；hdvideo（78856）、qilin（479362）、panda（141276）站上已经发布，2026-11-23 已全部补挂到 qB 并校验 100%；hdbao、tey、oshen、ptfans、itzmx、railgun、lemonhd 也在 2026-11-23 首次实发（见 2.1、2.2）；agsvpt 无发布权限，已 disable；qingwa、ubits 不是“无缺口”，最新 qB 口径分别是 402/3,388、656/3,388，缺口 2,986、2,732；其余没测或类目不符。证据见 `SITE-DEBUG.md`。

### 2.1 2026-11-23 新实发的 10 个站

这十站都是第一次向目标站上传，发布后站上 hash 和本地构造的 hash 不同；ptba 已确认是 `info.source` 被站点改写，cspt/dubhe/ptsbao/crabpt/hdbao/tey/oshen/ptfans/itzmx 的分片和标题未变但具体改写字段没有逐字节核对。程序按“该站白名单 + pieces 一致 + `info.name` 一致”放行，现场补挂成功。

| 站点 | 结论 | 实测结果 | 详情页 / 新种 hash | 备注 |
| --- | --- | --- | --- | --- |
| ptba | 可发 | 已发布并挂回，qB 校验 100% | https://1ptba.com/details.php?id=106718 ｜ `abf41c82…` | 站上把 `info.source` 写成 `[1ptba.com] 1PTBA.COM`；已加入 hash 改写白名单，后续同站不用再手工补挂 |
| cspt | 可发 | 已发布并挂回，qB 校验 100% | https://cspt.top/details.php?id=259870 ｜ `cf568e1a…` | 目标种 tracker 在 `tracker.cspt.cc`，页面域是 `cspt.top`；已补 announce 别名，站点种自带两条同站 announce |
| dubhe | 可发 | 已发布并挂回，qB 校验 100% | https://dubhe.site/details.php?id=55492 ｜ `b5ebc539…` | 第一次判重搜索 30 秒超时，重试通过；站上已有同片 1080p 其他版本，按策略继续发布；表单没有“完结”标签，发布后手动补 |
| ptsbao | 可发 | 已发布并挂回，qB 校验 100% | https://ptsbao.club/details.php?id=135787 ｜ `0b0087f7…` | `source_sel[1]` 是二级分类，已按 type 父类 + 标题自动填 `剧集-大陆`；上传也出现 hash 差异，已加入白名单；表单没有“完结”标签，发布后手动补 |
| crabpt | 可发 | 已发布并挂回，qB 校验 100% | https://crabpt.vip/details.php?id=240242 ｜ `732e2c99…` | 此前候选源站 `hdfans` 502，被失败冷却挡住，改选 `hdsky` 源后成功；站上已有同片 2160p 其他版本，按策略继续发布；上传出现 hash 差异，已加入白名单 |
| hdbao | 可发 | 已发布 2 条并挂回，qB 校验 100% | https://hdbao.cc/details.php?id=54402 ｜ `95eacf24…`；https://hdbao.cc/details.php?id=54403 ｜ `51ededb2…` | `autopost --limit 1` 实测连发两条（`posted_unresolved` 没计入 limit，已修）；站上同样改写 hash，两条按 pieces/标题校验补挂成功，已加入白名单 |
| tey | 可发（限非蓝光） | 已发布 1 条并挂回，qB 校验 100% | https://pt.tey.cc/details.php?id=623 ｜ `c9570904…` | 表单 `medium_sel[4]` 只有 WEB-DL/HDTV/DVDR/CD/Track，没有 Blu-ray；蓝光源会被选档门禁拦住，HDTV/WEB-DL 正常。qB 里 5 条老种全走 `tracker.tey.cc`，已补 announce 别名；站上改写 hash，已加入白名单；新种状态“未审”，2026-11-24 回访仍标 `title="未审"`，覆盖记录已落 `pending_review`（等站点审核），由回访跟进 |
| oshen | 可发 | 已发布 1 条并挂回，qB 校验 100% | http://www.oshen.win/details.php?id=57082 ｜ `22e3dd63…` | 两次失败后第三次恢复：判重搜索正常、上传成功；回包没给详情链接，落 `posted_unresolved`，按详情页补挂成功；站上 hash 与构造不同（构造 `0f73ae73…`，站上 `22e3dd63…`），已加入白名单；表单没有“完结”标签，发布后需人工补 |
| ptfans | 可发 | 已发布 1 条并挂回，qB 校验 100% | https://ptfans.cc/details.php?id=11111 ｜ `fdf27b29…` | 此前上传页和 IMDb 判重都超时；2026-11-23 恢复后一次通过。站上已有 720p 其他版本 6095，按策略继续发布；上传同样改写 hash（构造 `4fd4392f…`，站上 `fdf27b29…`），已加入白名单；表单无“完结”标签，人工补 |
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
| pthome | 已补挂 | qB 校验 100%，状态 `seeding` | https://www.pthome.net/details.php?id=284701 ｜ 构造 `b31d8eaf…`，站上 `ebaa6dd9…` | 源 hdfans 133877《人民的名义》2160p WEB-DL；qB 里 11-23 23:56 就在做种，本次补挂报“同 hash 跳过”，说明早已挂好 |
| nanyang | 已补挂 | qB 校验 100%，状态 `seeding` | https://nanyangpt.com/details.php?id=226159 ｜ 构造 `187d1487…`，站上 `8a9b9ab2…` | 源 soulvoice 54682《知否知否应是绿肥红瘦》4K WEB-DL；表单没有“中字”类标签复选框，需人工补 |
| ubits | 已补挂；审核没过已换图 | qB 校验 100%，做种中；覆盖记录 `review_rejected` | https://ubits.club/details.php?id=379266 ｜ 构造 `d9970bce…`，站上 `2a57b787…` | 源 hdsky 27637《伪装者》1080p HDTV；前两次报“上传失败！”，补 `source_sel[4]`（地区）和 `team_sel[4]`（`Other`）后通过。11-24 站内信判“海报与截图均无法访问（图床域名已失效）”：简介 9 张图全是已失效的 `i13.tietuku.com`；已换 imgbox 新图（1 海报 + 8 截图）并提交，页面仍显示不通过，等管理复审（详见 `SITE-DEBUG.md` 6.12） |
| hdfans | 已确认并补挂 | 287791 就是同一条《伪装者》，qB `b1e600a6…` 校验 100% | http://hdfans.org/details.php?id=287791 | 覆盖记录已是 `reseeded`（11-23 23:49 转种成功），qB 里 11-23 23:49 就在上传，无需重发 |
| tjupt | 登录态已恢复（演练通过，未实发） | `gatecheck --sites tjupt` 通过；真实上传超时未复测 | — | announce 地址按站点下载的种子改成 `tracker-public.tjupt.org`（https、带 passkey，已写进 `overrides.announceURL`，备份 `boxsend.json.bak-261124-101552`）。2026-11-24 11:35 上传曾 120s 超时且无回包。**2026-11-24 晚已从 CookieCloud 拉到 `tjupt.org` 的 `access_token`（有效期 2027-01-15）**，`torrents.php` / `takeupload.php` 均 200、非 `login.php`；`gatecheck` 构造字段 + 预审通过（站点无预审）。下一步再实发一条，看超时是否随登录态恢复消失。站上已有 `416154`（FLAC/DDP5.1）与 `236519`（同名 FRDS 合集、pieces 与本地不同），属实不同内容 |
| qingwa | 分区：Poirot 不走普通上传；《伪装者》已发并挂回 | Poirot = 站点判定过大，记为待走候选入口；《伪装者》`208575` 校验 100% | https://qingwapt.com/details.php?id=208575 ｜ 构造 `c78e9cb2…`，站上 `d1eeec5c…` | Poirot 回包其实是 `torrent_too_large` 表单（自动 submit 到 `offers.php?add_offer=1`）：站点要求改走候选入口，不是未知回包；程序记 `pending_offers`（待走候选入口），不算失败、不自动重试，`bsend status` 单列“待跟进”等人工去候选页提交（2026-11-24 11:33 真跑复现一次，覆盖记录落 `pending_offers`、审计 result=`pending_offers`、`status` 显示“待走候选入口 1”）。同站《伪装者》发布后自动挂回被 hash 校验拒绝，改用 `bsend pull` 把站点种子下回来挂进 qB，成功 |
| audiences | 判重路线已验证，未接入程序 | Kimi WebBridge 复用浏览器登录态实站检索成功 | — | 程序按“没确认重复就不上传”中止，没有盲发；无头 HTTP 复用不了浏览器会话。**2026-11-24 晚可行性实验通过**：Kimi WebBridge 驱动本机真实 Chrome，audiences 已登录 `Schalkiii`；页面内 `fetch('/torrents.php?search=…')` 返回 200、未跳登录、命中 2 个 `details.php?id=`、无人机验证。只做了只读检索，未提交任何判重。下一步把这条路线接进程序的判重流程（见下方 2.3 末尾） |

**audiences 的后续方案（2026-11-24 更新）**：不再折腾无头 HTTP 复用 cookie，改走 Kimi WebBridge 接管用户已登录的浏览器：判重检索交给人机已经登录的窗口做，拿到结果再回到程序决定发不发。可行性实验已通过（登录态复用 + 站内检索都成功），但还没接进程序；接入前 audiences 仍按“只人工发”处理。

### 2.4 2026-11-25 调试期小批量回归

这一轮对白名单站点逐站再发，原则是“先确认上一次到底发没发出去，再决定重不重发”。有代表性的结果：

| 站点 | 结果 | 详情页 | 备注 |
| --- | --- | --- | --- |
| baozi | 2 条，等审核 | https://p.t-baozi.cc/details.php?id=83738 | 波洛 294GB、知否；已挂回 qB |
| longpt | 1 条，等审核 | https://longpt.org/details.php?id=51248 | 知否；第二条上传页跳到 bonusrain.php，未发 |
| ptcafe | 1 条新发 + 1 条同 hash | https://ptcafe.club/details.php?id=137250 | 伪装者 |
| ptfans | 2 条已发布 | https://ptfans.cc/details.php?id=11125 | 知否、人民的名义 |
| pthome | 1 条已发布 + 1 条已存在补挂 | https://www.pthome.net/details.php?id=284810 | 金枝欲孽 78566 已补挂 |
| ptlgs | 2 条，等审核 | https://ptlgs.org/details.php?id=115313 | 伪装者、Coraline |
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
| lajidui | 2 条，等审核 | https://pt.lajidui.top/details.php?id=85000 | 知否、广告狂人 |
| lemonhd | 2 条已发布 | https://lemonhd.net/details.php?id=49077 | 知否、广告狂人 |
| city13 | 1 条已发布 | https://13city.org/details.php?id=88256 | 上传回 200 未识别，站内搜索确认已发出后补挂 |
| novahd | 2 条已发布 | https://pt.novahd.top/details.php?id=227655 | 伪装者、人民的名义；4K 60fps 选档已修 |
| qingwa | 1 条新发，等审核 | https://qingwapt.com/details.php?id=208692 | Bitter Moon；伪装者同内容已存在 |
| cspt | 2 条已发布，补清 MediaInfo | https://cspt.top/details.php?id=260050 | 站点不收 MediaInfo 和图片外 URL 标签，已清 `technical_info` |
| luckpt | 2 条，等审核 | https://pt.luckpt.de/details.php?id=61666 | 波洛、广告狂人；伪装者 61102 已补 MediaInfo |
| hdfans | 同 hash 跳过，未挂目标 tracker | http://hdfans.org/details.php?id=287874 | Liar Game 的 qB 同 hash 种只有 sbpt/ubits tracker，没有 hdfans；按“不改原种”跳过，实际未给 hdfans 做种 |
| sbpt | 同 hash / existed 补挂 | https://sbpt.link/details.php?id=21767 | 知否、广告狂人、A Touch of Zen |
| cyanbug | 判重复核超时 | https://cyanbug.net/details.php?id=32829 | 未上传；不盲目重发 |
| ubits | 上传被图床黑名单拦下 | https://ubits.club/details.php?id=379266 | 简介含 imgbox，站点要求换 pixhost 等图床 |
| qilin | 审核未过，简介已转 BBCode | https://www.hdkyl.in/details.php?id=479362 | 等站点重审 |
| tey | 波洛候选未上传：源种 0 图 | https://carpt.net/details.php?id=532 | 不是 tey 图床坏；tey 623《伪装者》9 张图正常。carpt 532 简介 0 个 [img]、编辑页无 takeedit 表单，程序每次重建简介都是 0 图，低于 3 张门禁 |
| tjupt | 294GB 大合集超时，未重发 | https://www.tjupt.org/details.php?id=236519 | 站点同名旧种不是我们的 |
| itzmx | 大种 403 / 超 266,240 字节 | https://pt.itzmx.com/details.php?id=13134 | 换小种再试 |
| audiences | 人机验证挡住判重 | — | 需浏览器登录态，暂不盲发 |

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
| freefarm | 农场 | pt.0ff.cc | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 请求没通：https://pt.0ff.cc/upload.php: error sending request for url (https://pt.0ff.cc/upload.php): operation timed out | 未测 | 2 |  |
| ggpt | GGPT | gamegamept.com | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://www.gamegamept.com/upload.php | ok | 1 |  |
| gtk | GTK | pt.gtkpw.xyz | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 请求没通：https://pt.gtkpw.xyz/upload.php: error sending request for url (https://pt.gtkpw.xyz/upload.php): operation timed out | 未测 | 2 |  |
| hdarea | 高清视界 | hdarea.club | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://hdarea.club/upload.php | 未测 | 6 |  |
| hdclone | 独自 | pt.hdclone.top | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 请求没通：https://pt.hdclone.top/upload.php: error sending request for url (https://pt.hdclone.top/upload.php): operation timed out | 未测 | 5 |  |
| hddolby | 杜比 | uhddolby.com | NexusPHP | 是 | — | 是 | fail | 目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：https://www.uhddolby.com/upload.php | unsupported | 5 |  |
| hdhome | 家园 | hdhome.org | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://hdhome.org/upload.php | 未测 | 6 |  |
| hdsky | 天空 | hdsky.me | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://hdsky.me/upload.php | unsupported | 6 |  |
| hhanclub | 憨憨 | hhanclub.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布」）：https://hhanclub.net/upload.php | unsupported | 5 |  |
| hitpt | 百川 | hitpt.com | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://www.hitpt.com/upload.php | ok | 1 |  |
| hudbt | 蝴蝶 | hudbt.hust.edu.cn | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 请求没通：https://hudbt.hust.edu.cn/upload.php: error sending request for url (https://hudbt.hust.edu.cn/upload.php): client error (Connect): tcp connect error: 由于连接方在一段时间后没有正确答复或连接的主机没有反应，连接尝试失败。 (os error 10060) | 未测 | 5 |  |
| kamept | 龟站 | kamept.com | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://kamept.com/upload.php | 未测 | 2 |  |
| keepfrds | 朋友 | pt.keepfrds.com | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://pt.keepfrds.com/upload.php | unsupported | 6 |  |
| march | 三月 | duckboobee.org | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://duckboobee.org/upload.php | ok | 1 |  |
| momentpt | 瞬间 | momentpt.top | NexusPHP | — | 是 | — | fail | 目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：https://www.momentpt.top/upload.php | 未测 | 2 | 已关站（savept.icu 2026-10-29）：站点已放告别页「Moment · 山水有相逢」 |
| movie52 | 52MOVIE | 52movie.top | NexusPHP | — | 是 | — | fail | 目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：https://www.52movie.top/upload.php | 未测 | 6 | 已关站（savept.icu 2026-10-20）：域名 52movie.top 已无法解析 |
| mypt | 我的PT | pt.mypt.cc | NexusPHP | 是 | — | 是 | fail | 目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：https://pt.mypt.cc/upload.php | unsupported | 0 |  |
| njtupt | 蒲园 | njtupt.top | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://njtupt.top/upload.php | ok | 5 |  |
| okpt | OK | okpt.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://www.okpt.net/upload.php | ok | 5 |  |
| piggo | 猪猪 | piggo.me | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 站点回 HTTP 403（多半是 cookie 失效）：https://piggo.me/upload.php | 未测 | 1 |  |
| playlet | PlayLet | playlet.cc | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://playlet.cc/upload.php | ok | 1 |  |
| pt52 | 52PT | 52pt.site | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://52pt.site/upload.php | 未测 | 5 |  |
| pter | 猫 | pterclub.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://pterclub.net/upload.php | unsupported | 7 |  |
| ptlao | 忘年桥 | ptlao.top | NexusPHP | — | 是 | — | fail | 抓目标站上传表单失败: 请求没通：https://ptlao.top/upload.php: error sending request for url (https://ptlao.top/upload.php): operation timed out | 未测 | 1 | 已关站（savept.icu 2026-10-21）：原 PT 站已停，域名现为「忘年桥资源站」，upload.php 404 |
| ptskit | 拾刻 | ptskit.org | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://www.ptskit.org/upload.php | ok | 2 |  |
| ptt | 时间 | pttime.org | NexusPHP | 是 | — | 是 | fail | 目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：https://www.pttime.org/upload.php | 未测 | 6 |  |
| rousi | 肉丝 | rousi.pro | NexusPHP | 是 | — | 是 | fail | 目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：https://rousi.pro/upload.php | unsupported | 1 |  |
| shadow | 影 | star-space.net | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: HTTP 404 抓页面 https://star-space.net/upload.php：File not found. | 未测 | 1 |  |
| siqi | 思齐 | si-qi.xyz | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://si-qi.xyz/upload.php | ok | 1 |  |
| sunny | 阳光 | sunnypt.top | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: HTTP 404 抓页面 https://sunnypt.top/upload.php：<!DOCTYPE html><html class="scrollbar-none" lang="zh-CN"><head><meta charSet="utf-8"/><meta name="viewport" content="wid | 未测 | 2 |  |
| tccf | TCCF | et8.org | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://et8.org/upload.php | unsupported | 5 |  |
| tokyo | Tokyo | tokyo-manga.top | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://www.tokyo-manga.top/upload.php | 未测 | 1 |  |
| ttg | 套 | totheglory.im | NexusPHP | 是 | — | 是 | fail | 目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：https://totheglory.im/upload.php | unsupported | 6 |  |
| tu88 | TU88 | pt.tu88.men | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 被跳到登录页/首页（cookie 可能已失效，或站内有待确认的公告/考核）：https://pt.tu88.men/upload.php | 未测 | 1 |  |
| u2 | 幼儿圈 | u2.dmhy.org | NexusPHP | 是 | 是 | — | fail | 抓目标站上传表单失败: 请求没通：http://u2.dmhy.org/upload.php: error sending request for url (http://u2.dmhy.org/upload.php): client error (Connect): tcp connect error: 由于连接方在一段时间后没有正确答复或连接的主机没有反应，连接尝试失败。 (os error 10060) | 未测 | 2 |  |
| ultrahd | UltraHD | ultrahd.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://ultrahd.net/upload.php | ok | 1 |  |
| wtsakura | 冬樱 | wintersakura.net | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://wintersakura.net/upload.php | ok | 2 |  |
| xingwan | 星湾 | xingwan.cc | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://xingwan.cc/upload.php | ok | 2 |  |
| zmpt | 织梦 | zmpt.cc | NexusPHP | 是 | — | 是 | fail | 该账号无发布权限（页面提示含「没有发布种子的权限」）：https://zmpt.cc/upload.php | ok | 2 |  |
| hdcity | 城市 | hdcity.city | NexusPHP | 是 | — | — | gap | 简介为空 | unsupported | 5 |  |
| musopia | 音乐乌托邦 | musopia.vip | NexusPHP | 是 | — | — | gap | medium_sel 未选 | 未测 | 1 |  |
| muxuege | 慕雪阁 | pt.muxuege.org | NexusPHP | 是 | — | — | gap | audiocodec_sel 未选 | ok | 1 |  |
| ptfans | PTFans | ptfans.cc | NexusPHP | 是 | — | — | ok | medium_sel 未选 | ok | 1 | 2026-11-23 已实发 `11111`，`fdf27b29…`，补挂 qB 校验 100%；站上改写 hash，已入白名单；站上已有 720p 其他版本，按策略继续发；表单无“完结”标签，人工补 |
| agsvpt | 末日 | www.agsvpt.com | NexusPHP | 否 | — | — | ok | team_sel[4] 未选 | 未测 | 1 | 无发布权限，2026-11-23 已 disable 并移出自动发种白名单 |
| aling | 爱玲 | pt.aling.de | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | 未测 | 1 |  |
| audiences | 观众 | audiences.me | NexusPHP | 是 | — | — | ok |  | unsupported | 6 | 2026-11-23 判重检索被人机验证拦住，按“未确认重复不上传”中止，未发布；无头 HTTP 复用不了浏览器会话。2026-11-24 晚 Kimi WebBridge 可行性实验通过：真实 Chrome 已登录 `Schalkiii`，页面内 `fetch('/torrents.php?search=…')` 返回 200、命中 2 个详情页、无验证码；只做只读检索，尚未接进程序，暂仍按只人工发处理（见 2.3 末尾） |
| baozi | 包子 | p.t-baozi.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| cangbao | 藏宝阁 | cangbao.ge | NexusPHP | 是 | — | — | ok | 简介默认转 BBCode；43994 已改 Encode/720p/BBCode，海报和截图已重传 imgbox，等站复审；门禁 `titleNoCjk+剪死图+至少3张` | ok | 1 | 43994 图片死链已修；2026-11-24 回访，详情页审核图标已标 `title="通过"`（审核已过） |
| city13 | 13City | 13city.org | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 1 |  |
| crabpt | 蟹黄堡 | crabpt.vip | NexusPHP | 是 | — | — | ok | codec_sel[6] 未选 | ok | 1 | 2026-11-23 已实发 240242，`732e…`；此前候选源站 hdfans 502，被失败冷却挡住，改选 hdsky 源后成功；站点改写 hash，已加入白名单，挂回校验 100%；2026-11-24 晚《伪装者》简介 9 张 `i13.tietuku.com` 死图已换 imgbox（复查 0 旧图 / 9 新图） |
| cspt | 财神 | cspt.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 | 2026-11-23 已实发 259870，`cf56…`；tracker 在 `tracker.cspt.cc`，已补别名；站点改写 hash，挂回校验 100% |
| cyanbug | 大青虫 | cyanbug.net | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 |  |
| dicmusic | 海豚 | dicmusic.com | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 3 |  |
| dragonhd | 龙之家 | dragonhd.xyz | NexusPHP | 是 | — | — | ok | team_sel 未选 | unsupported | 5 |  |
| dubhe | 天枢 | dubhe.site | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 已实发 55492，`b5eb…`；判重搜索偶发超时；站点改写 hash，挂回校验 100% |
| fenghuang | 凤凰 | pt.521.best | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| haitang | 海棠 | htpt.cc | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 7 |  |
| haoxue | 好学 | hxpt.org | NexusPHP | 是 | — | — | ok | category_id 未选 | unsupported | 5 |  |
| hdbao | 海德堡 | hdbao.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 | 2026-11-23 已实发 54402/54403，`95ea…`/`51ed…`；站上改写 hash，已加入白名单，两条均补挂校验 100%；`autopost --limit 1` 曾连发两条，已修 limit 计数 |
| hdfans | 红豆饭 | hdfans.org | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 5 | 2026-11-23 发《伪装者》时回包带 `existed=1`（指向 287791）；2026-11-24 核对 287791 就是同一条，qB 里 `b1e600a6…` 校验 100%，覆盖记录 `reseeded`，未重发；2026-11-24 晚该条简介 9 张 `i13.tietuku.com` 死图已换 imgbox（复查 0 旧图 / 9 新图）；当源站时常见 502 / 超时 |
| hdtime | 时光 | hdtime.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 6 |  |
| hdu | 好多油 | pt.upxin.net | NexusPHP | 是 | — | — | ok | processing_sel 未选 | unsupported | 5 |  |
| hdvideo | HDVideo | hdvideo.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 检查结论是上一次表单结果；`region_sel[4]`、`style_sel[4][]` 已补，78856 已补挂 qB |
| iloli | 爱萝莉 | mua.xloli.cc | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | 未测 | 1 |  |
| itzmx | Itz | pt.itzmx.com | NexusPHP | 是 | — | — | ok | team_sel 未选 | ok | 5 | 2026-11-23 首次实发 `13134`（`88e3103b…`）并补挂校验 100%；站点 torrent 文件上限 `266,240 bytes`，大合集/大剧包会被拒，改用小种《Liar Game》通过；站上改写 hash，已加入白名单 |
| kelu | Kelu | our.kelu.one | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 1 |  |
| kufei | 库非 | kufei.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| lajidui | 垃圾堆 | pt.lajidui.top | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 1 |  |
| lemonhd | 柠檬不甜 | lemonhd.net | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 首次实发 48970（`3c00cbb2…`）并补挂校验 100%；站上改写 hash，已加入白名单；savept.icu 同名 LemonHD（lemonhd.club）标注已关站 2026-03-13，本配置域名 lemonhd.net 实测存活 |
| longpt | 龙 | longpt.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 3 |  |
| luckpt | 幸运 | pt.luckpt.de | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 4 | 2026-11-24 晚《伪装者》`61102` 简介 9 张 `i13.tietuku.com` 死图已换 imgbox（复查 0 旧图 / 9 新图），回包 `edited=1&audit_pending=1` |
| nanyang | 南洋 | nanyangpt.com | NexusPHP | 是 | — | — | ok | [nanyang] 表单里没找到「中字」类标签复选框，请发布后手动补 | unsupported | 6 | 2026-11-24 首次实发 `226159`（《知否》，源 soulvoice）；站上 hash 与构造不同（构造 `187d1487…`，站上 `8a9b9ab2…`），已加入 hash 改写白名单，把站点种子下回来挂进 qB，校验 100%、单站 tracker（见 2.3） |
| nicept | 老师 | nicept.net | NexusPHP | 是 | — | — | ok | type 未选 | ok | 1 |  |
| novahd | nova | pt.novahd.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 |  |
| oshen | 奥申 | oshen.win | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 已实发 `57082`，`22e3dd63…`，补挂 qB 校验 100%；站上改写 hash，已入白名单；表单没有“完结”标签，人工补 |
| panda | 熊猫 | pandapt.net | NexusPHP | 是 | — | 是 | ok | medium_sel[5] 未选 | 未测 | 5 | 141276（《星球大战9：天行者崛起》UHD BluRay 1080p 单文件 11.56G，https://pandapt.net/details.php?id=141276 ）已补挂 qB/校验 100%，主标题去掉了 `[50%] 剩余时间` 中文、简介按【转载出处-制作说明-海报-简介-info-截图】重排并换 imgbox 图；门禁 `titleNoCjk+剪死图+至少4张（熊猫站规）+descrOrder`。2026-11-24 查清去向：第一次被 NoCaps 判主标题含中文 + 简介顺序不合规；重提的那版被站点当“候选种子”，24 小时内没做种被系统自动删除（不是审核不过被删）。qB 里那条还挂着 tracker，tracker 报 `torrent not registered`；2026-11-24 晚已暂停熊猫发种（从 `autoReseedSites` 摘掉、列入 `reseedPausedSites`），删还是重发待定 |
| ptba | 1PT | 1ptba.com | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 2 | 2026-11-23 已实发 106718，`abf4…`；站点改写 `info.source`，已加入 hash 改写白名单，挂回校验 100% |
| ptcafe | 咖啡 | ptcafe.club | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | ok | 1 |  |
| pthome | 铂金家 | pthome.net | NexusPHP | 是 | — | — | ok | team_sel 未选 | unsupported | 1 | 2026-11-24 首次实发 `284701`（《人民的名义》，源 hdfans）；站上 hash 与构造不同（构造 `b31d8eaf…`，站上 `ebaa6dd9…`），已加入白名单，补挂时确认 qB 里 11-23 23:56 就在做种、校验 100%（见 2.3） |
| ptlgs | 劳改所 | ptlgs.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | 未测 | 6 |  |
| ptsbao | 烧包 | ptsbao.club | NexusPHP | 是 | — | — | ok | processing_sel[1] 未选 | ok | 1 | 2026-11-23 已实发 135787，`0b0087…`；`source_sel[1]` 二级分类已自动填 `剧集-大陆`；站点改写 hash，已加入白名单，挂回校验 100%；表单无“完结”标签需人工补 |
| ptzone | 葡萄汁 | ptzone.xyz | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| qilin | 麒麟 | hdkyl.in | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 9 | 479362 已补挂 qB；站点审核不通过（缺图），不影响做种；2026-11-24 横幅仍写“审核不通过，原因：缺少海报或截图（图片少于 2 张）”，覆盖记录落 `review_rejected`。旧简介 7 张图全不可达（豆瓣海报防盗链 418、`img.pterclub.com` TLS 失败），已全部换成 imgbox 图（1 海报 + 6 截图）并提交，等复审；门禁 `剪死图+至少3张` |
| qingwa | 青蛙 | qingwapt.com | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 | announce 别名 `tracker.qingwapt.org` 已补；2026-11-24 Poirot 合集被站点判 `torrent_too_large`（要求改走 `offers.php` 候选），程序记 `pending_offers`（待走候选入口），不算失败、不自动重试，`status` 单列待跟进；同站《伪装者》发 `208575` 并挂回，校验 100%（构造 `c78e9cb2…`，站上 `d1eeec5c…`），当晚 9 张 `i13.tietuku.com` 死图已换 imgbox（复查 0 旧图 / 9 新图）；《知否》回包“已存在！”且 qB 已在做种（见 2.3） |
| railgun | Railgun | bilibili.download | NexusPHP | 是 | — | — | ok |  | ok | 2 | 2026-11-23 首次实发 60632（`72889498…`）并补挂校验 100%；站上改写 hash，已加入白名单 |
| sbpt | SBPT | sbpt.link | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 | 2026-11-24 晚核对《伪装者》`21684`：简介本来 0 张图，没有死图可换，未改 |
| shuioudao | 下水道 | sewerpt.com | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 |  |
| soulvoice | 聆音 | pt.soulvoice.club | NexusPHP | 是 | — | — | ok | type 未选 | ok | 1 |  |
| tangpt | 躺平 | tangpt.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| tey | 太乙 | pt.tey.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 已实发 `623`，`c9570904…`，补挂 qB 校验 100%；`medium_sel` 仅 WEB-DL/HDTV/DVDR/CD/Track，蓝光源会被门禁拦；qB 老种走 `tracker.tey.cc`，已补别名；站上改写 hash，已入白名单；新种状态“未审”（2026-11-24 回访仍未审，覆盖记录落 `pending_review`）。同一条简介曾带 9 张失效的 `i13.tietuku.com` 图，已换 imgbox 图并提交；门禁 `剪死图+至少3张` |
| tjupt | 北洋园 | tjupt.org | NexusPHP | 是 | — | — | ok | [tjupt] 表单里没找到「中字」类标签复选框，请发布后手动补 | unsupported | 1 | 2026-11-24 剧集分类的 `cname/ename/specificcat` 已能自动填；announce 以站点下载的种子定案为 `tracker-public.tjupt.org`（已写进 `overrides.announceURL`）；2026-11-24 晚从 CookieCloud 拉到 `tjupt.org` 的 `access_token`（2027-01-15 到期）并合并进 `cookies.json`，`torrents.php` / `takeupload.php` 均 200、非 `login.php`，`gatecheck` 通过；上传 120s 超时尚未复测；站上已有 `416154`、`236519`（pieces 与本地不同，属实不同内容） |
| ubits | 优堡 | ubits.club | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | ok | 8 | `source_sel[4]`（地区）和 `team_sel[4]`（`Other`）已能自动填，上面的检查结论是旧结果；2026-11-24 首次实发 `379266`（《伪装者》，源 hdsky），站上 hash 与构造不同（构造 `d9970bce…`，站上 `2a57b787…`），已加入白名单，把站点种子下回来挂进 qB，校验 100%、单站 tracker；该条简介 9 张 `i13.tietuku.com` 死图已换 imgbox，页面仍标不通过，等复审（见 2.3） |
| vclib | VC-Lib | pt.vclib.online | NexusPHP | 是 | — | — | ok | type 未选 | ok | 2 |  |
| xdy | 修道院 | xdypt.vip | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| xingtan | 杏坛 | xingtan.one | NexusPHP | 是 | — | — | ok | source_sel[16] 未选 | 未测 | 5 |  |
| xingyunge | 星陨阁 | pt.xingyungept.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| ziran | 自然 | naturept.top | NexusPHP | 是 | — | — | ok | type 未选 | ok | 2 |  |
| aither | Aither | aither.cc | custom | — | — | — | 未测 |  | 未测 | 0 |  |
| alpharatio | AlphaRatio | alpharatio.cc | Gazelle | — | — | — | 未测 |  | 未测 | 0 |  |
| animez | AnimeZ | animez.to | Gazelle | — | — | — | 未测 |  | 未测 | 0 |  |
| anthelion | Anthelion | anthelion.me | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| azusa | 梓喵 | azusa.wiki | NexusPHP | — | — | — | 未测 |  | 未测 | 0 |  |
| beyondhd | BeyondHD | beyond-hd.me | XBTIT | — | — | — | 未测 |  | 未测 | 3 |  |
| bitporn | BitPorn | bitporn.eu | custom | — | — | — | 未测 |  | 未测 | 1 |  |
| blutopia | Blutopia | blutopia.cc | Blu | — | — | — | 未测 |  | 未测 | 3 |  |
| brokenstones | BrokenStones | brokenstones.is | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| byr | 北邮 | byr.pt | NexusPHP | — | — | — | 未测 |  | 未测 | 0 |  |
| clearjav | ClearJAV | clearjav.com | XBTIT | — | — | — | 未测 |  | 未测 | 1 |  |
| empornium | 峨眉派 | empornium.sx | Unit3D | — | — | — | 未测 |  | 未测 | 1 |  |
| exoticaz | ExoticaZ | exoticaz.to | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| fappaizuri | Fappaizuri | fappaizuri.me | XBTIT | — | — | — | 未测 |  | 未测 | 0 |  |
| filelist | FileList | filelist.io | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
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
| ourbits | 我堡 | ourbits.club | custom | — | — | — | 未测 |  | 未测 | 2 |  |
| ptneko | 超科学PT喵 | ptneko.com | Unit3D | — | — | — | 未测 |  | 未测 | 0 |  |
| sjtu | 葡萄 | pt.sjtu.edu.cn | custom | — | — | — | 未测 |  | 未测 | 0 |  |
| sportscult | SportsCult | sportscult.org | Gazelle | — | — | — | 未测 |  | 未测 | 3 |  |
| tlf | 吐鲁番 | pt.eastgame.org | NexusPHP | — | — | — | 未测 |  | 未测 | 0 |  |
| torrentleech | TorrentLeech | torrentleech.cc | custom | — | — | — | 未测 |  | 未测 | 0 |  |
| yemapt | 野马 | yemapt.org | YemaPT | — | — | — | 未测 |  | 未测 | 5 |  |
| yinghua | 樱花 | pt.ying.us.kg | NexusPHP | — | 是 | — | 未测 |  | 未测 | 0 | 已关站（savept.icu 2026-09-20）：域名 pt.ying.us.kg 已无法解析 |
| yzyy | YZYY | yzyy.org | Discuz | — | — | — | 未测 |  | 未测 | 0 |  |
| zhuque | 朱雀 | zhuque.in | TNode | — | — | — | 未测 |  | 未测 | 1 |  |

## 4. 卡住的站和怎么处理

| 类型 | 站点 | 结论 | 处理 |
| --- | --- | --- | --- |
| 账号没有发布权限 | hdsky、pter、hdarea、hhanclub、btschool 等 29 站 | 页面提示“没有发布种子的权限” | 只停发种，辅种保留 |
| 动态上传界面 | ttg、keepfrds、hdcity、hddolby、ptt | 没有静态表单，或只有文件框和隐藏字段 | 当前版本不支持，记录站点形态 |
| 连接超时或连不通 | freefarm、gtk、hdclone、hudbt、u2、shadow、sunny | 域名、TLS 或连接超时 | 整站暂停，不写“站上没有”的记录 |
| 类目不符 | haitang、kelu、vclib、nicept、ziran、soulvoice、musopia、dicmusic | 曲艺、有声书、音乐、电子书 | 只做提示，不算表单缺口 |
| 关站或换形态 | momentpt、ptlao、movie52、yinghua | 告别页、404、域名解析失败 | 暂停，并在备注写 `statusNote` |

## 5. 已确认关站的站

对照 savept.icu 的关站记录：

| 站点 | 关站日期 | 实测情况 |
| --- | --- | --- |
| momentpt | 2026-10-29 | 放出了告别页 |
| ptlao | 2026-10-21 | 变成资源站，`upload.php` 返回 404 |
| movie52 | 2026-10-20 | 域名解析失败 |
| yinghua | 2026-09-20 | 域名解析失败 |

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
