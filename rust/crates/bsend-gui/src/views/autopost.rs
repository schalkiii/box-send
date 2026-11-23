//! 自动发种页：把转种草稿批量发布到目标站（门禁 + 日限额 + 退避 + 审计），可演练。
//!
//! 布局硬规则（见 DESIGN-SPEC §3 / §4 自动发种）：
//! - 控制条内站点 chip 区固定高度 `CHIP_ZONE_MAX_H` + 内部滚动；
//! - 演练（accent）与真发布（danger）之间用 `S4` 留白隔离，危险按钮文字明确不可撤销；
//! - 草稿预览进 `scroll_list`，行高固定；空 / 运行中 / 错误三态齐全。

use gpui::{div, prelude::*, px, AnyElement, Context};

use crate::theme;
use crate::types::Job;
use crate::widgets;
use crate::BsendGui;

/// 预览列表里草稿数定宽列
const COL_COUNT: gpui::Pixels = theme::COL_NUM;
/// 滚动条宽度（与 widgets::scroll_list 一致的工艺值）
const SCROLLBAR_W: gpui::Pixels = px(8.);

impl BsendGui {
    pub(crate) fn render_autopost(&self, cx: &mut Context<Self>) -> AnyElement {
        // ---- 草稿预览：按目标站归并计数（只读）----
        let mut by_site: Vec<(String, usize)> = Vec::new();
        for d in &self.drafts {
            match by_site.iter_mut().find(|(s, _)| s == &d.target_site) {
                Some((_, n)) => *n += 1,
                None => by_site.push((d.target_site.clone(), 1)),
            }
        }

        let error_hint = self
            .log
            .last()
            .map(|l| l.as_str())
            .filter(|l| l.starts_with("失败") || l.contains("中断"))
            .map(|l| l.to_string());

        // ---- 控制条：目标站子集（固定高度 + 内部滚动）+ 试运行 + 上限 ----
        let mut site_chips = div().flex().flex_wrap().items_center().gap_1();
        for s in &self.config.sites {
            let site_id = s.id.clone();
            let selected = self.autopost_sites.contains(&site_id);
            site_chips = site_chips.child(widgets::chip(
                &format!("autopost-site-{site_id}"),
                s.name.clone(),
                selected,
                cx.listener(move |this: &mut Self, _, _, cx| {
                    if let Some(pos) = this.autopost_sites.iter().position(|x| x == &site_id) {
                        this.autopost_sites.remove(pos);
                    } else {
                        this.autopost_sites.push(site_id.clone());
                    }
                    cx.notify();
                }),
            ));
        }

        let limit_presets: [(&str, Option<usize>); 4] = [
            ("1", Some(1)),
            ("5", Some(5)),
            ("10", Some(10)),
            ("不限", None),
        ];
        let mut limit_chips = div().flex().flex_none().items_center().gap_1();
        for (label, value) in limit_presets {
            let selected = self.autopost_limit == value;
            limit_chips = limit_chips.child(widgets::chip(
                &format!("autopost-limit-{label}"),
                label,
                selected,
                cx.listener(move |this: &mut Self, _, _, cx| {
                    this.autopost_limit = value;
                    cx.notify();
                }),
            ));
        }

        let bar = widgets::control_bar()
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(gpui::rgb(theme::TEXT_FAINT))
                    .truncate()
                    .child("目标站（空 = 全部有草稿的站）"),
            )
            .child(
                div()
                    .id("autopost-site-zone")
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
                "autopost-dry",
                if self.autopost_dry {
                    "试运行：开"
                } else {
                    "试运行：关"
                },
                self.autopost_dry,
                cx.listener(|this: &mut Self, _, _, cx| {
                    this.autopost_dry = !this.autopost_dry;
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
            .child(limit_chips);

        // ---- 主按钮区：运行中用 tag 占位；否则演练（accent）与真发布（danger）S4 隔离 ----
        let action = if self.busy {
            div()
                .flex_none()
                .child(widgets::tag("ap-busy", "任务运行中…", theme::WARN_BG))
                .into_any_element()
        } else {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .child(widgets::button_accent(
                    "ap-dry-run",
                    "开始演练（只审不发）",
                    cx.listener(|this: &mut Self, _, _, cx| {
                        this.autopost_dry = true;
                        this.start_job(Job::Autopost, cx);
                    }),
                ))
                .child(div().w(theme::S4))
                .child(widgets::button_danger(
                    "ap-publish",
                    "真发布（不可撤销）",
                    cx.listener(|this: &mut Self, _, _, cx| {
                        this.autopost_dry = false;
                        this.start_job(Job::Autopost, cx);
                    }),
                ))
                .into_any_element()
        };

        // ---- 草稿预览（只读）----
        let preview = if by_site.is_empty() {
            widgets::scroll_list("autopost-preview")
                .child(widgets::empty_state(
                    "暂无待发布草稿",
                    "先在「转种草稿」页确认转种；有草稿后这里会按目标站汇总条数。",
                ))
                .into_any_element()
        } else {
            let mut body = widgets::scroll_list("autopost-preview")
                .flex_col()
                .gap_1()
                .pb_1();
            for (i, (site, count)) in by_site.iter().enumerate() {
                body = body.child(
                    widgets::row(i)
                        .child(widgets::cell_flex(site.to_string()))
                        .child(widgets::cell(COL_COUNT, format!("{count} 条草稿"), true)),
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
                    .child(widgets::title("自动发种"))
                    .child(div().flex_1().min_w(px(0.)))
                    .when(self.busy, |d| {
                        d.child(widgets::badge(
                            format!("运行中 · 已处理 {} 格", self.cells_done),
                            theme::WARN,
                        ))
                    }),
            )
            .child(widgets::hint(
                "把「跑一轮」产出的转种草稿批量发布到目标站：逐条跑发布检查与审阅，通过才上传；判重跳过不占限额，拦截/失败进退避（24h/6h），每站每日限额按北京时间日计，全程落审计 auto-audit.jsonl。",
            ))
            .child(bar)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .child(action)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_xs()
                            .text_color(gpui::rgb(theme::TEXT_FAINT))
                            .truncate()
                            .child("演练只跑发布检查与字段构造、不提交；真发布逐条上传，进度与结论见右侧日志。"),
                    ),
            );

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
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(gpui::rgb(theme::TEXT_DIM))
                    .truncate()
                    .child(format!("当前草稿：{} 条", self.drafts.len())),
            )
            .child(preview);

        root.into_any_element()
    }
}
