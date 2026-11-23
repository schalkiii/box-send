//! 概览页：各站 已有 / 缺口 / 覆盖率 / 草稿 / 已跳过，带「跑一轮」旋钮、筛选与排序。
//!
//! 布局硬规则（见 DESIGN-SPEC §3）：
//! - 页面根 = `panel_card`，固定页头 + 固定控制条 + 固定表头 + `scroll_list` 长列表 + 固定图例；
//! - 定宽列之和 + gap 必须 ≤ 正文可用宽度；可变长文本一律进 `cell` / `truncate`；
//! - 行高固定 `ROW_H`，站点 chip 区固定高度 `CHIP_ZONE_MAX_H` 内部滚动。

use gpui::{div, prelude::*, px, AnyElement, Context, Pixels, SharedString};

use crate::theme;
use crate::types::SiteRow;
use crate::widgets;
use crate::BsendGui;

// ============ 本页列宽（表头与数据行共用；与 theme 令牌一致，末尾弹性列吃余量）============
/// 站点名列宽（取 theme::COL_SITE）
const COL_SITE: Pixels = theme::COL_SITE;
/// 数字列宽（已有 / 缺口 / 草稿 / 已跳过）
const COL_NUM: Pixels = theme::COL_NUM;
/// 覆盖率列：进度条 + 间距 + 百分比文字（整组右对齐）
const COL_RATIO_W: Pixels = theme::COL_RATIO;
/// 进度条像素宽度（控件内部工艺值）
const BAR_W: f32 = 72.;
/// 百分比文字定宽
const PCT_W: Pixels = px(44.);
/// 滚动条宽度（与 widgets::scroll_list 一致的工艺值）
const SCROLLBAR_W: Pixels = px(8.);

impl BsendGui {
    pub(crate) fn render_overview(&self, cx: &mut Context<Self>) -> AnyElement {
        // ---- KPI：stats 串形如 "3388 段内容、42153 条成员种子、37 条草稿；索引 …"，解析失败回退占位 ----
        let (contents, members, drafts) = parse_counts(&self.stats);
        let raw_stats = self.stats.clone();
        let contents = contents
            .map(|n| n.to_string())
            .unwrap_or_else(|| raw_stats.to_string());
        let members = members.map(|n| n.to_string()).unwrap_or_else(|| "—".into());
        let drafts = drafts
            .map(|n| n.to_string())
            .unwrap_or_else(|| self.drafts.len().to_string());

        // ---- 筛选 + 排序（只读本地状态，不改数据）----
        let mut rows: Vec<&SiteRow> = self
            .sites
            .iter()
            .filter(|r| match self.overview_filter {
                1 => r.gap > 0,
                2 => r.gap == 0,
                _ => true,
            })
            .collect();
        match self.overview_sort {
            // 默认：覆盖率降序（先看谁最铺得开，避免首屏一片 0%）
            1 => rows.sort_by_key(|a| std::cmp::Reverse(a.gap)),
            2 => rows.sort_by(|a, b| a.site.cmp(&b.site)),
            _ => rows.sort_by(|a, b| {
                b.ratio
                    .partial_cmp(&a.ratio)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
        }

        // ---- 图例：从各行 chips 汇总去重（颜色数据来自账本，不在视图里写死 hex）----
        let mut legend: Vec<(String, usize, u32)> = Vec::new();
        for r in &self.sites {
            for c in &r.chips {
                match legend.iter_mut().find(|(l, _, _)| l.as_str() == c.label) {
                    Some((_, n, _)) => *n += c.count,
                    None => legend.push((c.label.to_string(), c.count, c.color)),
                }
            }
        }

        // ---- 错误态：最近一条日志是失败 / 中断时给出红色提示行，不静默 ----
        let error_hint = self
            .log
            .last()
            .map(|l| l.as_str())
            .filter(|l| l.starts_with("失败") || l.contains("中断") || l.starts_with("快照失败"))
            .map(|l| l.to_string());

        // ---- 控制条 A：站点子集（固定高度 + 内部滚动）+ 试运行 + 上限 ----
        let mut site_chips = div().flex().flex_wrap().items_center().gap_1();
        for s in &self.config.sites {
            let site_id = s.id.clone();
            let selected = self.round_sites.contains(&site_id);
            site_chips = site_chips.child(widgets::chip(
                &format!("round-site-{site_id}"),
                s.name.clone(),
                selected,
                cx.listener(move |this: &mut Self, _, _, cx| {
                    if let Some(pos) = this.round_sites.iter().position(|x| x == &site_id) {
                        this.round_sites.remove(pos);
                    } else {
                        this.round_sites.push(site_id.clone());
                    }
                    cx.notify();
                }),
            ));
        }

        let limit_presets: [(&str, Option<usize>); 4] = [
            ("10", Some(10)),
            ("50", Some(50)),
            ("200", Some(200)),
            ("不限", None),
        ];
        let mut limit_chips = div().flex().flex_none().items_center().gap_1();
        for (label, value) in limit_presets {
            let selected = self.round_limit == value;
            limit_chips = limit_chips.child(widgets::chip(
                &format!("round-limit-{label}"),
                label,
                selected,
                cx.listener(move |this: &mut Self, _, _, cx| {
                    this.round_limit = value;
                    cx.notify();
                }),
            ));
        }

        let bar_a = widgets::control_bar()
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(gpui::rgb(theme::TEXT_FAINT))
                    .truncate()
                    .child("跑一轮站点（空 = 全部启用）"),
            )
            .child(
                div()
                    .id("round-site-zone")
                    .flex_1()
                    .min_w(px(0.))
                    .h(theme::CHIP_ZONE_MAX_H)
                    .pt_1()
                    .overflow_y_scroll()
                    .scrollbar_width(SCROLLBAR_W)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(site_chips),
            )
            .child(widgets::chip_warn(
                "round-dry",
                if self.round_dry {
                    "试运行：开"
                } else {
                    "试运行：关"
                },
                self.round_dry,
                cx.listener(|this: &mut Self, _, _, cx| {
                    this.round_dry = !this.round_dry;
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(gpui::rgb(theme::TEXT_FAINT))
                    .child("上限"),
            )
            .child(limit_chips)
            .child(widgets::button(
                "round-all",
                "全选",
                cx.listener(|this: &mut Self, _, _, cx| {
                    this.round_sites = this.config.sites.iter().map(|s| s.id.clone()).collect();
                    cx.notify();
                }),
            ))
            .child(widgets::button(
                "round-clear",
                "清空",
                cx.listener(|this: &mut Self, _, _, cx| {
                    this.round_sites.clear();
                    cx.notify();
                }),
            ));

        // ---- 控制条 B：筛选 + 排序 ----
        let bar_b = widgets::control_bar()
            .child(widgets::chip(
                "ov-filter-all",
                "全部",
                self.overview_filter == 0,
                cx.listener(|this: &mut Self, _, _, cx| {
                    this.overview_filter = 0;
                    cx.notify();
                }),
            ))
            .child(widgets::chip(
                "ov-filter-gap",
                "只看有缺口",
                self.overview_filter == 1,
                cx.listener(|this: &mut Self, _, _, cx| {
                    this.overview_filter = 1;
                    cx.notify();
                }),
            ))
            .child(widgets::chip(
                "ov-filter-full",
                "只看已满",
                self.overview_filter == 2,
                cx.listener(|this: &mut Self, _, _, cx| {
                    this.overview_filter = 2;
                    cx.notify();
                }),
            ))
            .child(div().flex_1().min_w(px(0.)))
            .child(widgets::chip(
                "ov-sort-ratio",
                "覆盖率降序",
                self.overview_sort == 0,
                cx.listener(|this: &mut Self, _, _, cx| {
                    this.overview_sort = 0;
                    cx.notify();
                }),
            ))
            .child(widgets::chip(
                "ov-sort-gap",
                "缺口降序",
                self.overview_sort == 1,
                cx.listener(|this: &mut Self, _, _, cx| {
                    this.overview_sort = 1;
                    cx.notify();
                }),
            ))
            .child(widgets::chip(
                "ov-sort-name",
                "站点名",
                self.overview_sort == 2,
                cx.listener(|this: &mut Self, _, _, cx| {
                    this.overview_sort = 2;
                    cx.notify();
                }),
            ));

        // ---- 表头（固定，不随滚动走）----
        let header_row = div()
            .flex()
            .flex_none()
            .items_center()
            .h(theme::HEAD_H)
            .px_2()
            .gap_2()
            .border_b_1()
            .border_color(gpui::rgb(theme::BORDER))
            .child(widgets::head_cell(COL_SITE, "站点", false))
            .child(widgets::head_cell(COL_NUM, "已有", true))
            .child(widgets::head_cell(COL_NUM, "缺口", true))
            .child(widgets::head_cell(COL_RATIO_W, "覆盖率", true))
            .child(widgets::head_cell(COL_NUM, "草稿", true))
            .child(widgets::head_cell(COL_NUM, "已跳过", true))
            .child(div().flex_1().min_w(px(0.)));

        // ---- 数据行（放进滚动列表）----
        let list = if rows.is_empty() {
            widgets::scroll_list("overview-list")
                .child(widgets::empty_state(
                    "没有符合条件的站点",
                    "调整上方筛选，或先跑一轮基线刷新覆盖记录。",
                ))
                .into_any_element()
        } else {
            let mut body = widgets::scroll_list("overview-list").pb_1();
            for (i, row) in rows.iter().enumerate() {
                let ratio_pct = format!("{:.0}%", row.ratio * 100.0);

                body = body.child(
                    widgets::row(i)
                        .child(widgets::cell(COL_SITE, row.site.clone(), false))
                        .child(widgets::cell(COL_NUM, row.have.to_string(), true))
                        .child(widgets::cell(COL_NUM, row.gap.to_string(), true))
                        .child(
                            div()
                                .flex_none()
                                .w(COL_RATIO_W)
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_1()
                                .child(widgets::progress(row.ratio.clamp(0.0, 1.0) as f32, BAR_W))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(PCT_W)
                                        .text_xs()
                                        .text_right()
                                        .text_color(gpui::rgb(theme::TEXT_DIM))
                                        .truncate()
                                        .child(ratio_pct),
                                ),
                        )
                        .child(widgets::cell(COL_NUM, row.drafts.to_string(), true))
                        .child(widgets::cell(COL_NUM, row.skipped.to_string(), true))
                        .child(div().flex_1().min_w(px(0.))),
                );
            }
            body.into_any_element()
        };

        // ---- 图例：压缩成一行可换行的 badge 墙 ----
        let mut legend_row = div().flex().flex_wrap().items_center().gap_x_3().gap_y_1();
        for (label, count, color) in legend {
            legend_row = legend_row.child(widgets::badge(format!("{label} {count}"), color));
        }

        let mut root = widgets::panel_card()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .child(widgets::title("索引概览"))
                    .child(div().flex_1().min_w(px(0.)))
                    .when(self.busy, |d| {
                        d.child(widgets::badge(
                            format!("运行中 · 已处理 {} 格", self.cells_done),
                            theme::WARN,
                        ))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .child(widgets::kpi("段内容", contents, "覆盖记录里的去重段落"))
                    .child(widgets::kpi("成员种子", members, "下载器里挂着的成员种"))
                    .child(widgets::kpi("草稿", drafts, "待转种草稿条数"))
                    .child(widgets::kpi(
                        "参与判定站点",
                        self.sites.len().to_string(),
                        "参与判定的站点",
                    )),
            )
            .child(bar_a)
            .child(bar_b);

        if let Some(err) = error_hint {
            root = root.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(gpui::rgb(theme::DANGER))
                    .truncate()
                    .child(err),
            );
        }

        root = root
            .child(
                widgets::hint(
                    "已有 = qB 里带该站 tracker 的内容数 · 缺口 = 全库 − 已有 · 覆盖率 = 已有 / 全库 · 草稿 = 该站待发 · 已跳过 = 终态跳过",
                ),
            )
            .child(header_row)
            .child(list)
            .child(
                div()
                    .flex_none()
                    .pt_1()
                    .border_t_1()
                    .border_color(gpui::rgb(theme::BORDER_SUBTLE))
                    .child(legend_row),
            );

        root.into_any_element()
    }
}

/// 从 stats 串解析前三个数字（段内容 / 成员种子 / 草稿）；解析不到返回 `None`。
fn parse_counts(stats: &SharedString) -> (Option<usize>, Option<usize>, Option<usize>) {
    let head = stats.split('；').next().unwrap_or(stats.as_str());
    let mut parts = head.split('、');
    let a = parts.next().and_then(leading_usize);
    let b = parts.next().and_then(leading_usize);
    let c = parts.next().and_then(leading_usize);
    (a, b, c)
}

/// 取字符串开头的连续数字解析为 usize。
fn leading_usize(seg: &str) -> Option<usize> {
    let digits: String = seg.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<usize>().ok()
    }
}
