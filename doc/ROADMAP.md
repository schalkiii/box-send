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
- 内容类型黑名单、白名单、暂停站、退避、日限额仍然在发布前检查。
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
- 上传后只从回包正文或最终跳转提取下载链接；
- 从目标站下载种子，先核对 `info hash`（新发种默认必须等于上传构造的新 hash；ptba/cspt/dubhe 这类站会改写 `info.source`，只在 pieces 和 `info.name` 都一致时放行）和 pieces（必须与本地源种一致），再独立添加回 qB；
- 站点侧已发布、但下载目标种或挂回 qB 失败时，记 `ReviewPending`、note 前缀 `posted_unresolved` / `existed_unresolved`，不记失败、不进退避、不占发布额度；
- 新增 `bsend reattach --site <id> --url <详情页>` 手工补挂：拉详情页/下载目标种（站点拒绝下载时可用 `--file` 给本地 `.torrent`），按 pieces 找本地内容后走同一条挂回逻辑。

已通过：`coverage_e2e` 26 条全部通过；真实发种 luckpt、sbpt 各一条并挂回校验；hdvideo 78856、panda 141276、qilin 479362 三条历史种子已补挂，qB 校验 100%、tracker 正常。2026-11-23 又首次向 `ptba`、`cspt`、`dubhe`、`ptsbao`、`crabpt`、`hdbao`、`tey`、`oshen`、`ptfans`、`itzmx` 发种（hdbao 2 条，其余各 1 条），十站都出现“站上 hash 与构造 hash 不同”，已按 pieces/标题校验补挂成功，并把十站加入 hash 改写白名单；ptsbao 的 `source_sel` 二级分类也已按 type 父类 + 标题自动填对；hdbao 首次实发还暴露 `autopost --limit` 只数 Posted、不数 `posted_unresolved` 的问题，已修；itzmx 的 torrent 文件上限为 `266,240 bytes`，改用《Liar Game》小种后才打通。

还没完成：

- 用更多真实站点跑完整链路，覆盖“站上已有 / existed / 下载被拒 / 校验失败”等分支；
- 藏宝阁 43994 仍等站点复审，数据层已改好但审核状态未解除。

### 3.2 文档里的站点数据

`SITES.md` 的 147 站表来自配置和上一次表单检查，不是实时状态。站点改版、关站、权限变化后要重新生成，不要拿旧表当现在能不能发的依据。

## 4. 还没做的

### 4.1 发布前再确认一次（代码已接，e2e 已覆盖，真实命中样本待跑）

真要发某一条之前，程序的 `reseed.rs` 第 2.5 步会先查一次目标站：

- 只对“这一轮真要发”的条目查；
- 每条最多查一次；
- 查到站上已有，不重复上传，改为下载目标种子挂回 qB；
- 搜索本身失败（网络、登录、限流）就中止这条并退避，不盲发。

`e2e_duplicate_hit_pushes_target_torrent_to_downloader` 已经覆盖“命中后下载目标种、pieces 比对、挂回 qB”这条路径。真实站点这边还没碰到一次完全相同的命中样本：本轮 ptba/cspt/dubhe 都是“站上已有其他版本”或“没有同条目”，走的是继续上传分支。下一步找一个目标站已有同条目的种子，跑一次只读判重确认，不再重复上传。

### 4.2 上传额度按结果分开算（已完成 2026-11-23）

原问题：旧口径下“站上已有”“重复”“失败”也可能消耗当轮上传次数，热门站查 10 次全是重复，一条也发不出去。

现在 `autopost` 只在 `ReseedOutcome::Posted` 时 `posted_today += 1`；`blocked`、`duplicate`、`failed` 不计入 `posted`。`parse_audit` 也按 `posted` 天数统计，日限额只算真正上传成功。文档之前写成“还在修”，已按代码实际行为改正。

注意：这里的“额度”是**发布额度**，不是上传请求总预算；全局 `roundBudget/uploadBudget` 仍会拦截所有真正的上传动作，两者不冲突。

### 4.3 判重和发后定位

- 上传回包里的详情页 id 不可信，已经出过“站上已发、本地判失败”。
- 发后定位缺陷已修（2026-11-23）：`extract_posted_links` 不再取第一条链接，而是按 id 配对详情/下载；最终跳转是详情页时优先用它；多个链接对不上时两个都不用，转 `posted_unresolved`；`userdetails.php` 不再被当成种子详情。
- 挂回前校验已补（2026-11-23）：新发种下载回来的 `.torrent` 默认必须与上传构造的新 hash 相同，pieces 必须与本地源种一致，否则拒绝挂回；下载/挂回失败不再记失败，落 `posted_unresolved`，判重路径落 `existed_unresolved`。
- 站点改写种子元信息的例外（2026-11-23）：ptba、cspt、dubhe、ptsbao、crabpt、hdbao、tey、oshen、ptfans、itzmx 实机确认站上 hash 与构造 hash 不同，pieces 和 `info.name` 一致；十站加入白名单后自动放行，其他站仍严格拒绝。ptba 已确认被改写的是 `info.source`，cspt/dubhe/ptsbao/crabpt/hdbao/tey/oshen/ptfans/itzmx 的具体改写字段未逐字节核对。
- announce 别名已补（2026-11-23）：baozi、novahd、xingyunge、ptlgs、pthome、agsvpt、cspt、tey 加了站点别名，`www.` 与裸域也归一化；cspt 的 `tracker.cspt.cc`、tey 的 `tracker.tey.cc` 都是“页面域和 tracker 域后缀不同”的例子，tey 不补别名会被误判成 qB 覆盖 0。
- 手工补挂入口已补：`bsend reattach --site <id> --url <详情页>`，可用 `--key` 指定内容、`--file` 给本地构造的 `.torrent`。
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
- **agsvpt**：提交域和登录域不一致，返回“你没有该权限”，建议剔除。
- **ptba / cspt / dubhe / ptsbao / crabpt / hdbao / tey / oshen / ptfans / itzmx**：2026-11-23 首次实发（hdbao 2 条，其余各 1 条）并挂回成功；十站都会改写种子元信息，站上 hash 与构造 hash 不同，已加入 hash 改写白名单。cspt 另有 `tracker.cspt.cc` 别名，tey 另有 `tracker.tey.cc` 别名；dubhe 判重搜索偶发 30 秒超时；ptsbao 的 `source_sel` 是二级分类，已按 type 父类 + 标题自动填；crabpt 此前候选源站 `hdfans` 502 退避，改走 `hdsky` 源后成功；hdbao 的 `autopost --limit` 计数缺陷已修；tey 表单没有 Blu-ray 媒介档，蓝光/Remux 源会被门禁拦，HDTV/WEB-DL 可发；oshen 前两次站点抖动，第三次才恢复；ptfans 此前上传页/判重超时，恢复后一次通过。
- **itzmx**：站点 torrent 文件上限 `266,240 bytes`，大合集/大剧包会被拒；改用 `ubits` 源的《Liar Game》小种（源种 96,934 bytes）首次实发 `13134` 并挂回，qB 校验 100%。后续给该站选种先看 `.torrent` 文件大小，不要只看内容体积；详情页没有“完结/中字”标签，发布后人工补。

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
