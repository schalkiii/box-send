# 站点适配文档、站内信与规则收敛复核（2026-11-28）

## 1. 适配文档还在维护

用户记忆里的“站点规则落盘文档”就是 `doc/SITES.md`。它现在 13 万字节级别，2026-11-28 下午刚更新，不是废弃临时表：

- 2.5：逐站门禁与适配经验总表；
- 2.6：2026-11-26 站内信与回访结论；
- 2.8—2.11：2026-11-27 的实发、挂回、站点是否真看见；
- 2.12：本轮 2026-11-28 站内信复核、遗留处理和规则收敛；
- 第 3 节：147 站大表，由 `scripts/site-matrix.ps1` 从配置和 `formcheck.json` 生成，手改只改“备注”列。

`doc/SITE-DEBUG.md`（63,239 字节）继续记新站接入和上传失败排查。两份都在用，但它们是“人看的说明书”，不是发布前能直接执行的规则集合；真正会拦发布的是 `crates/bsend/src/gates.rs`、`reseed/` 和站点配置。

## 2. 站内信检查范围

从 `auto-audit.jsonl` 的站点字段，加上手动发过种的 xingtan、haoxue，去重得到 48 个曾发种站点 id。本轮实际读到 **47 个**收件箱页面；只有 `hdbao` 仍打不开。此前被 HTTP 500、TLS 超时、连接重置或 WAF 挡住的 7 个站已全部补读：

| 站点 | 补读方式 | 结果 |
|---|---|---|
| kufei | 直连重试 6 次后 200 | 9 条：欢迎、2026-10/09/08/07/06 认领结算、七夕、魔力补偿、端午、上传无数据补偿；无审核类 |
| ptba | 直连 200 | 8 条：3 张补签卡、茉莉 1w、故障补偿、4 条“[自动化发送]站点存在安全漏洞”、2026-09 认领结算；无审核类 |
| xdy | 直连 200 | 6 条：国庆魔力 10010、自定义头衔、做种积分修复、种子数奖励、欢迎；无审核类 |
| dubhe | 真实浏览器 | 5 条：临邀名额、数据库迁移补偿、邮箱更换、用户破千、象岛数据承接临时邀请；无审核类 |
| gtk | 代理 + 解码后的 cookie | 第一页 10 条、第二页 1 条：安全漏洞提醒、连接问题、补签卡、魔力补偿、宕机补偿；无审核类 |
| fenghuang | 直连重试 4 次后 200 | 4 条：**“种子被编辑”**、无主题、开放注册、欢迎；编辑消息指向 39168 |
| ptzone | 真实浏览器打开收件箱 | 5 条全是系统消息：2026-10 认领结算、维护补偿、站点维护补偿 1/2、2026-09 认领结算；0 未读、无审核类 |

`hdbao` 仍不可读：浏览器 `ERR_CONNECTION_TIMED_OUT`、直连 `messages.php` HTTP 500、代理连接重置。保留错误证据，不能写成“没有消息”。原始结果在：

- `artifacts/site-message-scan-20261128.json`
- `artifacts/site-message-scan2-20261128.json`
- `artifacts/site-message-scan3-20261128.json`
- `artifacts/browser-inbox-20261128.json`
- `artifacts/inbox-six-retry-20261128.json`
- `artifacts/inbox-remaining-20261128.json`
- `artifacts/inbox-kufei-20261128.html` / `inbox-kufei-parsed-20261128.json`
- `artifacts/inbox-ptba-20261128.html` / `inbox-three-parsed-20261128.json`
- `artifacts/inbox-xdy-20261128.html`
- `artifacts/inbox-fenghuang-20261128.html` / `inbox-fenghuang-parsed-20261128.json`
- `artifacts/gtk-messages-20261128.html`
- `artifacts/ptzone-inbox-20261128.json`
- `artifacts/inbox-audit-extra-20261128.json`

## 3. 有问题的发种和当前处理

| 站点 | 证据 | 处理 | 当前状态 |
|---|---|---|---|
| LongPT | 51248 自动审核说未勾「高帧」；51065 断种提醒 | 51248 补「高帧」；51065 补「HDR」「英字」「高分」，并把 qB 里 33 位错误 passkey 改回 32 位 | [51248](https://longpt.org/details.php?id=51248) 标签“高帧 完结”；[51065](https://longpt.org/details.php?id=51065) 站点同伴表出现 `Schalkiii`、断种警告消失。上传入口仍未定位，程序继续按 `no-form` 不硬发 |
| AGSVPT | 647086（14 时前）说 546963 简介顺序不对，海报必须在文字之前 | 海报移到 `kdescr` 最前，顺序改为“海报→文字→截图”，详情页 `edited=1` | [546963](https://www.agsvpt.com/details.php?id=546963) 仍 `( 禁止 )`，旧拒绝横幅还在；站点没有用户侧“重新审核”按钮，只能等管理重审。不重复提交，不发站内信 |
| UBits | 1604294（3 天 1 时前）“被站点管理员审核通过”；1604226 是旧拒绝 | 不需再改种子，核对详情页 | [379266](https://ubits.club/details.php?id=379266) 页面已显示 `(通过)`，同伴表 `d492602647` 上传 98.80 GB；旧“页面仍标不通过”结论作废 |
| 熊猫 | 492589 审核通过、492588 编辑都是 1 时 3 分前；492590 拒绝 1 时前但原因为空；492591 又标未审核 | 141840 已按“转载出处→制作说明→海报→简介→info→截图”重排，完整 MediaInfo 放独立框 | [141840](https://pandapt.net/details.php?id=141840) 标题旁绿色 `title="通过"`、1 做种者、18 下载者；页面证据强于空原因消息，按页面通过记录，等下一轮消息再确认 |
| 青蛙 | 643787 拒绝：`A Touch of Zen` 的 `Blu-ray` 应写 `BluRay`、Remux 要勾标签；643723 是 `Bitter Moon` 的 MediaInfo/BDInfo 原因；643710/643639 是《伪装者》的 Complete/顺序/缺封面 | 208692 换 676 字节 BDInfo、音频 Other、补原生原盘标签；208575 去 `Complete`、补完结标签和海报；208804 已通过 | [208692](https://qingwapt.com/details.php?id=208692)、[208575](https://qingwapt.com/details.php?id=208575) 编辑后约 30 小时仍 `( 禁止 )`，按站点“修改后自动进复审”继续等，不重复提交；[208804](https://qingwapt.com/details.php?id=208804) 已通过并做种 |
| qilin | 354775 管理员 `白小纯` 审核通过；354776 同时间编辑；354778 同时间“因断种过久，被系统自动删除” | 479362 确认被删；qB 同内容 `bd348dfc…` 的 tracker 现在回 `Torrent not exists` | [479362](https://www.hdkyl.in/details.php?id=479362) 覆盖记录保持 `gone_from_site`；qB 里的死条目由外部清理工具处理，不手删、不再记“做种”。重发前必须重新构造该站专属种子并确保 tracker 回包 |
| 藏宝阁 | 58106（2 天 7 时前）《知否》拒绝“简介图片 1/3 张无法访问”；58117（1 天 12 时前）同标题通过。58042/58048（4 天 19/14 时前）《Quick Change》因缺媒介/分辨率/图片和图片全部失效拒绝；58072（3 天 11 时前）同标题通过。详情：[44189](https://cangbao.ge/details.php?id=44189) / [43994](https://cangbao.ge/details.php?id=43994) | 不重复提交，核对通过消息 | 两条都是“先拒后过”，当前已通过、无遗留。审核会在提交后再查一次图，剪死图不能只在本地做一次 |
| 财神 | 211299（1 天 8 时前）《The Disguiser 1080p HDTV》通过；211298（同时间）《知否》通过；211128/211037 因 MediaInfo 错误、简介含 MediaInfo、MediaInfo 栏为空拒绝；211038/211036 还因缺海报/截图、少于 3 张截图拒绝；211035 因简介含 MediaInfo 拒绝；210780（4 天 11 时前）《S07 Mad Men》因图片外 URL 标签和简介含 MediaInfo 拒绝，页面仍 `(禁止)` | 前三条已通过；259870 是旧重复草稿，正确合集 `Mad Men S01-S07` 已另发 | 211299/211298 通过；[259870](https://cspt.top/details.php?id=259870) 仍禁止，是旧重复草稿，不编辑成第二条重复，保留待人工处理 |
| 幸运 | 85608（10 天 1 时前）61102《The Disguiser Complete HDTV》被 LuckAudit 拒绝：写中字标签未选、简介缺 IMDb/豆瓣/TMDB 链接、MediaInfo 为空（总分 81，错误 2，警告 1） | 详情页已有 IMDb 链接和 MediaInfo；编辑页实际已勾「中字」（值 6，23 是原创），标签是“原创中字完结”；`edit-seed.py` 直连 30 秒超时，改用浏览器核对表单 | [61102](https://pt.luckpt.de/details.php?id=61102) 旧拒绝里的“中字标签未选”与实际不符，不再记为待修；页面无禁止/未审标记、1 个做种者，不重复发 |
| 杏坛 | 456304 缺出版信息、ISBN、封面，专业分类和标题也不规范 | 标题改“义务教育教科书英语八年级上册”，专业按同系列七年级教材改“高等教育”（`type=947`），补 2025 年 7 月版、ISBN `978-7-5720-3516-6`、157 页、22.00 元、简介、目录、文件清单；封面已上传官方图床并替换 `descr` 首图（上传 id 91772，[官方图](https://img.xingtan.one/i/2026/11/28/6b0a2f2e597b3.png)，1080×1503） | [456304](https://xingtan.one/details.php?id=456304) 编辑返回“编辑成功！”，旧 imgbox 封面 0 处、官方图 1 处，标签「确认 修正」；当前仍是 `[免费]` 剩余 1 天 15 时、标题旁 `title="未审"`，不是审核通过；ISBN 未在 PDF 内直接确认 |
| 凤凰 | 消息 1557“种子被编辑”（23 时 37 分前）指向 [39168](https://pt.521.best/details.php?id=39168)：简介开头有 1PTBA 免责声明和保种提示，全文 11 张图全失效（9 张 `t1.picb.cc`、1 张 `i.loli.net`、1 张 `pt.keepfrds.com` 表情图），是从 1PTBA 5565 整体复制过来的 | 用 `seed-images.py` 从本地海报和 S01E01 抽帧，上传 4 张 pixhost 图并写入 `screenshot-overrides.json`；生成新简介，删掉两段含 1PTBA 的 `[quote]`、删掉全部死图、把 4 张新图放最前，`edit-seed.py` 先演练再 `--apply` | 详情页复查 `1PTBA` 计数 0，新海报/截图各出现 2 次，标签仍“中字”，标题/大小未变；`takeedit` 返回 302 `edited=1`。管理员改了什么没有编辑历史，本轮只清跨站残留和死图，不回滚管理员改动 |

没有给任何站点管理发站内信或 PM。

其余已读站 `dragonhd`、`hdtime`、`hdvideo`、`lajidui`、`lemonhd`、`nanyang`、`novahd`、`ptcafe`、`ptlgs`、`pthome`、`ptsbao`、`railgun`、`shuioudao`、`tangpt`、`tjupt`、`city13`、`crabpt`、`cyanbug`、`muxuege`、`hdfans`、`haoxue`、`oshen`、`baozi`、`itzmx`、`xingyunge`、`tey`、`sbpt`、`iloli`、`PTFans`、`Audiences`、`kufei`、`ptba`、`xdy`、`dubhe`、`gtk`、`ptzone` 没有当前审核拒绝、删种或管理员警告。`nanyang` 只有旧邀请/招募，`tjupt` 只有评论通知，`tangpt` 只有红包领取，`iloli`/`PTFans`/`Audiences` 只有结算、补偿和旧删种通知；`kufei`/`ptba`/`xdy`/`dubhe`/`gtk`/`ptzone` 只有补偿、结算、福利和安全漏洞提醒。

## 4. 规则收敛判断

已发过很多种子，不等于站点规则已经收敛。此前工具的优化目标是“能发出去、能挂回”，不是“能过每个站点的自动审核”。每接一个新站就引入一套新规则；发种量本身不会自动把这些规则变成门禁。

| 收敛层 | 内容 | 现状 |
|---|---|---|
| 已收敛，代码会拦 | 全站单 tracker、上传后立即挂回、pieces/文件清单核对、剪死图、图片底线、标题技术参数顺序、完结去 `Complete`、青蛙 `Blu-ray`→`BluRay`、青蛙蓝光/原盘要 BDInfo、熊猫六段简介、hdvideo 地区/剧情必填 | 已在 `gates.rs`、`reseed/`、配置里执行 |
| 半收敛，本轮已补自动化 | 青蛙/AGSVPT 海报必须在简介文字之前；LongPT 60fps→「高帧」、HDR→「HDR」；跨站整体复制的简介要删掉源站免责声明/保种提示 | 已加 `posterFirst`、`strip_cross_site_boilerplate` 和 `wanted_tags_for_site`，补了三条单测；凤凰 39168 已用真实种子回放并修好 |
| 未收敛，仍需人工或补代码 | AGSVPT「方舟计划」入选条件、LongPT 真实上传入口和 User 等级发布时段、杏坛书籍专业分类/ISBN/封面必须转官方图床、haoxue 单页应用、audiences 的 Turnstile 判重、站点临时改规则和人工主观审核 | 杏坛、haoxue 已明确移出自动名单；主观条件不硬编码成“所有种子”；AGSVPT 546963 虽已有「⛵方舟」标签，仍因海报顺序被拒，说明“有标签≠会通过” |
| 外部依赖 | 图床/WAF/TLS/Cloudflare、详情页超时、收件箱 `messages.php` 连接重置 | 只能真实浏览器或重试兜底；`hdbao` 本轮仍 `ERR_CONNECTION_TIMED_OUT`，不能写成“没有消息” |

从这轮站内信和规则页还能看出这些确定性规则：

1. **LuckPT**：标题或简介有中文字幕时，必须勾「中字」（LuckPT 表单值 6；23 是「原创」，2 是「首发」，5 是「国语」，10 是「完结」）。61102 的旧拒绝写“中字未选”，但编辑页实际已勾中字=6，说明这条不是当前遗留；预审仍要保留这项断言。
2. **财神**：MediaInfo 必须放独立栏，简介里不能带 MediaInfo；简介里图片外不要包 `[url]`；海报 + 至少 3 张有效截图。259870 和 211128 都是这类结构问题。
3. **藏宝阁**：本地剪完死图不等于站点审核时还活着；提交后站点会再查一次，图床选择要更保守。
4. **凤凰/跨站复制**：从 1PTBA 这类源站整体复制简介时，会带上 `[quote] ... 本站(1PTBA) ... [/quote]` 的免责声明和保种提示，还会带源站已经失效的图。发布前要删掉“引用块里出现源站名”的整段，并把死图全部换掉；只删图不删免责声明，仍然算带广告。
5. **杏坛**：书籍封面必须在官方图床 `img.xingtan.one`，不是随便一个能打开的图床；官方登录页是 `https://img.xingtan.one/login`，注册页是 `https://img.xingtan.one/register`。2026-11-28 已把 456304 的封面转到官方图床并替换简介首图，详情页旧 `images4.imgbox.com` 0 处、官方图 1 处，编辑返回“编辑成功！”，标签「确认 修正」；当前仍是 `[免费]` 剩余 1 天 15 时、标题旁 `title="未审"`，不是审核通过。编辑后站点会问“是否需要添加确认标签?”，要选“是”，否则只留「修正」、没有「确认」。截图证据：`artifacts/xingtan-456304-cover-tags-20261128.png`、`artifacts/xingtan-456304-cover-display-20261128.png`。
6. **AGSVPT**：`方舟计划` 是 `torrents.php?tag_id=52` 的标签/专区，规则页和 FAQ 没有写入选条件；不能给所有种子自动勾 52。海报必须在简介文字之前这条已进 `posterFirst`，546963 仍等管理重审。
7. **LongPT**：`faq.php` 写明只有授权用户才能发布，`User` 等级只能在周六 12:00–周日 23:59 发布；`upload.php` 仍只有 `bonusrain.php` 捐款表单，真实上传入口没定位，程序继续按 `no-form` 中止。
8. **断种删除**：上传成功、审核通过、做种成功是三件事。熊猫 141276 是候选 24 小时没做种被系统删；麒麟 479362 是审核通过后被“断种过久，被系统自动删除”。以后发布状态分 `uploaded`、`pending_seed`、`seeded_confirmed`；前两段不能写“已做种”。被删后不盲目重发；qB 里的失效残留交给外部清理工具，不手删。

规则表下一版建议每个站固定这些列，能自动判定的进代码，不能的写“人工”：

- 上传入口、所需权限、可发布时间窗（LongPT 这类）；
- 标题/副标题语言、禁止词、外链要求；
- 海报是否必须第一位、最低图数、允许/禁止图床（杏坛官方 `img.xingtan.one`）；
- 简介分段顺序、MediaInfo 是否必须独立栏、是否允许正文带 MediaInfo；
- 标签推导条件（高帧/HDR/中字/原创/完结/方舟等），以及“有标签不等于必过”的审核备注；
- 书籍类必填：书名、作者/译者、出版社、出版年、ISBN/DOI、页数、定价、封面、目录、正文截图、分类；
- 审核标记文案、是否有用户侧复审入口、被拒后只能等还是能重提；
- 发后核验方式：站点同伴表、qB tracker 状态、progress、上传量；
- 已知不可达域名和备用入口，避免把网络错误写成种子错误。

## 5. 本轮代码和脚本变化

- `posterFirst`：`SiteRules` 新增字段；`CoverageSettings::rules_for` 对 `qingwa`、`agsvpt` 默认打开；`reseed/form.rs` 提交前把第一张 `[img]` 图提到最前，已合规的简介不动，后面截图顺序不变。
- `strip_cross_site_boilerplate`：`reseed/form.rs` 提交前删掉“引用块里有源站 bare host”的整段；只删源站引用，目标站自己的 `[quote]` 保留；已有单测覆盖“删源站引用、保留目标站引用”。
- LongPT 站点标签：`wanted_tags_for_site` 在通用标签后补「高帧」「HDR」，只对 `longpt` 生效。
- 收件箱扫描：新增 `scripts/scan-site-messages.py`，按配置和 `cookies.json` 并发读 `messages.php`，只读不点消息、不回信；另有 `scripts/browser-inbox-check.py` 用真实浏览器补读打不开的站。
- 图片脚本：`scripts/seed-images.py` 新增 `--poster-file`，`upload_pixhost` 改成 6 次重试（20 秒超时、间隔 4 秒），解决 `api.pixhost.to` 偶发读超时；`scripts/edit-seed.py` 的 cookie 值先 `unquote` 再设置，编辑页 GET 改成 6 次重试（每次 25 秒、间隔 4 秒）。
- 测试：本轮新增 1 条跨站免责声明单测；`cargo test --workspace` **539 条全绿**（核心 461、coverage_e2e 35、CLI 23、GUI 13、其余 e2e/单测），`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo build --release -p bsend-cli` 全部通过。证据：`artifacts/workspace-test-20261128-final.log`、`fmt-check-20261128-final.log`、`clippy-check-20261128-final.log`、`build-release-20261128-final.log`。

## 6. 还没做完的

1. `bsend` 还没有内置 `messages` 命令，收件箱巡检仍靠脚本/浏览器。
2. `bsend followup` 仍只读详情页审核标记，不读收件箱；审核拒绝、删种、管理员留言仍要单独扫 `messages.php`。
3. 发布后持续做种真值核验（站点同伴表 + qB tracker + progress）还没有做成每轮自动任务；qilin 479362 就是审核通过后被系统按断种删除，熊猫 141276 是候选 24 小时没做种被删。qB 里的失效残留交给外部清理工具，不手删。
4. `hdbao` 收件箱仍打不开，是唯一没读到消息的曾发种站；不能把超时当成“没有消息”。
5. qilin 479362 的 qB 死条目由外部清理工具处理，不手删；是否重发另定，重发前必须先保证“上传→立刻挂回 qB 做种→tracker 回包”。
6. 财神 259870 是旧重复草稿，仍 `(禁止)`，没有用户侧删除入口，保留待人工处理。
7. AGSVPT「方舟计划」入选条件、LongPT 真实上传入口和发布时段、haoxue 单页应用仍未收敛；这些不硬编码成通用门禁。
