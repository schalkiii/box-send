//! 发种统计页：按站点列出已发种数量、最后发种时间和最后一条种子链接。
//!
//! 数据只来自 `auto-audit.jsonl` 与 `reseed-log.jsonl`，按“站点 + 内容键”去重；
//! 只算站点收下新种的结果，不把「站上本来就有」「已跳过」「失败」算成发种。

use gpui::{div, prelude::*, px, AnyElement, Div, Pixels};

use crate::theme;
use crate::widgets;
use crate::BsendGui;

const COL_SITE: Pixels = theme::COL_SITE;
const COL_NUM: Pixels = theme::COL_NUM;
const COL_TIME: Pixels = theme::COL_RATIO;

impl BsendGui {
    pub(crate) fn render_poststats(&self) -> AnyElement {
        let total: usize = self.post_stats.iter().map(|r| r.posted).sum();
        let latest = self.post_stats.iter().max_by_key(|r| r.last_at);
        let latest_text = latest
            .map(|r| {
                if r.last_at_text().is_empty() {
                    format!("{}（时间缺失）", r.site)
                } else {
                    format!("{} · {}", r.site, r.last_at_text())
                }
            })
            .unwrap_or_else(|| "还没有成功发种记录".to_string());
        let latest_url = latest.map(|r| r.last_url.clone()).unwrap_or_default();

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
            .child(widgets::head_cell(COL_NUM, "已发种", true))
            .child(widgets::head_cell(COL_TIME, "最后发种时间", true))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_xs()
                    .text_color(gpui::rgb(theme::TEXT_FAINT))
                    .truncate()
                    .child("最后一条种"),
            );

        let list = if self.post_stats.is_empty() {
            widgets::scroll_list("poststats-list")
                .child(widgets::empty_state(
                    "还没有发种记录",
                    "跑过自动发种/转种后，这里会按站点汇总；只读本地日志，不联网。",
                ))
                .into_any_element()
        } else {
            let mut body = widgets::scroll_list("poststats-list").pb_1();
            for (i, row) in self.post_stats.iter().enumerate() {
                body = body.child(
                    widgets::row(i)
                        .child(widgets::cell(COL_SITE, row.site.clone(), false))
                        .child(widgets::cell(COL_NUM, row.posted.to_string(), true))
                        .child(widgets::cell(
                            COL_TIME,
                            if row.last_at_text().is_empty() {
                                "—".to_string()
                            } else {
                                row.last_at_text()
                            },
                            true,
                        ))
                        .child(widgets::cell_flex(if row.last_url.is_empty() {
                            "—".to_string()
                        } else {
                            row.last_url.clone()
                        })),
                );
            }
            body.into_any_element()
        };

        let mut root = widgets::panel_card()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .child(widgets::title("发种统计"))
                    .child(div().flex_1().min_w(px(0.)))
                    .child(widgets::badge(format!("共 {total} 条"), theme::ACCENT)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .child(widgets::kpi("站点", self.post_stats.len().to_string(), "有成功发种记录"))
                    .child(widgets::kpi("已发种", total.to_string(), "按站点 + 内容去重"))
                    .child(widgets::kpi("最近一次", latest_text, latest_url)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .bg(gpui::rgb(theme::SURFACE_ALT))
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(gpui::rgb(theme::TEXT_FAINT))
                            .child("四种结果怎么看（统计只算这四类）"),
                    )
                    .child(status_row(
                        "posted",
                        theme::SUCCESS,
                        "站点收下、详情页已拿到、种子已挂回 qB，页面没有未审/不通过标记。算一条新发种，但不等于现在一定有人下载。",
                    ))
                    .child(status_row(
                        "posted_verifying",
                        theme::WARN,
                        "站点收下、种子也进了 qB，但校验、tracker 或审核状态还没确认。算一条新发种，不能写成已做种。",
                    ))
                    .child(status_row(
                        "pending_review",
                        theme::ACCENT,
                        "站点收下但还要审核，审核通过前拿不到站点种子，没挂回 qB。算一条新发种，审核通过后由回访自动补挂。",
                    ))
                    .child(status_row(
                        "posted_unresolved",
                        theme::DANGER,
                        "已经上传，但回包没给可用详情页、站内也没定位到，种子没挂回 qB。算一条上传尝试，不重发，等人工核对/补挂。",
                    )),
            )
            .child(widgets::hint(
                "existed / skipped / failed 不算新发种；URL 带 existed=1 的旧记录也不算。时间固定按北京时间。",
            ))
            .child(header_row)
            .child(list);

        root = root.child(
            div()
                .flex_none()
                .text_xs()
                .text_color(gpui::rgb(theme::TEXT_FAINT))
                .child("最后一条种是该站最近一次成功记录里的详情页；日志没给链接时显示 —。"),
        );

        root.into_any_element()
    }
}

/// 一条结果说明：左侧固定宽度标签，右侧白话解释；四行共用同一套排版。
fn status_row(label: &'static str, color: u32, desc: &'static str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_start()
        .gap_2()
        .child(
            div()
                .w(px(132.))
                .flex_none()
                .child(widgets::badge(label, color)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_xs()
                .text_color(gpui::rgb(theme::TEXT_DIM))
                .child(desc),
        )
}
