//! 右侧任务日志面板：展开固定宽度、最新在上、单行截断、可滚动；可收起成窄条。
//! 不做滚动到底（避免跟用户正在进行的滚动打架）。
//!
//! 宽度固定的关键点：面板不能复用 `panel_card()`——它内部带 `flex_1()`，会把
//! `flex_basis` 置成 relative(0)；事后 `flex_none().w(...)` 只改 grow/shrink，
//! 不改 basis，`w(...)` 会被按内容收缩覆盖。这里显式写 `flex_basis + w + min_w`
//! 三件套，保证 340px 固定宽不被压成窄条。

use gpui::{div, prelude::*, px, rgb, AnyElement, Context, SharedString};

use crate::theme;
use crate::widgets;
use crate::BsendGui;

/// 收起态窄条宽度（竖向单字堆叠）
const LOG_COLLAPSE_W: gpui::Pixels = px(44.);

impl BsendGui {
    pub(crate) fn render_log(&self, cx: &mut Context<Self>) -> AnyElement {
        // ---- 收起态：44px 竖向窄条，点一下展开 ----
        if !self.log_open {
            return div()
                .id(SharedString::from("log-expand"))
                .flex()
                .flex_none()
                .flex_basis(LOG_COLLAPSE_W)
                .w(LOG_COLLAPSE_W)
                .min_w(LOG_COLLAPSE_W)
                .h_full()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_1()
                .py_2()
                .bg(rgb(theme::SURFACE))
                .rounded_lg()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(theme::SURFACE_HOVER)))
                .active(|s| s.opacity(0.8))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.log_open = true;
                    cx.notify();
                }))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme::TEXT_DIM))
                        .truncate()
                        .child("日"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme::TEXT_DIM))
                        .truncate()
                        .child("志"),
                )
                .into_any_element();
        }

        // ---- 展开态：固定宽度面板（自带容器，不走 panel_card 的 flex_1）----
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .text_sm()
                    .text_color(rgb(theme::TEXT))
                    .truncate()
                    .child(format!("任务日志（{} 条）", self.log.len())),
            )
            .child(div().flex_1().min_w(px(0.)))
            .child(widgets::button(
                "log-clear",
                "清空",
                cx.listener(|this, _, _, cx| {
                    this.log.clear();
                    cx.notify();
                }),
            ))
            .child(widgets::button(
                "log-collapse",
                "收起",
                cx.listener(|this, _, _, cx| {
                    this.log_open = false;
                    cx.notify();
                }),
            ));

        let mut body = widgets::scroll_list("log-list").pb(theme::S1);
        if self.log.is_empty() {
            body = body.child(widgets::empty_state(
                "暂无日志",
                "后台任务的每一步都会追加在这里。",
            ));
        } else {
            for line in self.log.iter().rev().take(200) {
                body = body.child(
                    div()
                        .flex_none()
                        .h(theme::LOG_ROW_H)
                        .text_xs()
                        .text_color(rgb(theme::TEXT_DIM))
                        .truncate()
                        .child(line.clone()),
                );
            }
        }

        div()
            .flex()
            .flex_none()
            .flex_basis(theme::LOG_W)
            .w(theme::LOG_W)
            .min_w(theme::LOG_W)
            .h_full()
            .min_h(px(0.))
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(theme::SURFACE))
            .rounded_lg()
            .overflow_hidden()
            .child(header)
            .child(body)
            .into_any_element()
    }
}
