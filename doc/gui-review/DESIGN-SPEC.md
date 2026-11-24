# BoxSend GUI 视觉与易用性规范（改造基线）

> 这份文档是**改造的唯一口径**。所有 `rust/crates/bsend-gui/src/views/*.rs` 都必须按这里来写。
> 改造前证据截图：`doc/gui-review/before/*.png`；改造前源码快照：`doc/gui-review/before-src/main.rs.txt`。
>
> 这是写给改界面代码的人看的施工记录，里面有布局尺寸和问题编号，不是给使用者看的说明。使用说明看 `../ARCHITECTURE.md` 和 `../OPERATIONS.md`。

## 0. 改造前实测问题（必须全部消除）

| # | 现象（截图可复现） | 根因 | 修法 |
|---|---|---|---|
| P1 | 覆盖率 `87%` 与「已在做种 530」文字**叠在一起** | 覆盖率列宽 90px 里塞了 80px 进度条 + 8px gap + 百分比文字 ≈ 114px，溢出部分直接画到下一列上 | 列宽按内容实测给足（进度条 72 + 间距 + 百分比定宽），数值列与覆盖率列各自独立 |
| P2 | 147 个站点的表格**被窗口底边裁掉**，图例永远看不到 | 页面根容器 `flex_col` 无滚动，行无限增高把父级撑破 | 页面 = 固定头部 + `overflow_y_scroll` 列表；行高固定 `ROW_H` |
| P3 | 窗口 900×620 时表格**整块消失**，只剩站点 chip 墙 | 「跑一轮」147 个站点 chip 换行 10 行，占满全部高度 | chip 区固定高度 `CHIP_ZONE_MAX_H` + 内部滚动；页面另有主滚动区 |
| P4 | 37 条草稿卡片**溢出底部**，标题断行断在 `.10bit-BeiTai` 中间 | 卡片无固定高度、标题无限换行、列表无滚动 | 卡片固定 `CARD_H`，标题 `line_clamp(1)`/`truncate`，URL 单行 `truncate` |
| P5 | 配置页 147 个站点行溢出 | 同 P2 | 同 P2 + 筛选 |
| P6 | 头部配置路径过长时挤压标题 | 长文本无 `min_w(0)+truncate` | 路径进 `flex_1 min_w(0) truncate`，完整路径下放状态栏 |
| P7 | 日志区 420px 无滚动，历史日志看不到 | 无滚动容器 | 最新在上 + 固定行高 + 滚动 + 可收起 |

## 1. 令牌（`src/theme.rs`，只增不改语义）

颜色：`BG / SURFACE / SURFACE_ALT / SURFACE_HOVER / BORDER / BORDER_SUBTLE / TEXT / TEXT_DIM / TEXT_FAINT / ACCENT / ACCENT_BG / SUCCESS / SUCCESS_BG / SUCCESS_HOVER / WARN / WARN_BG / DANGER / DANGER_BG / DANGER_HOVER`

尺寸：`S1..S5`(4/8/12/16/24)、`HEADER_H`=54、`TOOLBAR_H`=46、`STATUS_H`=28、`ROW_H`=30、`HEAD_H`=28、`CARD_H`=78、`BTN_H`=30、`CHIP_H`=24、`LOG_W`=340、`LOG_ROW_H`=19、`MIN_WIN_W`=1060、`MIN_WIN_H`=720、`COL_SITE`=168、`COL_NUM`=72、`COL_RATIO`=130、`CHIP_ZONE_MAX_H`=108

要求：视图里**不允许**出现字面量 `rgb(0x...)` 或裸 `px(13.)` 这类数字（例外：进度条内部高度 6px、圆点 7px 这种控件内部工艺值可以留在 widgets 里；视图文件里出现的几何值必须来自 theme 常量或本文件顶部的局部常量）。

## 2. 共享控件（`src/widgets.rs`）——一律直接用，不要自己再写一份按钮

| 控件 | 签名 | 用途 |
|---|---|---|
| `button(id,label,on_click)` | 次级按钮，30px 高 | 刷新/复制/打开/次要动作 |
| `button_accent(id,label,on_click)` | 绿底主按钮 | 跑一轮/确认转种/演练 |
| `button_danger(id,label,on_click)` | 红底危险按钮 | 真发布/删除 |
| `tag(id,label,bg)` | 只读标记（无 hover，不可点） | 「开/关」「试用中」这类纯状态 |
| `tab(id,label,active,on_click)` | 顶部 tab | 只有 main.rs 用 |
| `chip(id,label,active,on_click)` / `chip_warn(...)` | 24px 小筛选块 | 站点多选/上限预设/试运行 |
| `badge(label,color)` | 圆点+文字 | 状态图例、运行中 |
| `title(text)` / `hint(text)` | 面板标题 / 次级说明 | — |
| `kpi(label,value,sub)` | 三等分小卡片 | 概览 KPI 行 |
| `row(index)` | 固定行高 + 斑马纹数据行外壳 | 表格/配置/诊断列表 |
| `head_cell(w,text,right)` / `cell(w,text,right)` / `cell_flex(text)` | 表格三件套，自带 `truncate` | 表头与数据行共用同一组宽度常量 |
| `progress(ratio,width_px)` | 覆盖率进度条 | 概览 |
| `empty_state(title,desc)` | 空状态 | 空列表 |
| `scroll_list(id)` | `Stateful<Div>`，`flex_1 + min_h(0) + overflow_y_scroll` | 所有长列表 |
| `panel_card()` | 页面根壳（SURFACE + 圆角 + `overflow_hidden`） | 每页最外层 |
| `control_bar()` | 控制条（内含 chips） | 页顶旋钮区 |

## 3. 布局硬规则（违背任何一条都会复现 P1–P7）

1. **页面骨架**：`widgets::panel_card()`（自带 `flex_1 min_w(0) min_h(0) flex_col gap_2 p_3 overflow_hidden`）→ `.child(页头固定区)` → `.child(widgets::scroll_list("xxx-list") 包住全部可增长内容)`。绝不把长列表直接挂在页根上。
2. **文本必截断**：任何可变长文本（站点名、种子名、URL、路径、日志行）必须满足其一：① 在 `cell(w, ...)` / `head_cell(w, ...)` 里（已含 truncate）；② `.flex_1().min_w(px(0.)).truncate()`；③ `.truncate()` + 定宽。**禁止**把长文本直接放进 flex row 而不给宽度约束。
3. **一行总宽 ≤ 容器宽**：定宽列宽之和 + gap 之和必须 ≤ 页面可用宽度（默认 1280 窗口下正文区 ≈ 1280 - 24*2 - 340(日志) - 8 ≈ 884；日志收起时 ≈ 1232；最小窗口 1060 时正文内宽 ≈ 648）。概览列宽固定为：站点 `COL_SITE`=168 + 已有 `COL_NUM`=72 + 缺口 `COL_NUM`=72 + 覆盖率 `COL_RATIO`=130 + 草稿 `COL_NUM`=72 + 已跳过 `COL_NUM`=72，列间距 `gap_2`×5=40，合计 **626px**；表头与数据行共用同一组常量，行尾补 `flex_1` 吃掉余量。若需要加列，先减 `COL_SITE`。
4. **行高固定**：表格行 `h(theme::ROW_H)`、卡片 `h(theme::CARD_H)`、日志行 `h(theme::LOG_ROW_H)`。不定高的行会因为内部文本换行把整表节奏打乱。
5. **可点区域 ≥ 28px**：统一用 widgets 的按钮/chip（30/24 高 + `px_3`，命中区已够）。
6. **危险动作隔离**：真发布 / 删除用 `button_danger`，并与主按钮之间留一个 `div().w(theme::S4)` 或分隔线；文字里带对象名（`确认转种：《xxx》→ luckpt`）。
7. **空/忙/错三态**：空 → `empty_state`；运行中 → 页头右侧 `badge("运行中", WARN)` + 按钮禁用视觉（`tag` 替代按钮）；错误 → 红色 `hint` 行，不要静默。

## 4. 每页信息架构

### 概览（`views/overview.rs`，方法 `render_overview`）
1. 页头行：`title("索引概览")` + 右侧 4 个 `kpi`（段内容 / 成员种子 / 草稿 / 参与判定站点，数值从 `self.stats` 里解析或数 `self.sites.len()`；解析失败就显示 `stats` 原文）。
2. 控制条（`control_bar`）：站点多选 chips（**固定高度 + 内部滚动**，`CHIP_ZONE_MAX_H`）、试运行开关（`chip_warn`）、上限预设 chips、「全选/清空」小按钮。
3. 表格：表头（`head_cell` 固定，不随滚动走）+ `scroll_list("overview-list")` 里逐行 `row(i)`。
   - **列口径（6 列，顺序固定）**：`站点 | 已有 | 缺口 | 覆盖率 | 草稿 | 已跳过`
     - **已有** = qB 里带该站 tracker 的内容数（`SiteRow::have` ← `SiteStats::qb_covered`）
     - **缺口** = 全库内容数 − 已有（`SiteRow::gap` ← `qb_missing`）
     - **覆盖率** = 已有 / 全库内容数（`SiteRow::ratio` ← `qb_ratio`，进度条 + 百分比，整组右对齐）
     - **草稿** = 该站待发草稿数（`SiteRow::drafts`，按 `DraftRow::target_site` 计数）
     - **已跳过** = 该站终态跳过（内容黑名单等）的记录条数（`SiteRow::skipped` ← `CoverageStatus::Skipped`）
   - 表头与数据行共用 `COL_SITE / COL_NUM / COL_RATIO` 三个常量，数值列右对齐，行尾 `flex_1` 收尾；不再有「格子 / 已覆盖 / 未判定 / 状态分布」列。
4. 图例：折叠到表格下方的 `hint` 一行（`当色点=状态含义`），或做成右下角「图例」开关；默认显示 1 行压缩版，不占 5 行。
5. 排序：`self.overview_sort`（0 覆盖率降序 = 默认 / 1 缺口降序 / 2 站点名），用 chip 组暴露，不做表头点击态（GPUI 下命中区与滚动冲突风险高）。默认按覆盖率降序，是因为 147 站里约 88 站 `已有` 为 0，按缺口降序会让首屏全是 `0 / 3388 / 0%`，看着像数据坏了。
6. 筛选：`self.overview_filter`（0 全部 / 1 只看有缺口 `gap > 0` / 2 只看已满 `gap == 0`），三个 chip；不做自由文本检索框。

### 转种草稿（`views/drafts.rs`）
- 页头：`title("转种草稿（37 条）")` + 右侧「全部复制链接」不做（危险）；给「只看待处理」chip。
- 列表：`scroll_list` + 每张卡 `h(theme::CARD_H)`：左侧三行（标题 `truncate`、`源站 → 目标站`、`源链接 truncate`），右侧按钮组 `复制源站链接 | 审阅 | 确认转种(accent)`。
- 卡片之间的 gap 用 `S2`；卡片底色 `SURFACE_ALT`，圆角 `rounded_md`。

### 自动发种（`views/autopost.rs`）
- 页头 + 一段 `hint`（发布检查/日限额/失败冷却口径，允许 2–3 行换行）。
- 控制条：目标站 chips（fixed 高 + 滚动）、试运行 `chip_warn`、上限 chips。
- 主按钮区：试运行 → `button_accent("开始演练")`；真发布 → `button_danger("真发布（不可撤销）")`，前后各留 `S4` 分隔，旁边 `hint` 说明。
- 当前草稿预览：`scroll_list` 列出有草稿的目标站 + 该站草稿数（只读）。

### 配置（`views/config.rs`）
- 页头：`title("配置")` + `hint`（文件路径 truncate、备份 .bak-gui、热重载）。
- 动作：`重载配置` / `打开配置文件`（`button`），`真实发种总开关`（`tag` 显示当前值 + chip 切换）。
- 站点表：表头 `站点 | 启用 | 白名单 | 备注` + `scroll_list`，147 行 `row(i)`，两个可点 chip 每行。
- 检索预算 chips 放控制条（固定，不滚动）。

### 诊断（`views/diag.rs`）
- 页头 + 两个动作按钮（测试下载器 / 拉 CookieCloud）。
- 三段只读清单：cookie 在位（`scroll_list`）、今日发种台账（scroll）、路径信息（truncate）。
- 每段之间 `border_t_1` 细分隔线。

### 说明（`views/help.rs`）
- 自由函数 `pub(crate) fn render_help() -> AnyElement`。
- `panel_card` + `scroll_list("help-body")`，正文用 `hint` 段落，1–5 步用编号标题 `title` 小段；不要再写 6 行超长单段。

### 日志（`views/log.rs`）
- `render_log` 自行决定宽度与折叠：
  - 展开：`w(theme::LOG_W)`、`panel_card`、页头「任务日志（N 条）」+ 「清空」+「收起」
  - 收起：`w(px(44.))` 竖向窄条，一个「日志」chip 点开（`self.log_open = true`）
- 行：**最新在上**（`for l in self.log.iter().rev().take(200)`），每行 `h(theme::LOG_ROW_H)` + `truncate`，单行不换行。
- 滚轮滚动；不要每次渲染 `scroll_to_bottom`（会跟用户滚动打架）。

## 5. 验收清单（改完逐条自查）

- [ ] 1280×820：表格完整可见、底部图例可读、无任何文字重叠
- [ ] 1060×720（最小尺寸）：表头 + 至少 4 整行数据可见（其余靠滚动），图例换行但不压数据行，主视图与日志不互相挤压
- [ ] 1600×1000：留白合理，列不被拉成一坨、进度条长度固定
- [ ] 147 行站点表滚动到底能看到最后一行；滚轮可用
- [ ] 37 条草稿滚动到底能看到最后一条；标题不一味换行
- [ ] 每个长文本都以 `…` 结尾而不是越过边界
- [ ] 每个可点元素 hover 有反馈、命中高度 ≥ 28px
- [ ] 没有 `rgb(0x` 字面量残留在 views 里（`grep -n "rgb(0x" src/views` 应为空）

## 6. 本 crate 已验证可用的 gpui 0.2.2 API（不要猜别的）

已确认存在：`div / px / rgb / size / size_full / h / w / min_h / min_w / flex / flex_col / flex_row / flex_1 / flex_none / flex_wrap / flex_shrink_0 / items_center / justify_center / gap_1..gap_4 / gap_x_4 / gap_y_1 / p_1..p_4 / px_1..px_4 / py_1 / py_2 / mt_1 / mb_1 / text_xs / text_sm / text_base / text_lg / text_xl / font_weight / text_color / text_right / truncate / line_clamp / whitespace_nowrap / text_ellipsis / overflow_hidden / overflow_y_scroll / scrollbar_width / on_scroll_wheel / rounded_sm? 用 rounded_md / rounded_lg / rounded_full / bg / border_b_1 / border_t_1 / border_color / cursor_pointer / hover / active / on_click / child / children / when / id / track_scroll / ScrollHandle::scroll_to_bottom / set_offset / offset / into_any_element / AnyElement / Stateful<Div>`

**不要用**（该版本没有或不可访问）：`border_1()`、`max_w()`、`Pixels` 的 `.0` 字段（`Pixels(pub(crate) f32)`，取不到内部值；需要像素宽度时直接传 f32）、`max_h()`（用固定 `h()` 代替）、任何 `flex_gap`/`shadow_*`。
