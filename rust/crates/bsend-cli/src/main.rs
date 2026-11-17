//! bsend：box-send 的铺种引擎 CLI（Rust 版）。
//! 与 GUI 版共用同一份 `boxsend.json` / `cookies.json`，索引落在 dataDir/coverage.json。
//!
//! 用法：
//! ```text
//! bsend scan                     # 扫描下载器，建立/刷新内容索引（不动站点）
//! bsend plan --limit 5           # 预演一轮：检索照跑、推送只出结论、不落盘
//! bsend run --sites hdsky,pter   # 跑一整轮（辅种真推；转种只出草稿）
//! bsend status                   # 看索引现状
//! ```

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use bsend::config::Config;
use bsend::coverage::{run_round, Event, RoundOptions, RoundReport};
use bsend::index::{ContentIndex, CoverageStatus};
use bsend::qbittorrent::QBittorrent;
use bsend::sites::nexusphp::NexusPhp;

#[derive(Parser)]
#[command(
    name = "bsend",
    version,
    about = "铺种引擎：内容索引 → 源种定位 → 辅种/转种（box-send Rust 版）"
)]
struct Cli {
    /// boxsend.json 路径（与 GUI 版共用同一份配置）
    #[arg(long, default_value = "boxsend.json")]
    config: PathBuf,
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
    },
    /// 跑一整轮铺种：辅种真推下载器，转种出草稿清单
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
    },
    /// 看索引现状：内容数、覆盖分布、转种草稿、待找源
    Status,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config)
        .with_context(|| format!("加载配置 {} 失败", cli.config.display()))?;
    match cli.cmd {
        Cmd::Scan => cmd_scan(&cfg),
        Cmd::Plan { limit, sites } => cmd_round(&cfg, limit, sites, true, false),
        Cmd::Run {
            limit,
            sites,
            dry_run,
        } => cmd_round(&cfg, limit, sites, dry_run, true),
        Cmd::Status => cmd_status(&cfg),
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
    let allow: Option<HashSet<String>> = filter.as_ref().map(|v| v.iter().cloned().collect());
    let mut out = Vec::new();
    for s in &cfg.sites {
        if let Some(a) = &allow {
            if !a.contains(&s.id) {
                continue;
            }
        }
        let cookie = cfg.cookie_header_for(&s.announce_host);
        let mut adapter = NexusPhp::new(
            &s.id,
            &s.name,
            &s.url,
            s.search_url.as_deref(),
            cookie.as_deref(),
        )
        .with_context(|| format!("站点配置不合法：{}", s.id))?;
        adapter.user_agent = cfg.user_agent.clone();
        out.push(Arc::new(adapter));
    }
    if out.is_empty() {
        anyhow::bail!("没有可参与的站点（检查 boxsend.json sourceSites 的 enabled/assist.siteIDs/pausedSites）");
    }
    Ok(out)
}

fn cmd_scan(cfg: &Config) -> Result<()> {
    let qb = connect_qb(cfg)?;
    let torrents = qb.list_torrents()?;
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let files_of = |t: &bsend::models::LocalTorrent| qb.torrent_files(&t.hash).ok();
    let now = bsend::coverage::now_unix();
    let delta = index.merge_scan(&torrents, files_of, now, cfg.coverage.archive_days);
    index.save()?;
    println!(
        "扫描 {} 条种子：新内容 {}、并入 {}、移除 {}、归档 {}、跳过未完成 {}",
        torrents.len(),
        delta.new_contents,
        delta.merged,
        delta.removed_torrents,
        delta.purged_contents,
        delta.skipped_incomplete
    );
    println!(
        "索引：{} 段内容 → {}",
        index.contents.len(),
        cfg.coverage_state_path().display()
    );
    Ok(())
}

fn cmd_round(
    cfg: &Config,
    limit: Option<usize>,
    sites: Option<Vec<String>>,
    dry_run: bool,
    persist: bool,
) -> Result<()> {
    let qb = connect_qb(cfg)?;
    let adapters = build_adapters(cfg, &sites)?;
    println!(
        "参与站点（{}）：{}",
        adapters.len(),
        adapters
            .iter()
            .map(|a| a.site_id.clone())
            .collect::<Vec<_>>()
            .join("、")
    );
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let opts = RoundOptions {
        limit,
        site_filter: sites.map(|v| v.into_iter().collect()),
        dry_run,
        persist,
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
    for (kind, (used, max)) in &r.budget {
        println!("  预算 {kind}：{used}/{max}");
    }
    if !r.reseed_drafts.is_empty() {
        println!(
            "\n转种草稿（{} 条，v1 不自动上传，人工确认后走 GUI 版转种）：",
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
