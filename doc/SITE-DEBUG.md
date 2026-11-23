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

### 6.4 haitang 和 haoxue

- haitang：只有曲艺、有声书类目，和影视范围不匹配，已移出白名单，规则留档。
- haoxue：Next.js/Flux 单页应用，静态提交地址为空，旧检索路由 404；需要按前端接口重做，已移出白名单。

### 6.5 agsvpt、tey

- agsvpt：提交域和登录域不一致，返回“你没有该权限”并跳登录页，建议剔除。
- tey：发布检查曾报 `medium_sel` 没选，根因是该站 `medium_sel[4]` 只有 WEB-DL/HDTV/DVDR/CD/Track，蓝光/Remux 源没有对应项；HDTV/WEB-DL 源可正常选档。2026-11-23 已用 HDTV 源实发成功，见 6.7。

### 6.6 qilin 和 panda

站上其实已经发布（qilin 479362、panda 141276），但旧逻辑拿旧种或错误详情页当结果，记成了失败；新 hash 不在 qB。这是发后定位错误，不是上传失败。新版流程已经改成从回包正文/跳转里按 id 配对详情和下载链接，再下载目标种独立挂回；发后定位缺陷已修（见 `ROADMAP.md` 4.3）。2026-11-23 已补挂：qilin 479362（hash `bd348dfc…`，站点审核仍显示缺图，不影响做种）、panda 141276（hash `3f8ed774…`），两条 qB 校验 100%。

### 6.7 2026-11-23 新实发：ptba、cspt、dubhe、ptsbao、crabpt、hdbao、tey

七站都是第一次向目标站发种，上传本身成功；站上返回的 `.torrent` hash 与本地上传构造的 hash 不同。ptba 已确认是站点改写了 `info.source`；cspt、dubhe、ptsbao、crabpt、hdbao、tey 的 pieces 和 `info.name` 未变，但具体改写字段没有逐字节核对。程序默认拒绝 hash 不一致；七站经 pieces 和 `info.name` 核对后加入白名单，后续同站发种直接放行，不再落 `posted_unresolved`。

- **ptba**：源 `luckpt` 的 `金枝欲孽`（51.9 GB）→ `https://1ptba.com/details.php?id=106718`，站上 hash `abf41c82…`，构造 hash `4f8b63e2…`。站上 `info.source` 为 `[1ptba.com] 1PTBA.COM`，本地构造写的是 `ptba`。已用 `reattach` 补挂，qB 校验 100%。
- **cspt**：源 `ptba` 的 `Mad Men S01-S07`（95.1 GB）→ `https://cspt.top/details.php?id=259870`，站上 hash `cf568e1a…`，构造 hash `516522da…`。第一次补挂被 tracker 校验拦住：站上种子的 announce 在 `tracker.cspt.cc`，页面域是 `cspt.top`，`extra_announce_hosts` 缺别名。补 `cspt => ["tracker.cspt.cc"]` 后重新补挂成功；站上种子自带两条同站 announce，保留原样。
- **dubhe**：源 `soulvoice` 的 `知否知否应是绿肥红瘦`（119.6 GB）→ `https://dubhe.site/details.php?id=55492`，站上 hash `b5ebc539…`，构造 hash `0e53fd1b…`。第一次判重搜索 30 秒超时，重试才通过；表单里没有“完结”标签复选框，发布后需要人工补。已用 `reattach` 补挂，qB 校验 100%。
- **ptsbao**：源 `soulvoice` 的同一条《知否知否应是绿肥红瘦》→ `https://ptsbao.club/details.php?id=135787`，站上 hash `0b0087f7…`，构造 hash `bf6e43ae…`。`source_sel[1]` 是二级分类（`剧集-美剧` / `电影-WEB-DL`），旧逻辑会按“WEB-DL”错填成 `电影-WEB-DL`；已改成先识别二级分类、type 选完后再按父子关系填，实发时选到 `剧集-大陆(23)`。已用 `reattach` 补挂，qB 校验 100%；表单没有“完结”标签，发布后需要人工补。
- **crabpt**：源 `hdsky` 的 `The Disguiser`（88.9 GB）→ `https://crabpt.vip/details.php?id=240242`，站上 hash `732e2c99…`，构造 hash `fe9204e8…`。此前候选《人民的名义》卡在源站 `hdfans` 502（6 小时退避），这次改选 `hdsky` 源后一次通过；站上已有同片 2160p 其他版本 214580，按策略继续发布。已用 `reattach` 补挂，qB 校验 100%。
- **hdbao**：`autopost --sites hdbao --limit 1` 实际发了 2 条：源 `ptba` 的 `Mad Men S01-S07`（102.1 GB）→ `https://hdbao.cc/details.php?id=54402`，站上 hash `95eacf24…`，构造 hash `e88d740e…`；源 `hdfans` 的《人民的名义》（95.8 GB）→ `https://hdbao.cc/details.php?id=54403`，站上 hash `51ededb2…`，构造 hash `1f8a0104…`。两条都先落 `posted_unresolved`，`reattach` 后 qB 校验 100%。`--limit` 少算 `posted_unresolved` 的缺陷已修，后续同站不再连发。
- **tey**：源 `hdsky` 的 `The Disguiser`（HDTV 1080p，82.8 GB）→ `https://pt.tey.cc/details.php?id=623`，站上 hash `c9570904…`，构造 hash `465916fd…`。该站 `medium_sel[4]` 没有 Blu-ray，蓝光/Remux 候选会被选档门禁拦住；这次改用 HDTV 源才通过。qB 里 5 条老种全走 `tracker.tey.cc`，页面域是 `pt.tey.cc`，补 announce 别名后 qB 覆盖从 0 变 5，也能抄到 passkey。上传回包同样没给详情链接，落 `posted_unresolved`；`reattach` 后 qB 校验 100%。新种在站上暂时显示“未审”，等站点审核。

七站共同的结论：站点改写种子元信息是正常行为，不能把 hash 不一致一概当错种；但只对实测确认的站放行，且必须 pieces 和标题同时一致。新增站先按默认严格模式跑，出现 hash 不一致再核对，不要预先扩大白名单。

### 6.8 2026-11-23 未打通的站：oshen、itzmx、ptfans

- **oshen**：`formcheck` 通过（14 个字段、8 个下拉，选档正常）；实际 `reseed` 先遇到判重搜索 30 秒超时，重试后上传页返回“没有可解析的静态上传表单”。同一时刻再跑 `formcheck` 又正常，说明是站点侧限流或页面形态抖动，不是 cookie 失效。没有上传，未产生站点记录。
- **itzmx**：审阅和门禁都通过（10 个字段，`type=402` 剧集、`standard_sel=1` 1080p），两条候选都是大合集（294.3 GB / 197.2 GB）。两次 `reseed --yes` 都在 `takeupload.php` 被站点前置的 Kangle 拦下：`HTTP 403 access denied by input filter`，回包已落 `upload-debug/itzmx-1795423111-a0.html`。这不是 tracker/hash/权限问题，是站点侧输入过滤；未产生站点记录。先不重试，等换非合集候选或确认站点过滤规则。
- **ptfans**：审阅时目标站上传页 `https://ptfans.cc/upload.php` 超时；再试在 IMDb 判重搜索 `torrents.php?search=tt0094525…` 超时，流程按设计拒绝未确认重复的上传。没有上传，未产生站点记录；等站点连通性恢复再试。

### 6.9 cangbao（藏宝阁）

- 现象：`Quick Change 1990 BDRip X264 iNT-TLF`（43994）审核拒绝：未填写媒介类型、未选择分辨率、简介未包含图片。
- 媒介：标题只有 `BDRip`/`X264`，旧映射没把 BDRip 当光盘源，`medium_sel[4]` 留空。BDRip + x264 是重编码，站点 `Encode(3)` 才对。
- 分辨率：标题没有 `1080p`/`720p`，旧逻辑不从 MediaInfo 取值；实际 MediaInfo 是 1290x696，按长边归 `720p(3)`（裁黑边的 720p 编码）。
- 简介：站点简介框是 BBCode 编辑器，源站 hdfans 的简介是裸 HTML；原样提交后数据库里是 `&lt;fieldset&gt;…`，页面显示字面标签，审核端看不到 `<img>`。代码已改为默认把 HTML 简介转 BBCode（`descrFormat=html` 才保留 HTML），43994 已用 `[quote]`/`[img]` 重新提交，详情页正常渲染。
- 2026-11-23 抽查 29 条已发布种子，lajidui、ptcafe、sbpt、luckpt、ptlgs、tangpt、longpt、cyanbug、kufei、xingyunge、city13、shuioudao、xdy、baozi、novahd、ptzone 等站的历史简介也是字面 HTML；已用 `scripts/fix-bbcode-descr.py` 批量改成 BBCode，只改简介、不换种子文件，29 条复查全部为 BBCode。
- 修复后先只编辑 43994，不新发种；等审核通过、再考虑恢复藏宝阁自动发种。

## 7. 复现模板

```powershell
# 1) 新站第一次接入：只读表单检查
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" formcheck --sites <site>

# 2) 审阅一条，不发布
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" reseed --draft <N>

# 3) 单站试运行
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" autopost --dry-run --sites <site> --limit 1
```

需要看原始表单时，带 Cookie 打开上传页，打印所有 `..._sel` 下拉的 value 和显示文字。不要把 Cookie 值写进文档、日志或截图。

## 8. 相关文档

- 站点总账和白名单：`SITES.md`
- 数据文件和 `formcheck.json` 字段：`OPERATIONS.md`
- 发种流程和单站点规则：`ARCHITECTURE.md`
- 还没做完的事：`ROADMAP.md`
