//! 辅种编排（方案第三、六、七、九节）：索引 → 已做种标记 → 站点 worker 池 →
//! 统一决策表 → 统一推送尾巴 → 回报。
//!
//! 并发模型（对 Swift 版最大的结构性修正）：Swift 是全串行（站点外层 × 种子内层），
//! 1,000 内容 × 20 站、每站 8s+抖动 ≈ 50 小时。这里每个站一个串行队列（保持本站节奏），
//! 全局并发上限（默认 4 站同时），墙钟时间降到最慢那一站。
//!
//! 单写者原则：索引只在主线程被修改；worker 只发事件，不碰索引。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::Result;

use crate::bencode;
use crate::config::{Config, DownloaderSettings, ReseedMode};
use crate::index::{ContentIndex, CoverageStatus, ScanDelta, SiteCoverage, SourceRecord};
use crate::matcher;
use crate::models::{JudgeLevel, Layout, LocalTorrent, TorrentFingerprint};
use crate::piecesapi::{Capability, PiecesApi, QueryOutcome};
use crate::qbittorrent::QBittorrent;
use crate::resolver::{resolve_source, ResolveOutcome, SourceSite};
use crate::sites::nexusphp::{NexusPhp, SearchOutcome};
use crate::throttle::{BudgetKind, SiteThrottle};

/// 一轮结束的报告
#[derive(Debug, Default)]
pub struct RoundReport {
    pub scanned_torrents: usize,
    pub delta: ScanDelta,
    /// 各状态的格子数（本轮产出的结论）
    pub by_status: BTreeMap<CoverageStatus, usize>,
    /// 转种草稿清单（v1 转种臂只出报告不上传）
    pub reseed_drafts: Vec<DraftItem>,
    /// 预筛跳过的格子数（零请求）
    pub preskipped: usize,
    /// 预算耗尽、本轮根本没查成的格子数（不写索引，下轮从头来）
    pub skipped_no_budget: usize,
    /// 详情链接域名不属于目标站、被拒绝写入的格子数（不写索引）
    pub url_mismatched: usize,
    /// 各预算 (已用, 上限)
    pub budget: BTreeMap<&'static str, (i32, i32)>,
}

#[derive(Debug, Clone)]
pub struct DraftItem {
    pub content_key: String,
    pub name: String,
    pub source_site: String,
    pub source_url: String,
    pub target_site: String,
}

/// 进度事件（CLI 打印 / 测试断言用）
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // 事件走 mpsc 低频传递，装箱不值当
pub enum Event {
    Log(String),
    /// 本轮没查成的格子（预算耗尽一类）：**不写索引**，下轮从头来。
    /// 与 Miss 的区别：Miss 是「查过了，站内确实没有」（可负缓存），Skipped 是「根本没查」
    ItemSkipped {
        site_id: String,
        key: String,
        reason: String,
    },
    ItemDone {
        site_id: String,
        key: String,
        status: CoverageStatus,
        url: Option<String>,
        note: String,
        /// 源种定位成功时：要写回 entry.source 的记录
        source: Option<SourceRecord>,
        /// 转种草稿
        draft: Option<DraftItem>,
        /// 上轮推送暂停在下载器里那条种子的 hash（仅 Verifying 结论携带，收尾凭据）
        verify_hash: Option<String>,
    },
}

/// 一格待处理（内容 × 站点）。要用的东西全拷贝进来，worker 不读索引
#[derive(Debug, Clone)]
struct WorkItem {
    key: String,
    name: String,
    /// 代表种子（qB 里那条）的 v1 hash——worker 用它读本地文件清单
    rep_hash: String,
    save_path: String,
    category: Option<String>,
    hashes: Vec<String>,
    tracker_hosts: Vec<String>,
    source: Option<SourceRecord>,
    forbid_reseed: bool,
}

type FingerprintCache = Mutex<HashMap<String, Arc<TorrentFingerprint>>>;
/// 本轮内的源种定位结果缓存（多个目标站同一段内容都「站内没有」时，源站只走一遍）
type SourceCache = Mutex<HashMap<String, SourceRecord>>;
/// 站点队列池：worker 从这里整站认领待办（site_id → 该站的待办队列）
type SiteQueuePool = Arc<Mutex<Vec<(String, Vec<WorkItem>)>>>;

fn now() -> i64 {
    now_unix()
}

/// 当前 unix 秒（CLI 的 scan 也用）
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// qB 状态是不是还在校验/分配（没跑完就别下结论）。
/// 5.x 的状态名：`checkingDL` / `checkingUP` / `queuedForChecking` / `metaDL`——
/// 少一个都会把「还在排队校验」误判成「校验不过」，把好种子删掉。
fn is_checking(state: &str) -> bool {
    matches!(
        state,
        "checking"
            | "checkingFiles"
            | "checkingResumeData"
            | "checkingDL"
            | "checkingUP"
            | "queuedForChecking"
            | "allocating"
            | "metadownload"
            | "metaDL"
    )
}

/// 一轮的运行选项
#[derive(Debug, Clone, Default)]
pub struct RoundOptions {
    /// 最多处理多少格（小规模试跑用）
    pub limit: Option<usize>,
    /// 只跑这些站（id 过滤；None = 全部）
    pub site_filter: Option<HashSet<String>>,
    /// 预演：检索照跑、推送只出「可辅」结论不真加下载器
    pub dry_run: bool,
    /// 是否把结论落盘（plan 预演不落，避免污染索引）
    pub persist: bool,
    /// 只处理这些内容（contentKey 前缀或名称包含，实机调试/单条重试用）
    pub content_filter: Option<String>,
}

/// 把一条 ItemDone 写进索引。详情链接域名必须属于目标站（草稿格子存的是源种链接，
/// 按既有语义不校验）；域名不匹配返回 Err，调用方不得落格。
#[allow(clippy::too_many_arguments)]
fn insert_done_cell(
    index: &mut ContentIndex,
    cfg: &Config,
    site_id: &str,
    key: &str,
    status: CoverageStatus,
    url: Option<&str>,
    note: &str,
    source: Option<&SourceRecord>,
    verify_hash: Option<&str>,
    now: i64,
) -> Result<()> {
    if status != CoverageStatus::ReseedDraft {
        if let Some(u) = url {
            cfg.validate_coverage_url(site_id, u)?;
        }
    }
    if let Some(src) = source {
        if let Some(e) = index.contents.get_mut(key) {
            e.source = Some(src.clone());
        }
    }
    if let Some(e) = index.contents.get_mut(key) {
        e.coverage.insert(
            site_id.to_string(),
            SiteCoverage {
                status,
                at: now,
                url: url.map(|s| s.to_string()),
                note: if note.is_empty() {
                    None
                } else {
                    Some(note.to_string())
                },
                verify_hash: if status == CoverageStatus::Verifying {
                    verify_hash.map(|s| s.to_string())
                } else {
                    None
                },
            },
        );
    }
    Ok(())
}

/// 跑一整轮：扫描 → 合并索引 → 预筛 → 站点 worker 池 → 应用结论 → 落盘。
/// `adapters` 按（过滤后的）`cfg.sites` 顺序给每个站一个适配器（同 id 一一对应）。
pub fn run_round(
    cfg: &Config,
    qb: Arc<QBittorrent>,
    adapters: &[Arc<NexusPhp>],
    index: &mut ContentIndex,
    mut emit: impl FnMut(Event),
    opts: RoundOptions,
) -> Result<RoundReport> {
    // 跨进程互斥：GUI 与 CLI 同时跑轮会互相覆盖 coverage.json（后写覆盖先写）
    let data_dir = cfg
        .coverage_state_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let _lock = crate::lock::RunLock::acquire(&data_dir, "state")?;
    let mut report = RoundReport::default();
    let t0 = now();

    // ---- 1 扫描 + 增量合并 ----
    let torrents = qb.list_torrents()?;
    report.scanned_torrents = torrents.len();
    // 首扫描（或索引里还没有摘要的种子）要把文件清单抓回来算 manifest。qB 一次一条，
    // 4 万条串行十几分钟，这里按 scanConcurrency 并发；已有摘要的种子增量跳过。
    let files = prefetch_files(
        qb.as_ref(),
        &torrents,
        index,
        cfg.coverage.scan_concurrency,
        |done, total| {
            if total >= 500 && (done % 1000 == 0 || done == total) {
                emit(Event::Log(format!("读取文件清单 {done}/{total}")));
            }
        },
    );
    let files_of = |t: &LocalTorrent| files.get(&t.hash).cloned();
    report.delta = index.merge_scan(&torrents, files_of, t0, cfg.coverage.archive_days);
    // 文件清单（4 万条约 200MB）只在合并这一次用得上，worker 阶段靠逐条指纹缓存；
    // 不释放的话常驻进程（将来的 daemon）会一直背着它
    drop(files);
    emit(Event::Log(format!(
        "扫描 {} 条种子：新内容 {}、并入 {}、移除 {}、归档 {}、跳过未完成 {}",
        torrents.len(),
        report.delta.new_contents,
        report.delta.merged,
        report.delta.removed_torrents,
        report.delta.purged_contents,
        report.delta.skipped_incomplete
    )));

    // 站点过滤（CLI --sites）
    let active_sites: Vec<&crate::config::SiteConf> = cfg
        .sites
        .iter()
        .filter(|s| opts.site_filter.as_ref().is_none_or(|f| f.contains(&s.id)))
        .collect();

    // ---- 2 已做种标记（tracker hosts ∩ 参与站点 → Seeding，零请求）----
    for entry in index.contents.values_mut() {
        for site in &active_sites {
            if !entry.tracker_hosts.iter().any(|h| site.matches_tracker(h)) {
                continue;
            }
            let cur = entry.coverage.get(&site.id).map(|c| c.status);
            // 已辅/已转/校验中/否决是更强的结论，别被覆盖。
            // 站点审核相关的三条同理：qB 里挂着该站 tracker，不代表站上还在审/没删这条种子
            // （实测熊猫 141276：qB 还挂着 tracker，站上详情页已经报“没有该ID的种子”）。
            let overwrite = !matches!(
                cur,
                Some(CoverageStatus::Assisted)
                    | Some(CoverageStatus::Reseeded)
                    | Some(CoverageStatus::Verifying)
                    | Some(CoverageStatus::Rejected)
            ) && !cur.is_some_and(|s| s.is_review_related());
            if overwrite && cur != Some(CoverageStatus::Seeding) {
                entry.coverage.insert(
                    site.id.clone(),
                    SiteCoverage {
                        status: CoverageStatus::Seeding,
                        at: t0,
                        url: None,
                        note: Some("下载器里这条种子挂着该站 tracker".to_string()),
                        verify_hash: None,
                    },
                );
            }
        }
    }

    // ---- 2b 收尾上一轮「校验中」的格子（主线程直查下载器，不花站点预算）----
    if !opts.dry_run {
        for entry in index.contents.values_mut() {
            for site in &active_sites {
                let verdict = {
                    let Some(cell) = entry.coverage.get(&site.id) else {
                        continue;
                    };
                    if cell.status != CoverageStatus::Verifying {
                        continue;
                    }
                    let Some(vh) = cell.verify_hash.as_deref().filter(|h| !h.is_empty()) else {
                        continue;
                    };
                    collect_verifying(qb.as_ref(), vh, &cfg.coverage)
                };
                if let Some((status, note)) = verdict {
                    let Some(cell) = entry.coverage.get_mut(&site.id) else {
                        continue;
                    };
                    cell.status = status;
                    cell.at = t0;
                    cell.note = Some(note.clone());
                    cell.verify_hash = None;
                    *report.by_status.entry(status).or_insert(0) += 1;
                    emit(Event::Log(format!(
                        "[{}] 收尾「{}」：{}",
                        site.id, entry.name, note
                    )));
                }
            }
        }
    }

    // ---- 3 预筛（零请求跳过）+ 入队 ----
    let miss_cutoff = t0 - cfg.coverage.miss_ttl_days * 86_400;
    let mut site_queues: HashMap<String, Vec<WorkItem>> = HashMap::new();
    let mut priority: HashMap<String, (usize, i64)> = HashMap::new();
    for (key, entry) in &index.contents {
        if entry.torrents.is_empty() {
            continue; // 全部成员已消失，归档倒计时中，不占请求
        }
        if let Some(f) = &opts.content_filter {
            if !key.starts_with(f.as_str()) && !entry.name.contains(f.as_str()) {
                continue;
            }
        }
        // 大小/分类筛选（方案第九节：把分母线性砍下来比调间隔管用）
        let gb = entry.total_size as f64 / 1_073_741_824.0;
        if cfg.coverage.min_size_gb > 0.0 && gb < cfg.coverage.min_size_gb {
            continue;
        }
        if cfg.coverage.max_size_gb > 0.0 && gb > cfg.coverage.max_size_gb {
            continue;
        }
        if !cfg.coverage.categories.is_empty() {
            let hit = entry.torrents.values().any(|m| {
                m.category
                    .as_ref()
                    .is_some_and(|c| cfg.coverage.categories.contains(c))
            });
            if !hit {
                continue;
            }
        }
        let Some((rep_hash, meta)) = entry.pick_meta() else {
            continue;
        };
        let mut uncovered = 0;
        for site in &active_sites {
            let skip = match entry.coverage.get(&site.id).map(|c| c.status) {
                // 终态（含 verifying——校验中格子由收尾 pass 负责）零请求跳过
                Some(s) if s.is_settled() => true,
                // miss/待找源/待人工/失败：missTTL 内不重试。
                // 失败也歇一个 TTL——校验回滚类失败若每轮重推重删就是预算黑洞；
                // C 级待人工同样：站点后来上了正确版本，7 天后自然重查
                Some(_) => entry.coverage[&site.id].at > miss_cutoff,
                _ => false,
            };
            if skip {
                report.preskipped += 1;
                continue;
            }
            let item = WorkItem {
                key: key.clone(),
                name: entry.name.clone(),
                rep_hash: rep_hash.clone(),
                save_path: meta.save_path.clone(),
                category: meta.category.clone(),
                hashes: entry.all_hashes(),
                tracker_hosts: entry.tracker_hosts.clone(),
                source: entry.source.clone(),
                forbid_reseed: entry.forbid_reseed,
            };
            site_queues.entry(site.id.clone()).or_default().push(item);
            uncovered += 1;
        }
        // 优先级：未覆盖站多优先、新入库优先（方案第九节）。
        // 新入库用「成员种子里最晚 added_on」：首扫描时 first_seen 全一样，
        // 拿它当次要键等于按 hash 字典序排——实机上就是随机挑，没有意义。
        priority.insert(key.clone(), (uncovered, entry.newest_added()));
    }
    // 各站队列按内容优先级排（同一站内先处理未覆盖站多的内容）
    let prio = |key: &str| priority.get(key).cloned().unwrap_or((0, 0));
    for items in site_queues.values_mut() {
        items.sort_by_key(|i| std::cmp::Reverse(prio(&i.key)));
    }
    // 一轮的检索预算撑不起无限队列：4 万内容 × 9 站 = 37 万个待办，
    // 排进来只会白占内存（每格带名字/全文 hash/tracker 表）与排序时间。
    // 按预算给每站截断——预算 200 时最多也就跑得了百来格，多的下轮再来。
    let per_site_cap = cfg.coverage.search_budget.max(1) as usize;
    for items in site_queues.values_mut() {
        items.truncate(per_site_cap);
    }
    // --limit：从最长的队列尾部开始裁，保证每个站都能分到名额
    if let Some(lim) = opts.limit {
        let mut total: usize = site_queues.values().map(|q| q.len()).sum();
        while total > lim {
            let longest = site_queues.values_mut().max_by_key(|q| q.len()).unwrap();
            longest.pop();
            total -= 1;
        }
    }

    // ---- 4 站点 worker 池 ----
    let throttle = Arc::new(SiteThrottle::from_coverage(&cfg.coverage));
    let fp_cache: Arc<FingerprintCache> = Arc::new(Mutex::new(HashMap::new()));
    let source_cache: Arc<SourceCache> = Arc::new(Mutex::new(HashMap::new()));
    let pool: SiteQueuePool = Arc::new(Mutex::new(site_queues.into_iter().collect()));
    let adapters: Arc<Vec<Arc<NexusPhp>>> = Arc::new(adapters.to_vec());
    let cov = Arc::new(cfg.coverage.clone());
    let dl = Arc::new(cfg.downloader.clone());
    let (tx, rx) = channel::<Event>();

    let n_sites = pool.lock().unwrap().len();
    let n_workers = cfg.coverage.max_concurrent_sites.max(1).min(n_sites.max(1));
    emit(Event::Log(format!(
        "开工：{} 个站、{} 格待查、并发 {} 站",
        n_sites,
        pool.lock()
            .unwrap()
            .iter()
            .map(|(_, q)| q.len())
            .sum::<usize>(),
        n_workers
    )));
    let pieces_dir = cfg
        .coverage_state_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let pieces = Arc::new(Mutex::new(PiecesApi::load(&pieces_dir, &cfg.cookies)));
    let mut handles = Vec::new();
    for _ in 0..n_workers {
        let (pool, throttle, fp_cache, source_cache, qb) = (
            pool.clone(),
            throttle.clone(),
            fp_cache.clone(),
            source_cache.clone(),
            qb.clone(),
        );
        let pieces = pieces.clone();
        let (adapters, tx, cov, dl) = (adapters.clone(), tx.clone(), cov.clone(), dl.clone());
        let dry_run = opts.dry_run;
        handles.push(thread::spawn(move || {
            worker_loop(
                pool,
                throttle,
                fp_cache,
                source_cache,
                qb,
                adapters,
                tx,
                cov,
                dl,
                dry_run,
                pieces,
            )
        }));
    }
    drop(tx); // 全部 worker 结束后 rx 迭代自然收尾

    // ---- 5 主线程：收事件 → 应用到索引（单写者）----
    for ev in rx {
        match ev {
            Event::Log(m) => emit(Event::Log(m)),
            // 预算耗尽一类：不写索引，只计入报告（下轮从头查）
            Event::ItemSkipped {
                site_id,
                key,
                reason,
            } => {
                report.skipped_no_budget += 1;
                emit(Event::ItemSkipped {
                    site_id,
                    key,
                    reason,
                });
            }
            Event::ItemDone {
                site_id,
                key,
                status,
                url,
                note,
                source,
                draft,
                verify_hash,
            } => {
                // 覆盖记录里的详情链接域名必须属于目标站；域名不匹配就整条拒绝写入，
                // 不静默存进去。草稿格子存的是源种链接，按既有语义不校验。
                if let Err(e) = insert_done_cell(
                    &mut *index,
                    cfg,
                    &site_id,
                    &key,
                    status,
                    url.as_deref(),
                    &note,
                    source.as_ref(),
                    verify_hash.as_deref(),
                    t0,
                ) {
                    report.url_mismatched += 1;
                    emit(Event::Log(format!(
                        "覆盖记录拒绝写入（{site_id} / {key}）：{e:#}"
                    )));
                    continue;
                }
                *report.by_status.entry(status).or_insert(0) += 1;
                if let Some(d) = &draft {
                    report.reseed_drafts.push(d.clone());
                }
                emit(Event::ItemDone {
                    site_id,
                    key,
                    status,
                    url,
                    note,
                    source,
                    draft,
                    verify_hash,
                });
            }
        }
    }
    for h in handles {
        if h.join().is_err() {
            emit(Event::Log("一个站 worker 异常退出（本轮继续）".to_string()));
        }
    }
    pieces.lock().unwrap().save().ok();
    for kind in [BudgetKind::Search, BudgetKind::Source, BudgetKind::Upload] {
        report.budget.insert(
            kind.label(),
            (throttle.used(kind), throttle.budget_max(kind)),
        );
    }
    if opts.persist {
        index.save()?;
    }
    Ok(report)
}

// ---------- worker ----------

#[allow(clippy::too_many_arguments)] // 内部管道：worker 需要的全部共享件
fn worker_loop(
    pool: SiteQueuePool,
    throttle: Arc<SiteThrottle>,
    fp_cache: Arc<FingerprintCache>,
    source_cache: Arc<SourceCache>,
    qb: Arc<QBittorrent>,
    adapters: Arc<Vec<Arc<NexusPhp>>>,
    tx: Sender<Event>,
    cov: Arc<crate::config::CoverageSettings>,
    dl: Arc<DownloaderSettings>,
    dry_run: bool,
    pieces: Arc<Mutex<PiecesApi>>,
) {
    loop {
        // 认领一个站的队列（整站串行，保持本站节奏）
        let next = { pool.lock().unwrap().pop() };
        let Some((site_id, items)) = next else { break };
        let Some(adapter) = adapters.iter().find(|a| a.site_id == site_id) else {
            // 理论不可达（队列按 adapters 建），真发生了也别静默丢一整站的工作
            let _ = tx.send(Event::Log(format!(
                "[{site_id}] 队列没有对应适配器，本轮跳过该站（理论不可达）"
            )));
            continue;
        };
        let hits = prefetch_pieces(&site_id, adapter, &qb, &pieces, &items, &throttle);
        for item in items {
            let ev = process_item(
                &item,
                &site_id,
                adapter,
                &qb,
                &throttle,
                &fp_cache,
                &source_cache,
                &adapters,
                &cov,
                &dl,
                dry_run,
                &hits,
                &pieces,
            );
            let _ = tx.send(ev);
        }
    }
}

/// 源站定位的 pieces-hash 快通道：本地代表种子 → pieces hash → 源站 /api/pieces-hash
/// → torrent id → 详情页 → .torrent。只有 SameHash/SameFiles 才算同源。
/// 命中后调用方直接出草稿；未命中返回 None，由名字检索继续兜底。
#[allow(clippy::too_many_arguments)]
fn resolve_source_by_pieces(
    item: &WorkItem,
    qb: &QBittorrent,
    adapters: &[Arc<NexusPhp>],
    pieces: &Arc<Mutex<PiecesApi>>,
    fp: &TorrentFingerprint,
    throttle: &SiteThrottle,
    target_site: &str,
) -> Option<(String, String, String)> {
    let ph = {
        let mut api = pieces.lock().unwrap();
        api.pieces_hash_for(qb, &item.rep_hash)?
    };
    for a in source_adapters(adapters, target_site, &item.tracker_hosts) {
        // 能力表明确不支持的站不浪费请求；Unknown 允许试一次（站点会回 404/405）
        if let Some(api) = pieces.try_lock().ok().as_ref() {
            if api.capability(&a.site_id) == Capability::Unsupported {
                continue;
            }
        }
        if !throttle.acquire(&a.site_id, BudgetKind::Source) {
            continue;
        }
        let hit = {
            let mut api = pieces.lock().unwrap();
            let mut out = api.query(&a.site_id, a.base.as_str(), std::slice::from_ref(&ph));
            if out == QueryOutcome::Auth && api.harvest_passkeys(qb) > 0 {
                out = api.query(&a.site_id, a.base.as_str(), std::slice::from_ref(&ph));
            }
            match out {
                QueryOutcome::Hit(list) => {
                    list.into_iter().find(|(h, _)| h == &ph).map(|(_, id)| id)
                }
                _ => None,
            }
        };
        let Some(id) = hit else { continue };
        let detail = format!(
            "{}/details.php?id={id}",
            a.base.as_str().trim_end_matches('/')
        );
        if !throttle.acquire(&a.site_id, BudgetKind::Source) {
            continue;
        }
        let page = match a.fetch_torrent_page(&detail) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !throttle.acquire(&a.site_id, BudgetKind::Source) {
            continue;
        }
        let data = match a.download_torrent(&page, &detail) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let (level, note) = matcher::judge(&item.hashes, fp, &data, &page.name);
        match level {
            JudgeLevel::SameHash { .. } | JudgeLevel::SameFiles => {
                return Some((a.site_id.clone(), detail, note));
            }
            _ => continue,
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
/// 整站 pieces-hash 预扫：≤100 个内容一次请求换精确 torrent id 表。
/// 返回 key → (下载直链, 详情页 URL)；站点不支持/未授权/失败都返回空表（回退名字检索）。
fn prefetch_pieces(
    site_id: &str,
    adapter: &NexusPhp,
    qb: &QBittorrent,
    pieces: &Mutex<PiecesApi>,
    items: &[WorkItem],
    throttle: &SiteThrottle,
) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    let mut api = pieces.lock().unwrap();
    if api.capability(site_id) == Capability::Unsupported {
        return out;
    }
    let keyed = prefetch_keyed(items, |h, allow_export| {
        if allow_export {
            api.pieces_hash_for(qb, h)
        } else {
            api.cached_pieces_hash(h)
        }
    });
    if keyed.is_empty() {
        return out;
    }
    let base = adapter.base.as_str().trim_end_matches('/').to_string();
    let host = adapter
        .base
        .host_str()
        .unwrap_or("")
        .trim_start_matches("www.");
    // API 每请求 ≤100 项；多成员入批后按块领号、逐块查
    let mut refreshed = false;
    for chunk in keyed.chunks(100) {
        if !throttle.acquire(site_id, BudgetKind::Search) {
            return out;
        }
        let hashes: Vec<String> = chunk.iter().map(|(_, p)| p.clone()).collect();
        let mut outcome = api.query(site_id, adapter.base.as_str(), &hashes);
        if outcome == QueryOutcome::Auth {
            // 首次没有 passkey：从下载器 tracker 现采一次再试
            if api.harvest_passkeys(qb) > 0 {
                outcome = api.query(site_id, adapter.base.as_str(), &hashes);
            }
        }
        if let QueryOutcome::Hit(list) = outcome {
            // 缓存 passkey 可能过期（pieces API 仍接受、download.php 拒）：
            // 命中后从 qB tracker 定向刷新一次，下载直链才用的是当前权威值
            if !refreshed {
                api.refresh_passkey(qb, host);
                refreshed = true;
            }
            let pk = api.passkey(host).unwrap_or_default().to_string();
            for (ph, id) in list {
                if let Some((key, _)) = chunk.iter().find(|(_, p)| p == &ph) {
                    out.insert(
                        key.clone(),
                        (
                            crate::piecesapi::download_url(&base, id, &pk),
                            format!("{base}/details.php?id={id}"),
                        ),
                    );
                }
            }
        }
    }
    out
}

/// pieces 预扫候选收集：rep 的 ph（可让 qB 导出现算）+ 其余成员的**已缓存** ph
/// （只查缓存，零 qB 开销）。目标站种子的封装可能与任意成员一致而不一定是 rep——
/// sbpt 实测：rep 是 luckpt 重新打包，站内已有种与另一 CMCT 原始成员 pieces 一致，
/// 只查 rep 会漏命中。按 ph 去重；至多 100 段内容。
fn prefetch_keyed(
    items: &[WorkItem],
    mut ph_for: impl FnMut(&str, bool) -> Option<String>,
) -> Vec<(String, String)> {
    let mut keyed: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for it in items.iter().take(100) {
        if let Some(ph) = ph_for(&it.rep_hash, true) {
            if seen.insert(ph.clone()) {
                keyed.push((it.key.clone(), ph));
            }
        }
        for h in &it.hashes {
            if h.eq_ignore_ascii_case(&it.rep_hash) {
                continue;
            }
            if let Some(ph) = ph_for(h, false) {
                if seen.insert(ph.clone()) {
                    keyed.push((it.key.clone(), ph));
                }
            }
        }
    }
    keyed
}

/// Normalize a tracker announce / host string for comparison only.
///
/// This normalization is comparison-only: it never rewrites the value stored in the
/// index or sent to any site. It trims surrounding whitespace, lowercases, drops the
/// URL scheme (`http://`, `https://`, `udp://`, ...), drops path/query/fragment,
/// strips a numeric port suffix, and removes FQDN trailing dots.
///
/// - Userinfo (`user:pass@host`) is stripped: the credentials before `@` must not
///   participate in host comparison.
/// - Port stripping is skipped for bracketed IPv6 literals and for bare IPv6
///   literals (two or more colons, no brackets), so address colons are not mistaken
///   for a port separator. The bare-IPv6 branch is defensive: no bare IPv6 announce
///   showed up in the local qB sample; it keeps an address like `2001:db8::1` from
///   being truncated at its last colon segment.
/// - No IDNA / punycode conversion is performed: a Unicode domain and its `xn--`
///   ASCII form are not translated into each other. Site configs must write the
///   ASCII (punycode) host; introducing an IDNA crate is deliberately avoided.
fn normalize_tracker_host(raw: &str) -> String {
    let s = raw.trim().to_lowercase();
    // Drop scheme: keep everything after the first `://` if one is present.
    let s = match s.split_once("://") {
        Some((_, rest)) => rest,
        None => s.as_str(),
    };
    // Cut the authority off before the first path / query / fragment delimiter.
    let authority = match s.find(['/', '?', '#']) {
        Some(idx) => &s[..idx],
        None => s,
    };
    // Drop userinfo (`user:pass@host`): credentials must not reach host comparison.
    let authority = match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };
    // Strip a numeric port suffix. Bracketed IPv6 literals and bare IPv6 literals
    // (>= 2 colons) are left untouched here.
    let host = if authority.contains('[')
        || authority.contains(']')
        || authority.chars().filter(|c| *c == ':').count() >= 2
    {
        authority.to_string()
    } else {
        match authority.rfind(':') {
            Some(idx) if authority[idx + 1..].chars().all(|c| c.is_ascii_digit()) => {
                authority[..idx].to_string()
            }
            _ => authority.to_string(),
        }
    };
    // Remove one or more FQDN trailing dots.
    host.trim_end_matches('.').to_string()
}

/// qB tracker 与站点 base/announce host 的匹配（用于源站定向）。
///
/// Both sides go through [`normalize_tracker_host`] before the loose subdomain check;
/// the normalization is comparison-only and does not affect persisted data.
fn adapter_matches_tracker(a: &NexusPhp, tracker_hosts: &[String]) -> bool {
    if tracker_hosts.is_empty() {
        return true;
    }
    let mut hosts = a.announce_hosts.clone();
    if let Some(h) = a.base.host_str() {
        hosts.push(h.to_string());
    }
    let hosts: Vec<String> = hosts.iter().map(|h| normalize_tracker_host(h)).collect();
    tracker_hosts.iter().any(|t| {
        let t = normalize_tracker_host(t);
        hosts
            .iter()
            .any(|h| t == *h || t.ends_with(&format!(".{h}")) || h.ends_with(&format!(".{t}")))
    })
}

/// 源站候选：先按 qB tracker 定向（本地种子的 tracker 就是来源站），避免 92 站全扫——
/// 不可达的站每个要吃满超时，pieces 反查与名字检索两条路都必须用同一套定向。
/// tracker 没映射到任何参与站时才回退全部源站。
fn source_adapters<'a>(
    adapters: &'a [Arc<NexusPhp>],
    target_site: &str,
    tracker_hosts: &[String],
) -> Vec<&'a Arc<NexusPhp>> {
    let mut picked: Vec<&Arc<NexusPhp>> = adapters
        .iter()
        .filter(|a| a.site_id != target_site)
        .filter(|a| adapter_matches_tracker(a, tracker_hosts))
        .collect();
    if picked.is_empty() {
        picked = adapters
            .iter()
            .filter(|a| a.site_id != target_site)
            .collect();
    }
    picked
}

#[allow(clippy::too_many_arguments)]
fn process_item(
    item: &WorkItem,
    site_id: &str,
    adapter: &NexusPhp,
    qb: &QBittorrent,
    throttle: &SiteThrottle,
    fp_cache: &FingerprintCache,
    source_cache: &SourceCache,
    adapters: &[Arc<NexusPhp>],
    cov: &crate::config::CoverageSettings,
    dl: &DownloaderSettings,
    dry_run: bool,
    pieces_hits: &std::collections::HashMap<String, (String, String)>,
    pieces: &Arc<Mutex<PiecesApi>>,
) -> Event {
    let done = |status: CoverageStatus, note: String| Event::ItemDone {
        site_id: site_id.to_string(),
        key: item.key.clone(),
        status,
        url: None,
        note,
        source: None,
        draft: None,
        verify_hash: None,
    };
    let done_url = |status: CoverageStatus, note: String, url: String| Event::ItemDone {
        site_id: site_id.to_string(),
        key: item.key.clone(),
        status,
        url: Some(url),
        note,
        source: None,
        draft: None,
        verify_hash: None,
    };
    // 预算耗尽：格子本轮没查成，不能记 Miss（那是查过才有的负缓存）
    let skipped = |reason: &str| Event::ItemSkipped {
        site_id: site_id.to_string(),
        key: item.key.clone(),
        reason: reason.to_string(),
    };

    // 本地指纹（本轮缓存）：rep 拿不到再试其他成员——rep 可能已被移出 qB。
    // 全都拿不到多为瞬态（扫描期 qB WebAPI 争用）：不落 Failed（那会把辅种机会
    // 负缓存一个 TTL），本轮跳过下轮重试；成员真删光时扫描会归档内容，不会永久占队列。
    let mut fp = fingerprint_of(qb, fp_cache, &item.rep_hash);
    if fp.is_none() {
        for h in &item.hashes {
            if h.eq_ignore_ascii_case(&item.rep_hash) {
                continue;
            }
            fp = fingerprint_of(qb, fp_cache, h);
            if fp.is_some() {
                break;
            }
        }
    }
    let Some(fp) = fp else {
        return skipped("读不到本地种子的文件清单（qB 瞬态或成员已删），本轮跳过下轮重试");
    };

    // ---- pieces-hash 快速通道：预扫已给出精确 torrent id，跳过名字检索 ----
    if let Some((dl_url, detail_url)) = pieces_hits.get(&item.key) {
        if throttle.acquire(site_id, BudgetKind::Search) {
            if let Ok(data) = adapter.download_bytes(dl_url, adapter.base.as_str()) {
                throttle.note_ok(site_id);
                let (level, why) = matcher::judge(&item.hashes, &fp, &data, &item.name);
                return finish_judged(
                    item, site_id, qb, cov, dl, &data, detail_url, &fp, level, why, dry_run, done,
                    done_url,
                );
            }
        }
    }

    // ---- 检索臂（方案第六节：先检索后转种）----
    // 一次「检索」在站上可能是 3–6 发请求（查询词阶梯逐级放宽），所以预算与站间隔
    // 按请求领号（gate），而不是按 search 调用领号——否则 5 个查询会在 1 秒内连发。
    let gate_site = site_id.to_string();
    let mut gate = move || throttle.acquire(&gate_site, BudgetKind::Search);
    let mut found =
        match adapter.search_sized(&item.name, "", false, "", Some(fp.total_size), &mut gate) {
            Ok(SearchOutcome::Hit(h)) => Some(h),
            Ok(SearchOutcome::Miss) => None,
            Ok(SearchOutcome::BudgetExhausted) => return skipped("本轮检索预算用完，下次接着跑"),
            // 「检索报错」与「站内没有」是两回事：cookie 失效、站点 500 不能记成负缓存；
            // 限流（429/503）还要给该站退避，别按原节奏接着敲
            Err(e) => {
                if crate::sites::nexusphp::error_is_rate_limited(&e) {
                    throttle.note_rate_limited(site_id);
                }
                return done(CoverageStatus::Failed, format!("站内检索失败：{e:#}"));
            }
        };
    if found.is_none() && cov.relaxed_retry_search {
        match adapter.search_sized(&item.name, "", true, "", Some(fp.total_size), &mut gate) {
            Ok(SearchOutcome::Hit(h)) => found = Some(h),
            Ok(SearchOutcome::Miss) => {}
            Ok(SearchOutcome::BudgetExhausted) => {
                return skipped("本轮检索预算用完（宽松重试没跑），下次接着跑")
            }
            Err(e) => {
                if crate::sites::nexusphp::error_is_rate_limited(&e) {
                    throttle.note_rate_limited(site_id);
                }
                return done(CoverageStatus::Failed, format!("宽松检索失败：{e:#}"));
            }
        }
    }
    // 名字兜底：内容名常是合集/目录名，站点标题可能是某个成员种子的名字
    // （Coraline 实机：内容名 "Coraline (2009) - BD50..."，站点标题是成员种名）。
    // 只在多成员内容上补一次检索，且照常领号——预算/站间隔不变。
    if found.is_none() && cov.member_name_fallback && item.hashes.len() > 1 {
        let members = qb.torrent_names(&item.hashes);
        if let Some(alt) = pick_member_fallback(&item.name, &members) {
            match adapter.search_sized(&alt, "", false, "", Some(fp.total_size), &mut gate) {
                Ok(SearchOutcome::Hit(h)) => found = Some(h),
                Ok(SearchOutcome::Miss) => {}
                Ok(SearchOutcome::BudgetExhausted) => {
                    return skipped("本轮检索预算用完（成员名兜底没跑），下次接着跑")
                }
                Err(e) => {
                    if crate::sites::nexusphp::error_is_rate_limited(&e) {
                        throttle.note_rate_limited(site_id);
                    }
                    return done(CoverageStatus::Failed, format!("成员名兜底检索失败：{e:#}"));
                }
            }
        }
    }
    let Some(hit) = found else {
        // ---- 转种臂（站内没有）----
        return reseed_arm(
            item,
            site_id,
            &fp,
            qb,
            pieces,
            throttle,
            source_cache,
            adapters,
            cov,
            done,
        );
    };

    // 命中 → 详情页 → 下载 .torrent（各一次预算）
    if !throttle.acquire(site_id, BudgetKind::Search) {
        return skipped("本轮预算用完（检索已命中、详情页没读），下次接着跑");
    }
    let page = match adapter.fetch_torrent_page(&hit.detail_url) {
        Ok(p) => p,
        Err(e) => {
            return done(
                CoverageStatus::Failed,
                format!("详情页读不下来（cookie 可能失效）：{e:#}"),
            )
        }
    };
    if !throttle.acquire(site_id, BudgetKind::Search) {
        return skipped("本轮预算用完（详情页已读、种子没下载），下次接着跑");
    }
    let data = match adapter.download_torrent(&page, &hit.detail_url) {
        Ok(d) => d,
        Err(e) => {
            return done(
                CoverageStatus::Failed,
                format!("下载种子失败（cookie 可能失效）：{e:#}"),
            )
        }
    };
    throttle.note_ok(site_id);

    let (level, why) = matcher::judge(&item.hashes, &fp, &data, &page.name);
    finish_judged(
        item,
        site_id,
        qb,
        cov,
        dl,
        &data,
        &hit.detail_url,
        &fp,
        level,
        why,
        dry_run,
        done,
        done_url,
    )
}

/// 名字兜底候选：成员种子里与内容名不同、体积最大的那个。
/// 内容名常是合集/目录名，站点标题却可能是某个成员种子的名字；只挑一个，
/// 避免为不可控数量的成员名反复消耗检索预算。
fn pick_member_fallback(content_name: &str, members: &[(String, i64)]) -> Option<String> {
    let content = content_name.trim();
    members
        .iter()
        .filter(|(n, _)| {
            // Equality is compared after trimming both sides; case is left as-is.
            let trimmed = n.trim();
            !trimmed.is_empty() && trimmed != content
        })
        .max_by_key(|(_, size)| *size)
        .map(|(n, _)| n.clone())
}

/// 判定 → 布局 → 推种（快速通道与名字检索共用同一条尾巴）
#[allow(clippy::too_many_arguments)]
fn finish_judged(
    item: &WorkItem,
    site_id: &str,
    qb: &QBittorrent,
    cov: &crate::config::CoverageSettings,
    dl: &DownloaderSettings,
    data: &[u8],
    detail_url: &str,
    fp: &TorrentFingerprint,
    level: JudgeLevel,
    why: String,
    dry_run: bool,
    done: impl Fn(CoverageStatus, String) -> Event,
    done_url: impl Fn(CoverageStatus, String, String) -> Event,
) -> Event {
    match level {
        // 名字像但总大小/文件数完全不同：分集 vs 全集、不同分辨率/压制组。
        // 这不是「待人工确认」，补不了，直接落其他版本。
        JudgeLevel::Different => done_url(
            CoverageStatus::OtherVersion,
            format!("同名但内容不是同一段文件（{why}），未加入下载器"),
            detail_url.to_string(),
        ),
        JudgeLevel::NameOnly { .. } => done_url(
            CoverageStatus::ReviewPending,
            format!("名字对得上、总大小一致但文件清单拼不上：{why}"),
            detail_url.to_string(),
        ),
        JudgeLevel::Skip { why } => done(CoverageStatus::Rejected, why),
        JudgeLevel::SameFiles if !cov.auto_accept_same_files => done_url(
            CoverageStatus::ReviewPending,
            "站点重新打包过（hash 不同），按设置不自动采纳".into(),
            detail_url.to_string(),
        ),
        JudgeLevel::SameHash { .. } | JudgeLevel::SameFiles => {
            // 目录布局：直接加会重下一整份的先拦下（方案第七节；Swift 同款默认拦截）
            let site_paths: HashSet<String> = bencode::file_list(data)
                .map(|fl| fl.iter().map(|(p, _)| p.clone()).collect())
                .unwrap_or_default();
            let local_paths: HashSet<String> = fp.files.iter().map(|(p, _)| p.clone()).collect();
            match matcher::layout(&local_paths, &site_paths) {
                Layout::Differs => done_url(
                    CoverageStatus::ReviewPending,
                    "文件一致，但目录结构与本地不同，直接加会重下一整份，先不动下载器".into(),
                    detail_url.to_string(),
                ),
                // 内层一致、只有根目录名不同：推种时按 contentLayout=NoSubfolder
                // 复用既有内容目录（layout 模块按实时 savePath/文件清单决定）
                Layout::Same | Layout::RootDiffers => push_tail(
                    item, site_id, qb, cov, dl, data, detail_url, &why, dry_run, done_url,
                ),
            }
        }
    }
}

/// 统一推送尾巴（方案第七节）：暂停添加 → 轮询校验 → 满 100% 才启动；不满按设置回滚
/// （保留文件——留着它会自己重下一整份）；等到头还没结论就落「校验中」下一轮收尾。
#[allow(clippy::too_many_arguments)]
fn push_tail(
    item: &WorkItem,
    site_id: &str,
    qb: &QBittorrent,
    cov: &crate::config::CoverageSettings,
    dl: &DownloaderSettings,
    data: &[u8],
    detail_url: &str,
    why: &str,
    dry_run: bool,
    done_url: impl Fn(CoverageStatus, String, String) -> Event,
) -> Event {
    // 限速按辅种这台自己那份逐站表算
    let limit = dl
        .site_up_limits
        .get(site_id)
        .copied()
        .unwrap_or(dl.default_up_limit);
    let filename = format!(
        "{}.torrent",
        bencode::info_name(data).unwrap_or_else(|| item.name.clone())
    );
    let category = item.category.as_deref().or(dl.category.as_deref());
    // 落盘布局：沿用既有种子的实时 savepath；根名不同的同内容种按 NoSubfolder 复用
    let layout =
        crate::layout::resolve_for_torrent(qb, &item.rep_hash, data, &item.name, &item.save_path);
    if dry_run {
        return done_url(
            CoverageStatus::ReviewPending,
            format!("可辅（dry-run 未推送）：{why}｜{}", layout.describe()),
            detail_url.to_string(),
        );
    }
    if let crate::layout::AddLayout::Mismatch(why) = &layout {
        return done_url(
            CoverageStatus::ReviewPending,
            format!("布局对不上，不推（待人工）：{why}"),
            detail_url.to_string(),
        );
    }
    let save_path = layout.save_path().unwrap_or(&item.save_path).to_string();
    let add = match qb.add_torrent(
        data,
        &filename,
        &save_path,
        category,
        cov.skip_hash_check,
        limit,
        true,
        layout.content_layout(),
    ) {
        Ok(a) => a,
        Err(e) => {
            return done_url(
                CoverageStatus::Failed,
                format!("加进下载器失败：{e:#}"),
                detail_url.to_string(),
            )
        }
    };
    let site_hash = if add.id.is_empty() {
        bencode::info_hash_v1(data).unwrap_or_else(|| item.rep_hash.clone())
    } else {
        add.id
    };
    if add.already {
        // 「已存在于下载器」回执：tracker 补挂结果带进格子备注。
        // 只有 already 才走终态捷径——新加种的 note（限速未生效/missingFiles）
        // 不是「已存在」，必须继续 start + 校验，否则会永远暂停不作种。
        return done_url(
            CoverageStatus::Assisted,
            format!("{}（{why}）", add.note),
            detail_url.to_string(),
        );
    }
    let add_note_suffix = if add.note.is_empty() {
        String::new()
    } else {
        format!("；{}", add.note)
    };
    // qB 5.x：停止态的种子不会开始校验，必须 start 一次才会核对数据（实机 qB 5.3 验证）。
    // 核对期间的下载兜底由 add 时的 dlLimit=1 保证——内容对不上也不会开始狂下。
    let _ = qb.set_paused(&[&site_hash], false);
    let poll = cov.verify_poll_sec.max(1);
    let mut waited: u64 = 0;
    while waited < cov.verify_wait_sec {
        if let Ok(Some((progress, state))) = qb.torrent_progress(&site_hash) {
            if progress >= 0.999 {
                // 核对通过：解除兜底限速，种子已经在跑（做种中）
                let _ = qb.set_download_limit(&site_hash, 0);
                return done_url(
                    CoverageStatus::Assisted,
                    format!("校验到 100%，已在做种（{why}）{add_note_suffix}"),
                    detail_url.to_string(),
                );
            }
            // 校验跑完仍不满：文件对不上——先停住再决定去留，别让它开始重下
            if waited > 0 && !is_checking(&state) {
                let _ = qb.set_paused(&[&site_hash], true);
                if cov.remove_on_verify_fail {
                    let _ = qb.remove_torrent(&site_hash, true);
                    return done_url(
                        CoverageStatus::Failed,
                        format!(
                            "校验后只到 {:.0}%，已删掉刚加的种子（文件保留）：分片结构与本地不同，无法跨种",
                            progress * 100.0
                        ),
                        detail_url.to_string(),
                    );
                }
                return done_url(
                    CoverageStatus::Failed,
                    format!(
                        "校验后只到 {:.0}%，种子已停止：分片结构与本地不同，别让它开始重下",
                        progress * 100.0
                    ),
                    detail_url.to_string(),
                );
            }
        }
        thread::sleep(Duration::from_secs(poll));
        waited += poll;
    }
    // 落「校验中」：不落盘的话这一格重启就人间蒸发，那条暂停的种子也没人收尾。
    // verify_hash 是下一轮「收尾 pass」的凭据（主线程直查下载器，不花站点预算）
    Event::ItemDone {
        site_id: site_id.to_string(),
        key: item.key.clone(),
        status: CoverageStatus::Verifying,
        url: Some(detail_url.to_string()),
        note: format!(
            "校验还没跑完（等了 {} 秒），下一轮接着收尾（{why}）",
            cov.verify_wait_sec
        ),
        source: None,
        draft: None,
        verify_hash: Some(site_hash),
    }
}

/// 转种臂（站内没有）：定位源种 → 出转种草稿。
/// 真正的上传由 `autopost`（白名单+日限额+门禁）或 `reseed --draft N --yes` 执行。
#[allow(clippy::too_many_arguments)] // 内部管道：worker 需要的全部共享件
fn reseed_arm(
    item: &WorkItem,
    site_id: &str,
    fp: &TorrentFingerprint,
    qb: &QBittorrent,
    pieces: &Arc<Mutex<PiecesApi>>,
    throttle: &SiteThrottle,
    source_cache: &SourceCache,
    adapters: &[Arc<NexusPhp>],
    cov: &crate::config::CoverageSettings,
    done: impl Fn(CoverageStatus, String) -> Event,
) -> Event {
    if cov.reseed_mode == ReseedMode::Off {
        return done(CoverageStatus::Miss, "站内没有（转种臂关闭）".into());
    }
    if cov.reseed_paused_sites.iter().any(|s| s == site_id) {
        return done(
            CoverageStatus::Miss,
            "站内没有（该站暂不参与转种：无发布权限/上传界面 v1 不支持，见 assist.reseedPausedSites）"
                .into(),
        );
    }
    if item.forbid_reseed {
        return done(
            CoverageStatus::Miss,
            "站内没有（源种带禁转标记，永不转种；辅种不受影响）".into(),
        );
    }
    let mk_draft_event =
        |status: CoverageStatus, note: String, src: SourceRecord| Event::ItemDone {
            site_id: site_id.to_string(),
            key: item.key.clone(),
            status,
            url: if src.detail_url.is_empty() {
                None
            } else {
                Some(src.detail_url.clone())
            },
            note,
            source: Some(src.clone()),
            draft: if src.detail_url.is_empty() {
                None
            } else {
                Some(DraftItem {
                    content_key: item.key.clone(),
                    name: item.name.clone(),
                    source_site: src.site_id.clone(),
                    source_url: src.detail_url.clone(),
                    target_site: site_id.to_string(),
                })
            },
            verify_hash: None,
        };
    // 源种已定位（本轮缓存或索引里）→ 直接出草稿
    let cached: Option<SourceRecord> = source_cache
        .lock()
        .unwrap()
        .get(&item.key)
        .cloned()
        .or_else(|| item.source.clone());
    if let Some(src) = &cached {
        if !src.detail_url.is_empty() {
            return mk_draft_event(
                CoverageStatus::ReseedDraft,
                format!(
                    "站内没有；源种已定位（{}），转种草稿待人工确认",
                    src.site_id
                ),
                src.clone(),
            );
        }
        // miss 负缓存期内不再找源
        if let Some(miss_at) = src.miss_at {
            if now() - miss_at < cov.source_miss_ttl_days * 86_400 {
                return done(
                    CoverageStatus::WaitSource,
                    "站内没有，源站也都没有（负缓存期内）".into(),
                );
            }
        }
    }
    // pieces-hash 精确反查优先：本地内容字节命中源站即同源，不受片名改写影响。
    if let Some((src_site, detail_url, note)) =
        resolve_source_by_pieces(item, qb, adapters, pieces, fp, throttle, site_id)
    {
        let rec = SourceRecord {
            site_id: src_site.clone(),
            detail_url: detail_url.clone(),
            resolved_at: now(),
            miss_at: None,
        };
        source_cache
            .lock()
            .unwrap()
            .insert(item.key.clone(), rec.clone());
        return mk_draft_event(
            CoverageStatus::ReseedDraft,
            format!("站内没有；源种定位成功（{src_site}：pieces-hash 精确命中 {note}），转种草稿待人工确认"),
            rec,
        );
    }
    // 走源种定位：名字检索兜底。
    // 刚检索过的目标站要剔掉——同一套查询词再问一遍「源站」纯属白花请求
    // （站内没有就是没有，换目标站才是新信息）。实机一轮里这一条能省掉一半请求。
    // 源站候选与 pieces 反查同一套 tracker 定向（source_adapters）。
    let sites: Vec<SourceSite<'_>> = source_adapters(adapters, site_id, &item.tracker_hosts)
        .into_iter()
        .map(|a| SourceSite {
            client: a.as_ref(),
            announce_hosts: a.announce_hosts.clone(),
        })
        .collect();
    if sites.is_empty() {
        return done(
            CoverageStatus::WaitSource,
            "站内没有；本轮没有别的源站可问（参与站只有这一家）".into(),
        );
    }
    match resolve_source(
        &item.name,
        &item.tracker_hosts,
        &item.hashes,
        fp,
        &sites,
        cov.relaxed_retry_search,
        throttle,
    ) {
        ResolveOutcome::Resolved {
            site_id: src_site,
            detail_url,
            name: _,
            data: _,
            level: _,
            note,
        } => {
            let rec = SourceRecord {
                site_id: src_site.clone(),
                detail_url: detail_url.clone(),
                resolved_at: now(),
                miss_at: None,
            };
            source_cache
                .lock()
                .unwrap()
                .insert(item.key.clone(), rec.clone());
            mk_draft_event(
                CoverageStatus::ReseedDraft,
                format!("站内没有；源种定位成功（{src_site}：{note}），转种草稿待人工确认"),
                rec,
            )
        }
        ResolveOutcome::Miss { clean: true, .. } => {
            let rec = SourceRecord::miss(now());
            source_cache
                .lock()
                .unwrap()
                .insert(item.key.clone(), rec.clone());
            mk_draft_event(
                CoverageStatus::WaitSource,
                "站内没有，源站也都没有（记入待找源清单）".into(),
                rec,
            )
        }
        ResolveOutcome::Miss {
            clean: false,
            attempts,
        } => done(
            CoverageStatus::Failed,
            format!(
                "源站定位没问成（不记负缓存）：{}",
                attempts.first().map(|a| a.note.clone()).unwrap_or_default()
            ),
        ),
        // 预算耗尽不等于「站内没有」：落 Miss 会把辅种机会负缓存一个 TTL，
        // 与检索侧语义对齐——本轮跳过，不写格子，下轮重试
        ResolveOutcome::BudgetExhausted { .. } => Event::ItemSkipped {
            site_id: site_id.to_string(),
            key: item.key.clone(),
            reason: "源种定位预算用完，本轮不落结论（下轮接着试）".into(),
        },
    }
}

/// 校验中格子的收尾判定（方案第七节「下一轮收尾」）：满 100% → 启动；
/// 校验完仍不满 → 按 removeOnVerifyFail 回滚或保持暂停；还在校验/这次查询没通 →
/// None（下轮再看）。qB 查询不过站点预算。
fn collect_verifying(
    qb: &QBittorrent,
    hash: &str,
    cov: &crate::config::CoverageSettings,
) -> Option<(CoverageStatus, String)> {
    match qb.torrent_progress(hash) {
        Ok(Some((progress, state))) => {
            if progress >= 0.999 {
                let _ = qb.set_download_limit(hash, 0);
                let _ = qb.set_paused(&[hash], false);
                Some((
                    CoverageStatus::Assisted,
                    "校验到 100%，已在做种（上一轮推送收尾）".to_string(),
                ))
            } else if is_checking(&state) {
                None
            } else if cov.remove_on_verify_fail {
                let _ = qb.set_paused(&[hash], true);
                let _ = qb.remove_torrent(hash, true);
                Some((
                    CoverageStatus::Failed,
                    format!(
                        "校验后只到 {:.0}%，已删掉刚加的种子（文件保留）：分片结构与本地不同，无法跨种",
                        progress * 100.0
                    ),
                ))
            } else {
                let _ = qb.set_paused(&[hash], true);
                Some((
                    CoverageStatus::Failed,
                    format!(
                        "校验后只到 {:.0}%，种子已停止：分片结构与本地不同，别让它开始重下",
                        progress * 100.0
                    ),
                ))
            }
        }
        // 下载器里查无此种子（用户手删一类）：记失败，别永远挂着校验中
        Ok(None) => Some((
            CoverageStatus::Failed,
            "下载器里已找不到那条种子（可能被手动删除），收尾失败".to_string(),
        )),
        Err(_) => None,
    }
}

/// 首扫描（或索引里还没有清单摘要的种子）预取文件清单。
///
/// 只有「hash 在索引里查不到」或「所属条目还是 hash-only 占位（没有摘要）」的种子需要抓；
/// 已有摘要的种子一律跳过——这是二次扫描便宜的关键（4 万条的索引重扫零请求）。
/// 抓不到的 seed 不进结果，merge_scan 会退回 hash-only 条目，下一轮再补。
pub fn prefetch_files(
    qb: &QBittorrent,
    torrents: &[LocalTorrent],
    index: &ContentIndex,
    concurrency: usize,
    mut on_progress: impl FnMut(usize, usize),
) -> HashMap<String, Vec<(String, i64)>> {
    let known: HashSet<String> = index.hash_to_key().into_keys().collect();
    let mut needs_digest: HashSet<String> = HashSet::new();
    for e in index.contents.values() {
        if e.manifest_digest.is_empty() {
            needs_digest.extend(e.all_hashes());
        }
    }
    let mut need: Vec<String> = Vec::new();
    for t in torrents {
        if t.progress < 1.0 {
            continue;
        }
        let hashes = t.all_hashes();
        let covered = hashes.iter().any(|h| known.contains(h));
        let pending = hashes.iter().any(|h| needs_digest.contains(h));
        if !covered || pending {
            need.push(t.hash.clone());
        }
    }
    need.sort_unstable();
    need.dedup();
    qb.torrent_files_many(&need, concurrency, |done, total, _ok| {
        on_progress(done, total)
    })
}

fn fingerprint_of(
    qb: &QBittorrent,
    cache: &FingerprintCache,
    hash: &str,
) -> Option<Arc<TorrentFingerprint>> {
    if let Some(f) = cache.lock().unwrap().get(hash) {
        return Some(f.clone());
    }
    let files = qb.torrent_files(hash).ok()?;
    let fp = Arc::new(TorrentFingerprint::from_local_files(&files));
    cache.lock().unwrap().insert(hash.to_string(), fp.clone());
    Some(fp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::ContentEntry;

    #[test]
    fn member_name_fallback_picks_largest_different_name() {
        let members = vec![
            ("合集名".to_string(), 10),
            ("Coraline.2009.1080p...PIXELPOLICE".to_string(), 900),
            ("花絮.mkv".to_string(), 5),
        ];
        assert_eq!(
            pick_member_fallback("合集名", &members).as_deref(),
            Some("Coraline.2009.1080p...PIXELPOLICE")
        );
        // 全部与内容名相同 / 空表 → 没有候选，不发多余检索
        assert_eq!(
            pick_member_fallback("合集名", &[("合集名".to_string(), 1)]),
            None
        );
        assert_eq!(pick_member_fallback("x", &[]), None);
    }

    fn adapter(id: &str, url: &str) -> Arc<NexusPhp> {
        Arc::new(NexusPhp::new(id, id, url, None, None).unwrap())
    }

    #[test]
    fn source_adapters_follow_tracker_and_exclude_target() {
        let adapters = vec![
            Arc::new(
                NexusPhp::new("luckpt", "luckpt", "https://pt.luckpt.de/", None, None)
                    .unwrap()
                    .with_announce_hosts(vec!["tracker.luckpt.de".to_string()]),
            ),
            adapter("sbpt", "https://sbpt.link/"),
            adapter("other", "https://other.example/"),
        ];
        // tracker 命中 luckpt（announce 别名也算）→ 只问 luckpt，目标站 sbpt 剔除
        let hosts = vec!["tracker.luckpt.de".to_string()];
        let picked = source_adapters(&adapters, "sbpt", &hosts);
        assert_eq!(
            picked
                .iter()
                .map(|a| a.site_id.as_str())
                .collect::<Vec<_>>(),
            vec!["luckpt"]
        );
        // 目标是 luckpt 时，tracker 指向 sbpt → 只问 sbpt
        let hosts = vec!["sbpt.link".to_string()];
        let picked = source_adapters(&adapters, "luckpt", &hosts);
        assert_eq!(
            picked
                .iter()
                .map(|a| a.site_id.as_str())
                .collect::<Vec<_>>(),
            vec!["sbpt"]
        );
        // tracker 为空 → 除目标外全问（老索引没有 tracker 信息时的兜底）
        let picked = source_adapters(&adapters, "sbpt", &[]);
        assert_eq!(picked.len(), 2);
        // tracker 没映射到任何参与站 → 回退全部源站（不能静默丢源）
        let hosts = vec!["foreign-tracker.example".to_string()];
        let picked = source_adapters(&adapters, "sbpt", &hosts);
        assert_eq!(picked.len(), 2);
        // 子域双向匹配：tracker 是站点 host 的子域也认
        let hosts = vec!["www.sbpt.link".to_string()];
        let picked = source_adapters(&adapters, "luckpt", &hosts);
        assert_eq!(
            picked
                .iter()
                .map(|a| a.site_id.as_str())
                .collect::<Vec<_>>(),
            vec!["sbpt"]
        );
    }

    fn work_item(key: &str, rep: &str, members: &[&str]) -> WorkItem {
        WorkItem {
            key: key.to_string(),
            name: key.to_string(),
            rep_hash: rep.to_string(),
            save_path: String::new(),
            category: None,
            hashes: members.iter().map(|s| s.to_string()).collect(),
            tracker_hosts: vec![],
            source: None,
            forbid_reseed: false,
        }
    }

    #[test]
    fn prefetch_keyed_includes_cached_member_pieces() {
        // sbpt 实测场景：rep 是 luckpt 重新打包（ph-rep），站内已有种与另一成员
        // （ph-orig，已缓存）pieces 一致——只查 rep 会漏，缓存成员必须入批
        let cache: HashMap<String, String> = [
            ("aaa".to_string(), "ph-orig".to_string()),
            ("bbb".to_string(), "ph-rep".to_string()),
        ]
        .into_iter()
        .collect();
        let items = vec![work_item("m:1", "bbb", &["aaa", "BBB", "ccc"])];
        let mut exported: Vec<String> = Vec::new();
        let keyed = prefetch_keyed(&items, |h, allow_export| {
            if allow_export {
                exported.push(h.to_string());
            }
            cache.get(&h.to_lowercase()).cloned()
        });
        assert_eq!(
            keyed,
            vec![
                ("m:1".to_string(), "ph-rep".to_string()),
                ("m:1".to_string(), "ph-orig".to_string()),
            ]
        );
        // 只有 rep 允许导出现算；未缓存成员（ccc）不触发 qB 导出
        assert_eq!(exported, vec!["bbb".to_string()]);
    }

    #[test]
    fn prefetch_keyed_dedups_and_caps_contents() {
        let cache: HashMap<String, String> = [
            ("a".to_string(), "ph-shared".to_string()),
            ("b".to_string(), "ph-shared".to_string()),
        ]
        .into_iter()
        .collect();
        let items = vec![work_item("m:1", "a", &["a"]), work_item("m:2", "b", &["b"])];
        let keyed = prefetch_keyed(&items, |h, _| cache.get(h).cloned());
        assert_eq!(keyed.len(), 1, "同 ph 去重，不重复占批量名额");
        let many: Vec<WorkItem> = (0..150)
            .map(|i| {
                let h = format!("h{i}");
                work_item(&format!("m:{i}"), &h, &[h.as_str()])
            })
            .collect();
        let keyed = prefetch_keyed(&many, |h, _| Some(format!("ph-{h}")));
        assert_eq!(keyed.len(), 100, "至多 100 段内容");
    }

    // ---------- pick_member_fallback edge cases ----------

    #[test]
    fn member_fallback_empty_or_same_name_returns_none() {
        // No candidates at all: nothing to search with.
        assert_eq!(pick_member_fallback("content", &[]), None);
        // The only member shares the content name: filtered out, no fallback.
        assert_eq!(
            pick_member_fallback("content", &[("content".to_string(), 100)]),
            None
        );
        // Whitespace-only names count as empty candidates.
        assert_eq!(
            pick_member_fallback("content", &[("   ".to_string(), 100)]),
            None
        );
    }

    #[test]
    fn member_fallback_tie_keeps_last_member_in_order() {
        // Equal sizes: max_by_key returns the last maximal element, so the choice is
        // deterministic and follows the member order returned by the downloader.
        let members = vec![
            ("first.mkv".to_string(), 100),
            ("second.mkv".to_string(), 100),
        ];
        assert_eq!(
            pick_member_fallback("content", &members).as_deref(),
            Some("second.mkv")
        );
    }

    #[test]
    fn member_fallback_exact_name_excluded_smaller_other_name_kept() {
        // The exact content-name member is excluded even when it is by far the largest;
        // a different-named member is still picked as the fallback.
        let members = vec![("content".to_string(), 900), ("alt.mkv".to_string(), 10)];
        assert_eq!(
            pick_member_fallback("content", &members).as_deref(),
            Some("alt.mkv")
        );
    }

    #[test]
    fn member_fallback_equality_trims_whitespace() {
        // Equality against content_name is compared after trimming both sides: a member
        // padded with surrounding whitespace still counts as the content name and is
        // excluded, leaving no fallback when it is the only member.
        let members = vec![(" content ".to_string(), 100)];
        assert_eq!(pick_member_fallback("content", &members), None);
    }

    #[test]
    fn member_fallback_trimmed_same_name_excluded_picks_real_other() {
        // The whitespace-padded same-name member (by far the largest) is excluded; the
        // genuinely different-named member is still picked as the fallback.
        let members = vec![
            (" Show.S01 ".to_string(), 900),
            ("Show.S01.E01.mkv".to_string(), 100),
        ];
        assert_eq!(
            pick_member_fallback("Show.S01", &members).as_deref(),
            Some("Show.S01.E01.mkv")
        );
    }

    // ---------- adapter_matches_tracker edge cases ----------

    fn adapter_with_announce(id: &str, url: &str, announce: &[&str]) -> Arc<NexusPhp> {
        Arc::new(
            NexusPhp::new(id, id, url, None, None)
                .unwrap()
                .with_announce_hosts(announce.iter().map(|s| s.to_string()).collect()),
        )
    }

    #[test]
    fn tracker_match_is_case_insensitive() {
        let a = adapter("sbpt", "https://sbpt.link/");
        // Both sides are lowercased before comparison, so mixed/upper case hits.
        assert!(adapter_matches_tracker(&a, &["SBPT.LINK".to_string()]));
        assert!(adapter_matches_tracker(&a, &["Sbpt.Link".to_string()]));
    }

    #[test]
    fn tracker_match_ignores_base_url_scheme() {
        // host_str() strips the scheme, so http vs https on the site base does not matter.
        let http = adapter("a", "http://host.example/");
        let https = adapter("b", "https://host.example/");
        let hosts = vec!["host.example".to_string()];
        assert!(adapter_matches_tracker(&http, &hosts));
        assert!(adapter_matches_tracker(&https, &hosts));
    }

    #[test]
    fn tracker_match_hits_second_entry_when_first_is_foreign() {
        let a = adapter("sbpt", "https://sbpt.link/");
        let hosts = vec!["unrelated.example".to_string(), "sbpt.link".to_string()];
        assert!(adapter_matches_tracker(&a, &hosts));
    }

    #[test]
    fn tracker_match_rejects_foreign_host() {
        let a = adapter("sbpt", "https://sbpt.link/");
        assert!(!adapter_matches_tracker(
            &a,
            &["foreign-tracker.example".to_string()]
        ));
    }

    #[test]
    fn tracker_match_empty_host_list_matches_every_adapter() {
        // No tracker info on record: caller falls back to asking every source site.
        let a = adapter("sbpt", "https://sbpt.link/");
        assert!(adapter_matches_tracker(&a, &[]));
    }

    #[test]
    fn tracker_match_accepts_reverse_subdomain_and_announce_alias() {
        // Reverse direction: the site host is a subdomain of the tracker host.
        let parent = adapter("luckpt", "https://pt.luckpt.de/");
        assert!(adapter_matches_tracker(&parent, &["luckpt.de".to_string()]));
        // Announce aliases configured for the site also participate in matching.
        let alias =
            adapter_with_announce("luckpt", "https://pt.luckpt.de/", &["tracker.luckpt.de"]);
        assert!(adapter_matches_tracker(
            &alias,
            &["tracker.luckpt.de".to_string()]
        ));
    }

    #[test]
    fn tracker_match_ignores_port_and_trailing_dot() {
        // Comparison-only normalization strips the port suffix and the FQDN trailing
        // dot, so raw announce strings match the plain site host.
        let a = adapter("sbpt", "https://tracker.example/");
        assert!(adapter_matches_tracker(
            &a,
            &["tracker.example:8080".to_string()]
        ));
        assert!(adapter_matches_tracker(
            &a,
            &["tracker.example.".to_string()]
        ));
    }

    #[test]
    fn normalize_tracker_host_strips_scheme_port_path_case_whitespace_and_dot() {
        assert_eq!(
            normalize_tracker_host("tracker.example:8080"),
            "tracker.example"
        );
        assert_eq!(
            normalize_tracker_host("tracker.example."),
            "tracker.example"
        );
        assert_eq!(
            normalize_tracker_host("tracker.example.."),
            "tracker.example"
        );
        assert_eq!(
            normalize_tracker_host("udp://tracker.example:6969/announce"),
            "tracker.example"
        );
        assert_eq!(
            normalize_tracker_host("https://TRACKER.example/announce.php?passkey=x"),
            "tracker.example"
        );
        assert_eq!(
            normalize_tracker_host("  tracker.example  "),
            "tracker.example"
        );
        assert_eq!(normalize_tracker_host("tracker.example"), "tracker.example");
        // Bracketed IPv6 literals skip the port strip; the trailing port suffix is kept.
        assert_eq!(
            normalize_tracker_host("[2001:db8::1]:8080"),
            "[2001:db8::1]:8080"
        );
    }

    #[test]
    fn normalize_tracker_host_strips_userinfo() {
        // `user:pass@host`：@ 前的凭据剥离后再比 host，端口一并正常剥掉。
        assert_eq!(
            normalize_tracker_host("user:pass@tracker.example:8080/announce"),
            "tracker.example"
        );
        assert_eq!(
            normalize_tracker_host("https://name:secret@tracker.example/announce.php?passkey=x"),
            "tracker.example"
        );
        // 没有 userinfo 时行为不变（回归）。
        assert_eq!(
            normalize_tracker_host("https://tracker.example/announce.php?passkey=x"),
            "tracker.example"
        );
    }

    #[test]
    fn normalize_tracker_host_skips_port_strip_for_ipv6_literals() {
        // 裸 IPv6（无方括号、多个冒号）：跳过端口剥离，地址末段不会被误当端口砍掉。
        assert_eq!(normalize_tracker_host("2001:db8::1"), "2001:db8::1");
        assert_eq!(
            normalize_tracker_host("2001:db8::1:8080"),
            "2001:db8::1:8080"
        );
        // 带方括号的 IPv6 维持既有行为：端口后缀保留（既有断言钉住）。
        assert_eq!(
            normalize_tracker_host("[2001:db8::1]:8080"),
            "[2001:db8::1]:8080"
        );
        assert_eq!(
            normalize_tracker_host("http://[2001:db8::1]:8080/announce"),
            "[2001:db8::1]:8080"
        );
    }

    #[test]
    fn tracker_match_strips_userinfo_before_matching() {
        // adapter 级回归：带 userinfo 的 announce 归一后仍命中站点 host。
        let a = adapter("sbpt", "https://tracker.example/");
        assert!(adapter_matches_tracker(
            &a,
            &["user:pass@tracker.example:8080/announce".to_string()]
        ));
        // 端口与尾点回归（既有形态照样命中）。
        assert!(adapter_matches_tracker(
            &a,
            &["tracker.example:8080".to_string()]
        ));
        assert!(adapter_matches_tracker(
            &a,
            &["tracker.example.".to_string()]
        ));
    }

    #[test]
    fn tracker_match_normalizes_announce_url_forms() {
        let a = adapter("sbpt", "https://tracker.example/");
        // Full announce URL with udp scheme, port and path hits the plain host.
        assert!(adapter_matches_tracker(
            &a,
            &["udp://tracker.example:6969/announce".to_string()]
        ));
        assert!(adapter_matches_tracker(
            &a,
            &["tracker.example.".to_string()]
        ));
        assert!(adapter_matches_tracker(
            &a,
            &["tracker.example:8080".to_string()]
        ));
        // A foreign host with a port still does not match.
        assert!(!adapter_matches_tracker(
            &a,
            &["tracker.other:8080".to_string()]
        ));
        // Reverse subdomain still matches (regression).
        let parent = adapter("luckpt", "https://pt.luckpt.de/");
        assert!(adapter_matches_tracker(&parent, &["luckpt.de".to_string()]));
    }

    // ---------- is_checking edge cases ----------

    #[test]
    fn is_checking_recognizes_qb_checking_states() {
        for state in [
            "checking",
            "checkingFiles",
            "checkingResumeData",
            "checkingDL",
            "checkingUP",
            "queuedForChecking",
            "allocating",
            "metadownload",
            "metaDL",
        ] {
            assert!(is_checking(state), "expected checking state: {state}");
        }
    }

    #[test]
    fn is_checking_rejects_download_queue_and_settled_states() {
        for state in [
            "",
            "queuedDL",
            "queuedUP",
            "downloading",
            "uploading",
            "stalledDL",
            "stalledUP",
            "pausedDL",
            "pausedUP",
            "seeding",
            "completed",
        ] {
            assert!(!is_checking(state), "expected non-checking state: {state}");
        }
    }

    #[test]
    fn is_checking_is_exact_case_sensitive_match_currently() {
        // Current behavior (pinned by this test): exact camelCase match; case variants
        // and surrounding whitespace do not hit.
        assert!(!is_checking("CHECKINGDL"));
        assert!(!is_checking("checkingdl"));
        assert!(!is_checking("MetaDL"));
        assert!(!is_checking("metaDL "));
        assert!(!is_checking(" metaDL"));
    }

    fn test_cfg() -> Config {
        Config {
            config_path: std::env::temp_dir().join("boxsend-coverage-test.json"),
            data_dir: None,
            downloader: crate::config::DownloaderSettings {
                url: "http://127.0.0.1:1".into(),
                username: "u".into(),
                password: "p".into(),
                save_path: None,
                category: None,
                default_up_limit: 0,
                site_up_limits: std::collections::HashMap::new(),
            },
            sites: vec![
                crate::config::SiteConf {
                    id: "agsvpt".into(),
                    name: "AGSVPT".into(),
                    url: "https://www.agsvpt.com/".into(),
                    framework: crate::sites::Framework::NexusPHP,
                    search_url: None,
                    api_key: None,
                    announce_hosts: vec!["www.agsvpt.com".into(), "tracker.agsvpt.cn".into()],
                    announce_url: None,
                    poster_field: None,
                    screenshot_field: None,
                    proxy: None,
                },
                crate::config::SiteConf {
                    id: "cyanbug".into(),
                    name: "CyanBug".into(),
                    url: "https://cyanbug.net/".into(),
                    framework: crate::sites::Framework::NexusPHP,
                    search_url: None,
                    api_key: None,
                    announce_hosts: vec!["cyanbug.net".into()],
                    announce_url: None,
                    poster_field: None,
                    screenshot_field: None,
                    proxy: None,
                },
            ],
            coverage: crate::config::CoverageSettings::default(),
            user_agent: "bsend-test".into(),
            cookies: std::collections::HashMap::new(),
            cookie_cloud: None,
            skipped_sites: Vec::new(),
        }
    }

    #[test]
    fn coverage_url_must_belong_to_target_site() {
        let cfg = test_cfg();
        // 本站详情链接放行
        cfg.validate_coverage_url("agsvpt", "https://www.agsvpt.com/details.php?id=5586")
            .unwrap();
        // 源站链接写进目标站格子 → 拒绝
        let err = cfg
            .validate_coverage_url("agsvpt", "https://cyanbug.net/details.php?id=5586")
            .unwrap_err();
        assert!(err.to_string().contains("cyanbug.net"), "{err}");
        // 站点不在配置里 → 拒绝
        assert!(cfg
            .validate_coverage_url("nope", "https://x.example/details.php?id=1")
            .is_err());
    }

    #[test]
    fn insert_done_cell_rejects_foreign_url_and_keeps_index() {
        let cfg = test_cfg();
        let mut index = ContentIndex::in_memory();
        index.contents.insert(
            "m:5826983b00126e58".into(),
            ContentEntry {
                name: "Some Show".into(),
                total_size: 1,
                file_count: 1,
                manifest_digest: "0".repeat(16),
                torrents: Default::default(),
                tracker_hosts: vec![],
                first_seen: 0,
                gone_since: None,
                coverage: Default::default(),
                source: None,
                forbid_reseed: false,
            },
        );
        // 红：把源站链接写进 agsvpt 做种格 → 拒绝，索引里不留格子
        let res = insert_done_cell(
            &mut index,
            &cfg,
            "agsvpt",
            "m:5826983b00126e58",
            CoverageStatus::Seeding,
            Some("https://cyanbug.net/details.php?id=5586"),
            "下载器已挂该站 tracker",
            None,
            None,
            0,
        );
        assert!(res.is_err());
        assert!(!index
            .contents
            .get("m:5826983b00126e58")
            .unwrap()
            .coverage
            .contains_key("agsvpt"));

        // 绿：本站详情链接 → 落格
        insert_done_cell(
            &mut index,
            &cfg,
            "agsvpt",
            "m:5826983b00126e58",
            CoverageStatus::Seeding,
            Some("https://www.agsvpt.com/details.php?id=5586"),
            "下载器已挂该站 tracker",
            None,
            None,
            0,
        )
        .unwrap();
        let cell = index
            .contents
            .get("m:5826983b00126e58")
            .unwrap()
            .coverage
            .get("agsvpt")
            .unwrap();
        assert_eq!(
            cell.url.as_deref(),
            Some("https://www.agsvpt.com/details.php?id=5586")
        );

        // 草稿格子存源种链接是既有语义，不校验域名
        insert_done_cell(
            &mut index,
            &cfg,
            "cyanbug",
            "m:5826983b00126e58",
            CoverageStatus::ReseedDraft,
            Some("https://cyanbug.net/details.php?id=5586"),
            "",
            None,
            None,
            0,
        )
        .unwrap();
    }
}
