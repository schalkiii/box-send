# 自动转种规划（B + A）· 2026-11-18

> 目标：把「辅种 + 转种」从半自动推进到**全自动转种**，并用**站点规范门禁**保证每次发种
> 都符合目标站规则；辅种通道升级为 pieces-hash 批量查询（B），项目定位收缩为
> 「覆盖运营 + 发种引擎」（A）。

## 0. 定位（A）

| 能力 | 定位 | 理由 |
| --- | --- | --- |
| 辅种 | **基础能力**，走 pieces-hash 主通道 + 名字检索兜底 | SeedHound / reseedhound-rs / IYUU 已把纯辅种做到极致，不与它们争「谁辅得好」 |
| 覆盖运营 | **核心价值 1**：本地库 × 站点矩阵，算「哪段内容在哪个站还缺」 | 另三家都是「给定种子找可辅目标」的正向视角，没有库级缺口运营 |
| 自动发种 | **核心价值 2**：表单适配 + 选档 + 官方预审 + 发后自检 + 落盘复用 | 三家都不做发种；这是脏活、也是不可替代的部分 |
| 门禁 | **核心价值 3**：把各站发种规范变成可执行、可审计的规则 | 全自动的前提是「不该发的一律发不出去」 |

## 1. 现状与差距

- 当前辅种：站内**按名字检索**（严格→宽松），每内容每站 3–7 请求；预算 200/轮 → 约 30–60 内容/轮。
- 当前转种：草稿 + 人工 `--draft N --yes`；已有本地预检 + 官方预审（LuckAudit）+ 发后自检。
- 差距：① 辅种通道吞吐低、依赖名字可搜索性；② 转种没有站点级规范门禁，全自动不可行。

### pieces-hash 支持率（2026-11-18 实测，92 个活跃站）

探测方法：`POST {base}/api/pieces-hash`，body `{"passkey": "...", "pieces_hash": ["…"]}`，
回包 `{"ret":0,"msg":"torrent.querybypieceshash","data":{<pieces_hash>: <torrent_id>}}`；
用 qB 里同站种子的真实 passkey + 真实 `sha1(info.pieces)` 端到端验证（discfan/hdfans/
crabpt/kufei/dubhe/njtupt 均返回正确 torrent id）。

| 结果 | 站点数 | 说明 |
| --- | --- | --- |
| 支持（200 + `ret=0`） | **52 / 92（57%）** | 见下清单 |
| 未通 | 40 | 其中 8 站只是缺 passkey（拿到即可再测）：agsvpt、ptlgs、dstudio、kamept、kelu、tey、musopia、tokyo；其余为 404/405/JSON 非预期（hdsky、chdbits、hdhome、cmct、pter 等大站无此接口） |

支持站点：baozi、cangbao、carpt、city13、crabpt、cspt、cyanbug、daxiangjiao、discfan、
dubhe、fenghuang、ggpt、hdbao、hdfans、hdtime、hdvideo、hitpt、kufei、lajidui、lemonhd、
longpt、luckpt、march、muxuege、nicept、njtupt、novahd、okpt、oshen、playlet、ptba、
ptcafe、ptfans、ptsbao、ptskit、ptzone、qilin、qingwa、railgun、sbpt、shuioudao、siqi、
soulvoice、tangpt、ubits、ultrahd、vclib、wtsakura、xdy、xingwan、xingyunge、zmpt。

**收益估算**：单请求可带 ≤100 个 pieces_hash；每内容查询成本从 3–7 请求降到 ≈0.01–0.02，
同等预算下每轮可覆盖的内容数提升 2 个数量级；且能顺带给转种臂一个廉价的「站内有没有」探针。

## 2. P1：pieces-hash 通道（B，先做）

### 数据链

```
qB /torrents/export?hash=…（或 BT_backup 目录）
  → bencode::pieces_hash(data) = sha1(info["pieces"])
  → 索引缓存 TorrentMeta.pieces_hash（二次扫描零请求）
  → 站点 passkey（从 qB tracker URL 抓，按根域缓存）
  → POST /api/pieces-hash（≤100/批，并发 2/站）
  → {pieces_hash: torrent_id} → 下载 .torrent → 现有 judge/layout → 推种
```

### 改动点

| 模块 | 改动 | 难度 |
| --- | --- | --- |
| `bencode` | 新增 `pieces()` / `pieces_hash()`（含 v2 兜底：无 pieces 时用 file tree 摘要） | 低 |
| `qbittorrent` | 新增 `export_torrent(hash)`；`torrent_meta` 已可拿 tracker | 低 |
| `index` | `TorrentMeta.pieces_hash`（`#[serde(default)]`，老索引兼容）；扫描时补算 | 低 |
| 新 `piecesapi` 模块 | passkey 采集/缓存、批量查询、站点能力表（支持/不支持/未知） | 中 |
| `coverage::process_item` | 先走 pieces-hash 探针（命中即下载 + 现有判定）；不支持/失败 → 名字检索兜底 | 中 |
| 配置 | `assist.piecesHash: auto|off`；per-site 能力缓存（自动学习：401→补 passkey，404→标记不支持） | 低 |

### 验收

- 同一批内容：名字检索 vs pieces-hash 的**命中数**与**请求数**对比（目标：请求降 ≥95%，命中不降）。
- 误报率 = 0（pieces_hash 相等 ⇒ 内容一致，判定仍是 A/B 级；layout 闸门不变）。
- 不支持站自动回退，行为与今天一致。

## 3. P2：站点规范门禁（G0–G5）

每条门禁三态：**pass / auto-fix（自动修复后可 pass）/ block（拦下，转人工）**，全部留证据。

| 门禁 | 检查项 | 依据 | 默认动作 |
| --- | --- | --- | --- |
| G0 幂等 | 目标站已有同内容（pieces-hash / 名称精确）；同站 7 天内失败冷却；本地种子 progress=1 | 索引 + 站点查询 | block（跳过，不算失败） |
| G1 准入 | 发布权限（formcheck 预检）；站点未暂停；站内每日/每轮发种限额；cookie 有效 | `reseedPausedSites`、`uploadBudget`、denied 关键词 | block / 熔断 |
| G2 规范 | 分类唯一命中；质量档（媒介/编码/分辨率/音频）齐全且站点有对应项；标题长度与模板；副标题/简介/MediaInfo 必填；链接（IMDb/豆瓣/TMDB）；标签（中字等）；截图数量 | 站点规则表 + 表单选项 | auto-fix（能修的）/ block |
| G3 官方预审 | LuckAudit `passed=true`；❌ 项映射修复或阻断 | 站点预审接口 | block |
| G4 发后自检 | 回读标题/MediaInfo/链接/选档是否落上 | 新种详情页 | 记录 + 告警（不推翻发布） |
| G5 审计 | 每次发布留字段快照、预审分、结果、耗时；连续失败熔断 | 本地审计库 | 站点降级/暂停 |

### 站点规则表（`assist.reseedRules.<siteId>`）

```json
{
  "title": {"maxLen": 200, "requireChinese": false},
  "required": ["type", "mediumSel", "codecSel", "standardSel", "descr"],
  "links": ["imdb"], "screenshots": {"min": 3},
  "allowedTypes": ["电视剧", "电影"], "bannedKeywords": ["枪版"],
  "dailyLimit": 5, "note": "来源：站点规则页 2026-11-18"
}
```

未配置的站点走**保守默认**：只允许「官方预审通过 + 全字段命中 + 无歧义」的内容自动发，
其余一律 block（宁慢勿错）。规则来源：站点规则页 + 实测驳回原因 + 官方预审口径。

## 4. P3：受控自动发种

1. **门禁试运行**（只读）：`bsend gatecheck --draft N` 输出门禁矩阵与证据，对 54 个可发站全跑一遍，
   人工复核误拦/漏拦，校准规则表。
2. **Canary**：3 个站（luckpt + 两个规则清晰的站）开启 `autoReseed`，连续 20 次发布 0 违规、
   官方预审全过 → 扩大。
3. **扩面**：按站点分批放开（先 20 站 → 54 站），每站 `dailyLimit`、失败熔断（连续 3 次失败自动暂停）。
4. **全自动**：`bsend run --auto` 一条命令完成「辅种 + 转种 + 门禁 + 发布 + 自检」；必留
   `--dry-run` 与 `--only` 供回放。

## 5. P4：运营与观测

- 指标：pieces-hash 命中率、请求/内容、门禁 pass/fix/block 分布、预审通过率、发布成功率、误拦率。
- 报表：`bsend report` 增「转种质量」段；飞书周报（复用现有推送）。
- 审计查询：`bsend audit --site X --since …`（发布快照与结果）。

## 6. 里程碑与验收

| 阶段 | 交付物 | 验收 |
| --- | --- | --- |
| P1 | pieces-hash 通道 + 支持表 + 回退 | 请求降 ≥95%、命中不降、e2e 覆盖命中/未命中/不支持三条路径 |
| P2 | 门禁引擎 + 规则表 + `gatecheck` | 54 站演练输出可解释矩阵；误拦率 ≤5%（人工抽样） |
| P3 | `run --auto` + canary 数据 | canary 20 次 0 违规；全量后发布成功率 ≥85%、预审通过率 ≥90% |
| P4 | 报表/周报/审计 | 指标可查、告警可达 |

## 7. 风险与回退

- **站点 API 无正式文档**：pieces-hash 端点靠实测；失败/改版时自动回退名字检索，能力表记未知。
- **发种违规成本高**：门禁默认保守、canary 分批、每站限额、失败熔断；任何门禁异常 → 停自动、回人工。
- **passkey 泄露面**：passkey 只在本地（从 qB tracker 读、缓存于 dataDir，不进日志）。
- **账号风险**：单站频率与日限额硬约束；cookie 失效立即停该站。

## 8. 进展记录

- **2026-11-18 · P1 第 1 步**：`bencode::pieces()/pieces_hash()` 落地（sha1(info.pieces)，
  golden 单测对齐 hashlib），v2-only 种子返回 None（回退名字检索）。
- **2026-11-18 · luckpt 门禁实证 100/100**：源站（hdsky）详情页无 MediaInfo → 新增
  `mediainfo` 模块走 MediaInfo.dll C 接口（GUI exe 无法无头运行；`Language=en` 标准文本），
  从本地内容目录取最大视频生成 4198 字节报告；完结剧集自动勾「完结」标签（完结/全集/
  全N集/Complete）。官方预审从 86/100（❌2）→ 95（❌1）→ **100/100 通过**。
  配置项：`assist.mediaInfoDll`（缺省自动发现 `C:\Program Files\MediaInfo\MediaInfo.dll`）。
- **2026-11-18 · P1 第 2–3 步**：`qbittorrent::export_torrent`（qB 导出 .torrent）+
  `all_tracker_urls`（passkey 采集）；新增 `piecesapi` 模块：passkey/能力表/pieces_hash
  三份缓存（`dataDir/pieces-cache.json`）、批量查询（≤100/批）、回包解析（Hit/Miss/
  Unsupported/Auth/Error）、下载直链（`download.php?id=…&passkey=…`）。单测 5 项。
  待接入：`coverage::process_item` 走「先 pieces-hash 探针、未命中再名字检索」+ e2e 三路径。
- **2026-11-18 · 方法学归档**：`doc/SITE-DEBUG-GUIDE.md` 统一收纳七步调试流程、
  16 类根因、可复用经验、54 站问题档案与复现模板。
- **2026-11-19 · 选档补齐（由真实门禁驳回驱动）**：① 无集数标记但有 19xx/20xx 年份 →
  按电影推断分类（luckpt 预审 79→89）；② 光盘源 + x264/x265 重编码 → 媒介选 `Encode`
  而非 Blu-ray 原盘（89→96）；③ MediaInfo 里 Audio 段有中文轨 → 勾「国语」标签
  （96→100，零 ❌ 零 ⚠️）。三条均有单测。
- **2026-11-19 · 真实新种 canary 成功**：Dark.City.1998（源 carpt → 目标 luckpt）
  门禁全绿、官方预审 100/100，上传回「发布成功」，详情页 `id=10230`。
  复盘更正：回包给的 `id=10230` 是**中转/错误 id**，不是种子被删。人工核实真实种子
  `id=61140` 存在且可下载（站内搜索「Dark.City.1998」可查到）。已新增
  `relocate_posted_detail`：回包详情页打不开时按发布标题站内检索、逐条核对详情页标题，
  自动校正为真实链接；校正仍失败才记 Reseeded + 人工确认，绝不重发。
- **交付状态**：P1/P2/P3/P4 代码与测试闭环；137→139 单测 + 17 e2e 全过，
  CI 彩排通过。白名单仍为 luckpt/hdfans/ptba，日限额 2，自动发种入口
  `bsend autopost`（`--dry-run` 只跑门禁）；若要扩大到 54 站，改 `autoReseedSites`
  并先逐站 `gatecheck`/`autopost --dry-run` 校准规则。
- **2026-11-19 · 演练开发（禁止真发）**：新增只读 `bsend gatecheck` 与
  `assist.publishEnabled` 总闸（缺省 false；CLI 与库三处入口在写请求前拦截）。
  演练样本补齐 MediaInfo 标准分段/截图后 luckpt 官方预审 100/100。
  ptfans 的媒介有两个同名 Encode（value 7/8）通过逐站
  `reseedRules.ptfans.selectPicks.medium_sel=8` 显式消歧；通用规则仍保留
  「同名不同标签 → 不猜」（muxuege 两个 AAC 留空）。
- **真发解冻步骤**：编辑 `boxsend.json` 的 `assist.publishEnabled=true`，
  同时确认 `autoReseedSites` 白名单；未显式改前任何 `--yes`/autopost 都会在请求前退出。
- **2026-11-21 · 真实发种测试（仅 luckpt）**：真实草稿迭代修复 HDTV 条件回退、
  DDP/声道词边界、NBSP MediaInfo、本地 MediaInfo 回填、死图剪除、IMDb 区上传前判重。
  经查 existed 是站点按条目判重（同片 2160p 已存在），qB 未辅种该版本是文件清单不同
  （1080p vs 2160p），非辅种故障；命中判重会在 POST 前停住。
