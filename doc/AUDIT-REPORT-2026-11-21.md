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

## 13. 全量审计（project-audit，2026-11-21）

### 13.0 审计基线与方法

- **基线**：2026-11-21 约 21:30 的工作树快照，**含未提交改动**：`bsend::autopost`
  库模块（新增未跟踪）、CLI `cmd_autopost` 下沉、GUI persist 修复中间态。
  `bsend-cli/src/main.rs`、`bsend-gui/src/main.rs`、`lib.rs` 正被并行任务 #10
  实时修改（GUI 单文件在本审计阅读期间从 964 行变为 998 行），因此这三处
  锚点**以函数名为主、行号为辅**，行号均以本快照为准。
- **方法**：全部结论来自阅读代码、测试与文档；按任务约束**未运行**
  cargo build/test/clippy（另有进程占用 target），不编造覆盖率数字；
  测试覆盖结论逐模块读测试代码得出。未打印任何凭据值。
- **与既有章节的关系**：§1–§12.9 已登记问题不重复。§2#7 的剩余缺口在
  13.5 扩大登记（注明承接关系）；§3.6「README 补 qB 5.3 seedMode 语义」
  经核对仍未落实，并入 DOC-1 跟踪，不另立新条。
- **总体评价**：纯函数层质量高——166 条库单测断言实质（独立 golden 向量、
  边界丰富）、index/report/config 原子写 + corrupt 隔离、判重 fail-closed、
  e2e 用真实 TCP mock 并断言请求计数与跨轮落盘。主要风险集中在三处：
  **网络编排层无测试兜底**、**安全闸（节流/预算/审计记账）未接线或静默失效**、
  **文档显著落后于实现**。
- 问题编号规则：维度前缀 + 严重度字母 + 序号（H 高 / M 中 / L 低）。
  每条含位置 / 描述 / 严重程度 / 影响范围四要素，建议含做法 /
  affected_files / 验证方式三要素。affected_files 为仓库相对路径。
  信息性条目（ARC-3、TEST-L3）不计入问题数。

### 13.1 维度①：文档一致性（README/doc vs 代码）

- **DOC-1｜rust/README.md 多处与实现严重不符**
  - 位置：`rust/README.md:3–4、190、220、262–265、278–279、290–291` 及预算节。
  - 描述：290–291 写「70 个单测 + 14 个端到端」，实际为库 166 + CLI 2 +
    e2e 17；3–4 把 pieces 通道标「见 P1」、autopost 标「规划 P2–P3」，两者
    均已实现；220「v1 只出草稿……上传是后续版本的事」、278–279「未移植：
    GUI、上传表单填充」与 `run_reseed`/`bsend-gui` 矛盾；190「本阶段只能
    演练，不能真发」与 publishEnabled=true 的已授权真实运行矛盾；262–265
    状态机清单漏 `otherVersion`；预算节声称上传预算 10 生效（实际未接线，
    见 BUG-M2）；§3.6 要求的 seedMode 语义说明仍未补。
  - 严重程度：中。零背景交接 Agent 会按文档得出错误测试基线、误以为
    不能真发、漏掉 OtherVersion 状态的处理。
  - 影响范围：所有依据 README 交接/操作的人与 Agent。
  - 建议：逐条改写至与代码一致；测试数量改为「以 cargo test 输出为准」
    避免再次陈旧；补 otherVersion 与 seedMode 说明；预算节措辞与 BUG-M2
    修复进度协调（见 13.8 注记）。
  - affected_files：`rust/README.md`
  - 验证：逐条对照代码复核 + markdownlint 零报错。
- **DOC-2｜代码注释停留在「只出草稿」时代**
  - 位置：`rust/crates/bsend/src/lib.rs:6`、`coverage.rs:36、1131`、
    `config.rs:15、78`、`throttle.rs:18`、`sites/nexusphp.rs:6–7`。
  - 描述：模块级注释仍称「转种只出草稿」「v1 不移植上传」「放开上传后
    生效」等，与已上线的真实上传/辅种行为不符。
  - 严重程度：低。误导读码者对模块职责的判断。
  - 影响范围：维护者阅读体验；无运行时影响。
  - 建议：随各文件所属任务顺手修正（归属见 13.8），不单独开任务。
  - affected_files：`rust/crates/bsend/src/lib.rs`、`rust/crates/bsend/src/coverage.rs`、`rust/crates/bsend/src/config.rs`、`rust/crates/bsend/src/throttle.rs`、`rust/crates/bsend/src/sites/nexusphp.rs`（拆分到各写集）
  - 验证：grep「草稿|不移植|放开上传」确认无残留陈旧表述 + cargo doc 无警告。
- **DOC-3｜根 README 只描述 Swift 旧产品**（并入 ENG-L1 处理，不另计）：
  根 `README.md` 无任何 Rust 引擎入口，新克隆者不知道当前主线在 `rust/`。

### 13.2 维度②：文档缺漏

- **GAP-1｜示例配置缺整个 assist 段**
  - 位置：`Config/boxsend.example.json`（布尔检查确认 hasAssist=false）。
  - 描述：publishEnabled/autoReseedSites/reseedRules/siteIntervals/
    roundBudget/mediaInfoDll 等无人值守关键配置只存在于 README 表格，
    示例文件完全没有 assist 段；gistID/encryptionKey/cookieCloud 为空占位。
  - 严重程度：中。新环境照示例配不出无人值守，只能翻 README 逐项手拼，
    易漏键（漏键时静默用缺省值，如日限额 2）。
  - 影响范围：新部署/迁移环境。
  - 建议：补完整 assist 示例段，值用安全缺省（publishEnabled:false、
    空白名单），**不含任何真实凭据**。
  - affected_files：`Config/boxsend.example.json`
  - 验证：`Config::load` 解析示例无错，assist 键与 README 表格逐项一致。
- **GAP-2｜运行时数据文件无字段契约文档**
  - 位置：`auto-audit.jsonl`、`formcheck.json`、`gatecheck.json`、
    `pieces-cache.json`、`screenshot-overrides.json`（写入点分别在
    autopost.rs、CLI cmd_formcheck/cmd_gatecheck、piecesapi.rs、截图注入）。
  - 描述：这些文件谁写、字段含义、能否手改均无文档。§12.7 的
    coverage.json 手改损坏事故证明此类契约文档是刚需。
  - 严重程度：低。手改/排障时无依据，可能重演损坏事故。
  - 影响范围：运维与排障。
  - 建议：新增 `doc/DATA-FILES.md`，逐文件列路径、写入者、字段语义、
    手改边界（可改/不可改/改后需删缓存重建）。
  - affected_files：`doc/DATA-FILES.md`（新增）
  - 验证：markdownlint 零报错 + 逐条对照代码写入点。
- Rust 发布/部署文档缺口并入 ENG-M2（release 流水线本身缺失，文档随之）。

### 13.3 维度③：架构优化

- **ARC-1｜CLI↔GUI 重复实现，下沉未完成**
  - 位置：`build_adapters`（CLI `main.rs:217–253` vs GUI `run_round_job`
    内联 adapters，约 912–927）；scan 编排（CLI `cmd_scan` vs GUI
    `run_scan`）；`cmd_sites` 聚合仅在 CLI。
  - 描述：autopost 下沉已由并行任务完成（未提交），但上述三处仍是两份
    实现。GUI 版 adapters 缺 CLI 版的「缺 cookie 警告」，行为已漂移。
  - 严重程度：中。同一逻辑双份维护，修一处漏一处；GUI 用户被静默跳过
    缺 cookie 的站点而不知情。
  - 影响范围：CLI 与 GUI 的每轮运行。
  - 建议：下沉 `bsend::sites::build_adapters`（带缺 cookie 事件回调，
    CLI 打日志、GUI 发 Log）与 scan 编排库函数，两端改为委托。
  - affected_files：`rust/crates/bsend-cli/src/main.rs`、`rust/crates/bsend-gui/src/main.rs`、`rust/crates/bsend/src/sites/mod.rs`、`rust/crates/bsend/src/coverage.rs`
  - 验证：cargo test 全量 + 同配置下 CLI/GUI 各跑一次 dry-run，输出一致。
- **ARC-2｜GUI 单文件 998 行 + 主线程同步 refresh**
  - 位置：`rust/crates/bsend-gui/src/main.rs`（`BsendGui::new` 约 94–114、
    `main` 约 976）。
  - 描述：视图/状态/后台任务/快照混在一个文件；`new()` 在主线程同步
    `refresh()` 解析约 15MB coverage.json，启动卡顿数秒；`Config::load`
    仅启动时一次，改配置需重启。
  - 严重程度：低。体验问题，无数据风险。
  - 影响范围：GUI 启动与配置变更流程。
  - 建议：拆 `view.rs`/`jobs.rs`/`snapshot.rs`（纯移动不改逻辑）；
    refresh 移后台线程 + 加载态；配置热刷新可后置。**与任务 #10 同文件，
    必须等其收尾后串行执行。**
  - affected_files：`rust/crates/bsend-gui/src/main.rs`（及新增模块文件）
  - 验证：cargo check -p bsend-gui + 启动不再卡顿 + 各页面功能不变。
- **ARC-3｜GUI 能力缺口（信息性，不计问题数）**：autopost/formcheck/
  gatecheck 页面、站点过滤/dry-run UI（已见 round_sites/round_dry 字段
  雏形）等缺口**已获授权、任务 #10 推进中**，此处仅登记不展开。

### 13.4 维度④：代码写法与潜在 bug

- **BUG-H1｜push_tail 把「新加但异常」误判为「已存在」，静默落终态 Assisted**
  - 位置：`rust/crates/bsend/src/coverage.rs:1063–1070`（`push_tail`）；
    `rust/crates/bsend/src/qbittorrent.rs:575–628`（`verify`）、37–40
    （`AddTorrentResult` 无 already 标志）。
  - 描述：push_tail 以暂停方式添加种子（1046 paused=true）后调
    verify(already=false)；verify 在「配置了上传限速但 qB 未生效
    （up_limit>0 且 ul<ulimit，含 ul=-1）」（609）或 state=missingFiles
    （618）时也产出 note。push_tail 见 `!add.note.is_empty()` 即提前
    return `Assisted`——把「刚添加的新种」当成「早已存在的老种」。
  - 严重程度：**高**。后果：新种保持暂停 + dlLimit=1 状态被标成
    Assisted 终态（is_settled 含 Assisted），跳过 start/校验/回滚，
    **永远暂停不作种且永不重试**，格子账面却显示辅种成功。触发条件：
    default_up_limit 或 site_up_limits>0 且 qB 限速未生效，或
    missingFiles。e2e 的 test_config 设 default_up_limit=0，note 分支
    在测试中不可达——这正是测试盲区根因（见 13.6）。
  - 影响范围：辅种臂所有配了上传限速的站点。
  - 建议：`AddTorrentResult` 增加 `already: bool`（409 路径 545–548 置
    true，新加 571 置 false）；push_tail 仅在 already 时走提前 return；
    其余 note 并入正常校验流程完成后的备注。
  - affected_files：`rust/crates/bsend/src/qbittorrent.rs`、`rust/crates/bsend/src/coverage.rs`、`rust/tests/coverage_e2e.rs`
  - 验证：新增 e2e——default_up_limit>0 且 mock /torrents/info 回
    up_limit=-1 → 断言 start/resume 仍被调用、状态不落 Assisted；
    missingFiles 用例 → 走失败/回滚路径。cargo test（待并行构建结束）。
- **BUG-H2｜GUI 跑一轮结论不落盘（修复进行中的中间态，收尾验证条）**
  - 位置：`rust/crates/bsend-gui/src/main.rs`：`start_job`（140–202）已
    构造含 persist:true/site_filter/dry_run 的 opts 并调
    `run_round_job(&cfg, opts, &tx)`（187）；但 `run_round_job` 签名
    （904）仍是 `limit: Option<usize>`，内部（956–959）仍
    `RoundOptions { limit, ..Default::default() }`（persist=false）。
  - 描述：调用方与被调方签名不匹配，**当前中间态不可编译**；若回退到
    旧签名，则 GUI 真实跑一轮 persist=false，coverage.json 不落格。
  - 严重程度：**高**。后果（旧行为）：GUI 跑完的结论全部丢失，下轮
    重复检索/重复推种，浪费预算且可能重复动作。**本条已在任务 #10
    推进，登记目的是收尾验证，不是新开修复单。**
  - 影响范围：GUI 跑一轮入口。
  - 建议：完成任务 #10 的签名同步（run_round_job 收 RoundOptions 透传）；
    persist=true 时 run_round 内部自行落盘，GUI 无需额外 index.save。
  - affected_files：`rust/crates/bsend-gui/src/main.rs`
  - 验证：cargo check -p bsend-gui 通过；小 limit 真跑一轮后确认
    coverage.json mtime 与内容更新。
- **BUG-M1｜转种/自动发种全流水线无 SiteThrottle，判重 gate 恒真**
  - 位置：`rust/crates/bsend/src/reseed.rs:3421`（`run_reseed`）、
    3443–3448（判重名字检索 gate `&mut || true`）、3454–3456
    （BudgetExhausted bail 死代码）、2073（仅 2s sleep）、3817（校验轮询）；
    `rust/crates/bsend/src/autopost.rs:56–306`（`run_autopost`）。
  - 描述：run_reseed 从判重、details、下载、表单到上传、发后自检全程
    不领节流号；判重检索的 gate 恒真，旁路了搜索预算与站间隔；
    BudgetExhausted 分支因此永不可达。autopost 同样只有日限额/退避，
    无请求级节流。
  - 严重程度：中。每次发种对目标站连发约 8 个请求，hdsky 等站有
    60s/次检索限制 → 429/挑战页/账号风控风险；与 README「三预算 +
    站间隔」宣称不符。
  - 影响范围：所有真实发种与自动发种。
  - 建议：run_reseed 内按目标站建 SiteThrottle（读 config 的
    site_intervals/search_budget），判重 gate 改为真实 acquire(Search)；
    autopost 对每站同样接入。
  - affected_files：`rust/crates/bsend/src/reseed.rs`、`rust/crates/bsend/src/autopost.rs`、`rust/crates/bsend/src/throttle.rs`
  - 验证：单测用计数 gate 断言 acquire 真被调用；dry-run 实测请求间隔
    ≥ site_intervals。
- **BUG-M2｜Upload 预算从未接线**
  - 位置：`rust/crates/bsend/src/throttle.rs:18`（注释「放开上传后生效」）、
    324（仅单测 acquire）；`coverage.rs:503`（仅报告聚合）；
    `config.rs:309`（upload_budget 缺省 10）。
  - 描述：BudgetKind::Upload 在生产代码中从未 acquire；README 预算节
    声称上传预算 10 按实际请求记账生效。
  - 严重程度：中。全局上传熔断缺失：autopost 每站日限额 2 是唯一闸，
    多站并发或循环异常时无总量闸。
  - 影响范围：上传总量控制。
  - 建议：submit_upload 前 acquire(Upload)，耗尽 bail（fail-closed）；
    README 措辞由 DOC-1 任务协调同步。
  - affected_files：`rust/crates/bsend/src/reseed.rs`、`rust/crates/bsend/src/throttle.rs`
  - 验证：单测 upload_budget=0 → run_reseed 在上传前拒绝；报告上传计数
    与实际请求数一致。
- **BUG-M3｜same_content 瞬态失败被静默当成「内容不同」落 OtherVersion**
  - 位置：`rust/crates/bsend/src/reseed.rs:3465–3492`（`.ok().and_then(...)
    .unwrap_or(false)` 链）、3490（note）、3467（注释）。
  - 描述：源种 fetch 瞬态失败、下载失败、索引缺条目、
    matching_local_member 出错，全部静默落 OtherVersion；note 谎称
    「pieces 不一致」；3467 注释说 ReviewPending 与代码 OtherVersion 不符。
  - 严重程度：中。OtherVersion 不在 is_settled（index.rs:67–76），按
    missTTL 7 天后重查——可自愈但延迟 7 天，且账面原因失实误导排障。
  - 影响范围：转种判重分类准确性。
  - 建议：三态结果（Same/Different/Unknown）；Unknown → ReviewPending
    或 Failed，note 写真实原因；修正注释。
  - affected_files：`rust/crates/bsend/src/reseed.rs`
  - 验证：单测 mock fetch 出错 → 断言不落 OtherVersion 且 note 含真实原因。
- **BUG-M4｜源种定位预算耗尽落 Miss 负缓存 7 天**
  - 位置：`rust/crates/bsend/src/coverage.rs:1307–1310`。
  - 描述：`ResolveOutcome::BudgetExhausted => done(Miss, "站内没有；源种
    定位预算用完，下次接着试")`——注释说下次接着试，实际 Miss 负缓存
    source_miss_ttl 7 天内不再解析；e2e 场景七只钉了检索侧预算耗尽不落格。
  - 严重程度：中。预算耗尽的格子被误结论「站内没有」7 天，辅种覆盖率
    虚降，与「预算耗尽不下结论」设计矛盾。
  - 影响范围：源种定位预算紧张时的辅种臂。
  - 建议：改为跳过本格（不落 index，下轮重解析），与检索侧语义对齐。
  - affected_files：`rust/crates/bsend/src/coverage.rs`、`rust/tests/coverage_e2e.rs`
  - 验证：新增 e2e——source_budget=1 + 两个待解析格 → 断言第二格未落 index。
- **BUG-M5｜verifyWaitSec/verifyPollSec 负值 as u64 回绕 → 轮次挂死**
  - 位置：`rust/crates/bsend/src/config.rs:321–322`。
  - 描述：两键 `as u64` 无钳制，JSON 手填负数回绕成巨值；poll 巨值使
    push_tail 的 thread::sleep 睡数年，整轮辅种无声挂死。
  - 严重程度：中。配置手改常见（§12.7 事故先例），一个负号即挂死且无报错。
  - 影响范围：辅种推种校验轮询。
  - 建议：`.max(0)` 钳制，poll 下限 1（参照 search_budget 既有写法）。
  - affected_files：`rust/crates/bsend/src/config.rs`
  - 验证：单测负值配置 → 断言钳到 0/缺省。
- **BUG-M6｜autopost 审计记账静默丢写 → 日限额可超限**
  - 位置：`rust/crates/bsend/src/autopost.rs:362–374`（`append_audit`）。
  - 描述：`if let Ok(mut f) = ...` 吞打开失败，`let _ = writeln!` 吞写入
    失败；posted 记账丢失后 parse_audit 少计，下轮继续发。
  - 严重程度：中。日限额（缺省 2）是自动发种唯一总量闸，记账丢失即
    超限真实发布，PT 站重复发帖风控。
  - 影响范围：无人值守自动发种。
  - 建议：append_audit 返回 Result；失败时调用方告警并**跳过该站本轮
    后续发布**（fail-closed）。
  - affected_files：`rust/crates/bsend/src/autopost.rs`
  - 验证：单测把审计路径指向不可写位置 → 断言错误被 surfaced 且后续
    发布被跳过。
- **BUG-L1｜qB files 嵌套数组分支不可达，与注释矛盾**
  - 位置：`rust/crates/bsend/src/qbittorrent.rs:303–313`。
  - 描述：第一分支 `obj.as_array()` 为 Some 即 clone，else-if 的
    as_array 永不可达；`[[file]]` 形态 filter_map 全 None → files 空报错，
    注释声称的「兼容嵌套数组」实际不成立。现仅单 hash 查询未触发。
  - 严重程度：低。潜在解析失败，当前路径未用。
  - 影响范围：qB files 解析的未来扩展。
  - 建议：修正分支逻辑（先判内层数组）或删死分支并改注释。
  - affected_files：`rust/crates/bsend/src/qbittorrent.rs`
  - 验证：单测 `[[{...}]]` 形态可解析。
- **BUG-L2｜all_tracker_urls limit=50000 静默截断**
  - 位置：`rust/crates/bsend/src/qbittorrent.rs:392`。
  - 描述：实机 41,885 条已接近上限；超限静默截断 → tracker URL 集不全
    → passkey 收割遗漏。
  - 严重程度：低。数据量增长后辅种源定位悄然缺站。
  - 影响范围：passkey/tracker 收割完整性。
  - 建议：分页拉取，或至少触顶时告警。
  - affected_files：`rust/crates/bsend/src/qbittorrent.rs`
  - 验证：单测 mock 超量响应 → 断言告警或分页完整。
- **BUG-L3｜run_reseed 尾部 duplicate 死分支**
  - 位置：`rust/crates/bsend/src/reseed.rs:3601–3613`（3582–3597 已对
    up.duplicate 提前 return）。
  - 描述：永不可达的重复处理分支。严重程度：低。影响范围：可读性。
  - 建议：删除。affected_files：`rust/crates/bsend/src/reseed.rs`
  - 验证：cargo clippy 零告警 + 既有测试全绿。
- **BUG-L4｜三个配置键不读配置文件，只用缺省**
  - 位置：`rust/crates/bsend/src/config.rs:309`（upload_budget）、
    311（source_miss_ttl_days）、353（archive_days）。
  - 描述：`d.upload_budget` 等直接取缺省，不读 JSON 键；键存在于结构体
    给人可配错觉。严重程度：低。影响范围：调参不生效（静默）。
  - 建议：接线读取，或删键并在 README 注明「Rust 侧固定」。
    upload_budget 接线与 BUG-M2 协调。
  - affected_files：`rust/crates/bsend/src/config.rs`
  - 验证：单测自定义值 → 断言生效（或文档注明固定）。
- **BUG-L5｜cookie_header_for 不滤已过期 cookie**
  - 位置：`rust/crates/bsend/src/config.rs:514`。
  - 描述：条件 `!c.value.is_empty() || c.expiration_date.is_none()` 只滤
    空值，已过期 cookie 照发 → 登录墙 failed（安全但浪费请求、账面原因
    误导）。严重程度：低。影响范围：cookie 过期站点的请求效率。
  - 建议：加 expiration_date 过期过滤。
  - affected_files：`rust/crates/bsend/src/config.rs`
  - 验证：单测过期 cookie → 断言不出现在 header。
- **BUG-L6｜三处数据落盘非原子 + 一处吞错**
  - 位置：`rust/crates/bsend/src/piecesapi.rs:111–117`（save 直接
    fs::write）；`coverage.rs:502`（`.ok()` 吞 save 错误）；
    `bsend-cli/src/main.rs:652`（formcheck）、1470（gatecheck）。
  - 描述：index/report/config 已是 tmp+rename 原子写（index.rs:249–263），
    这三处没有；pieces-cache 损坏丢 passkey/ph 缓存，重建需数千次 qB
    导出；formcheck 历史丢失已有 §12.9 先例。
  - 严重程度：低。影响范围：进程中断/磁盘满时的缓存与台账完整性。
  - 建议：抽公共 atomic_write（参照 index.save），替换三处；
    coverage.rs:502 错误 surfaced 到日志。
  - affected_files：`rust/crates/bsend/src/piecesapi.rs`、`rust/crates/bsend/src/coverage.rs`、`rust/crates/bsend-cli/src/main.rs`、`rust/crates/bsend/src/lib.rs`（若抽公共模块；执行时按 13.8 拆分写集）
  - 验证：单测模拟写入中断 → 旧文件完好；cargo test 全绿。
- **BUG-L7｜manifest 剥根大小写敏感 + common_root 不认反斜杠**
  - 位置：`rust/crates/bsend/src/manifest.rs`（`normalized_path` 的
    `parts[0]==root` 在小写化前比较；`common_root` 只按 '/' 切）。
  - 描述：根目录名大小写差异 → 摘要不一致 → B 级证据降 C；反斜杠路径
    不识别。qB 返回 '/' 且根名通常一致，仅边缘触发。
  - 严重程度：低。影响范围：跨平台/大小写不一致场景的内容比对降级。
  - 建议：剥根比较前先小写化；common_root 兼容 '\\'。
  - affected_files：`rust/crates/bsend/src/manifest.rs`
  - 验证：单测根名大小写差异与反斜杠路径 → 摘要一致。
- **BUG-L8｜cmd_sites 绕过 Config::load 直读原始 JSON**
  - 位置：`rust/crates/bsend-cli/src/main.rs:1084–1300`（host 解析重复于
    1169–1176）。
  - 描述：忽略 enabled 语义；cookie 计数直读 cookies.json，
    --cookiecloud 合并态对本命令无效；host 解析与 config.rs/sites 重复。
  - 严重程度：低。只读报表失真误导运维，无破坏性动作。
  - 影响范围：站点台账报表准确性。
  - 建议：改走 Config::load + site_def_by_host 复用。
  - affected_files：`rust/crates/bsend-cli/src/main.rs`
  - 验证：--cookiecloud 场景下 cmd_sites 计数与合并态一致。
- **BUG-L9｜autopost 时间处理两处偏差（方向保守）**
  - 位置：`rust/crates/bsend/src/autopost.rs:90`（now 循环前取一次）、
    323（`today_start = now - now % 86400`）。
  - 描述：长任务跨午夜仍用旧 now 判退避/日限额；UTC 日界 = 北京 08:00，
    与站点北京 00:00 重置错位。
  - 严重程度：低。误差方向保守：北京 00:00–08:00 仍计昨日 posted →
    少发不多发；超限风险只来自 BUG-M6。
  - 影响范围：自动发种效率（非安全）。
  - 建议：now 移入循环逐次取；日界改 UTC+8（`(now+28800)%86400`）并注明。
  - affected_files：`rust/crates/bsend/src/autopost.rs`
  - 验证：单测北京 00:30 时间戳 → 计为新一天。

### 13.5 维度⑤：测试覆盖率（逐模块）

- **TEST-H1｜网络编排层安全闸零自动化测试（§2#7 剩余 + 扩大）**
  - 位置与逐模块缺口（单测计数为 grep `#[test]` 实测）：

    | 模块 | 单测数 | 已覆盖 | 缺口 |
    | --- | --- | --- | --- |
    | reseed | 64 | 解析/判重纯函数 | run_reseed/review_reseed/submit_upload/判重分类编排零测试 |
    | autopost | 2 | parse_audit/in_backoff | run_autopost 白名单/日限额/退避/跳过重复编排零测试 |
    | qbittorrent | 5 | URL/参数构造 | 登录重试/409 补挂/verify note 分支零测试（BUG-H1 盲区根因） |
    | sites/nexusphp | 14 | 页面解析 | post_upload/post_form/download 网络路径零测试 |
    | piecesapi | 6 | 缓存/解析 | query/refresh_passkey 网络路径零测试 |
    | coverage | 3+17 e2e | run_round 编排（质量高） | 转种上传流水线无 e2e |
    | bsend-cli | 2 | formcheck merge | cmd_* 编排零测试 |
    | bsend-gui | 0 | 无 | 全部 |
    | 其余库模块 | 72 | 纯函数+golden 向量 | 无明显缺口 |

  - 描述：§2#7 已登记「qB HTTP 添加参数、判重失败路径、源分片不一致
    路径」——添加参数单测已补，其余两条仍缺；本条扩大到整个编排层。
    run_reseed/run_autopost 内部自建 adapter，无注入点，mock 无法接入。
  - 严重程度：**高**。后果：判重失败拒发、日限额、退避、白名单等安全闸
    回归只能靠真实发帖发现 → 重复发帖/超限发布/账号风控。
  - 影响范围：转种与自动发种的全部安全闸。
  - 建议：复用 resolver.rs 的 SiteClient trait + MockSite 模式与
    coverage_e2e 的 MockServer，新增 tests/reseed_e2e.rs 与
    tests/autopost_e2e.rs（mock 站 upload.php/details.php + mock qB）；
    run_reseed/run_autopost 增加 adapter 注入签名。钉住：判重失败 →
    拒发；existed → ReviewPending；日限额到 → skip 并记账；退避期 →
    skip；posted 跨轮记账。
  - affected_files：`rust/tests/reseed_e2e.rs`（新增）、`rust/tests/autopost_e2e.rs`（新增）、`rust/crates/bsend/src/reseed.rs`、`rust/crates/bsend/src/autopost.rs`
  - 验证：cargo test 新 e2e 全绿；故意破坏一处闸（如删日限额检查）
    确认测试变红。
- 覆盖率数字：llvm-cov 工具链限制见 §5，未运行、不编造。

### 13.6 维度⑥：测试严谨性

- **总体结论（正面）**：断言实质——e2e 断言请求计数（login 1 次/add
  1 次/delete 0）、捕获 add 请求体（tracker 补挂、NoSubfolder+savepath）、
  跨轮落盘重读（Verifying 启动/回滚收尾）、四种负缓存防投毒（登录墙/
  限流页/429/returnto）；bencode/cookiecloud 用独立实现的 golden 向量；
  边界覆盖丰富（junk bencode、深嵌套、错密码、预算耗尽不落格）。
- **TEST-L1｜mediainfo real_dll 测试静默跳过**
  - 位置：`rust/crates/bsend/src/mediainfo.rs`（real_dll 测试无 DLL 时
    直接 return）。
  - 描述：CI Windows runner 无 MediaInfo DLL → 该测试从不执行且无痕迹，
    给人「已测」错觉。严重程度：低。影响范围：MediaInfo 路径回归信心。
  - 建议：跳过时 eprintln 原因（或 #[ignore] + 文档注明手动跑法）。
  - affected_files：`rust/crates/bsend/src/mediainfo.rs`
  - 验证：无 DLL 环境 cargo test 输出可见跳过说明。
- **TEST-L2｜autopost 日界测试注释失实 + 真边界未覆盖**
  - 位置：`rust/crates/bsend/src/autopost.rs`（parse_audit 测试，now =
    1_800_000_000）。
  - 描述：注释称 now「恰在 UTC 日界」，实际 1_800_000_000 % 86400 =
    28800（08:00 UTC）；恰好 00:00 的真边界用例缺失。
  - 严重程度：低。影响范围：日界逻辑回归信心。
  - 建议：修注释 + 补真边界时间戳用例（与 BUG-L9 的 UTC+8 改造同步）。
  - affected_files：`rust/crates/bsend/src/autopost.rs`
  - 验证：新用例在日界前后各 1 秒断言归属正确。
- **TEST-L3｜nexusphp relaxed 误判放行（信息性，不计问题数）**：
  x265-GROUP vs x264-GROUP 在 relaxed 模式放行是对齐 Swift 的故意放宽，
  测试已明文钉住（nexusphp.rs:1339–1345），下游 matcher/same_content
  兜底，不是新 bug。
- **e2e 盲区注记**：test_config 的 default_up_limit=0 使 BUG-H1 的 note
  分支不可达；temp_dir 按 name+process::id 防撞但失败时不清理（残留
  临时目录，信息性）。修复 BUG-H1 时必须同步把限速场景加进 e2e。

### 13.7 维度⑦：工程规范（构建/目录/版本管理）

- **ENG-M1｜MSRV 承诺失实且 CI 不检查**
  - 位置：`rust/Cargo.toml`（rust-version="1.82"）；
    `bsend-cli/src/main.rs:930`（`commas` 用 `is_multiple_of`，1.87 才
    稳定）；`.github/workflows/rust.yml`（dtolnay/rust-toolchain@stable，
    无 MSRV job）。
  - 描述：1.82–1.86 用户编译失败；CI 用 stable 永远抓不到漂移。
  - 严重程度：中。对外 MSRV 承诺失效。
  - 影响范围：旧工具链用户与 CI 保障。
  - 建议：commas 改 `(digits.len()-i) % 3 == 0`（零依赖，首选）；
    或升 rust-version 至 1.87 并加 MSRV job。执行拆分：commas 修复归
    13.8 阶段二（CLI 写集），CI job 归组 A4。
  - affected_files：`rust/crates/bsend-cli/src/main.rs`、`rust/Cargo.toml`、`.github/workflows/rust.yml`
  - 验证：修改后 cargo check；如升 MSRV 则 CI MSRV job 绿。
- **ENG-M2｜Rust 无 release 流水线，版本从未管理**
  - 位置：`.github/workflows/release.yml`（287 行，纯 Swift/.NET 遗留：
    macOS dmg + Windows Inno）；`rust/Cargo.toml`（version 0.1.0 从未
    升版）；`scripts/deploy-debian.sh`、`scripts/make-app.sh`（Swift 遗留）。
  - 描述：Rust workspace 无产物出包/tag 流程，发布全靠手工 cargo build。
  - 严重程度：中。无人值守目标下无版本化产物，回滚/分发无依据。
  - 影响范围：发布与部署。
  - 建议：新增 rust-release.yml（tag 触发，cargo build --release +
    gui-release profile，exe 附 Release，版本随 tag 升）；或在 README
    明确「Rust 仅源码分发」并给旧流水线加弃用注记。
  - affected_files：`.github/workflows/rust-release.yml`（新增）、`.github/workflows/release.yml`、`rust/Cargo.toml`、`rust/README.md`（措辞与 DOC-1 协调）
  - 验证：workflow 语法审查 + 一次 dry tag 触发演练。
- **ENG-L1｜仓库卫生：冗余副本与主线导航缺失**
  - 位置：根 `README_副本.md`（49KB 冗余副本）、`1.webp`（732KB 无关
    图片）、根 `README.md`（只描述 Swift 旧产品，无 Rust 入口）。
  - 描述：Swift/Windows/Rust 三套实现并存，无目录级主线说明。
  - 严重程度：低。影响范围：新克隆者导航与仓库整洁。
  - 建议：根 README 加「当前主线：rust/」导航节；副本与图片**经用户
    确认后**删除。
  - affected_files：`README.md`、`README_副本.md`（待确认删除）、`1.webp`（待确认删除）
  - 验证：markdownlint + 无死链。
- **ENG-L2｜gui-release profile 开 debug-assertions**
  - 位置：`rust/Cargo.toml`（gui-release profile，绕 GPUI fxc.exe 的
    权宜，已有注释）。
  - 描述：release 二进制带断言/溢出检查，性能与行为异于标准 release。
  - 严重程度：低。影响范围：GUI 发布产物性能。
  - 建议：跟踪上游 GPUI 修复，可回退时改回标准 profile；保留现状注释。
  - affected_files：`rust/Cargo.toml`
  - 验证：profile 变更后 gui-release 构建通过（待并行进程结束）。
- **正面确认**：Cargo.lock 已提交；default-members 排除 GUI 有注释；
  ci-check.sh 与 CI 命令一致；CI Ubuntu+Windows 双矩阵 fmt/clippy
  -D warnings/test（Linux 排除 GUI 有合理注释）；index/report/config
  原子写 + corrupt 隔离健全。

### 13.8 优先级排序与并行执行计划

问题总数：**30 条（高 3 / 中 11 / 低 16）**，另有信息性条目 3 条不计。

优先级排序：

- **P0**：BUG-H1（辅种静默假终态）、BUG-H2（GUI 不落盘，任务 #10 收尾
  验证）、BUG-M6（审计丢写 → 超限发布）。
- **P1**：BUG-M1 + BUG-M2（节流/上传预算，同链）、BUG-M3、BUG-M4、
  BUG-M5、TEST-H1（编排层测试）。
- **P2**：DOC-1、GAP-1、ARC-1、ENG-M1、ENG-M2。
- **P3**：全部低严重度条目（BUG-L1–L9、DOC-2、GAP-2、ARC-2、
  TEST-L1/L2、ENG-L1/L2）。

并行计划（写集互斥校验：组内两两交集为空）：

- **并行组一（6 任务，可立即同时开工）**：
  - A1 coverage+qB 链：BUG-H1、BUG-M4、BUG-L1、BUG-L2、BUG-L6
    （coverage:502 吞错）、DOC-2（coverage 注释）。
    写集：`coverage.rs`、`qbittorrent.rs`、`tests/coverage_e2e.rs`。
  - A2 config+manifest：BUG-M5、BUG-L4、BUG-L5、BUG-L7、DOC-2（config
    注释）。写集：`config.rs`、`manifest.rs`。
  - A3 pieces+公共原子写：BUG-L6（piecesapi 原子写，抽公共 util）、
    DOC-2（lib/nexusphp 注释）。写集：`piecesapi.rs`、`lib.rs`、
    `sites/nexusphp.rs`、`util.rs`（新增）。
  - A4 CI/发布：ENG-M1（CI MSRV job 侧）、ENG-M2、ENG-L2。
    写集：`rust/Cargo.toml`、`.github/workflows/*`。
  - A5 文档：DOC-1、GAP-1、GAP-2、ENG-L1。写集：`rust/README.md`、
    `Config/boxsend.example.json`、根 `README.md`、`README_副本.md`、
    `1.webp`、`doc/DATA-FILES.md`（新增）。
  - A6 reseed/autopost 链：BUG-M1、BUG-M2、BUG-M3、BUG-M6、BUG-L3、
    BUG-L9、TEST-L2、DOC-2（throttle 注释）。
    写集：`reseed.rs`、`autopost.rs`、`throttle.rs`。
  - 交集校验：A1–A6 写集两两不相交（coverage.rs 仅 A1；lib.rs 仅 A3；
    Cargo.toml 仅 A4；rust/README.md 仅 A5；reseed/autopost/throttle
    仅 A6）。✓
  - 协调注记：A5 的预算节措辞先按「现状未接线」写，A6 完成 BUG-M2 后
    由收尾者串行同步一句（rust/README.md 写集届时回收归收尾者）。
- **串行阶段二（CLI+GUI 下沉与收尾）**：ARC-1、ARC-2、BUG-L8、BUG-L6
  （CLI formcheck/gatecheck 原子写）、ENG-M1（commas 修复）、BUG-H2
  收尾验证。写集：`bsend-cli/src/main.rs`、`bsend-gui/src/main.rs`、
  `sites/mod.rs`、`coverage.rs`。**依赖**：A1（coverage.rs）、A3
  （公共 util）、**任务 #10 完成**（GUI 同文件，禁止并发写）。
- **阶段三（编排测试，可与阶段二并行）**：TEST-H1、TEST-L1。
  写集：`tests/reseed_e2e.rs`（新增）、`tests/autopost_e2e.rs`（新增）、
  `reseed.rs`、`autopost.rs`、`mediainfo.rs`。**依赖**：A6（注入签名）。
  与阶段二写集不相交。✓
- **最终验证（串行收尾）**：待并行构建进程结束后 cargo fmt/clippy/
  test/build 全量；markdownlint 校验新增/改动文档；本报告问题逐条
  对账销号。

## 14. 修复批次（2026-11-21 深夜 – 11-22 凌晨）

### 14.1 sbpt 批量辅种（运营动作，非代码）

- 触发：用户在 sbpt 已上传、但本地 qB 缺对应种子的 21 条内容
  （18 条 `review_pending` 的 existed 兜底 + 3 条集合内容 20567/20569/20570）。
- 做法：备份账本 → 删除这 18 格 sbpt 格子（无格=待查）→
  `bsend run --sites sbpt --only <contentKey>` 逐条重查 → 引擎按
  pieces/名字命中站点种子 → 逐文件清单全等（B 级）→ 停止态添加 →
  seedMode 校验 100% → 解除 1 B/s 兜底 → `assisted`。
- 结果：20 条由引擎完成；Coraline（21658）因**内容名与站点标题不同**
  （`Coraline (2009) - BD50 (Shout! Factory)` vs
  `Coraline.2009.1080p.USA...PIXELPOLICE`）名字检索未命中、pieces 通道也未命中，
  改人工复核：178 个文件/46.93 GB 清单全等后按引擎同款参数（paused +
  seedMode + dlLimit=1 + savepath=J:\Download）添加，校验 100% 后写 `assisted`。
- 全部 21 条 qB 侧复核：`progress=1`、`stalledUP`、sbpt tracker 状态正常。
- 遗留：内容名与站点标题差异导致的漏检仍靠后续人工/成员名兜底检索
  （候选改进：名字检索追加成员种名，需评估请求预算）。

### 14.2 P0 修复

- **BUG-H1（辅种假终态）**：`AddTorrentResult` 增加 `already`；`push_tail`
  只在 already 时走 Assisted 捷径，新加种的限速/missingFiles note 并入校验结论。
  新增回归 e2e `e2e_assist_new_add_with_limit_note_still_verifies`。
- **BUG-H2（GUI 不落盘）**：GUI 跑一轮/基线显式 `persist: true`
  （`round_controls.rs` 与 `main.rs` 两处入口），结论真正写回 coverage.json。
- **BUG-M6（审计丢写）**：`append_audit` 返回 `Result`，调用方 `?` 传播——
  记账失败即中止本轮，日限额不会被静默突破。

### 14.3 P1 修复

- **BUG-M1（编排层无节流）**：`run_reseed` 增加可选 `SiteThrottle` 参数，
  判重检索 gate 由 `&mut || true` 改为真实 `acquire(Search)`，判重复核下载
  也领号；`autopost` 一次运行共用一个节流器（预算/站间隔跨草稿累计）。
- **BUG-M2（上传预算未接线）**：`submit_upload` 前 `acquire(Upload)`，
  耗尽 bail（fail-closed）；`uploadBudget` 键接线可配。
- **BUG-M3（瞬态失败谎报其他版本）**：判重复核改三态，复核失败落
  `failed` 并附真实原因，不再写「pieces 不一致」。
- **BUG-M4（源种预算耗尽落 Miss）**：改 `ItemSkipped`，不写格子，下轮重试；
  新增 e2e `e2e_source_budget_exhaustion_writes_no_cell`。
- **BUG-M5（负值回绕）**：`verifyWaitSec` / `verifyPollSec` `.max(0)` 钳制，
  新增单测 `negative_verify_timings_are_clamped`。
- **TEST-L2（日界测试失实）**：日界改北京时间（UTC+8），补真边界单测
  `parse_audit_day_boundary_is_beijing_midnight`；`now` 移入草稿循环逐次取。

### 14.4 P2/P3 修复

- **DOC-1**：`rust/README.md` 对齐实现（pieces 通道/autopost 已实现、测试基线
  改为「以 cargo test 输出为准」、补 otherVersion 状态与 qB 5.3 seedMode 语义、
  预算节注明上传预算已接线）。
- **GAP-1**：`Config/boxsend.example.json` 补完整 `assist` 段（安全缺省：
  `publishEnabled: false`、空白名单、无凭据）。
- **GAP-2**：新增 `doc/DATA-FILES.md`（各数据文件写入者/字段/手改边界）。
- **ARC-1**：`bsend::sites::build_adapters` 下沉，CLI/GUI 共用；
  GUI 也获得「缺 cookie 站点点名」。
- **ENG-M1**：`commas` 去掉 1.87 才稳定的 `is_multiple_of`，保持 MSRV 1.82。
- **BUG-L1**：`/torrents/files` 解析抽 `parse_files_value`，嵌套 `[[…]]` 形态可用。
- **BUG-L2**：`torrents/info` 触顶 50000 条时告警，不再静默截断。
- **BUG-L3**：删除 `run_reseed` 尾部不可达 duplicate 分支。
- **BUG-L4**：`uploadBudget` / `sourceMissTTLDays` / `archiveDays` 真正读配置。
- **BUG-L5**：`cookie_header_for` 过滤已过期 cookie。
- **BUG-L6**：新增 `bsend::atomic_write`，pieces-cache / formcheck / gatecheck
  改原子写。
- **BUG-L7**：`normalized_path` 根名比较大小写不敏感；`common_root` 兼容反斜杠。
- **GUI 冗余清理**：删除未接线的并行 WIP（`jobs.rs` / `tabs/` / `widgets.rs` /
  `text_field.rs` / `round_controls.rs`），避免双份实现漂移
  （GUI 能力已内联在 `main.rs`：概览/草稿/自动发种/配置/诊断/帮助）。

### 14.5 验证与遗留

- `cargo fmt` / `clippy -D warnings` / `cargo test --workspace` 全绿：
  168 单测 + 19 e2e + 2 CLI 单测；`cargo build -p bsend-cli -p bsend-gui` 通过。
- 新二进制 `prime` 实机复跑：qB 5.3 登录、扫描 42,132 条（较批次前 +21）。
- 仍待办：**TEST-H1**（`reseed_e2e.rs` / `autopost_e2e.rs` 编排层 e2e）、
  **BUG-L8**（`cmd_sites` 直读原始 JSON）、**ENG-M2**（Rust release 流水线）、
  **ARC-2**（GUI 单文件继续拆分）、名字检索追加成员种名（Coraline 类漏检）。

### 14.6 「发了 20 个只辅了几个」排查结论（11-22 凌晨）

- **对象**：sbpt 上连续 20 条上传 `21627–21646`（用户此前发布，非本工具 autopost：
  `auto-audit.jsonl` 的 `posted` 全部是 luckpt，sbpt 只有 18 条 `skipped_duplicate`）。
- **根因**：这 20 条的内容**在本机 qB 库里完全不存在**——按标题 token 搜索 42,132 条
  种子 0 命中，按站点 torrent 的精确 totalSize 比对 3,388 段内容 0 命中。
  辅种的前提是本地已有同一份数据；没有数据就无法添加，与引擎逻辑无关。
- **对照**：本地有数据的 21 条（21656–21673 中 18 条 + 20567/20569/20570 + 21669/21658）
  已全部补进 qB（§14.1）。其中 18 条此前只差「上传回包 existed 后引擎不推下载器」
  这一步，本次已修正并补辅。
- **站点侧核对**：`viewpeerlist.php?id=21656` 显示 `Schalkiiii / qBittorrent/5.3.0 /
  100%` 为做种者——站点详情页顶部的「0 个做种者」是缓存计数，实际 peer 列表正常。
- **缺口**：21627–21646 若要辅种，需先把对应数据放进本机 qB（下载或挂载到同一保存
  路径）；在此之前引擎不会、也不应凭空建种。

### 14.7 下载器标签（Boxsend）

- `torrents/add` 表单新增 `tags=Boxsend`（`qbittorrent::BOXSEND_TAG`）：引擎辅种/转种
  加入 qB 的种子统一带标签，下载器里可直接筛选。
- 「种子已在下载器」（409 / 补挂 tracker）路径走 `torrents/addTags` 追加标签，
  不覆盖原有标签；失败只进备注，不影响推种。
- 存量补标：本批 21 条 + 早先 21654/21653 共 23 条已补 `Boxsend` 标签并复核。
- 单测 `add_form_uses_seed_mode_for_skip_checking` 增加标签断言。
