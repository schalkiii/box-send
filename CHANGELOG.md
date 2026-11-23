# Changelog

版本以 git tag 为准（Rust workspace 当前 `0.1.0`）。只记录用户可见/行为变化；
逐条技术细节见 `doc/ARCHITECTURE.md`、`doc/OPERATIONS.md`、`doc/ROADMAP.md` 与 git 历史。

## [Unreleased]

### Added

- 发布前门禁新增 `titleNoCjk`、`requireImages`、`descrOrder`：主标题含汉字、最终简介图片不足、简介段落乱序都会在上传前拦下；门禁改为读 `apply_site_rules` 之后真正要提交的简介，不再看源页原文。藏宝阁配 2 张图 + 剪死图，熊猫高清配 4 张图和【转载出处-制作说明-海报-简介-info-截图】顺序。
- `scripts/imgbox-upload.py`：匿名上传 imgbox，返回原图直链；无需 API key，单图 10MB，实测带藏宝阁/熊猫 Referer 均 200。
- `scripts/edit-seed.py`：只改已发布种子的标题/简介，默认演练，`--apply` 才提交；其余表单原样回填。
- 2026-11-23 修复藏宝阁 43994：豆瓣海报 418/HTML、`imgs.loukky.com` TLS 失败，重传 imgbox 后详情页两张图正常；修复熊猫 141276：主标题去掉 `[50%] 剩余时间` 中文尾巴，简介重排为六段并补海报 + 3 张截图。
- 辅种：pieces-hash 批量通道 + 名字检索（严格 / 宽松 / 最大成员种名兜底）。
- 转种：预检 → 官方预审 → 上传 → 发后自检 → 推种校验全流程；`autopost`
  白名单 + 日限额（北京时间）+ 退避 + 审计（`auto-audit.jsonl`）。
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
- 新增 `bsend reattach --site <id> --url <详情页>`：补挂站上已发布、qB 未挂回的种子；
  `--key` 指定内容，`--file` 在站点拒绝下载时给本地构造的 `.torrent`。
- 2026-11-23 继续首次向 `railgun`、`lemonhd` 发种并挂回：两站都发布 `carpt` 的 Poirot 合集，
  站上详情分别为 `60632`、`48970`，站上 hash 与构造 hash 不同（railgun `72889498…` vs 构造 `6debbce8…`，
  lemonhd `3c00cbb2…` vs 构造 `d50dfa19…`）；两站上传回包都没给详情链接，先落 `posted_unresolved`，
  加入 hash 改写白名单后 `reattach`，qB 校验 100%，tracker 分别只有 `bilibili.download`、
  `tracker.lemonhd.net`。两站目标站都已有其他版本，按“同名其他版本照样发”策略继续发布。
  累计首次实发 12 站、13 颗目标种。`reattach` 挂回后读 tracker 列表增加 3 次短重试，
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

### Changed

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
- 同 hash 目标种一律不改动原种子（不补挂 tracker、不打标签），记「同 hash 跳过」；
  qB 不允许同 hash 两条，新加种子只适用于 hash 不同的目标种。

### Fixed

- 藏宝阁 43994 审核驳回修复：BDRip+x264 归 `Encode(3)`，标题无分辨率时按 MediaInfo 长边归档
  （1290x696 → `720p(3)`）；源站 HTML 简介提交前默认转 BBCode，避免站点把 `<fieldset>`/`<img>`
  转义成字面标签。已在线编辑 43994，未新发种。
- 历史已发布种子简介批量转 BBCode：29 条（sbpt/luckpt/lajidui/ptcafe/tangpt/xdy 等）逐个编辑复查，
  只改简介、不换种子文件；`scripts/fix-bbcode-descr.py` 保留为可重复执行的维护脚本。
- GUI 概览页 clippy：`sort_by` 改 `sort_by_key(Reverse(...))`，图例查找去掉多余借用。

- 发后挂回不再把“站上已发、qB 没挂上”记成发种失败：下载目标种失败、hash/pieces 校验
  不过或挂回 qB 失败时，格子落 `review_pending`，note 前缀 `posted_unresolved` /
  `existed_unresolved`，不进退避、不占发布额度。
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
