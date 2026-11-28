//! 后台任务：扫描 / 跑一轮 / 转种 / 自动发种 / CookieCloud。
//! 全部在 std::thread 里跑，事件经 mpsc 回 GUI；纯搬运自 main.rs，行为不变。

use std::sync::mpsc::Sender;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use bsend::autopost::{run_autopost, AutopostEvent, AutopostOptions};
use bsend::config::Config;
use bsend::cookiecloud::CookieCloud;
use bsend::coverage::{now_unix, prefetch_files, run_round, Event, RoundOptions};
use bsend::index::ContentIndex;
use bsend::index::CoverageStatus;
use bsend::qbittorrent::QBittorrent;
use bsend::report::CoverageReport;

use crate::status_chip;
use crate::types::{GuiEvent, SiteRow, Snapshot};

pub(crate) fn snapshot(cfg: &Config) -> Result<Snapshot> {
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
            // qB 真值口径：已有 = qB 里带该站 tracker 的内容数，缺口与覆盖率同源派生
            have: s.qb_covered,
            gap: s.qb_missing,
            ratio: s.qb_ratio,
            drafts: report
                .drafts
                .iter()
                .filter(|d| d.target_site.eq_ignore_ascii_case(&s.site_id))
                .count(),
            skipped: s
                .by_status
                .get(&CoverageStatus::Skipped)
                .copied()
                .unwrap_or(0),
            site: s.site_id,
            chips: s
                .by_status
                .into_iter()
                .filter(|(_, n)| *n > 0)
                .map(|(st, n)| status_chip(st, n))
                .collect(),
        })
        .collect();
    let post_stats = bsend::poststats::site_post_stats(
        cfg.coverage_state_path()
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".")),
    );
    Ok(Snapshot {
        stats,
        sites,
        drafts: report.drafts,
        post_stats,
    })
}

pub(crate) fn run_scan(cfg: &Config, tx: &Sender<GuiEvent>) -> Result<()> {
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

pub(crate) fn run_round_job(cfg: &Config, opts: RoundOptions, tx: &Sender<GuiEvent>) -> Result<()> {
    let qb = Arc::new(QBittorrent::new(
        &cfg.downloader.url,
        &cfg.downloader.username,
        &cfg.downloader.password,
    )?);
    let _ = tx.send(GuiEvent::Log(qb.test_connection().context("连不上下载器")?));

    // 与 CLI 共用同一份适配器构造：缺 cookie 的站点名点进日志（以前 GUI 静默跳过）
    let adapters = bsend::sites::build_adapters(cfg, None, |missing| {
        let _ = tx.send(GuiEvent::Log(format!(
            "注意：这些站没有 cookie，本轮到它们只会得到 failed：{}（诊断页可拉 CookieCloud）",
            missing.join("、")
        )));
    })?;
    if let Some(n) = cfg.skipped_notice() {
        let _ = tx.send(GuiEvent::Log(n));
    }

    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let report = run_round(
        cfg,
        qb,
        &adapters,
        &mut index,
        |ev| {
            let is_cell = matches!(ev, Event::ItemDone { .. } | Event::ItemSkipped { .. });
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
            if is_cell {
                let _ = tx.send(GuiEvent::CellDone);
            }
        },
        opts,
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

pub(crate) fn test_qb_job(cfg: &Config, tx: &Sender<GuiEvent>) -> Result<()> {
    let qb = QBittorrent::new(
        &cfg.downloader.url,
        &cfg.downloader.username,
        &cfg.downloader.password,
    )
    .context("下载器配置不合法")?;
    let msg = qb.test_connection().context("连不上下载器")?;
    let _ = tx.send(GuiEvent::Log(msg));
    Ok(())
}

pub(crate) fn pull_cookies_job(config_path: &std::path::Path, tx: &Sender<GuiEvent>) -> Result<()> {
    let mut cfg = Config::load(config_path).context("重载配置失败")?;
    let settings = cfg
        .cookie_cloud
        .take()
        .context("配置里没有 CookieCloud 段（host/key/password 齐了才算配好）")?;
    let cc = CookieCloud::new(settings)?;
    let pulled = cc
        .pull()
        .context("CookieCloud 拉取失败（服务没起或密钥不对）")?;
    let merged = cfg.merge_cookies(pulled);
    let path = cfg.save_cookies()?;
    let _ = tx.send(GuiEvent::Log(format!(
        "CookieCloud 合并 {merged} 个 host 的 cookie，已写 {}",
        path.display()
    )));
    let _ = tx.send(GuiEvent::ReloadConfig);
    Ok(())
}

pub(crate) fn run_autopost_job(
    cfg: &Config,
    opts: AutopostOptions,
    tx: &Sender<GuiEvent>,
) -> Result<()> {
    let summary = run_autopost(cfg, &opts, &mut |ev| {
        let lines: Vec<String> = match ev {
            AutopostEvent::Log(m) => vec![m],
            AutopostEvent::DraftStart {
                name,
                source_site,
                target_site,
            } => vec![format!("== 「{name}」{source_site} → {target_site}")],
            AutopostEvent::Gates { gates, .. } => gates
                .iter()
                .map(|g| format!("   [{}] {}：{}", g.verdict.as_str(), g.gate, g.message))
                .collect(),
            AutopostEvent::Outcome {
                site,
                result,
                url,
                error,
                ..
            } => vec![match result {
                "posted" => format!("→ 已发布（{site}）：{}", url.unwrap_or_default()),
                "skipped_duplicate" => format!("→ 已跳过（{site}）：目标站已有同条目"),
                "blocked" => format!("→ 发布检查拦截（{site}），转人工"),
                "review_failed" => {
                    format!("→ 审阅未通过（{site}）：{}", error.unwrap_or_default())
                }
                "no_permission" => format!(
                    "→ 站侧阻塞（{site}）：账号无发布权限，待人工处理，不算失败、不自动重发：{}",
                    error.unwrap_or_default()
                ),
                "two_fa" => format!(
                    "→ 站侧阻塞（{site}）：站点要求先过二次验证，待人工在浏览器完成，不算失败、不自动重发：{}",
                    error.unwrap_or_default()
                ),
                "challenge_gate" => format!(
                    "→ 站侧阻塞（{site}）：被人机验证拦截，待人工在浏览器过一次验证，不算失败、不自动重发：{}",
                    error.unwrap_or_default()
                ),
                "spa_api" => format!(
                    "→ 站侧阻塞（{site}）：上传界面是纯前端/接口，程序暂不支持，不算失败、不自动重发：{}",
                    error.unwrap_or_default()
                ),
                _ => format!("→ 发布失败（{site}）：{}", error.unwrap_or_default()),
            }],
        };
        for line in lines {
            let _ = tx.send(GuiEvent::Log(line));
        }
    })?;
    let _ = tx.send(GuiEvent::Log(format!(
        "自动发种结束：本次发布 {} 条",
        summary.posted
    )));
    if let Ok(s) = snapshot(cfg) {
        let _ = tx.send(GuiEvent::Snapshot(s));
    }
    Ok(())
}
