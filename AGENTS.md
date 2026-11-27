# AGENTS.md — BoxSend（本仓库）

## 这是什么
PT 站自动发种/跨站辅种工具：读 qB 里的内容组和 tracker，找出目标站缺口 → 审阅 + 发布前检查 → 上传 → 从目标站下载种子挂回 qB 做种。Rust workspace 在 `rust/`，桌面界面用 GPUI；另有 Swift 原版（`Sources/`）与 Windows 打包脚本（`windows/`）。

## 常用命令
```powershell
Set-Location rust
cargo fmt --all -- --check                           # 提交前必跑；CI 第一条就是它，格式不对直接红
cargo test --workspace                              # 全量门禁：当前 528 条；提交前必跑
cargo clippy --workspace --all-targets -- -D warnings  # 全量门禁：提交前必跑
cargo test -p bsend                                  # 单测 + e2e（快速回路）
cargo test -p bsend --lib reseed::tests::exist_marker # 跑单条用例
cargo build --release -p bsend-cli                   # 产物 rust/target/release/bsend.exe

# 试运行：拉上传页构造字段 + 跑发布前检查，不上传
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" --cookiecloud autopost --dry-run --sites sbpt --limit 5
# 真实发布（白名单 + 日限额 + 失败冷却 + 发布前检查）
.\target\release\bsend.exe --config "$env:LOCALAPPDATA\BoxSend\boxsend.json" --cookiecloud autopost --sites luckpt --limit 2
```

## 运行数据
配置与数据目录：`C:\Users\Schal\AppData\Local\BoxSend`（`boxsend.json` 是真实配置；`coverage.json` 是覆盖记录；`auto-audit.jsonl` 是发种审计；`reseed-log.jsonl` 是转种动作日志）。字段格式与手改边界见 `doc/OPERATIONS.md`，改 `coverage.json` 前必读。

## 技术栈
Rust 2021；crate：`bsend`（引擎）、`bsend-cli`、`bsend-gui`。错误用 `anyhow`，HTTP 用 `reqwest`（默认跟随重定向），种子文件格式（Bencode）自实现于 `bencode.rs`。

## 目录与约定
- `rust/crates/bsend/src/`：`reseed.rs`（审阅/上传/挂回主流程，约 3.6k 行；表单解析、预审、上传、发后写回、测试已拆到 `reseed/{form,audit,upload,post,tests,title}.rs`）、`autopost.rs`（自动发种编排）、`coverage.rs` / `report.rs`（覆盖记录与统计）、`matcher.rs`（判重）、`sites/nexusphp.rs`（站点适配）、`config.rs`、`gates.rs`；CLI 参数定义在 `bsend-cli/src/cli.rs`，命令处理在 `bsend-cli/src/commands.rs`。MSRV 1.88（`rust/Cargo.toml` 声明，CI 有 `MSRV 1.88` job）。
- 注释语言跟随所在文件既有习惯（现状以中文为主）；日志与文档用中文。

## 不能违反的规则
- **判断目标站有没有，以 qB 为准（2026-11-22 起）**：qB 里有目标站 tracker 就视为站上已有，没有就视为大概率没有；不再逐站搜索。只有真要发某条之前，才对目标站再确认一次。
- **同 `info hash` 不等于已经给目标站做种，也不等于没做种**：qB 不允许同一个 `info hash` 存两份，遇到同 hash 时不能只看日志里的 `same_hash_skip`；必须查 qB 这条种子的 tracker 和 progress。`progress=1` 只代表本地校验完成；还要看目标站 tracker 的 qB 状态：`status=2` 才是 tracker 已回包，`status=1` 是还没联系上，`status=4` 是 tracker 报错。只有“只挂目标站 tracker + progress=1 + tracker status=2”才能记“站点已看见做种”；tracker 没回包只能写“已挂回、待站点确认”（2026-11-27 city13 88464 在 17:20 站点页 0 做种者时是反例；17:50 站点页出现自己 1 个做种者、上传 70.08 MB，才按人工证据改成 `reseeded`，tracker 仍 status=4）。只挂别站 tracker、没有 tracker，或 progress<1 都不能记“已做种”，也不能给原种补挂，更不能 add 第二条。**发后挂回返回 `SameHashSkip` 时不是发布成功**：审计要落 `same_hash_skipped`、note 写“当前没做种，待人工处理”，不能包成 `posted`；2026-11-22 熊猫 10265 就是旧版把这条报成 posted 的现场。
- **判重口径统一叫 `info hash` 和 pieces 指纹**：`info hash` 是整个 `info` 字典的 SHA1；`pieces` 是每个分片 SHA1 串；pieces 指纹（代码 `pieces_hash`）是对 `pieces` 整串再算的 SHA1，不是单个分片，也不是 `info hash`。分片长度不同会让 pieces 指纹不同，即使文件内容一样。
- **audiences 的站内检索固定走 Kimi WebBridge**：用用户已登录的真实浏览器打开种子列表页、等 Cloudflare Turnstile token，再同源 `fetch` 搜索 URL；IMDb 区判重也走这个入口，不退回无头 HTTP，也不把“人机验证”页面当成“站内没有”。token 为空时明确报需要人工点一次验证；如果页面 `visibilityState=hidden`（窗口最小化或不在前台），Turnstile 不会渲染 iframe，要明确报“标签页在后台”并提示切到前台，不能硬等或盲发。详见 `doc/ARCHITECTURE.md` 5.5.2。
- **一个种子只挂一个站点**：禁止 `addTrackers`，禁止把多个站的 announce 合并。上传种子时写目标站专属 `info.source`，让新 info hash 和源种子不同；发布后从目标站下载 `.torrent`，作为独立种子挂回 qB。
- 发布不看画质：站上有同名其他版本照样发（`assist.allowOtherVersion`）；只有站点明确返回 `existed` 才不发。
- **HTTP 200 但回包认不出来时不能直接记失败**：`HTTP 200 未识别的返回` 先按发布名 + 本地总大小站内核对；找到就继续下载站点种子、核对 pieces 并挂回 qB，找不到落 `posted_unresolved` 待人工。403/404、空回包和明确的站点拒绝仍按失败处理；不要把“200 未识别”写成 `failed`。
- **上传页没有 file input / textarea 就不算上传表单**：`form_action` 找不到上传表单时必须返回空，不能退回整页取第一个 `<form>`；只有 hidden/number 的捐款、搜索、登录表单归 `NoForm`。longpt 实测只有 `bonusrain.php` 捐款表单，程序按 `no-form` 中止，不得把种子 POST 到捐款接口。
- 标题含 REMUX 时，`medium_sel` 先精确找 `UHD Remux`（4K）或 `BD Remux`/`Blu-ray Remux`/`Remux`（非 4K），站点没有再回退 `BluRay`/`Blu-ray`；不能让 `Blu-ray` 泛匹配把 `BD原盘`/`BD DIY`/`BD Remux` 算成歧义留空。
- 标题按站点规范重排：完结剧集去掉 `Complete`，技术参数按“分辨率 → 片源 → 视频编码 → 音频编码”的顺序；站点改写标题时同样按这套顺序生成，不再把 `USA`、`MA`、`5.1` 塞到音频后面。青蛙的 Remux 标题片源必须写 `BluRay`（不要 `Blu-ray` 的横杠），并勾「Remux」标签。
- 第一次向 qB 里还没有该站种子的站点发种时，抄不到该站 passkey，必须在 `sourceSites[].overrides.announceURL` 填该站完整 tracker 地址；没填会直接中止，不会伪造无 passkey 的种子。
- **判重以 info hash 和分片（pieces）为准；只有名称相同且总大小相等时才进人工核对，总大小不同就是不同内容。**
- **站上 hash 与上传构造不同也能挂回，但只认 pieces 和文件清单**：站点常改写 `info.source` 或标题；挂回前必须核对 pieces 指纹一致、文件清单一致（比较时忽略首段根目录名），不看 hash、也不看标题是否一字不差。`allows_target_hash_rewrite` 白名单只用于日志标注“这个站已实测改写过”，不是放行条件；pieces 或文件清单对不上就拒绝挂回，宁可落人工。
- **重复判定**：只认最终跳转的 `existed=1`、错误框里的明确重复措辞，或重填页/超过 8KB 的长页面里的“该种子已存在！”/“该种子已经存在！”/“已存在！”精确提示；上传表单原样返回且没有精确提示不算重复，帮助文案里的泛化“提示种子已存在”也不能当重复。
- **页面抓取重试边界**：检索、详情页、上传表单页读取只在明确的超时/连接重置/连接关闭/管道断开时最多再试 2 次；403、登录墙、站点限流不重试，不能把 cookie 失效当成网络抖动。
- **`existed=1` 只证明站点有同 `info hash` 的条目，不能证明是谁先发的**：判断归属要看详情页“由 … 发布”和发布时间；不要因为 `existed` 就把自己的条目说成“别人发的可辅种”。sbpt 2026-11-25 核实的七条都是本站账号 `Schalkiiii`，详见 `doc/SITES.md` 2.5.1。
- **上传页的独立海报/截图字段不能省**：站上把海报、截图放在 `posterField` / `screenshotField` 时，程序要从最终简介按顺序抽图填进去；表单没有这个字段就不硬塞，简介没图时清空，不能只把图塞在简介里就算完成。ptlgs 是 `cover` + `screenshots`，站规要求海报加至少 3 张截图。
- **面向人的文字一律说白话**：不用自造缩写或代号当术语。比如“把站点上的种子下回来挂进 qB”就写这句话，不要只丢一个命令名；命令名只出现在命令示例或“命令是 `bsend xxx`”这种明确指代里，正文先讲做什么，再给命令。日志、`status` / `audit` 输出、文档同样遵守。
- **等站点侧动作要留成待跟进**：站点要求改走候选入口（如 `torrent_too_large` → `offers.php`）记 `pending_offers`（待走候选入口）；站上已发布但站点还没审完记 `pending_review`（等站点审核）；站点明写审核不通过记 `review_rejected`；回访时详情页说没有这条记 `gone_from_site`。都不算失败、不自动重发、不进失败冷却，但必须能在 `bsend status` / `bsend audit` 里看到；不要写成 `skipped` 一了百了。审核这三类由回访收尾（`bsend followup`；自动发种每轮开头也会跑一次）：页面不再有审核标记就把站点种子下回来挂进 qB。页面标记只认明写的字样，判定规则在 `sites/nexusphp.rs` 的 `review_mark_in`。
- 失败冷却默认关闭：`assist.backoffHours` 不填或填 0 时，被拦或失败的条目下一轮直接重试；只有显式配小时数才在窗口内跳过（拦截与失败共用同一个窗口，`--dry-run` 同样受限），依据写在 `auto-audit.jsonl`。
- 待发条数从 qB 的实时缺口生成：`autopost` 读下载器种子列表，`--dry-run` 读索引里最后一次扫描结果。旧 `reseed_draft` 记录只作过渡期兼容。加白名单不会凭空产生待发条目。
- 内容范围由 `assist.skipKeywords` 拦教育/成人，命中不占上传额度。
- **未经用户明确允许，不得给站点管理发站内信/PM**（包括“问审核原因”“申请复审”）。审核被拒时只改种子、改代码、补证据；需要沟通先问用户。
- **修复必须脚本化，人工只做只读检查和已发种子紧急纠错**：发现字段、图片、标题或挂回问题时，先改代码或仓库脚本；只有已经发到站上的种子才允许人工紧急纠错。任何修复都要先用 `scripts/replay-reseed.py`（内部跑 `bsend reseed --draft <N>`，不提交）或 `edit-seed.py` 的默认演练复现，核对提交字段、图片数和标题，再执行 `--apply` / `--yes`；不能用手点站点表单、手改 `screenshot-overrides.json` 当常规修复路径。修复后补单测或回放脚本，证明同一条问题不会再犯。
- **上传成功就立刻把站点种子挂回 qB 做种**（2026-11-24 起是硬步骤，没有开关）：上传后立即从站点下载 `.torrent`、按分片核对、作为独立种子加入 qB；站点刚建种时常先回 `torrent not registered`，程序固定重试 4 轮（每轮 15 秒），仍拿不到才落“待人工确认”，回访用 `bsend pull` 补。站点要先审才给下载的，落 `pending_review`，审完由 `bsend followup` 自动挂回。
- 发种前门禁按站点配置执行：`titleNoCjk` 拦主标题汉字，`descrOrder` 校验简介标题顺序，`requireLinks` / `requireMediainfo` 校验必填。**剪死图是所有站点无条件执行的硬规则**（不再读 `assist.pruneDeadImages`）：提交前逐图探测，打不开的图直接剪掉；`hdsky.me` / `img.hdsky.me` / `imgur.com` 热链即使本机 200 也按死图处理，UBits 另把 `imgbox.com` 按死图处理。**要图的站有硬性最低图数**也是硬规则（藏宝阁/优堡/太乙/麒麟 3 张，熊猫 4 张 + 段序，ptlgs 4 张 = 1 张海报 + 3 张截图；写在 `gates::image_floor`，配置里写多少都不算）。图床统一用 pixhost：先跑 `scripts/rehost-descr-images.py` 把简介里所有非 pixhost 图片转存，再用 `scripts/swap-descr-images.py` 按顺序换链接，最后改简介；不能靠关掉门禁绕过。源站简介本来就没有图时，用 `scripts/seed-images.py` 从本地视频抽帧，或按豆瓣条目自动取海报，上传 pixhost 后写入 `screenshot-overrides.json`。同一部剧有 HDTV/WEB-DL 等多个内容键时，用 `seed-images.py --for-name` 或 `--also-content` 一次写齐，不能只补一个键；`screenshot-overrides.json` 不手改。发布时程序会自动追加，不许手工在简介里编图。
- **每轮批量发种前先扫各站收件箱**：审核拒绝、删种、管理员留言都要在 `messages.php` 里看一遍，不能只看 `followup` 已经记录的种子状态；漏看的拒绝消息按站点逐条落 `review_rejected` 并修种子。
- **熊猫发种暂停已撤销（2026-11-27）**：`panda` 已从 `assist.reseedPausedSites` 移出并留在 `autoReseedSites`；141840 已真发并挂回 qB 校验 100%、等审核。历史 141276 是候选种子 24 小时没做种被系统删的，旧账残留 `3f8ed774…` 挂失效 tracker，待人工清理；重发仍必须先“上传→立刻挂回 qB 做种”。
- 详细规则见 `doc/ARCHITECTURE.md`；运行数据与巡检见 `doc/OPERATIONS.md`；站点清单见 `doc/SITES.md`；未完成项见 `doc/ROADMAP.md`。历史审计与批次报告已并入这些文档，不再单独保留。

