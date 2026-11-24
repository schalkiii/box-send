//! bsend 的 GPUI 图形界面（全功能 Rust 原生，不再交接 GUI 版）。
//!
//! 四块内容：
//! - 覆盖概览：内容/成员种子/各站格子与覆盖率，带状态图例与「跑一轮」旋钮；
//! - 转种草稿：源站 + 详情页链接 + 目标站，「审阅」只审不发（字段清单进日志），
//!   「确认转种」原生走 预检 → 上传 → 发后自检 → 推种 → 校验一条龙
//!   （LuckAudit 三查在预检里拦）；
//! - 自动发种：把草稿批量真发布（门禁 + 日限额 + 失败冷却 + 审计），可试运行；
//! - 配置：开关/站点启用/白名单/预算直接写回 boxsend.json（备份 + 热重载）；
//! - 诊断：下载器连通、CookieCloud 拉取、各站 cookie 在位、发种台账与失败冷却；
//! - 后台任务：扫描 / 基线 / 跑一轮 / 转种 / 自动发种，事件走 mpsc，GPUI 定时器轮询后更新视图。

mod config_io;
mod jobs;
// 设计令牌是完整的一套刻度，未用到的档位保留备用
#[allow(dead_code)]
mod theme;
mod types;
mod views;
mod widgets;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use bsend::autopost::AutopostOptions;
use bsend::config::Config;
use bsend::coverage::RoundOptions;
use bsend::index::{ContentIndex, CoverageStatus};
use bsend::qbittorrent::QBittorrent;
use bsend::report::DraftRow;
use bsend::reseed::{request_from_draft, review_reseed, run_reseed};
use gpui::{
    div, prelude::*, px, rgb, size, App, Application, AsyncApp, Bounds, FontWeight, SharedString,
    Timer, WeakEntity, Window, WindowBounds, WindowOptions,
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
    /// 右侧日志面板是否展开（收起后主视图占满宽度）
    log_open: bool,
    /// 概览筛选：0 全部 / 1 只看有缺口 / 2 只看已满
    overview_filter: usize,
    /// 概览排序：0 未判定降序 / 1 站点名 / 2 覆盖率升序
    overview_sort: usize,
    /// 草稿筛选：0 全部 / 1 只看待处理
    drafts_filter: usize,
    /// 配置筛选：0 全部 / 1 只看已启用
    config_filter: usize,
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
            log_open: true,
            overview_filter: 0,
            overview_sort: 0,
            drafts_filter: 0,
            config_filter: 0,
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
        let result: Result<()> = config_io::save_config_with_backup(&self.config_path, mutate);
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
            ..Default::default()
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

impl Render for BsendGui {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.render_header();
        let toolbar = self.render_toolbar(cx);
        let body = match self.tab {
            Tab::Overview => self.render_overview(cx),
            Tab::Drafts => self.render_drafts(cx),
            Tab::Autopost => self.render_autopost(cx),
            Tab::Config => self.render_config(cx),
            Tab::Diag => self.render_diag(cx),
            Tab::Help => views::help::render_help(),
        };
        let log_panel = self.render_log(cx);

        // 三段式外壳：固定头部 / 固定工具栏 / 可伸缩主体 / 固定状态栏。
        // 只有主体允许收缩（min_h(0)），所以窗口再小也不会把头部和工具栏压成重叠。
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme::BG))
            .text_color(rgb(theme::TEXT))
            .text_sm()
            .child(header)
            .child(toolbar)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .gap_2()
                    .px_3()
                    .pb_2()
                    .child(body)
                    .child(log_panel),
            )
            .child(self.render_status_bar())
    }
}

impl BsendGui {
    /// 头部：产品名 + 一句话定位 + 索引摘要（截断）+ 运行中徽标
    fn render_header(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_3()
            .px_4()
            .h(theme::HEADER_H)
            .bg(rgb(theme::BG))
            .child(
                div()
                    .flex_none()
                    .text_xl()
                    .font_weight(FontWeight(600.0))
                    .text_color(rgb(theme::ACCENT))
                    .child("BoxSend"),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(theme::TEXT_FAINT))
                    .child("PT 辅种引擎"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_xs()
                    .text_color(rgb(theme::TEXT_FAINT))
                    .truncate()
                    .child(self.stats.clone()),
            )
            .when(self.busy, |d| {
                d.child(div().flex_none().child(widgets::badge(
                    format!("运行中 · 已处理 {} 格", self.cells_done),
                    theme::WARN,
                )))
            })
    }

    /// 工具栏：左侧 tab（不能被压缩），右侧常驻任务按钮
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs: [(&str, &str, Tab); 6] = [
            ("tab-overview", "概览", Tab::Overview),
            ("tab-drafts", "转种草稿", Tab::Drafts),
            ("tab-autopost", "自动发种", Tab::Autopost),
            ("tab-config", "配置", Tab::Config),
            ("tab-diag", "诊断", Tab::Diag),
            ("tab-help", "说明", Tab::Help),
        ];
        let mut strip = div().flex().flex_none().items_center().gap_1();
        for (id, label, target) in tabs {
            strip = strip.child(widgets::tab(
                id,
                label,
                self.tab == target,
                cx.listener(move |this: &mut Self, _, _, cx| {
                    this.tab = target;
                    cx.notify();
                }),
            ));
        }

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .h(theme::TOOLBAR_H)
            .bg(rgb(theme::SURFACE))
            .border_b_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .child(strip)
            .child(div().flex_1().min_w(px(0.)))
            .child(widgets::button(
                "job-refresh",
                "刷新",
                cx.listener(|this, _, _, cx| this.start_job(Job::Refresh, cx)),
            ))
            .child(widgets::button(
                "job-scan",
                "扫描",
                cx.listener(|this, _, _, cx| this.start_job(Job::Scan, cx)),
            ))
            .child(widgets::button(
                "job-prime",
                "基线",
                cx.listener(|this, _, _, cx| this.start_job(Job::Prime, cx)),
            ))
            .child(widgets::button_accent(
                "job-round",
                "跑一轮",
                cx.listener(|this, _, _, cx| this.start_job(Job::Round, cx)),
            ))
    }

    /// 底部状态栏：运行状态 / 统计 / 最近一条日志 / 配置路径（固定高度，永远可见）
    fn render_status_bar(&self) -> impl IntoElement {
        let last = self
            .log
            .last()
            .cloned()
            .unwrap_or_else(|| "尚无日志".into());
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_3()
            .px_3()
            .h(theme::STATUS_H)
            .bg(rgb(theme::SURFACE))
            .border_t_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .text_xs()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().size(px(7.)).rounded_full().bg(rgb(if self.busy {
                        theme::WARN
                    } else {
                        theme::SUCCESS
                    })))
                    .child(div().text_color(rgb(theme::TEXT_DIM)).child(if self.busy {
                        format!("运行中 · 已处理 {} 格", self.cells_done)
                    } else {
                        "空闲".to_string()
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(theme::TEXT_FAINT))
                    .child(format!(
                        "站点 {} · 草稿 {} · 日志 {}",
                        self.sites.len(),
                        self.drafts.len(),
                        self.log.len()
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_color(rgb(theme::TEXT_FAINT))
                    .truncate()
                    .child(last),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(360.))
                    .text_right()
                    .text_color(rgb(theme::TEXT_FAINT))
                    .truncate()
                    // 只显示尾部两段，避免长路径被截成 "C:\Users\Schal\AppData\Local\BoxSe"
                    .child(short_path(&self.config_path)),
            )
    }
}

/// 路径尾部两段（"…\BoxSend\boxsend.json"），状态栏用，避免长路径被截成半截
fn short_path(path: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .rev()
        .take(2)
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let tail = parts.into_iter().rev().collect::<Vec<_>>().join("\\");
    format!("…\\{tail}")
}

fn status_chip(status: CoverageStatus, count: usize) -> StatusChip {
    let (label, color) = match status {
        CoverageStatus::Seeding => ("已在做种", 0x3fb950),
        CoverageStatus::Assisted => ("已辅种", 0x58a6ff),
        CoverageStatus::Reseeded => ("已转种", 0xbc8cff),
        CoverageStatus::Verifying => ("校验中", 0xd29922),
        CoverageStatus::ReviewPending => ("待人工确认", 0xf0883e),
        CoverageStatus::OtherVersion => ("其他版本", 0xd29922),
        CoverageStatus::SameHashSkip => ("同 hash 跳过", 0x8b949e),
        CoverageStatus::Rejected => ("人工否决", 0x8b949e),
        CoverageStatus::Miss => ("站内没有", 0x484f58),
        CoverageStatus::WaitSource => ("待找源", 0x39c5cf),
        CoverageStatus::Failed => ("失败", 0xf85149),
        CoverageStatus::ReseedDraft => ("转种草稿", 0xdb61a2),
        // 兜底：引擎侧新增状态（如内容黑名单跳过）不让图例崩，统一按「已跳过」灰展示
        _ => ("已跳过", 0x6e7681),
    };
    StatusChip {
        label,
        count,
        color,
    }
}

fn main() {
    let config_path = resolve_config_path(std::env::args().nth(1).map(PathBuf::from));
    let cfg = Config::load(&config_path).unwrap_or_else(|e| {
        show_error(&format!(
            "加载配置失败：\n{}\n\n{e:#}\n\n可用命令行参数指定配置文件路径。",
            config_path.display()
        ));
        std::process::exit(1);
    });

    let path_for_window = config_path.clone();
    Application::new().run(move |cx: &mut App| {
        // 默认 1280×820（147 个站点的表格需要高度），最小 1060×720：
        // 低于这个尺寸连表头都放不下，直接由窗口管理器挡住，避免出现字体重叠。
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(theme::MIN_WIN_W, theme::MIN_WIN_H)),
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

/// 配置文件定位：命令行参数优先；无参数时先找当前目录，再找系统默认
/// `%LOCALAPPDATA%\BoxSend\boxsend.json`。双击 exe 时当前目录是 target\debug，
/// 只有回退到系统默认位置才能正常启动（否则会静默退出，看起来像「双击没反应」）。
fn resolve_config_path(arg: Option<PathBuf>) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    let local = std::env::var_os("LOCALAPPDATA")
        .map(|v| PathBuf::from(v).join("BoxSend").join("boxsend.json"));
    pick_config_path(arg, &cwd, local.as_deref())
}

fn pick_config_path(arg: Option<PathBuf>, cwd: &Path, local: Option<&Path>) -> PathBuf {
    if let Some(a) = arg {
        return a;
    }
    let cwd_cfg = cwd.join("boxsend.json");
    if cwd_cfg.exists() {
        return cwd_cfg;
    }
    if let Some(l) = local {
        return l.to_path_buf();
    }
    cwd_cfg
}

/// 加载失败时给用户一个可见的错误框（GUI 子系统没有控制台，eprintln 看不到）
#[cfg(windows)]
fn show_error(message: &str) {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(
            hwnd: *mut core::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            u_type: u32,
        ) -> i32;
    }
    let wide = |s: &str| {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<u16>>()
    };
    let text = wide(message);
    let caption = wide("BoxSend");
    // MB_OK | MB_ICONERROR = 0x00000010
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), 0x10);
    }
}

#[cfg(not(windows))]
fn show_error(message: &str) {
    eprintln!("{message}");
}

#[cfg(test)]
mod tests {
    use super::pick_config_path;
    use std::path::PathBuf;

    #[test]
    fn config_path_prefers_arg_then_cwd_then_local_appdata() {
        let dir = std::env::temp_dir().join(format!("bsend-gui-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cwd_cfg = dir.join("boxsend.json");
        let local = dir.join("local").join("boxsend.json");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();

        // 命令行参数优先
        let arg = PathBuf::from("custom.json");
        assert_eq!(pick_config_path(Some(arg.clone()), &dir, Some(&local)), arg);
        // 当前目录有配置就用当前目录
        std::fs::write(&cwd_cfg, "{}").unwrap();
        assert_eq!(pick_config_path(None, &dir, Some(&local)), cwd_cfg);
        // 当前目录没有则回退到 %LOCALAPPDATA%
        std::fs::remove_file(&cwd_cfg).unwrap();
        assert_eq!(pick_config_path(None, &dir, Some(&local)), local);
    }
}
