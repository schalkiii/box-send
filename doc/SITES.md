# 站点清单与现状

这张表回答四个问题：配置里有哪些站、哪些参与自动发种、哪些站卡住了、卡在什么地方。它不回答“某个站已经覆盖了多少内容”，那要看 `bsend status`。适配方法和单站排查过程见 `SITE-DEBUG.md`，发种规则见 `ARCHITECTURE.md`。

## 1. 当前范围

- 配置里一共 **147** 个站；
- 参与扫描的 **101** 个；
- 允许自动发种的 **42** 个（`assist.autoReseedSites`）。

白名单只是“允许发”，不等于“一定发得出去”。实际发布还要过站点权限、字段检查、当天额度和退避。

2026-11-22 起，不再为了判断“站上有没有”逐个搜索，也不再日常批量跑表单检查。表单检查只在新站第一次接入时做一次，用来拿上传页字段和下拉选项。以前的检查结论只作参考，站点改版后可能失效。

当前白名单（42 站）：

```
audiences, baozi, cangbao, dragonhd, hdfans, hdtime, hdvideo, kufei, lajidui,
lemonhd, longpt, luckpt, ptba, ptcafe, ptfans, pthome, ptlgs, ptsbao, ptzone,
qingwa, sbpt, shuioudao, tangpt, xdy, agsvpt, city13, crabpt, cspt, cyanbug,
dubhe, hdbao, itzmx, nanyang, novahd, oshen, panda, qilin, railgun, tey,
tjupt, ubits, xingyunge
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

扩容候选里，city13、novahd、xingyunge、cyanbug 这 4 个站已经实测发布并挂回成功；hdvideo（78856）、qilin（479362）、panda（141276）站上已经发布，2026-11-23 已全部补挂到 qB 并校验 100%；hdbao、tey、oshen、ptfans、itzmx、railgun、lemonhd 也在 2026-11-23 首次实发（见 2.1、2.2）；agsvpt 权限不足；ubits 是站上已有、无缺口；其余没测或类目不符。证据见 `SITE-DEBUG.md`。

### 2.1 2026-11-23 新实发的 10 个站

这十站都是第一次向目标站上传，发布后站上 hash 和本地构造的 hash 不同；ptba 已确认是 `info.source` 被站点改写，cspt/dubhe/ptsbao/crabpt/hdbao/tey/oshen/ptfans/itzmx 的分片和标题未变但具体改写字段没有逐字节核对。程序按“该站白名单 + pieces 一致 + `info.name` 一致”放行，现场补挂成功。

| 站点 | 结论 | 实测结果 | 详情页 / 新种 hash | 备注 |
| --- | --- | --- | --- | --- |
| ptba | 可发 | 已发布并挂回，qB 校验 100% | https://1ptba.com/details.php?id=106718 ｜ `abf41c82…` | 站上把 `info.source` 写成 `[1ptba.com] 1PTBA.COM`；已加入 hash 改写白名单，后续同站不用再手工补挂 |
| cspt | 可发 | 已发布并挂回，qB 校验 100% | https://cspt.top/details.php?id=259870 ｜ `cf568e1a…` | 目标种 tracker 在 `tracker.cspt.cc`，页面域是 `cspt.top`；已补 announce 别名，站点种自带两条同站 announce |
| dubhe | 可发 | 已发布并挂回，qB 校验 100% | https://dubhe.site/details.php?id=55492 ｜ `b5ebc539…` | 第一次判重搜索 30 秒超时，重试通过；站上已有同片 1080p 其他版本，按策略继续发布；表单没有“完结”标签，发布后手动补 |
| ptsbao | 可发 | 已发布并挂回，qB 校验 100% | https://ptsbao.club/details.php?id=135787 ｜ `0b0087f7…` | `source_sel[1]` 是二级分类，已按 type 父类 + 标题自动填 `剧集-大陆`；上传也出现 hash 差异，已加入白名单；表单没有“完结”标签，发布后手动补 |
| crabpt | 可发 | 已发布并挂回，qB 校验 100% | https://crabpt.vip/details.php?id=240242 ｜ `732e2c99…` | 此前候选源站 `hdfans` 502 退避，改选 `hdsky` 源后成功；站上已有同片 2160p 其他版本，按策略继续发布；上传出现 hash 差异，已加入白名单 |
| hdbao | 可发 | 已发布 2 条并挂回，qB 校验 100% | https://hdbao.cc/details.php?id=54402 ｜ `95eacf24…`；https://hdbao.cc/details.php?id=54403 ｜ `51ededb2…` | `autopost --limit 1` 实测连发两条（`posted_unresolved` 没计入 limit，已修）；站上同样改写 hash，两条按 pieces/标题校验补挂成功，已加入白名单 |
| tey | 可发（限非蓝光） | 已发布 1 条并挂回，qB 校验 100% | https://pt.tey.cc/details.php?id=623 ｜ `c9570904…` | 表单 `medium_sel[4]` 只有 WEB-DL/HDTV/DVDR/CD/Track，没有 Blu-ray；蓝光源会被选档门禁拦住，HDTV/WEB-DL 正常。qB 里 5 条老种全走 `tracker.tey.cc`，已补 announce 别名；站上改写 hash，已加入白名单；新种状态“未审”，等站点审核 |
| oshen | 可发 | 已发布 1 条并挂回，qB 校验 100% | http://www.oshen.win/details.php?id=57082 ｜ `22e3dd63…` | 两次失败后第三次恢复：判重搜索正常、上传成功；回包没给详情链接，落 `posted_unresolved`，按详情页补挂成功；站上改写 hash（构造 `0f73ae73…`，站上 `22e3dd63…`），已加入白名单；表单没有“完结”标签，发布后需人工补 |
| ptfans | 可发 | 已发布 1 条并挂回，qB 校验 100% | https://ptfans.cc/details.php?id=11111 ｜ `fdf27b29…` | 此前上传页和 IMDb 判重都超时；2026-11-23 恢复后一次通过。站上已有 720p 其他版本 6095，按策略继续发布；上传同样改写 hash（构造 `4fd4392f…`，站上 `fdf27b29…`），已加入白名单；表单无“完结”标签，人工补 |
| itzmx | 可发（限小种） | 已发布 1 条并挂回，qB 校验 100% | https://pt.itzmx.com/details.php?id=13134 ｜ `88e3103b…` | 站点 torrent 文件上限 `266,240 bytes`，大合集/大剧包普遍超限；改用 `ubits` 源的《Liar Game》小种（源种 96,934 bytes）一次通过。上传后 hash 被站点改写（构造 `7744200c…`，站上 `88e3103b…`），已加入白名单；详情页没找到“中字/完结”标签，发布后手动补 |

### 2.2 2026-11-23 继续实发：railgun、lemonhd

这两站也是第一次实发，上传成功、站上 hash 与构造 hash 不同；按 pieces + `info.name` 校验后补挂，qB 进度 100%。

| 站点 | 结论 | 实测结果 | 详情页 / 新种 hash | 备注 |
| --- | --- | --- | --- | --- |
| railgun | 可发 | 已发布 1 条并挂回，qB 校验 100% | https://bilibili.download/details.php?id=60632 ｜ `72889498…` | 源 `carpt` 的 Poirot 合集；上传回包未给详情链接，落 `posted_unresolved`；补入 hash 改写白名单后 `reattach`，qB 校验 100%，tracker 为 `bilibili.download` |
| lemonhd | 可发 | 已发布 1 条并挂回，qB 校验 100% | https://lemonhd.net/details.php?id=48970 ｜ `3c00cbb2…` | 源 `carpt` 的 Poirot 合集；站上已有不同版本 13452，按策略继续发布；上传回包未给详情链接，落 `posted_unresolved`；补入 hash 改写白名单后 `reattach`，qB 校验 100%，tracker 为 `tracker.lemonhd.net` |

## 3. 全部站点（147 个）

> 这张表由配置和 `formcheck.json` 生成，不是实时状态。`bsend sites --md` 和 `scripts/site-matrix.ps1` 都可以重新生成；手改只改“备注”列。

先说明表里的缩写和取值：

- **站点程序**：站点用的论坛/种子站程序，当前只有 NexusPHP 参与上传；
- **参与扫描**：`sourceSites[].enabled`，是否进入扫描范围；
- **整站暂停**：`pausedSites`，连辅种也停；
- **只停发种**：`reseedPausedSites`，辅种照常；
- **表单检查**：最近一次 `formcheck` 的结论，`ok` 表示能拿到表单，`fail` 表示拿不到，`gap` 表示表单有缺口，`未测` 表示没查过；
- **检查结论**：那次检查的具体原因；
- **分片校验**：`ok` 表示能用分片比对，`unsupported` 表示当前不支持，“未测”表示还没验证；
- **Cookie 数**：本地保存的该站 Cookie 条数，不是登录状态。

| 站点 | 名称 | 域名 | 站点程序 | 参与扫描 | 整站暂停 | 只停发种 | 表单检查 | 检查结论 | 分片校验 | Cookie 数 | 备注 |
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
| agsvpt | 末日 | agsvpt.com | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | 未测 | 1 |  |
| aling | 爱玲 | pt.aling.de | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | 未测 | 1 |  |
| audiences | 观众 | audiences.me | NexusPHP | 是 | — | — | ok |  | unsupported | 6 |  |
| baozi | 包子 | p.t-baozi.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| cangbao | 藏宝阁 | cangbao.ge | NexusPHP | 是 | — | — | ok | 简介默认转 BBCode；43994 已改 Encode/720p/BBCode，待复审核 | ok | 1 | 43994 审核驳回后修复中 |
| city13 | 13City | 13city.org | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 1 |  |
| crabpt | 蟹黄堡 | crabpt.vip | NexusPHP | 是 | — | — | ok | codec_sel[6] 未选 | ok | 1 | 2026-11-23 已实发 240242，`732e…`；此前候选源站 hdfans 502 退避，改选 hdsky 源后成功；站点改写 hash，已加入白名单，挂回校验 100% |
| cspt | 财神 | cspt.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 | 2026-11-23 已实发 259870，`cf56…`；tracker 在 `tracker.cspt.cc`，已补别名；站点改写 hash，挂回校验 100% |
| cyanbug | 大青虫 | cyanbug.net | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 |  |
| dicmusic | 海豚 | dicmusic.com | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 3 |  |
| dragonhd | 龙之家 | dragonhd.xyz | NexusPHP | 是 | — | — | ok | team_sel 未选 | unsupported | 5 |  |
| dubhe | 天枢 | dubhe.site | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 已实发 55492，`b5eb…`；判重搜索偶发超时；站点改写 hash，挂回校验 100% |
| fenghuang | 凤凰 | pt.521.best | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| haitang | 海棠 | htpt.cc | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 7 |  |
| haoxue | 好学 | hxpt.org | NexusPHP | 是 | — | — | ok | category_id 未选 | unsupported | 5 |  |
| hdbao | 海德堡 | hdbao.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 | 2026-11-23 已实发 54402/54403，`95ea…`/`51ed…`；站上改写 hash，已加入白名单，两条均补挂校验 100%；`autopost --limit 1` 曾连发两条，已修 limit 计数 |
| hdfans | 红豆饭 | hdfans.org | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 5 |  |
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
| luckpt | 幸运 | pt.luckpt.de | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 4 |  |
| nanyang | 南洋 | nanyangpt.com | NexusPHP | 是 | — | — | ok | [nanyang] 表单里没找到「中字」类标签复选框，请发布后手动补 | unsupported | 6 |  |
| nicept | 老师 | nicept.net | NexusPHP | 是 | — | — | ok | type 未选 | ok | 1 |  |
| novahd | nova | pt.novahd.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 |  |
| oshen | 奥申 | oshen.win | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 已实发 `57082`，`22e3dd63…`，补挂 qB 校验 100%；站上改写 hash，已入白名单；表单没有“完结”标签，人工补 |
| panda | 熊猫 | pandapt.net | NexusPHP | 是 | — | — | ok | medium_sel[5] 未选 | 未测 | 5 | 141276 已补挂 qB，校验 100% |
| ptba | 1PT | 1ptba.com | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 2 | 2026-11-23 已实发 106718，`abf4…`；站点改写 `info.source`，已加入 hash 改写白名单，挂回校验 100% |
| ptcafe | 咖啡 | ptcafe.club | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | ok | 1 |  |
| pthome | 铂金家 | pthome.net | NexusPHP | 是 | — | — | ok | team_sel 未选 | unsupported | 1 |  |
| ptlgs | 劳改所 | ptlgs.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | 未测 | 6 |  |
| ptsbao | 烧包 | ptsbao.club | NexusPHP | 是 | — | — | ok | processing_sel[1] 未选 | ok | 1 | 2026-11-23 已实发 135787，`0b0087…`；`source_sel[1]` 二级分类已自动填 `剧集-大陆`；站点改写 hash，已加入白名单，挂回校验 100%；表单无“完结”标签需人工补 |
| ptzone | 葡萄汁 | ptzone.xyz | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| qilin | 麒麟 | hdkyl.in | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 9 | 479362 已补挂 qB；站点审核不通过（缺图），不影响做种 |
| qingwa | 青蛙 | qingwapt.com | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 |  |
| railgun | Railgun | bilibili.download | NexusPHP | 是 | — | — | ok |  | ok | 2 | 2026-11-23 首次实发 60632（`72889498…`）并补挂校验 100%；站上改写 hash，已加入白名单 |
| sbpt | SBPT | sbpt.link | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 |  |
| shuioudao | 下水道 | sewerpt.com | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 |  |
| soulvoice | 聆音 | pt.soulvoice.club | NexusPHP | 是 | — | — | ok | type 未选 | ok | 1 |  |
| tangpt | 躺平 | tangpt.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| tey | 太乙 | pt.tey.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | 2026-11-23 已实发 `623`，`c9570904…`，补挂 qB 校验 100%；`medium_sel` 仅 WEB-DL/HDTV/DVDR/CD/Track，蓝光源会被门禁拦；qB 老种走 `tracker.tey.cc`，已补别名；站上改写 hash，已入白名单；新种状态“未审” |
| tjupt | 北洋园 | tjupt.org | NexusPHP | 是 | — | — | ok | [tjupt] 表单里没找到「中字」类标签复选框，请发布后手动补 | unsupported | 1 |  |
| ubits | 优堡 | ubits.club | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | ok | 8 |  |
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
