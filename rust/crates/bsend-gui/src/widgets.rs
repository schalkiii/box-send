//! 共享控件：按钮 / tab / chip / 徽标 / KPI / 表格头与单元格 / 空状态 / 进度条。
//! 所有页只调这里的控件，保证间距、圆角、字号、hover 反馈全站一致。
//!
//! 硬规则（对本 crate 所有视图有效）：
//! 1. 任何文本都必须能截断：放在定宽容器里配 `.truncate()`，或放在 `.flex_1().min_w(px(0.))` 里；
//! 2. 长列表必须放进 `id(...).overflow_y_scroll()` 的滚动容器，行高固定（theme::ROW_H / CARD_H）；
//! 3. 一个 flex 行里的子元素总宽不得超过容器宽度（超出部分会画到隔壁列上 = 字体重叠）。

use gpui::prelude::*;
use gpui::{
    div, px, rgb, App, ClickEvent, Div, FontWeight, IntoElement, SharedString, Stateful, Window,
};

use crate::theme;

/// 次级按钮（刷新 / 打开 / 复制这类无副作用动作）
pub(crate) fn button(
    id: &str,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex_none()
        .h(theme::BTN_H)
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(theme::SURFACE_HOVER))
        .hover(|s| s.bg(rgb(0x2d333b)))
        .active(|s| s.opacity(0.75))
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .text_color(rgb(theme::TEXT))
        .on_click(on_click)
        .child(div().truncate().child(label.into()))
}

/// 主操作按钮（跑一轮 / 确认转种 / 演练自动发种）
pub(crate) fn button_accent(
    id: &str,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex_none()
        .h(theme::BTN_H)
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(theme::SUCCESS_BG))
        .hover(|s| s.bg(rgb(theme::SUCCESS_HOVER)))
        .active(|s| s.opacity(0.75))
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .font_weight(FontWeight(600.0))
        .text_color(rgb(0xffffff))
        .on_click(on_click)
        .child(div().truncate().child(label.into()))
}

/// 危险操作按钮（真发布 / 删除）：红底白字，和主按钮视觉隔离
pub(crate) fn button_danger(
    id: &str,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex_none()
        .h(theme::BTN_H)
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(theme::DANGER_BG))
        .hover(|s| s.bg(rgb(theme::DANGER_HOVER)))
        .active(|s| s.opacity(0.75))
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .font_weight(FontWeight(600.0))
        .text_color(rgb(0xffffff))
        .on_click(on_click)
        .child(div().truncate().child(label.into()))
}

/// 只读状态标记（不是按钮：没有 hover、不可点）
pub(crate) fn tag(id: &str, label: impl Into<SharedString>, bg: u32) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex_none()
        .h(theme::BTN_H)
        .px_3()
        .flex()
        .items_center()
        .bg(rgb(bg))
        .rounded_md()
        .text_sm()
        .text_color(rgb(theme::TEXT_DIM))
        .child(div().truncate().child(label.into()))
}

/// 顶部 tab
pub(crate) fn tab(
    id: &str,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex_none()
        .h(px(30.))
        .px_3()
        .flex()
        .items_center()
        .rounded_md()
        .bg(rgb(if active {
            theme::ACCENT_BG
        } else {
            theme::SURFACE_ALT
        }))
        .hover(|s| {
            s.bg(rgb(if active {
                theme::ACCENT_BG
            } else {
                theme::SURFACE_HOVER
            }))
        })
        .active(|s| s.opacity(0.8))
        .cursor_pointer()
        .text_sm()
        .text_color(rgb(if active { 0xffffff } else { theme::TEXT_DIM }))
        .on_click(on_click)
        .child(div().truncate().child(label.into()))
}

/// 可点选的筛选项（站点选择 / 上限预设 / 试运行开关）
pub(crate) fn chip(
    id: &str,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex_none()
        .h(theme::CHIP_H)
        .px_2()
        .flex()
        .items_center()
        .rounded_md()
        .bg(rgb(if active {
            theme::ACCENT_BG
        } else {
            theme::SURFACE_ALT
        }))
        .hover(|s| {
            s.bg(rgb(if active {
                theme::ACCENT_BG
            } else {
                theme::SURFACE_HOVER
            }))
        })
        .active(|s| s.opacity(0.8))
        .cursor_pointer()
        .text_xs()
        .text_color(rgb(if active { 0xffffff } else { theme::TEXT_DIM }))
        .on_click(on_click)
        .child(div().truncate().child(label.into()))
}

/// 警告色 chip（试运行开关打开时的视觉提醒）
pub(crate) fn chip_warn(
    id: &str,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex_none()
        .h(theme::CHIP_H)
        .px_2()
        .flex()
        .items_center()
        .rounded_md()
        .bg(rgb(if active {
            theme::WARN_BG
        } else {
            theme::SURFACE_ALT
        }))
        .hover(|s| {
            s.bg(rgb(if active {
                theme::WARN_BG
            } else {
                theme::SURFACE_HOVER
            }))
        })
        .active(|s| s.opacity(0.8))
        .cursor_pointer()
        .text_xs()
        .text_color(rgb(if active { 0xffffff } else { theme::TEXT_DIM }))
        .on_click(on_click)
        .child(div().truncate().child(label.into()))
}

/// 圆点 + 文字的状态徽标
pub(crate) fn badge(label: impl Into<SharedString>, color: u32) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .child(div().size(px(7.)).flex_none().rounded_full().bg(rgb(color)))
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme::TEXT_DIM))
                .truncate()
                .child(label.into()),
        )
}

/// 面板标题
pub(crate) fn title(text: impl Into<SharedString>) -> Div {
    div()
        .text_lg()
        .font_weight(FontWeight(600.0))
        .text_color(rgb(theme::TEXT))
        .truncate()
        .child(text.into())
}

/// 次级说明文字（可换行，不截断）
pub(crate) fn hint(text: impl Into<SharedString>) -> Div {
    div()
        .text_xs()
        .text_color(rgb(theme::TEXT_DIM))
        .child(text.into())
}

/// KPI 小卡片：标签 + 主数值 + 补充说明
pub(crate) fn kpi(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    sub: impl Into<SharedString>,
) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .bg(rgb(theme::SURFACE_ALT))
        .rounded_md()
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme::TEXT_FAINT))
                .truncate()
                .child(label.into()),
        )
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight(600.0))
                .text_color(rgb(theme::TEXT))
                .truncate()
                .child(value.into()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme::TEXT_FAINT))
                .truncate()
                .child(sub.into()),
        )
}

/// 表头单元格（定宽）
pub(crate) fn head_cell(w: gpui::Pixels, text: impl Into<SharedString>, right: bool) -> Div {
    let mut d = div()
        .flex_none()
        .w(w)
        .text_xs()
        .text_color(rgb(theme::TEXT_FAINT))
        .truncate();
    if right {
        d = d.text_right();
    }
    d.child(text.into())
}

/// 表格单元格（定宽 + 截断）
pub(crate) fn cell(w: gpui::Pixels, text: impl Into<SharedString>, right: bool) -> Div {
    let mut d = div().flex_none().w(w).text_sm().truncate();
    if right {
        d = d.text_right();
    }
    d.child(text.into())
}

/// 吃掉剩余宽度的单元格（截断，不挤压邻居）
pub(crate) fn cell_flex(text: impl Into<SharedString>) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .text_sm()
        .truncate()
        .child(text.into())
}

/// 数据行外壳：固定行高 + 细分隔线 + 斑马纹
pub(crate) fn row(index: usize) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(theme::ROW_H)
        .px_2()
        .gap_2()
        .bg(rgb(if index % 2 == 1 {
            theme::SURFACE_ALT
        } else {
            theme::SURFACE
        }))
        .border_b_1()
        .border_color(rgb(theme::BORDER_SUBTLE))
        .text_sm()
}

/// 空状态
pub(crate) fn empty_state(
    title_text: impl Into<SharedString>,
    desc: impl Into<SharedString>,
) -> Div {
    div()
        .flex()
        .flex_1()
        .min_h(px(120.))
        .items_center()
        .justify_center()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_base()
                .text_color(rgb(theme::TEXT_DIM))
                .child(title_text.into()),
        )
        .child(
            div()
                .w(px(520.))
                .text_xs()
                .text_color(rgb(theme::TEXT_FAINT))
                .child(desc.into()),
        )
}

/// 覆盖率进度条（width 为像素宽度）
pub(crate) fn progress(ratio: f32, width: f32) -> Div {
    let clamped = ratio.clamp(0.0, 1.0);
    div()
        .flex_none()
        .w(px(width))
        .h(px(6.))
        .rounded_full()
        .bg(rgb(theme::SURFACE_HOVER))
        .child(
            div()
                .w(px(width * clamped))
                .h(px(6.))
                .rounded_full()
                .bg(rgb(if clamped >= 1.0 {
                    theme::SUCCESS
                } else if clamped >= 0.5 {
                    theme::ACCENT
                } else {
                    theme::WARN
                })),
        )
}

/// 纵向滚动容器：定宽定高、行高固定、自己处理滚轮
pub(crate) fn scroll_list(id: &'static str) -> Stateful<Div> {
    div()
        .id(SharedString::from(id.to_string()))
        .flex_1()
        .min_h(px(0.))
        .w_full()
        .overflow_y_scroll()
        .scrollbar_width(px(8.))
}

/// 面板外壳：底色 + 圆角 + 内边距 + 裁切（子元素溢出不会画到面板外）
pub(crate) fn panel_card() -> Div {
    div()
        .flex()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .flex_col()
        .gap_2()
        .p_3()
        .bg(rgb(theme::SURFACE))
        .rounded_lg()
        .overflow_hidden()
}

/// 控制条（内含 chips 的次级块）
pub(crate) fn control_bar() -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .p_2()
        .bg(rgb(theme::SURFACE_ALT))
        .rounded_md()
}
