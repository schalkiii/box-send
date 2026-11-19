# 站点总账（Site Registry）

> **一处汇总**：每个参与站点的「域名 / 框架 / 是否启用 / 是否整站暂停 / 是否只停发种 /
> 最近一次实测结论与原因 / pieces-hash 支持 / cookie 条目数 / 关站或人工备注」。
>
> **数据来源**（全部可复现）：
> - 配置：`boxsend.json`（`sourceSites[].enabled`、`assist.pausedSites`、`assist.reseedPausedSites`、`statusNote`）
> - 实测：`dataDir/formcheck.json`（`bsend formcheck` 每次运行自动落盘）
> - pieces：`dataDir/pieces-cache.json`（能力表，实测与运行时学习）
> - cookie：`cookies.json`（按 host 统计条目数，不打印值）
> - 关站：savept.icu 对照结论（见 `doc/REAL-RUN-2026-11-17.md` 第十七节）
>
> **重新生成**：`bsend formcheck`（刷新实测）→ `bsend sites --md > doc/SITE-REGISTER.md`（表格部分）。
> 本文件头部与统计为静态文字；表格永远以命令输出为准。
>
> 统计：147 站｜实测 齐 54 / 缺口 4 / 失败 46 / 未测 43；
> pieces 支持 52 / 不支持 20 / 未测 75。

| 站点 | 名称 | 域名 | 框架 | 启用 | 全停 | 停转种 | 实测 | 实测原因 | pieces | cookie | 备注 |
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
| ptfans | PTFans | ptfans.cc | NexusPHP | 是 | — | — | gap | medium_sel 未选 | ok | 1 |  |
| agsvpt | 末日 | agsvpt.com | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | 未测 | 1 |  |
| aling | 爱玲 | pt.aling.de | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | 未测 | 1 |  |
| audiences | 观众 | audiences.me | NexusPHP | 是 | — | — | ok |  | unsupported | 6 |  |
| baozi | 包子 | p.t-baozi.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| cangbao | 藏宝阁 | cangbao.ge | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| city13 | 13City | 13city.org | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 1 |  |
| crabpt | 蟹黄堡 | crabpt.vip | NexusPHP | 是 | — | — | ok | codec_sel[6] 未选 | ok | 1 |  |
| cspt | 财神 | cspt.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 |  |
| cyanbug | 大青虫 | cyanbug.net | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 |  |
| dicmusic | 海豚 | dicmusic.com | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 3 |  |
| dragonhd | 龙之家 | dragonhd.xyz | NexusPHP | 是 | — | — | ok | team_sel 未选 | unsupported | 5 |  |
| dubhe | 天枢 | dubhe.site | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| fenghuang | 凤凰 | pt.521.best | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| haitang | 海棠 | htpt.cc | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 7 |  |
| haoxue | 好学 | hxpt.org | NexusPHP | 是 | — | — | ok | category_id 未选 | unsupported | 5 |  |
| hdbao | 海德堡 | hdbao.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 |  |
| hdfans | 红豆饭 | hdfans.org | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 5 |  |
| hdtime | 时光 | hdtime.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 6 |  |
| hdu | 好多油 | pt.upxin.net | NexusPHP | 是 | — | — | ok | processing_sel 未选 | unsupported | 5 |  |
| hdvideo | HDVideo | hdvideo.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| iloli | 爱萝莉 | mua.xloli.cc | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | 未测 | 1 |  |
| itzmx | Itz | pt.itzmx.com | NexusPHP | 是 | — | — | ok | team_sel 未选 | 未测 | 5 |  |
| kelu | Kelu | our.kelu.one | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 1 |  |
| kufei | 库非 | kufei.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| lajidui | 垃圾堆 | pt.lajidui.top | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 1 |  |
| lemonhd | 柠檬不甜 | lemonhd.net | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 | savept.icu 同名 LemonHD（lemonhd.club）标注已关站 2026-03-13；本配置域名 lemonhd.net 实测存活（upload.php 200，NexusPHP 表单正常），暂不处理 |
| longpt | 龙 | longpt.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 3 |  |
| luckpt | 幸运 | pt.luckpt.de | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 4 |  |
| nanyang | 南洋 | nanyangpt.com | NexusPHP | 是 | — | — | ok | [nanyang] 表单里没找到「中字」类标签复选框，请发布后手动补 | unsupported | 6 |  |
| nicept | 老师 | nicept.net | NexusPHP | 是 | — | — | ok | type 未选 | ok | 1 |  |
| novahd | nova | pt.novahd.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 |  |
| oshen | 奥申 | oshen.win | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| panda | 熊猫 | pandapt.net | NexusPHP | 是 | — | — | ok | medium_sel[5] 未选 | 未测 | 5 |  |
| ptba | 1PT | 1ptba.com | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 2 |  |
| ptcafe | 咖啡 | ptcafe.club | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | ok | 1 |  |
| pthome | 铂金家 | pthome.net | NexusPHP | 是 | — | — | ok | team_sel 未选 | unsupported | 1 |  |
| ptlgs | 劳改所 | ptlgs.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | 未测 | 6 |  |
| ptsbao | 烧包 | ptsbao.club | NexusPHP | 是 | — | — | ok | processing_sel[1] 未选 | ok | 1 |  |
| ptzone | 葡萄汁 | ptzone.xyz | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| qilin | 麒麟 | hdkyl.in | NexusPHP | 是 | — | — | ok | processing_sel[4] 未选 | ok | 9 |  |
| qingwa | 青蛙 | qingwapt.com | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 2 |  |
| railgun | Railgun | bilibili.download | NexusPHP | 是 | — | — | ok |  | ok | 2 |  |
| sbpt | SBPT | sbpt.link | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 |  |
| shuioudao | 下水道 | sewerpt.com | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 5 |  |
| soulvoice | 聆音 | pt.soulvoice.club | NexusPHP | 是 | — | — | ok | type 未选 | ok | 1 |  |
| tangpt | 躺平 | tangpt.top | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| tey | 太乙 | pt.tey.cc | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | 未测 | 1 |  |
| tjupt | 北洋园 | tjupt.org | NexusPHP | 是 | — | — | ok | [tjupt] 表单里没找到「中字」类标签复选框，请发布后手动补 | unsupported | 1 |  |
| ubits | 优堡 | ubits.club | NexusPHP | 是 | — | — | ok | source_sel[4] 未选 | ok | 8 |  |
| vclib | VC-Lib | pt.vclib.online | NexusPHP | 是 | — | — | ok | type 未选 | ok | 2 |  |
| xdy | 修道院 | xdypt.vip | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| xingtan | 杏坛 | xingtan.one | NexusPHP | 是 | — | — | ok | source_sel[16] 未选 | 未测 | 5 |  |
| xingyunge | 星陨阁 | pt.xingyungept.org | NexusPHP | 是 | — | — | ok | team_sel[4] 未选 | ok | 1 |  |
| ziran | 自然 | naturept.top | NexusPHP | 是 | — | — | ok | type 未选 | 未测 | 2 |  |
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
## 门禁演练（只读，不上传种子）

`bsend gatecheck` 逐站构造字段 + 官方预审（有该接口的站）+ G1–G5 门禁矩阵，
结果落 `dataDir/gatecheck.json`。最近一次：1 站 →
通过 1 / 阻断 0 / 错误 0（错误多为无发布权限/连不通）。

| 站点 | 门禁结论 | 官方预审 | 阻断原因 |
| --- | --- | --- | --- |

通过门禁 ≠ 可立即发种：`assist.publishEnabled` 默认 false，只有用户显式开启、站点在
`autoReseedSites` 白名单内、且日限额未用完时才会真实上传。
