# 转种/辅种语义与排障口径

本文回答三类常见误解：新站种子为什么不在 qB、`skipped_duplicate` 是什么、
`NoSubfolder` 在做什么。实现见 `rust/crates/bsend/src/reseed.rs`、
`rust/crates/bsend/src/qbittorrent.rs`、`rust/crates/bsend/src/layout.rs`。

## 1. 正确流程

1. 源站种子在 qB 中已有完整数据（这是「能转种」的前提）。
2. 发布到目标站（上传目标站 `.torrent`）。
3. **从目标站下载该种 `.torrent`**（不是从 qB 里找目标种，qB 里本来就没有）。
4. 用本地源站同分片数据把它挂上 qB。
5. 校验到 100%，目标站 tracker 开始做种。

所以「qB 里没有目标种」不能解释漏辅；唯一判据是**本地有没有同分片数据可挂**。

## 2. 三种情况（按 info hash / 分片判定）

| 情况 | 判定 | 引擎行为 |
| --- | --- | --- |
| 目标种 hash 与 qB 已有种不同，pieces 一致 | 可跨站辅种 | **新加一个种子**（savepath 复用本地数据；必要时 NoSubfolder），校验 100% |
| 目标种 hash 与 qB 已有种完全相同 | qB 不允许同 hash 两条 | 按策略**不改动原种子**（不补挂 tracker、不打标签），格子记 `same_hash_skip`（同 hash 跳过） |
| pieces 不一致 | 不同版本（重压/不同分片） | 记 `other_version`，不推种 |

说明：qB 以 info hash 为唯一键，无法为同 hash 建第二个条目。要「新加一个种子」，前提是
目标站种子与本地种子的 info hash 不同（站点重打包/改名常见）；同 hash 时不存在既
「不改原种子」又「让该站做种」的方案，因此选择跳过并留明确记录。

## 3. `NoSubfolder` 是什么

qB 的多文件种子默认落到 `<savepath>/<种子根名>/`。目标站与源站的根名经常不同：

```text
本地已有：J:\Download\Show.S01.1080p-GRP\a.mkv
目标站根名：Show S01 1080p GRP
```

直接沿用 `J:\Download` 会新建 `J:\Download\Show S01 1080p GRP\`，文件不存在 → 校验 0%。
引擎核对两边「去掉根名后的文件清单」：

- 清单完全一致 → savepath 指到既有内容目录 `J:\Download\Show.S01.1080p-GRP`，
  并带 `contentLayout=NoSubfolder`（告诉 qB 不再建根子目录，直接把目标种文件映射进
  该目录），从而复用同一份数据校验到 100%；
- 清单不一致 / 单文件种子文件名不同 → 拒绝推种（`AddLayout::Mismatch`），不猜测。

## 4. 审计词义（避免误读）

- `skipped_duplicate`（auto-audit）／`assisted`（格子）：**目标站已有同分片条目**，
  本次不重复上传。它**不是**站点拒绝上传；包含「本工具此前已发布」的种子。
- `same_hash_skipped`／`same_hash_skip`：目标种与 qB 已有种子同 hash，按策略不改原种子。
- `other_version`：目标站同片但 pieces 不一致，无法跨种。
- `existed_pending`：上传回包标记已存在，但当时无法定位/复核，待人工确认。

## 6. 日志

- `reseed-log.jsonl`（dataDir）：每次 `run_reseed` 一行，含 `at / started_at / key /
  name / source_site / source_url / target_site / result / url / note`；
  CLI、GUI「确认转种」、autopost 共用。写失败只打 stderr（fail-open）。
- `auto-audit.jsonl`：autopost 的门禁/审计流水（日限额与退避依据），不是唯一动作日志。

## 7. 审阅产物直发（自动发种路径）

`review_reseed` 返回的 `ReviewReport` 除了给人看的字段，还带一份发布侧载荷：
审阅时已经取到的源/目标适配器与源页（`carry`）。自动发种的门禁通过后，
`run_reseed_reviewed` 直接拿这份报告发布：

- **不再二次抓源页 / 上传页**，也不再重复调官方预审；
- **提交字段就是门禁看到的那一份**（`prepared.fields` 原样提交），
  避免「审阅时缺字段被补上、发布时又换一份」的漂移；
- 审阅时的官方预审结论一并复用：没通过直接拦，接口异常则沿用「按本地预检继续」；
- 审阅站点与请求站点不一致、或报告缺载荷，一律拒绝提交。

CLI 单发 / GUI「确认转种」没有审阅产物，仍走 `run_reseed` 现场构造（预检 → 上传页 →
官方预审）；两条路径都过同一个 `publishEnabled` 总闸。自动发种这条路径由
`autopost.rs` 的 `ReseedRunner` 第六个参数传入，测试里注入假 runner 即可验证
「门禁通过的那份 == 真正提交的那份」。

## 8. 历史案例（2026-11-21/22）

- sbpt `21656–21673`（18 条）：qB 有 100% 源站成员（例：`Black.2005…` 32 个成员，
  progress=1.0）。`skipped_duplicate` 来自工具在后续一轮发现这些条目已在 sbpt 上
  （用户此前发布），当时只写格子、没下载目标种推 qB → 站点有、qB 没有。已修：
  判重命中/existed 现在都会下载目标种并推 qB（hash 不同→新加；同 hash→按策略跳过）。
- sbpt `21627–21646`（20 条）：用户确认由工具发布、非手工；但当时直接转种路径没有
  持久日志，无法还原是「发布后推种失败」还是「中断在推种前」。已新增
  `reseed-log.jsonl`，此后每次转种可追溯；这 20 条待拿到有效 sbpt cookie 后逐条核对
  本地同分片数据并补辅。
