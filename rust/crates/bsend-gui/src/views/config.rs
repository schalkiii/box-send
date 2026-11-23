//! 配置页：总开关 / 站点启用 / 白名单 / 检索预算直接写回 boxsend.json。
//! 所有写操作都走 `mutate_config`（自动 .bak-gui 备份 + 热重载，保留未知键）；
//! 147 个站点行靠滚动，固定表头，长文本一律截断。

use gpui::{div, prelude::*, px, rgb, AnyElement, Context};
use serde_json::Value;

use crate::theme;
use crate::widgets;
use crate::BsendGui;

// 站点表列宽（表头与数据行共用；定宽列之和 + gap 必须小于正文可用宽度）
const COL_ENABLE: gpui::Pixels = px(104.);
const COL_WHITELIST: gpui::Pixels = px(124.);

impl BsendGui {
    pub(crate) fn render_config(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = widgets::panel_card();

        // ---- 读取原始 JSON 段：页头计数与表内行数共用同一份过滤结果 ----
        let assist = self
            .raw_config
            .get("assist")
            .cloned()
            .unwrap_or(Value::Null);
        let publish_on = assist
            .get("publishEnabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let budget = assist
            .get("roundBudget")
            .and_then(|v| v.as_i64())
            .unwrap_or(200);

        let whitelist: Vec<String> = assist
            .get("siteIDs")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let raw_sites: Vec<&Value> = self
            .raw_config
            .get("sourceSites")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        let total = raw_sites.len();

        let mut entries: Vec<(String, String, bool, bool, String)> = Vec::new();
        for s in &raw_sites {
            let Some(site_id) = s.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let site_id = site_id.to_string();
            let name = s
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(&site_id)
                .to_string();
            let enabled = s.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
            if self.config_filter == 1 && !enabled {
                continue;
            }
            let in_whitelist = whitelist.contains(&site_id);
            let note = s
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            entries.push((site_id, name, enabled, in_whitelist, note));
        }
        let visible = entries.len();

        // ---- 页头（固定，不随列表滚动）----
        // 计数口径 = 当前筛选下可见行数；筛选「只看已启用」时行数随之变少。
        let count_label = if self.config_filter == 1 {
            format!("已启用 {visible} / 共 {total} 站")
        } else {
            format!("共 {visible} 站")
        };
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(widgets::title("配置"))
            .child(div().flex_1().min_w(px(0.)))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_FAINT))
                    .truncate()
                    .child(count_label),
            );

        let path_hint = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(rgb(theme::TEXT_DIM))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .child(format!(
                        "文件：{}｜改动即写回：旧版备份 .bak-gui、未知键保留、热重载生效（运行中任务不受影响）",
                        self.config_path.display()
                    )),
            );

        // ---- 动作 + 总开关 + 筛选 + 预算（控制条固定高度，不滚动）----
        let action_bar = widgets::control_bar()
            .child(widgets::button(
                "cfg-reload",
                "重载配置",
                cx.listener(|this, _, _, cx| {
                    this.reload_config();
                    cx.notify();
                }),
            ))
            .child(widgets::button(
                "cfg-open",
                "打开配置文件",
                cx.listener(|this, _, _, cx| {
                    let path = this.config_path.clone();
                    match std::process::Command::new("cmd")
                        .args(["/C", "start", "", "notepad"])
                        .arg(&path)
                        .spawn()
                    {
                        Ok(_) => {
                            this.push_log("已用记事本打开配置文件（手工改完点「重载配置」）".into())
                        }
                        Err(e) => this.push_log(format!("打开编辑器失败：{e}")),
                    }
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_DIM))
                    .child("真实发种"),
            )
            .child(widgets::chip(
                "cfg-publish",
                if publish_on { "开" } else { "关" },
                publish_on,
                cx.listener(move |this, _, _, cx| {
                    this.mutate_config(cx, move |v| {
                        v["assist"]["publishEnabled"] = Value::Bool(!publish_on);
                    });
                }),
            ));

        let mut filter_bar = widgets::control_bar()
            .child(widgets::chip(
                "cfg-filter-all",
                "全部",
                self.config_filter == 0,
                cx.listener(|this, _, _, cx| {
                    this.config_filter = 0;
                    cx.notify();
                }),
            ))
            .child(widgets::chip(
                "cfg-filter-enabled",
                "只看已启用",
                self.config_filter == 1,
                cx.listener(|this, _, _, cx| {
                    this.config_filter = 1;
                    cx.notify();
                }),
            ))
            .child(div().flex_1().min_w(px(0.)))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_DIM))
                    .truncate()
                    .child(format!("每轮检索预算（当前 {budget}）")),
            );
        for value in [100_i64, 200, 400] {
            filter_bar = filter_bar.child(widgets::chip(
                &format!("cfg-budget-{value}"),
                value.to_string(),
                budget == value,
                cx.listener(move |this, _, _, cx| {
                    this.mutate_config(cx, move |v| {
                        v["assist"]["roundBudget"] = Value::from(value);
                    });
                }),
            ));
        }

        // ---- 配置读取失败：红色提示，不静默 ----
        if self.raw_config.is_null() {
            return panel
                .child(header)
                .child(path_hint)
                .child(action_bar)
                .child(
                    div()
                        .flex_none()
                        .text_sm()
                        .text_color(rgb(theme::DANGER))
                        .child("配置读取失败（文件缺失或非法 JSON），修复后点「重载配置」。"),
                )
                .into_any_element();
        }

        // ---- 站点表：固定表头 + 滚动列表 ----
        let table_head = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_2()
            .h(theme::HEAD_H)
            .border_b_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .child(widgets::head_cell(theme::COL_SITE, "站点", false))
            .child(widgets::head_cell(COL_ENABLE, "启用", false))
            .child(widgets::head_cell(COL_WHITELIST, "白名单", false))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_xs()
                    .text_color(rgb(theme::TEXT_FAINT))
                    .truncate()
                    .child("备注"),
            );

        let mut list = widgets::scroll_list("config-sites").pb(theme::S1);
        if entries.is_empty() {
            list = list.child(widgets::empty_state(
                "没有匹配的站点",
                "调整筛选条件，或检查配置里的 sourceSites 段。",
            ));
        }
        for (i, (site_id, name, enabled, in_whitelist, note)) in entries.into_iter().enumerate() {
            let enabled_id = site_id.clone();
            let whitelist_id = site_id.clone();
            let row = widgets::row(i)
                .child(widgets::cell(theme::COL_SITE, name.clone(), false))
                .child(
                    div()
                        .flex_none()
                        .w(COL_ENABLE)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(widgets::chip(
                            &format!("cfg-enable-{site_id}"),
                            if enabled { "启用" } else { "停用" },
                            enabled,
                            cx.listener(move |this, _, _, cx| {
                                let target_id = enabled_id.clone();
                                this.mutate_config(cx, move |v| {
                                    if let Some(arr) =
                                        v.get_mut("sourceSites").and_then(|x| x.as_array_mut())
                                    {
                                        for item in arr.iter_mut() {
                                            if item.get("id").and_then(|x| x.as_str())
                                                == Some(target_id.as_str())
                                            {
                                                item["enabled"] = Value::Bool(!enabled);
                                            }
                                        }
                                    }
                                });
                            }),
                        )),
                )
                .child(
                    div()
                        .flex_none()
                        .w(COL_WHITELIST)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(widgets::chip(
                            &format!("cfg-whitelist-{site_id}"),
                            if in_whitelist {
                                "白名单内"
                            } else {
                                "白名单外"
                            },
                            in_whitelist,
                            cx.listener(move |this, _, _, cx| {
                                let target_id = whitelist_id.clone();
                                this.mutate_config(cx, move |v| {
                                    let mut list: Vec<String> = v["assist"]["siteIDs"]
                                        .as_array()
                                        .map(|a| {
                                            a.iter()
                                                .filter_map(|x| x.as_str().map(String::from))
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    if let Some(pos) = list.iter().position(|x| x == &target_id) {
                                        list.remove(pos);
                                    } else {
                                        list.push(target_id.clone());
                                    }
                                    v["assist"]["siteIDs"] =
                                        Value::Array(list.into_iter().map(Value::from).collect());
                                });
                            }),
                        )),
                )
                .child(widgets::cell_flex(note));
            list = list.child(row);
        }

        let mut page = panel
            .child(header)
            .child(path_hint)
            .child(action_bar)
            .child(filter_bar)
            .child(table_head);

        if let Some(notice) = self.config.skipped_notice() {
            page = page.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(theme::WARN))
                    .truncate()
                    .child(notice),
            );
        }
        page = page.child(list);
        page = page.child(
            div()
                .flex_none()
                .text_xs()
                .text_color(rgb(theme::TEXT_FAINT))
                .child("站点启用 = 参与检索/辅种；白名单空 = 全部启用站都参与。"),
        );

        page.into_any_element()
    }
}
