# Changelog

版本以 git tag 为准（Rust workspace 当前 `0.1.0`）。只记录用户可见/行为变化；
逐条技术细节见 `doc/ARCHITECTURE.md`、`doc/OPERATIONS.md`、`doc/ROADMAP.md` 与 git 历史。

## [Unreleased]

### Changed

- **“上传失败”后面的真实原因不再被标题吞掉**（2026-11-27）：UBits 把原因放在 `<h1>上传失败！</h1>` 后面的 `<p>` 里，页面 `<title>` 也含“上传失败”，旧逻辑按关键词截取只拿到标题，审计里只剩“上传失败！ - Powered by NexusPHP”。现在先读 h1 后面的正文，UBits 的真实回包会记成“站点拒绝：检测到种子简介中包含黑名单图床…”，不再需要人工翻 `upload-debug` 才知道原因。新增单测 `upload_failure_reason_after_h1_is_preserved`。
- **重复上传回包里的“该种子已存在！”不再记成失败**（2026-11-27）：站点把这句话放在重填页的普通提示区（页面仍带上传表单）或超过 8KB 的长页面时，现在都按精确完整句判重复；只写“已存在！”的长页面也认。错误框里的“已存在”仍按原逻辑先认，帮助文案里的泛化“提示种子已存在”有反例测试，不会被误判。pthome、qingwa 的历史审计仍是 `failed`，尚未用真实回包重跑，先补单测覆盖。
- **HTTP 200 但回包认不出来时先核对、不直接判失败**（2026-11-27）：上传接口回 `HTTP 200 未识别的返回`（含“回包已存 …”变体）时，程序不再抛“上传失败”，而是先按发布名 + 本地总大小做站内检索；找到就继续下载站点种子并挂回 qB，找不到落 `posted_unresolved` 待人工确认。403/404、空回包和站点明确的拒绝文案仍走原失败口径。city13、hdvideo 这类“站点可能已收下、页面结构认不出”的条目因此不会白丢一次发布额度；真实回包尚未复测。
- **Remux 选档先精确命中 Remux 档**（2026-11-27）：标题/副标题含 REMUX 时，`medium_sel` 先找 `UHD Remux`（4K）或 `BD Remux` / `Blu-ray Remux` / `Remux`（非 4K），站点没有 Remux 档才回退 `BluRay` / `Blu-ray`。修的是 hdfans、kufei 的 `BD Remux` 被 `BD原盘` / `BD DIY` 同名候选卡成歧义而留空，以及 ptsbao 只有 `BluRay` 档时选不上。hdfans 草稿 78 落 `23`、kufei 79 落 `6`、ptsbao 82 落 `1`（processing=Remux、source=电影-Remux）。
- **上传页没有上传控件时不再拿捐款/导航表单顶替**（2026-11-27）：`form_action` 找不到上传表单就返回空，不再退回整页取第一个 `<form>`；静态表单判定只认 file input 或 textarea，只有 hidden/number 的捐款、搜索、登录表单归 `NoForm`。longpt 的 `upload.php` 实测只有 `bonusrain.php` 捐款表单（0 个 file input），现在明确报“目标站没有可解析的静态上传表单，v1 暂不支持”，不会把种子 POST 到捐款接口；`takeupload.php` GET 也只回“上传失败！”页，真实入口仍未定位。
- **检索和详情页超时自动重试**（2026-11-27）：HTTP 检索、详情页、上传页抓取遇到明确的超时/连接重置/连接关闭/管道断开时，最多再试 2 次（共 3 次，间隔 0.5 秒、1 秒）；403、登录墙、站点限流不重试。单测覆盖“第一次超时、第二次命中”的检索路径；dragonhd、ptzone 的真实超时尚未复测。
- **2026-11-27 实发五条**：熊猫 [141840](https://pandapt.net/details.php?id=141840)（《Liar Game S01 2007 2160p WEB-DL》，源 ubits 187730，40,063,028,807 字节 / 13 文件，7 图逐张 200、简介六段齐全，qB hash `d60643d0b6eb0d1759ae0f6c318d72e3647a1ada`，本地构造 hash `2d562e251808618560714b9955986951cf407605`，页面角标 `title="未审"`，单 tracker `tracker.pandapt.net`）；慕雪阁 [59990](https://pt.muxuege.org/details.php?id=59990)（《知否知否》4K WEB-DL H265 60fps，源 soulvoice 54682，128,401,739,986 字节 / 73 文件，源简介只剩 1 张豆瓣海报、抽帧补 3 张 pixhost 截图，qB hash `76692871e8c0ed233f0d7828e1f6d56a7566ae54`，页面角标 `title="未审"`，单 tracker `pt.muxuege.org`）；凤凰 [39168](https://pt.521.best/details.php?id=39168)（《广告狂人 Mad Men S01-S07》，源 ptba 5565，95.10GB / 107 文件，简介 10 图，qB hash 前缀 `f8a8abe66511`）；星潭 [456304](https://xingtan.one/details.php?id=456304)（「初中英语八年级上册【电子课本】」，书籍/PDF 类目，7,830,090 字节 / 1 文件，上传 info hash `7e028f0e3eaf9daf5ee0ea8db4888ed17cf182cf`，pieces 指纹 `608379ab68ae0c441483c264a30f37934f57783f`，候选区待审核）。熊猫、慕雪阁、星潭已挂回 qB 校验 100%、只挂目标站一个 tracker；熊猫、慕雪阁当前 `pending_review` 等审核；凤凰已挂回 qB 做种（`reseeded`）。望雪 [23943](https://www.hxpt.org/torrent/23943)（《知否知否》）走站点接口真发，不是引擎发的；站点 info_hash `85edbcb1738078e8e9b71c7fe9728e3b53d6be5a`，pieces 指纹 `5804349c90ee57d3886f0c01d7b27f96bb6c9001`（与本地一致），73 个 mp4 / 128,401,739,986 字节，5 张图（海报 1、九宫格 1、截图 3）全部 206 存活；上午两次复查仍 `approval_status=0`、收件箱无驳回留言，待审期下载接口回 404，当次没挂回 qB，审核通过后由回访挂回。
- **发种前按 `descrOrder` 自动装配简介分段**：只在站点规则配了 `descrOrder` 时触发，没配的站简介一字不动。源简介六段齐全且顺序正确时原样保留；否则按规则顺序拼段（转载出处 / 制作说明 / 海报 / 简介 / info / 截图），装配后自查段序、图数下限、MediaInfo，缺任一项阻断不提交。熊猫 141840 是首条实测；历史 141276 那版是发到站上之后手工重排的。
- **按站代理**：`sourceSites[].overrides.proxy` 填了代理地址后，只有这一个站的 HTTP 客户端走代理；空/缺省维持直连，行为不变。qB 下载器、CookieCloud、其他站仍直连；日志只记代理 host:port。gtk 已配 `http://127.0.0.1:7890`，走代理链路可达，临时加进白名单等站点发布窗口。
- **站侧阻塞四类从失败口径拆出**：`no_permission` / `two_fa` / `challenge_gate` / `spa_api` 独立审计分类，不进失败冷却，覆盖记录落 `skipped` 中性终态并在备注写“站侧阻塞 + 分类”，`bsend audit` 合计单列“站侧阻塞”行；老的 `review_failed` / `failed` 路径不变、仍进冷却。
- **`announceURL` 主机自动并入允许清单**：配置填了 `overrides.announceURL` 后，该地址主机和页面域名、内置别名、手写 `announceHosts` 合并去重；识别已有做种和上传守卫两处消费同一份清单，没填 announceURL 的站行为不变。修的是 iloli 这类 announce 用独立域（`a.x-anime.cc`）的站：旧逻辑按站点域名匹配，既认不出已有做种，上传守卫又把配置主机当别站拦下。
- **iloli 拒因查清：账号发布记录的二次元浓度门槛，不是同剧重复**：《知否》两次提交都被拒、未建种；回包留档抓到错误框原文——`发布者二次元浓度不足，无法发布该类型资源。当前数量浓度 0.00%（须不低于 50%），体积浓度 0.00%（须不低于 40%）。` 站点上传页规则原文写明：在种子区发非二次元时，二次元条数须 ≥50%、体积须 ≥40%，二次元只算漫画/CG杂图/动漫杂志和动画，非二次元算电视剧/综艺/纪录片/电影，来源写日本（JPN）的不计入。这个号当前做种 163 条（其中动画 15 条、232.94GB），但发布记录 0 条，所以数量/体积都是 0.00%——比例按发布记录算，不是按做种算。回包留档 `artifacts\iloli-upload-1795736380253.html`；在发布记录养到门槛以上前不再试发非动画条目。
- **上传被拒又解析不出原因时，回包正文落临时目录**（已落地，单测覆盖；2026-11-27 iloli 真实拒绝回包已留档实测）：写到系统临时目录 `bsend-upload-<站点id>-<时间戳>.html`，错误信息带路径和剥标签后的可见文字；能解析出具体原因的失败不写临时文件。
- **配置名单更新（2026-11-27）**：自动发种白名单 46 站（新进 panda、iloli、muxuege、fenghuang；gtk 复核后临时加入等窗口；hdu、aling、xingtan、haoxue 复核后移出）；只停发种 35 站，熊猫已撤销发种暂停并留在白名单；整站暂停 8 站（hudbt、freefarm、hdclone、u2、shadow、sunny、piggo、tu88）。freefarm 的 `pt.0ff.cc` 与 `farmm.cc` 是同一站点的镜像，配置维持 `pt.0ff.cc`，不切 `farmm.cc`。

- **上传页先分类，再谈缺什么字段**：新增上传页类型分类器，把抓到的页面归成七类——静态表单 / 无权限 / 登录失效 / 2FA 中间页 / 人机验证拦截 / 单页应用走接口 / 无表单；判定顺序是登录墙 → 权限 → 静态表单 → 2FA → 验证拦截 → 单页应用 → 兜底，**静态表单优先**（页面挂了 WAF 探测脚本但真有表单的站不会被误判）。演练结果落盘到 `formcheck.json` 的 `errorKind`，中止报错带前缀并点名阻塞原因，人机验证/2FA 明确要求"前台真实浏览器、标签页不能最小化"，不再统一糊成"没有可解析的静态上传表单"。2026-11-26 实测 6 站：hddolby 静态表单（17 字段 / 9 下拉，只是要过 2FA）、hdcity 两段式、mypt 时段禁传 + Cloudflare、rousi 单页应用、ptt 与 ttg 是无发布权限。
- **`formcheck` 和 `gatecheck` 用同一套逐站规则**：原先只有 `gatecheck` 加载 `reseedRules` 并跑 `selectPicks`，`formcheck` 不加载，同一个站两边结论打架（演练说 team 没选、门禁说已选）。现在两条链路共用一套规则，单测锁住结论一致。
- **地区下拉、年代档、同名候选都有通用处理**：`source_sel` 在部分站是**地区**不是画质，通用填充按标题/副标题/简介里的地区词配对，配不上落其它；ubits、lajidui 原来写死在站点函数里的地区映射迁到这一层（行为不变）。`processing_sel` 遇到年份语义（如 2015=19）从标题→副标题→简介→MediaInfo 抽发行年份，抽不到留空不硬填。同名全等选项（muxuege 两个 AAC）按页面文档顺序取首个，不再一律判歧义留空。
- **分类联动行不再误报缺口**：xingtan 这类按分类渲染 `mode_<分类id>` 隐藏行的站，隐藏行的空下拉不记缺口（实测 326 条不再误报）；但页面带 `type` 分类下拉的站（iloli、muxuege）不启用这条排除，分支行照常校验。
- **启用 agsvpt（末日）**：`sourceSites.enabled=true` + 进 `autoReseedSites` + `team_sel` 兜底 Other=22。2026-11-26 浏览器实测上传页 200、登录身份 Schalkiii、表单完整无权限拒绝字样，`gatecheck` 全绿（17 字段 / 9 下拉 / 7 项检查）；qB 里 806 条该站 tracker 种子可抄 passkey，不用手填 announce。
- **整站暂停从 13 站降到 8 站**：摘掉 gtk（实测可恢复：登录态有效、上传页 37 字段、共享 tracker `t.myaltbox.com` 有 170 条可抄 passkey，`overrides.announceURL` 已填）和四个已关站 momentpt、movie52、ptlao、yinghua。gtk 2026-11-27 配了按站代理后链路可达，临时加进白名单等发布窗口（见上“按站代理”一条）。
- **3 个表单缺口站配置落地**：iloli `team_sel`=Other 5、muxuege `team_sel`=Other 52 + 普通 AAC=6、fenghuang `team_sel`=other 1；xingtan 隐藏行修掉后不再报缺口；haoxue、hdu 确认是单页应用，aling 是源站自己回 500（不是 WAF），都只记形态不填假字段。

- **站点可以声明“上传必须勾的标签”，没勾上就在提交前拦下**：原先标签只做能填就填、不检查必填；现在站点配置可以写必须勾选的标签（字段名、标签值、说明文案），构造完成后缺哪个就提交前拦下并点名缺哪个；代码层不预设任何站点“一律勾某标签”，AGSVPT 要用就在配置里声明 `tags[4][]` + `52`。2026-11-26 AGSVPT [546963](https://www.agsvpt.com/details.php?id=546963) 因“方舟计划种子未选方舟标签、缺少海报或截图”被拒，按此口径修复。标签值同时纠正：方舟是 **52**；**13 是杜比、51 是合并**，之前代码注释和记录把 13 当方舟，已改正。
- **海报和截图分开算图数底线**：原先只有“总图数”一条底线（藏宝阁、优堡、太乙、麒麟、熊猫、ptlgs 仍是总图数口径，不变）；新增两条独立口径：AGSVPT 至少 1 张海报 + 1 张截图，北洋园（tjupt）至少 1 张图。有独立海报/截图字段的站按字段算，没有独立字段的按“首图算海报、其余算截图”。2026-11-26 实测：AGSVPT [546963](https://www.agsvpt.com/details.php?id=546963) 修到 1 张海报 + 3 张截图；北洋园 [652794](https://www.tjupt.org/details.php?id=652794) 补 1 张海报 + 1 张截图。
- **审核被拒的标记补上 `(禁止)` / `（禁止）` 字样**：原先只认明写“审核不通过”的横幅；现在详情页标题旁标禁止、或写“审核不通过”，一律判审核被拒、落“站点审核没过”，不能再记成“已在做种”。2026-11-26 AGSVPT [546963](https://www.agsvpt.com/details.php?id=546963) 页面标题旁带 `(禁止)`，按此口径记 `review_rejected`。
- **覆盖记录里的详情页链接必须属于目标站**：写入前校验域名，把源站链接写进目标站格子会被拒绝写入并报“链接与站点不符”；草稿阶段存源种链接不受影响。2026-11-26 纠正内容键 `m:5826983b00126e58` 的 agsvpt 格子：原来错写成 `seeding` + 源站 `cyanbug.net` 链接，已改为 `review_rejected` + `https://www.agsvpt.com/details.php?id=546963`。
- **青蛙 Remux 标题片源归一覆盖更多写法**：原先只把 `Blu-ray` 改成 `BluRay`；现在 `Blu-ray`、`Blu-Ray`、`BLU-RAY`、`Blu-RAY`、`blu-Ray`、`Blu ray`、`BLURAY`、`bluray` 都归一到 `BluRay`；`Remux`、`REMUX`、`remux` 都能识别并勾“Remux”标签。2026-11-26 复核青蛙 [208804](https://qingwapt.com/details.php?id=208804)：标题已是 `BluRay`、Remux 标签已勾、页面无审核标记，回访判定干净，覆盖记录 `reseeded`，没有重复提交。
- **回访只回访配置里有的站**：原先 `bsend followup` 的覆盖范围没写清；现在明确只回访配置里有的站，不在配置里的站直接跳过。AGSVPT 不在发布目标里所以一直被跳过——这是它被拒后没人管、覆盖记录还能错写成 `seeding` 的原因之一。2026-11-26 核实。
- **上传失败按回包特征分六类报**：原先所有失败都笼统显示“上传失败”；现在请求层超时/断连报“上传请求没通”（同一字节重投一次），后端空回包明确写“HTTP 200 空回包，通常是站点后端处理超时或崩溃，本次未确认发布”，缺必填项拒绝带片段，种子过大落“待走候选入口”不自动重试，其他站点拒绝取错误框原文、兜底“站点拒绝（回包未展开具体原因）”，登录失效/回表单/未识别各保留原判定。2026-11-26 北洋园两条非空正文“上传失败！”因未留快照记原因待查；跑批前设 `BSEND_DUMP_UPLOAD=1` 才存回包快照，不设事后查不出拒在哪个字段。

- 麒麟（qilin）下载种子的 tracker 实际是 `www.hdkylin.top`，程序原先只认 `tracker.hdkyl.in`，挂回时把本站种子判成“非目标站 tracker”后中止。两个 host 都加入别名；`480763` 重跑挂回后返回“下载器里已有同 hash、只挂目标站 tracker”，覆盖状态从 `review_pending` 变 `seeding`。
- 站点审核标记改为“硬拒绝优先”：页面同时写“审核不通过”和“修改后自动进入待审队列”时，先判拒绝，不再把已拒绝的种子误记成待审。`bsend followup --sites qingwa` 已把 `208692`、`208575` 正确写回 `review_rejected`。
- 青蛙（qingwa）蓝光原盘/DIY 资源改用源页 BDInfo，不再拿本地 MediaInfo 顶替；音频下拉按页面选项选“Other/其他”，真原盘补“原生原盘”标签，Remux 不补。`208692` 的技术栏已换成 692 字节 BDInfo，简介去掉 MediaInfo 参数块。
- 简介图片硬规则补最后一道：剪前有图、剪后 0 张时写入本地阻断缺口，审阅预检显示阻断，`--yes`/自动发种在提交前直接拒绝；白色巨塔、伪装者的“图片全剪光仍能发”路径已堵住。
- 合集标题保留季范围：中文前缀里的 `S01-S07`/`E01-E21` 不再被裁掉，Mad Men 合集审阅标题从 `S07` 恢复为 `S01-S07`。

- 垃圾堆（lajidui）转种自动填三个下拉：`processing_sel` 按容器填 MP4/MKV，`source_sel` 按简介地区填大陆/香港/台湾/日本/韩国/欧美/Other，`team_sel` 先按名尾制作组匹配，匹配不到选 Other。2026-11-26 已发 [85084](https://pt.lajidui.top/details.php?id=85084)、[85085](https://pt.lajidui.top/details.php?id=85085)、[84999](https://pt.lajidui.top/details.php?id=84999)、[85000](https://pt.lajidui.top/details.php?id=85000) 用 `edit-seed.py` 先演练后提交：85084/85085 补到 4 图，84999 填 MP4/大陆/Other，85000 标题改为 `Mad Men S01-S07` 并填 MKV/欧美/FRDS；四条仍等站点审核。

- 青蛙（qingwa）Remux 资源标题按站点规范把 `Blu-ray` 写成 `BluRay`，并在表单有「Remux」复选框时自动勾选；2026-11-26 `208804` 已按此改好，站内信 `643794` 确认审核通过，qB 本来就在做种。

- 源标题清洗修正两处：中文名后面直接跟 `1080p`/`HDTV`/`x265` 这类技术词时，不再把中文名裁掉；`[50%] (通过)` 这类“促销角标 + 审核提示”会一起从标题尾部剥掉，不再把角标带进发布名。白色巨塔、Liar Game 的转种审阅实测由错误标题恢复为正常标题。

- 图片检查覆盖 BBCode（`[img]` / `[img=尺寸]`）和 HTML 两种写法；`hdsky.me`、`img.hdsky.me`、`imgur.com` 源站热链即使本机 200 也按死图剪掉，UBits 另把 `imgbox.com` 按死图处理。新增 `scripts/rehost-descr-images.py` 把简介图片统一转存 pixhost。2026-11-25 藏宝阁 44189 因 3 张 hdsky 图被拒，已换 pixhost 等复审。

- 上传超时可按站点配置：`assist.reseedRules.<site>.uploadTimeoutSec`（缺省 120 秒）。2026-11-25 tjupt 真发《我们的父辈》25GB 成功并挂回 qB（`652794`，校验 100%，等站点审核）；同一批 274GB《波洛全季》合集上传 5 分钟仍回 HTTP 200 空正文、站上无条目，属于站点后端处理超时，重试仍会空回包，要发得先拆小或换内容。

- 挂回校验不再要求 `info hash` 或标题与上传构造一致：只核对 pieces 指纹和文件清单（比较时忽略首段根目录名），对得上就挂回；站点改写 hash/标题时白名单只用于日志标注“已实测改写”，不再参与放行或拦截。pieces 或文件清单对不上仍拒绝挂回。
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
- **源站简介没有图时自动补图**：新增 `scripts/seed-images.py`，用本机 ffmpeg 从视频抽帧，也可按豆瓣条目自动取海报（`--douban`）或传海报 URL，上传到 pixhost（imgbox 仅历史兼容），写入 `screenshot-overrides.json`；发布时程序自动把这些图追加到简介。波洛候选 carpt 532 已补 3 张，tey 预检从“0 图”变为“3 张 ≥ 3”；tey 仍未上传，是站点 `medium_sel` 没有 Bluray 选项，不是图床。
- **图片门禁改成代码硬规则**：剪死图不再读配置，所有站点上传前无条件逐图探测、剪掉死图；要图的站有硬性最低图数（`gates::image_floor`：藏宝阁/优堡/太乙/麒麟 3 张，熊猫 4 张，ptlgs 4 张 = 1 张海报 + 3 张截图），配置里的 `pruneDeadImages` / `requireImages` 已删除、写了也会被覆盖。
- **上传成功就立刻挂回 qB 改成硬步骤**：上传后立即下载站点 `.torrent`、核对分片并加入 qB；站点刚建种时的 `torrent not registered` 固定重试 4 轮、每轮 15 秒，仍拿不到才落待人工。所有站点、所有发种入口都走这条，没有配置开关。
- **熊猫暂停发种**：`panda` 从 `assist.autoReseedSites` 摘掉并列入 `assist.reseedPausedSites`；站上 141276 是候选种子 24 小时没做种被系统删的，重发需先“上传→立刻挂回 qB 做种”。
- 2026-11-24 晚把《伪装者》同一内容的另外四站简介死图换掉：crabpt 240242、hdfans 287791、qingwa 208575、luckpt 61102 的 9 张 `i13.tietuku.com` 全部换成 imgbox，复查 0 张旧图 / 9 张新图；sbpt 21684 本来没有图片，不需要换。

### Added

- **`bsend reseed` 新增 `--site <目标站>` 和 `--only <名称/内容键子串>`**（2026-11-27）：`--site` 先按目标站筛草稿，解决同一内容在多个站有缺口时 `--only` 先命中别站的问题；`--only` 不再靠草稿编号选条，编号漂移不会发错。iloli 已定为二次元站：`selectPicks.type=405` 只在自动分类落“动漫”含糊分支时纠正，电影/电视剧不套用；`allowedTypes=["动画"]` 落成硬缺口，手动 `reseed` 也拦。首条动画《Ghiblies Episode 2》真发 https://mua.xloli.cc/details.php?id=56513 ，`type=405 动画 (Animation)`、4 张 pixhost 图、qB 校验 100%；非动画《知否》复测被白名单阻断，未提交。

- `scripts/replay-reseed.py`：按草稿号跑 `bsend reseed --draft <N>` 且不提交，用 `--expect` 断言提交字段/图片数/标题，用 `--reject` 挡住旧问题回归；修完问题先回放，再执行提交。
- `scripts/edit-seed.py` 新增 `--processing`（改 MP4/MKV 容器下拉）、`--team`（改制作组下拉）和 `--append-image`（按 `[img]` 追加图片，已有同链接就跳过）。
- `scripts/seed-images.py` 新增 `--also-content`（同一剧多个内容键逐个补图）和 `--for-name`（按覆盖记录里的内容名子串找齐所有键）；仍默认上传 pixhost、默认 `--dry-run` 只抽帧不上传。
- 上传页独立海报/截图字段：`sourceSites[].overrides.posterField` / `screenshotField` 指定字段名后，程序从最终简介（剪死图、转 BBCode 之后）按出现顺序抽图，第一张填海报，其余一行一个填截图；表单没有该字段不硬塞，简介没图时清空，避免提交死链。ptlgs 的 `cover` / `screenshots` 是首个实测站，总图数按 4 张进 `image_floor`；2026-11-25 真发《苦月亮》`115350`，详情页海报和截图区块都有图，qB 校验 100%。

- `scripts/edit-seed.py --clear-technical-info`：清空 `technical_info` 字段，配合 `--descr-file` 处理不收 MediaInfo 的站点（财神 260049/260050 实测）。
- `scripts/edit-seed.py` 扩展：`--technical-info-file`（别名 `--bdinfo-file`）写技术栏，空文件拒绝提交；`--add-tag`/`--remove-tag` 按页面真实字段（`tags[]`/`tags[4][]`）增删标签并保留原有勾选；音频/片源/编码/分辨率按字段前缀定位，写入前校验 option value 存在；只改简介/技术信息时不会再清空标签。
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

- 上传成功但校验没跑完时不再写“转种完成”：新增 `posted_verifying` 审计词（界面写“已发布，种子已加入下载器，校验还没跑完，不能算已在做种”），日限额照常算一次上传；旧版和普通发布共用 `Posted`，用户会以为已经在做种。
- 上传请求网络层超时或断连时同一字节重投一次，第二次仍不通才报失败；旧版第一次超时就直接退出，tjupt 这类慢站没有第二次机会。
- HTTP 200 但正文 0 字节不再混成“未识别的返回”，明确写“站点后端没有返回内容，通常是处理超时或崩溃；本次未确认发布”。tjupt 274GB 合集 5 分钟空回包是首个现场。
- 审核待定/审核不通过但 note 明写“已挂回 qB”的，转种日志 `seeded` 记为 true；不再因为格子状态不是 `reseeded` 就写成没做种（tjupt `652794` 已挂回、校验 100%，旧逻辑会写 `seeded=false`）。
- 发后挂回返回 `same_hash_skipped` 时不再包成 `posted`：站上发布成功，但下载器里同 `info hash` 的种子没有只挂目标站 tracker，qB 不允许同 hash 存两条，按策略不改原种，当前并没有做种。旧版把任何非 `Reseeded` 的结果都写成 `posted`，日志出现 `posted + same_hash_skip`、`seeded=false`（2026-11-22 熊猫 10265 现场）；现在按 `same_hash_skipped` 落账，note 明确写“当前没做种，待人工处理”，日限额也不把这条算成已发布。
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
