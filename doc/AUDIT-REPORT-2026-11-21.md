# box-send 交付前审计报告（2026-11-21）

## 1. 现状

- 代码规模：Rust workspace（`bsend` 库 + `bsend-cli` + `bsend-gui`），另有 Swift/Windows 历史实现；
  当前自动化执行路径以 Rust CLI 为准。
- 发布路径：`run_reseed` 负责源种下载、目标站预审、上传、目标 torrent 回填与 qB 添加；
  `autopost` 负责队列、门禁与日限额。
- 当前运行态：仅 luckpt 开启真实发布；`publishEnabled=true`、`skipPostCheck=true`、
  `skipHashCheck=true`，白名单仅 luckpt。
- qB 版本：5.3.0（WebAPI 2.16）。该版本已移除 `skip_checking`，实际使用 `seedMode=true`。

## 2. 问题清单

| # | 位置 | 问题 | 严重度 | 影响 |
|---|------|------|--------|------|
| 1 | `reseed.rs` / 历史草稿 | 人工草稿曾绕过源种身份校验，发布本地没有的 1080p 版本（绿里奇迹 id 61299） | 高 | 站点污染、假辅种、需人工删除 |
| 2 | `qbittorrent.rs` | 依赖 qB 旧参数 `skip_checking`；qB 5.3 忽略该参数 | 高 | 每条新种进入 hash 校验，错过站点出种时效 |
| 3 | `reseed.rs` | 判重失败（网络异常/预算耗尽）曾继续上传 | 高 | 站点已有同片时仍可能产生重复发布 |
| 4 | `coverage.rs` / 候选生成 | 外部 Python 脚本按标题/IMDb 预筛，脚本不在仓库、无回归测试 | 中 | 候选筛选行为不可重复验证 |
| 5 | `reseed.rs` | 媒介/编码/音轨/标签靠启发式词表，真实站表单有大量变体 | 中 | 预审拦截率高，需要反复补词元 |
| 6 | 文档 | `SITE-REGISTER` 的 55/3/34 门禁统计早于本轮发布硬化；README 尚未说明 qB 5.3 seedMode | 中 | 交接时状态误导 |
| 7 | 测试 | 无 qB HTTP 添加参数、判重失败路径、源分片不一致路径的自动化回归测试 | 高 | 关键事故只能靠真实发种发现 |

## 3. 改进建议

1. **（已完成，需回归）源种身份硬闸**：`reseed.rs` 的 `matching_local_member` 要求源种 pieces hash
   与 qB 中 100% 成员一致；单测待补纯函数测试。
2. **（已完成，需回归）qB 5.3**：`qbittorrent.rs` 同时发送 `skip_checking` 与 `seedMode`；
   建议增加请求体单测（可拆出无网络 form builder）。
3. **（已完成）判重 fail-closed**：IMDb/标题检索异常或预算耗尽即拒绝上传；建议把
   “明确 Miss 才放行”写成状态机测试。
4. **待做**：把候选筛选脚本纳入 `scripts/`，不依赖临时目录；脚本只读、无 token 输出。
5. **待做**：为 `build_upload_fields` 增加真实表单 fixtures（AC3、DD+、H265、粤语、中字、
   REMUX、死图剪除）回归测试。
6. **待做**：更新 `SITE-REGISTER` 的当前门禁/真实发布统计，README 增加 qB 5.3 语义。
7. **待做**：删除绿里奇迹站点条目（id 61299）需用户站内操作；本地 qB 已无该种。

## 4. 优先级与并行性

- P0 串行：qB seedMode 回归 + 源分片硬闸回归 + 判重 fail-closed 回归（均触及 `reseed.rs` / `qbittorrent.rs`）。
- P1 可并行：候选脚本入库（`scripts/`，不影响 Rust 源）；文档同步（`doc/`、`README.md`）。
- P2 串行：真实发种批量回归依赖上述 P0 完成。

## 5. 测试分支执行结果（software-improvement / testing）

- 基线：151 单测 + 17 e2e 全绿。
- 新增回归：
  - `add_form_uses_seed_mode_for_skip_checking`：断言 qB 5.3 必发 `seedMode=true`。
  - `audio_codec_falls_back_to_mediainfo`：标题无音轨码时从 MediaInfo 选 AC-3。
  - `ddplus_maps_to_ddp_eac3`：`DD+` 归一化为 `dd` 后映射 `DDP/E-AC3`。
  - AC3→DD/AC3、H265→H.265/HEVC、粤语/中字识别用例。
- 结果：154 单测 + 17 e2e 全绿。
- 覆盖率：`cargo llvm-cov` 在当前 Windows GNU stable 因缺少 `profiler_builtins` 无法执行，
  是工具链限制，不是测试失败；覆盖率数字暂标 pending，不用假数据。

## 6. 真实发种回归结果（仅 luckpt）

| 内容 | id | 结果 |
|---|---|---|
| 如父如子 | 61298 | 发布成功；qB `418d95ad610f` 100% |
| 雄狮少年 | 61307 | 发布成功；qB `b5c5a2158450` 100%（seedMode 验证） |

- 绿里奇迹 `61299` 是错误版本发布（1080p CMCT vs 本地 2160p UHD），已撤出 qB；
  站点条目待删除，索引 `review_pending`。
- 其余批次在发布前被程序化拦截：目标站同片版本判重、官方预审图片/音轨/标签未过，
  或源站下载链接 404；没有再把错误种子发出去。

## 7. 本轮追加回归（15:19-16:00）

- 新增 `HDR10` 标签只从 UHD/2160p 标题推断，不再从 1080p MediaInfo 误判。
- 英语标签只在无中文字幕、无中文标题/副标题时添加，避免“英语与中文内容冲突”。
- 音频回退改为“标题已有音轨码优先，MEDIAINFO 只在标题缺失时兜底”。
- 新增测试后：155 单测 + 17 e2e 全绿。
- 继续尝试：Hotel Mumbai / The Intern 被 luckpt 同片其他版本判重；土拨鼠之日/武状元苏乞儿/
  赌侠/鹿鼎记2 被预审图片/中文标题等拦截；没有错误上传。
- 当前真实成功仍为 2 条：如父如子 61298、雄狮少年 61307。

## 8. 浪潮与判重复核（15:46）

- 浪潮站点 id `61305` 下载后的 info hash = qB `c4e9a59d9a94...`，pieces 也一致；
  说明“没加 qB”是 qB 5.3 `skip_checking` 失效，不是源/目标错配。用户手动加入后 100%。
- 判重抽样（12 Angry Men / The Intern / 绿里奇迹）目标种子 info hash 与本地成员不同，
  且目标 pieces 与本地所有成员都不同，确属其他版本，不是同一个种子。
- 代码已把判重命中进一步分为：
  - 目标种 pieces 与本地成员一致 → `Assisted`（同内容已覆盖）；
  - pieces 不一致 → `ReviewPending`（其他版本，不传）。
- 新增标题清洗：中文前缀 + 英文正式名时，上传标题去掉中文前缀；纯中文标题不变。
  可消除 LuckAudit “主标题包含中文字符”一类拦截。
- 剩余真实测试：6 条 hdfans 候选最终因图片数量不足/预审标签被拦，未硬发。

## 9. 源站适配器与截图（15:20-16:20）

- 修正 `run --sites`：目标站过滤不再连带过滤源站适配器；qB tracker 指向的源站现在都参与
  source resolver，修正 `wait_source` 的调用层根因。
- 图片不足：已实现本地视频抽帧 → Imgur → `screenshot-overrides.json` → 发布时注入简介；
  Imgur 注入图不参与死图探测，源页图片仍逐张探测。
- 新增上传回包 `existed=1` 硬处理：即使带详情链接也一律 ReviewPending，不记 Rese​eded、不加 qB。
- 鹿鼎记2 的 6419 是无效 existed 页，索引已标 ReviewPending；未加入 qB。
- 156 单测 + 17 e2e；fmt/clippy 零告警。

## 10. 源站/截图/真实新增（16:00-16:30）

- 修正 `run --sites`：目标站过滤不连带源站；tracker 指向的源站现在参与 resolver。
- 图片不足：本地视频抽帧（ffmpeg）→ Imgur → `screenshot-overrides.json` → 发布注入；
  已实测「武状元苏乞儿」预审 100/100。
- 真实新增：The Man From Earth（luckpt 61319，qB `65f1d7a22c33` 100%）。
- Wave 61305 已由用户手动加入 qB，站点 hash/pieces 与 qB `c4e9a59d9a94` 一致。
- 上传回包 `existed=1` 或详情标题不符一律 ReviewPending，不记 Reseeded。
- 新增标题清洗、精确 checkbox 匹配（HDR10 不再误勾 HDR10+）。
- 157 单测 + 17 e2e；fmt/clippy 零告警。

## 11. 最终状态（16:30）

- 新增 `other_version` 账本状态：目标站已有其他版本、pieces 不一致 → 程序化终态，不再进人工确认。
- 已迁移既有“其他版本”记录到 `other_version`。
- 真实新增：The Man From Earth（luckpt 61319，qB `65f1d7a22c33` 100%）。
- 截图链路：本地 ffmpeg 抽帧 → Imgur → `screenshot-overrides.json` → 发布注入；
  武状元苏乞儿预审 100/100，但目标站已有同片。
- 源站适配器修正：`run --sites` 只过滤目标站，tracker 源站仍全部参与。
- 157 单测 + 17 e2e；fmt/clippy 零告警。

## 12. sbpt 适配与实测（续）

### 12.1 适配结论

- sbpt 沿用上一会话的基础注册（启用、`autoReseedSites` 白名单、`reseedRules.sbpt`：
  标题 ≤200、要链接、要 MediaInfo、日限 20、剪死图）；formcheck ok、gatecheck pass。
- 表单实测与通用引擎完全兼容：`type` 分类 / `medium_sel[4]` / `codec_sel[4]` /
  `standard_sel[4]` / IMDb 字段 `url` / 副标题 `small_descr`；无 audiocodec、无中字标签
  （软警告合理）。选档与站内现有种惯例一致（details 21653：电影/Encode/H.265/1080p 无标签），
  **不需要 sbpt 专属配对代码**——luckpt 沉淀的通用逻辑（词元配对、Encode 规则、标题清洗、
  WAF on* 剥离、截图注入、existed 硬处理）已覆盖。
- `team_sel[4]` 软提示不阻断：sbpt 制作组选项只有 HDS/CHD/MySiLU/WiKi/Other，
  CMCT 等配不上；实测站点接受无制作组上传（与 luckpt Rust 侧行为一致）。

### 12.2 源页解析两处修复（luckpt 实测暴露，通用受益）

- **促销倒计时标题**：luckpt h1 带 `[免费] 剩余时间：6天18时` 尾巴，角标不在尾部导致
  整体剥不掉。`strip_promo_suffix` 先剥「剩余时间/剩餘時間」倒计时尾巴（简繁字符集都认，
  正文里提到「剩余时间」但后面不是倒计时的不动），promo 词表补「免費」。
- **转义简介**：luckpt kdescr 存储整体转义一层的 HTML（`&lt;fieldset&gt;…`，202 转义标签
  vs 170 裸标签，裸标签是 nl2br 的 `<br />`）。新增 `unescape_descr`：转义标签 ≥3 且多于
  裸标签，并且富文本标记 ≥2 种或完全没有裸标签时，用新增的 `decode_entities_once`
  单趟解码还原（既有 `decode_entities` 会二次解码 `&amp;lt;`，不能用于此场景，保留不动）。
  混在 nl2br 裸 br 里的 b/i/u 教程文本不会误解码。
- 新增测试：`strips_free_promo_countdown_tail`、`escaped_descr_is_decoded_once`、
  `entities_decode_single_pass`。160 单测 + 17 e2e 全绿；fmt/clippy 零告警。

### 12.3 真实发种尝试：浪潮（luckpt 61305 → sbpt）

- 复审干净：标题 `Die Welle 2008 FRA BluRay 1080p x264 DTS-CMCT`（倒计时已剥）、
  简介还原为裸 HTML、选档 电影/Encode/H.264/1080p、IMDb tt1063669、剪死图 11 张。
- `reseed --draft 2 --yes`：预检通过 → 判重报「未找到」→ 源种分片硬闸通过
  （本地成员 `0092c951c8c0`）→ 上传 → **站点回 existed=1**：sbpt 已有完全相同的种子
  （details 21654，标题/IMDb 均一致）。引擎按 existed 硬处理记 `review_pending`、
  不重复发布、不加 qB——兜底闸按设计生效。
- **判重 fail-open 缺口**：事后实测 sbpt 的 IMDb 区检索（search_area=4）与三级名字查询
  都能命中 21654，引擎 live 复现也 Hit；当时那次 Miss 最可能是瞬态 WAF 挑战/限流页
  （200 + 无结果锚点）被当成「站内没有」。`block_notice` 原本只认限流文案与 403 标题，
  不认 JS 挑战页。已补挑战页标记（`<title>Just a moment`、challenge-platform、cf-chl、
  cf-turnstile、Verify you are human、Checking your browser、DDoS-Guard、人机验证），
  挑战页一律冒错 → 判重 fail-closed 拒绝上传，不再放行到 existed 兜底。
  「Just a moment」只匹配 title 标签，避免撞电影名。新增 `block_pages_are_detected` 用例。

### 12.4 源站定向补全（pieces 反查路径）

- 实测 `plan --sites sbpt --limit 8` 十几分钟不出第一格：上一会话的 WIP 只把**名字检索
  兜底路径**按 qB tracker 定向，`resolve_source_by_pieces`（pieces-hash 精确反查）仍在
  92 站全扫——不可达源站每个吃满 30 秒超时，单格可爬几十分钟。
- 抽出 `source_adapters`（tracker 定向 + 目标站剔除 + 无映射回退全部），pieces 反查与
  名字检索两条路共用，定向语义不再漂移。新增 `source_adapters_follow_tracker_and_exclude_target`
  单测（别名 announce、子域双向、空 tracker 兜底、无映射回退）。
- 161 单测 + 17 e2e 全绿；fmt/clippy 零告警。

### 12.5 辅种实测

- 候选探测：60 条缓存 pieces hash 批量反查 sbpt API → 9 命中，7 格已在做种、
  2 格是 existed=1 留下的 review_pending（浪潮 21654 / 这个男人来自地球 21653）。
  缓存样本里没有「sbpt 有、本地未挂 sbpt tracker」的新候选；这两格经 API 独立
  确认同源后人工复核重置（删格），交辅种臂走完整路径。
- 真实辅种成功 2 条（`run --sites sbpt --only`，修复后二进制）：

| 内容 | sbpt id | 判定 | qB 新种 | 结果 |
|---|---|---|---|---|
| 浪潮 | 21654 | B 级（5 文件 10.00 GB 清单全等，站点重新打包） | `28381e25a622` | 校验 100%，做种，限速已解除 |
| 这个男人来自地球 | 21653 | B 级（3 文件 4.16 GB 清单全等） | `4d9af0097c2c` | 校验 100%，做种，限速已解除 |

- 账本两格 `assisted`（带详情页链接）；浪潮成员 41→42、sbpt tracker 并入
  trackerHosts；次轮扫描正确并入新种（42107→42108，新内容 0）。
- 路径实证：prefetch 多成员 pieces 命中 → passkey 定向刷新 → 下载 bencode →
  judge B 级 → 落盘门（NoSubfolder 映射既有目录）→ 暂停添加 → seedMode 校验
  100% → 解除限速 → assisted。全程 search 预算 2/200、source 0、upload 0。
- 发种侧结论（§12.3）：sbpt 已有完全相同种子时 existed=1 兜底按设计生效，
  不重复发布；这两格最终由辅种臂覆盖，无需发种。

### 12.6 prefetch 多成员 pieces 修复

- 实测暴露：浪潮内容格有 41 条成员种子，sbpt 已有种（21654）的 pieces 与成员
  `0092c951c8c0`（CMCT 原始封装，ph 已缓存）一致，而 prefetch 只查 rep 一条 ph
  （rep 是 luckpt 重新打包 `c4e9a59d9a94`，封装不同）→ pieces 通道漏命中 →
  名字检索用中文内容名「浪潮.2018.FRA…」对 sbpt 德文标题天然 Miss → 落转种草稿。
- 修复：抽出纯函数 `prefetch_keyed`——rep 的 ph（可让 qB 导出现算）+ 其余成员的
  **已缓存** ph（`PiecesApi::cached_pieces_hash`，只查缓存零 qB 开销）一起入批，
  按 ph 去重；API ≤100 项/请求，多成员入批后按块领号逐块查。
- 新增单测 `prefetch_keyed_includes_cached_member_pieces`（rep+缓存成员入批、
  未缓存成员不触发导出）、`prefetch_keyed_dedups_and_caps_contents`（去重、100 段上限）。
  163 单测 + 17 e2e 全绿；fmt/clippy 零告警。
- 已知边界：成员 ph 都没缓存且 rep 封装与目标站不同时仍会漏（名字检索兜底）；
  不为非 rep 成员批量导出 .torrent（每轮数千次 qB 导出不可接受）。

### 12.7 账本手改 corrupt 事故与恢复

- 事故：重置两格 review_pending 的脚本写入 `{status, at, note}`，漏了 `url`/
  `verifyHash` 字段。`SiteCoverage` 的 `url`/`note` 无 `#[serde(default)]`
  （index.rs），缺字段 → 整个 coverage.json 反序列化失败 → 引擎按设计挪成
  `coverage.json.corrupt` 留证、从零重建索引——2943 格历史结论被丢（firstSeen 全重置）。
- 恢复：从重置前备份 `coverage.json.bak-assist-1795259497` 还原全量账本
  （3388 内容 / 25115 格），重建产物挪存 `coverage.json.bak-rebuild-*`；
  重置脚本改写全字段形状（url/note/verifyHash 显式 null）后重跑，
  全账本 0 格缺必需字段。
- 教训：手改 coverage.json 必须保持引擎写出的完整字段形状；引擎的 corrupt
  隔离按设计生效（留证、不崩、不无声覆盖），但「重建」对运营账本代价大，
  手改前先备份、改后校验字段完整性。
- 追加教训（第二次 corrupt）：重置写了 `status: "missing"`——枚举是 snake_case
  且合法值是 `miss`（负缓存，TTL 内会被预筛跳过，不能用于「立即重查」）。
  正确重置 = **删除整个 sbpt 格子**（无格 = 待查）。脚本已改删格模式。
- 另一疏漏：修复后只跑 `cargo test`/`clippy` 不会刷新 `target/debug/bsend.exe`，
  实测轮用的仍是旧二进制——改代码后必须 `cargo build` 再实测。

### 12.8 sbpt 下载两道闸：passkey 过期 + 首次下载确认页

- 实测：pieces 命中后下载 sbpt 种子拿回 HTML。逐层探测定位两道独立的闸：
  1. **缓存 passkey 过期**：pieces-cache 里的 sbpt passkey 与 qB 正在做种的
     tracker live passkey 不一致；旧值被 `download.php` 拒（200 + 25 字节
     「account disabed or parked」），但 pieces API 仍接受——只靠 API 成功
     发现不了过期。修复：`PiecesApi::refresh_passkey`（单站定向、从 qB tracker
     覆盖缓存，与 harvest 的「只补缺失不串号」语义区分），prefetch 命中后
     刷新一次再拼下载直链。live passkey 实测两格都直接回 bencode。
  2. **首次下载确认页**（cookie 路径）：无 passkey 的下载链接 302 到
     `downloadnotice.php?type=firsttime`（NexusPHP 通用首次下载规则确认）。
     修复：`parse_download_notice_form`（scraper 提取 `id="continuedownload"`
     表单，action="?" 解析回当前页，checkbox 只取 checked）+ `download_torrent`
     遇确认页自动 POST 一次再重试原链接。新增单测 `download_notice_form_parsed`。
- 164 单测 + 17 e2e 全绿；fmt/clippy 零告警；CLI 二进制已重建。

### 12.9 formcheck 单站运行覆盖历史（收尾暴露）+ 总账重生成

- 收尾重生成 `doc/SITE-REGISTER.md` 时统计异常：实测从「齐 54 / 缺口 4 /
  失败 46 / 未测 43」塌成「齐 1 / 未测 146」。根因：`cmd_formcheck` 落盘
  `results` 只含本次跑到的站，`--sites sbpt` 单站演练把整个 formcheck.json
  覆盖成 1 条，其余 146 站历史结论在总账里全变「未测」。
- 修复：落盘前按站点合并——读旧文件，保留本次未跑站的最近结论，只覆盖
  重跑站（main.rs `cmd_formcheck`）。总账语义本就是「按站点取最近一次」。
- 恢复：旧文件无备份，从 git HEAD 的 SITE-REGISTER 主表（当时 formcheck
  输出的忠实记录）恢复 103 站 verdict + 原因（fail→error / gap→core /
  ok→soft，与 `cmd_sites` 的原因提取顺序一致），标 `restored: true` 留证；
  fields/selects/at 元数据不可恢复。sbpt 用当前真实条目。恢复后分布
  ok 54 / gap 4 / fail 46，与 HEAD 统计吻合。原文件备份为
  `formcheck.json.bak-sbpt-only`（机器本地）。
- 总账重生成脚本两处防坑：主表后还有「门禁演练」章节，只替换主表、保留
  尾部；统计行按新表重算（pieces 支持 52→53：sbpt 与 ziran 本轮学习为 ok）。
- Lint（markdownlint，仓库约定禁 MD013/MD060）：审计报告与 README 零报错；
  SITE-REGISTER 修掉 MD032/MD055/MD056 三处结构问题后，余 MD034×86（裸 URL）
  / MD033×3（抓页 HTML 片段）登记 not-applicable——均在 CLI 生成的表格内，
  文件约定「表格永远以命令输出为准」，改写即破坏与命令输出的一致性。
- 合并逻辑抽纯函数 `merge_formcheck_results` + 2 条回归单测；全链
  fmt/clippy/test/build 复验（166 单测 + 17 e2e）。
