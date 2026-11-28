//! 站点审核的回访。
//!
//! 发出去的种子，站点要先审的，发种时记成「等站点审核」（[`CoverageStatus::PendingReview`]）；
//! 页面明写不通过的记成「站点审核没过」。这里负责把它们接着往下走：
//! 再看一眼详情页——审完了，就把站点种子下回来挂进 qB（下载器里本来就有就只改结论）；
//! 没过就记原因；站上已经没有这条就记下来。回访只读详情页、下载站点种子，不上传、不发帖。
//!
//! 判定（看到什么 → 做什么）是纯函数 [`decide`]，不碰网络和文件；打站点、写格子在
//! [`recheck_reviews`] 里，两边的单测分开写。

use anyhow::{Context as _, Result};
use std::collections::HashSet;
use url::Url;

use crate::bencode;
use crate::config::Config;
use crate::coverage::{now_unix, verify_hash_from_entry, verifying_was_posted_here};
use crate::index::{ContentIndex, CoverageStatus};
use crate::qbittorrent::{tracker_status_note, QBittorrent, TrackerInfo};
use crate::reseed::{adapter_for, pull_with_data};
use crate::sites::host_matches;
use crate::sites::nexusphp::{NexusPhp, NoSuchTorrent, ReviewMark, TorrentPage};
use crate::throttle::{BudgetKind, SiteThrottle};

/// 回访范围和方式
#[derive(Debug, Clone, Default)]
pub struct ReviewCheckOptions {
    /// 连我们自己发出去的条目也回访一遍（已转种 / 校验中，或带详情页链接的“已在做种”），
    /// 找出还没记下来的“未审 / 审核不通过”。不开时只看已经记了审核结论的条目。
    pub all_posted: bool,
    /// 只看和报告：不改覆盖记录，也不往下载器里加种子
    pub dry_run: bool,
    /// 只回访这些站点（站点 id）；空 = 全部
    pub sites: Option<HashSet<String>>,
    /// 最多回访多少条
    pub limit: Option<usize>,
}

/// 一次回访的清点
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReviewCheckSummary {
    /// 选出来要看的条数
    pub selected: usize,
    /// 真的打开了详情页的条数
    pub checked: usize,
    /// 仍在等审核（结论没变）
    pub still_pending: usize,
    /// 仍在“审核没过 / 站上已找不到”（结论没变）
    pub still_bad: usize,
    /// 新记下的“等站点审核”
    pub marked_pending: usize,
    /// 新记下的“站点审核没过”
    pub marked_rejected: usize,
    /// 新记下的“站上已找不到”
    pub marked_gone: usize,
    /// 审核结束并收尾：挂回 qB，或下载器里本来就在做种、改回已转种
    pub settled: usize,
    /// 演练时看到“审完了”：正式回访才会真的改格子 / 挂回 qB 的条数
    pub would_settle: usize,
    /// 正常条目（`all_posted` 带进来的）没有审核标记，不用动
    pub clean: usize,
    /// 页面打不开（网络故障、登录失效、限流、解析不出）：不算结论，格子不动
    pub errors: usize,
    /// 站点节流没领到号，这一轮没看
    pub no_budget: usize,
}

/// 回访一条时看到的情况
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    /// 页面还标着等审核（站点原话）
    Pending(String),
    /// 页面明写审核不通过（站点给的原因）
    Rejected(String),
    /// 详情页说没有这条种子（页面原话）
    Gone(String),
    /// 页面正常，没有审核标记
    Clear,
    /// 审核图标标着“通过”（藏宝阁实测 `title="通过"`）
    Approved(String),
    /// 页面打不开：不算结论
    Unreachable(String),
}

/// 看到情况后要做什么
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 什么都不改
    Keep,
    /// 改成这个状态（备注跟着换）
    Mark(CoverageStatus),
    /// 审核结束了：确认站点种子在 qB 里做种，不在就下回来挂进去
    Settle,
}

/// 看到的情况 → 要做的事。纯函数。
///
/// - 打不开：一律不动（登录失效、限流都不是站点对这条种子的结论）。
/// - 页面还标着等审核 / 不通过 / 没有这条：和现有结论一样就不动，不一样就改过去。
/// - 页面正常（没有审核标记，或图标标着“通过”）：只有之前记了审核结论的才要收尾；
///   `all_posted` 带进来的正常条目不用动。
pub fn decide(current: CoverageStatus, seen: &Seen) -> Action {
    match seen {
        Seen::Unreachable(_) => Action::Keep,
        Seen::Gone(_) => {
            if current == CoverageStatus::GoneFromSite {
                Action::Keep
            } else {
                Action::Mark(CoverageStatus::GoneFromSite)
            }
        }
        Seen::Pending(_) => {
            if current == CoverageStatus::PendingReview {
                Action::Keep
            } else {
                Action::Mark(CoverageStatus::PendingReview)
            }
        }
        Seen::Rejected(_) => {
            if current == CoverageStatus::ReviewRejected {
                Action::Keep
            } else {
                Action::Mark(CoverageStatus::ReviewRejected)
            }
        }
        Seen::Clear | Seen::Approved(_) => {
            if current.is_review_related() {
                Action::Settle
            } else {
                Action::Keep
            }
        }
    }
}

/// 这条种子在下载器里已经在做种（已转种 / 已辅种 / 已在做种 / 校验中）
fn seeding_before(status: CoverageStatus) -> bool {
    matches!(
        status,
        CoverageStatus::Reseeded
            | CoverageStatus::Assisted
            | CoverageStatus::Seeding
            | CoverageStatus::Verifying
    )
}

/// 这是我们自己发出去、并且已经挂回下载器的条目（`all_posted` 只带这些）：
/// 已转种 / 校验中，或者带详情页链接的“已在做种”（手工挂回、补挂的旧条目会落成这样）。
/// 不带链接的“已在做种”是扫描按下载器 tracker 写的——那是站上别人的种子，成千上万条，
/// 而且别人种子的审核结论和我们没关系。
fn posted_by_us(status: CoverageStatus, has_url: bool) -> bool {
    match status {
        CoverageStatus::Reseeded | CoverageStatus::Verifying => true,
        CoverageStatus::Seeding => has_url,
        _ => false,
    }
}

/// 这条格子要不要回访。纯函数。
/// - 记了审核结论的（等审核 / 没过 / 站上没有）：一定看，它们靠回访收尾；没有详情页链接的看不了。
/// - `all_posted` 才带上我们自己发出去的。
pub fn should_visit(status: CoverageStatus, has_url: bool, opts: &ReviewCheckOptions) -> bool {
    has_url && (status.is_review_related() || (opts.all_posted && posted_by_us(status, has_url)))
}

/// 改结论时写进格子的备注。纯函数。
pub fn note_for(current: CoverageStatus, seen: &Seen) -> String {
    // 审核结论变了，但做种没停：备注里说清楚
    let seeding = if seeding_before(current) {
        "；种子此前已挂回 qB，仍在做种"
    } else {
        ""
    };
    match seen {
        Seen::Pending(why) => {
            format!("站上已发布，等站点审核（回访：站点页面还标着“{why}”）{seeding}")
        }
        Seen::Rejected(why) => format!(
            "站点审核没过（回访：{why}）{seeding}；改简介或种子后在站上处理，程序不重发"
        ),
        Seen::Gone(what) => format!(
            "站上已找不到这条种子（回访：{what}）；下载器里那条种子还挂着该站 tracker，由人决定删不删"
        ),
        _ => String::new(),
    }
}

/// 改一条格子的结论（不落盘；调用方最后统一保存）
fn mark_cell(
    index: &mut ContentIndex,
    key: &str,
    site: &str,
    status: CoverageStatus,
    note: &str,
) -> bool {
    let Some(entry) = index.contents.get_mut(key) else {
        return false;
    };
    let Some(cell) = entry.coverage.get_mut(site) else {
        return false;
    };
    cell.status = status;
    cell.at = now_unix();
    cell.note = Some(note.to_string());
    cell.verify_hash = None;
    true
}

/// 审核结束后的收尾结果
#[derive(Debug, Clone, PartialEq, Eq)]
enum SettleKind {
    /// 站点种子下回来挂进了 qB（后面是写进格子的状态）
    Pulled(CoverageStatus),
    /// 下载器里本来就有这条站点种子（后面是写进格子的状态）
    Already(CoverageStatus),
    /// 演练：下载器里没有，正式回访才会挂回
    WouldPull,
}

/// 审核结束了：确认站点种子在 qB 里做种，不在就下回来挂进去。
#[allow(clippy::too_many_arguments)] // 内部管道：挂回需要的全部共享件
fn settle(
    cfg: &Config,
    qb: &QBittorrent,
    tgt: &NexusPhp,
    throttle: &SiteThrottle,
    key: &str,
    site: &str,
    url: &str,
    page: &TorrentPage,
    index: &mut ContentIndex,
    dry_run: bool,
    log: &mut dyn FnMut(&str),
) -> Result<SettleKind> {
    // 下载站点种子也是一次站点请求：照该站的间隔排队、占一份额度
    if !throttle.acquire(site, BudgetKind::Search) {
        anyhow::bail!("这一轮没领到请求额度，站点种子没下载，下次再看");
    }
    let tdata = tgt.download_torrent(page, url)?;
    let hash = bencode::info_hash_v1(&tdata)
        .context("站点种子缺少 v1 info hash，没法确认下载器里有没有它")?;
    let in_qb = qb
        .torrent_progress(&hash)
        .with_context(|| format!("查下载器里有没有站点种子 {hash} 失败"))?;
    let Some((progress, _state)) = in_qb else {
        if dry_run {
            log(&format!(
                "审核结束·{site}：站点种子下载器里还没有，正式回访时会下回来挂进 qB"
            ));
            return Ok(SettleKind::WouldPull);
        }
        let (status, _) = pull_with_data(cfg, qb, site, url, Some(key), &tdata, index, log)?;
        log(&format!(
            "审核结束·{site}：站点种子已下回来挂进 qB（{}）",
            status.label()
        ));
        return Ok(SettleKind::Pulled(status));
    };
    let (status, what) = if progress >= 0.999 {
        (CoverageStatus::Reseeded, "已在做种")
    } else {
        (CoverageStatus::Verifying, "下载器里正在校验")
    };
    log(&format!(
        "审核结束·{site}：这条站点种子下载器里本来就有，{what}"
    ));
    if !dry_run {
        if let Some(entry) = index.contents.get_mut(key) {
            if let Some(cell) = entry.coverage.get_mut(site) {
                cell.status = status;
                cell.at = now_unix();
                cell.verify_hash = (status == CoverageStatus::Verifying).then(|| hash.clone());
                cell.note = Some(format!(
                    "站点审核已结束（页面不再有审核标记）；这条站点种子下载器里本来就有，{what}"
                ));
            }
        }
    }
    Ok(SettleKind::Already(status))
}

/// 回访一遍：逐条打开详情页，按 [`decide`] 改结论或收尾。返回清点结果。
///
/// 每条最多两次站点请求（详情页、站点种子；节流按每站间隔排队），不上传、不发帖。
/// 演练（`dry_run`）照样打开详情页、下载站点种子来看结论，但不改覆盖记录、不往下载器里加东西。
pub fn recheck_reviews(
    cfg: &Config,
    qb: &QBittorrent,
    index: &mut ContentIndex,
    opts: &ReviewCheckOptions,
    log: &mut dyn FnMut(&str),
) -> Result<ReviewCheckSummary> {
    let mut summary = ReviewCheckSummary::default();

    // 选条目：已记审核结论的都看；all_posted 再带上正常做种/已转种的已发布条目。
    // 没有详情页链接的没法回访，跳过。
    let mut items: Vec<(String, String, String, CoverageStatus)> = Vec::new();
    for (key, entry) in &index.contents {
        for (site, cell) in &entry.coverage {
            let url = cell.url.as_deref().filter(|u| !u.is_empty());
            if !should_visit(cell.status, url.is_some(), opts) {
                continue;
            }
            let Some(url) = url else {
                continue;
            };
            if let Some(sites) = &opts.sites {
                if !sites.contains(site) {
                    continue;
                }
            }
            items.push((key.clone(), site.clone(), url.to_string(), cell.status));
        }
    }
    // 等审核的排前面：它们审完了才有活干
    items.sort_by_key(|(key, site, _, status)| {
        let rank = match status {
            CoverageStatus::PendingReview => 0,
            s if s.is_review_related() => 1,
            _ => 2,
        };
        (rank, key.clone(), site.clone())
    });
    if let Some(limit) = opts.limit {
        items.truncate(limit);
    }
    summary.selected = items.len();
    if items.is_empty() {
        return Ok(summary);
    }

    // 回访自己一份节流：每站间隔照配置走，预算按这次要看的条数给足（每条最多 2 次请求）
    let throttle = SiteThrottle::from_coverage(&cfg.coverage);
    throttle.add_budget(BudgetKind::Search, items.len() as i32 * 2 + 8);
    let known: HashSet<&str> = cfg.sites.iter().map(|s| s.id.as_str()).collect();
    let mut dirty = false;

    for (key, site, url, current) in items {
        let name = index
            .contents
            .get(&key)
            .map(|e| e.name.clone())
            .unwrap_or_default();
        if !known.contains(site.as_str()) {
            log(&format!("跳过 {site}：配置里没有这个站点（{name}）"));
            summary.errors += 1;
            continue;
        }
        let tgt = match adapter_for(cfg, &site) {
            Ok(t) => t,
            Err(e) => {
                log(&format!("跳过 {site}：站点用不了（{e:#}）"));
                summary.errors += 1;
                continue;
            }
        };
        tgt.log_proxy(log);
        if !throttle.acquire(&site, BudgetKind::Search) {
            log(&format!("{site}：这一轮没领到请求额度，下次再看（{name}）"));
            summary.no_budget += 1;
            continue;
        }
        let fetched = tgt.fetch_torrent_page_marked(&url);
        summary.checked += 1;
        let seen = match &fetched {
            Ok((_, Some((ReviewMark::Pending, why)))) => Seen::Pending(why.clone()),
            Ok((_, Some((ReviewMark::Rejected, why)))) => Seen::Rejected(why.clone()),
            Ok((_, Some((ReviewMark::Approved, why)))) => Seen::Approved(why.clone()),
            Ok((_, None)) => Seen::Clear,
            Err(e) => match e.downcast_ref::<NoSuchTorrent>() {
                Some(ns) => Seen::Gone(ns.what.clone()),
                None => Seen::Unreachable(format!("{e:#}")),
            },
        };
        match decide(current, &seen) {
            Action::Keep => match &seen {
                Seen::Pending(_) => summary.still_pending += 1,
                Seen::Rejected(_) | Seen::Gone(_) => summary.still_bad += 1,
                Seen::Clear | Seen::Approved(_) => summary.clean += 1,
                Seen::Unreachable(e) => {
                    log(&format!(
                        "打不开详情页，不算结论，格子不动（{name} → {site}）：{e}"
                    ));
                    summary.errors += 1;
                }
            },
            Action::Mark(status) => {
                let note = note_for(current, &seen);
                log(&format!("{}（{name} → {site}）：{}", status.label(), note));
                match status {
                    CoverageStatus::PendingReview => summary.marked_pending += 1,
                    CoverageStatus::ReviewRejected => summary.marked_rejected += 1,
                    CoverageStatus::GoneFromSite => summary.marked_gone += 1,
                    _ => {}
                }
                if !opts.dry_run && mark_cell(index, &key, &site, status, &note) {
                    dirty = true;
                }
            }
            Action::Settle => {
                let Ok((page, _)) = &fetched else {
                    summary.errors += 1;
                    continue;
                };
                match settle(
                    cfg,
                    qb,
                    &tgt,
                    &throttle,
                    &key,
                    &site,
                    &url,
                    page,
                    index,
                    opts.dry_run,
                    log,
                ) {
                    Ok(SettleKind::WouldPull) => summary.would_settle += 1,
                    Ok(SettleKind::Pulled(_)) => {
                        summary.settled += 1;
                        dirty = true;
                    }
                    Ok(SettleKind::Already(_)) => {
                        summary.settled += 1;
                        dirty |= !opts.dry_run;
                    }
                    Err(e) => {
                        log(&format!(
                            "审核结束了，但没能确认下载器里的做种情况，格子先不动（{name} → {site}）：{e:#}"
                        ));
                        summary.errors += 1;
                    }
                }
            }
        }
    }
    if dirty {
        index.save()?;
    }
    Ok(summary)
}

/// 发布后做种核验的范围和方式
#[derive(Debug, Clone, Default)]
pub struct SeedingCheckOptions {
    /// 只看和报告：不改覆盖记录
    pub dry_run: bool,
    /// 只核这些站点（站点 id）；空 = 全部
    pub sites: Option<HashSet<String>>,
    /// 最多核多少条；0 = 这轮不核
    pub limit: Option<usize>,
}

/// 一次做种核验的清点
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SeedingCheckSummary {
    /// 选出来要核的条数
    pub selected: usize,
    /// 真的查过下载器 / 详情页的条数
    pub checked: usize,
    /// 站点同伴表看到当前账号在做种，已确认
    pub confirmed: usize,
    /// 还留在“校验中”（tracker 没回包、同伴表没看到自己、本地还在校验）
    pub pending: usize,
    /// 站点详情页已经没有这条种子
    pub gone: usize,
    /// 下载器里找不到挂回种子，或挂回种子已不在
    pub missing: usize,
    /// 页面 / tracker / 同伴表打不开，没法定论
    pub errors: usize,
    /// 站点节流没领到号，这一轮没看
    pub no_budget: usize,
}

/// 这条格子要不要做发布后做种核验：只看自己发出去并挂回过的种子。
/// `Seeding` 是扫描按下载器 tracker 写的（可能成千上万条），不在这条路上碰。
pub fn should_check_seeding(status: CoverageStatus) -> bool {
    matches!(
        status,
        CoverageStatus::Reseeded | CoverageStatus::Assisted | CoverageStatus::Verifying
    )
}

/// qB 当前状态是不是“正在做种”。`progress=1` 也可能是暂停、排队或重新校验，
/// 这些状态不能算正在做种；只有真正处于上传/做种态才继续看 tracker 和同伴表。
fn is_active_seeding_state(state: &str) -> bool {
    matches!(state, "uploading" | "stalledUP" | "forcedUP")
}

/// 只保留挂在目标站 tracker 上的条目；qB 里可能还挂着别的 tracker，不能拿别站的
/// `status=2` 冒充目标站已回包。
fn target_tracker_infos(infos: &[TrackerInfo], adapter: &NexusPhp) -> Vec<TrackerInfo> {
    infos
        .iter()
        .filter(|t| {
            Url::parse(&t.url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_string()))
                .is_some_and(|host| {
                    adapter
                        .announce_hosts
                        .iter()
                        .any(|site_host| host_matches(&host, site_host))
                })
        })
        .cloned()
        .collect()
}

/// 已确认做种时回到哪个状态：跨站补挂回「已辅种」，其余（新发种、校验中）回「已转种」。
fn confirmed_status(current: CoverageStatus, posted_here: bool) -> CoverageStatus {
    if current == CoverageStatus::Assisted || !posted_here {
        CoverageStatus::Assisted
    } else {
        CoverageStatus::Reseeded
    }
}

/// 写一条做种核验结论。状态没变时不改 `at`（`at` 是结论时间），只更新
/// `last_seed_check`，让下一轮按最久没看过的顺序轮转。
fn write_seed_cell(
    index: &mut ContentIndex,
    key: &str,
    site: &str,
    status: CoverageStatus,
    note: &str,
    verify_hash: Option<&str>,
    now: i64,
) -> bool {
    let Some(entry) = index.contents.get_mut(key) else {
        return false;
    };
    let Some(cell) = entry.coverage.get_mut(site) else {
        return false;
    };
    if cell.status != status {
        cell.at = now;
    }
    cell.status = status;
    cell.note = Some(note.to_string());
    cell.verify_hash = verify_hash.map(|s| s.to_string());
    cell.last_seed_check = Some(now);
    true
}

/// 回访一遍：逐条核对“已转种 / 已辅种 / 校验中”的种子，看看现在还在不在做种。
///
/// 证据顺序：先看 qB 里那条挂回种子的 progress 和目标站 tracker 是否回包；
/// tracker 确认后，再打开站点详情页和 `viewpeerlist.php`，看当前账号在不在做种者列表里。
/// 只有“qB 校验 100% + 目标站 tracker 回包 + 站点同伴表看到当前账号”才算确认做种。
/// 同伴表打不开、认不出当前账号、或只看到别人，都不能写成已做种。
pub fn check_seeding(
    cfg: &Config,
    qb: &QBittorrent,
    index: &mut ContentIndex,
    opts: &SeedingCheckOptions,
    log: &mut dyn FnMut(&str),
) -> Result<SeedingCheckSummary> {
    let mut summary = SeedingCheckSummary::default();
    let limit = opts.limit.unwrap_or(20);
    if limit == 0 {
        return Ok(summary);
    }
    let mut items: Vec<(String, String, String, CoverageStatus, Option<i64>, i64)> = Vec::new();
    for (key, entry) in &index.contents {
        for (site, cell) in &entry.coverage {
            if !should_check_seeding(cell.status) {
                continue;
            }
            let Some(url) = cell.url.as_deref().filter(|u| !u.is_empty()) else {
                continue;
            };
            if opts
                .sites
                .as_ref()
                .is_some_and(|sites| !sites.contains(site))
            {
                continue;
            }
            items.push((
                key.clone(),
                site.clone(),
                url.to_string(),
                cell.status,
                cell.last_seed_check,
                cell.at,
            ));
        }
    }
    // 最久没核过的排前面；同一时间按结论时间、内容键稳定排序。
    items.sort_by_key(|(key, site, _, _, checked, at)| {
        (checked.unwrap_or(0), *at, key.clone(), site.clone())
    });
    items.truncate(limit);
    summary.selected = items.len();
    if items.is_empty() {
        return Ok(summary);
    }

    let throttle = SiteThrottle::from_coverage(&cfg.coverage);
    // 每条最多详情页 + 同伴表两次站点请求
    throttle.add_budget(BudgetKind::Search, items.len() as i32 * 2 + 8);
    let known: HashSet<&str> = cfg.sites.iter().map(|s| s.id.as_str()).collect();
    let mut dirty = false;

    for (key, site, url, current, _last, _at) in items {
        let name = index
            .contents
            .get(&key)
            .map(|e| e.name.clone())
            .unwrap_or_default();
        if !known.contains(site.as_str()) {
            log(&format!("跳过 {site}：配置里没有这个站点（{name}）"));
            summary.errors += 1;
            continue;
        }
        let tgt = match adapter_for(cfg, &site) {
            Ok(t) => t,
            Err(e) => {
                log(&format!("跳过 {site}：站点用不了（{e:#}）"));
                summary.errors += 1;
                continue;
            }
        };
        tgt.log_proxy(log);
        if !throttle.acquire(&site, BudgetKind::Search) {
            log(&format!(
                "{site}：这一轮没领到请求额度，做种核验下次再看（{name}）"
            ));
            summary.no_budget += 1;
            continue;
        }
        let fetched = tgt.fetch_torrent_page_marked(&url);
        summary.checked += 1;
        let (page, review) = match fetched {
            Ok(v) => v,
            Err(e) => {
                if e.downcast_ref::<NoSuchTorrent>().is_some() {
                    let note = format!(
                        "站上已找不到这条种子（做种核验：{}）；qB 里的残留由人决定删不删",
                        e
                    );
                    log(&format!("站上已找不到（{name} → {site}）"));
                    if !opts.dry_run {
                        dirty |= write_seed_cell(
                            index,
                            &key,
                            &site,
                            CoverageStatus::GoneFromSite,
                            &note,
                            None,
                            now_unix(),
                        );
                    }
                    summary.gone += 1;
                } else {
                    log(&format!(
                        "打不开详情页，做种核验不算结论，格子不动（{name} → {site}）：{e:#}"
                    ));
                    summary.errors += 1;
                }
                continue;
            }
        };
        if let Some((mark, why)) = review {
            let (status, note) = match mark {
                ReviewMark::Pending => (
                    CoverageStatus::PendingReview,
                    format!("站上已发布，等站点审核（做种核验：站点页面还标着“{why}”）"),
                ),
                ReviewMark::Rejected => (
                    CoverageStatus::ReviewRejected,
                    format!(
                        "站点审核没过（做种核验：{why}）；改简介或种子后在站上处理，程序不重发"
                    ),
                ),
                ReviewMark::Approved => {
                    let note = format!("站点审核已通过（做种核验：{why}）");
                    (current, note)
                }
            };
            if status != current {
                log(&format!("{}（{name} → {site}）：{note}", status.label()));
                if !opts.dry_run {
                    dirty |= write_seed_cell(index, &key, &site, status, &note, None, now_unix());
                }
                summary.pending += 1;
                continue;
            }
        }

        let (hash, posted_here) = {
            let Some(entry) = index.contents.get(&key) else {
                summary.errors += 1;
                continue;
            };
            let cell = entry.coverage.get(&site);
            let posted_here = match current {
                CoverageStatus::Reseeded => true,
                CoverageStatus::Verifying => cell
                    .and_then(|c| c.note.as_deref())
                    .is_none_or(|n| verifying_was_posted_here(n) || !n.contains("补挂")),
                CoverageStatus::Assisted => false,
                _ => true,
            };
            let hash = cell
                .and_then(|c| c.verify_hash.clone())
                .filter(|h| !h.is_empty())
                .or_else(|| verify_hash_from_entry(entry, &tgt));
            (hash, posted_here)
        };
        let Some(hash) = hash else {
            let note = "下载器里找不到挂着该站 tracker 的种子；站点端是否在做种要人工核对";
            log(&format!("{note}（{name} → {site}）"));
            if !opts.dry_run {
                dirty |= write_seed_cell(
                    index,
                    &key,
                    &site,
                    CoverageStatus::Verifying,
                    note,
                    None,
                    now_unix(),
                );
            }
            summary.missing += 1;
            continue;
        };

        let progress = match qb.torrent_progress(&hash) {
            Ok(Some((progress, state))) => (progress, state),
            Ok(None) => {
                let note =
                    "下载器里已找不到这条挂回种子（可能被手动删除）；站点端是否还在做种要人工核对";
                log(&format!("{note}（{name} → {site}）"));
                if !opts.dry_run {
                    dirty |= write_seed_cell(
                        index,
                        &key,
                        &site,
                        CoverageStatus::Verifying,
                        note,
                        None,
                        now_unix(),
                    );
                }
                summary.missing += 1;
                continue;
            }
            Err(e) => {
                log(&format!(
                    "查下载器进度失败，做种核验格子不动（{name} → {site}）：{e:#}"
                ));
                summary.errors += 1;
                continue;
            }
        };
        let (progress, state) = progress;
        if progress < 0.999 {
            let note = format!(
                "本地校验到 {:.0}%，还没确认站点看见做种；下一轮复核（下载器状态：{state}）",
                progress * 100.0
            );
            log(&format!("校验中（{name} → {site}）：{note}"));
            if !opts.dry_run {
                dirty |= write_seed_cell(
                    index,
                    &key,
                    &site,
                    CoverageStatus::Verifying,
                    &note,
                    Some(&hash),
                    now_unix(),
                );
            }
            summary.pending += 1;
            continue;
        }
        if !is_active_seeding_state(&state) {
            let note = format!("本地校验到 100%，但下载器状态是 {state}，不是正在做种；下一轮复核");
            log(&format!("没在做种（{name} → {site}）：{note}"));
            if !opts.dry_run {
                dirty |= write_seed_cell(
                    index,
                    &key,
                    &site,
                    CoverageStatus::Verifying,
                    &note,
                    Some(&hash),
                    now_unix(),
                );
            }
            summary.pending += 1;
            continue;
        }

        let (tracker_confirmed, tracker_note) = match qb.tracker_info(&hash) {
            Ok(infos) => {
                let target = target_tracker_infos(&infos, &tgt);
                (
                    target.iter().any(|t| t.status == 2),
                    if target.is_empty() {
                        "qB 里没有该站 tracker".to_string()
                    } else {
                        tracker_status_note(&target)
                    },
                )
            }
            Err(e) => (false, format!("tracker 状态读不到：{e}")),
        };
        if !tracker_confirmed {
            let note = format!("校验到 100%，但{tracker_note}；还没确认站点看见做种，下一轮复核");
            log(&format!("等待 tracker 回包（{name} → {site}）：{note}"));
            if !opts.dry_run {
                dirty |= write_seed_cell(
                    index,
                    &key,
                    &site,
                    CoverageStatus::Verifying,
                    &note,
                    Some(&hash),
                    now_unix(),
                );
            }
            summary.pending += 1;
            continue;
        }

        if page.current_uid.is_none() && page.current_name.is_none() {
            let note = format!(
                "校验到 100%，{tracker_note}；但页面上认不出当前账号，没法核对站点同伴表，下一轮复核"
            );
            log(&format!("认不出当前账号（{name} → {site}）：{note}"));
            if !opts.dry_run {
                dirty |= write_seed_cell(
                    index,
                    &key,
                    &site,
                    CoverageStatus::Verifying,
                    &note,
                    Some(&hash),
                    now_unix(),
                );
            }
            summary.pending += 1;
            continue;
        }

        if !throttle.acquire(&site, BudgetKind::Search) {
            log(&format!(
                "{site}：同伴表没领到请求额度，做种核验下次再看（{name}）"
            ));
            summary.no_budget += 1;
            continue;
        }
        let peers = match tgt.fetch_peerlist(&url, &page) {
            Ok(p) => p,
            Err(e) => {
                let note =
                    format!("校验到 100%，{tracker_note}；但站点同伴表打不开（{e:#}），下一轮复核");
                log(&format!("同伴表打不开（{name} → {site}）：{note}"));
                if !opts.dry_run {
                    dirty |= write_seed_cell(
                        index,
                        &key,
                        &site,
                        CoverageStatus::Verifying,
                        &note,
                        Some(&hash),
                        now_unix(),
                    );
                }
                summary.pending += 1;
                summary.errors += 1;
                continue;
            }
        };
        if peers.self_seeding {
            let status = confirmed_status(current, posted_here);
            let note = format!(
                "校验到 100%，{tracker_note}；站点同伴表看到当前账号在做种（{} 个做种者），已确认",
                peers.seeders.unwrap_or(0)
            );
            log(&format!("{}（{name} → {site}）：{note}", status.label()));
            if !opts.dry_run {
                dirty |= write_seed_cell(index, &key, &site, status, &note, None, now_unix());
            }
            summary.confirmed += 1;
        } else {
            let note = format!(
                "校验到 100%，{tracker_note}；但站点同伴表没看到当前账号（{} 个做种者），可能统计延迟，下一轮复核",
                peers.seeders.unwrap_or(0)
            );
            log(&format!("同伴表未见自己（{name} → {site}）：{note}"));
            if !opts.dry_run {
                dirty |= write_seed_cell(
                    index,
                    &key,
                    &site,
                    CoverageStatus::Verifying,
                    &note,
                    Some(&hash),
                    now_unix(),
                );
            }
            summary.pending += 1;
        }
    }
    if dirty {
        index.save()?;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeding_scope_is_only_our_posted_statuses() {
        for st in [
            CoverageStatus::Reseeded,
            CoverageStatus::Assisted,
            CoverageStatus::Verifying,
        ] {
            assert!(should_check_seeding(st), "{st:?}");
        }
        for st in [
            CoverageStatus::Seeding,
            CoverageStatus::PendingReview,
            CoverageStatus::Miss,
            CoverageStatus::Failed,
        ] {
            assert!(!should_check_seeding(st), "{st:?}");
        }
    }

    #[test]
    fn only_active_seeding_states_count() {
        for state in ["uploading", "stalledUP", "forcedUP"] {
            assert!(is_active_seeding_state(state), "{state}");
        }
        for state in [
            "pausedUP",
            "stoppedUP",
            "queuedUP",
            "checkingUP",
            "downloading",
            "error",
            "missingFiles",
            "",
        ] {
            assert!(!is_active_seeding_state(state), "{state}");
        }
    }

    #[test]
    fn confirmed_status_keeps_assisted_and_marks_new_posts_reseeded() {
        assert_eq!(
            confirmed_status(CoverageStatus::Assisted, false),
            CoverageStatus::Assisted
        );
        assert_eq!(
            confirmed_status(CoverageStatus::Verifying, false),
            CoverageStatus::Assisted
        );
        assert_eq!(
            confirmed_status(CoverageStatus::Verifying, true),
            CoverageStatus::Reseeded
        );
        assert_eq!(
            confirmed_status(CoverageStatus::Reseeded, true),
            CoverageStatus::Reseeded
        );
    }

    #[test]
    fn only_review_cells_by_default_and_our_posts_with_all() {
        let plain = ReviewCheckOptions::default();
        let all = ReviewCheckOptions {
            all_posted: true,
            ..Default::default()
        };
        // 记了审核结论的一定看（得有详情页链接）
        for st in [
            CoverageStatus::PendingReview,
            CoverageStatus::ReviewRejected,
            CoverageStatus::GoneFromSite,
        ] {
            assert!(should_visit(st, true, &plain), "{st:?}");
            assert!(!should_visit(st, false, &plain), "{st:?} 没链接看不了");
        }
        // 自己发的：默认不看，--all 才看
        for st in [CoverageStatus::Reseeded, CoverageStatus::Verifying] {
            assert!(!should_visit(st, true, &plain), "{st:?}");
            assert!(should_visit(st, true, &all), "{st:?}");
        }
        // 带详情页链接的“已在做种”（手工挂回的旧条目）--all 时看；扫描写的（没链接）不看
        assert!(!should_visit(CoverageStatus::Seeding, true, &plain));
        assert!(should_visit(CoverageStatus::Seeding, true, &all));
        assert!(!should_visit(CoverageStatus::Seeding, false, &all));
        // 和我们发的无关的不看
        for st in [
            CoverageStatus::Failed,
            CoverageStatus::Miss,
            CoverageStatus::Assisted,
            CoverageStatus::PendingOffers,
        ] {
            assert!(!should_visit(st, true, &all), "{st:?}");
        }
    }

    #[test]
    fn unreachable_never_changes_a_cell() {
        for cur in [
            CoverageStatus::PendingReview,
            CoverageStatus::ReviewRejected,
            CoverageStatus::GoneFromSite,
            CoverageStatus::Reseeded,
        ] {
            assert_eq!(
                decide(cur, &Seen::Unreachable("cookie 失效".into())),
                Action::Keep,
                "{cur:?}"
            );
        }
    }

    #[test]
    fn same_conclusion_is_kept() {
        assert_eq!(
            decide(CoverageStatus::PendingReview, &Seen::Pending("未审".into())),
            Action::Keep
        );
        assert_eq!(
            decide(
                CoverageStatus::ReviewRejected,
                &Seen::Rejected("缺图".into())
            ),
            Action::Keep
        );
        assert_eq!(
            decide(
                CoverageStatus::GoneFromSite,
                &Seen::Gone("详情页写：没有该ID的种子".into())
            ),
            Action::Keep
        );
    }

    #[test]
    fn new_marks_replace_old_ones() {
        // 发种时记了等审核，回访发现站上已经没有这条
        assert_eq!(
            decide(
                CoverageStatus::PendingReview,
                &Seen::Gone("详情页写：没有该ID的种子".into())
            ),
            Action::Mark(CoverageStatus::GoneFromSite)
        );
        // 等审核 → 不通过
        assert_eq!(
            decide(
                CoverageStatus::PendingReview,
                &Seen::Rejected("审核不通过，原因：缺少海报".into())
            ),
            Action::Mark(CoverageStatus::ReviewRejected)
        );
        // 本来在做种的（all_posted 带进来），页面上标着未审 / 不通过
        assert_eq!(
            decide(CoverageStatus::Reseeded, &Seen::Pending("未审".into())),
            Action::Mark(CoverageStatus::PendingReview)
        );
        assert_eq!(
            decide(
                CoverageStatus::Seeding,
                &Seen::Rejected("审核不通过".into())
            ),
            Action::Mark(CoverageStatus::ReviewRejected)
        );
    }

    #[test]
    fn clear_page_settles_only_review_cells() {
        for seen in [Seen::Clear, Seen::Approved("通过".into())] {
            for cur in [
                CoverageStatus::PendingReview,
                CoverageStatus::ReviewRejected,
                CoverageStatus::GoneFromSite,
            ] {
                assert_eq!(decide(cur, &seen), Action::Settle, "{cur:?} / {seen:?}");
            }
            for cur in [
                CoverageStatus::Reseeded,
                CoverageStatus::Seeding,
                CoverageStatus::Assisted,
            ] {
                assert_eq!(decide(cur, &seen), Action::Keep, "{cur:?} / {seen:?}");
            }
        }
    }

    #[test]
    fn notes_say_whether_it_keeps_seeding() {
        let n = note_for(CoverageStatus::PendingReview, &Seen::Pending("未审".into()));
        assert!(n.contains("等站点审核") && n.contains("未审"), "{n}");
        assert!(!n.contains("仍在做种"), "{n}");

        let n = note_for(
            CoverageStatus::Seeding,
            &Seen::Rejected("审核不通过，原因：缺少海报或截图".into()),
        );
        assert!(
            n.contains("缺少海报或截图") && n.contains("仍在做种"),
            "{n}"
        );

        let n = note_for(
            CoverageStatus::PendingReview,
            &Seen::Gone("详情页写：没有该ID的种子".into()),
        );
        assert!(n.starts_with("站上已找不到这条种子"), "{n}");
        assert!(!n.contains("），回访"), "别套两层括号：{n}");
    }
}
