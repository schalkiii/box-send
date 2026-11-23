# BoxSend GUI 视觉与易用性 Review

范围：`rust/crates/bsend-gui`（GPUI 0.2.2）。评审期间只读真实配置与覆盖记录，只截图、不点发布/删除，不在评审过程里提交 git；交付后由主流程统一提交。

## 1. 结论

改造前 GUI 的三类硬故障已全部消除：

1. **字体重叠**：概览表「覆盖率」列（90px 里塞 80px 进度条 + gap + 百分比 ≈ 114px）把 `87%` 画到了「已在做种 530」上面。
2. **长列表溢出被裁**：站点 chip 墙、37 张草稿卡、147 行站点表都没有滚动容器，直接顶出窗口底边被裁。
3. **整块消失**：窗口缩到 900×620 时，147 个站点 chip 换行占满高度，表格和图例被挤出可视区，页面上只剩一片 chip。

改造方式不是逐个打补丁，而是先把单文件 1607 行的 `main.rs` 拆成 **外壳 + 令牌 + 控件 + 视图** 四层，再让每个视图只组合控件、只读令牌，从结构上让「重叠 / 溢出」不可能发生。

## 2. 架构：单文件 → 四层

```
crates/bsend-gui/src/
├── main.rs      外壳：状态字段、jobs、轮询、header / toolbar / status bar、impl Render
├── theme.rs     令牌：颜色 / 间距 / 尺寸 / 列宽（唯一来源）
├── widgets.rs   控件：button / chip / badge / cell / row / kpi / scroll_list / panel_card …
├── types.rs     视图数据模型（SiteRow / DraftRow / StatusChip）
├── jobs.rs      bsend 引擎 → 视图模型的映射
└── views/      每页一个文件，均为 impl BsendGui 的 render_xx
    ├── overview.rs  drafts.rs  autopost.rs
    └── config.rs    diag.rs    help.rs     log.rs
```

关键收益：**列宽不再是各行自己算的魔法数字**。表头与数据行取同一组 `theme::COL_*` 常量，任何一列改宽度，表头和数据行同时跟着变，错列在结构上被消掉。

## 3. 问题 → 修法对照

| # | 现象（改造前） | 根因 | 修法 |
|---|---|---|---|
| D1 | 日志抽屉展开后只有约 44px 宽，只剩半个「任」字，日志正文完全看不见 | 侧栏复用 `widgets::panel_card()`，其内部 `flex_1` 把 `flex_basis` 置为 0；事后 `.w(340)` 只改 grow/shrink，`basis:0 + grow:0` 下按内容收缩 | 侧栏不再复用卡片，自建容器并同时钉死 `flex_none + flex_basis(LOG_W) + w(LOG_W) + min_w(LOG_W)`，收起窄条补 `flex_basis(44)/w(44)/min_w(44)/truncate` |
| D2 | 「覆盖率」表头右对齐，数据却是「进度条+百分比」左对齐，进度条顶到下一列 | 列宽 90px 装不下 80px 进度条 + 8px gap + 百分比；且整组按左对齐排 | 列宽给到 `COL_RATIO=130`（进度条 72 + gap 4 + 百分比 44 = 120 ≤ 130），整组 `justify_end()` 右对齐，右边缘与表头同线 |
| D3 | 站点 chip 区出现半截字形 | `CHIP_ZONE_MAX_H=96` 装不下 4 行 24px chip（24×4 + 4×3 = 108） | 改 108，超出部分内部滚动，不再把表格顶出可视区 |
| D4 | KPI 写「站点数」，实际数的是参与判定的站点 | 口径不清 | 改「参与判定站点」，副标题说明 |
| D5 | 状态栏长路径被截成 `…\BoxSe` | 整段路径 `truncate` 从尾部切，砍掉了最有信息量的目录名 | `short_path()` 只取尾部两段（`BoxSend\coverage.json`） |
| D6 | 各滚动列表滚到底，最后一行贴面板底边被切一半 | 列表容器无底部内边距 | overview / drafts / config / diag / log 五处滚动列表统一 `pb(S1)` |
| D7 | 配置页顶部「共 N 站」与筛选后行数对不上 | 计数取 `sourceSites` 原始总数，表格取过滤结果 | 计数改取可见行数：全部模式 `共 N 站`，筛选模式 `已启用 N / 共 M 站` |
| D8 | 概览表列名与含义对不上覆盖记录（格子 / 已覆盖 / 未判定） | 沿用内部中间量命名 | 改为 qB 真值口径六列：站点 / 已有 / 缺口 / 覆盖率 / 草稿 / 已跳过（见 §4） |
| D9 | 长列表一律不能滚 | 每个页面的列表都直接 `flex_col` 堆 child | 统一 `widgets::scroll_list(id)`（`overflow_y_scroll` + `scrollbar_width` + 固定 `ROW_H` 行） |

## 4. 概览表：新口径六列

| 列 | 宽度常量 | 对齐 | 字段 | 含义 |
|---|---|---|---|---|
| 站点 | `COL_SITE` = 168 | 左 | `site` | 站点 id |
| 已有 | `COL_NUM` = 72 | 右 | `have` ← `SiteStats::qb_covered` | qB 里带该站 tracker 的内容数 |
| 缺口 | `COL_NUM` = 72 | 右 | `gap` ← `qb_missing` | 全库内容数 − 已有 |
| 覆盖率 | `COL_RATIO` = 130 | 整组右 | `ratio` ← `qb_ratio` | 已有 / 全库内容数（进度条 72 + 百分比 44） |
| 草稿 | `COL_NUM` = 72 | 右 | `drafts` | 该站待发草稿数 |
| 已跳过 | `COL_NUM` = 72 | 右 | `skipped` ← `CoverageStatus::Skipped` | 该站终态跳过（内容黑名单等）的记录条数 |

宽度核算：定宽 168 + 72×4 + 130 = **586px**，列间距 `gap_2` × 5 = **40px**，行内合计 **626px ≤ 648px**（窗口最小宽 1060 时正文内宽）。表头与数据行末尾各挂一个 `flex_1 min_w(0)` 吃掉余量，宽窗时列宽不被拉伸。

默认排序为**覆盖率降序**（`overview_sort = 0`），不是缺口降序：真实数据里 147 站有 88 站 `已有` 为 0，按缺口降序会让首屏整片 `0 / 3388 / 0%`，看着像数据坏了；按覆盖率降序首屏直接是 xingtan 1724 / 51%、pter 923 / 27%、btschool 823 / 24% 这种真实大数。另两档为「缺口降序」（找最欠铺的站）与「站点名」。

原「状态分布」列下线，其信息由表格下方图例承担（按 `StatusChip.label` 去重、`count` 求和显示为 `标签 数量`）。

筛选/排序随之改口径：`overview_filter` 0 全部 / 1 只看有缺口 `gap > 0` / 2 只看已满 `gap == 0`；`overview_sort` 0 缺口降序 / 1 站点名 / 2 覆盖率升序。

## 5. 视觉规范落地要点

- 三层底色：`BG #0b0d11` → `SURFACE #14171d` → `SURFACE_ALT #191d25`；两档字色 `TEXT #e6e9f0` / `TEXT_DIM #8b939f`；主色 `ACCENT #4c8dff`；语义色 success / warn / danger 各配 10% 底。
- 间距刻度 `S1–S5 = 4 / 8 / 12 / 16 / 24`，尺寸 `HEADER_H 54 / TOOLBAR_H 46 / STATUS_H 28 / ROW_H 30 / HEAD_H 28 / CARD_H 78 / BTN_H 30 / CHIP_H 24 / LOG_W 340`；窗口最小尺寸 `1060×720`（实测该尺寸下表格仍留 4 整行 + 第 5 行半行可滚，图例换两行但不压数据行；低于 720 就只剩半行，故挡住）。
- 7 条布局硬规则（定高区 vs 滚动区分工、一行总宽上限、行高对齐、主轴不混用、抽屉收敛、`overflow_hidden` 兜底）与 gpui 0.2.2 可用/禁用 API 白名单写在 `DESIGN-SPEC.md`。

## 6. 改动文件清单

- `rust/crates/bsend-gui/src/main.rs`
- `rust/crates/bsend-gui/src/theme.rs`（新增）
- `rust/crates/bsend-gui/src/widgets.rs`（新增）
- `rust/crates/bsend-gui/src/types.rs`（新增）
- `rust/crates/bsend-gui/src/jobs.rs`（新增）
- `rust/crates/bsend-gui/src/views/{mod,overview,drafts,autopost,config,diag,help,log}.rs`（新增）
- `doc/gui-review/DESIGN-SPEC.md`、`doc/gui-review/REVIEW.md`、`doc/gui-review/tools/*.ps1`
- `doc/gui-review/before/*`（改造前证据）、`doc/gui-review/after/*`（改造后）、`doc/gui-review/compare-*.png`（对比图）

未触碰：`bsend` 引擎、`bsend-cli`、`Cargo.toml`、`boxsend.json`、`coverage.json`。

## 7. 验证

- `cargo check -p bsend-gui --profile gui-release --message-format short` → 0 error / 0 warning（仅剩三方 `proc-macro-error2` 的 future-incompat 提示）。
- `cargo build --profile gui-release -p bsend-gui` → 成功，产物 `rust/target/gui-release/bsend-gui.exe`。
- `cargo test -p bsend-gui --profile gui-release` → `running 3 tests` / `test result: ok. 3 passed; 0 failed`（覆盖 `short_path()` 路径截短、`parse_counts()` 统计串解析等纯函数）。
- 像素验收：`before/` 与 `after/` 同尺寸对照，覆盖概览 1280×820、最小窗口 1060×720、宽窗 1600×1000、草稿、配置、日志展开六种状态。

> 构建注意：标准 `cargo build --release` 会在 gpui 的 build.rs 处因缺 `fxc.exe`（Windows SDK）失败，必须用仓库自带的 `--profile gui-release`（运行时编译着色器）。

## 8. 截图索引

对比图（`doc/gui-review/` 根目录，左 BEFORE / 右 AFTER）：

| 文件 | 对照内容 |
|---|---|
| `compare-overview-1280x820.png` | 概览：覆盖率文字叠印 + 表格被裁 → 六列新口径、行高对齐、图例独立 |
| `compare-overview-narrow.png` | 窄窗：chip 墙占满、表格消失 → 1060×720 下四行表格完整可滚 |
| `compare-drafts.png` | 草稿：37 卡溢出、标题断在 `.10bit-BeiTai` → 固定卡高、单行标题、内部滚动 |
| `compare-config.png` | 配置：147 行溢出无滚动 → 四列对齐、行可滚、末行不切 |

单帧：`before/01..04`（改造前缺陷）、`after/01-overview-1280x820.png`、`after/02-overview-1060x720.png`、`after/03-overview-1600x1000.png`、`after/04-drafts-1280x820.png`、`after/05-config-1280x820.png`、`after/06-overview-1280x820-log-open.png`。

像素核验（三档窗口一致）：首行 `xingtan / 1724 / 1664 / 51%`；六列右缘对齐差值 ≤2px（字符抗锯齿）；日志展开实体宽 340px，主表六列位置不变。

## 9. 已知取舍

1. **滚动区最后一行按像素裁切**：任何高度下，列表视口边界都可能落在某行中间，露半行属于滚动裁切而非文本重叠；拉高窗口或滚动即可看到整行。
2. **最小窗口 1060×720**：再小必然出现「只剩半行表格」，所以直接设成窗口下限挡住，而不是让它缩成一团。
3. **收起态日志窄条（44px）未截到实测图**：日志头「收起/清空」按钮对脚本合成点击不响应（tab/chip 的合成点击正常），展开态 340px 已验证；窄条宽高由 `flex_basis/w/min_w` 三件套钉死，建议人工点一次「收起」复核。
4. **真实数据里 88/147 个站 `已有` 为 0**：这是 qB 里确实没有该站 tracker 的内容，不是统计错误；默认排序选覆盖率降序就是为了避免首屏整片 0%。
