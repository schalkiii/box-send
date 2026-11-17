//! bsend 的 GPUI 图形界面（R0/R1 能力的可视化外壳）。
//!
//! 三块内容：
//! - 覆盖概览：内容/成员种子/各站格子与覆盖率；
//! - 转种草稿：源站 + 详情页链接 + 目标站，「复制源站链接并打开 GUI 版」完成人工确认交接；
//! - 后台任务：扫描 / 基线 / 跑一轮，事件走 mpsc，GPUI 定时器轮询后更新视图。
//!
//! 注意：Rust 版转种只到草稿（v1 不上传）；真发种必须人工到 GUI 版「批量转种」页确认。

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use bsend::config::Config;
use bsend::coverage::{now_unix, prefetch_files, run_round, Event, RoundOptions};
use bsend::index::ContentIndex;
use bsend::qbittorrent::QBittorrent;
use bsend::report::{CoverageReport, DraftRow};
use bsend::sites::nexusphp::NexusPhp;
use gpui::{
    div, prelude::*, px, rgb, size, App, Application, AsyncApp, Bounds, ClipboardItem, FontWeight,
    SharedString, Timer, WeakEntity, Window, WindowBounds, WindowOptions,
};

const GUI_EXE: &str = r"C:\Program Files\BoxSend\BoxSend.exe";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Drafts,
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Job {
    Scan,
    Prime,
    Round,
    Refresh,
}

#[derive(Clone)]
struct SiteRow {
    site: String,
    cells: usize,
    covered: usize,
    pending: usize,
    ratio: f64,
}

#[derive(Clone)]
struct Snapshot {
    stats: String,
    sites: Vec<SiteRow>,
    drafts: Vec<DraftRow>,
}

enum GuiEvent {
    Log(String),
    Snapshot(Snapshot),
    Busy(bool),
}

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
        };
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
        if self.log.len() > 200 {
            self.log.remove(0);
        }
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
                Job::Refresh => "刷新",
            }
        ));
        cx.notify();

        let cfg = self.config.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = match job {
                Job::Scan => run_scan(&cfg, &tx),
                Job::Prime => run_round_job(&cfg, Some(0), &tx),
                Job::Round => run_round_job(&cfg, None, &tx),
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
                        }
                    }
                    cx.notify();
                });
            },
        )
        .detach();
    }

    /// 人工确认交接：复制源站链接 + 拉起 GUI 版「批量转种」页
    fn handoff(&mut self, draft: &DraftRow, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(draft.source_url.clone()));
        let mut log = format!(
            "已复制源站链接（{} → {}）：{}；请在 GUI 版「批量转种」页粘贴、勾选目标站并确认",
            draft.source_site, draft.target_site, draft.source_url
        );
        match std::process::Command::new(GUI_EXE).spawn() {
            Ok(_) => log.push_str("（已启动 GUI 版）"),
            Err(e) => log.push_str(&format!("（GUI 版启动失败：{e}；可手动打开 {GUI_EXE}）")),
        }
        self.tab = Tab::Help;
        self.push_log(log);
        cx.notify();
    }
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
                d.child(div().text_sm().text_color(rgb(0xd29922)).child("运行中…"))
            });

        let body = match self.tab {
            Tab::Overview => self.render_overview(cx),
            Tab::Drafts => self.render_drafts(cx),
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
                    .child(self.render_log()),
            )
    }
}

impl BsendGui {
    fn render_overview(&self, _cx: &mut Context<Self>) -> gpui::AnyElement {
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
                    .child("索引概览"),
            )
            .child(div().text_color(dim).child(self.stats.clone()));
        for row in &self.sites {
            let line = format!(
                "{:<12} 格子 {:>5}   已覆盖 {:>5}   未判定 {:>5}   覆盖率 {:>6.1}%",
                row.site,
                row.cells,
                row.covered,
                row.pending,
                row.ratio * 100.0
            );
            col = col.child(div().child(line));
        }
        col.into_any_element()
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
                    .child(format!(
                        "转种草稿（{} 条；本版只出草稿，上传需人工确认）",
                        self.drafts.len()
                    )),
            );
        for (i, draft) in self.drafts.iter().enumerate() {
            let id = format!("draft-{i}");
            let label = format!(
                "{}｜{} → {}｜源站链接：{}",
                draft.name, draft.source_site, draft.target_site, draft.source_url
            );
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .bg(rgb(0x1c2128))
                    .rounded_md()
                    .child(div().flex_1().min_w(px(0.)).child(label))
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
                        &format!("handoff-{id}"),
                        "确认交接 → GUI 版",
                        cx.listener({
                            let d = draft.clone();
                            move |this, _, _, cx| this.handoff(&d, cx)
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

    fn render_log(&self) -> impl IntoElement {
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
                    .text_base()
                    .font_weight(FontWeight(600.0))
                    .child("任务日志"),
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
                            .take(60)
                            .map(|l| div().child(l.clone())),
                    ),
            )
    }
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
        .child(div().text_base().font_weight(FontWeight(600.0)).child("人工确认后走 GUI 版转种"))
        .child(div().child("1. 在「转种草稿」页点「确认交接 → GUI 版」：源站详情页链接会复制到剪贴板，并尝试启动 BoxSend GUI 版。"))
        .child(div().child("2. GUI 版进入「批量转种」页，把链接粘贴到「种子详情页链接」输入框；源站一般自动识别，不对就手动指定。"))
        .child(div().child("3. 在「转种分组」里勾选目标站（草稿里写的那个站），确认「转种到目标站」已勾选。"))
        .child(div().child("4. 点「开始运行」——上传、发布、推种都由 GUI 版完成，本 Rust 版不代你发种。"))
        .child(div().text_color(rgb(0x8b949e)).child("说明：Rust 版转种臂按方案只到草稿；源站链接/判定级别/目标站都在草稿里，避免误发。"))
        .into_any_element()
}

fn snapshot(cfg: &Config) -> Result<Snapshot> {
    let index = ContentIndex::load(&cfg.coverage_state_path());
    let path = cfg.coverage_state_path().display().to_string();
    let report = CoverageReport::build(&index, &cfg.sites, 200, now_unix(), &path);
    let members: usize = index.contents.values().map(|e| e.torrents.len()).sum();
    let stats = format!(
        "{} 段内容、{} 条成员种子、{} 条草稿；索引 {}",
        report.totals.contents, members, report.totals.drafts, path
    );
    let sites = report
        .sites
        .into_iter()
        .map(|s| SiteRow {
            site: s.site_id,
            cells: s.cells,
            covered: s.covered,
            pending: s.pending,
            ratio: s.covered_ratio,
        })
        .collect();
    Ok(Snapshot {
        stats,
        sites,
        drafts: report.drafts,
    })
}

fn run_scan(cfg: &Config, tx: &Sender<GuiEvent>) -> Result<()> {
    let qb = Arc::new(QBittorrent::new(
        &cfg.downloader.url,
        &cfg.downloader.username,
        &cfg.downloader.password,
    )?);
    let hello = qb.test_connection().context("连不上下载器")?;
    let _ = tx.send(GuiEvent::Log(hello));
    let torrents = qb.list_torrents()?;
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let files = prefetch_files(
        qb.as_ref(),
        &torrents,
        &index,
        cfg.coverage.scan_concurrency,
        |done, total| {
            if done % 5000 == 0 || done == total {
                let _ = tx.send(GuiEvent::Log(format!("文件清单 {done}/{total}")));
            }
        },
    );
    let delta = index.merge_scan(
        &torrents,
        |t| files.get(&t.hash).cloned(),
        now_unix(),
        cfg.coverage.archive_days,
    );
    index.save()?;
    let _ = tx.send(GuiEvent::Log(format!(
        "扫描 {} 条种子：新内容 {}、并入 {}、移除 {}",
        torrents.len(),
        delta.new_contents,
        delta.merged,
        delta.removed_torrents
    )));
    let _ = tx.send(GuiEvent::Snapshot(snapshot(cfg)?));
    Ok(())
}

fn run_round_job(cfg: &Config, limit: Option<usize>, tx: &Sender<GuiEvent>) -> Result<()> {
    let qb = Arc::new(QBittorrent::new(
        &cfg.downloader.url,
        &cfg.downloader.username,
        &cfg.downloader.password,
    )?);
    let _ = tx.send(GuiEvent::Log(qb.test_connection().context("连不上下载器")?));

    let mut adapters: Vec<Arc<NexusPhp>> = Vec::new();
    for s in &cfg.sites {
        let cookie = cfg.cookie_header_for(s.primary_host());
        let mut adapter = NexusPhp::new(
            &s.id,
            &s.name,
            &s.url,
            s.search_url.as_deref(),
            cookie.as_deref(),
        )
        .with_context(|| format!("站点配置不合法：{}", s.id))?;
        adapter.user_agent = cfg.user_agent.clone();
        adapters.push(Arc::new(
            adapter.with_announce_hosts(s.announce_hosts.clone()),
        ));
    }

    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let report = run_round(
        cfg,
        qb,
        &adapters,
        &mut index,
        |ev| {
            let line = match ev {
                Event::Log(m) => m,
                Event::ItemSkipped {
                    site_id,
                    key,
                    reason,
                } => {
                    format!("[{site_id}] {key} 跳过：{reason}")
                }
                Event::ItemDone {
                    site_id,
                    status,
                    note,
                    ..
                } => {
                    format!("[{site_id}] {} {note}", status.label())
                }
            };
            let _ = tx.send(GuiEvent::Log(line));
        },
        RoundOptions {
            limit,
            ..Default::default()
        },
    )?;
    let _ = tx.send(GuiEvent::Log(format!(
        "本轮：{:?}；检索预算 {}/{}",
        report.by_status,
        report.budget.get("search").map(|v| v.0).unwrap_or(0),
        report.budget.get("search").map(|v| v.1).unwrap_or(0)
    )));
    let _ = tx.send(GuiEvent::Snapshot(snapshot(cfg)?));
    Ok(())
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
