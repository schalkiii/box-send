# 新站接入与上传失败排查

这份文档讲怎么把一个新站接进来、上传被拒时从哪查、已经查过哪些站。站点清单和白名单见 `SITES.md`，整体发种流程见 `ARCHITECTURE.md`。

## 1. 什么时候用

- 新站第一次接入；
- 老站改版，原来的字段或下拉选项失效；
- 上传被站点拒绝，需要找原因。

2026-11-22 以后，不再为了判断“站上有没有”去逐个站点搜索。表单检查（命令 `formcheck`）只允许新站第一次接入时做一次，用来拿上传页字段和下拉选项；日常不要批量探测。

## 2. 先用哪些命令

| 命令 | 用途 | 会不会改站点 |
| --- | --- | --- |
| `bsend scan` | 建立或刷新 qB 内容索引 | 只读 qB |
| `bsend formcheck --sites a,b` | 拉上传页，核对字段和下拉选项 | 只读站点，不提交表单；仅新站首次 |
| `bsend layoutcheck --limit N` | 核对种子加入 qB 后的目录布局 | 只读 qB |
| `bsend plan --only 名称` | 预演一轮，只出结论 | 读站点；过渡期命令，会逐步退役 |
| `bsend reseed --draft N` | 审阅一条待发，跑预检、官方预审、字段检查 | 读站点 |
| `bsend reseed --draft N --yes` | 真正上传 | 写站点 |
| `bsend gatecheck` | 查看发布检查规则矩阵 | 只读 |

## 3. 接入一个新站要做什么

### 第零步：填好该站 tracker 地址

第一次向这个站发种时，qB 里还没有该站种子，程序抄不到带 passkey 的 tracker。先在配置里该站的 `overrides.announceURL` 填完整地址（从站点“做种/下载”页面复制），再做后面的检查。没填会直接中止上传，不会伪造一个没有 passkey 的种子。

### 第一步：能不能连上

检查域名解析、TLS、首页和上传页的 HTTP 状态。

- 域名解析失败：按关站处理；
- 连接超时：只标“连不通”，不要写成“站上没有这条内容”；
- 有证书错误：先确认是不是本机代理或证书链问题，不要直接放弃。

### 第二步：登录和权限

带 Cookie 打开上传页。

- 看到密码输入框：Cookie 失效，整站暂停；
- 看到“没有发布种子的权限”（简体和繁体都要认）：账号没有发布权限，只停发种，辅种照常；
- 正常看到上传表单：继续下一步。

### 第三步：找到真正的上传表单

页面上可能有多个表单。按下面的顺序打分定位：

```
有 type="file" 的 > action 含 takeupload 的 > action 含 upload 的
```

导航栏的搜索表单经常混进来，早期有 9 个站因为把搜索表单当上传表单，字段类型全错。找不到静态表单，说明是 JavaScript 动态上传或自定义界面，当前版本不支持，记录下来即可。

### 第四步：核对必填字段

重点看：分类、媒介、编码、分辨率、音频编码、标题、简介、MediaInfo。

- 页面没有标题输入框：标题由站点自动生成，不算缺口，发完再核对；
- 缺 MediaInfo：可以用本机的 `MediaInfo.dll` 生成；
- 缺简介：先看源站有没有简介，不要自己编。

### 第五步：核对下拉选项

每个站的下拉选项都要拉真实值，不要凭字段名猜。

- 一个页面有多组同类下拉（例如 `medium_sel[4]` 和 `medium_sel[9]`）：每组独立配对，不能只配第一组；
- 站点写的标签和本地不同（例如站内写 `TV`、分辨率只写 `1080`）：用别名表吸收；
- 同名但含义不同（例如 `source_sel` 在某个站其实表示地区）：先判断和影视域有没有关系，无关就跳过；
- 同名重复项（两个 `AAC` 没有其他属性区分）：留空，并把歧义写进结论，不要猜。

### 第六步：跑一次只审不发

用 `bsend reseed --draft N` 看三处：

1. 本地预检：标题、简介、MediaInfo、截图是否齐；
2. 站点官方预审：有些站有自己的检查接口；
3. 将要提交的字段：尤其是多组下拉和必填项。

源站缺 MediaInfo 时用本机 DLL 生成；完结剧集要勾“完结”；缺链接按 IMDb、豆瓣、TMDB 补。

### 第七步：把结论写回站点表

只改 `SITES.md` 对应行的备注，不新建按日期命名的报告。原始表单输出留在 `formcheck.json` 里，方便下次对比。

## 4. 常见原因

| 原因 | 怎么认 | 怎么处理 | 例子 |
| --- | --- | --- | --- |
| 账号没有发布权限 | 页面出现“没有发布种子的权限”，简繁体都有 | 只停发种，辅种保留 | hdsky、pter 等 29 站 |
| Cookie 失效或 403 | 跳登录页、返回 403 | 整站暂停，重新同步 Cookie | piggo、tu88 |
| 域名失效或关站 | 域名解析失败 | 标记关站并暂停 | movie52、yinghua |
| 站点换形态 | 首页正常，上传页 404 | 按关站或待重做处理 | ptlao |
| JavaScript 动态上传 | 没有静态表单，或只有文件框和隐藏字段 | 当前版本不支持 | ttg、keepfrds、hdcity |
| 搜索表单混入 | 字段类型变成搜索选项 | 按表单特征重新定位 | 早期 9 站 |
| 多组参数化下拉 | 同名字段带不同编号 | 每组独立配对 | ptba、iloli、panda |
| 选项文字不同 | 站内写 `TV`、裸数字 `1080` | 加别名映射 | aling、ptcafe、ptsbao |
| 同名不同义 | `source_sel` 实际是地区 | 判断域相关性后跳过 | aling、ubits |
| 同名重复选项 | 两个 `AAC` 无法区分 | 留空并记录歧义 | muxuege |
| 站点改写 `info.source` | 站上 hash 与上传构造 hash 不同，pieces 和标题相同 | 只对实测确认的站放行；其他站仍拒绝，防错种 | ptba、cspt、dubhe |
| 站点不收这类内容 | 站内同媒介 0 条 | 老实记缺口，不硬塞 | ptfans |
| 垂直站类目不符 | 只有曲艺、电子书、音乐 | 软提示，不算表单缺口 | haitang、kelu、dicmusic |
| 标题自动生成 | 没有标题输入框 | 不算缺口，发后核对 | tjupt |
| 不是 NexusPHP | 字段结构完全不同 | 当前版本不参与 | 17 个其他框架站点 |
| 字符边界错误 | 程序 panic 或选错值 | 按字符边界截断 | xingtan |
| 把网页描述当字段 | `<meta description>` 被读成上传字段 | 只认 `input`、`textarea`、`select` | hdcity |

## 5. 反复验证过的经验

- **先看结构，再看文字**：找上传表单看 `type=file` 和 `takeupload`；文字只用来判断权限和限流。
- **先拿证据再改映射**：改任何选项前，先拉真实的 `<select>`、字段名、权限页。没有证据不要猜。
- **用候选链吸收差异**：标题字段 `name → title`，副标题 `small_descr → subtitle`，简介 `descr → release_desc`。不要为单个站写死分支。
- **允许三种降级**：有歧义就留空；和影视无关就跳过；无法验证就回到旧行为。
- **MediaInfo 用 DLL**：图形界面版不能无界面运行；剧集包取单集或最大的视频文件做代表。
- **发完立刻挂回**：上传成功后马上从目标站下载种子并加进 qB。qB 5.3 已经移除 `skip_checking`，实际用 `seedMode=true` 和 `paused=false`。
- **处理 Cloudflare**：它会对表单里的内联 `on*=` 属性计数，4 处就可能 403；提交前清掉事件属性。
- **不要信单个回包 id**：luckpt 实测回包 `id=10230`，真实种子是 `61140`；多条新种的初始详情页打不开。发后只从回包正文或最终跳转里找链接，并且详情和下载的 id 必须对得上；对不上就记 `posted_unresolved` 等人工核对，不额外搜索。
- **停用要分层**：`pausedSites` 整站停；`reseedPausedSites` 只停发种；关站另外写 `statusNote`。
- **`existed=1` 不是作者证明**：它只说明站点上已有同 `info hash` 的条目。判断是自己发的还是别人发的，必须看详情页“由 … 发布”和发布时间；sbpt 七条实测作者全是本站账号 `Schalkiiii`，不能叫成“别人发的可辅种”。
- **简介有图不等于独立字段有图**：ptlgs 这类站把海报、截图放在 `cover` / `screenshots`，数值存在 `required` 也不代表内容被填了；上传页有字段就必须从最终简介抽图填进去。详情页 `ktorrentcover` 为空、简介 0 张图，就是没填的实证，不是站点不显示。
- **门禁分三层看**：全站硬规则（单 tracker、逐图剪死图、最低图数、上传后立刻挂回）、按站配置（`titleNoCjk`、`descrOrder`、必填字段、选档、日限额）、站点侧限制（没有 Blu-ray 档、种子文件上限、图床黑名单、Cloudflare）。前两层能改代码/配置解决，第三层只能绕开发种对象或换入口，不能硬填。

## 6. 已经查过的站

### 6.1 hdvideo

- 现象：提交后被通用“上传失败！”拒绝，文件字段和 token 都排除了。
- 原因：必填的地区字段 `region_sel[4]` 没选、题材多选 `style_sel[4][]` 没勾。
- 处理：配置里补 `region_sel[4]=2`（美国）和 `style_sel[4][]=6`（剧情）；调试时设 `BSEND_DUMP_UPLOAD=1`，会把上传回包和表单落盘。
- 结果：发布检查 8/8 通过，回包明文“发布成功！”，新种 `78856`（11.56 GB，1080p，国语/中字/HDR10）。这是必填项缺失导致的拒绝，不是文件字段问题。
- 补挂：上传回包把新种 `78856` 和相关旧种 `14469` 混在一起，旧的发后定位取错成 `14469`，qB 里没有挂上 78856。代码已改为按 id 配对详情/下载（详见 `ROADMAP.md` 4.3）。2026-11-23 已从详情页 `https://hdvideo.top/details.php?id=78856` 下载目标种补挂，qB hash `dd7f6097…`、校验 100%、状态 `stalledUP`；站点种自带 `tracker.hdvideo.top` + `hdvideo.top` 两条同站 announce，保留原样。不要重复上传。

### 6.2 luckpt

- 现象：站点自带的发布检查（LuckAudit）95/100；BDRip 的媒介和分辨率选不中；截图数量不足。
- 图片死链原因：豆瓣图床防盗链，补 `Referer` 后从 418 变 200；`img.pterclub.com` 在本机 TLS 连不上，剪掉死图后截图仍不够。不要靠关闭“清理死图”来掩盖。
- 表单实测：提交地址是 `https://pt.luckpt.de/takeupload.php`，18 个字段、9 个下拉；`medium_sel[4]` 没有 `BDRip`，BDRip 归在 `Encode(7)`，现有配置已经是 7。
- `standard_sel`、`codec_sel`、`audiocodec_sel` 和具体内容有关，不要写死一个值；等真实 BDRip 待发条目试运行时再复验。
- `team_sel` 只是软提示；可以兜底成 `Other(5)`，但会丢制作组信息，默认不填。

luckpt 的候选规则（**还没有写进 `boxsend.json`**）：

```json
"reseedRules": {
  "luckpt": {
    "selectPicks": {
      "medium_sel": "7"
    }
  }
}
```

### 6.3 qingwa

665/667 条有记录，约 99.7%。没覆盖的是成人合集和多片合集，前者被内容黑名单拦掉，后者不是适配器故障。保留白名单，不再投入映射修复。

- **Remux 标题与标签（2026-11-26 站内信实测）**：`A Touch of Zen 1971 1080p Blu-ray REMUX AVC LPCM 1.0-WScode` 被拒，原因两条：片源必须写 `BluRay`，不能带 `Blu-ray` 的横杠；媒介是 Remux，必须勾「Remux」标签。已在 `edit.php?id=208804` 把标题改成 `... 1080p BluRay REMUX ...` 并勾上 `tags[4][]=15`，页面提示“编辑成功”，标签显示 `中字 国语 Remux`。站内信 `643794` 后续确认该条审核通过，qB 本来就在做种。代码同步加 `normalize_title_for_site`（只对 qingwa 把 `Blu-ray` 写成 `BluRay`）和 `wanted_tags` 的 Remux 自动勾选，避免下一条再犯。

### 6.4 haitang 和 haoxue

- haitang：只有曲艺、有声书类目，和影视范围不匹配，已移出白名单，规则留档。
- haoxue：Next.js/Flux 单页应用，静态提交地址为空，旧检索路由 404；需要按前端接口重做，已移出白名单。

### 6.5 agsvpt、tey

- agsvpt：提交域和登录域不一致，返回“你没有该权限”并跳登录页；2026-11-23 已 `enabled=false` 并移出自动发种白名单。
- tey：发布检查曾报 `medium_sel` 没选，根因是该站 `medium_sel[4]` 只有 WEB-DL/HDTV/DVDR/CD/Track，蓝光/Remux 源没有对应项；HDTV/WEB-DL 源可正常选档。2026-11-23 已用 HDTV 源实发成功，见 6.7。

### 6.6 qilin 和 panda

站上其实已经发布（qilin 479362、panda 141276），但旧逻辑拿旧种或错误详情页当结果，记成了失败；新 hash 不在 qB。这是发后定位错误，不是上传失败。新版流程已经改成从回包正文/跳转里按 id 配对详情和下载链接，再下载目标种独立挂回；发后定位缺陷已修（见 `ROADMAP.md` 4.3）。2026-11-23 已补挂：qilin 479362（hash `bd348dfc…`，站点审核仍显示缺图，不影响做种）、panda 141276（hash `3f8ed774…`），两条 qB 校验 100%。panda 的审核拒绝（主标题中文/简介顺序）已在 6.11 修复并重提，等复审。2026-11-24 回访：详情页报“没有该ID的种子”，重提的版本没留在站上，覆盖记录已改 `gone_from_site`；qB 里那条还在。

### 6.7 2026-11-23 新实发：ptba、cspt、dubhe、ptsbao、crabpt、hdbao、tey、oshen、ptfans、itzmx

十站都是第一次向目标站发种，上传本身成功；站上返回的 `.torrent` hash 与本地上传构造的 hash 不同。ptba 已确认是站点改写了 `info.source`；cspt、dubhe、ptsbao、crabpt、hdbao、tey、oshen、ptfans、itzmx 的 pieces 和 `info.name` 未变，但具体改写字段没有逐字节核对。**这是 2026-11-23 当时的规则**：程序默认拒绝 hash 不一致，十站经 pieces 和 `info.name` 核对后加入白名单，后续同站发种直接放行。现行规则已改为只核对 pieces + 文件清单，白名单只用于日志标注，见 `ARCHITECTURE.md` 5.3。

- **ptba**：源 `luckpt` 的 `金枝欲孽`（51.9 GB）→ `https://1ptba.com/details.php?id=106718`，站上 hash `abf41c82…`，构造 hash `4f8b63e2…`。站上 `info.source` 为 `[1ptba.com] 1PTBA.COM`，本地构造写的是 `ptba`。已把站点种子下回来挂进 qB，qB 校验 100%。
- **cspt**：源 `ptba` 的 `Mad Men S01-S07`（95.1 GB）→ `https://cspt.top/details.php?id=259870`，站上 hash `cf568e1a…`，构造 hash `516522da…`。第一次补挂被 tracker 校验拦住：站上种子的 announce 在 `tracker.cspt.cc`，页面域是 `cspt.top`，`extra_announce_hosts` 缺别名。补 `cspt => ["tracker.cspt.cc"]` 后重新补挂成功；站上种子自带两条同站 announce，保留原样。
- **dubhe**：源 `soulvoice` 的 `知否知否应是绿肥红瘦`（119.6 GB）→ `https://dubhe.site/details.php?id=55492`，站上 hash `b5ebc539…`，构造 hash `0e53fd1b…`。第一次判重搜索 30 秒超时，重试才通过；表单里没有“完结”标签复选框，发布后需要人工补。已把站点种子下回来挂进 qB，qB 校验 100%。
- **ptsbao**：源 `soulvoice` 的同一条《知否知否应是绿肥红瘦》→ `https://ptsbao.club/details.php?id=135787`，站上 hash `0b0087f7…`，构造 hash `bf6e43ae…`。`source_sel[1]` 是二级分类（`剧集-美剧` / `电影-WEB-DL`），旧逻辑会按“WEB-DL”错填成 `电影-WEB-DL`；已改成先识别二级分类、type 选完后再按父子关系填，实发时选到 `剧集-大陆(23)`。已把站点种子下回来挂进 qB，qB 校验 100%；表单没有“完结”标签，发布后需要人工补。
- **crabpt**：源 `hdsky` 的 `The Disguiser`（88.9 GB）→ `https://crabpt.vip/details.php?id=240242`，站上 hash `732e2c99…`，构造 hash `fe9204e8…`。此前候选《人民的名义》卡在源站 `hdfans` 502（当时按 6 小时失败冷却被挡住），这次改选 `hdsky` 源后一次通过；站上已有同片 2160p 其他版本 214580，按策略继续发布。已把站点种子下回来挂进 qB，qB 校验 100%。
- **hdbao**：`autopost --sites hdbao --limit 1` 实际发了 2 条：源 `ptba` 的 `Mad Men S01-S07`（102.1 GB）→ `https://hdbao.cc/details.php?id=54402`，站上 hash `95eacf24…`，构造 hash `e88d740e…`；源 `hdfans` 的《人民的名义》（95.8 GB）→ `https://hdbao.cc/details.php?id=54403`，站上 hash `51ededb2…`，构造 hash `1f8a0104…`。两条都先落 `posted_unresolved`，挂回后 qB 校验 100%。`--limit` 少算 `posted_unresolved` 的缺陷已修，后续同站不再连发。
- **tey**：源 `hdsky` 的 `The Disguiser`（HDTV 1080p，82.8 GB）→ `https://pt.tey.cc/details.php?id=623`，站上 hash `c9570904…`，构造 hash `465916fd…`。该站 `medium_sel[4]` 没有 Blu-ray，蓝光/Remux 候选会被选档门禁拦住；这次改用 HDTV 源才通过。qB 里 5 条老种全走 `tracker.tey.cc`，页面域是 `pt.tey.cc`，补 announce 别名后 qB 覆盖从 0 变 5，也能抄到 passkey。上传回包同样没给详情链接，落 `posted_unresolved`；挂回后 qB 校验 100%。新种在站上暂时显示“未审”，等站点审核。2026-11-24 回访仍未审，覆盖记录落 `pending_review`，由回访接着跟。
- **oshen**：源 `soulvoice` 的《知否知否应是绿肥红瘦》（119.6 GB）→ `http://www.oshen.win/details.php?id=57082`，站上 hash `22e3dd63…`，构造 hash `0f73ae73…`。前两次分别断在“send failed because receiver is gone”和判重搜索超时；第三次站点恢复，判重正常、上传成功。回包没给详情链接，落 `posted_unresolved`；挂回后 qB 校验 100%，tracker 为 `https://www.oshen.win/announce.php?...`。表单没有“完结”标签，发布后需要人工补。
- **ptfans**：源 `carpt` 的 `Agatha.Christies.Poirot.S01-S13.Collection`（294.3 GB）→ `https://ptfans.cc/details.php?id=11111`，站上 hash `fdf27b29…`，构造 hash `4fd4392f…`。此前上传页和 IMDb 判重都超时；2026-11-23 站点恢复后一次通过。站上已有 720p 其他版本 `6095`，按“同名其他版本照样发”的策略继续上传。回包没给详情链接，落 `posted_unresolved`；挂回后 qB 校验 100%，tracker 为 `https://ptfans.cc/announce.php?...`。表单没有“完结”标签，发布后需要人工补。
- **itzmx**：源 `ubits` 的 `Liar.Game.S01.2007.2160p.WEB-DL.H264.AAC-ADWeb`（40.0 GB）→ `https://pt.itzmx.com/details.php?id=13134`，站上 hash `88e3103b…`，构造 hash `7744200c…`。此前大合集候选都被站点以“种子文件过大”拒绝；改用这个源种只有 96,934 bytes 的条目后一次通过。上传回包没给详情链接，落 `posted_unresolved`，但 `reseed-log.jsonl` 已记录详情页；补 `itzmx` 到 hash 改写白名单后挂回，qB 校验 100%，tracker 为 `https://pt.itzmx.com/announce.php?...`。详情页没找到“中字/完结”标签，发布后需要人工补。

十站共同的结论：站点改写种子元信息是正常行为，不能把 hash 不一致一概当错种；但只对实测确认的站放行，且必须 pieces 和标题同时一致。新增站先按默认严格模式跑，出现 hash 不一致再核对，不要预先扩大白名单。

### 6.8 itzmx 的 torrent 文件上限（已绕过）

- 站点 `takeupload.php` 对 `.torrent` 文件有硬上限 **266,240 bytes**，超过时返回 HTTP 200 和错误框：`上传失败！ 种子文件过大！最大允许266,240 bytes。请使用更大的区块大小重新制作种子文件，或者将内容分为多个种子发布。` 回包证据在 `upload-debug/itzmx-1795436357-a0.html`。
- 早前 Poirot 合集、周星驰合集两次返回 Kangle `HTTP 403 access denied by input filter`（`upload-debug/itzmx-1795423111-a0.html`），容易误判成 WAF 规则；后续用非合集候选拿到 200 业务错误后才确认真正门槛是**文件大小**，403 更像超大请求体触发的前置拦截。
- 解决方式不是重做分片，而是换一个源种本身足够小的候选：`Liar Game` 的源种 96,934 bytes，构造后仍远低于上限，上传一次成功。大合集/大剧包仍会被 266,240 bytes 卡住，后续给该站选种时先看 `.torrent` 文件大小，不要只看内容体积。
- 上传错误文本提取还暴露了一个 Rust panic：错误文本含中文时 `body[p..p+120]` 会切在 UTF-8 字符中间。已改为 `floor_char_boundary` 并加回归测试 `error_extraction_does_not_split_utf8`，避免再把业务错误变成进程崩溃。

### 6.9 cangbao（藏宝阁）

- 现象：`Quick Change 1990 BDRip X264 iNT-TLF`（43994）审核拒绝：未填写媒介类型、未选择分辨率、简介未包含图片。
- 媒介：标题只有 `BDRip`/`X264`，旧映射没把 BDRip 当光盘源，`medium_sel[4]` 留空。BDRip + x264 是重编码，站点 `Encode(3)` 才对。
- 分辨率：标题没有 `1080p`/`720p`，旧逻辑不从 MediaInfo 取值；实际 MediaInfo 是 1290x696，按长边归 `720p(3)`（裁黑边的 720p 编码）。
- 简介：站点简介框是 BBCode 编辑器，源站 hdfans 的简介是裸 HTML；原样提交后数据库里是 `&lt;fieldset&gt;…`，页面显示字面标签，审核端看不到 `<img>`。代码已改为默认把 HTML 简介转 BBCode（`descrFormat=html` 才保留 HTML），43994 已用 `[quote]`/`[img]` 重新提交，详情页正常渲染。
- 2026-11-23 抽查 29 条已发布种子，lajidui、ptcafe、sbpt、luckpt、ptlgs、tangpt、longpt、cyanbug、kufei、xingyunge、city13、shuioudao、xdy、baozi、novahd、ptzone 等站的历史简介也是字面 HTML；已用 `scripts/fix-bbcode-descr.py` 批量改成 BBCode，只改简介、不换种子文件，29 条复查全部为 BBCode。
- 修复后先只编辑 43994，不新发种；2026-11-23 已重提，等复审再考虑恢复藏宝阁自动发种。
- 2026-11-23 复审仍拒绝，原因变成“简介图片全部无法访问”。用登录 Cookie 读 43994 编辑页确认简介只剩两张图：豆瓣海报 `img9.doubanio.com` 裸请求 418/HTML 防盗链，`imgs.loukky.com` 本机 TLS 失败；都不可达。已用 imgbox 重传海报/截图（`images4.imgbox.com/f6/3a/MpwvlgiL_o.jpg`、`/af/5c/Z9Nm7U76_o.jpg`），带藏宝阁 Referer 均 200 image/jpeg，`edit-seed.py` 只改简介后详情页两张图正常显示。等站审核，不再私信管理。2026-11-24 回访：详情页审核图标已标 `title="通过"`，审核已过。
- 发种前门禁已加（当时按配置；2026-11-24 晚起改为全站无条件硬规则）：逐图探测死图，少于图数门槛直接拦；不再出现“图片全死但照样提交”。

### 6.11 panda（熊猫高清）审核拒绝

- 141276 `Star Wars Episode IX ... MNHDR-FRDS` 被 `NoCaps` 拒绝：主标题不得含中文、简介框要按【转载出处-制作说明-海报-简介-info-截图】顺序。
- 旧标题从源站带回了 `[50%] 剩余时间：26年4月`，中文促销尾巴没被剥掉；已在编辑页改成纯英文标题，中文只留在副标题。
- 源简介只有 `转自KEEPFRDS` + 豆瓣文字 + 两张空 `fieldset`，没有海报/截图，也不符合顺序。已重写为六个显式标记段落：转载出处 → 制作说明 → 海报（imgbox）→ 简介 → info → 截图（3 张 imgbox）；4 张图都带熊猫 Referer 实测 200 image/jpeg。编辑已提交，详情页图片和段落顺序可读，等站审核。
- 门禁已加（当时按配置；2026-11-24 晚起图门禁改为代码硬规则）：`titleNoCjk=true`、剪死图、至少 4 张图、`descrOrder` 六个标记；源简介缺图/乱序会在上传前被拦，不再靠人工事后补。


### 6.10 2026-11-23 继续实发：railgun、lemonhd

- **railgun**：源 `carpt` 的 `Agatha.Christies.Poirot.S01-S13.Collection`（294.3 GB）→ `https://bilibili.download/details.php?id=60632`，站上 hash `72889498…`，构造 hash `6debbce8…`。目标站已有其他版本 4414，按策略继续发布；上传回包没给详情链接，落 `posted_unresolved`；补 `railgun` 到 hash 改写白名单后挂回，qB 校验 100%，tracker 为 `https://bilibili.download/announce.php?...`。第一次挂回读 tracker 列表时 qB 返回 404，日志“tracker 列表核对跳过”；已确认这是刚 add 后的瞬态，挂回读列表增加 3 次短重试。
- **lemonhd**：同源同内容 → `https://lemonhd.net/details.php?id=48970`，站上 hash `3c00cbb2…`，构造 hash `d50dfa19…`。站上已有其他版本 13452，按策略继续发布；上传回包没给详情链接，落 `posted_unresolved`；补 `lemonhd` 到 hash 改写白名单后挂回，qB 校验 100%，tracker 为 `https://tracker.lemonhd.net/announce.php?...`。
- 两站都只改写了元信息，pieces 和 `info.name` 与本地源种一致；当时仍按“白名单 + pieces + 标题”三重校验放行。现行规则已改成 pieces + 文件清单，白名单只记日志。

### 6.12 2026-11-24 图片类的审核退回：ubits 379266、tey 623、麒麟 479362

- **ubits 379266**（《伪装者》1080p HDTV，源 hdsky）：站内信写“种子审核拒绝，原因：海报与截图均无法访问（图床域名已失效）”。详情页简介是 9 张 `http://i13.tietuku.com/*.jpg`（1 海报 + 8 截图），域名 DNS 无记录、http/https 都连不上，站点说得对。
- **tey 623**（同一条内容发到太乙）：简介同样是从源站带来的那 9 张贴图库链接，只是当时页面还标着 `title="未审"`，没到拒的阶段。
- 为什么藏宝阁没事：11-23 那次只修了藏宝阁 43994 自己那一份简介，也只给 cangbao/panda/sbpt/luckpt/ptlgs 开了死图门禁。这条内容 11-24 凌晨发到 ubits/tey 时，两站都没开门禁，于是带着死图照样提交。
- **麒麟 479362**（《铁拳男人》2005 BluRay）：横幅写“审核不通过，原因：缺少海报或截图（图片少于 2 张）”。简介其实有 7 张图，但 1 张豆瓣海报 `img9.doubanio.com` 裸请求回 418/HTML（防盗链），6 张 `img.pterclub.com` 本机 TLS 连不上——站审时等于 0 张可达图。
- 修法三站一样（新脚本都进了 `scripts/`）：
  1. `py -3 scripts/fetch-descr.py <站点> <id>`：读编辑页 takeedit 表单，把简介字段原样落盘（默认只读，不提交）；
  2. 备图：海报按原简介里的豆瓣图 id 带 `Referer: https://movie.douban.com/` 取；截图用 `py -3 scripts/seed-images.py --content <内容键> --video <本地视频>` 从本机正片按时间比例抓（本机没有 ffmpeg 时脚本会去 MoviePilot 路径找）。《伪装者》41 集取 EP01/EP06/EP11 各 3 帧，《铁拳男人》单文件取 6 帧；
  3. `py -3 scripts/imgbox-upload.py --json <图...>`：拿 `images4.imgbox.com` 直链；
  4. `py -3 scripts/swap-descr-images.py --descr-file 原简介 --urls 直链,直链,... --out 新简介`：按出现顺序 1:1 只换链接、不动排版；BBCode 与 HTML 都支持，张数对不上直接报错不写文件；
  5. `py -3 scripts/edit-seed.py --site <站点> --id <id> --descr-file 新简介` 先演练，再加 `--apply` 提交；
  6. 复查详情页：`imgbox.com` 出现次数等于张数（tey 那条每张在简介里出现两次，所以是 18）、旧图站点计数 0；覆盖记录用 `py -3 scripts/set-cell-note.py <内容键> <站点> "说明"` 写成“已换图、等复审、别重发”，状态留 `review_rejected`，交给 `bsend followup` 盯。
- 结果：ubits 379266 详情页 tietuku 0 处 / imgbox 9 处；tey 623 tietuku 0 / imgbox 18；麒麟 479362 豆瓣与 pterclub 都 0 / imgbox 14（7 张 ×2）。三站页面还挂着“不通过 / 未审”，站点没有用户侧“重新提交审核”的入口，等管理复审；程序不重发、不私信管理。
- 门禁现状（2026-11-24 晚起）：剪死图是全站无条件硬规则；要图的站有硬性最低图数（ubits/tey/麒麟/藏宝阁 3 张，熊猫 4 张 + 段落顺序，ptlgs 4 张 = 1 海报 + 3 截图），由 `gates::image_floor` 硬编码，不再逐站配 `pruneDeadImages` / `requireImages`。以后再发同类内容，死图会在上传前被剪掉或拦下，不会等到站审才发现。

### 6.13 2026-11-25 调试期回归：标题、图床与简介字段

- **青蛙标题规则（站内信实测）**：《伪装者》208575 被拒，原因是完结剧集标题不用写 `Complete`，勾“完结”标签即可；参数顺序必须是“分辨率 → 片源类型 → 视频编码 → 音频编码”。代码已加 `reorder_release_title`：完结内容去 `Complete`、技术词按固定顺序排，站点改写标题时也按这套顺序生成。2026-11-25 回访 208575 页面已标“待审”，208692 仍“未审”；不私信管理。
- **Qilin 的简介必须是 BBCode**：479362 原来简介是 HTML `<img>`，站审按 BBCode 图片计数，显示“图片少于 2 张”。`scripts/fix-bbcode-descr.py` 把整段 HTML 转成 `[quote]`/`[img]`，提交后详情页 `[img]` 正常渲染；审核结果还要等站点重跑。
- **CSpt 不收 MediaInfo 和图片外 URL 标签**：260049/260050 上传后都被拒，原因“简介中包含 Mediainfo”。站点的 `technical_info` 字段单独存 MediaInfo，此外简介里还带 `[url]`。处理方式：清空 `technical_info`、把 `[url=…]` 包皮去掉只留文字，再用 `edit-seed.py --clear-technical-info` 提交；等待重审。以后向 CSpt 发种前先按这条规则预处理源简介，不要等拒绝后再补。
- **UBITS 图床黑名单**：知否 3 张 imgbox 图被 `takeupload.php` 直接拒绝，错误框写“检测到种子简介中包含黑名单图床，请将图片重新上传至其他图床（例如 pixhost.to）”。这不是网络故障，也不是重复；程序要能在上传前识别站点图床黑名单，或改用 pixhost 直链。测试 pixhost API `POST https://api.pixhost.to/images`（multipart `img`）可用，返回 `show_url`，原图直链在 `img*.pixhost.to/images/...`。
- **NovaHD 的 4K 60fps 选档**：`standard_sel[4]` 同时有 `2160p/4K` 和 `2160p/4K 60Fps`，旧逻辑按 `2160p` 匹配出两个候选，判为歧义留空，发布检查报 `standard_sel 未选`。新增 `pick_resolution_option`：标题带 `60fps` 就选带 60Fps 的档，没写帧率就选普通 2160p 档；已用人民的名义实发验证。
- **标题重排的边界**：`Coraline 2009 1080p USA Blu-ray AVC DTS-HD MA 5.1-PIXELPOLICE` 旧算法把 `USA`、`MA`、`5.1` 全塞到技术词后面，页面变成 `DTS-HD USA MA 5.1`。已把地区（USA/UK/JPN…）排在片源后、视频前，把 `MA`/`5.1`/`2Audio` 归到音频细节，压制组仍在最后；hdbao 54657、ptlgs 115313 已手工修正页面标题。
- **200 未识别不能当失败**：city13 知否上传返回 HTTP 200 未识别，站内搜索 `Ming Lan` 命中自己刚发的 88256，随后 `bsend pull` 成功挂回。遇到这类回包，先站内搜索或看账号上传列表，确认没发出去才允许重试。
- **审核回访里仍显示旧结论**：qilin、shuioudao、luckpt 的部分页面在改简介后仍显示旧“审核不通过”；qingwa 208575 改标题后已转“待审”，208692 仍“未审”。编辑页字段已经更新；`bsend followup` 继续按页面标记跟踪，不把旧文案当新拒绝，也不重发。等站点自动复审或管理处理。

### 6.14 2026-11-25 源站无图补图与 audiences 的 Kimi 判重

- **源站 0 图不要当“tey 图床坏了”**：tey 623《伪装者》9 张图全部 HTTP 200；卡住的是《波洛》源种 `carpt 532`，简介 428 字节、0 个 `[img]`，编辑页没有 `takeedit` 表单（无编辑权限）。tey 每次从源种重建简介都只有 0 图，低于 3 张门禁。
- **补图脚本**：`py -3 scripts/seed-images.py --content <内容键> --video <本地视频>`。脚本用本机 ffmpeg 抽帧（可多条视频各取一帧，或单条按比例取多帧），也可用 `--douban <条目id>` 自动取豆瓣海报，默认上传 pixhost（UBITS 接受；tey 也可用），写入 `screenshot-overrides.json`；`prepare_released` 会把图追加到简介末尾。波洛这次用 S01E01 / S07E01 / S10E01 各取一帧，3 张图都返回 200；tey 预检从“简介图片 0 张”变成“3 张 ≥ 3”。下一道门是 `medium_sel`：tey 的选项只有 WEB-DL / HDTV / DVDR / CD / Track，没有 Bluray，这条 Blu-ray 合集不能硬填 HDTV，所以仍未上传；不是图床问题。
- **audiences 判重**：代码 `kimi.rs` 固定走 Kimi WebBridge，先在用户已登录的真实浏览器打开 `torrents.php`，等 Cloudflare Turnstile token，再同源 `fetch` 搜索 URL；不再走无头 HTTP。2026-11-25 实测卡点不是“没点勾选框”：Kimi 能选中已打开的 audiences 标签页，但 Edge 窗口最小化时浏览器把页面判为 `document.visibilityState=hidden`，Turnstile 连 iframe 都不生成，`cf-turnstile-response` 一直为空；显式 `turnstile.render` 返回 widget id 也没有 token。程序现在会单独报“标签页在后台、Turnstile 不会生成验证框”，提示把标签页切到前台后重试，不把验证页当“站内没有”。
- **术语**：`info hash` 是整个 `info` 字典的 SHA1，tracker/announce 不在 `info` 里；`pieces` 是每个分片的 SHA1 串；pieces 指纹（`pieces_hash`）是对 `pieces` 整串再算的 SHA1；分片长度是 piece length。三者不要混叫，详见 `ARCHITECTURE.md` 7.1。

### 6.15 2026-11-25 sbpt 归属与 ptlgs 独立图字段

- **sbpt 七条不是“别人发的可辅种”**：21764、21767、21768、21780、21782、21783、21784 的详情页作者全部是本站账号 `Schalkiiii`。`existed=1` 只能说明站点按同 `info hash` 判重，不能说明谁先发。21764 早上先被旧 `same_hash_skip` 误记，后来按 qB progress/tracker 复核为做种；其余按 `existed` 路径下载目标种、校验 pieces 后挂回。详情链接见 `SITES.md` 2.5.1。
- **ptlgs 的 `cover` / `screenshots` 是独立必填字段**：上传页有 `cover`、`screenshots`，页面写“至少需要 3 张原始截图”；实际详情页 `115117`、`115313` 的 `ktorrentcover` 是 `<img src=''/>`，简介 0 张图，说明旧代码没填独立字段。Rust 已补 `posterField` / `screenshotField`：从最终简介按顺序抽图，第一张填海报，其余一行一个填截图；字段不存在就不硬塞，简介没图时清空。ptlgs 总图数按 4 张进 `gates::image_floor`。2026-11-25 真发《苦月亮》`115350` 做字段复测：详情页 `ktorrentcover` 有海报，`ktorrentscreenshots` 有 3 张截图，qB 校验 100%；站点仍标“未审”，等审核结果。
- **为什么先剪死图再抽字段**：`apply_site_rules` 会先剪死图、转 BBCode，再重新抽一次独立字段；否则简介剪掉了死图，独立字段还留着死链，审核端仍会退回。

```powershell
# 1) 新站第一次接入：只读表单检查
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" formcheck --sites <site>

# 2) 审阅一条，不发布
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" reseed --draft <N>

# 3) 单站试运行
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" autopost --dry-run --sites <site> --limit 1
```

需要看原始表单时，带 Cookie 打开上传页，打印所有 `..._sel` 下拉的 value 和显示文字。不要把 Cookie 值写进文档、日志或截图。

### 6.16 为什么早期发布没有立刻做种（2026-11-25 复核）

- **现场证据**：2026-11-22 熊猫 `10265` 的 `reseed-log.jsonl` 一行是 `result=posted`、`cell_status=same_hash_skip`、`seeded=false`。上传确实成功，但 qB 里同 `info hash` 的种子不是只挂熊猫 tracker；qB 不允许同 hash 存两条，程序按策略跳过。旧版把任何非 `Reseeded` 的结果都包成 `posted`，所以日志和界面看起来“发布完成”，实际没做种。
- **原因一：早期上传没有目标站专属 `info.source`。** 2026-11-23 08:38 之前，`torrent_rewrite.rs` 还没接入上传链路；目标种 hash 往往等于 qB 里源种 hash，qB 只能保留一条，挂回必然走同 hash 跳过。现在 `run_reseed` 上传前会写 `info.source`、断言新旧 hash 不同，并检查新 hash 不在 qB。
- **原因二：站点刚建种时 `download.php` 先回 `torrent not registered`。** 旧版只下载一次，失败就落 `posted_unresolved`，不会自己再试；2026-11-24 的 `951c476` 改成固定 4 轮、每轮 15 秒。
- **原因三：站点改写 hash/标题时旧版直接拒绝。** 2026-11-25 的 `727001c` 把挂回校验改成只核对 pieces 指纹和文件清单（忽略首段根目录名）；hash、标题不同都放行，白名单只用于日志标注。
- **现在的边界**：新上传走“写目标站 `info.source` → 上传 → 立刻下载站点种 → 校验 pieces/文件清单 → 挂 qB”这条硬链路；同 hash 跳过仍按项目规则不补挂、不加第二条，但已修掉“跳过却报 posted”的残留（2026-11-25）。历史同 hash 条目仍需人工决定删种或重发，程序不自动改原种。

### 6.17 tjupt 上传超时：小种能发，274GB 合集后端超时（2026-11-25）

- **25GB 单条成功**：源 hdfans 24558《我们的父辈》25.31GB 真发 tjupt，`130695` 字节种子上传后回包带详情链接；站点把 hash 改成 `2e8e1a98…`、标题也改写，但 pieces 和文件清单与构造一致，程序按新规则挂回 qB，校验 100%。详情页 `https://www.tjupt.org/details.php?id=652794`，页面还标“待审核”，覆盖记录 `pending_review`，日志 note 写明“种子已挂回 qB 做种”。
- **274GB 合集失败在站点侧**：源 carpt 532《大侦探波洛全季》274.06GB，种子 371628 字节、17542 个分片。上传 POST 120 秒时 `operation timed out`；放宽到 300 秒后回 HTTP 200、正文 0 字节，正好 5 分钟，站上搜索仍只有旧的 `416154`、`236519`，没有新条目。同一请求换成 400KB 假文件只回必填项错误（2.3 秒），说明网络和 multipart 没问题，是 tjupt 后端处理这个大种子到超时/崩溃后返回空正文。
- **代码对应**：`assist.reseedRules.<site>.uploadTimeoutSec` 可逐站放宽上传超时（缺省 120 秒）；网络层超时/断连现在会同一字节重投一次，第二次仍不通才报失败；HTTP 200 空正文单独报“站点后端没有返回内容，通常是处理超时或崩溃；本次未确认发布”，不再混成“未识别的返回”。
- **日志口径**：审核待定/审核不通过但 note 明写“已挂回 qB”的，`reseed-log.jsonl` 的 `seeded` 从这次修复后的新记录起记为 true；不再因为格子状态不是 `reseeded` 就写成没做种。`652794` 那行仍是修复前写的 `seeded=false`，属于历史记录，不回改。
- **待跟进**：274GB 合集重试仍会空回包，要发得先拆小或换内容。失败冷却默认关闭，自动发种下一轮仍会再试；不想再等 5 分钟，得把它从本轮候选里摘掉。25GB 的 `652794` 等站点审核，审完由 `bsend followup` 收尾。

## 7. 调试期回归的检查顺序

逐站再发时按这五步走，目的是不漏发，也不把同一条重复发出去：

1. **先看 qB 和账号上传列表**：qB 里有目标站 tracker 就当作已有；没有再看站内搜索或账号的已发布列表。回包超时、200 未识别、空白页都只算“结果未知”，不能直接重发。
2. **发前人工核对五件事**：标题技术参数顺序、简介里的图片和 MediaInfo、分类和下拉选项、站上有没有同名或同分片、站点最近站内信里有没有新规则。
3. **上传后立刻把站点种子下回来挂进 qB**：下载 `.torrent` 后核对分片和 `info.name`；站点刚建种常先回 `torrent not registered`，按程序内置的 4 轮、每轮 15 秒重试，仍拿不到才落待人工。
4. **审核状态按页面明写字样记**：站上已发布但页面还标“未审”记等站点审核；明写“审核不通过”记审核没过；页面说没有这条记站上已找不到。旧拒信不会因为改了简介/标题自动消失，`bsend followup` 继续按页面跟踪，不能把旧文案当新拒绝。
5. **改种子前先读站规和错误框原文**：标题顺序、完结标签、简介是否收 MediaInfo、图片图床黑名单都要按站点原话处理；没有用户明确允许，不给站点管理发站内信。

判断优先级始终是：**站上有没有 → 以 qB 和账号上传列表为准；一条种子只挂一个站；结果未知先核实再决定重发。**

## 8. 相关文档

- 站点总账和白名单：`SITES.md`
- 数据文件和 `formcheck.json` 字段：`OPERATIONS.md`
- 发种流程和单站点规则：`ARCHITECTURE.md`
- 还没做完的事：`ROADMAP.md`
