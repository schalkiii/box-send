//! 诊断页：下载器连通、CookieCloud 拉取、各站 cookie 在位、今日发种台账与退避。
//! 三段只读清单，分段用细分隔线；空/忙/错三态都有可见反馈。

use gpui::{div, prelude::*, px, rgb, AnyElement, Context};

use bsend::autopost::{audit_path_of, in_backoff, parse_audit};
use bsend::coverage::now_unix;

use crate::theme;
use crate::types::Job;
use crate::widgets;
use crate::BsendGui;

const COL_COUNT: gpui::Pixels = px(90.);

impl BsendGui {
    pub(crate) fn render_diag(&self, cx: &mut Context<Self>) -> AnyElement {
        let now = now_unix();
        let audit_path = audit_path_of(&self.config);
        let audit_text = std::fs::read_to_string(&audit_path).unwrap_or_default();
        let (posted_today, last_bad) = parse_audit(&audit_text, now);
        let backoff_count = last_bad
            .values()
            .filter(|v| in_backoff(Some(**v), now))
            .count();

        // ---- 页头：标题 + 运行中徽标 ----
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(widgets::title("诊断"))
            .child(div().flex_1().min_w(px(0.)))
            .when(self.busy, |d| {
                d.child(widgets::badge("运行中", theme::WARN))
            });

        // ---- 动作按钮：忙时降级为只读 tag ----
        let actions = if self.busy {
            div()
                .flex_none()
                .items_center()
                .gap_2()
                .child(widgets::tag("diag-qb", "测试下载器", theme::SURFACE_ALT))
                .child(widgets::tag(
                    "diag-cookiecloud",
                    "拉取 CookieCloud",
                    theme::SURFACE_ALT,
                ))
        } else {
            div()
                .flex_none()
                .items_center()
                .gap_2()
                .child(widgets::button(
                    "diag-qb",
                    "测试下载器连接",
                    cx.listener(|this, _, _, cx| this.start_job(Job::TestQb, cx)),
                ))
                .child(widgets::button(
                    "diag-cookiecloud",
                    "拉取 CookieCloud",
                    cx.listener(|this, _, _, cx| this.start_job(Job::PullCookies, cx)),
                ))
        };

        let cloud_note = div().flex_none().child(widgets::hint(
            "CookieCloud 服务没起时会报错——可先用本地 cookies.json，服务恢复后再拉；拉取成功自动合并并热重载。",
        ));

        // ---- 第一段：各站 cookie 在位 ----
        let cookie_label = div()
            .flex_none()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_DIM))
                    .child("各站 cookie："),
            )
            .child(div().flex_1().min_w(px(0.)))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_FAINT))
                    .truncate()
                    .child(format!("{} 站", self.config.sites.len())),
            );

        let mut cookie_list = widgets::scroll_list("diag-cookies").pb(theme::S1);
        if self.config.sites.is_empty() {
            cookie_list = cookie_list.child(widgets::empty_state(
                "没有参与站点",
                "配置里 enabled 且在白名单内的站点才会列在这里。",
            ));
        }
        for (i, s) in self.config.sites.iter().enumerate() {
            let has_cookie = self.config.cookie_header_for(s.primary_host()).is_some();
            let status = if has_cookie {
                ("cookie 在位", theme::SUCCESS)
            } else {
                ("缺 cookie（拉 CookieCloud 或手工补）", theme::DANGER)
            };
            cookie_list = cookie_list.child(
                widgets::row(i)
                    .child(widgets::cell(theme::COL_SITE, s.name.clone(), false))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .items_center()
                            .child(widgets::badge(status.0, status.1)),
                    ),
            );
        }

        // ---- 第二段：今日发种台账 ----
        let mut posted: Vec<(&String, &u32)> = posted_today.iter().collect();
        posted.sort();

        let posted_label = div()
            .flex_none()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_DIM))
                    .child("今日发种台账："),
            )
            .child(div().flex_1().min_w(px(0.)))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_FAINT))
                    .truncate()
                    .child(format!(
                        "退避中 {backoff_count} 条（发布检查拦截 24h / 失败 6h）"
                    )),
            );

        let mut posted_list = widgets::scroll_list("diag-posted").pb(theme::S1);
        if posted.is_empty() {
            posted_list = posted_list.child(
                div()
                    .flex_none()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_FAINT))
                    .child("今日尚无自动发布记录（北京时间日界）。"),
            );
        }
        for (i, (site, count)) in posted.iter().enumerate() {
            posted_list = posted_list.child(
                widgets::row(i)
                    .child(widgets::cell_flex(site.to_string()))
                    .child(widgets::cell(COL_COUNT, count.to_string(), true)),
            );
        }

        // ---- 第三段：路径信息（全部 truncate，不换行）----
        let paths = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_xs()
                    .text_color(rgb(theme::TEXT_DIM))
                    .truncate()
                    .child(format!(
                        "索引：{}",
                        self.config.coverage_state_path().display()
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_xs()
                    .text_color(rgb(theme::TEXT_DIM))
                    .truncate()
                    .child(format!("审计：{}", audit_path.display())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_xs()
                    .text_color(rgb(theme::TEXT_DIM))
                    .truncate()
                    .child(format!("Cookie：{}", self.config.cookies_path().display())),
            );

        let page = widgets::panel_card()
            .child(header)
            .child(actions)
            .child(cloud_note)
            .when(self.config.cookie_cloud.is_none(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(rgb(theme::WARN))
                        .truncate()
                        .child("未配置 CookieCloud 段：cookie 需手工放进本地 cookies.json。"),
                )
            })
            .child(cookie_label)
            .child(cookie_list)
            .child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(rgb(theme::BORDER_SUBTLE))
                    .child(posted_label),
            )
            .child(posted_list)
            .child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(rgb(theme::BORDER_SUBTLE))
                    .child(paths),
            );

        page.into_any_element()
    }
}
