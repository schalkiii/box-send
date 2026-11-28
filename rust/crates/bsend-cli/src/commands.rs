//! CLI 各命令的处理函数；从 `main.rs` 拆出，参数解析仍在 `cli.rs`。

use super::*;

pub(super) fn cmd_scan(cfg: &Config) -> Result<()> {
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

pub(super) fn cmd_round(
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

pub(super) fn print_event(ev: Event) {
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

pub(super) fn short_key(key: &str) -> &str {
    // contentKey 是 m:<16hex> / h:<40hex>，界面只露尾巴
    if key.len() > 10 {
        &key[..10.min(key.len())]
    } else {
        key
    }
}

pub(super) fn print_report(r: &RoundReport) {
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

/// 站上已发布、但下载/挂回 qB 失败的 note 前缀：`posted_unresolved` 是新发定位失败，
/// `existed_unresolved` 是判重命中后挂回失败。两者都要进“站上已发、qB 未挂回”待跟进，
/// 不能只认新发那一个前缀。
pub(super) fn is_qb_mount_unresolved(note: Option<&str>) -> bool {
    note.is_some_and(|n| n.starts_with("posted_unresolved") || n.starts_with("existed_unresolved"))
}

/// 待跟进清单的一行：「内容名」→ 站点：站点当时说的话（截掉操作提示和原始回包，最多 90 个字）
pub(super) fn follow_up_item(name: &str, site: &str, note: Option<&str>) -> String {
    let why: String = note
        .map(|n| {
            n.split("（原始回包")
                .next()
                .unwrap_or(n)
                .split("：需人工")
                .next()
                .unwrap_or(n)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let short = |s: &str, n: usize| -> String {
        let head: String = s.chars().take(n).collect();
        if s.chars().count() > n {
            format!("{head}…")
        } else {
            head
        }
    };
    if why.is_empty() {
        format!("「{}」→ {site}", short(name, 48))
    } else {
        format!("「{}」→ {site}：{}", short(name, 48), short(&why, 90))
    }
}

pub(super) fn print_follow_up_items(items: &[String]) {
    for item in items.iter().take(10) {
        println!("      {item}");
    }
    if items.len() > 10 {
        println!("      ……还有 {} 条", items.len() - 10);
    }
}

pub(super) fn cmd_stats(cfg: &Config, json: bool) -> Result<()> {
    let data_dir = cfg
        .coverage_state_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let rows = bsend::poststats::site_post_stats(&data_dir);
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    let total: usize = rows.iter().map(|r| r.posted).sum();
    println!(
        "发种统计（{}）：{} 个站点、去重后 {} 条\n",
        data_dir.display(),
        rows.len(),
        total
    );
    println!("| 站点 | 已发种 | 最后发种时间 | 最后一条种 |");
    println!("|------|--------|--------------|------------|");
    for r in rows {
        println!(
            "| {} | {} | {} | {} |",
            r.site,
            r.posted,
            if r.last_at_text().is_empty() {
                "—".to_string()
            } else {
                r.last_at_text()
            },
            if r.last_url.is_empty() {
                "—"
            } else {
                r.last_url.as_str()
            }
        );
    }
    Ok(())
}

pub(super) fn cmd_status(cfg: &Config) -> Result<()> {
    let index = ContentIndex::load(&cfg.coverage_state_path());
    let mut by_status: BTreeMap<CoverageStatus, usize> = BTreeMap::new();
    let mut member_torrents = 0usize;
    let mut gone = 0usize;
    let mut resolved_sources = 0usize;
    let mut posted_unresolved = 0usize;
    let mut drafts = Vec::new();
    // 待跟进的几类都要点名（光有总数没法跟进）：每类一个清单，附一句站点当时说的话
    let mut follow: BTreeMap<CoverageStatus, Vec<String>> = BTreeMap::new();
    let mut posted_unresolved_items: Vec<String> = Vec::new();
    let mut pending_by_status: BTreeMap<CoverageStatus, usize> = BTreeMap::new();
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
            if cell.status == CoverageStatus::ReviewPending
                && is_qb_mount_unresolved(cell.note.as_deref())
            {
                posted_unresolved += 1;
                posted_unresolved_items.push(follow_up_item(&e.name, site, cell.note.as_deref()));
            }
            // 等站点侧动作的几类（操作提示和原始回包留给 audit）
            if cell.status.is_review_related() || cell.status == CoverageStatus::PendingOffers {
                follow.entry(cell.status).or_default().push(follow_up_item(
                    &e.name,
                    site,
                    cell.note.as_deref(),
                ));
            }
            match cell.status {
                CoverageStatus::ReseedDraft => {
                    drafts.push(format!("「{}」→ {}", e.name, site));
                }
                // Listed from `report.wait_source` below (deduped by name);
                // keep it out of the pending bucket, same as before.
                CoverageStatus::WaitSource => {}
                st if !st.is_settled() => {
                    *pending_by_status.entry(st).or_insert(0) += 1;
                }
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

    // Per-site numbers come from the shared report builder, so `status` and
    // `report` stay on the same coverage counting rules.
    let path_text = cfg.coverage_state_path().display().to_string();
    let mut report = CoverageReport::build(
        &index,
        &cfg.sites,
        0,
        bsend::coverage::now_unix(),
        &path_text,
    );
    // 待发清单按 qB 里最后一次扫描到的 tracker 派生，与 autopost 的实时口径一致；
    // 这样 status 不会再显示只有旧记录才有的草稿。
    let allow: HashSet<String> = cfg.coverage.auto_reseed_sites.iter().cloned().collect();
    let mut limits: HashMap<String, usize> = HashMap::new();
    for site in &cfg.sites {
        let rules = cfg.coverage.rules_for(&site.id);
        limits.insert(site.id.clone(), rules.daily_limit() as usize);
    }
    report.derive_qb_gap_drafts_from_index(&index, &cfg.sites, &allow, &limits);

    // Whitelisted sites without a draft: call this out even on a fresh index,
    // because listing a site in the whitelist on its own never produces drafts.
    let no_draft = whitelist_without_drafts(&report, &cfg.coverage.auto_reseed_sites);
    if !no_draft.is_empty() {
        let names: Vec<String> = no_draft
            .iter()
            .map(|id| {
                report
                    .sites
                    .iter()
                    .find(|s| s.site_id == *id)
                    .map(|s| {
                        if s.name.is_empty() {
                            id.clone()
                        } else {
                            format!("{}（{}）", s.name, id)
                        }
                    })
                    .unwrap_or_else(|| id.clone())
            })
            .collect();
        println!("\n白名单站点暂无草稿：{}", names.join("、"));
        println!(
            "  以上站点本轮没有可发条目：qB 里没有缺口，或源种子未定位、内容已消失。待发由 `bsend autopost` 每轮按 qB 实时缺口生成；`bsend scan` 只刷新索引。"
        );
    }

    if by_status.is_empty() {
        println!("还没有任何处理记录：先 `bsend scan` 建索引；辅种或发种成功后才会写处理记录。");
        return Ok(());
    }
    println!("覆盖分布：");
    for (st, n) in &by_status {
        println!("  {} {}", st.label(), n);
    }
    let follow_total: usize = follow.values().map(Vec::len).sum::<usize>() + posted_unresolved;
    if follow_total > 0 {
        println!("  待跟进（等站点侧动作，程序不自动重试）：");
        let groups: [(&CoverageStatus, &str); 4] = [
            (
                &CoverageStatus::PendingOffers,
                "等走候选入口（站点判种子过大，需到该站候选页提交）",
            ),
            (
                &CoverageStatus::PendingReview,
                "等站点审核（每轮自动发种开头、以及 `bsend followup` 会回访；审完自动把站点种子挂回 qB）",
            ),
            (
                &CoverageStatus::ReviewRejected,
                "站点审核没过（要人改简介或种子后在站上处理；程序不重发）",
            ),
            (
                &CoverageStatus::GoneFromSite,
                "站上已找不到（下载器里那条还挂着该站 tracker，删不删由人定）",
            ),
        ];
        for (st, title) in groups {
            let Some(items) = follow.get(st) else {
                continue;
            };
            println!("    {title}：{}", items.len());
            print_follow_up_items(items);
        }
        if posted_unresolved > 0 {
            println!(
                "    站上已发、qB 未挂回：{posted_unresolved}（按详情页用 `bsend pull` 挂回，勿重发）"
            );
            print_follow_up_items(&posted_unresolved_items);
        }
        println!(
            "      （清单只列前 10 条；回访：`bsend followup`，明细：`bsend audit --days 7`）"
        );
    }

    // 草稿按白名单拆分：未启用组只计数、不改覆盖记录，不算待发
    let allow: std::collections::HashSet<String> =
        cfg.coverage.auto_reseed_sites.iter().cloned().collect();
    let (enabled_drafts, disabled_drafts) = split_drafts_by_allow(&report, &allow);

    println!("\n单站覆盖（按 qB 里是否已有该站 tracker 计算）：");
    let header = [
        pad_right("站点", 14),
        pad_left("已有", 6),
        pad_left("缺口", 6),
        pad_left("覆盖率", 8),
        pad_left("草稿", 6),
        pad_left("已跳过", 6),
    ];
    println!("  {}", header.join(" "));
    for s in &report.sites {
        let label = if s.name.is_empty() {
            s.site_id.as_str()
        } else {
            s.name.as_str()
        };
        let skipped = s
            .by_status
            .get(&CoverageStatus::Skipped)
            .copied()
            .unwrap_or(0);
        let row = [
            pad_right(label, 14),
            pad_left(&commas(s.qb_covered as i64), 6),
            pad_left(&commas(s.qb_missing as i64), 6),
            pad_left(&format!("{:.1}%", s.qb_ratio * 100.0), 8),
            pad_left(&commas(draft_count_for_site(&report, &s.site_id) as i64), 6),
            pad_left(&commas(skipped as i64), 6),
        ];
        println!("  {}", row.join(" "));
    }
    println!(
        "  已有 = qB 里有该站 tracker 的内容数；缺口 = 全库内容数 − 已有；覆盖率 = 已有 / 全库内容数（当前 {} 条）",
        commas(report.totals.contents as i64)
    );
    println!("  已跳过 = 命中内容类型黑名单等按策略不再自动发的草稿（第二轮起不再入队）");
    println!("\n待发草稿 {} 条", enabled_drafts.len());
    if !disabled_drafts.is_empty() {
        let mut disabled_sites: Vec<&str> = disabled_drafts
            .iter()
            .map(|d| d.target_site.as_str())
            .collect();
        disabled_sites.sort_unstable();
        disabled_sites.dedup();
        println!(
            "目标站未启用 {} 条（{}）—— 不在自动发种白名单，不算待发；以后加白名单仍可用",
            disabled_drafts.len(),
            disabled_sites.join("、")
        );
    }

    if !pending_by_status.is_empty() {
        let total: usize = pending_by_status.values().sum();
        let parts: Vec<String> = pending_by_status
            .iter()
            .map(|(st, n)| format!("{} {n}", st.label()))
            .collect();
        println!(
            "\n待办覆盖记录（共 {total}）：{}（下一轮会接着处理）",
            parts.join("、")
        );
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
    // Reuse the report's wait-source list: per-site cells would count the same
    // content name once per site, while the report dedupes by content name.
    if !report.wait_source.is_empty() {
        println!("\n待找源（{}）：", report.wait_source.len());
        for n in report.wait_source.iter().take(20) {
            println!("  {n}");
        }
        if report.wait_source.len() > 20 {
            println!("  … 共 {} 条", report.wait_source.len());
        }
    }
    Ok(())
}

/// 转种草稿的审阅与提交：`--draft N` 审阅第 N 条（不 POST），加 `--yes` 才真发
pub(super) fn cmd_reseed(
    cfg: &Config,
    draft: Option<usize>,
    yes: bool,
    only: Option<&str>,
    site: Option<&str>,
) -> Result<()> {
    if yes {
        bsend::config::ensure_publish_enabled(cfg)?;
    }
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let path_text = cfg.coverage_state_path().display().to_string();
    let mut report = CoverageReport::build(
        &index,
        &cfg.sites,
        20,
        bsend::coverage::now_unix(),
        &path_text,
    );
    let allow: HashSet<String> = cfg.coverage.auto_reseed_sites.iter().cloned().collect();
    let mut limits: HashMap<String, usize> = HashMap::new();
    for site in &cfg.sites {
        let rules = cfg.coverage.rules_for(&site.id);
        limits.insert(site.id.clone(), rules.daily_limit() as usize);
    }
    report.derive_qb_gap_drafts_from_index(&index, &cfg.sites, &allow, &limits);
    if report.drafts.is_empty() {
        println!("没有可发条目：qB 里没有目标站缺口，或源种子还没定位。先 `bsend scan` 刷新索引。");
        return Ok(());
    }
    let site_filter = site.map(str::trim).filter(|s| !s.is_empty());
    let candidates: Vec<&bsend::report::DraftRow> = report
        .drafts
        .iter()
        .filter(|d| site_filter.is_none_or(|s| d.target_site.eq_ignore_ascii_case(s)))
        .collect();
    if candidates.is_empty() {
        let wanted = site_filter.unwrap_or("指定站");
        println!("没有可发条目：目标站「{wanted}」当前没有缺口草稿。");
        return Ok(());
    }
    if draft.is_none() && only.is_none() {
        let total = candidates.len();
        println!(
            "可发条目（{total} 条，按 qB 缺口现场生成）。`bsend reseed --draft N` 审阅第 N 条，加 `--yes` 真提交："
        );
        for (i, d) in candidates.iter().take(200).enumerate() {
            println!(
                "  #{} 「{}」{} → {}（源：{}）",
                i + 1,
                d.name,
                d.source_site,
                d.target_site,
                d.source_url
            );
        }
        if total > 200 {
            println!("  ……只显示前 200 条，共 {total} 条。");
        }
        return Ok(());
    }
    let d = if let Some(filter) = only.map(str::trim).filter(|s| !s.is_empty()) {
        candidates
            .iter()
            .copied()
            .find(|d| d.name.contains(filter) || d.content_key.starts_with(filter))
            .ok_or_else(|| anyhow::anyhow!("没有名称或 content key 含「{filter}」的草稿"))?
    } else {
        let n = draft.ok_or_else(|| anyhow::anyhow!("草稿序号和筛选条件至少要有一个"))?;
        candidates
            .get(n.checked_sub(1).unwrap_or(usize::MAX))
            .copied()
            .ok_or_else(|| anyhow::anyhow!("没有第 {n} 条草稿（共 {} 条）", candidates.len()))?
    };
    let req = bsend::reseed::request_from_draft(
        &d.content_key,
        &d.name,
        &d.source_site,
        &d.source_url,
        &d.target_site,
    );
    let mut log = |line: &str| println!("  {line}");
    if !yes {
        let rep = bsend::reseed::review_reseed(cfg, &req, &mut log, None)?;
        println!();
        println!("{}", rep.render(200));
        println!("以上为审阅（未提交）。确认无误后加 `--yes` 真提交。");
        return Ok(());
    }
    let qb = connect_qb(cfg)?;
    let outcome = bsend::reseed::run_reseed(cfg, &qb, &req, &mut index, &mut log, None)?;
    println!("{}", outcome.display());
    // 手动发布同样要进 auto-audit：自动队列的日限额/失败冷却不能漏看这批。
    if let Err(e) = bsend::autopost::append_manual_audit(
        cfg,
        &req.target_site,
        &req.content_key,
        outcome.audit_result(),
        outcome.url(),
        &outcome.display(),
    ) {
        eprintln!("写发种审计失败（本次发布已成功，审计缺口需人工补）：{e}");
    }
    Ok(())
}

pub(super) fn cmd_pull(
    cfg: &Config,
    site: &str,
    url: &str,
    key: Option<&str>,
    file: Option<&std::path::Path>,
) -> Result<()> {
    let qb = connect_qb(cfg)?;
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let mut log = |line: &str| println!("  {line}");
    let (status, detail) =
        bsend::reseed::pull_published(cfg, &qb, site, url, key, file, &mut index, &mut log)?;
    println!("挂回结果：{} → {}", status.label(), detail);
    Ok(())
}

/// 只读巡检各站收件箱：GET `messages.php` 列表页，列标题、发件人、时间、已读/未读和粗分类；
/// 不点消息、不回信、不改覆盖记录。
pub(super) fn cmd_inbox(
    cfg: &Config,
    sites: Option<Vec<String>>,
    limit: usize,
    concurrency: usize,
    as_json: bool,
) -> Result<()> {
    let wanted: Vec<String> = match sites {
        Some(v) => v,
        None => {
            // 没指定站点时只扫“有发种记录”的站：数据源与 `bsend stats` 相同，
            // 避免把 147 个配置站全打开一遍。手动发过、日志里没有的站用 --sites 补。
            let data_dir = cfg
                .coverage_state_path()
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            bsend::poststats::site_post_stats(&data_dir)
                .into_iter()
                .map(|s| s.site)
                .collect()
        }
    };
    let mut ids: Vec<String> = wanted
        .into_iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        anyhow::bail!("没有可巡检的站点：发种统计里没有记录；用 --sites 指定站点 id");
    }
    let adapters = build_adapters(cfg, &Some(ids.clone()))?;
    let missing: Vec<String> = ids
        .iter()
        .filter(|id| !adapters.iter().any(|a| &a.site_id == *id))
        .cloned()
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "注意：这些站不在当前参与列表里，未巡检：{}",
            missing.join("、")
        );
    }
    if adapters.is_empty() {
        anyhow::bail!(
            "这些站点不在当前参与列表里，或不是可访问的 NexusPHP 站：{}",
            ids.join("、")
        );
    }
    if !as_json {
        println!(
            "只读巡检 {} 个站的收件箱（GET messages.php；不点消息、不回信）",
            adapters.len()
        );
    }
    let reports = bsend::inbox::scan_sites(&adapters, limit, concurrency);
    if as_json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
        return Ok(());
    }

    let ok = reports.iter().filter(|r| r.status == "ok").count();
    let failed = reports.len().saturating_sub(ok);
    let unread = reports
        .iter()
        .flat_map(|r| &r.messages)
        .filter(|m| m.unread)
        .count();
    let rejected = reports
        .iter()
        .flat_map(|r| &r.messages)
        .filter(|m| m.kind == bsend::inbox::InboxKind::ReviewRejected)
        .count();
    let removed = reports
        .iter()
        .flat_map(|r| &r.messages)
        .filter(|m| m.kind == bsend::inbox::InboxKind::TorrentRemoved)
        .count();
    let warned = reports
        .iter()
        .flat_map(|r| &r.messages)
        .filter(|m| m.kind == bsend::inbox::InboxKind::Warning)
        .count();
    println!(
        "结果：成功 {ok} 站、打不开 {failed} 站；未读 {unread} 条，审核不通过 {rejected} 条（含历史）、删种 {removed} 条、警告 {warned} 条"
    );
    for report in &reports {
        if report.status != "ok" {
            let why = if report.login_expired {
                "登录态可能失效"
            } else {
                "请求失败"
            };
            println!("  {}：{why} — {}", report.site, report.error);
            continue;
        }
        let total = report.messages.len();
        let unread_here = report.messages.iter().filter(|m| m.unread).count();
        println!(
            "  {}：最近 {} 条消息，未读 {}{}",
            report.site,
            total,
            unread_here,
            if report.title.is_empty() {
                String::new()
            } else {
                format!("（{}）", report.title)
            }
        );
        let mut other = 0;
        for msg in &report.messages {
            if msg.kind == bsend::inbox::InboxKind::Other {
                other += 1;
                continue;
            }
            println!(
                "    [{}]{} {}（{} / {}）{}",
                msg.kind.label(),
                if msg.unread { " [未读]" } else { "" },
                msg.title,
                if msg.sender.is_empty() {
                    "发件人未知"
                } else {
                    msg.sender.as_str()
                },
                if msg.at.is_empty() {
                    "时间未知"
                } else {
                    msg.at.as_str()
                },
                if msg.url.is_empty() {
                    String::new()
                } else {
                    format!(" {}", msg.url)
                }
            );
        }
        if other > 0 {
            println!("    其他消息 {other} 条（用 --json 看全部）");
        }
    }
    Ok(())
}

/// 回访站点审核：逐条打开“等站点审核 / 审核没过 / 站上已找不到”的详情页，按页面上的结论改格子；
/// 审完了就把站点种子下回来挂进 qB。不上传、不发帖。自动发种每轮开头也会跑一遍同样的回访。
pub(super) fn cmd_followup(
    cfg: &Config,
    dry_run: bool,
    all: bool,
    sites: Option<Vec<String>>,
    limit: Option<usize>,
    as_json: bool,
) -> Result<()> {
    let qb = connect_qb(cfg)?;
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let opts = ReviewCheckOptions {
        all_posted: all,
        dry_run,
        sites: sites.map(|v| v.into_iter().collect()),
        limit,
    };
    if !as_json && dry_run {
        println!("演练：照样打开详情页、下载站点种子来看结论，但不改覆盖记录、不往下载器里加种子");
    }
    let mut log = |line: &str| println!("  {line}");
    let s = bsend::review::recheck_reviews(cfg, &qb, &mut index, &opts, &mut log)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    println!(
        "回访结果：选出 {} 条，打开了 {} 条详情页（{}）",
        s.selected,
        s.checked,
        cfg.coverage_state_path().display()
    );
    println!(
        "  结论没变：仍在等审核 {}、仍是不通过或站上没有 {}",
        s.still_pending, s.still_bad
    );
    println!(
        "  新记下：等站点审核 {}、站点审核没过 {}、站上已找不到 {}",
        s.marked_pending, s.marked_rejected, s.marked_gone
    );
    if dry_run {
        println!(
            "  审完了（正式回访才会改格子、挂回 qB）：{}",
            s.would_settle
        );
    }
    println!(
        "  审核结束已收尾：{}（站点种子已挂进 qB，或下载器里本来就在做种）",
        s.settled
    );
    println!(
        "  页面正常没有审核标记：{}；打不开不算结论：{}；没领到请求额度：{}",
        s.clean, s.errors, s.no_budget
    );
    if s.selected == 0 {
        println!(
            "  没有要回访的条目（只看记了审核结论的格子{}）",
            if all {
                ""
            } else {
                "；加 --all 连自己发的、已在做种的也看一遍"
            }
        );
    }
    Ok(())
}

/// 单站 formcheck 落盘记录。旧字段（site/verdict/fields/selects/core/soft/error/at）
/// 保持不变以兼容 sites 总账与 scripts/site-matrix.ps1；新增 action（绝对提交地址）
/// 与 selectOptions（下拉名 -> [[value, label], …] 选项表）。
pub(super) fn formcheck_fact(
    d: &bsend::reseed::FormDrill,
    core: &[String],
    soft: &[String],
) -> serde_json::Value {
    let select_options: serde_json::Map<String, serde_json::Value> = d
        .select_options
        .iter()
        .map(|(name, opts)| {
            let pairs: Vec<serde_json::Value> = opts
                .iter()
                .map(|(v, l)| serde_json::json!([v, l]))
                .collect();
            (name.clone(), serde_json::Value::Array(pairs))
        })
        .collect();
    serde_json::json!({
        "site": d.site_id,
        "verdict": if d.error.is_some() { "fail" } else if core.is_empty() { "ok" } else { "gap" },
        "fields": d.field_count,
        "selects": d.select_count,
        "core": core,
        "soft": soft,
        "error": d.error,
        "errorKind": d.error_kind,
        "action": d.action,
        "selectOptions": serde_json::Value::Object(select_options),
        "at": bsend::coverage::now_unix(),
    })
}

/// 发种表格演练（只审不发）：逐站拉 upload.php 跑与真实转种同一套字段构造，
/// 核对表单完备性与选档映射，绝不 POST。
pub(super) fn cmd_formcheck(cfg: &Config, sites: Option<Vec<String>>) -> Result<()> {
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
        let site_rules = cfg.coverage.rules_for(&a.site_id);
        let d = bsend::reseed::drill_upload_form_with_rules(a, &site_rules);
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
        facts.push(formcheck_fact(&d, &core, &soft));
    }
    println!(
        "\n汇总：{} 站｜表单齐 {}｜有缺口 {}｜不可发种 {}（权限/登录/无表单/其他）",
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
pub(super) fn merge_formcheck_results(
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
pub(super) struct LayoutMember {
    hash: String,
    meta: bsend::models::LocalTorrent,
    files: Vec<(String, i64)>,
}

/// 落盘核对（只读）：抽若干内容，用下载器实时 name/save_path/文件清单，
/// 模拟「把这一段内容的种子推到另一个站时」会用哪个 savepath（与真实推种同一套判定）。
pub(super) fn cmd_layoutcheck(cfg: &Config, limit: usize, only: Option<String>) -> Result<()> {
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

pub(super) fn cmd_report(
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
    let mut report = CoverageReport::build(
        &index,
        &picked,
        top,
        bsend::coverage::now_unix(),
        &path_text,
    );
    let allowed: HashSet<String> = cfg.coverage.auto_reseed_sites.iter().cloned().collect();
    let mut limits: HashMap<String, usize> = HashMap::new();
    for site in &picked {
        let rules = cfg.coverage.rules_for(&site.id);
        limits.insert(site.id.clone(), rules.daily_limit() as usize);
    }
    report.derive_qb_gap_drafts_from_index(&index, &picked, &allowed, &limits);
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
    const COLS: [(&str, usize, bool); 14] = [
        ("站点", 12, false),
        ("历史已判", 8, true),
        ("历史已覆盖", 10, true),
        ("做种", 8, true),
        ("辅种", 6, true),
        ("转种", 6, true),
        ("站内没有", 9, true),
        ("待找源", 7, true),
        ("待人工", 7, true),
        ("失败", 6, true),
        ("已跳过", 6, true),
        ("qB已有", 8, true),
        ("qB缺口", 8, true),
        ("qB覆盖率", 9, true),
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
            pad_left(&commas(s.covered as i64), 10),
            pad_left(&commas(n(CoverageStatus::Seeding)), 8),
            pad_left(&commas(n(CoverageStatus::Assisted)), 6),
            pad_left(&commas(n(CoverageStatus::Reseeded)), 6),
            pad_left(&commas(n(CoverageStatus::Miss)), 9),
            pad_left(&commas(n(CoverageStatus::WaitSource)), 7),
            pad_left(&commas(n(CoverageStatus::ReviewPending)), 7),
            pad_left(&commas(n(CoverageStatus::Failed)), 6),
            pad_left(&commas(n(CoverageStatus::Skipped)), 6),
            pad_left(&commas(s.qb_covered as i64), 8),
            pad_left(&commas(s.qb_missing as i64), 8),
            pad_left(&format!("{:.1}%", s.qb_ratio * 100.0), 9),
        ];
        println!("{}", cells.join(" "));
    }
    println!(
        "历史已判/历史已覆盖是旧记录口径，仅供历史对比；qB 已有 = qB 里有该站 tracker 的内容数，缺口 = 全库内容数 − 已有，覆盖率 = 已有 / 全库内容数（{} 条）。",
        commas(report.totals.contents as i64)
    );

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

/// 发种审计周报（只读）：聚合 auto-audit.jsonl，按天 × 站点出计数；不写覆盖记录、不发请求。
pub(super) fn cmd_audit(
    cfg: &Config,
    days: usize,
    site: Option<String>,
    as_json: bool,
) -> Result<()> {
    let path = bsend::autopost::audit_path_of(cfg);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "没有审计记录（{}）：先跑 bsend autopost 产生审计。",
                path.display()
            );
            return Ok(());
        }
        Err(e) => return Err(e).with_context(|| format!("读审计文件 {} 失败", path.display())),
    };
    let report =
        bsend::report::audit_daily(&text, bsend::coverage::now_unix(), days, site.as_deref());

    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let when = chrono::DateTime::from_timestamp(report.generated_at, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| report.generated_at.to_string());
    let site_note = site
        .as_deref()
        .map(|s| format!("｜站点 {s}"))
        .unwrap_or_default();
    println!("发种审计周报（最近 {days} 天，北京时间日界）{site_note}");
    println!("审计文件 {}｜生成时间 {when}", path.display());
    println!();

    if report.days.is_empty() {
        println!("窗口内没有审计记录。");
        if report.totals.malformed > 0 {
            println!("其中畸形行 {} 条（已跳过）。", report.totals.malformed);
        }
        return Ok(());
    }

    const COLS: [(&str, usize, bool); 7] = [
        ("站点", 14, false),
        ("发布", 8, true),
        ("拦截", 8, true),
        ("失败", 8, true),
        ("重复", 8, true),
        ("其他版本", 9, true),
        ("其他", 8, true),
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

    for day in &report.days {
        println!("{}", day.date);
        println!("  {}", header.join(" "));
        let mut sub = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        let mut pending = 0usize;
        let mut waiting = 0usize;
        let mut refused = 0usize;
        let mut unresolved = 0usize;
        for (sid, c) in &day.by_site {
            let row = [
                pad_right(sid, 14),
                pad_left(&commas(c.posted as i64), 8),
                pad_left(&commas(c.blocked as i64), 8),
                pad_left(&commas(c.failed as i64), 8),
                pad_left(&commas(c.duplicate as i64), 8),
                pad_left(&commas(c.other_version as i64), 9),
                pad_left(&commas(c.other as i64), 8),
            ];
            println!("  {}", row.join(" "));
            sub.0 += c.posted;
            sub.1 += c.blocked;
            sub.2 += c.failed;
            sub.3 += c.duplicate;
            sub.4 += c.other_version;
            sub.5 += c.other;
            pending += c.pending_offers;
            waiting += c.pending_review;
            refused += c.review_rejected;
            unresolved += c.posted_unresolved;
        }
        let sub_row = [
            pad_right("当日小计", 14),
            pad_left(&commas(sub.0 as i64), 8),
            pad_left(&commas(sub.1 as i64), 8),
            pad_left(&commas(sub.2 as i64), 8),
            pad_left(&commas(sub.3 as i64), 8),
            pad_left(&commas(sub.4 as i64), 9),
            pad_left(&commas(sub.5 as i64), 8),
        ];
        println!("  {}", sub_row.join(" "));
        if pending + waiting + refused + unresolved > 0 {
            println!(
                "  其中待跟进（不是失败）：待走候选入口 {pending}｜等站点审核 {waiting}｜站点审核没过 {refused}｜站上已发未挂回 {unresolved}"
            );
        }
        println!();
    }

    let t = &report.totals;
    println!(
        "合计：发布 {}｜拦截 {}｜失败 {}｜重复 {}｜其他版本 {}｜其他 {}",
        t.posted, t.blocked, t.failed, t.duplicate, t.other_version, t.other
    );
    println!(
        "待跟进：待走候选入口 {}｜等站点审核 {}｜站点审核没过 {}｜站上已发未挂回 {}（等回访或人工处理，不算失败、不重试）",
        t.pending_offers, t.pending_review, t.review_rejected, t.posted_unresolved
    );
    let blocked_total = t.no_permission + t.two_fa + t.challenge_gate + t.spa_api;
    if blocked_total > 0 {
        println!(
            "站侧阻塞（不是失败、不进冷却）：无权限 {}｜2FA {}｜人机验证 {}｜单页应用 {}",
            t.no_permission, t.two_fa, t.challenge_gate, t.spa_api
        );
    }
    println!(
        "计入记录 {} 条；畸形行 {} 条（跳过未计入）。",
        t.records, t.malformed
    );
    Ok(())
}

/// 终端显示宽度：CJK 全角字符占 2 列（表格对齐不能按 char 数算）
pub(super) fn disp_width(s: &str) -> usize {
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

pub(super) fn pad_left(s: &str, width: usize) -> String {
    let d = disp_width(s);
    format!("{}{}", " ".repeat(width.saturating_sub(d)), s)
}

pub(super) fn pad_right(s: &str, width: usize) -> String {
    let d = disp_width(s);
    format!("{}{}", s, " ".repeat(width.saturating_sub(d)))
}

/// 千分位（表格里 4 万和 4 千一眼要能分清）
pub(super) fn commas(n: i64) -> String {
    let neg = n < 0;
    // `abs()` 在 i64::MIN 上会溢出；`unsigned_abs()` 保留完整绝对值。
    let digits = n.unsigned_abs().to_string();
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
pub(super) fn size_text(bytes: i64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    }
}

/// 从 CookieCloud 拉取并合入本进程的 cookie（不写盘）
pub(super) fn apply_cookiecloud(cfg: &mut Config) -> Result<()> {
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
pub(super) fn cmd_cookies(cfg: &mut Config, from: &str, write: bool, all: bool) -> Result<()> {
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
pub(super) fn cmd_sites(cfg: &Config, as_json: bool, as_md: bool) -> Result<()> {
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

pub(super) fn yn(v: &serde_json::Value, k: &str) -> &'static str {
    if v.get(k).and_then(|x| x.as_bool()).unwrap_or(false) {
        "是"
    } else {
        "—"
    }
}

/// 自动发种：核心流程在 bsend::autopost（CLI/GUI 共用），这里只把事件渲染成终端输出。
pub(super) fn cmd_autopost(
    cfg: &Config,
    dry_run: bool,
    limit: Option<usize>,
    sites: Option<Vec<String>>,
    no_reconcile: bool,
    loop_mode: bool,
    interval_secs: u64,
) -> Result<()> {
    use anyhow::Context as _;
    use bsend::autopost::{run_autopost, AutopostEvent, AutopostOptions};
    let opts = AutopostOptions {
        dry_run,
        limit,
        sites,
        reconcile: !no_reconcile,
        loop_mode,
        interval_secs,
    };
    let mut emit = |ev: AutopostEvent| {
        match ev {
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
            "blocked" => println!("   → 发布检查拦截，转人工\n"),
            "skipped_duplicate" => println!("   → 已跳过：目标站已有同条目，未上传\n"),
            "posted" => println!("   → 已发布：{}\n", url.unwrap_or_default()),
            "posted_unresolved" => println!(
                "   → 已上传但站内定位不到条目，待人工/下轮复核：{}\n",
                url.unwrap_or_default()
            ),
            "pending_offers" => {
                println!("   → 站点要求走候选入口（offers.php），已记入待跟进，不自动重试\n")
            }
            "pending_review" => println!(
                "   → 站点收下了，但还没审完，暂时拿不到站点种子；已记入待跟进，审完会自动挂回 qB\n      {}\n",
                error.unwrap_or_default()
            ),
            "review_rejected" => println!(
                "   → 站点审核没过：已记入待跟进，要人改简介或种子后在站上处理；程序不重发\n      {}\n",
                error.unwrap_or_default()
            ),
            "no_permission" => println!(
                "   → 站侧阻塞：账号在该站无发布权限，待人工确认后再处理；不算失败、程序不自动重发\n      {}\n",
                error.unwrap_or_default()
            ),
            "two_fa" => println!(
                "   → 站侧阻塞：站点要求先过二次验证，待在前台浏览器完成后再处理；不算失败、程序不自动重发\n      {}\n",
                error.unwrap_or_default()
            ),
            "challenge_gate" => println!(
                "   → 站侧阻塞：被人机验证拦截，待在前台浏览器过一次验证后再处理；不算失败、程序不自动重发\n      {}\n",
                error.unwrap_or_default()
            ),
            "spa_api" => println!(
                "   → 站侧阻塞：上传界面是纯前端/接口，程序暂不支持；不算失败、程序不自动重发\n      {}\n",
                error.unwrap_or_default()
            ),
            "failed" => println!("   → 发布失败：{}\n", error.unwrap_or_default()),
            _ => {}
        },
    }
    };
    if !loop_mode {
        run_autopost(cfg, &opts, &mut emit)?;
        return Ok(());
    }
    // 循环模式：单进程串行跑多轮，每轮都是一次完整 `run_autopost`。
    // 失败冷却 / 日限额 / 白名单 / 跨进程锁都在 `run_autopost` 内部，这里不重复实现。
    // 退出语义：单轮返回 `Err` 属致命错误（连不上下载器、发布总闸未开、锁拿不到等），
    // 打印后以非零退出；单条草稿级失败已在审计里记，不会让整轮 Err。
    // `--limit` 按轮计：每轮各自受 limit 约束。Ctrl+C 走默认 SIGINT 终止，不装信号处理器。
    let interval = std::time::Duration::from_secs(interval_secs.max(1));
    let mut round = 1usize;
    loop {
        println!("====== 第 {round} 轮开始 ======");
        run_autopost(cfg, &opts, &mut emit)
            .with_context(|| format!("第 {round} 轮自动发种失败（致命，终止循环）"))?;
        let next = chrono::Local::now() + chrono::Duration::seconds(interval.as_secs() as i64);
        println!(
            "第 {round} 轮结束；下次运行 {}，间隔 {} 秒\n",
            next.format("%Y-%m-%d %H:%M:%S"),
            interval.as_secs()
        );
        round += 1;
        std::thread::sleep(interval);
    }
}

/// 全站发布检查演练：用演练样本逐站构造上传字段 → 官方预审（有则调，不上传种子）→ 发布检查矩阵。
/// 全程只 GET 上传页 / POST 官方预审接口，绝不调用 takeupload.php。
#[derive(Debug, serde::Serialize)]
pub(super) struct GatecheckRow {
    site: String,
    name: String,
    verdict: String,
    note: String,
    audit: String,
    audit_details: Vec<String>,
    core_gaps: Vec<String>,
    gates: Vec<(String, String, String)>,
    /// 构造出来的提交字段（演练留证用，不 POST）
    fields: Vec<(String, String)>,
}

pub(super) fn cmd_gatecheck(cfg: &Config, sites: Option<Vec<String>>, as_json: bool) -> Result<()> {
    let adapters = build_adapters(cfg, &sites)?;
    let info = bsend::reseed::drill_release();
    let mut rows: Vec<GatecheckRow> = Vec::new();
    println!(
        "全站发布检查演练（只构造字段 + 官方预审，绝不上传种子）｜样本「{}」｜{} 站",
        info.name,
        adapters.len()
    );
    for a in &adapters {
        let site_rules = cfg.coverage.rules_for(&a.site_id);
        let mut prep = match bsend::reseed::prepare_upload(a, &info) {
            Ok(mut p) => {
                let _ = bsend::reseed::prepare_category_fields(a, &mut p, &info, &site_rules);
                p
            }
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
                    fields: vec![],
                });
                continue;
            }
        };
        bsend::reseed::apply_site_rules(&mut prep, &site_rules, None);
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
        let descr_final = bsend::reseed::submitted_descr(&prep, &info.descr);
        let (poster_count, screenshot_count) = bsend::reseed::image_split_counts(&prep);
        let input = bsend::gates::GateInput {
            rules: &rules,
            name: &info.name,
            subtitle: &info.subtitle,
            descr: descr_final,
            mediainfo: &info.mediainfo,
            has_links: info.has_media_links(),
            type_label: type_label.as_deref(),
            core_gaps: &core,
            required_gaps: &prep.required_gaps,
            poster_count,
            screenshot_count,
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
            fields: prep.fields.clone(),
        });
    }
    let save_path = cfg
        .coverage_state_path()
        .parent()
        .map(|p| p.join("gatecheck.json"))
        .unwrap_or_else(|| PathBuf::from("gatecheck.json"));
    let payload = serde_json::to_string_pretty(&rows)?;
    if let Err(e) = bsend::atomic_write(&save_path, payload.as_bytes()) {
        println!("发布检查结果落盘失败（不影响演练）：{e}");
    }
    if as_json {
        println!("{payload}");
        return Ok(());
    }
    println!(
        "{:<14}{:<10}{:<8}{:<44}阻断项",
        "站点", "名称", "发布检查", "官方预审"
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
        "\n汇总：{} 站｜发布检查通过 {}｜阻断 {}｜错误 {}",
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

    fn cli_test_config(tag: &str) -> Config {
        let dir = std::env::temp_dir().join(format!("bsend-cli-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("boxsend.json");
        let root = serde_json::json!({
            "dataDir": dir.to_string_lossy(),
            "downloader": {"url": "http://127.0.0.1:1"},
            "sourceSites": [
                {"id": "nx", "name": "NX", "url": "https://nx.example/", "framework": "NexusPHP", "enabled": true}
            ]
        });
        std::fs::write(&path, root.to_string()).unwrap();
        Config::load(&path).unwrap()
    }

    #[test]
    fn status_with_empty_index_is_ok() {
        assert!(cmd_status(&cli_test_config("status-empty")).is_ok());
    }

    #[test]
    fn audit_without_file_is_ok() {
        assert!(cmd_audit(&cli_test_config("audit-empty"), 7, None, false).is_ok());
    }

    #[test]
    fn report_with_empty_index_is_ok() {
        assert!(cmd_report(&cli_test_config("report-empty"), 10, None, None).is_ok());
    }

    #[test]
    fn parses_pull_command() {
        let cli = Cli::try_parse_from([
            "bsend",
            "pull",
            "--site",
            "hdvideo",
            "--url",
            "https://hdvideo.top/details.php?id=78856",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Pull {
                site,
                url,
                key,
                file,
            } => {
                assert_eq!(site, "hdvideo");
                assert!(url.contains("78856"));
                assert!(key.is_none());
                assert!(file.is_none());
            }
            _ => panic!("expected pull command"),
        }
    }

    #[test]
    fn pull_command_keeps_old_name_as_hidden_alias() {
        use clap::CommandFactory as _;

        // 命令改名前写下的脚本还能跑；旧名不出现在 --help 里。
        let cli = Cli::try_parse_from([
            "bsend",
            "reattach",
            "--site",
            "hdvideo",
            "--url",
            "https://hdvideo.top/details.php?id=78856",
        ])
        .unwrap();
        assert!(matches!(cli.cmd, Cmd::Pull { .. }));
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("pull"));
        assert!(!help.contains("reattach"), "{help}");
    }

    #[test]
    fn followup_command_keeps_old_name_as_hidden_alias() {
        use clap::CommandFactory as _;

        // 回访命令一开始叫 reviewcheck，当天改名 followup；旧名照样能跑，但不出现在 --help 里。
        let cli = Cli::try_parse_from(["bsend", "reviewcheck", "--dry-run"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Followup { dry_run: true, .. }));
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("followup"));
        assert!(!help.contains("reviewcheck"), "{help}");
    }

    #[test]
    fn formcheck_fact_keeps_legacy_fields_and_adds_action_options() {
        let d = bsend::reseed::FormDrill {
            site_id: "kufei".into(),
            field_count: 16,
            select_count: 9,
            action: Some("https://kufei.example/takeupload.php".into()),
            select_options: vec![("type".into(), vec![("404".into(), "剧集".into())])],
            ..Default::default()
        };
        let fact = formcheck_fact(&d, &[], &["team_sel[4] 未选".into()]);
        // 旧字段原样保留（sites 总账 / site-matrix.ps1 依赖）
        assert_eq!(fact["site"], "kufei");
        assert_eq!(fact["fields"], 16);
        assert_eq!(fact["selects"], 9);
        assert_eq!(fact["verdict"], "ok");
        assert!(fact["core"].as_array().unwrap().is_empty());
        assert_eq!(fact["soft"][0], "team_sel[4] 未选");
        assert!(fact.get("at").is_some());
        // 新增字段：绝对提交地址 + 下拉选项表
        assert_eq!(fact["action"], "https://kufei.example/takeupload.php");
        assert_eq!(
            fact["selectOptions"]["type"][0],
            serde_json::json!(["404", "剧集"])
        );
    }

    #[test]
    fn status_defaults_to_boxsend_json_and_no_cookiecloud() {
        let cli = Cli::try_parse_from(["bsend", "status"]).expect("status 可解析");
        assert_eq!(cli.config, PathBuf::from("boxsend.json"));
        assert!(!cli.cookiecloud);
        assert!(matches!(cli.cmd, Cmd::Status));
    }

    #[test]
    fn global_cookiecloud_is_accepted_after_subcommand() {
        // 文档里的写法是 `bsend status --cookiecloud`；global 参数必须允许放在子命令后面。
        let cli = Cli::try_parse_from(["bsend", "status", "--cookiecloud"]).expect("global 参数");
        assert!(cli.cookiecloud);
        assert!(matches!(cli.cmd, Cmd::Status));
    }

    #[test]
    fn report_parses_site_list_and_top() {
        let cli = Cli::try_parse_from(["bsend", "report", "--top", "3", "--sites", "hdsky,pter"])
            .expect("report 可解析");
        match cli.cmd {
            Cmd::Report { top, sites, json } => {
                assert_eq!(top, 3);
                assert_eq!(sites, Some(vec!["hdsky".into(), "pter".into()]));
                assert!(json.is_none());
            }
            _ => panic!("expected report"),
        }
    }

    #[test]
    fn audit_defaults_to_seven_days_and_text_output() {
        let cli = Cli::try_parse_from(["bsend", "audit"]).expect("audit 可解析");
        match cli.cmd {
            Cmd::Audit { days, site, json } => {
                assert_eq!(days, 7);
                assert!(site.is_none());
                assert!(!json);
            }
            _ => panic!("expected audit"),
        }
    }

    #[test]
    fn autopost_loop_mode_flag_is_parsed() {
        let cli = Cli::try_parse_from([
            "bsend",
            "autopost",
            "--loop-mode",
            "--interval-secs",
            "5",
            "--no-reconcile",
        ])
        .expect("autopost 可解析");
        match cli.cmd {
            Cmd::Autopost {
                loop_mode,
                interval_secs,
                no_reconcile,
                ..
            } => {
                assert!(loop_mode);
                assert_eq!(interval_secs, 5);
                assert!(no_reconcile);
            }
            _ => panic!("expected autopost"),
        }
    }

    #[test]
    fn commas_handles_zero_negatives_and_max() {
        assert_eq!(commas(0), "0");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1_000), "1,000");
        assert_eq!(commas(1_234_567), "1,234,567");
        assert_eq!(commas(-1_234), "-1,234");
        assert_eq!(commas(i64::MAX), "9,223,372,036,854,775,807");
        assert_eq!(commas(i64::MIN), "-9,223,372,036,854,775,808");
    }

    #[test]
    fn size_text_switches_units_at_expected_boundaries() {
        assert_eq!(size_text(0), "0 MB");
        assert_eq!(size_text(1_048_576), "1 MB");
        assert_eq!(size_text(1_073_741_824), "1.0 GB");
        assert_eq!(size_text(1_610_612_736), "1.5 GB");
    }

    #[test]
    fn display_width_and_padding_handle_cjk_and_emoji() {
        assert_eq!(disp_width("abc"), 3);
        assert_eq!(disp_width("中"), 2);
        assert_eq!(disp_width("a中b"), 4);
        assert_eq!(disp_width("😀"), 2);
        assert_eq!(pad_right("中", 4), "中  ");
        assert_eq!(pad_left("中", 4), "  中");
        // 宽度不够时只补空格，不截断内容
        assert_eq!(pad_right("abcdef", 3), "abcdef");
    }

    #[test]
    fn unresolved_notes_cover_posted_and_existed() {
        assert!(is_qb_mount_unresolved(Some(
            "posted_unresolved：回包没给详情页"
        )));
        assert!(is_qb_mount_unresolved(Some(
            "existed_unresolved：目标种下载失败"
        )));
        assert!(!is_qb_mount_unresolved(Some("review_pending：清单对不上")));
        assert!(!is_qb_mount_unresolved(None));
    }

    #[test]
    fn follow_up_item_strips_raw_reply_and_truncates() {
        let long_name = "A".repeat(60);
        let item = follow_up_item(
            &long_name,
            "hdsky",
            Some("站点说不行（原始回包：HTTP 500 很长很长的正文）"),
        );
        assert!(item.contains("站点说不行"), "{item}");
        assert!(!item.contains("原始回包"), "{item}");
        assert!(item.contains('…'), "{item}");
        assert_eq!(item.chars().filter(|c| *c == 'A').count(), 48);
        assert_eq!(follow_up_item("短名", "pter", None), "「短名」→ pter");
    }
}
