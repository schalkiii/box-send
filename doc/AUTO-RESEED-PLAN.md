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
