//! bsend 的 GPUI 图形界面（全功能 Rust 原生，不再交接 GUI 版）。
//!
//! 四块内容：
//! - 覆盖概览：内容/成员种子/各站格子与覆盖率，带状态图例与「跑一轮」旋钮；
//! - 转种草稿：源站 + 详情页链接 + 目标站，「审阅」只审不发（字段清单进日志），
//!   「确认转种」原生走 预检 → 上传 → 发后自检 → 推种 → 校验一条龙
//!   （LuckAudit 三查在预检里拦）；
//! - 自动发种：把草稿批量真发布（门禁 + 日限额 + 退避 + 审计），可试运行；
//! - 配置：开关/站点启用/白名单/预算直接写回 boxsend.json（备份 + 热重载）；
//! - 诊断：下载器连通、CookieCloud 拉取、各站 cookie 在位、发种台账与退避；
//! - 后台任务：扫描 / 基线 / 跑一轮 / 转种 / 自动发种，事件走 mpsc，GPUI 定时器轮询后更新视图。

mod jobs;
mod types;

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use bsend::autopost::{audit_path_of, in_backoff, parse_audit, AutopostOptions};
use bsend::config::Config;
use bsend::coverage::{now_unix, RoundOptions};
use bsend::index::{ContentIndex, CoverageStatus};
use bsend::qbittorrent::QBittorrent;
use bsend::report::DraftRow;
use bsend::reseed::{request_from_draft, review_reseed, run_reseed};
use gpui::{
    div, prelude::*, px, rgb, size, App, Application, AsyncApp, Bounds, ClipboardItem, FontWeight,
    SharedString, Timer, WeakEntity, Window, WindowBounds, WindowOptions,
};

use crate::jobs::{
    pull_cookies_job, run_autopost_job, run_round_job, run_scan, snapshot, test_qb_job,
};
use crate::types::{GuiEvent, Job, SiteRow, Snapshot, StatusChip, Tab};

struct BsendGui {
    config: Arc<Config>,
    config_path: PathBuf,
    tab: Tab,
    busy: bool,
    stats: SharedString,
    sites: Vec<SiteRow>,
    drafts: Vec<DraftRow>,
    log: Vec<SharedString>,
    tx: Sender<GuiEvent>,
    rx: Arc<Mutex<Receiver<GuiEvent>>>,
    /// 跑一轮控制：空站点集 = 全部启用站
    round_sites: Vec<String>,
    round_dry: bool,
    round_limit: Option<usize>,
    cells_done: usize,
    /// 自动发种控制：空站点集 = 全部有草稿的目标站
    autopost_sites: Vec<String>,
    autopost_dry: bool,
    autopost_limit: Option<usize>,
    /// 配置页直接改原始 JSON（保留未知键），改完热重载 Config
    raw_config: serde_json::Value,
}

impl BsendGui {
    fn new(config: Arc<Config>, config_path: PathBuf) -> Self {
        let (tx, rx) = channel();
        let mut this = Self {
            config,
            config_path,
            tab: Tab::Overview,
            busy: false,
            stats: "载入中…".into(),
            sites: Vec::new(),
            drafts: Vec::new(),
            log: Vec::new(),
            tx,
            rx: Arc::new(Mutex::new(rx)),
            round_sites: Vec::new(),
            round_dry: false,
            round_limit: None,
            cells_done: 0,
            autopost_sites: Vec::new(),
            autopost_dry: false,
            autopost_limit: Some(10),
            raw_config: serde_json::Value::Null,
        };
        this.raw_config = load_raw(&this.config_path);
        this.refresh();
        this
    }

    /// 从 coverage.json 生成快照（本地读，不联网）
    fn refresh(&mut self) {
        match snapshot(&self.config) {
            Ok(s) => {
                self.log.push("快照已刷新".into());
                self.apply_snapshot(s);
            }
            Err(e) => self.push_log(format!("快照失败：{e:#}")),
        }
    }

    fn apply_snapshot(&mut self, s: Snapshot) {
        self.stats = s.stats.into();
        self.sites = s.sites;
        self.drafts = s.drafts;
    }

    fn push_log(&mut self, line: String) {
        self.log.push(line.into());
        if self.log.len() > 2000 {
            self.log.remove(0);
        }
    }

    /// 改配置：读最新文件 → 只动目标键（保留未知键）→ 备份 .bak-gui → 临时文件替换 → 热重载
    fn mutate_config(
        &mut self,
        cx: &mut Context<Self>,
        mutate: impl FnOnce(&mut serde_json::Value),
    ) {
        if self.busy {
            self.push_log("任务运行中，配置改动等任务结束再试".into());
            cx.notify();
            return;
        }
        let result: Result<()> = (|| {
            let text = std::fs::read_to_string(&self.config_path).context("读配置失败")?;
            let mut root: serde_json::Value =
                serde_json::from_str(&text).context("配置不是合法 JSON")?;
            mutate(&mut root);
            let file_name = self
                .config_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let backup = self
                .config_path
                .with_file_name(format!("{file_name}.bak-gui"));
            std::fs::write(&backup, &text).context("写备份失败")?;
            let tmp = self.config_path.with_extension("json.tmp-gui");
            std::fs::write(&tmp, serde_json::to_string_pretty(&root)?).context("写临时配置失败")?;
            std::fs::rename(&tmp, &self.config_path).context("替换配置失败")?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.reload_config();
                self.push_log("配置已保存（旧版备份为同名 .bak-gui）".into());
            }
            Err(e) => self.push_log(format!("配置保存失败：{e:#}")),
        }
        cx.notify();
    }

    /// 重新加载 Config 与原始 JSON（运行中的任务仍用旧 Arc，不受影响）
    fn reload_config(&mut self) {
        match Config::load(&self.config_path) {
            Ok(cfg) => {
                self.config = Arc::new(cfg);
                self.refresh();
            }
            Err(e) => self.push_log(format!("重载配置失败：{e:#}")),
        }
        self.raw_config = load_raw(&self.config_path);
    }

    fn start_job(&mut self, job: Job, cx: &mut Context<Self>) {
        if self.busy {
            self.push_log("已有任务在跑，请等它结束".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.push_log(format!(
            "启动任务：{}",
            match job {
                Job::Scan => "扫描",
                Job::Prime => "基线",
                Job::Round => "跑一轮",
                Job::Autopost => "自动发种",
                Job::TestQb => "测试下载器",
                Job::PullCookies => "拉取 CookieCloud",
                Job::Refresh => "刷新",
            }
        ));
        cx.notify();

        let cfg = self.config.clone();
        let tx = self.tx.clone();
        // persist 必须显式 true：RoundOptions 派生 Default 的 persist=false，
        // 此前 GUI 跑一轮/基线的结论不落盘（与 CLI run 行为不一致）
        let opts = match job {
            Job::Prime => RoundOptions {
                limit: Some(0),
                persist: true,
                ..Default::default()
            },
            Job::Round => {
                self.cells_done = 0;
                RoundOptions {
                    limit: self.round_limit,
                    site_filter: if self.round_sites.is_empty() {
                        None
                    } else {
                        Some(self.round_sites.iter().cloned().collect())
                    },
                    dry_run: self.round_dry,
                    persist: true,
                    content_filter: None,
                }
            }
            _ => RoundOptions::default(),
        };
        let autopost_opts = AutopostOptions {
            dry_run: self.autopost_dry,
            limit: self.autopost_limit,
            sites: if self.autopost_sites.is_empty() {
                None
            } else {
                Some(self.autopost_sites.clone())
            },
        };
        let config_path = self.config_path.clone();
        std::thread::spawn(move || {
            let result = match job {
                Job::Scan => run_scan(&cfg, &tx),
                Job::Prime | Job::Round => run_round_job(&cfg, opts, &tx),
                Job::Autopost => run_autopost_job(&cfg, autopost_opts, &tx),
                Job::TestQb => test_qb_job(&cfg, &tx),
                Job::PullCookies => pull_cookies_job(&config_path, &tx),
                Job::Refresh => snapshot(&cfg).map(|s| {
                    let _ = tx.send(GuiEvent::Snapshot(s));
                }),
            };
            match result {
                Ok(()) => {
                    let _ = tx.send(GuiEvent::Log(format!("{job:?} 完成")));
                }
                Err(e) => {
                    let _ = tx.send(GuiEvent::Log(format!("失败：{e:#}")));
                }
            }
            let _ = tx.send(GuiEvent::Busy(false));
        });
    }

    /// 原生转种：预检 → 上传 → 推种 → 校验 → 格子落账（按钮点击即人工确认）
    fn start_reseed(&mut self, draft: &DraftRow, cx: &mut Context<Self>) {
        if self.busy {
            self.push_log("已有任务在跑，请等它结束".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.push_log(format!(
            "确认转种：{}（{} → {}）。预检 → 上传 → 推种 → 校验，进度见日志",
            draft.name, draft.source_site, draft.target_site
        ));
        cx.notify();

        let cfg = self.config.clone();
        let tx = self.tx.clone();
        let req = request_from_draft(
            &draft.content_key,
            &draft.name,
            &draft.source_site,
            &draft.source_url,
            &draft.target_site,
        );
        std::thread::spawn(move || {
            let result: Result<()> = (|| {
                let qb = QBittorrent::new(
                    &cfg.downloader.url,
                    &cfg.downloader.username,
                    &cfg.downloader.password,
                )
                .context("连不上下载器")?;
                let _ = tx.send(GuiEvent::Log(qb.test_connection().context("连不上下载器")?));
                let mut index = ContentIndex::load(&cfg.coverage_state_path());
                let log_tx = tx.clone();
                let mut log = move |line: &str| {
                    let _ = log_tx.send(GuiEvent::Log(line.to_string()));
                };
                match run_reseed(&cfg, &qb, &req, &mut index, &mut log, None) {
                    Ok(outcome) => log(&outcome.display()),
                    Err(e) => {
                        log(&format!("转种失败：{e:#}"));
                        return Err(e);
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                let _ = tx.send(GuiEvent::Log(format!("转种流程中断：{e:#}")));
            }
            if let Ok(s) = snapshot(&cfg) {
                let _ = tx.send(GuiEvent::Snapshot(s));
            }
            let _ = tx.send(GuiEvent::Busy(false));
        });
    }

    /// 审阅草稿：跑「源页解析 → 预检 → 构造上传字段」但不提交，字段清单打进日志
    fn start_review(&mut self, draft: &DraftRow, cx: &mut Context<Self>) {
        if self.busy {
            self.push_log("已有任务在跑，请等它结束".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.push_log(format!(
            "审阅草稿：{}（{} → {}）。只审不发，字段清单随后打进日志",
            draft.name, draft.source_site, draft.target_site
        ));
        cx.notify();

        let cfg = self.config.clone();
        let tx = self.tx.clone();
        let req = request_from_draft(
            &draft.content_key,
            &draft.name,
            &draft.source_site,
            &draft.source_url,
            &draft.target_site,
        );
        std::thread::spawn(move || {
            let result: Result<()> = (|| {
                let log_tx = tx.clone();
                let mut log = move |line: &str| {
                    let _ = log_tx.send(GuiEvent::Log(line.to_string()));
                };
                let rep = review_reseed(&cfg, &req, &mut log, None)?;
                for line in rep.render(200).lines() {
                    log(line);
                }
                if rep.preflight.ok() {
                    log("审阅通过：点「确认转种」才会真正提交");
                } else {
                    log("审阅未通过：预检有阻断项，补齐后再发");
                }
                Ok(())
            })();
            if let Err(e) = result {
                let _ = tx.send(GuiEvent::Log(format!("审阅中断：{e:#}")));
            }
            let _ = tx.send(GuiEvent::Busy(false));
        });
    }

    /// 轮询后台事件并刷新视图
    fn start_polling(&mut self, cx: &mut Context<Self>) {
        let rx = self.rx.clone();
        cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| loop {
                Timer::after(Duration::from_millis(200)).await;
                let batch: Vec<GuiEvent> = {
                    let lock = rx.lock().unwrap();
                    let mut out = Vec::new();
                    while let Ok(ev) = lock.try_recv() {
                        out.push(ev);
                    }
                    out
                };
                if batch.is_empty() {
                    continue;
                }
                let _ = this.update(cx, |this, cx| {
                    for ev in batch {
                        match ev {
                            GuiEvent::Log(line) => this.push_log(line),
                            GuiEvent::Snapshot(s) => this.apply_snapshot(s),
                            GuiEvent::Busy(b) => this.busy = b,
                            GuiEvent::CellDone => this.cells_done += 1,
                            GuiEvent::ReloadConfig => this.reload_config(),
                        }
                    }
                    cx.notify();
                });
            },
        )
        .detach();
    }
}

/// 读原始配置 JSON（失败返回 Null，配置页显示错误而不是崩溃）
fn load_raw(path: &std::path::Path) -> serde_json::Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn button(
    id: &str,
    label: impl Into<SharedString>,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex_none()
        .px_3()
        .py_1()
        .bg(rgb(0x30363d))
        .hover(|s| s.bg(rgb(0x3d444d)))
        .active(|s| s.opacity(0.8))
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .text_color(rgb(0xe6edf3))
        .on_click(on_click)
        .child(label.into())
}

/// 主色调按钮（确认/运行类动作）
fn button_accent(
    id: &str,
    label: impl Into<SharedString>,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex_none()
        .px_3()
        .py_1()
        .bg(rgb(0x238636))
        .hover(|s| s.bg(rgb(0x2ea043)))
        .active(|s| s.opacity(0.8))
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .font_weight(FontWeight(600.0))
        .text_color(rgb(0xffffff))
        .on_click(on_click)
        .child(label.into())
}

impl Render for BsendGui {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = rgb(0x0d1117);
        let panel = rgb(0x161b22);
        let fg = rgb(0xe6edf3);
        let dim = rgb(0x8b949e);
        let accent = rgb(0x58a6ff);

        let toolbar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .bg(panel)
            .child(button(
                "tab-overview",
                "概览",
                cx.listener(|this, _, _, cx| {
                    this.tab = Tab::Overview;
                    cx.notify();
                }),
            ))
            .child(button(
                "tab-drafts",
                "转种草稿",
                cx.listener(|this, _, _, cx| {
                    this.tab = Tab::Drafts;
                    cx.notify();
                }),
            ))
            .child(button(
                "tab-autopost",
                "自动发种",
                cx.listener(|this, _, _, cx| {
                    this.tab = Tab::Autopost;
                    cx.notify();
                }),
            ))
            .child(button(
                "tab-config",
                "配置",
                cx.listener(|this, _, _, cx| {
                    this.tab = Tab::Config;
                    cx.notify();
                }),
            ))
            .child(button(
                "tab-diag",
                "诊断",
                cx.listener(|this, _, _, cx| {
                    this.tab = Tab::Diag;
                    cx.notify();
                }),
            ))
            .child(button(
                "tab-help",
                "确认说明",
                cx.listener(|this, _, _, cx| {
                    this.tab = Tab::Help;
                    cx.notify();
                }),
            ))
            .child(div().flex_1())
            .child(button(
                "job-refresh",
                "刷新",
                cx.listener(|this, _, _, cx| {
                    this.start_job(Job::Refresh, cx);
                }),
            ))
            .child(button(
                "job-scan",
                "扫描",
                cx.listener(|this, _, _, cx| {
                    this.start_job(Job::Scan, cx);
                }),
            ))
            .child(button(
                "job-prime",
                "基线",
                cx.listener(|this, _, _, cx| {
                    this.start_job(Job::Prime, cx);
                }),
            ))
            .child(button(
                "job-round",
                "跑一轮",
                cx.listener(|this, _, _, cx| {
                    this.start_job(Job::Round, cx);
                }),
            ));

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_3()
            .px_4()
            .pt_3()
            .pb_1()
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight(600.0))
                    .text_color(accent)
                    .child("bsend · 辅种引擎"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(dim)
                    .child(format!("配置：{}", self.config_path.display())),
            )
            .when(self.busy, |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xd29922))
                        .child(format!("运行中… 已处理 {} 格", self.cells_done)),
                )
            });

        let body = match self.tab {
            Tab::Overview => self.render_overview(cx),
            Tab::Drafts => self.render_drafts(cx),
            Tab::Autopost => self.render_autopost(cx),
            Tab::Config => self.render_config(cx),
            Tab::Diag => self.render_diag(cx),
            Tab::Help => render_help(),
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .child(header)
            .child(toolbar)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .gap_3()
                    .p_3()
                    .child(body)
                    .child(self.render_log(cx)),
            )
    }
}

impl BsendGui {
    fn render_overview(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let panel = rgb(0x161b22);
        let dim = rgb(0x8b949e);
        let line = rgb(0x30363d);

        // 表头：列宽与数据行一致（固定宽度列，整列左/右对齐）
        let header_row = div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(line)
            .text_xs()
            .text_color(dim)
            .child(cell_text(px(240.), "站点", false))
            .child(cell_text(px(70.), "格子", true))
            .child(cell_text(px(80.), "已覆盖", true))
            .child(cell_text(px(80.), "未判定", true))
            .child(cell_text(px(90.), "覆盖率", true))
            .child(div().flex_1().child("状态分布"));

        let mut rows = div().flex().flex_col();
        for row in &self.sites {
            let bar_w = px(80. * row.ratio.clamp(0.0, 1.0) as f32);
            let chips = row.chips.iter().map(|c| {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().size(px(8.)).rounded_full().bg(rgb(c.color)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(dim)
                            .child(format!("{} {}", c.label, c.count)),
                    )
            });
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(rgb(0x21262d))
                    .child(cell_text(px(240.), row.site.clone(), false))
                    .child(cell_text(px(70.), row.cells.to_string(), true))
                    .child(cell_text(px(80.), row.covered.to_string(), true))
                    .child(cell_text(px(80.), row.pending.to_string(), true))
                    .child(
                        div()
                            .flex_none()
                            .w(px(90.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(80.))
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(rgb(0x21262d))
                                    .child(
                                        div().w(bar_w).h(px(6.)).rounded_full().bg(rgb(0x58a6ff)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(dim)
                                    .child(format!("{:.0}%", row.ratio * 100.0)),
                            ),
                    )
                    .child(div().flex_1().flex().items_center().gap_3().children(chips)),
            );
        }

        // 图例：格子是什么 + 各状态含义
        let legend_items = [
            (0x3fb950, "已在做种", "下载器里该种子挂着这个站的 tracker"),
            (0x58a6ff, "已辅种", "从这个站找回种子并跨种成功"),
            (0xbc8cff, "已转种", "发种到这个站且校验通过"),
            (0xd29922, "校验中", "推上去了还没等到校验结论"),
            (0xf0883e, "待人工确认", "站内命中但文件清单对不上（C 级）"),
            (0x8b949e, "人工否决", "人工确认不是同一内容"),
            (
                0x484f58,
                "站内没有",
                "查过了，站内确实没有（负缓存期内不重搜）",
            ),
            (0x39c5cf, "待找源", "站内没有，源站也没定位到源种"),
            (0xf85149, "失败", "推送/上传失败，原因在格子备注里"),
            (0xdb61a2, "转种草稿", "源种已定位，等「转种草稿」页确认"),
        ];
        let legend = {
            let mut items = div().flex().flex_wrap().gap_x_4().gap_y_1();
            for (color, label, meaning) in legend_items {
                items = items.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().size(px(8.)).rounded_full().bg(rgb(color)))
                        .child(div().text_xs().child(label))
                        .child(
                            div()
                                .text_xs()
                                .text_color(dim)
                                .child(format!("—— {meaning}")),
                        ),
                );
            }
            items
        };

        // 「跑一轮」旋钮：站点子集（空 = 全部启用站）、试运行、处理格数上限
        let mut site_chips = div().flex().flex_wrap().items_center().gap_1();
        for s in &self.config.sites {
            let site_id = s.id.clone();
            let is_selected = self.round_sites.contains(&site_id);
            let bg = if is_selected { 0x1f6feb } else { 0x30363d };
            site_chips = site_chips.child(
                div()
                    .id(SharedString::from(format!("round-site-{site_id}")))
                    .px_2()
                    .py(px(2.))
                    .rounded_md()
                    .bg(rgb(bg))
                    .cursor_pointer()
                    .text_xs()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(pos) = this.round_sites.iter().position(|x| x == &site_id) {
                            this.round_sites.remove(pos);
                        } else {
                            this.round_sites.push(site_id.clone());
                        }
                        cx.notify();
                    }))
                    .child(s.name.clone()),
            );
        }
        let limit_presets: [(&str, Option<usize>); 4] = [
            ("10", Some(10)),
            ("50", Some(50)),
            ("200", Some(200)),
            ("不限", None),
        ];
        let mut limit_chips = div().flex().items_center().gap_1();
        for (label, value) in limit_presets {
            let is_selected = self.round_limit == value;
            let bg = if is_selected { 0x1f6feb } else { 0x30363d };
            limit_chips = limit_chips.child(
                div()
                    .id(SharedString::from(format!("round-limit-{label}")))
                    .px_2()
                    .py(px(2.))
                    .rounded_md()
                    .bg(rgb(bg))
                    .cursor_pointer()
                    .text_xs()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.round_limit = value;
                        cx.notify();
                    }))
                    .child(label),
            );
        }
        let round_controls = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .p_2()
            .bg(rgb(0x1c2128))
            .rounded_md()
            .child(div().text_xs().text_color(dim).child("跑一轮："))
            .child(site_chips)
            .child(
                div()
                    .id("round-dry")
                    .px_2()
                    .py(px(2.))
                    .rounded_md()
                    .bg(rgb(if self.round_dry { 0x9e6a03 } else { 0x30363d }))
                    .cursor_pointer()
                    .text_xs()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.round_dry = !this.round_dry;
                        cx.notify();
                    }))
                    .child(if self.round_dry {
                        "试运行：开"
                    } else {
                        "试运行：关"
                    }),
            )
            .child(div().text_xs().text_color(dim).child("上限"))
            .child(limit_chips);

        div()
            .flex()
            .flex_1()
            .min_w(px(0.))
            .flex_col()
            .gap_2()
            .p_3()
            .bg(panel)
            .rounded_lg()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight(600.0))
                    .child("索引概览"),
            )
            .child(div().text_color(dim).child(self.stats.clone()))
            .child(round_controls)
            .child(
                div()
                    .text_xs()
                    .text_color(dim)
                    .child("格子 = 一段内容 × 一个站点 的判定单元：按文件清单去重的每段内容，对每个参与站记一个结论。"),
            )
            .child(header_row)
            .child(rows)
            .child(div().pt_2().border_t_1().border_color(line).child(legend))
            .into_any_element()
    }

    /// 自动发种：把「跑一轮」产出的草稿批量真发布（门禁 + 日限额 + 退避 + 审计）
    fn render_autopost(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let panel = rgb(0x161b22);
        let dim = rgb(0x8b949e);

        let mut site_chips = div().flex().flex_wrap().items_center().gap_1();
        for s in &self.config.sites {
            let site_id = s.id.clone();
            let is_selected = self.autopost_sites.contains(&site_id);
            let bg = if is_selected { 0x1f6feb } else { 0x30363d };
            site_chips = site_chips.child(
                div()
                    .id(SharedString::from(format!("autopost-site-{site_id}")))
                    .px_2()
                    .py(px(2.))
                    .rounded_md()
                    .bg(rgb(bg))
                    .cursor_pointer()
                    .text_xs()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(pos) = this.autopost_sites.iter().position(|x| x == &site_id) {
                            this.autopost_sites.remove(pos);
                        } else {
                            this.autopost_sites.push(site_id.clone());
                        }
                        cx.notify();
                    }))
                    .child(s.name.clone()),
            );
        }
        let limit_presets: [(&str, Option<usize>); 4] = [
            ("1", Some(1)),
            ("5", Some(5)),
            ("10", Some(10)),
            ("不限", None),
        ];
        let mut limit_chips = div().flex().items_center().gap_1();
        for (label, value) in limit_presets {
            let is_selected = self.autopost_limit == value;
            let bg = if is_selected { 0x1f6feb } else { 0x30363d };
            limit_chips = limit_chips.child(
                div()
                    .id(SharedString::from(format!("autopost-limit-{label}")))
                    .px_2()
                    .py(px(2.))
                    .rounded_md()
                    .bg(rgb(bg))
                    .cursor_pointer()
                    .text_xs()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.autopost_limit = value;
                        cx.notify();
                    }))
                    .child(label),
            );
        }

        div()
            .flex()
            .flex_1()
            .min_w(px(0.))
            .flex_col()
            .gap_2()
            .p_3()
            .bg(panel)
            .rounded_lg()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight(600.0))
                    .child("自动发种"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(dim)
                    .child("把「跑一轮」产出的转种草稿批量发布到目标站：逐条跑门禁（G1–G5）与审阅，通过才上传；判重跳过不占限额，拦截/失败进退避（24h/6h），每站每日限额按 UTC 日计，全程落审计 auto-audit.jsonl。站点不选 = 全部有草稿的目标站。"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .bg(rgb(0x1c2128))
                    .rounded_md()
                    .child(div().text_xs().text_color(dim).child("目标站"))
                    .child(site_chips)
                    .child(
                        div()
                            .id("autopost-dry")
                            .px_2()
                            .py(px(2.))
                            .rounded_md()
                            .bg(rgb(if self.autopost_dry { 0x9e6a03 } else { 0x30363d }))
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.autopost_dry = !this.autopost_dry;
                                cx.notify();
                            }))
                            .child(if self.autopost_dry { "试运行：开" } else { "试运行：关" }),
                    )
                    .child(div().text_xs().text_color(dim).child("上限"))
                    .child(limit_chips),
            )
            .child(
                div().flex().items_center().gap_2().child(button_accent(
                    "job-autopost",
                    if self.autopost_dry {
                        "演练自动发种（只审不发）"
                    } else {
                        "开始自动发种（真发布）"
                    },
                    cx.listener(|this, _, _, cx| {
                        this.start_job(Job::Autopost, cx);
                    }),
                )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(dim)
                    .child("进度与每条结论见右侧日志；发布成功后草稿自动出队，可回「概览」刷新核对。"),
            )
            .into_any_element()
    }

    /// 配置页：改 boxsend.json 的开关/白名单/预算（保留未知键，自动备份 + 热重载）
    fn render_config(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let panel = rgb(0x161b22);
        let dim = rgb(0x8b949e);
        let mut col = div()
            .flex()
            .flex_1()
            .min_w(px(0.))
            .flex_col()
            .gap_2()
            .p_3()
            .bg(panel)
            .rounded_lg()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight(600.0))
                    .child("配置"),
            )
            .child(div().text_xs().text_color(dim).child(format!(
                "文件：{}｜改动即写回：旧版备份 .bak-gui、未知键保留、热重载生效（运行中任务不受影响）。cookie/下载器等字段请用「打开配置文件」手工编辑。",
                self.config_path.display()
            )))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(button(
                        "cfg-reload",
                        "重载配置",
                        cx.listener(|this, _, _, cx| {
                            this.reload_config();
                            cx.notify();
                        }),
                    ))
                    .child(button(
                        "cfg-open",
                        "打开配置文件",
                        cx.listener(|this, _, _, cx| {
                            let path = this.config_path.clone();
                            match std::process::Command::new("cmd")
                                .args(["/C", "start", "", "notepad"])
                                .arg(&path)
                                .spawn()
                            {
                                Ok(_) => this.push_log(
                                    "已用记事本打开配置文件（手工改完点「重载配置」）".into(),
                                ),
                                Err(e) => this.push_log(format!("打开编辑器失败：{e}")),
                            }
                            cx.notify();
                        }),
                    )),
            );

        if self.raw_config.is_null() {
            return col
                .child(
                    div()
                        .text_color(rgb(0xf85149))
                        .child("配置读取失败（文件缺失或非法 JSON），修复后点「重载配置」。"),
                )
                .into_any_element();
        }
        let assist = self
            .raw_config
            .get("assist")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        // 真实发种总开关（assist.publishEnabled）
        let publish_on = assist
            .get("publishEnabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().text_xs().text_color(dim).child("真实发种总开关"))
                .child(
                    div()
                        .id("cfg-publish")
                        .px_2()
                        .py(px(2.))
                        .rounded_md()
                        .bg(rgb(if publish_on { 0x238636 } else { 0x30363d }))
                        .cursor_pointer()
                        .text_xs()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.mutate_config(cx, move |v| {
                                v["assist"]["publishEnabled"] =
                                    serde_json::Value::Bool(!publish_on);
                            });
                        }))
                        .child(if publish_on { "开" } else { "关" }),
                ),
        );

        // 站点启用 + 辅种白名单（白名单空 = 全部启用站）
        let whitelist: Vec<String> = assist
            .get("siteIDs")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let raw_sites = self
            .raw_config
            .get("sourceSites")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut site_rows = div().flex().flex_col().gap_1();
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
            let in_whitelist = whitelist.contains(&site_id);
            let enabled_id = site_id.clone();
            let whitelist_id = site_id.clone();
            site_rows = site_rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(160.)).text_xs().child(name))
                    .child(
                        div()
                            .id(SharedString::from(format!("cfg-enable-{site_id}")))
                            .px_2()
                            .py(px(2.))
                            .rounded_md()
                            .bg(rgb(if enabled { 0x238636 } else { 0x30363d }))
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let target_id = enabled_id.clone();
                                this.mutate_config(cx, move |v| {
                                    if let Some(arr) =
                                        v.get_mut("sourceSites").and_then(|x| x.as_array_mut())
                                    {
                                        for item in arr.iter_mut() {
                                            if item.get("id").and_then(|x| x.as_str())
                                                == Some(target_id.as_str())
                                            {
                                                item["enabled"] = serde_json::Value::Bool(!enabled);
                                            }
                                        }
                                    }
                                });
                            }))
                            .child(if enabled { "启用" } else { "停用" }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("cfg-whitelist-{site_id}")))
                            .px_2()
                            .py(px(2.))
                            .rounded_md()
                            .bg(rgb(if in_whitelist { 0x1f6feb } else { 0x30363d }))
                            .cursor_pointer()
                            .text_xs()
                            .on_click(cx.listener(move |this, _, _, cx| {
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
                                    v["assist"]["siteIDs"] = serde_json::json!(list);
                                });
                            }))
                            .child(if in_whitelist {
                                "白名单内"
                            } else {
                                "白名单外"
                            }),
                    ),
            );
        }
        col = col
            .child(
                div()
                    .text_xs()
                    .text_color(dim)
                    .child("站点启用 = 参与检索/辅种；白名单空 = 全部启用站都参与"),
            )
            .child(site_rows);

        // 检索预算预设（assist.roundBudget）
        let budget = assist
            .get("roundBudget")
            .and_then(|v| v.as_i64())
            .unwrap_or(200);
        let mut budget_chips = div().flex().items_center().gap_1();
        for value in [100_i64, 200, 400] {
            let is_selected = budget == value;
            budget_chips = budget_chips.child(
                div()
                    .id(SharedString::from(format!("cfg-budget-{value}")))
                    .px_2()
                    .py(px(2.))
                    .rounded_md()
                    .bg(rgb(if is_selected { 0x1f6feb } else { 0x30363d }))
                    .cursor_pointer()
                    .text_xs()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.mutate_config(cx, move |v| {
                            v["assist"]["roundBudget"] = serde_json::json!(value);
                        });
                    }))
                    .child(value.to_string()),
            );
        }
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(dim)
                        .child(format!("每轮检索预算（当前 {budget}）")),
                )
                .child(budget_chips),
        );

        col.into_any_element()
    }

    /// 诊断页：下载器连通、CookieCloud 拉取、各站 cookie 在位、自动发种审计台账
    fn render_diag(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let panel = rgb(0x161b22);
        let dim = rgb(0x8b949e);
        let now = now_unix();
        let audit_text = std::fs::read_to_string(audit_path_of(&self.config)).unwrap_or_default();
        let (posted_today, last_bad) = parse_audit(&audit_text, now);
        let backoff_count = last_bad
            .values()
            .filter(|v| in_backoff(Some(**v), now))
            .count();

        let mut cookie_rows = div().flex().flex_col().gap_1();
        for s in &self.config.sites {
            let has_cookie = self.config.cookie_header_for(s.primary_host()).is_some();
            cookie_rows = cookie_rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(160.)).text_xs().child(s.name.clone()))
                    .child(div().size(px(8.)).rounded_full().bg(rgb(if has_cookie {
                        0x3fb950
                    } else {
                        0xf85149
                    })))
                    .child(div().text_xs().text_color(dim).child(if has_cookie {
                        "cookie 在位"
                    } else {
                        "缺 cookie（拉 CookieCloud 或手工补）"
                    })),
            );
        }

        let mut posted_rows = div().flex().flex_col().gap_1();
        let mut sites_sorted: Vec<(&String, &u32)> = posted_today.iter().collect();
        sites_sorted.sort();
        for (site, count) in sites_sorted {
            posted_rows = posted_rows.child(
                div()
                    .text_xs()
                    .child(format!("{site}：今日已自动发 {count} 条（UTC 日界）")),
            );
        }
        if posted_today.is_empty() {
            posted_rows = posted_rows.child(
                div()
                    .text_xs()
                    .text_color(dim)
                    .child("今日尚无自动发布记录"),
            );
        }

        div()
            .flex()
            .flex_1()
            .min_w(px(0.))
            .flex_col()
            .gap_2()
            .p_3()
            .bg(panel)
            .rounded_lg()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight(600.0))
                    .child("诊断"),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(button(
                        "diag-qb",
                        "测试下载器连接",
                        cx.listener(|this, _, _, cx| {
                            this.start_job(Job::TestQb, cx);
                        }),
                    ))
                    .child(button(
                        "diag-cookiecloud",
                        "拉取 CookieCloud",
                        cx.listener(|this, _, _, cx| {
                            this.start_job(Job::PullCookies, cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(dim)
                    .child("CookieCloud 服务没起时会报错——可先用本地 cookies.json，服务恢复后再拉。拉取成功自动合并并热重载。"),
            )
            .child(div().text_xs().text_color(dim).child("各站 cookie："))
            .child(cookie_rows)
            .child(
                div()
                    .pt_2()
                    .border_t_1()
                    .border_color(rgb(0x30363d))
                    .child(div().text_xs().text_color(dim).child("自动发种台账："))
                    .child(posted_rows)
                    .child(
                        div()
                            .text_xs()
                            .text_color(dim)
                            .child(format!("退避中 {backoff_count} 条（门禁拦截 24h / 失败 6h）")),
                    ),
            )
            .child(div().text_xs().text_color(dim).child(format!(
                "索引：{}｜审计：{}",
                self.config.coverage_state_path().display(),
                audit_path_of(&self.config).display()
            )))
            .into_any_element()
    }

    fn render_drafts(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let panel = rgb(0x161b22);
        let dim = rgb(0x8b949e);
        let mut col = div()
            .flex()
            .flex_1()
            .min_w(px(0.))
            .flex_col()
            .gap_2()
            .p_3()
            .bg(panel)
            .rounded_lg()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight(600.0))
                    .child(format!("转种草稿（{} 条）", self.drafts.len())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(dim)
                    .child("「审阅」= 只审不发：跑源页解析、预检、字段构造，把将提交的字段清单打进日志。核对没问题再点「确认转种」= 人工确认：预检 → 上传 → 发后自检 → 推种 → 校验。预检拦三类问题（MediaInfo 空 / 简介缺 IMDb·豆瓣·TMDB / 中字标签），上传页带 LuckAudit 的站另跑站点官方预审，不通过不会上传。"),
            );
        for (i, draft) in self.drafts.iter().enumerate() {
            let id = format!("draft-{i}");
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .bg(rgb(0x1c2128))
                    .rounded_md()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().child(draft.name.clone()))
                            .child(div().text_xs().text_color(dim).child(format!(
                                "{} → {}｜源站：{}",
                                draft.source_site, draft.target_site, draft.source_url
                            ))),
                    )
                    .child(button(
                        &format!("copy-{id}"),
                        "复制源站链接",
                        cx.listener({
                            let d = draft.clone();
                            move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    d.source_url.clone(),
                                ));
                            }
                        }),
                    ))
                    .child(button(
                        &format!("review-{id}"),
                        "审阅",
                        cx.listener({
                            let d = draft.clone();
                            move |this, _, _, cx| this.start_review(&d, cx)
                        }),
                    ))
                    .child(button_accent(
                        &format!("reseed-{id}"),
                        "确认转种",
                        cx.listener({
                            let d = draft.clone();
                            move |this, _, _, cx| this.start_reseed(&d, cx)
                        }),
                    )),
            );
        }
        if self.drafts.is_empty() {
            col = col.child(
                div()
                    .text_color(dim)
                    .child("暂无草稿。跑一轮后，站内没有的段落在源站定位成功时会出现在这里。"),
            );
        }
        col.into_any_element()
    }

    fn render_log(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .w(px(420.))
            .flex_col()
            .gap_1()
            .p_3()
            .bg(rgb(0x161b22))
            .rounded_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight(600.0))
                            .child("任务日志"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x8b949e))
                            .child(format!("（存 {} 条，显示最新 200）", self.log.len())),
                    )
                    .child(div().flex_1())
                    .child(button(
                        "log-clear",
                        "清空",
                        cx.listener(|this, _, _, cx| {
                            this.log.clear();
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .text_color(rgb(0x8b949e))
                    .children(
                        self.log
                            .iter()
                            .rev()
                            .take(200)
                            .map(|l| div().child(l.clone())),
                    ),
            )
    }
}

/// 固定宽度文本列（数字右对齐）
fn cell_text(width: gpui::Pixels, text: impl Into<SharedString>, right: bool) -> impl IntoElement {
    let mut d = div().flex_none().w(width).text_sm();
    if right {
        d = d.text_right();
    }
    d.child(text.into())
}

fn render_help() -> gpui::AnyElement {
    div()
        .flex()
        .flex_1()
        .flex_col()
        .gap_2()
        .p_4()
        .bg(rgb(0x161b22))
        .rounded_lg()
        .child(div().text_base().font_weight(FontWeight(600.0)).child("转种全流程（Rust 原生）"))
        .child(div().child("1. 「转种草稿」页先点「审阅」——只审不发：把将提交的字段清单打进日志，核对标题/副标题/简介/MediaInfo/链接/标签。"))
        .child(div().child("2. 核对没问题再点「确认转种」——这一次点击就是人工确认，随后全自动："))
        .child(div().child("   ① 预检：MediaInfo 非空 / 简介带 IMDb·豆瓣·TMDB 链接 / 中字标签（检测到中文字幕时）——任何一条阻断都不会上传；上传页带 LuckAudit 的站还会把字段送站点官方预审，官方判未通过同样不上传。"))
        .child(div().child("   ② 上传：拉目标站 upload.php 表单，透传 hidden 字段、填标题/副标题/简介/MediaInfo/条目链接/标签，multipart 提交。"))
        .child(div().child("   ③ 发后自检：回读新种详情页，核对标题/MediaInfo/条目链接真的落上了（漏了会在日志里点名）。"))
        .child(div().child("   ④ 推种：下载目标站的新种子，暂停加入下载器（dlLimit=1 兜底），启动校验。"))
        .child(div().child("   ⑤ 收尾：校验到 100% 解除限速、格子记「已转种」；分片对不上就停掉/删掉；超时记「校验中」下一轮收尾。"))
        .child(div().child("3. 「任务日志」实时显示每一步；失败原因（cookie 失效 / 限流 / 必填项缺失 / 种子已存在）都会写明。"))
        .child(div().child("4. 上传后自动提交 MediaInfo 到独立表单字段（technical_info 等），没有独立字段就内嵌简介，避免「MediaInfo 为空」被打回。"))
        .child(div().text_color(rgb(0x8b949e)).child("说明：v1 表单引擎只做站点无关的通用字段；个别站的特殊下拉（质量档位/两步上传）预检会提示「表单不认识的必填项」，遇到再逐站补 override。"))
        .into_any_element()
}

fn status_chip(status: CoverageStatus, count: usize) -> StatusChip {
    let (label, color) = match status {
        CoverageStatus::Seeding => ("已在做种", 0x3fb950),
        CoverageStatus::Assisted => ("已辅种", 0x58a6ff),
        CoverageStatus::Reseeded => ("已转种", 0xbc8cff),
        CoverageStatus::Verifying => ("校验中", 0xd29922),
        CoverageStatus::ReviewPending => ("待人工确认", 0xf0883e),
        CoverageStatus::OtherVersion => ("其他版本", 0xd29922),
        CoverageStatus::Rejected => ("人工否决", 0x8b949e),
        CoverageStatus::Miss => ("站内没有", 0x484f58),
        CoverageStatus::WaitSource => ("待找源", 0x39c5cf),
        CoverageStatus::Failed => ("失败", 0xf85149),
        CoverageStatus::ReseedDraft => ("转种草稿", 0xdb61a2),
    };
    StatusChip {
        label,
        count,
        color,
    }
}

fn main() {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("boxsend.json"));
    let cfg = Config::load(&config_path).unwrap_or_else(|e| {
        eprintln!("加载配置 {} 失败：{e:#}", config_path.display());
        std::process::exit(1);
    });

    let path_for_window = config_path.clone();
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.), px(760.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                let view = cx.new(|_| BsendGui::new(Arc::new(cfg), path_for_window.clone()));
                view.update(cx, |this, cx| this.start_polling(cx));
                view
            },
        )
        .expect("打开窗口失败");
        cx.activate(true);
    });
}
