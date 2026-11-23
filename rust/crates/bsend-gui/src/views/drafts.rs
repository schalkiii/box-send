//! 转种草稿页：源站 + 详情链接 + 目标站，「审阅」只审不发，「确认转种」人工确认后一条龙。
//!
//! 布局硬规则（见 DESIGN-SPEC §3 / §4 草稿）：
//! - 卡片固定 `CARD_H`，标题 / 站点行 / 链接行全部单行截断，不无限换行；
//! - 卡片列表进 `scroll_list`，卡片间距 `S2`，底色 `SURFACE_ALT` + 圆角；
//! - 按钮统一走 widgets（命中高度 `BTN_H`，带 hover 反馈）。

use gpui::{div, prelude::*, px, AnyElement, ClipboardItem, Context};

use crate::theme;
use crate::widgets;
use crate::BsendGui;

impl BsendGui {
    pub(crate) fn render_drafts(&self, cx: &mut Context<Self>) -> AnyElement {
        // ---- 筛选：0 全部 / 1 只看待处理（草稿列表本身即待处理队列，状态字段在账本侧）----
        let visible: Vec<_> = self.drafts.iter().collect();

        let error_hint = self
            .log
            .last()
            .map(|l| l.as_str())
            .filter(|l| l.starts_with("失败") || l.contains("中断"))
            .map(|l| l.to_string());

        let mut root = widgets::panel_card()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .child(widgets::title(format!(
                        "转种草稿（{} 条）",
                        self.drafts.len()
                    )))
                    .child(div().flex_1().min_w(px(0.)))
                    .child(widgets::chip(
                        "draft-filter-all",
                        "全部",
                        self.drafts_filter == 0,
                        cx.listener(|this: &mut Self, _, _, cx| {
                            this.drafts_filter = 0;
                            cx.notify();
                        }),
                    ))
                    .child(widgets::chip(
                        "draft-filter-pending",
                        "只看待处理",
                        self.drafts_filter == 1,
                        cx.listener(|this: &mut Self, _, _, cx| {
                            this.drafts_filter = 1;
                            cx.notify();
                        }),
                    ))
                    .when(self.busy, |d| {
                        d.child(widgets::badge(
                            format!("运行中 · 已处理 {} 格", self.cells_done),
                            theme::WARN,
                        ))
                    }),
            )
            .child(widgets::hint(
                "「审阅」= 只审不发：跑源页解析、预检、字段构造，把将提交的字段清单打进日志。核对没问题再点「确认转种」= 人工确认：预检 → 上传 → 发后自检 → 推种 → 校验。",
            ));

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

        let list = if visible.is_empty() {
            widgets::scroll_list("drafts-list")
                .child(widgets::empty_state(
                    "暂无草稿",
                    "跑一轮后，站内没有的段落在源站定位成功时会出现在这里。",
                ))
                .into_any_element()
        } else {
            let mut body = widgets::scroll_list("drafts-list")
                .flex_col()
                .gap_2()
                .pb_1();
            for (i, draft) in visible.iter().enumerate() {
                let d = (*draft).clone();
                let d_url = d.source_url.clone();
                let d_review = d.clone();
                let d_reseed = d.clone();
                let card = div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .h(theme::CARD_H)
                    .bg(gpui::rgb(theme::SURFACE_ALT))
                    .rounded_md()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(gpui::rgb(theme::TEXT))
                                    .line_clamp(1)
                                    .truncate()
                                    .child(d.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui::rgb(theme::TEXT_DIM))
                                    .truncate()
                                    .child(format!("{} → {}", d.source_site, d.target_site)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui::rgb(theme::TEXT_FAINT))
                                    .truncate()
                                    .child(format!("源站：{}", d.source_url)),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(widgets::button(
                                &format!("copy-draft-{i}"),
                                "复制源站链接",
                                cx.listener(move |_, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(d_url.clone()));
                                }),
                            ))
                            .child(widgets::button(
                                &format!("review-draft-{i}"),
                                "审阅",
                                cx.listener(move |this: &mut Self, _, _, cx| {
                                    this.start_review(&d_review, cx)
                                }),
                            ))
                            .child(widgets::button_accent(
                                &format!("reseed-draft-{i}"),
                                "确认转种",
                                cx.listener(move |this: &mut Self, _, _, cx| {
                                    this.start_reseed(&d_reseed, cx)
                                }),
                            )),
                    );
                body = body.child(card);
            }
            body.into_any_element()
        };

        root.child(list).into_any_element()
    }
}
