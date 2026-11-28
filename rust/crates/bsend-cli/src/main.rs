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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;

use bsend::config::Config;
use bsend::cookiecloud::CookieCloud;
use bsend::coverage::{prefetch_files, run_round, Event, RoundOptions, RoundReport};
use bsend::index::{ContentIndex, CoverageStatus};
use bsend::qbittorrent::QBittorrent;
use bsend::report::{
    draft_count_for_site, split_drafts_by_allow, whitelist_without_drafts, CoverageReport,
};
use bsend::review::ReviewCheckOptions;
use bsend::sites::nexusphp::NexusPhp;

mod cli;
mod commands;

use cli::{Cli, Cmd};
use commands::*;

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
        Cmd::Stats { json } => cmd_stats(&cfg, json),
        Cmd::Reseed {
            draft,
            yes,
            only,
            site,
        } => cmd_reseed(&cfg, draft, yes, only.as_deref(), site.as_deref()),
        Cmd::Pull { site, url, key } => cmd_pull(&cfg, &site, &url, key.as_deref()),
        Cmd::Followup {
            dry_run,
            all,
            sites,
            limit,
            json,
        } => cmd_followup(&cfg, dry_run, all, sites, limit, json),
        Cmd::Report { top, json, sites } => cmd_report(&cfg, top, json, sites),
        Cmd::Formcheck { sites } => cmd_formcheck(&cfg, sites),
        Cmd::Layoutcheck { limit, only } => cmd_layoutcheck(&cfg, limit, only),
        Cmd::Sites { json, md } => cmd_sites(&cfg, json, md),
        Cmd::Gatecheck { sites, json } => cmd_gatecheck(&cfg, sites, json),
        Cmd::Audit { days, site, json } => cmd_audit(&cfg, days, site, json),
        Cmd::Autopost {
            dry_run,
            limit,
            sites,
            no_reconcile,
            loop_mode,
            interval_secs,
        } => cmd_autopost(
            &cfg,
            dry_run,
            limit,
            sites,
            no_reconcile,
            loop_mode,
            interval_secs,
        ),
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
    let out = bsend::sites::build_adapters(cfg, filter.as_deref(), |missing| {
        eprintln!(
            "注意：这些站没有 cookie，本轮到它们只会得到 failed：{}（可用 --cookiecloud 或 bsend cookies --from cookiecloud --write 补）",
            missing.join("、")
        );
    })?;
    // 被框架/缺 url/白名单/暂停排除的启用站也要点名，否则用户以为它们参与了
    if let Some(n) = cfg.skipped_notice() {
        eprintln!("{n}");
    }
    Ok(out)
}
