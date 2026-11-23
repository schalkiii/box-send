//! 说明页：转种全流程。自由函数 `render_help()`，不持有视图状态。

use gpui::{div, prelude::*, rgb, AnyElement, Div, SharedString};

use crate::theme;
use crate::widgets;

/// 说明页正文：分段编号标题 + 次级说明，长内容进滚动容器。
pub(crate) fn render_help() -> AnyElement {
    let mut body = widgets::scroll_list("help-body");

    body = body.child(widgets::hint(
        "一段内容在源站定位到源种后，先出现在「转种草稿」页；人工确认后才会真正上传到目标站。全程进度与结论见右侧任务日志。",
    ));

    body = body.child(step_block(
        "1. 审阅（只审不发）",
        "在草稿页点「审阅」：跑源页解析、预检、上传字段构造，把将提交的标题 / 副标题 / 简介 / MediaInfo / 条目链接 / 标签清单打进日志，供人工核对；这一步不会提交任何内容。",
    ));

    body = body.child(step_block(
        "2. 确认转种（点击即人工确认，随后全自动）",
        "核对无误后点「确认转种」，依次执行：",
    ));
    for line in [
        "① 预检：MediaInfo 非空、简介带 IMDb / 豆瓣 / TMDB 链接、检测到中文字幕时带中字标签；任一阻断项都不会上传。上传页带 LuckAudit 的站还会把字段送站点官方预审，官方判未通过同样不上传。",
        "② 上传：拉目标站 upload.php 表单，透传 hidden 字段，填标题 / 副标题 / 简介 / MediaInfo / 条目链接 / 标签，multipart 提交。",
        "③ 发后自检：回读新种详情页，核对标题 / MediaInfo / 条目链接真的落上了，漏项会在日志里点名。",
        "④ 推种：下载目标站的新种子，暂停加入下载器（dlLimit=1 兜底），启动校验。",
        "⑤ 收尾：校验到 100% 解除限速、格子记「已转种」；分片对不上就停掉 / 删掉；超时记「校验中」，下一轮收尾。",
    ] {
        body = body.child(
            div()
                .flex_none()
                .text_xs()
                .text_color(rgb(theme::TEXT_DIM))
                .child(line.to_string()),
        );
    }

    body = body.child(step_block(
        "3. 日志与失败原因",
        "任务日志实时显示每一步；cookie 失效 / 限流 / 必填项缺失 / 种子已存在等失败原因都会写明，进度与结论按时间顺序追加。",
    ));

    body = body.child(step_block(
        "4. MediaInfo 提交",
        "上传后 MediaInfo 自动提交到独立表单字段（technical_info 等）；没有独立字段就内嵌简介，避免「MediaInfo 为空」被打回。",
    ));

    body = body.child(
        div()
            .flex_none()
            .text_xs()
            .text_color(rgb(theme::TEXT_FAINT))
            .child("说明：v1 表单引擎只做站点无关的通用字段；个别站的特殊下拉（质量档位 / 两步上传）预检会提示「表单不认识的必填项」，遇到再逐站补 override。"),
    );

    widgets::panel_card()
        .child(widgets::title("转种全流程（Rust 原生）"))
        .child(body)
        .into_any_element()
}

/// 单个步骤块：小标题 + 一段说明。
fn step_block(title: impl Into<SharedString>, body: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex_none()
                .text_sm()
                .text_color(rgb(theme::TEXT))
                .child(title.into()),
        )
        .child(widgets::hint(body))
}
