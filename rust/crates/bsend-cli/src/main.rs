//! bsend：box-send 的辅种引擎 CLI（Rust 版）。
//! 与 GUI 版共用同一份 `boxsend.json` / `cookies.json`，索引落在 dataDir/coverage.json。
//!
//! 用法：
//! ```text
//! bsend scan                     # 扫描下载器，建立/刷新内容索引（不动站点）
//! bsend plan --limit 5           # 预演一轮：检索照跑、推送只出结论、不落盘
//! bsend run --sites hdsky,pter   # 跑一整轮（辅种真推；转种只出草稿）
//! bsend reseed --draft 1         # 审阅第 1 条转种草稿（只审不发）
//! bsend reseed --draft 1 --yes   # 真提交：预检 → 上传 → 发后自检 → 推种校验
//! bsend status                   # 看索引现状
//! ```

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use bsend::config::Config;
use bsend::cookiecloud::CookieCloud;
use bsend::coverage::{prefetch_files, run_round, Event, RoundOptions, RoundReport};
use bsend::index::{ContentIndex, CoverageStatus};
use bsend::qbittorrent::QBittorrent;
use bsend::report::CoverageReport;
use bsend::sites::nexusphp::NexusPhp;

#[derive(Parser)]
#[command(
    name = "bsend",
    version,
    about = "辅种引擎：内容索引 → 源种定位 → 辅种/转种（box-send Rust 版）"
)]
struct Cli {
    /// boxsend.json 路径（与 GUI 版共用同一份配置）
    #[arg(long, default_value = "boxsend.json")]
    config: PathBuf,
    /// 本轮从 CookieCloud 拉取 cookie（覆盖 cookies.json 里同 host 的条目，不写盘）
    #[arg(long, global = true)]
    cookiecloud: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 扫描下载器，建立/刷新内容索引（不动站点）
    Scan,
    /// 预演一轮：检索照跑，推送只出「可辅」结论，不落盘不真加
    Plan {
        /// 最多处理多少格
        #[arg(long)]
        limit: Option<usize>,
        /// 只跑这些站（逗号分隔的站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
        /// 只处理这段内容（contentKey 前缀或名称包含；调试/单条重试用）
        #[arg(long)]
        only: Option<String>,
    },
    /// 跑一整轮辅种：辅种真推下载器，转种出草稿清单
    Run {
        /// 最多处理多少格
        #[arg(long)]
        limit: Option<usize>,
        /// 只跑这些站（逗号分隔的站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
        /// 不真加下载器（与 plan 的区别：run --dry-run 会把结论落盘）
        #[arg(long)]
        dry_run: bool,
        /// 只处理这段内容（contentKey 前缀或名称包含；调试/单条重试用）
        #[arg(long)]
        only: Option<String>,
    },
    /// 只做基线：把「已在做种」的格子标好并落盘，零站点请求（先看清家底）
    Prime,
    /// 看 cookie 现状：逐站条目数、最早过期、有没有鉴权字段（不打印 cookie 值）
    Cookies {
        /// 数据来源：json（读 cookies.json）或 cookiecloud（现场拉取解密）
        #[arg(long, default_value = "json")]
        from: String,
        /// 把结果写回 cookies.json（GUI 读同一份；先备份成 cookies.json.bak）
        #[arg(long)]
        write: bool,
        /// 列出全部域名（默认只看参与辅种的站点）
        #[arg(long)]
        all: bool,
    },
    /// 看索引现状：内容数、覆盖分布、转种草稿、待找源
    Status,
    /// 转种草稿的审阅与提交（默认只审阅不提交）
    Reseed {
        /// 转第 N 条草稿（1 起编号；不带此参数只列草稿）
        #[arg(long)]
        draft: Option<usize>,
        /// 真提交（预检 → 上传 → 发后自检 → 推种校验）；不带则只审阅
        #[arg(long)]
        yes: bool,
    },
    /// 覆盖报告：逐站覆盖率、未覆盖最多清单、草稿与待找源（可导出 JSON）
    Report {
        /// 未覆盖清单显示多少条
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// 导出 JSON（camelCase，原子写；给 GUI / daemon 吃）
        #[arg(long)]
        json: Option<PathBuf>,
        /// 只看这些站（逗号分隔的站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
    },
    /// 发种表格演练（只审不发）：逐站拉 upload.php 构造字段核对完备性，绝不 POST
    Formcheck {
        /// 只查这些站（逗号分隔的站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
    },
    /// 全站门禁演练（只读）：构造上传字段 + 官方预审 + 门禁矩阵，绝不上传种子
    Gatecheck {
        /// 只演练这些站（逗号分隔站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 自动发种：草稿 → 审阅 → 门禁 → 发布（白名单 + 日限额 + 审计）；--dry-run 只跑门禁
    Autopost {
        /// 只跑门禁与审阅，不发布
        #[arg(long)]
        dry_run: bool,
        /// 本次最多发布多少条
        #[arg(long)]
        limit: Option<usize>,
        /// 只处理这些目标站的草稿（逗号分隔站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
    },

    /// 站点档案汇总：配置开关 + 最近一次实测结论 + pieces 支持 + cookie 概况
    Sites {
        /// 输出 JSON（给脚本/GUI 吃）
        #[arg(long)]
        json: bool,
        /// 输出 Markdown 表格（写文档用）
        #[arg(long)]
        md: bool,
    },

    /// 落盘核对（只读）：抽若干内容，用下载器实时数据核对「推种时会用哪个 savepath」
    Layoutcheck {
        /// 最多看多少段内容
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// 只看这段内容（contentKey 前缀或名称包含）
        #[arg(long)]
        only: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut cfg = Config::load(&cli.config)
        .with_context(|| format!("加载配置 {} 失败", cli.config.display()))?;
    if cli.cookiecloud {
        // 无人值守时 CookieCloud 服务可能没起：降级用本地 cookies.json，不让整轮挂掉。
        // 只打根因（不打含访问键的完整 URL）。
        if let Err(e) = apply_cookiecloud(&mut cfg) {
            eprintln!(
                "警告：CookieCloud 拉取失败，改用本地 cookies.json（{}）",
                e.root_cause()
            );
        }
    }
    match cli.cmd {
        Cmd::Scan => cmd_scan(&cfg),
        Cmd::Plan { limit, sites, only } => cmd_round(&cfg, limit, sites, only, true, false),
        Cmd::Run {
            limit,
            sites,
            dry_run,
            only,
        } => cmd_round(&cfg, limit, sites, only, dry_run, true),
        Cmd::Prime => cmd_round(&cfg, Some(0), None, None, false, true),
        Cmd::Cookies { from, write, all } => cmd_cookies(&mut cfg, &from, write, all),
        Cmd::Status => cmd_status(&cfg),
        Cmd::Reseed { draft, yes } => cmd_reseed(&cfg, draft, yes),
        Cmd::Report { top, json, sites } => cmd_report(&cfg, top, json, sites),
        Cmd::Formcheck { sites } => cmd_formcheck(&cfg, sites),
        Cmd::Layoutcheck { limit, only } => cmd_layoutcheck(&cfg, limit, only),
        Cmd::Sites { json, md } => cmd_sites(&cfg, json, md),
        Cmd::Gatecheck { sites, json } => cmd_gatecheck(&cfg, sites, json),
        Cmd::Autopost {
            dry_run,
            limit,
            sites,
        } => cmd_autopost(&cfg, dry_run, limit, sites),
    }
}

fn connect_qb(cfg: &Config) -> Result<Arc<QBittorrent>> {
    let qb = QBittorrent::new(
        &cfg.downloader.url,
        &cfg.downloader.username,
        &cfg.downloader.password,
    )?;
    let hello = qb.test_connection().with_context(|| {
        "连不上下载器（boxsend.json 的 downloader/assistDownloaderID 指的那台）"
    })?;
    println!("{hello}");
    Ok(Arc::new(qb))
}

fn build_adapters(cfg: &Config, filter: &Option<Vec<String>>) -> Result<Vec<Arc<NexusPhp>>> {
    bsend::sites::build_adapters(cfg, filter.as_deref(), |missing| {
        eprintln!(
            "注意：这些站没有 cookie，本轮到它们只会得到 failed：{}（可用 --cookiecloud 或 bsend cookies --from cookiecloud --write 补）",
            missing.join("、")
        );
    })
}

fn cmd_scan(cfg: &Config) -> Result<()> {
    let qb = connect_qb(cfg)?;
    let started = std::time::Instant::now();
    let torrents = qb.list_torrents()?;
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let files = prefetch_files(
        qb.as_ref(),
        &torrents,
        &index,
        cfg.coverage.scan_concurrency,
        |done, total| {
            if total >= 2000 && (done % 2000 == 0 || done == total) {
                println!(
                    "  读取文件清单 {done}/{total}（{}s）",
                    started.elapsed().as_secs()
                );
            }
        },
    );
    let files_of = |t: &bsend::models::LocalTorrent| files.get(&t.hash).cloned();
    let now = bsend::coverage::now_unix();
    let delta = index.merge_scan(&torrents, files_of, now, cfg.coverage.archive_days);
    index.save()?;
    println!(
        "扫描 {} 条种子：新内容 {}、并入 {}、移除 {}、归档 {}、跳过未完成 {}（耗时 {}s）",
        torrents.len(),
        delta.new_contents,
        delta.merged,
        delta.removed_torrents,
        delta.purged_contents,
        delta.skipped_incomplete,
        started.elapsed().as_secs()
    );
    let size = std::fs::metadata(cfg.coverage_state_path())
        .map(|m| m.len())
        .unwrap_or(0);
    println!(
        "索引：{} 段内容、{} 条成员种子、{} MB → {}",
        index.contents.len(),
        index
            .contents
            .values()
            .map(|e| e.torrents.len())
            .sum::<usize>(),
        size / 1_048_576,
        cfg.coverage_state_path().display()
    );
    Ok(())
}

fn cmd_round(
    cfg: &Config,
    limit: Option<usize>,
    sites: Option<Vec<String>>,
    only: Option<String>,
    dry_run: bool,
    persist: bool,
) -> Result<()> {
    let qb = connect_qb(cfg)?;
    // 目标站过滤不能连带过滤源站：qB tracker 指向的源站必须在 adapters 里，
    // 否则 resolver 只能查 --sites 内的一两个源站（wait_source 的根因之一）。
    let adapters = build_adapters(cfg, &None)?;
    println!(
        "参与站点：源站适配器 {} 个（全量保留供源定位）；本轮目标：{}",
        adapters.len(),
        sites
            .as_ref()
            .map(|v| v.join("、"))
            .unwrap_or_else(|| "全部启用站".to_string())
    );
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let opts = RoundOptions {
        limit,
        site_filter: sites.map(|v| v.into_iter().collect()),
        dry_run,
        persist,
        content_filter: only,
    };
    let report = run_round(cfg, qb, &adapters, &mut index, print_event, opts)?;
    print_report(&report);
    if persist {
        println!("索引已更新：{}", cfg.coverage_state_path().display());
    } else {
        println!("（预演模式：结论不落盘）");
    }
    Ok(())
}

fn print_event(ev: Event) {
    match ev {
        Event::Log(m) => println!("{m}"),
        Event::ItemSkipped {
            site_id,
            key,
            reason,
        } => {
            println!("[{site_id}] 跳过 {}（{reason}）", short_key(&key));
        }
        Event::ItemDone {
            site_id,
            key,
            status,
            url,
            note,
            ..
        } => {
            let url_text = url.map(|u| format!(" → {u}")).unwrap_or_default();
            println!(
                "[{site_id}] {} {}（{}）{}",
                status.label(),
                short_key(&key),
                note,
                url_text
            );
        }
    }
}

fn short_key(key: &str) -> &str {
    // contentKey 是 m:<16hex> / h:<40hex>，界面只露尾巴
    if key.len() > 10 {
        &key[..10.min(key.len())]
    } else {
        key
    }
}

fn print_report(r: &RoundReport) {
    println!("\n==== 本轮汇总 ====");
    if !r.by_status.is_empty() {
        for (st, n) in &r.by_status {
            println!("  {} {}", st.label(), n);
        }
    }
    if r.preskipped > 0 {
        println!("  预筛跳过 {}", r.preskipped);
    }
    if r.skipped_no_budget > 0 {
        println!("  预算没排上（下轮再来） {}", r.skipped_no_budget);
    }
    for (kind, (used, max)) in &r.budget {
        println!("  预算 {kind}：{used}/{max}");
    }
    if !r.reseed_drafts.is_empty() {
        println!(
            "\n转种草稿（{} 条，不自动上传；`bsend reseed --draft N` 审阅，GUI「转种草稿」页或加 --yes 执行）：",
            r.reseed_drafts.len()
        );
        for d in &r.reseed_drafts {
            println!(
                "  「{}」源种 {} → 目标 {}",
                d.name, d.source_site, d.target_site
            );
        }
    }
}

fn cmd_status(cfg: &Config) -> Result<()> {
    let index = ContentIndex::load(&cfg.coverage_state_path());
    let mut by_status: BTreeMap<CoverageStatus, usize> = BTreeMap::new();
    let mut member_torrents = 0usize;
    let mut gone = 0usize;
    let mut resolved_sources = 0usize;
    let mut wait_source = Vec::new();
    let mut drafts = Vec::new();
    let mut pending_cells = 0usize;
    for e in index.contents.values() {
        member_torrents += e.torrents.len();
        if e.torrents.is_empty() {
            gone += 1;
        }
        if e.source.as_ref().is_some_and(|s| s.is_resolved()) {
            resolved_sources += 1;
        }
        for (site, cell) in &e.coverage {
            *by_status.entry(cell.status).or_insert(0) += 1;
            match cell.status {
                CoverageStatus::ReseedDraft => {
                    drafts.push(format!("「{}」→ {}", e.name, site));
                }
                CoverageStatus::WaitSource => {
                    wait_source.push(e.name.clone());
                }
                st if !st.is_settled() => pending_cells += 1,
                _ => {}
            }
        }
    }
    println!(
        "索引（{}）：{} 段内容、{} 条成员种子",
        cfg.coverage_state_path().display(),
        index.contents.len(),
        member_torrents
    );
    if gone > 0 {
        println!("  归档倒计时中（下载器里已消失）：{gone}");
    }
    println!("  源种已定位：{resolved_sources}");
    if by_status.is_empty() {
        println!("还没有任何覆盖结论（先跑 bsend scan / bsend run）");
        return Ok(());
    }
    println!("覆盖分布：");
    for (st, n) in &by_status {
        println!("  {} {}", st.label(), n);
    }
    if pending_cells > 0 {
        println!("待办格子：{pending_cells}（下一轮会接着处理）");
    }
    if !drafts.is_empty() {
        println!("\n转种草稿（{}）：", drafts.len());
        for d in drafts.iter().take(20) {
            println!("  {d}");
        }
        if drafts.len() > 20 {
            println!("  … 共 {} 条", drafts.len());
        }
    }
    if !wait_source.is_empty() {
        println!("\n待找源（{}）：", wait_source.len());
        for n in wait_source.iter().take(20) {
            println!("  {n}");
        }
        if wait_source.len() > 20 {
            println!("  … 共 {} 条", wait_source.len());
        }
    }
    Ok(())
}

/// 转种草稿的审阅与提交：`--draft N` 审阅第 N 条（不 POST），加 `--yes` 才真发
fn cmd_reseed(cfg: &Config, draft: Option<usize>, yes: bool) -> Result<()> {
    if yes {
        bsend::config::ensure_publish_enabled(cfg)?;
    }
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let path_text = cfg.coverage_state_path().display().to_string();
    let report = CoverageReport::build(
        &index,
        &cfg.sites,
        20,
        bsend::coverage::now_unix(),
        &path_text,
    );
    if report.drafts.is_empty() {
        println!("没有转种草稿。先 `bsend run` 跑一轮，站里没有的段落定位到源种后会出草稿。");
        return Ok(());
    }
    let Some(n) = draft else {
        println!(
            "转种草稿（{} 条）。`bsend reseed --draft N` 审阅第 N 条，加 `--yes` 真提交：",
            report.drafts.len()
        );
        for (i, d) in report.drafts.iter().enumerate() {
            println!(
                "  #{} 「{}」{} → {}（源：{}）",
                i + 1,
                d.name,
                d.source_site,
                d.target_site,
                d.source_url
            );
        }
        return Ok(());
    };
    let d = report
        .drafts
        .get(n.checked_sub(1).unwrap_or(usize::MAX))
        .ok_or_else(|| anyhow::anyhow!("没有第 {n} 条草稿（共 {} 条）", report.drafts.len()))?;
    let req = bsend::reseed::request_from_draft(
        &d.content_key,
        &d.name,
        &d.source_site,
        &d.source_url,
        &d.target_site,
    );
    let mut log = |line: &str| println!("  {line}");
    if !yes {
        let rep = bsend::reseed::review_reseed(cfg, &req, &mut log)?;
        println!();
        println!("{}", rep.render(200));
        println!("以上为审阅（未提交）。确认无误后加 `--yes` 真提交。");
        return Ok(());
    }
    let qb = connect_qb(cfg)?;
    let url = bsend::reseed::run_reseed(cfg, &qb, &req, &mut index, &mut log, None)?;
    if url.is_empty() {
        println!("转种结束（站点已有该种子，格子记为已覆盖）");
    } else {
        println!("转种完成：{url}");
    }
    Ok(())
}

/// 发种表格演练（只审不发）：逐站拉 upload.php 跑与真实转种同一套字段构造，
/// 核对表单完备性与选档映射，绝不 POST。
fn cmd_formcheck(cfg: &Config, sites: Option<Vec<String>>) -> Result<()> {
    let adapters = build_adapters(cfg, &sites)?;
    let info = bsend::reseed::drill_release();
    println!(
        "发种表格演练（只审不发，绝不 POST）｜样本「{}」｜共 {} 站\n",
        info.name,
        adapters.len()
    );
    let mut ok_n = 0usize;
    let mut gap_n = 0usize;
    let mut fail_n = 0usize;
    let mut facts: Vec<serde_json::Value> = Vec::new();
    for a in &adapters {
        let d = bsend::reseed::drill_upload_form(a);
        let core = d.core_gaps();
        let soft = d.soft_gaps();
        let icon = if d.error.is_some() {
            fail_n += 1;
            "✗"
        } else if core.is_empty() {
            ok_n += 1;
            "✅"
        } else {
            gap_n += 1;
            "⚠ "
        };
        let picks: Vec<String> = d
            .select_picks
            .iter()
            .map(|(f, l)| {
                let k = match f.as_str() {
                    "type" => "分类",
                    s if s.starts_with("source_sel") => "来源",
                    s if s.starts_with("medium_sel") => "媒介",
                    s if s.starts_with("codec_sel") => "编码",
                    s if s.starts_with("standard_sel") => "分辨率",
                    s if s.starts_with("audiocodec_sel") => "音频",
                    s if s.starts_with("processing_sel") => "处理",
                    s if s.starts_with("team_sel") => "制作组",
                    _ => "其他",
                };
                format!("{k}={l}")
            })
            .collect();
        let media = d.mediainfo_field.as_deref().unwrap_or("缺");
        let tags = if d.picked_tags.is_empty() {
            "—".to_string()
        } else {
            d.picked_tags.join("、")
        };
        let picks_text = if picks.is_empty() {
            "—".to_string()
        } else {
            picks.join(" ")
        };
        println!(
            "{icon} {:<16} 字段{:<3} 下拉{:<2} MediaInfo:{:<6} 标签:{:<4} 选档：{picks_text}",
            d.site_id, d.field_count, d.select_count, media, tags,
        );
        for g in core.iter().chain(soft.iter()) {
            println!("     · {g}");
        }
        facts.push(serde_json::json!({
            "site": d.site_id,
            "verdict": if d.error.is_some() { "fail" } else if core.is_empty() { "ok" } else { "gap" },
            "fields": d.field_count,
            "selects": d.select_count,
            "core": &core,
            "soft": &soft,
            "error": d.error,
            "at": bsend::coverage::now_unix(),
        }));
    }
    println!(
        "\n汇总：{} 站｜表单齐 {}｜有缺口 {}｜不可发种/失败 {}（权限/登录/限流）",
        adapters.len(),
        ok_n,
        gap_n,
        fail_n
    );
    let path = cfg
        .coverage_state_path()
        .parent()
        .map(|p| p.join("formcheck.json"))
        .unwrap_or_else(|| PathBuf::from("formcheck.json"));
    // 按站点合并历史结论：单站/小批实测只覆盖本次跑到的站，
    // 其余站保留最近一次结论（sites 总账按站点取最近，整体覆盖会把历史抹成「未测」）
    let mut old_results: Vec<serde_json::Value> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(arr) = v.get("results").and_then(|r| r.as_array()) {
                old_results = arr.clone();
            }
        }
    }
    let merged = merge_formcheck_results(&old_results, facts);
    let payload = serde_json::json!({
        "at": bsend::coverage::now_unix(),
        "sample": info.name,
        "results": merged,
    });
    let body = serde_json::to_string_pretty(&payload)?;
    match bsend::atomic_write(&path, body.as_bytes()) {
        Ok(_) => println!("实测结论已落盘：{}", path.display()),
        Err(e) => println!("实测结论落盘失败（不影响演练）：{e}"),
    }

    Ok(())
}

/// formcheck 结论按站点合并：未重跑站保留旧结论，重跑站以本次为准
fn merge_formcheck_results(
    old: &[serde_json::Value],
    facts: Vec<serde_json::Value>,
) -> Vec<serde_json::Value> {
    let rerun: HashSet<&str> = facts
        .iter()
        .filter_map(|f| f.get("site").and_then(|s| s.as_str()))
        .collect();
    let mut merged: Vec<serde_json::Value> = old
        .iter()
        .filter(|o| {
            o.get("site")
                .and_then(|s| s.as_str())
                .is_some_and(|id| !rerun.contains(id))
        })
        .cloned()
        .collect();
    merged.extend(facts);
    merged
}

/// 落盘核对用的一条成员种子（hash + 下载器实时记账 + 文件清单）
struct LayoutMember {
    hash: String,
    meta: bsend::models::LocalTorrent,
    files: Vec<(String, i64)>,
}

/// 落盘核对（只读）：抽若干内容，用下载器实时 name/save_path/文件清单，
/// 模拟「把这一段内容的种子推到另一个站时」会用哪个 savepath（与真实推种同一套判定）。
fn cmd_layoutcheck(cfg: &Config, limit: usize, only: Option<String>) -> Result<()> {
    let index = ContentIndex::load(&cfg.coverage_state_path());
    let qb = connect_qb(cfg)?;
    let only_lc = only.as_ref().map(|s| s.to_lowercase());
    let (mut shown, mut same_n, mut remap_n, mut bad_n) = (0usize, 0usize, 0usize, 0usize);
    println!("落盘核对（只读，不推种）｜按下载器实时 name/savePath/文件清单判定\n");
    for (key, entry) in index.contents.iter() {
        if shown >= limit {
            break;
        }
        if let Some(sub) = &only_lc {
            if !key.to_lowercase().contains(sub) && !entry.name.to_lowercase().contains(sub) {
                continue;
            }
        }
        // 实时记账（最多 4 条成员；拿不到的文件清单留空，判定会退回沿用）
        let mut members: Vec<LayoutMember> = Vec::new();
        for h in entry.torrents.keys().take(4) {
            let Some(meta) = qb.torrent_meta(h).ok().flatten() else {
                continue;
            };
            let files = qb.torrent_files(h).unwrap_or_default();
            members.push(LayoutMember {
                hash: h.to_string(),
                meta,
                files,
            });
        }
        if members.is_empty() {
            continue;
        }
        shown += 1;
        println!(
            "== 「{}」（{} 个成员在下载器里）",
            entry.name,
            members.len()
        );
        let base = &members[0];
        println!(
            "   基准 {:<10} {:<44} save={}  {} 个文件",
            &base.hash[..base.hash.len().min(10)],
            base.meta.name,
            base.meta.save_path,
            base.files.len()
        );
        for m in members.iter().skip(1) {
            let l = bsend::layout::plan_add_layout(
                &m.meta.name,
                &m.files,
                &base.meta.name,
                &base.meta.save_path,
                &base.files,
            );
            let tag = match &l {
                bsend::layout::AddLayout::Same { .. } => {
                    same_n += 1;
                    "同盘沿用"
                }
                bsend::layout::AddLayout::Remap { .. } => {
                    remap_n += 1;
                    "复用既有目录"
                }
                bsend::layout::AddLayout::Mismatch(_) => {
                    bad_n += 1;
                    "不能推"
                }
            };
            println!(
                "   {:<10} {:<44} save={}  → {}：{}",
                &m.hash[..m.hash.len().min(10)],
                m.meta.name,
                m.meta.save_path,
                tag,
                l.describe()
            );
        }
    }
    println!("\n汇总：{shown} 段内容｜同盘沿用 {same_n}｜复用既有目录 {remap_n}｜不能推 {bad_n}");
    Ok(())
}

fn cmd_report(
    cfg: &Config,
    top: usize,
    json: Option<PathBuf>,
    sites: Option<Vec<String>>,
) -> Result<()> {
    let index = ContentIndex::load(&cfg.coverage_state_path());
    let allow: Option<HashSet<String>> = sites.map(|v| v.into_iter().collect());
    let picked: Vec<bsend::config::SiteConf> = cfg
        .sites
        .iter()
        .filter(|s| allow.as_ref().is_none_or(|a| a.contains(&s.id)))
        .cloned()
        .collect();
    let path_text = cfg.coverage_state_path().display().to_string();
    let report = CoverageReport::build(
        &index,
        &picked,
        top,
        bsend::coverage::now_unix(),
        &path_text,
    );
    if report.totals.contents == 0 {
        println!("索引是空的（{path_text}）：先跑 bsend scan");
        return Ok(());
    }

    let when = chrono::DateTime::from_timestamp(report.generated_at, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| report.generated_at.to_string());
    println!("覆盖报告 {path_text}");
    println!("生成时间 {when}");
    println!(
        "索引：{} 段内容、{} 条成员种子（归档倒计时 {}、源种已定位 {}）",
        commas(report.totals.contents as i64),
        commas(report.totals.member_torrents as i64),
        report.totals.gone_contents,
        report.totals.resolved_sources
    );
    println!();
    // 表头与数据行都按「终端显示宽度」补齐：中文是 2 列，用 {:<12} 会错位
    const COLS: [(&str, usize, bool); 12] = [
        ("站点", 12, false),
        ("格子", 8, true),
        ("已覆盖", 8, true),
        ("做种", 8, true),
        ("辅种", 6, true),
        ("转种", 6, true),
        ("站内没有", 9, true),
        ("待找源", 7, true),
        ("待人工", 7, true),
        ("失败", 6, true),
        ("未判定", 9, true),
        ("覆盖率", 8, true),
    ];
    let header: Vec<String> = COLS
        .iter()
        .map(|(t, w, right)| {
            if *right {
                pad_left(t, *w)
            } else {
                pad_right(t, *w)
            }
        })
        .collect();
    println!("{}", header.join(" "));
    for s in &report.sites {
        let n = |st: CoverageStatus| s.by_status.get(&st).copied().unwrap_or(0) as i64;
        let cells = [
            pad_right(&s.site_id, 12),
            pad_left(&commas(s.cells as i64), 8),
            pad_left(&commas(s.covered as i64), 8),
            pad_left(&commas(n(CoverageStatus::Seeding)), 8),
            pad_left(&commas(n(CoverageStatus::Assisted)), 6),
            pad_left(&commas(n(CoverageStatus::Reseeded)), 6),
            pad_left(&commas(n(CoverageStatus::Miss)), 9),
            pad_left(&commas(n(CoverageStatus::WaitSource)), 7),
            pad_left(&commas(n(CoverageStatus::ReviewPending)), 7),
            pad_left(&commas(n(CoverageStatus::Failed)), 6),
            pad_left(&commas(s.pending as i64), 9),
            pad_left(&format!("{:.1}%", s.covered_ratio * 100.0), 8),
        ];
        println!("{}", cells.join(" "));
    }

    if !report.uncovered.is_empty() {
        println!(
            "\n未覆盖最多（前 {} 条，按缺站数→体积）：",
            report.uncovered.len()
        );
        for it in &report.uncovered {
            println!(
                "  缺 {:>2} 站  {:>8}  {}",
                it.missing_sites.len(),
                size_text(it.total_size),
                it.name
            );
        }
    }
    if !report.drafts.is_empty() || !report.wait_source.is_empty() {
        println!(
            "\n转种草稿 {} 条、待找源 {} 条（详见 bsend status / --json）",
            report.drafts.len(),
            report.wait_source.len()
        );
    }
    if let Some(p) = &json {
        report.write_json(p)?;
        println!("\nJSON 已写入 {}", p.display());
    }
    Ok(())
}

/// 终端显示宽度：CJK 全角字符占 2 列（表格对齐不能按 char 数算）
fn disp_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            let wide = matches!(cp,
                0x1100..=0x115F
                | 0x2E80..=0xA4CF
                | 0xAC00..=0xD7A3
                | 0xF900..=0xFAFF
                | 0xFE30..=0xFE6F
                | 0xFF00..=0xFF60
                | 0xFFE0..=0xFFE6
                | 0x1F300..=0x1F64F
                | 0x20000..=0x3FFFD
            );
            if wide {
                2
            } else {
                1
            }
        })
        .sum()
}

fn pad_left(s: &str, width: usize) -> String {
    let d = disp_width(s);
    format!("{}{}", " ".repeat(width.saturating_sub(d)), s)
}

fn pad_right(s: &str, width: usize) -> String {
    let d = disp_width(s);
    format!("{}{}", s, " ".repeat(width.saturating_sub(d)))
}

/// 千分位（表格里 4 万和 4 千一眼要能分清）
fn commas(n: i64) -> String {
    let neg = n < 0;
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        // 从右往左每 3 位插逗号。等价写法 `(len-i) % 3 == 0` 会被 clippy 要求用
        // is_multiple_of（1.87 才稳定），而 Cargo.toml 承诺 MSRV 1.82
        if i > 0 && i % 3 == digits.len() % 3 {
            out.push(',');
        }
        out.push(c);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

/// 字节数 → 人看的大小（GB 起算，辅种看的都是大文件）
fn size_text(bytes: i64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    }
}

/// 从 CookieCloud 拉取并合入本进程的 cookie（不写盘）
fn apply_cookiecloud(cfg: &mut Config) -> Result<()> {
    let Some(settings) = cfg.cookie_cloud.clone() else {
        anyhow::bail!(
            "boxsend.json 里没有配 cookieCloud（host/key/password），先在 GUI 设置里填好再试"
        );
    };
    let cc = CookieCloud::new(settings)?;
    let pulled = cc.pull()?;
    let n = cfg.merge_cookies(pulled);
    println!("CookieCloud：拉取并解密 {n} 个域名的 cookie");
    Ok(())
}

/// `bsend cookies`：逐站看 cookie 现状（绝不打印 cookie 值）
fn cmd_cookies(cfg: &mut Config, from: &str, write: bool, all: bool) -> Result<()> {
    match from {
        "json" => {}
        "cookiecloud" | "cc" => apply_cookiecloud(cfg)?,
        other => anyhow::bail!("--from 只支持 json / cookiecloud，收到 {other}"),
    }
    if write {
        let path = cfg.save_cookies()?;
        println!("已写入 {}（旧文件备份为 cookies.json.bak）", path.display());
    }
    if cfg.cookies.is_empty() {
        println!("没有任何 cookie（cookies.json 为空，且没有从 CookieCloud 拉到）");
        return Ok(());
    }

    // host 匹配：相等或子域（.pterclub.net 覆盖 tracker.pterclub.net）
    let matched = |host: &str| -> Vec<&bsend::config::CookieEntry> {
        let host = host.to_lowercase();
        let mut out = Vec::new();
        for (key, list) in &cfg.cookies {
            let key = key.to_lowercase();
            if host == key || host.ends_with(&format!(".{key}")) {
                out.extend(list.iter());
            }
        }
        out
    };
    // 鉴权字段：NexusPHP 系 c_secure_uid / c_secure_pass，Unit3D 系 token，Discuz 系 auth
    let looks_authed = |list: &[&bsend::config::CookieEntry]| {
        list.iter().any(|c| {
            let n = c.name.to_lowercase();
            n.contains("uid")
                || n.contains("pass")
                || n.contains("token")
                || n.contains("auth")
                || n == "session"
        })
    };
    let fmt_expiry = |list: &[&bsend::config::CookieEntry]| -> String {
        let earliest = list
            .iter()
            .filter_map(|c| c.expiration_date)
            .filter(|d| *d > 0.0)
            .fold(f64::INFINITY, f64::min);
        if !earliest.is_finite() {
            return "会话级".to_string();
        }
        let expired = earliest <= chrono::Local::now().timestamp() as f64;
        let text = chrono::DateTime::from_timestamp(earliest as i64, 0)
            .map(|d| {
                d.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .unwrap_or_else(|| "-".to_string());
        if expired {
            format!("{text}（已过期）")
        } else {
            text
        }
    };

    if all {
        let mut keys: Vec<&String> = cfg.cookies.keys().collect();
        keys.sort();
        println!("\n{:<28} {:>4}  {:>16}  鉴权", "域名", "条目", "最早过期");
        for host in keys {
            let list: Vec<&bsend::config::CookieEntry> = cfg.cookies[host].iter().collect();
            println!(
                "{:<28} {:>4}  {:>16}  {}",
                host,
                list.len(),
                fmt_expiry(&list),
                if looks_authed(&list) { "有" } else { "无" }
            );
        }
        println!("\n（--all：共 {} 个域名）", cfg.cookies.len());
        return Ok(());
    }

    // 默认只看参与辅种的站点
    println!(
        "\n{:<12} {:<22} {:>4}  {:>16}  鉴权",
        "站点", "域名", "条目", "最早过期"
    );
    let mut missing: Vec<&str> = Vec::new();
    for site in &cfg.sites {
        let host = site.primary_host();
        let list = matched(host);
        if list.is_empty() {
            missing.push(site.id.as_str());
            println!("{:<12} {:<22} {:>4}  {:>16}  -", site.id, host, 0, "缺");
        } else {
            println!(
                "{:<12} {:<22} {:>4}  {:>16}  {}",
                site.id,
                host,
                list.len(),
                fmt_expiry(&list),
                if looks_authed(&list) { "有" } else { "无" }
            );
        }
    }
    println!(
        "\n参与站点 {} 个，有 cookie 的 {} 个；cookies 库共 {} 个域名（--all 看全部）",
        cfg.sites.len(),
        cfg.sites.len() - missing.len(),
        cfg.cookies.len()
    );
    if !missing.is_empty() {
        println!("这些站没有 cookie，跑到会 failed：{}", missing.join("、"));
    }
    Ok(())
}

/// 站点档案汇总：把「配置开关 + 最近一次实测 + pieces 支持 + cookie」并到一处。
/// `--json` 给脚本；`--md` 生成文档表格；默认人读对齐表。
fn cmd_sites(cfg: &Config, as_json: bool, as_md: bool) -> Result<()> {
    use serde_json::Value;
    let raw = std::fs::read_to_string(&cfg.config_path)
        .with_context(|| format!("读配置 {} 失败", cfg.config_path.display()))?;
    let root: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let entries = root
        .get("sourceSites")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let data_dir = cfg
        .coverage_state_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();

    // 最近一次 formcheck 结论（dataDir/formcheck.json）
    let mut drill: BTreeMap<String, Value> = BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string(data_dir.join("formcheck.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(arr) = v.get("results").and_then(|r| r.as_array()) {
                for r in arr {
                    if let Some(id) = r.get("site").and_then(|s| s.as_str()) {
                        drill.insert(id.to_string(), r.clone());
                    }
                }
            }
        }
    }
    // pieces-hash 能力表（dataDir/pieces-cache.json）
    let mut pieces: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string(data_dir.join("pieces-cache.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(m) = v.get("capabilities").and_then(|c| c.as_object()) {
                for (k, val) in m {
                    if let Some(s) = val.as_str() {
                        pieces.insert(k.clone(), s.to_string());
                    }
                }
            }
        }
    }
    // cookie 条目数：用 Config 里已合并（含 --cookiecloud）且未过期的视图，
    // 不再直读 cookies.json——否则 --cookiecloud 场景下报表与真实运行态不一致
    let now_ts = bsend::coverage::now_unix() as f64;
    let mut cookies: BTreeMap<String, usize> = BTreeMap::new();
    for (host, list) in &cfg.cookies {
        let alive = list
            .iter()
            .filter(|c| !c.value.is_empty())
            .filter(|c| !c.expiration_date.is_some_and(|e| e > 0.0 && e < now_ts))
            .count();
        cookies.insert(
            host.to_lowercase().trim_start_matches("www.").to_string(),
            alive,
        );
    }
    let paused: HashSet<String> = cfg.coverage.paused_sites.iter().cloned().collect();
    let rpaused: HashSet<String> = cfg.coverage.reseed_paused_sites.iter().cloned().collect();

    let mut rows: Vec<Value> = Vec::new();
    for e in entries {
        let id = e
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let name = e
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let url = e
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let host = bsend::url_host(&url)
            .map(|h| h.trim_start_matches("www.").to_string())
            .unwrap_or_default();
        let fw = e
            .get("framework")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let enabled = e.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let note = e
            .get("statusNote")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let d = drill.get(&id);
        let verdict = d
            .and_then(|x| x.get("verdict"))
            .and_then(|v| v.as_str())
            .unwrap_or("未测")
            .to_string();
        let reason = d
            .and_then(|x| {
                x.get("error")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        x.get("core")
                            .and_then(|c| c.as_array())
                            .and_then(|a| a.first())
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .or_else(|| {
                        x.get("soft")
                            .and_then(|c| c.as_array())
                            .and_then(|a| a.first())
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
            })
            .unwrap_or_default();
        rows.push(serde_json::json!({
            "site": id,
            "name": name,
            "host": host,
            "framework": fw,
            "enabled": enabled,
            "paused": paused.contains(&id),
            "reseedPaused": rpaused.contains(&id),
            "drill": verdict,
            "drillReason": reason,
            "pieces": pieces.get(&id).cloned().unwrap_or_else(|| "未测".into()),
            "cookies": cookies.get(&host).copied().unwrap_or(0),
            "note": note,
        }));
    }
    // 排序：失败 → 缺口 → 齐 → 未测，同档按站点 id
    rows.sort_by_key(|r| {
        let d = r.get("drill").and_then(|v| v.as_str()).unwrap_or("");
        let pr = match d {
            "fail" => 0,
            "gap" => 1,
            "ok" => 2,
            _ => 3,
        };
        (
            pr,
            r.get("site")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        )
    });

    if as_json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if as_md {
        println!("| 站点 | 名称 | 域名 | 框架 | 启用 | 全停 | 停转种 | 实测 | 实测原因 | pieces | cookie | 备注 |");
        println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
        for r in &rows {
            let g = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            println!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                g("site"),
                g("name"),
                g("host"),
                g("framework"),
                yn(r, "enabled"),
                yn(r, "paused"),
                yn(r, "reseedPaused"),
                g("drill"),
                g("drillReason").replace('|', "/"),
                g("pieces"),
                r.get("cookies").and_then(|v| v.as_u64()).unwrap_or(0),
                g("note").replace('|', "/"),
            );
        }
        return Ok(());
    }
    println!(
        "站点档案（{} 个）｜启用=参与辅种/转种；全停=pausedSites；停转种=reseedPausedSites；实测=最近一次 formcheck\n",
        rows.len()
    );
    println!(
        "{:<14}{:<10}{:<30}{:<10}{:<5}{:<5}{:<7}{:<6}{:<10}{:<7}",
        "站点", "名称", "域名", "框架", "启用", "全停", "停转种", "实测", "pieces", "cookie"
    );
    for r in &rows {
        let g = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        println!(
            "{:<14}{:<10}{:<30}{:<10}{:<5}{:<5}{:<7}{:<6}{:<10}{:<7}",
            g("site"),
            g("name"),
            g("host"),
            g("framework"),
            yn(r, "enabled"),
            yn(r, "paused"),
            yn(r, "reseedPaused"),
            g("drill"),
            g("pieces"),
            r.get("cookies").and_then(|v| v.as_u64()).unwrap_or(0),
        );
    }
    Ok(())
}

fn yn(v: &serde_json::Value, k: &str) -> &'static str {
    if v.get(k).and_then(|x| x.as_bool()).unwrap_or(false) {
        "是"
    } else {
        "—"
    }
}

/// 自动发种：核心流程在 bsend::autopost（CLI/GUI 共用），这里只把事件渲染成终端输出。
fn cmd_autopost(
    cfg: &Config,
    dry_run: bool,
    limit: Option<usize>,
    sites: Option<Vec<String>>,
) -> Result<()> {
    use bsend::autopost::{run_autopost, AutopostEvent, AutopostOptions};
    let opts = AutopostOptions {
        dry_run,
        limit,
        sites,
    };
    run_autopost(cfg, &opts, &mut |ev| match ev {
        AutopostEvent::Log(line) => println!("{line}"),
        AutopostEvent::DraftStart {
            name,
            source_site,
            target_site,
        } => println!("== 「{name}」{source_site} → {target_site}"),
        AutopostEvent::Gates { gates, .. } => {
            for r in &gates {
                println!("   [{:5}] {}：{}", r.verdict.as_str(), r.gate, r.message);
            }
        }
        AutopostEvent::Outcome {
            result, url, error, ..
        } => match result {
            "review_failed" => println!("   审阅失败：{}", error.unwrap_or_default()),
            "blocked" => println!("   → 门禁拦截，转人工\n"),
            "skipped_duplicate" => println!("   → 已跳过：目标站已有同条目，未上传\n"),
            "posted" => println!("   → 已发布：{}\n", url.unwrap_or_default()),
            "failed" => println!("   → 发布失败：{}\n", error.unwrap_or_default()),
            _ => {}
        },
    })?;
    Ok(())
}

/// 全站门禁演练：用演练样本逐站构造上传字段 → 官方预审（有则调，不上传种子）→ 门禁矩阵。
/// 全程只 GET 上传页 / POST 官方预审接口，绝不调用 takeupload.php。
#[derive(Debug, serde::Serialize)]
struct GatecheckRow {
    site: String,
    name: String,
    verdict: String,
    note: String,
    audit: String,
    audit_details: Vec<String>,
    core_gaps: Vec<String>,
    gates: Vec<(String, String, String)>,
}

fn cmd_gatecheck(cfg: &Config, sites: Option<Vec<String>>, as_json: bool) -> Result<()> {
    let adapters = build_adapters(cfg, &sites)?;
    let info = bsend::reseed::drill_release();
    let mut rows: Vec<GatecheckRow> = Vec::new();
    println!(
        "全站门禁演练（只构造字段 + 官方预审，绝不上传种子）｜样本「{}」｜{} 站",
        info.name,
        adapters.len()
    );
    for a in &adapters {
        let mut prep = match bsend::reseed::prepare_upload(a, &info) {
            Ok(p) => p,
            Err(e) => {
                rows.push(GatecheckRow {
                    site: a.site_id.clone(),
                    name: a.site_name.clone(),
                    verdict: "error".into(),
                    note: format!("{e:#}"),
                    audit: "—".into(),
                    audit_details: vec![],
                    core_gaps: vec![],
                    gates: vec![],
                });
                continue;
            }
        };
        let site_rules = cfg
            .coverage
            .reseed_rules
            .get(&a.site_id)
            .cloned()
            .unwrap_or_default();
        bsend::reseed::apply_site_rules(&mut prep, &site_rules);
        let drill = bsend::reseed::analyze_prepared(&a.site_id, &a.site_name, &prep, &info);
        let core = drill.core_gaps();
        let type_label = drill
            .select_picks
            .iter()
            .find(|(n, _)| n == "type")
            .map(|(_, l)| l.clone());
        let rules = site_rules;
        let mut audit_text = "站点无预审".to_string();
        let mut audit_details = Vec::new();
        let mut audit_passed = None;
        if bsend::reseed::pre_audit_supported(&prep.form_html) {
            match bsend::reseed::run_pre_audit(a, &info, &prep) {
                Ok(v) => {
                    audit_text = v.summary();
                    audit_details = v
                        .details
                        .iter()
                        .map(|d| d.line())
                        .chain(v.suggestions.iter().cloned())
                        .collect();
                    audit_passed = Some(v.passed);
                }
                Err(e) => audit_text = format!("预审接口失败：{e:#}"),
            }
        }
        let input = bsend::gates::GateInput {
            rules: &rules,
            name: &info.name,
            subtitle: &info.subtitle,
            descr: &info.descr,
            mediainfo: &info.mediainfo,
            has_links: info.has_media_links(),
            type_label: type_label.as_deref(),
            core_gaps: &core,
            audit_passed,
        };
        let results = bsend::gates::evaluate(&input);
        let pass = bsend::gates::all_passed(&results);
        let gates = results
            .iter()
            .map(|r| {
                (
                    r.gate.to_string(),
                    r.verdict.as_str().to_string(),
                    r.message.clone(),
                )
            })
            .collect();
        rows.push(GatecheckRow {
            site: a.site_id.clone(),
            name: a.site_name.clone(),
            verdict: if pass { "pass" } else { "block" }.into(),
            note: format!(
                "字段{} / 下拉{} / MediaInfo:{}",
                drill.field_count,
                drill.select_count,
                drill.mediainfo_field.as_deref().unwrap_or("缺")
            ),
            audit: audit_text,
            audit_details,
            core_gaps: core,
            gates,
        });
    }
    let save_path = cfg
        .coverage_state_path()
        .parent()
        .map(|p| p.join("gatecheck.json"))
        .unwrap_or_else(|| PathBuf::from("gatecheck.json"));
    let payload = serde_json::to_string_pretty(&rows)?;
    if let Err(e) = bsend::atomic_write(&save_path, payload.as_bytes()) {
        println!("门禁结果落盘失败（不影响演练）：{e}");
    }
    if as_json {
        println!("{payload}");
        return Ok(());
    }
    println!(
        "{:<14}{:<10}{:<8}{:<44}阻断项",
        "站点", "名称", "门禁", "官方预审"
    );
    for r in &rows {
        let blockers: Vec<String> = r
            .gates
            .iter()
            .filter(|(_, v, _)| v == "block")
            .map(|(g, _, m)| format!("{g}:{m}"))
            .collect();
        println!(
            "{:<14}{:<10}{:<8}{:<44}{}",
            r.site,
            r.name,
            r.verdict,
            r.audit.chars().take(42).collect::<String>(),
            blockers.join("；").chars().take(90).collect::<String>()
        );
    }
    let ok = rows.iter().filter(|r| r.verdict == "pass").count();
    println!(
        "\n汇总：{} 站｜门禁通过 {}｜阻断 {}｜错误 {}",
        rows.len(),
        ok,
        rows.iter().filter(|r| r.verdict == "block").count(),
        rows.iter().filter(|r| r.verdict == "error").count()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formcheck_merge_keeps_unrerun_sites() {
        let old = vec![
            serde_json::json!({"site": "a", "verdict": "ok"}),
            serde_json::json!({"site": "b", "verdict": "fail", "error": "无权限"}),
        ];
        let facts = vec![serde_json::json!({"site": "b", "verdict": "ok"})];
        let merged = merge_formcheck_results(&old, facts);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0]["site"], "a");
        assert_eq!(merged[1]["site"], "b");
        assert_eq!(merged[1]["verdict"], "ok");
        assert!(merged[1].get("error").is_none());
    }

    #[test]
    fn formcheck_merge_empty_old_and_new_site() {
        let merged = merge_formcheck_results(&[], vec![serde_json::json!({"site": "x"})]);
        assert_eq!(merged.len(), 1);
        let old = vec![serde_json::json!({"site": "a"})];
        let merged = merge_formcheck_results(&old, vec![serde_json::json!({"site": "b"})]);
        assert_eq!(merged.len(), 2);
    }
}
