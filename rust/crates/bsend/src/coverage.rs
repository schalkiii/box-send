//! 铺种编排（方案第三、六、七、九节）：索引 → 已做种标记 → 站点 worker 池 →
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
use crate::models::{JudgeLevel, Layout, TorrentFingerprint};
use crate::qbittorrent::QBittorrent;
use crate::resolver::{resolve_source, ResolveOutcome, SourceSite};
use crate::sites::nexusphp::NexusPhp;
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

/// qB 状态是不是还在校验/分配（没跑完就别下结论）
fn is_checking(state: &str) -> bool {
    matches!(
        state,
        "checking" | "checkingFiles" | "checkingResumeData" | "allocating" | "metadownload"
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
    let mut report = RoundReport::default();
    let t0 = now();

    // ---- 1 扫描 + 增量合并 ----
    let torrents = qb.list_torrents()?;
    report.scanned_torrents = torrents.len();
    let files_of = |t: &crate::models::LocalTorrent| qb.torrent_files(&t.hash).ok();
    report.delta = index.merge_scan(&torrents, files_of, t0, cfg.coverage.archive_days);
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
            if !entry
                .tracker_hosts
                .iter()
                .any(|h| crate::sites::host_matches(h, &site.announce_host))
            {
                continue;
            }
            let cur = entry.coverage.get(&site.id).map(|c| c.status);
            // 已辅/已转/校验中/否决是更强的结论，别被覆盖
            let overwrite = !matches!(
                cur,
                Some(CoverageStatus::Assisted)
                    | Some(CoverageStatus::Reseeded)
                    | Some(CoverageStatus::Verifying)
                    | Some(CoverageStatus::Rejected)
            );
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
        // 优先级：未覆盖站多优先、新入库优先（方案第九节）
        priority.insert(key.clone(), (uncovered, entry.first_seen));
    }
    // 各站队列按内容优先级排（同一站内先处理未覆盖站多的内容）
    let prio = |key: &str| priority.get(key).cloned().unwrap_or((0, 0));
    for items in site_queues.values_mut() {
        items.sort_by_key(|i| std::cmp::Reverse(prio(&i.key)));
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
    let throttle = Arc::new(SiteThrottle::new(
        Duration::from_secs_f64(cfg.coverage.per_site_interval_sec.max(0.0)),
        Duration::from_secs_f64(cfg.coverage.jitter_sec.max(0.0)),
        cfg.coverage.search_budget,
        cfg.coverage.source_budget,
        cfg.coverage.upload_budget,
    ));
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
    let mut handles = Vec::new();
    for _ in 0..n_workers {
        let (pool, throttle, fp_cache, source_cache, qb) = (
            pool.clone(),
            throttle.clone(),
            fp_cache.clone(),
            source_cache.clone(),
            qb.clone(),
        );
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
                report.preskipped += 1;
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
                if let Some(src) = &source {
                    if let Some(e) = index.contents.get_mut(&key) {
                        e.source = Some(src.clone());
                    }
                }
                if let Some(e) = index.contents.get_mut(&key) {
                    e.coverage.insert(
                        site_id.clone(),
                        SiteCoverage {
                            status,
                            at: t0,
                            url: url.clone(),
                            note: if note.is_empty() {
                                None
                            } else {
                                Some(note.clone())
                            },
                            // 非 Verifying 结论一律清空（收尾凭据只属于校验中格子）
                            verify_hash: if status == CoverageStatus::Verifying {
                                verify_hash.clone()
                            } else {
                                None
                            },
                        },
                    );
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
) {
    loop {
        // 认领一个站的队列（整站串行，保持本站节奏）
        let next = { pool.lock().unwrap().pop() };
        let Some((site_id, items)) = next else { break };
        let Some(adapter) = adapters.iter().find(|a| a.site_id == site_id) else {
            continue;
        };
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
            );
            let _ = tx.send(ev);
        }
    }
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

    // 本地指纹（本轮缓存；拿不到就没法逐文件比对）
    let Some(fp) = fingerprint_of(qb, fp_cache, &item.rep_hash) else {
        return done(
            CoverageStatus::Failed,
            "读不到本地种子的文件清单，没法逐文件比对".into(),
        );
    };

    // ---- 检索臂（方案第六节：先检索后转种）----
    if !throttle.acquire(site_id, BudgetKind::Search) {
        return skipped("本轮检索预算用完，下次接着跑");
    }
    let mut found = match adapter.search(&item.name, "", false, "") {
        Ok(h) => h,
        // 「检索报错」与「站内没有」是两回事：cookie 失效、站点 500 不能记成负缓存
        Err(e) => return done(CoverageStatus::Failed, format!("站内检索失败：{e:#}")),
    };
    if found.is_none() && cov.relaxed_retry_search {
        if !throttle.acquire(site_id, BudgetKind::Search) {
            return skipped("本轮检索预算用完（宽松重试没跑），下次接着跑");
        }
        found = match adapter.search(&item.name, "", true, "") {
            Ok(h) => h,
            Err(e) => return done(CoverageStatus::Failed, format!("宽松检索失败：{e:#}")),
        };
    }
    let Some(hit) = found else {
        // ---- 转种臂（站内没有）----
        return reseed_arm(
            item,
            site_id,
            &fp,
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
    match level {
        JudgeLevel::NameOnly { .. } => done_url(
            CoverageStatus::ReviewPending,
            format!("名字对得上但文件清单对不上：{why}"),
            hit.detail_url,
        ),
        JudgeLevel::Skip { why } => done(CoverageStatus::Rejected, why),
        JudgeLevel::SameFiles if !cov.auto_accept_same_files => done_url(
            CoverageStatus::ReviewPending,
            "站点重新打包过（hash 不同），按设置不自动采纳".into(),
            hit.detail_url,
        ),
        JudgeLevel::SameHash { .. } | JudgeLevel::SameFiles => {
            // 目录布局：直接加会重下一整份的先拦下（方案第七节；Swift 同款默认拦截）
            let site_paths: HashSet<String> = bencode::file_list(&data)
                .map(|fl| fl.iter().map(|(p, _)| p.clone()).collect())
                .unwrap_or_default();
            let local_paths: HashSet<String> = fp.files.iter().map(|(p, _)| p.clone()).collect();
            match matcher::layout(&local_paths, &site_paths) {
                Layout::Differs => done_url(
                    CoverageStatus::ReviewPending,
                    "文件一致，但目录结构与本地不同，直接加会重下一整份，先不动下载器".into(),
                    hit.detail_url,
                ),
                Layout::RootDiffers => done_url(
                    CoverageStatus::ReviewPending,
                    "文件一致，但站点种子的内层目录名与本地不同，直接加会重下一整份，先不动下载器"
                        .into(),
                    hit.detail_url,
                ),
                Layout::Same => push_tail(
                    item,
                    site_id,
                    qb,
                    cov,
                    dl,
                    &data,
                    &hit.detail_url,
                    &why,
                    dry_run,
                    done_url,
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
    if dry_run {
        return done_url(
            CoverageStatus::ReviewPending,
            format!("可辅（dry-run 未推送）：{why}"),
            detail_url.to_string(),
        );
    }
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
    let add = match qb.add_torrent(
        data,
        &filename,
        &item.save_path,
        category,
        false,
        limit,
        true,
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
    if !add.note.is_empty() {
        // 「已存在于下载器」等回执：tracker 补挂结果带进格子备注
        return done_url(
            CoverageStatus::Assisted,
            format!("{}（{why}）", add.note),
            detail_url.to_string(),
        );
    }
    let poll = cov.verify_poll_sec.max(1);
    let mut waited: u64 = 0;
    while waited < cov.verify_wait_sec {
        if let Ok(Some((progress, state))) = qb.torrent_progress(&site_hash) {
            if progress >= 0.999 {
                let _ = qb.set_paused(&[&site_hash], false);
                return done_url(
                    CoverageStatus::Assisted,
                    format!("校验到 100%，已启动（{why}）"),
                    detail_url.to_string(),
                );
            }
            // 校验跑完仍不满：文件对不上
            if waited > 0 && !is_checking(&state) {
                if cov.remove_on_verify_fail {
                    let _ = qb.remove_torrent(&site_hash, true);
                    return done_url(
                        CoverageStatus::Failed,
                        format!(
                            "校验后只到 {:.0}%，已删掉刚加的种子（文件保留）：内层路径与本地对不上",
                            progress * 100.0
                        ),
                        detail_url.to_string(),
                    );
                }
                return done_url(
                    CoverageStatus::Failed,
                    format!(
                        "校验后只到 {:.0}%，种子保持暂停：文件对不上，别让它开始重下",
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

/// 转种臂（站内没有）：v1 草稿模式——只定位源种、生成待发清单，不上传。
/// 上传涉及 148 站每家一套的表单适配，是 R4 阶段的事（方案第八节安全闸门）。
#[allow(clippy::too_many_arguments)] // 内部管道：worker 需要的全部共享件
fn reseed_arm(
    item: &WorkItem,
    site_id: &str,
    fp: &TorrentFingerprint,
    throttle: &SiteThrottle,
    source_cache: &SourceCache,
    adapters: &[Arc<NexusPhp>],
    cov: &crate::config::CoverageSettings,
    done: impl Fn(CoverageStatus, String) -> Event,
) -> Event {
    if cov.reseed_mode == ReseedMode::Off {
        return done(CoverageStatus::Miss, "站内没有（转种臂关闭）".into());
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
    // 走源种定位：源站名单 = 全部参与站点（已在做种的源站优先，resolver 内排序）
    let sites: Vec<SourceSite<'_>> = adapters
        .iter()
        .map(|a| SourceSite {
            client: a.as_ref(),
            announce_host: a
                .base
                .host_str()
                .map(|h| h.to_lowercase())
                .unwrap_or_default(),
        })
        .collect();
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
        ResolveOutcome::BudgetExhausted { .. } => done(
            CoverageStatus::Miss,
            "站内没有；源种定位预算用完，下次接着试".into(),
        ),
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
                let _ = qb.set_paused(&[hash], false);
                Some((
                    CoverageStatus::Assisted,
                    "校验到 100%，已启动（上一轮推送收尾）".to_string(),
                ))
            } else if is_checking(&state) {
                None
            } else if cov.remove_on_verify_fail {
                let _ = qb.remove_torrent(hash, true);
                Some((
                    CoverageStatus::Failed,
                    format!(
                        "校验后只到 {:.0}%，已删掉刚加的种子（文件保留）：内层路径与本地对不上",
                        progress * 100.0
                    ),
                ))
            } else {
                Some((
                    CoverageStatus::Failed,
                    format!(
                        "校验后只到 {:.0}%，种子保持暂停：文件对不上，别让它开始重下",
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
