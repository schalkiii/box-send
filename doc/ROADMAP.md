# BoxSend 还要做什么

这份文档只记“现在做到哪、下一步做什么、有什么风险”。已完成的历史流水不再展开。判断规则见 `ARCHITECTURE.md`，运行排障见 `OPERATIONS.md`。

## 1. 当前主线

把“逐个站点搜索判断有没有”改成“以 qB 的 tracker 为准”，并保证一个种子只挂一个站点。

要达成四件事：

1. 判断阶段不再请求站点，只读 qB；
2. 待发清单从 qB 的实时缺口生成，不再依赖旧的“没有”记录；
3. 上传前从 qB 导出源种子，改写 tracker 和目标站专属来源标记，让新 hash 唯一；
4. 发布后从目标站下载种子，独立挂回 qB，不和源种子混在一起。

## 2. 已经做完的

### 2.1 判断与报表

- 覆盖报表新增“qB 里有没有”的统计：`qbCovered`、`qbMissing`、`qbRatio`。
- `bsend status` 的单站表改成“已有 / 缺口 / 覆盖率”三列，直接看 qB 里的实际情况。
- `bsend report` 的主表也加了这三列，旧的“已判记录 / 判定覆盖 / 全库覆盖”保留作历史对比。
- 有单元测试覆盖：qB 里有 tracker 就算已有，子域名也算；没有记录也能算出覆盖率；全库为空不会出现除零或 NaN。
- 2026-11-23 补齐 7 个 announce 别名：`baozi`、`novahd`、`xingyunge`、`ptlgs`、`pthome`、`agsvpt`、`cspt`，并把 `www.` 与裸域归一化（`tjupt`）。qB 覆盖率不再把已做种的内容算成缺口；例如 agsvpt 从 0 → 757、baozi 0 → 569、novahd 0 → 601、xingyunge 0 → 566、pthome 0 → 566、ptlgs 0 → 584，cspt、tjupt 也补回真实值。

### 2.2 待发清单

- `autopost` 不再要求覆盖记录里先有 `reseed_draft` 条目。
- 每一轮读 qB 的实时种子列表，找出“白名单站点没有 tracker、源种子已定位、内容还在 qB、没有禁止转载”的组合。
- 同名内容对同一个站只生成一条；按内容体积从大到小排，每站按当天额度截断。
- 内容类型黑名单、白名单、暂停站、失败冷却（默认关闭）、日限额仍然在发布前检查。
- 已经有单元测试覆盖：qB 已有目标站 tracker 时不生成；源站不转自己；下载器里已有的站会跳过。

### 2.3 文档

- 原来 17 份分散文档合并成 5 份：`ARCHITECTURE.md`、`OPERATIONS.md`、`SITES.md`、`SITE-DEBUG.md`、`ROADMAP.md`。
- 文档改成白话叙述，先说“为什么”，再说“怎么做”，最后说“出问题看哪里”。
- 删除 `work/expand/`、`work-site-json.txt` 和空目录 `work/`。

### 2.4 桌面界面

- 概览表改成六列：站点 / 已有 / 缺口 / 覆盖率 / 草稿 / 已跳过；表头和数据行共用同一组列宽，窄窗口不再错位。
- 默认按覆盖率降序；最小窗口 1060×720。
- 改前/改后截图在 `doc/gui-review/compare-*.png`，设计口径在 `doc/gui-review/DESIGN-SPEC.md`。

## 3. 正在做的

### 3.1 单种子单站点上传

代码已经接入了大部分：

- 从 qB 导出源种子；
- 保留文件清单、分片、名称和分片长度；
- 把 tracker 换成目标站地址；
- 第一次向某站发种时，可以在该站配置 `overrides.announceURL` 里填完整 tracker 地址，解决“qB 里还没有该站种子、抄不到 passkey”的问题；填了配置优先，没填才从 qB 现有 tracker 里找；
- 写入目标站专属的 `info.source`；
- 计算新 hash，确认不在 qB 现有集合里；
- 上传后优先从回包正文、最终跳转或 `submit_upload` 已解析的详情 id 取目标种；回包没有链接时，按发布名 + 本地总大小站内检索，并核对详情页大小；
- 从目标站下载种子，先核对 `info hash`（新发种默认必须等于上传构造的新 hash；ptba/cspt/dubhe 这类站会改写 `info.source`，只在 pieces 和 `info.name` 都一致时放行）和 pieces（必须与本地源种一致），再独立添加回 qB；不拿本地构造种顶替；
- 站点侧已发布、但下载目标种或挂回 qB 失败时，记 `ReviewPending`、note 前缀 `posted_unresolved` / `existed_unresolved`，不记失败、不进失败冷却、不占发布额度；
- 手工补挂：只拉详情页、下载站点生成的 `.torrent`，按 pieces 找本地内容后走同一条挂回逻辑（命令 `bsend reattach --site <id> --url <详情页>`）；已移除本地构造种的 `--file` 入口。

已通过：`coverage_e2e` 26 条全部通过；真实发种 luckpt、sbpt 各一条并挂回校验；hdvideo 78856、panda 141276、qilin 479362 三条历史种子已补挂，qB 校验 100%、tracker 正常。2026-11-23 又首次向 `ptba`、`cspt`、`dubhe`、`ptsbao`、`crabpt`、`hdbao`、`tey`、`oshen`、`ptfans`、`itzmx`、`railgun`、`lemonhd` 发种（hdbao 2 条，其余各 1 条），十二站都出现“站上 hash 与构造 hash 不同”，已按 pieces/标题校验补挂成功，并把十二站加入 hash 改写白名单；ptsbao 的 `source_sel` 二级分类也已按 type 父类 + 标题自动填对；hdbao 首次实发还暴露 `autopost --limit` 只数 Posted、不数 `posted_unresolved` 的问题，已修；itzmx 的 torrent 文件上限为 `266,240 bytes`，改用《Liar Game》小种后才打通。2026-11-23 发布检查新增 `titleNoCjk`、`requireImages`、`descrOrder`，并让门禁读最终提交的简介（剪死图/转 BBCode 之后），藏宝阁、熊猫高清已启用。

还没完成：

- 用更多真实站点跑完整链路，覆盖“站上已有 / existed / 下载被拒 / 校验失败”等分支；
- **三条“站上已发、qB 未挂回”已补齐**（2026-11-24，详见 `SITES.md` 2.3）：pthome `284701`、nanyang `226159`、ubits `379266` 三站加入 hash 改写白名单后，用 `bsend reattach` 把站点种子下回来挂进 qB，qB 校验 100%、只挂单站 tracker、状态 `seeding`。站上 hash 与构造 hash 的不同点（具体改了哪个 info 字段）仍未逐字节核对。
- **hdfans `287791` 已核实**（2026-11-24）：就是《伪装者》同一条，qB 里 `b1e600a6…` 校验 100%，覆盖记录 `reseeded`，没有重发。
- **tjupt**：announce 地址按站点下载的种子定案为 `tracker-public.tjupt.org`（https、带 passkey），已写进 `overrides.announceURL`；上传仍撞 `takeupload.php` 超时——30s 全局超时不够，上传单步已放宽到 120s，等下一次实跑验证。站上已有 `416154`（FLAC/DDP5.1）与 `236519`（同名 FRDS 合集，pieces 与本地不同），属实不同内容，不算重复。
- **audiences**：判重检索被人机验证拦住，程序按“未确认重复不上传”中止。浏览器里过了验证也救不了无头请求：cookie 里没有 `uid/pass` 类鉴权字段，直连会被跳 `login.php`。后续不改无头请求，改成走 Kimi WebBridge 复用用户浏览器的登录态做判重（可行性待验证，验证前这个站仍只人工发）。
- **qingwa 的 Poirot 合集**：不是“回包未识别”，回包是 `torrent_too_large` 表单（自动 submit 到 `offers.php?add_offer=1`），站点要求改走候选入口；程序记 `pending_offers`（待走候选入口），不算失败、不自动重试，`bsend status` 单列“待跟进”，等人去候选页提交。
- 藏宝阁 43994 已换 imgbox 图并重提，熊猫 141276 已改英文标题/重排简介，两条都还在等站点复审，审核状态未解除；这两条作为待跟进事项留在本文件，覆盖记录里没有格子，暂时只能人工盯。后续可考虑把“等站点审核”也落成可查询的待跟进状态（目前只有 `pending_offers` 是机器化的）；
- 死图重传目前是 `scripts/imgbox-upload.py` + `scripts/edit-seed.py` 手工/半自动，尚未接成发布前自动重传；门禁会先拦住，不会带死图上传。

### 3.2 文档里的站点数据

`SITES.md` 的 147 站表来自配置和上一次表单检查，不是实时状态。站点改版、关站、权限变化后要重新生成，不要拿旧表当现在能不能发的依据。

## 4. 还没做的

### 4.1 发布前再确认一次（代码已接，e2e 已覆盖，真实命中样本待跑）

真要发某一条之前，程序的 `reseed.rs` 第 2.5 步会先查一次目标站：

- 只对“这一轮真要发”的条目查；
- 每条最多查一次；
- 查到站上已有，不重复上传，改为下载目标种子挂回 qB；
- 搜索本身失败（网络、登录、限流）就中止这条，按失败冷却设置决定要不要下一轮再试，不盲发。

`e2e_duplicate_hit_pushes_target_torrent_to_downloader` 已经覆盖“命中后下载目标种、pieces 比对、挂回 qB”这条路径。真实站点这边还没碰到一次完全相同的命中样本：本轮 ptba/cspt/dubhe 都是“站上已有其他版本”或“没有同条目”，走的是继续上传分支。下一步找一个目标站已有同条目的种子，跑一次只读判重确认，不再重复上传。

### 4.2 上传额度按结果分开算（已完成 2026-11-23）

原问题：旧口径下“站上已有”“重复”“失败”也可能消耗当轮上传次数，热门站查 10 次全是重复，一条也发不出去。

现在 `autopost` 只在 `ReseedOutcome::Posted` 时 `posted_today += 1`；`blocked`、`duplicate`、`failed` 不计入 `posted`。`parse_audit` 也按 `posted` 天数统计，日限额只算真正上传成功。文档之前写成“还在修”，已按代码实际行为改正。

注意：这里的“额度”是**发布额度**，不是上传请求总预算；全局 `roundBudget/uploadBudget` 仍会拦截所有真正的上传动作，两者不冲突。

### 4.3 判重和发后定位

- 上传回包里的详情页 id 不可信，已经出过“站上已发、本地判失败”。
- 发后定位缺陷已修（2026-11-23）：`extract_posted_links` 不再取第一条链接，而是按 id 配对详情/下载；最终跳转是详情页时优先用它；多个链接对不上时两个都不用，转 `posted_unresolved`；`userdetails.php` 不再被当成种子详情。无链接时再按名称 + 总大小检索，避免同名不同版本抓错。
- 挂回前校验已补（2026-11-23）：新发种下载回来的 `.torrent` 默认必须与上传构造的新 hash 相同，pieces 必须与本地源种一致，否则拒绝挂回；下载/挂回失败不再记失败，落 `posted_unresolved`，判重路径落 `existed_unresolved`。
- 站点改写种子元信息的例外（2026-11-23）：ptba、cspt、dubhe、ptsbao、crabpt、hdbao、tey、oshen、ptfans、itzmx、railgun、lemonhd 实机确认站上 hash 与构造 hash 不同，pieces 和 `info.name` 一致；十二站加入白名单后自动放行，其他站仍严格拒绝。ptba 已确认被改写的是 `info.source`，其余站的具体改写字段未逐字节核对。2026-11-24 又加入 pthome、nanyang、ubits（同样是站上 hash ≠ 构造 hash、pieces 一致），现在共十五站；白名单只影响原始上传后的自动挂回，补挂命令不设期望 hash，不受它限制。
- announce 别名已补（2026-11-23）：baozi、novahd、xingyunge、ptlgs、pthome、agsvpt、cspt、tey 加了站点别名，`www.` 与裸域也归一化；cspt 的 `tracker.cspt.cc`、tey 的 `tracker.tey.cc` 都是“页面域和 tracker 域后缀不同”的例子，tey 不补别名会被误判成 qB 覆盖 0。2026-11-24 又补了 qingwa 的 `tracker.qingwapt.org`。
- 详情页大小解析纠偏（2026-11-24）：`4e3a8b1` 为了补 0.01 显示余量，把大小解析放宽成“任意位置的数字都试”，又留着缺单位按 B 算的旧逻辑，结果“上传于 2026.11.23”“做种 12”这类计数被当成字节数；标准 NexusPHP 的大小又和类型、媒介挤在同一格（超过 40 字）根本取不到。两条路都指向同一个后果：发后定位、源站检索把正确候选判成“大小不符”跳过。现在只认“数字 + 显式单位”，优先取以“大小 / Size”开头的单元格，再退回“整格只有一个带单位大小”的短单元格，MediaInfo、文件列表、页眉账户统计都跳过；取不到就不参与大小比对。用 `Tests/BoxSendTests/Fixtures/` 里 7 张真实详情页做了断言。
- 详情页标题比对前先裁掉站点角标（2026-11-24）：`[免费] 剩余时间：23时59分钟` 这类后缀不是标题正文，不裁会把真重复判成“标题不符”。同日又修了裁的位置：原来按内置角标表的顺序裁，`X [2X] [Free]` 会留下 `X [2X]`；现在裁在标题里最早出现的那个角标上。
- 选 announce 与校验 `overrides.announceURL` 的 host 口径统一（2026-11-24）：这两处原来写的是“相等或以 `.host` 结尾”，和“已在做种预筛”的 `host_matches` 不是一套；站点地址带 `www.` 时（如 `www.pthome.net`）会把自己的 `t.pthome.net` 当成别站，没内置别名就报“不在 announceHosts 里”。现在三处都走 `host_matches`。
- 新站字段补齐（2026-11-24）：tjupt 剧集分类的 `cname/ename/specificcat`（上传页静态 HTML 里没有，是分类页动态插入的）、ubits 的 `source_sel[4]`（地区）/`team_sel[4]`（`Other`）都能自动填；不补会被 `takeupload.php` 直接拒绝。
- 站点判定“种子过大”有了明确归宿（2026-11-24）：青蛙（qingwa）的 Poirot 合集回包不是未知页面，而是 `torrent_too_large` 表单自动 submit 到 `offers.php?add_offer=1`，即站点要求改走候选入口。`extract_upload_error` 现在识别这类回包并给出中文原因，`submit_upload` 命中时把覆盖记录落 `pending_offers`（待走候选入口）而不是记失败或当已跳过：不入队重试，但 `status` / `audit` 单列出来给人跟进；候选表单不代填。
- 上传步骤单独放宽超时（2026-11-24）：所有站点共用的 30s 超时对 `takeupload.php` 不够（北洋园连挂两次），现在只给上传这一步 120s，列表/检索仍保持 30s，避免把整站超时整体拉长。
- “等站点侧动作”不再当已跳过（2026-11-24）：新增覆盖状态 `pending_offers`（待走候选入口）与审计词 `pending_offers`；`bsend status` 单列“待跟进（等站点侧动作，程序不自动重试）”，`bsend audit` 合计带“待走候选”数量。这类记录不入队重试、也不算失败。
- 发种失败冷却默认关闭（2026-11-24）：`CoverageSettings.backoff_hours` 默认 0，可由 `assist.backoffHours` 显式打开；拦截与失败共用一个窗口，日志与 GUI 诊断页同步改成“冷却”口径。默认关闭的理由：手动跑一轮由人决定重试，机器不该替人压着。
- 文档口径统一（2026-11-24）：正文不再把命令名当动词/术语（例如不写“reattach 一下”），先讲做什么再给命令；AGENTS.md 补了这条规则。
- 手工补挂入口：`bsend reattach --site <id> --url <详情页>`，可用 `--key` 指定内容；不再接受本地构造的 `.torrent`，避免挂上站点没通过的废种。
- hdvideo 78856、qilin 479362、panda 141276 三条历史结果已补挂完成，qB 校验 100%；qilin 站点仍显示审核不通过（缺图），不影响做种。

### 4.4 旧“没有”记录不再新增

新流程已经不依赖这些记录，但旧扫描路径还会写 `miss`。要逐步停掉：

- 判断阶段不请求站点；
- 不再为“内容 × 站点”写“没有”记录；
- 旧记录保留作负缓存，不主动清空。

### 4.5 拆分 `reseed.rs`

`reseed.rs` 超过 5,000 行，改一处容易碰坏别处。计划拆成：

```
reseed/mod.rs          共享类型和入口
reseed/flow.rs         主流程编排
reseed/judge.rs        判重和决策
reseed/upload.rs       表单、上传、发后处理
reseed/coverage_io.rs  覆盖记录读写
```

只移动代码，不改行为。每拆一批都要编译、跑测试，确认对外接口不变。

## 5. 已知问题

### 5.1 站点适配

- **hdvideo**：必填项已补（`region_sel[4]=2`、`style_sel[4][]=6`），站上已发布 78856；发后定位取错旧种 14469（缺陷已修）。2026-11-23 已手工补挂 78856，qB hash `dd7f6097…` 校验 100%。
- **luckpt**：截图和 BDRip 选档还没完全过。豆瓣图床防盗链补 `Referer` 已解决；`img.pterclub.com` 在本机 TLS 连不上，截图数量仍不足。BDRip 的媒介档已确认是 `Encode(7)`。
- **qingwa**：未覆盖的是成人合集和多片合集，属于内容黑名单范围，不是适配器故障。
- **haitang**：只有曲艺、有声书类目，和影视范围不匹配，已移出白名单。
- **haoxue**：Next.js/Flux 单页应用，静态上传页拿不到，需要按前端接口重做，已移出白名单。
- **agsvpt**：提交域和登录域不一致，返回“你没有该权限”；2026-11-23 已 disable 并移出自动发种白名单。
- **ptba / cspt / dubhe / ptsbao / crabpt / hdbao / tey / oshen / ptfans / itzmx / railgun / lemonhd**：2026-11-23 首次实发（hdbao 2 条，其余各 1 条）并挂回成功；十二站都会改写种子元信息，站上 hash 与构造 hash 不同，已加入 hash 改写白名单。cspt 另有 `tracker.cspt.cc` 别名，tey 另有 `tracker.tey.cc` 别名；dubhe 判重搜索偶发 30 秒超时；ptsbao 的 `source_sel` 是二级分类，已按 type 父类 + 标题自动填；crabpt 此前候选源站 `hdfans` 502，被失败冷却挡住，改走 `hdsky` 源后成功；hdbao 的 `autopost --limit` 计数缺陷已修；tey 表单没有 Blu-ray 媒介档，蓝光/Remux 源会被门禁拦，HDTV/WEB-DL 可发；oshen 前两次站点抖动，第三次才恢复；ptfans 此前上传页/判重超时，恢复后一次通过；railgun、lemonhd 上传回包都没给详情链接，按详情页补挂成功。
- **itzmx**：站点 torrent 文件上限 `266,240 bytes`，大合集/大剧包会被拒；改用 `ubits` 源的《Liar Game》小种（源种 96,934 bytes）首次实发 `13134` 并挂回，qB 校验 100%。后续给该站选种先看 `.torrent` 文件大小，不要只看内容体积；详情页没有“完结/中字”标签，发布后人工补。
- **pthome / nanyang / ubits**：2026-11-24 首次上传成功（`284701` / `226159` / `379266`），站上 hash 与构造的不同，三站已加入 hash 改写白名单，把站点种子下回来挂进 qB，校验 100%。ubits 的 `source_sel[4]`、`team_sel[4]` 不补会被拒，现已自动填。
- **hdfans**：`287791` 已核实是《伪装者》同一条，qB `b1e600a6…` 校验 100%，覆盖记录 `reseeded`，未重发。
- **tjupt**：动态字段已能填；announce 定案 `tracker-public.tjupt.org`（写进 `overrides.announceURL`）；上传受 `takeupload.php` 超时限制，上传单步已放宽到 120s，待实跑验证。
- **audiences**：判重检索被人机验证拦住，且无头 HTTP 复用不了浏览器会话；后续改走 Kimi WebBridge 复用用户浏览器登录态（待验证）。
- **qingwa**：announce 别名已补；《知否》回包“已存在！”且 qB 已在做种；Poirot 合集被站点判 `torrent_too_large`（要求走 `offers.php` 候选），记 `pending_offers`（待走候选入口），不自动重试、`status` 单列待跟进；《伪装者》发 `208575` 并挂回，校验 100%。

### 5.2 工程

- 配置测试的临时目录按进程号共用，偶尔互相干扰。
- 部分临时目录只用进程号做前缀，同一进程并发跑测试可能撞名。
- 界面的自动化测试还很少。
- 旧版 `startup.log` 已没有程序写，可以删。

## 6. 风险

- **qB 有 tracker 不代表站上一定还在**：站点删种、账号被禁都会出现。发布前再确认一次是兜底。
- **qB 没 tracker 不代表一定没发过**：站上可能有，只是本地没挂。确认命中后要转人工核对，不能当失败。
- **改写种子可能泄露 passkey**：tracker 只能取目标站配置里的地址，日志要脱敏。
- **旧“没有”记录可能误导**：新流程不读它们；迁移期不要拿旧覆盖率高低下结论。
- **两个 Agent 同时改 `reseed.rs` 会互相覆盖**：单种子单站点改造结束前，其他工作不要碰这个文件。

## 7. 相关文档

- 为什么这样判断、怎么发一条种子：`ARCHITECTURE.md`
- 数据文件、命令、巡检、排障：`OPERATIONS.md`
- 站点清单和受阻原因：`SITES.md`、`SITE-DEBUG.md`
