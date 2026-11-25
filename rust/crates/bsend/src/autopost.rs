//! 自动发种核心：草稿 → 审阅 → 门禁 → 发布，白名单 + 日限额 + 失败冷却 + 审计留痕。
//! CLI 与 GUI 共用本模块：调用方只渲染事件，不重复实现流程逻辑。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::config::Config;
use crate::coverage::now_unix;
use crate::gates::{self, GateResult};
use crate::index::{ContentIndex, CoverageStatus};
use crate::models::LocalTorrent;
use crate::qbittorrent::QBittorrent;
use crate::report::{reconcile_marks, CoverageReport};
use crate::reseed;
use crate::review::ReviewCheckOptions;

pub struct AutopostOptions {
    /// 只跑门禁与审阅，不碰站点写接口
    pub dry_run: bool,
    /// 本次最多发布多少条（skipped_duplicate 不计数）
    pub limit: Option<usize>,
    /// 只处理这些目标站的草稿（大小写不敏感；None = 全部）
    pub sites: Option<Vec<String>>,
    /// 真实发布前是否做草稿对账：下载器已挂目标站完成种子的草稿格子改写 Seeding。
    /// `--no-reconcile` 关闭；只关对账，不影响其他流程。
    pub reconcile: bool,
    /// 是否串行循环跑多轮（`--loop`）；单轮模式为 false。
    pub loop_mode: bool,
    /// 循环模式轮间间隔秒数（`--interval-secs`，默认 1800）。
    pub interval_secs: u64,
}

impl Default for AutopostOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            limit: None,
            sites: None,
            reconcile: true,
            loop_mode: false,
            interval_secs: 1800,
        }
    }
}

pub enum AutopostEvent {
    /// 进度/说明文本（CLI 直接打印，GUI 进日志面板）
    Log(String),
    DraftStart {
        name: String,
        source_site: String,
        target_site: String,
    },
    /// 该草稿的门禁矩阵（演练与真发都会发）
    Gates {
        key: String,
        site: String,
        gates: Vec<GateResult>,
    },
    /// 单条草稿终态：review_failed / blocked / skipped_duplicate / posted / failed
    Outcome {
        key: String,
        site: String,
        result: &'static str,
        url: Option<String>,
        error: Option<String>,
    },
}

#[derive(Default)]
pub struct AutopostSummary {
    pub posted: usize,
}

/// 单草稿发布执行器。生产路径是 `reseed::run_reseed_reviewed`（复用审阅产物）；
/// 测试注入假实现即可在不碰真实站点的前提下验证白名单/日限额/失败冷却/审计等编排安全闸。
/// 第六个参数是门禁通过的那份审阅产物——发布必须提交同一份字段（BUG-M7）。
pub type ReseedRunner<'a> = dyn FnMut(
        &Config,
        &reseed::ReseedRequest,
        &mut ContentIndex,
        &mut dyn FnMut(&str),
        Option<&crate::throttle::SiteThrottle>,
        Option<reseed::ReviewReport>,
    ) -> Result<reseed::ReseedOutcome>
    + 'a;

/// 单草稿审阅执行器。生产路径是 `reseed::review_reseed`；测试注入假实现即可
/// 在不碰真实站点的前提下验证门禁放行/拦截与 posted/blocked 记账。
pub type ReviewRunner<'a> = dyn FnMut(
        &Config,
        &reseed::ReseedRequest,
        &mut dyn FnMut(&str),
        Option<&crate::throttle::SiteThrottle>,
    ) -> Result<reseed::ReviewReport>
    + 'a;

/// 跑一遍自动发种。审计落在索引同目录 auto-audit.jsonl；posted 按北京时间日计限额。
pub fn run_autopost(
    cfg: &Config,
    opts: &AutopostOptions,
    emit: &mut impl FnMut(AutopostEvent),
) -> Result<AutopostSummary> {
    if !opts.dry_run {
        crate::config::ensure_publish_enabled(cfg)?;
    }
    // 跨进程互斥：CLI 计划任务 + 开着的 GUI 各自读同一份审计/账本会突破日限额
    let data_dir = cfg
        .coverage_state_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let _lock = crate::lock::RunLock::acquire(&data_dir, "state")?;
    if opts.dry_run {
        // 演练只跑审阅+门禁，不碰下载器：下载器停机/换机时也要能做门禁演练。
        // 演练不连下载器，对账跳过（拿不到种子列表）。
        return run_autopost_with(
            cfg,
            opts,
            None,
            emit,
            &mut |cfg, req, log, throttle| reseed::review_reseed(cfg, req, log, throttle),
            &mut |_, _, _, _, _, _| anyhow::bail!("dry-run 不应调用发布器（内部错误）"),
        );
    }
    let qb = QBittorrent::new(
        &cfg.downloader.url,
        &cfg.downloader.username,
        &cfg.downloader.password,
    )?;
    let hello = qb.test_connection().with_context(|| {
        "连不上下载器（boxsend.json 的 downloader/assistDownloaderID 指的那台）"
    })?;
    emit(AutopostEvent::Log(hello));
    // 对账只用一次全量列表：`torrents/info` 一次拉全量，不逐 hash 打接口。
    let torrents = qb
        .list_torrents()
        .with_context(|| "拉取下载器种子列表失败（对账/辅种都需要全量列表）")?;
    // 每轮开头先回访站点审核：之前发出去、站点还没放行的，审完了就趁这一轮把站点种子挂回 qB。
    // 回访自己出错只记日志，不挡后面的发种。
    recheck_reviews_once(cfg, &qb, emit);
    run_autopost_with(
        cfg,
        opts,
        Some(&torrents),
        emit,
        &mut |cfg, req, log, throttle| reseed::review_reseed(cfg, req, log, throttle),
        &mut |cfg, req, index, log, throttle, reviewed| match reviewed {
            Some(rep) => reseed::run_reseed_reviewed(cfg, &qb, req, index, log, throttle, rep),
            None => reseed::run_reseed(cfg, &qb, req, index, log, throttle),
        },
    )
}

/// 自动发种每轮开头的一次审核回访（只看记了审核结论的格子；`bsend followup` 不带参数时一样）。
/// 没有要回访的条目就不出声；回访出错只记日志，不影响后面的发种。
fn recheck_reviews_once(cfg: &Config, qb: &QBittorrent, emit: &mut impl FnMut(AutopostEvent)) {
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let opts = ReviewCheckOptions::default();
    let ran = {
        let mut log = |line: &str| emit(AutopostEvent::Log(format!("审核回访｜{line}")));
        crate::review::recheck_reviews(cfg, qb, &mut index, &opts, &mut log)
    };
    match ran {
        Ok(s) if s.selected == 0 => {}
        Ok(s) => emit(AutopostEvent::Log(format!(
            "审核回访：看了 {} 条详情页；仍在等审核 {}、仍是不通过或站上没有 {}；新记下等审核 {} / 不通过 {} / 站上已找不到 {}；审完收尾 {}；打不开 {}",
            s.checked, s.still_pending, s.still_bad, s.marked_pending, s.marked_rejected,
            s.marked_gone, s.settled, s.errors
        ))),
        Err(e) => emit(AutopostEvent::Log(format!(
            "审核回访没跑完（不影响发种）：{e:#}"
        ))),
    }
}

/// `run_autopost` 的编排主体（不含下载器连接与总闸）：调用方负责把 reviewer/runner
/// 接到真实 `review_reseed`/`run_reseed` 或测试假实现上。公开给测试与 GUI 复用。
#[doc(hidden)]
pub fn run_autopost_with(
    cfg: &Config,
    opts: &AutopostOptions,
    torrents: Option<&[LocalTorrent]>,
    emit: &mut impl FnMut(AutopostEvent),
    reviewer: &mut ReviewRunner<'_>,
    runner: &mut ReseedRunner<'_>,
) -> Result<AutopostSummary> {
    let mut summary = AutopostSummary::default();
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let mut report = CoverageReport::build(&index, &cfg.sites, 20, now_unix(), "");
    let allow: HashSet<String> = cfg.coverage.auto_reseed_sites.iter().cloned().collect();
    // 草稿对账：下载器里已挂目标站完成种子的草稿，格子从 ReseedDraft 改写 Seeding。
    // 纯判定在 `report::reconcile_marks`；这里只做落格 + save + 汇总日志，判定纯函数可单测。
    match torrents {
        Some(torrents) if opts.reconcile => {
            let h2k = index.hash_to_key();
            let marks = reconcile_marks(&report.drafts, torrents, &cfg.sites, &h2k, &index);
            if !marks.is_empty() {
                let now = now_unix();
                for m in &marks {
                    if let Some(entry) = index.contents.get_mut(&m.key) {
                        if let Some(cell) = entry.coverage.get_mut(&m.site) {
                            cell.status = CoverageStatus::Seeding;
                            cell.at = now;
                            cell.note = Some(m.note.clone());
                        }
                    }
                }
                index.save()?;
                let mut per_site: HashMap<String, usize> = HashMap::new();
                for m in &marks {
                    *per_site.entry(m.site.clone()).or_insert(0) += 1;
                }
                let parts: Vec<String> = per_site.iter().map(|(s, n)| format!("{s} {n}")).collect();
                emit(AutopostEvent::Log(format!(
                    "对账：{} 条草稿已挂种 → 改写 Seeding（{}）",
                    marks.len(),
                    parts.join("、")
                )));
                // 重建报告：否则本轮 report.drafts 还是旧草稿，循环会重复处理已挂种条目
                report = CoverageReport::build(&index, &cfg.sites, 20, now_unix(), "");
            }
        }
        Some(_) => emit(AutopostEvent::Log("--no-reconcile：跳过草稿对账".into())),
        None => {
            if opts.dry_run {
                emit(AutopostEvent::Log("演练不连下载器，跳过对账".into()));
            } else {
                emit(AutopostEvent::Log("未提供下载器种子列表，跳过对账".into()));
            }
        }
    }
    // 白名单只决定「哪些站可以发」，不再要求账本里先有草稿格。
    // 草稿从 qB 缺口实时派生：qB 没有该站 tracker、源种已定位、内容还在下载器里。
    let per_site_limit: HashMap<String, usize> = cfg
        .sites
        .iter()
        .map(|site| {
            let rules = cfg.coverage.rules_for(&site.id);
            (site.id.clone(), rules.daily_limit() as usize)
        })
        .collect();
    // 演练时白名单为空 = 全站预演（只审阅、不发布）；真实发布时白名单为空 =
    // 不发任何站，避免配置漏填后把全部站点都发一遍。
    let derive_allow: HashSet<String> = if opts.dry_run && allow.is_empty() {
        cfg.sites.iter().map(|s| s.id.clone()).collect()
    } else {
        allow.clone()
    };
    let gap_drafts = match torrents {
        Some(torrents) => report.derive_qb_gap_drafts(
            &index,
            &cfg.sites,
            &derive_allow,
            torrents,
            &per_site_limit,
        ),
        None => report.derive_qb_gap_drafts_from_index(
            &index,
            &cfg.sites,
            &derive_allow,
            &per_site_limit,
        ),
    };
    emit(AutopostEvent::Log(format!(
        "草稿由 qB 缺口派生：{} 条（{}）",
        gap_drafts,
        if allow.is_empty() && opts.dry_run {
            "未配置白名单，演练覆盖全部站点".to_string()
        } else {
            format!("白名单 {} 站，按每站每日额度截断", allow.len())
        }
    )));
    if allow.is_empty() {
        emit(AutopostEvent::Log(if opts.dry_run {
            "未配置 assist.autoReseedSites（自动发种白名单）——本次只做门禁演练，不会发布任何东西。"
                .into()
        } else {
            "未配置 assist.autoReseedSites（自动发种白名单）——真实发布不会发出任何东西。".into()
        }));
    }
    if gap_drafts == 0 {
        emit(AutopostEvent::Log(
            "没有转种草稿：qB 里没有可派生的缺口，或今日额度已经用完。".into(),
        ));
        return Ok(summary);
    }
    let audit_path = audit_path_of(cfg);
    let now = now_unix();
    let (mut posted_today, last_bad) = std::fs::read_to_string(&audit_path)
        .map(|text| parse_audit(&text, now))
        .unwrap_or_default();

    emit(AutopostEvent::Log(format!(
        "自动发种（{}）｜草稿 {} 条｜白名单 {} 站｜审计 {}\n",
        if opts.dry_run {
            "演练，不发布"
        } else {
            "真实发布"
        },
        report.drafts.len(),
        allow.len(),
        audit_path.display()
    )));
    // 一次自动发种共用一个节流器：上传预算/站间隔跨草稿累计，超量即停
    let throttle = crate::throttle::SiteThrottle::from_coverage(&cfg.coverage);
    // 演练模式不会 posted，`limit` 改为限制「实际审阅的草稿条数」，
    // 否则 --dry-run --limit N 会审完全部草稿，既慢又白耗站点请求。
    let mut drilled = 0usize;
    // `--limit` 是“本轮最多处理多少条”的执行上限，不是只看 Posted：
    // `posted_unresolved`（已上传但定位不到详情）同样消耗一次上传，必须计数，
    // 否则 hdbao 实测 `--limit 1` 会连发两条。
    let mut attempted = 0usize;
    for d in &report.drafts {
        // 长任务跨午夜时，失败冷却/日限额要按当前时刻算，不能用本轮开跑时的时间
        let now = now_unix();
        if let Some(l) = opts.limit {
            let used = if opts.dry_run { drilled } else { attempted };
            if used >= l {
                break;
            }
        }
        let site = &d.target_site;
        if let Some(ss) = &opts.sites {
            if !ss.iter().any(|s| s.eq_ignore_ascii_case(site)) {
                continue;
            }
        }
        // 内容类型黑名单：教育/成人等不进自动发种（只发影视、剧集、纪录片）。
        // 终态化：真实路径下把格子改写 Skipped 落账，第二轮起不再入队；幂等——
        // 已是 Skipped 的格子不会再出现在 report.drafts 里。dry-run 演练不落账本。
        if let Some(hit) = skip_hit(&d.name, &cfg.coverage.skip_keywords) {
            if !opts.dry_run {
                if let Some(entry) = index.contents.get_mut(&d.content_key) {
                    if let Some(cell) = entry.coverage.get_mut(site) {
                        cell.status = CoverageStatus::Skipped;
                        cell.at = now;
                        cell.note = Some(format!("命中内容类型黑名单「{hit}」"));
                    }
                }
                index.save()?;
            }
            emit(AutopostEvent::Log(format!(
                "跳过 {}：命中内容类型黑名单「{}」",
                d.name, hit
            )));
            continue;
        }
        // 白名单：真实发布永远要求；演练时空白名单 = 全站门禁演练，
        // 已配白名单则只演练「会真实发布的那部分」，与真发预览一致。
        let whitelisted = allow.contains(site);
        if !whitelisted && (!opts.dry_run || !allow.is_empty()) {
            emit(AutopostEvent::Log(format!(
                "跳过 {}：不在自动发种白名单",
                site
            )));
            continue;
        }
        if in_backoff(
            last_bad
                .get(&(d.content_key.clone(), site.clone()))
                .copied(),
            now,
            cfg.coverage.backoff_hours,
        ) {
            emit(AutopostEvent::Log(format!(
                "跳过 {}：上次拦截/失败还在冷却期内（{} 小时）",
                d.name, cfg.coverage.backoff_hours
            )));
            continue;
        }
        let rules = cfg.coverage.rules_for(site);
        let used = posted_today.get(site).copied().unwrap_or(0);
        if used >= rules.daily_limit() {
            emit(AutopostEvent::Log(format!(
                "跳过 {}：「{}」今日已自动发 {used}/{}",
                site,
                d.name,
                rules.daily_limit()
            )));
            continue;
        }
        if opts.dry_run {
            if let Some(l) = opts.limit {
                if drilled >= l {
                    break;
                }
            }
            drilled += 1;
        }
        let site_name = cfg
            .sites
            .iter()
            .find(|s| &s.id == site)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| site.clone());
        let req = reseed::request_from_draft(
            &d.content_key,
            &d.name,
            &d.source_site,
            &d.source_url,
            site,
        );
        emit(AutopostEvent::DraftStart {
            name: d.name.clone(),
            source_site: d.source_site.clone(),
            target_site: site.clone(),
        });
        let dry_run = opts.dry_run;
        let mut log = |line: &str| {
            if dry_run {
                emit(AutopostEvent::Log(format!("   {line}")));
            }
        };
        let rep = match reviewer(cfg, &req, &mut log, Some(&throttle)) {
            Ok(r) => r,
            Err(e) => {
                append_audit(
                    &audit_path,
                    serde_json::json!({"at": now, "site": site, "key": d.content_key,
                        "result": "review_failed", "error": format!("{e:#}")}),
                )?;
                emit(AutopostEvent::Outcome {
                    key: d.content_key.clone(),
                    site: site.clone(),
                    result: "review_failed",
                    url: None,
                    error: Some(format!("{e:#}")),
                });
                continue;
            }
        };
        let drill = reseed::analyze_prepared(site, &site_name, &rep.prepared, &rep.info);
        let core = drill.core_gaps();
        let type_label = drill
            .select_picks
            .iter()
            .find(|(n, _)| n == "type")
            .map(|(_, l)| l.clone());
        let descr_final = reseed::submitted_descr(&rep.prepared, &rep.info.descr);
        let results = {
            let input = gates::GateInput {
                rules: &rules,
                name: &rep.info.name,
                subtitle: &rep.info.subtitle,
                descr: descr_final,
                mediainfo: &rep.info.mediainfo,
                has_links: rep.info.has_media_links(),
                type_label: type_label.as_deref(),
                core_gaps: &core,
                required_gaps: &rep.prepared.required_gaps,
                audit_passed: rep.audit.as_ref().map(|a| a.passed),
            };
            gates::evaluate(&input)
        };
        let gate_json: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                serde_json::json!({"gate": r.gate, "verdict": r.verdict.as_str(), "message": r.message})
            })
            .collect();
        let pass = gates::all_passed(&results);
        emit(AutopostEvent::Gates {
            key: d.content_key.clone(),
            site: site.clone(),
            gates: results,
        });
        if opts.dry_run {
            emit(AutopostEvent::Log(format!(
                "   → {}（演练，未发布）\n",
                if pass { "门禁通过" } else { "门禁拦截" }
            )));
            continue;
        }
        if !pass {
            append_audit(
                &audit_path,
                serde_json::json!({"at": now, "site": site, "key": d.content_key,
                    "result": "blocked", "gates": gate_json}),
            )?;
            emit(AutopostEvent::Outcome {
                key: d.content_key.clone(),
                site: site.clone(),
                result: "blocked",
                url: None,
                error: None,
            });
            continue;
        }
        if !opts.dry_run {
            attempted += 1;
        }
        match runner(
            cfg,
            &req,
            &mut index,
            &mut |l| {
                emit(AutopostEvent::Log(format!("   {l}")));
            },
            Some(&throttle),
            Some(rep),
        ) {
            Ok(reseed::ReseedOutcome::Duplicate(dup)) => {
                index.save()?;
                let result = reseed::ReseedOutcome::Duplicate(dup.clone()).audit_result();
                append_audit(
                    &audit_path,
                    serde_json::json!({"site": site, "key": d.content_key,
                        "result": result, "url": dup.url, "note": dup.note, "gates": gate_json}),
                )?;
                // 等站点审核 / 审核没过：把站点原话带给界面（`bsend status` 的待跟进里也会列）
                let says_why = matches!(result, "failed" | "pending_review" | "review_rejected");
                emit(AutopostEvent::Outcome {
                    key: d.content_key.clone(),
                    site: site.clone(),
                    result,
                    url: dup.url,
                    error: says_why.then_some(dup.note),
                });
            }
            Ok(reseed::ReseedOutcome::Posted(url)) => {
                index.save()?;
                // 先落审计再记账：审计写失败就中止后续发布，避免日限额失守
                append_audit(
                    &audit_path,
                    serde_json::json!({"at": now, "site": site, "key": d.content_key,
                        "result": "posted", "url": url, "gates": gate_json}),
                )
                .with_context(|| {
                    format!("已发布 {url} 但审计落盘失败，停止后续发布以免超日限额，请手工补记")
                })?;
                summary.posted += 1;
                *posted_today.entry(site.clone()).or_insert(0) += 1;
                emit(AutopostEvent::Outcome {
                    key: d.content_key.clone(),
                    site: site.clone(),
                    result: "posted",
                    url: Some(url),
                    error: None,
                });
            }
            Err(e) => {
                append_audit(
                    &audit_path,
                    serde_json::json!({"at": now, "site": site, "key": d.content_key,
                        "result": "failed", "error": format!("{e:#}"), "gates": gate_json}),
                )?;
                emit(AutopostEvent::Outcome {
                    key: d.content_key.clone(),
                    site: site.clone(),
                    result: "failed",
                    url: None,
                    error: Some(format!("{e:#}")),
                });
            }
        }
    }
    emit(AutopostEvent::Log(if opts.dry_run {
        "结束：演练模式，未发布任何内容".into()
    } else {
        "结束：已按门禁执行（明细见 auto-audit.jsonl）".into()
    }));
    Ok(summary)
}

/// 审计文件位置：索引同目录 auto-audit.jsonl
pub fn audit_path_of(cfg: &Config) -> PathBuf {
    cfg.coverage_state_path()
        .parent()
        .map(|p| p.join("auto-audit.jsonl"))
        .unwrap_or_else(|| PathBuf::from("auto-audit.jsonl"))
}

/// 各站今日 posted 计数（北京时间日界，UTC+8）
pub type PostedToday = HashMap<String, u32>;
/// (内容, 站点) → (最近一次拦截/失败时间, 是否门禁拦截)，失败冷却判定用
pub type LastBad = HashMap<(String, String), (i64, bool)>;

/// 读审计 JSONL：今日各站 posted 计数 + 每 (内容, 站点) 最近一次拦截/失败（失败冷却用）。
/// 日界按北京时间（UTC+8）：PT 站按北京 00:00 重置，UTC 日界会错位到北京 08:00。
pub fn parse_audit(text: &str, now: i64) -> (PostedToday, LastBad) {
    const UTC8: i64 = 8 * 3600;
    let today_start = now - (now + UTC8).rem_euclid(86400);
    let mut posted_today: HashMap<String, u32> = HashMap::new();
    let mut last_bad: HashMap<(String, String), (i64, bool)> = HashMap::new();
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let result = v.get("result").and_then(|r| r.as_str()).unwrap_or("");
            let at = v.get("at").and_then(|a| a.as_i64()).unwrap_or(0);
            if result == "posted" && at >= today_start {
                if let Some(s) = v.get("site").and_then(|s| s.as_str()) {
                    *posted_today.entry(s.to_string()).or_insert(0) += 1;
                }
            }
            if matches!(result, "blocked" | "failed" | "review_failed") {
                if let (Some(k), Some(s)) = (
                    v.get("key").and_then(|s| s.as_str()),
                    v.get("site").and_then(|s| s.as_str()),
                ) {
                    let slot = last_bad
                        .entry((k.to_string(), s.to_string()))
                        .or_insert((0, false));
                    if at > slot.0 {
                        *slot = (at, result == "blocked");
                    }
                }
            }
        }
    }
    (posted_today, last_bad)
}

/// 内容黑名单命中：ASCII 关键词按「词」比（`Adult` 不会命中 `Adulthood`），
/// 中文关键词按子串比（中文没有空格分词）。名称先统一小写，非字母数字折成空格。
fn skip_hit<'a>(name: &str, keywords: &'a [String]) -> Option<&'a str> {
    let lower = name.to_lowercase();
    let tokenized: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    for kw in keywords {
        let k = kw.to_lowercase();
        let ascii = k.is_ascii();
        let hit = if ascii {
            tokenized.split_whitespace().any(|t| t == k)
        } else {
            lower.contains(&k)
        };
        if hit {
            return Some(kw.as_str());
        }
    }
    None
}

/// 失败重试冷却：同一内容 + 同一站点在 `hours` 小时内不再重试。
/// `hours = 0`（默认）表示不冷却——手动跑一轮时该不该重试由人决定，
/// 冷却只会把刚修好问题的验证拖到几小时后。门禁拦截与失败共用同一个窗口，
/// 需要更保守时把 `assist.backoffHours` 填大。
pub fn in_backoff(last: Option<(i64, bool)>, now: i64, hours: u32) -> bool {
    if hours == 0 {
        return false;
    }
    match last {
        Some((at, _is_block)) => now - at < i64::from(hours) * 3600,
        None => false,
    }
}

/// 审计追加（JSONL）。写失败必须中止本轮：posted 记账丢失会让日限额失守、失败冷却失效。
/// `at` 一律取**写入时刻**：循环起点的时间只用于白名单/日限额/失败冷却判定，
/// 单条发布可能跨午夜（校验等待最长 verifyWaitSec），记账不能沿用旧快照。
fn append_audit(path: &Path, mut entry: serde_json::Value) -> Result<()> {
    use std::io::Write as _;
    if let Some(obj) = entry.as_object_mut() {
        obj.insert("at".into(), serde_json::json!(now_unix()));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).context("审计目录创建失败")?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("审计文件打不开：{}", path.display()))?;
    writeln!(f, "{entry}").context("审计写入失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_windows() {
        let now = 1_800_000_000i64;
        // 默认不冷却：`hours = 0` 直接放行，哪怕是刚失败的那一条
        assert!(!in_backoff(None, now, 0));
        assert!(!in_backoff(Some((now - 60, true)), now, 0));
        // 显式给窗口才冷却：6 小时内挡住，7 小时后放行
        assert!(in_backoff(Some((now - 5 * 3600, false)), now, 6));
        assert!(!in_backoff(Some((now - 7 * 3600, false)), now, 6));
        // 门禁拦截与失败共用同一个窗口
        assert!(in_backoff(Some((now - 5 * 3600, true)), now, 6));
    }

    #[test]
    fn skip_hit_uses_token_boundaries_for_ascii() {
        let kws: Vec<String> = vec!["adult".into(), "儿童".into(), "阿U".into()];
        assert_eq!(skip_hit("Adult.World.2024", &kws), Some("adult"));
        assert_eq!(skip_hit("Adulthood.2019", &kws), None);
        assert_eq!(skip_hit("儿童画教材40套", &kws), Some("儿童"));
        assert_eq!(skip_hit("阿U学英语.S01", &kws), Some("阿U"));
        assert_eq!(skip_hit("Children.of.Men.2006", &kws), None);
    }

    #[test]
    fn parse_audit_counts_today_and_latest_bad() {
        let now = 1_800_000_000i64; // 08:00 UTC = 北京 16:00，不在日界上
        let text = format!(
            concat!(
                "{{\"at\":{},\"site\":\"sbpt\",\"key\":\"m:a\",\"result\":\"posted\"}}\n",
                "{{\"at\":{},\"site\":\"sbpt\",\"key\":\"m:b\",\"result\":\"posted\"}}\n",
                "{{\"at\":{},\"site\":\"sbpt\",\"key\":\"m:c\",\"result\":\"blocked\"}}\n",
                "{{\"at\":{},\"site\":\"sbpt\",\"key\":\"m:c\",\"result\":\"failed\"}}\n",
                "not-json\n"
            ),
            now - 10,
            now - 86400 - 10, // 昨天的 posted 不计今日
            now - 7200,
            now - 3600,
        );
        let (posted, bad) = parse_audit(&text, now);
        assert_eq!(posted.get("sbpt").copied().unwrap_or(0), 1);
        // 同键取最新：failed（非拦截）覆盖更早的 blocked
        assert_eq!(
            bad.get(&("m:c".into(), "sbpt".into())),
            Some(&(now - 3600, false))
        );
    }

    #[test]
    fn parse_audit_day_boundary_is_beijing_midnight() {
        // (t+28800) % 86400 == 0 即北京 00:00（t % 86400 必须是 57600）
        let midnight = 1_800_028_800i64;
        assert_eq!((midnight + 28800) % 86400, 0);
        let text = format!(
            concat!(
                "{{\"at\":{},\"site\":\"sbpt\",\"key\":\"m:a\",\"result\":\"posted\"}}\n",
                "{{\"at\":{},\"site\":\"sbpt\",\"key\":\"m:b\",\"result\":\"posted\"}}\n"
            ),
            midnight - 1,
            midnight,
        );
        // 北京 00:00 整：只有 00:00:00 那条算今天（前一天 23:59:59 不算）
        let (posted, _) = parse_audit(&text, midnight);
        assert_eq!(posted.get("sbpt").copied().unwrap_or(0), 1);
        // 北京 00:00 前 1 秒：此刻的「今天」从 D-86400 起，D-86401 那条属昨天
        let before = midnight - 1;
        let text_before = format!(
            concat!(
                "{{\"at\":{},\"site\":\"sbpt\",\"key\":\"m:a\",\"result\":\"posted\"}}\n",
                "{{\"at\":{},\"site\":\"sbpt\",\"key\":\"m:b\",\"result\":\"posted\"}}\n"
            ),
            before - 2,
            before - 86400,
        );
        let (n, _) = parse_audit(&text_before, before);
        assert_eq!(n.get("sbpt").copied().unwrap_or(0), 1);
    }

    // ---------------- 编排层安全闸（注入式，零网络） ----------------

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("bsend-autopost-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 造一个带 N 条 sbpt 草稿的索引 + 对应配置；daily_limit 由调用方定
    fn cfg_with_drafts(dir: &std::path::Path, daily_limit: u32, drafts: usize) -> Config {
        let mut rules = std::collections::BTreeMap::new();
        rules.insert(
            "sbpt".to_string(),
            crate::gates::SiteRules {
                daily_limit: Some(daily_limit),
                ..Default::default()
            },
        );
        let cfg = Config {
            config_path: dir.join("boxsend.json"),
            data_dir: Some(dir.to_path_buf()),
            downloader: crate::config::DownloaderSettings {
                url: "http://127.0.0.1:1".into(),
                username: "u".into(),
                password: "p".into(),
                save_path: None,
                category: None,
                default_up_limit: 0,
                site_up_limits: std::collections::HashMap::new(),
            },
            sites: vec![crate::config::SiteConf {
                id: "sbpt".into(),
                name: "SBPT".into(),
                url: "https://sbpt.link/".into(),
                framework: crate::sites::Framework::NexusPHP,
                search_url: None,
                api_key: None,
                announce_hosts: vec!["sbpt.link".into()],
                announce_url: None,
                poster_field: None,
                screenshot_field: None,
            }],
            coverage: crate::config::CoverageSettings {
                auto_reseed_sites: vec!["sbpt".into()],
                reseed_publish_enabled: true,
                reseed_rules: rules,
                per_site_interval_sec: 0.0,
                jitter_sec: 0.0,
                ..Default::default()
            },
            user_agent: "bsend-test".into(),
            cookies: std::collections::HashMap::new(),
            cookie_cloud: None,
            skipped_sites: Vec::new(),
        };
        let mut index = crate::index::ContentIndex::load(&cfg.coverage_state_path());
        for i in 0..drafts {
            let key = format!("m:test{i:016x}");
            index.contents.insert(
                key,
                crate::index::ContentEntry {
                    name: format!("Draft {i}"),
                    total_size: 1,
                    file_count: 1,
                    manifest_digest: "0".repeat(16),
                    torrents: std::collections::BTreeMap::from([(
                        "a".repeat(40),
                        crate::index::TorrentMeta {
                            save_path: "/data".into(),
                            category: None,
                            last_seen: 0,
                            tracker_hosts: vec![],
                            hash_v2: None,
                            added_on: 0,
                        },
                    )]),
                    tracker_hosts: vec![],
                    first_seen: 0,
                    gone_since: None,
                    coverage: std::collections::BTreeMap::from([(
                        "sbpt".to_string(),
                        crate::index::SiteCoverage {
                            status: crate::index::CoverageStatus::ReseedDraft,
                            at: 0,
                            url: None,
                            note: None,
                            verify_hash: None,
                        },
                    )]),
                    source: Some(crate::index::SourceRecord {
                        site_id: "luckpt".into(),
                        detail_url: "https://pt.luckpt.de/details.php?id=1".into(),
                        resolved_at: 0,
                        miss_at: None,
                    }),
                    forbid_reseed: false,
                },
            );
        }
        index.save().unwrap();
        cfg
    }

    fn collect_logs<'a>(logs: &'a mut Vec<String>) -> impl FnMut(AutopostEvent) + 'a {
        move |e| {
            if let AutopostEvent::Log(m) = e {
                logs.push(m);
            }
        }
    }

    /// 这些用例在审阅前就被安全闸拦下：走到审阅即说明门禁顺序错了
    fn no_review(
        _: &Config,
        _: &reseed::ReseedRequest,
        _: &mut dyn FnMut(&str),
        _: Option<&crate::throttle::SiteThrottle>,
    ) -> Result<reseed::ReviewReport> {
        panic!("该用例不应走到审阅")
    }

    #[test]
    fn empty_whitelist_real_run_publishes_nothing() {
        let dir = temp_dir("whitelist");
        let mut cfg = cfg_with_drafts(&dir, 2, 1);
        cfg.coverage.auto_reseed_sites.clear();
        let mut called = 0;
        let mut runner = |_: &Config,
                          _: &reseed::ReseedRequest,
                          _: &mut ContentIndex,
                          _: &mut dyn FnMut(&str),
                          _: Option<&crate::throttle::SiteThrottle>,
                          _: Option<reseed::ReviewReport>| {
            called += 1;
            Ok(reseed::ReseedOutcome::Posted(
                "https://sbpt.link/details.php?id=1".to_string(),
            ))
        };
        let mut logs = Vec::new();
        let summary = run_autopost_with(
            &cfg,
            &AutopostOptions {
                dry_run: false,
                limit: None,
                sites: None,
                ..Default::default()
            },
            None,
            &mut collect_logs(&mut logs),
            &mut no_review,
            &mut runner,
        )
        .unwrap();
        assert_eq!(called, 0, "空白名单不能调用发布器");
        assert_eq!(summary.posted, 0);
        assert!(logs
            .iter()
            .any(|l| l.contains("未配置 assist.autoReseedSites")));
        assert!(logs.iter().any(|l| l.contains("没有转种草稿")));
    }

    #[test]
    fn autopost_daily_limit_preskips_without_touching_site() {
        let dir = temp_dir("daily");
        let cfg = cfg_with_drafts(&dir, 1, 2);
        // 今日已发 1 条（北京时间日界由 parse_audit 负责，这里直接写 now）
        let audit = audit_path_of(&cfg);
        std::fs::write(
            &audit,
            format!(
                "{{\"at\":{},\"site\":\"sbpt\",\"key\":\"m:x\",\"result\":\"posted\"}}\n",
                now_unix()
            ),
        )
        .unwrap();
        let mut called = 0;
        let mut runner = |_: &Config,
                          _: &reseed::ReseedRequest,
                          _: &mut ContentIndex,
                          _: &mut dyn FnMut(&str),
                          _: Option<&crate::throttle::SiteThrottle>,
                          _: Option<reseed::ReviewReport>| {
            called += 1;
            Ok(reseed::ReseedOutcome::Duplicate(reseed::DuplicateOutcome {
                status: crate::index::CoverageStatus::Assisted,
                url: None,
                note: "已覆盖".into(),
            }))
        };
        let mut logs = Vec::new();
        let summary = run_autopost_with(
            &cfg,
            &AutopostOptions {
                dry_run: false,
                limit: None,
                sites: None,
                ..Default::default()
            },
            None,
            &mut collect_logs(&mut logs),
            &mut no_review,
            &mut runner,
        )
        .unwrap();
        assert_eq!(called, 0, "日限额已满不能调用发布器");
        assert_eq!(summary.posted, 0);
        assert!(logs.iter().any(|l| l.contains("今日已自动发")));
    }

    #[test]
    fn autopost_backoff_skips_recently_blocked_draft() {
        let dir = temp_dir("backoff");
        let mut cfg = cfg_with_drafts(&dir, 2, 1);
        // 默认 0 = 不冷却，这里显式打开 6 小时窗口来测这条闸
        cfg.coverage.backoff_hours = 6;
        let key = "m:test0000000000000000";
        let audit = audit_path_of(&cfg);
        std::fs::write(
            &audit,
            format!(
                "{{\"at\":{},\"site\":\"sbpt\",\"key\":\"{key}\",\"result\":\"blocked\"}}\n",
                now_unix() - 3600
            ),
        )
        .unwrap();
        let mut called = 0;
        let mut runner = |_: &Config,
                          _: &reseed::ReseedRequest,
                          _: &mut ContentIndex,
                          _: &mut dyn FnMut(&str),
                          _: Option<&crate::throttle::SiteThrottle>,
                          _: Option<reseed::ReviewReport>| {
            called += 1;
            Ok(reseed::ReseedOutcome::Duplicate(reseed::DuplicateOutcome {
                status: crate::index::CoverageStatus::Assisted,
                url: None,
                note: "已覆盖".into(),
            }))
        };
        let mut logs = Vec::new();
        let _ = run_autopost_with(
            &cfg,
            &AutopostOptions {
                dry_run: false,
                limit: None,
                sites: None,
                ..Default::default()
            },
            None,
            &mut collect_logs(&mut logs),
            &mut no_review,
            &mut runner,
        )
        .unwrap();
        assert_eq!(called, 0, "冷却期内不能调用发布器");
        assert!(logs.iter().any(|l| l.contains("冷却期")));
    }

    #[test]
    fn autopost_site_filter_and_limit_stop_before_site_contact() {
        let dir = temp_dir("filter");
        let cfg = cfg_with_drafts(&dir, 2, 2);
        let mut called = 0;
        let mut runner = |_: &Config,
                          _: &reseed::ReseedRequest,
                          _: &mut ContentIndex,
                          _: &mut dyn FnMut(&str),
                          _: Option<&crate::throttle::SiteThrottle>,
                          _: Option<reseed::ReviewReport>| {
            called += 1;
            Ok(reseed::ReseedOutcome::Duplicate(reseed::DuplicateOutcome {
                status: crate::index::CoverageStatus::Assisted,
                url: None,
                note: "已覆盖".into(),
            }))
        };
        // 站点过滤：只看 luckpt，草稿全是 sbpt
        let _ = run_autopost_with(
            &cfg,
            &AutopostOptions {
                dry_run: false,
                limit: None,
                sites: Some(vec!["luckpt".into()]),
                ..Default::default()
            },
            None,
            &mut |_| {},
            &mut no_review,
            &mut runner,
        )
        .unwrap();
        // limit 0：立即停
        let _ = run_autopost_with(
            &cfg,
            &AutopostOptions {
                dry_run: false,
                limit: Some(0),
                sites: None,
                ..Default::default()
            },
            None,
            &mut |_| {},
            &mut no_review,
            &mut runner,
        )
        .unwrap();
        assert_eq!(called, 0, "过滤/限额为零时不能碰站点");
    }

    #[test]
    fn append_audit_fails_closed_on_unwritable_path() {
        let dir = temp_dir("audit-fail");
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        // 父路径是文件 → 目录创建/打开必然失败，必须把错误 surface 出来
        let path = blocker.join("auto-audit.jsonl");
        assert!(append_audit(&path, serde_json::json!({"at": 1})).is_err());
    }

    #[test]
    fn append_audit_stamps_write_time_not_stale_now() {
        let dir = temp_dir("audit-time");
        let path = dir.join("auto-audit.jsonl");
        // 调用方传的 at=1 是「循环起点」的旧快照；落盘必须改成写入时刻
        append_audit(&path, serde_json::json!({"at": 1, "site": "sbpt"})).unwrap();
        let line = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        let at = v.get("at").and_then(|a| a.as_i64()).unwrap();
        assert!(
            (now_unix() - at).abs() < 60,
            "at 必须是写入时刻（差 {} 秒）",
            now_unix() - at
        );
    }

    #[test]
    fn duplicate_outcomes_map_to_distinct_audit_results() {
        use crate::index::CoverageStatus as S;
        let outcome = |st| {
            reseed::ReseedOutcome::Duplicate(reseed::DuplicateOutcome {
                status: st,
                url: None,
                note: "x".into(),
            })
        };
        assert_eq!(outcome(S::Assisted).audit_result(), "skipped_duplicate");
        assert_eq!(outcome(S::OtherVersion).audit_result(), "other_version");
        assert_eq!(outcome(S::ReviewPending).audit_result(), "existed_pending");
        assert_eq!(
            outcome(S::Failed).audit_result(),
            "failed",
            "复核失败必须走失败语义（失败冷却 6h），不能记成重复"
        );
        assert_eq!(
            reseed::ReseedOutcome::Posted("u".into()).audit_result(),
            "posted"
        );
    }

    #[test]
    fn dry_run_does_not_require_downloader() {
        // 无草稿时演练应立即返回：下载器 URL 指向不可达端口，若仍去连接会 Err
        let dir = temp_dir("dry-no-qb");
        let cfg = cfg_with_drafts(&dir, 2, 0);
        let mut logs = Vec::new();
        let summary = run_autopost(
            &cfg,
            &AutopostOptions {
                dry_run: true,
                limit: None,
                sites: None,
                ..Default::default()
            },
            &mut collect_logs(&mut logs),
        )
        .unwrap();
        assert_eq!(summary.posted, 0);
        assert!(logs.iter().any(|l| l.contains("没有转种草稿")));
    }

    /// 造一份能过全部门禁的审阅产物：与真实上传字段同一套命名约定，
    /// 表单里没有下拉 = 无核心缺口。
    fn passing_review(name: &str) -> reseed::ReviewReport {
        let info = reseed::ReleaseInfo {
            name: name.to_string(),
            descr: "简介 https://www.imdb.com/title/tt1234567/".into(),
            mediainfo: "General\nFormat : Matroska".into(),
            imdb: Some("tt1234567".into()),
            ..Default::default()
        };
        reseed::ReviewReport {
            prepared: reseed::PreparedUpload {
                action: "https://sbpt.link/upload.php".into(),
                form_url: "https://sbpt.link/upload.php".into(),
                fields: vec![
                    ("name".into(), info.name.clone()),
                    ("descr".into(), info.descr.clone()),
                    ("media_info".into(), info.mediainfo.clone()),
                    ("imdb_url".into(), info.descr.clone()),
                ],
                picked_tags: Vec::new(),
                tag_warn: None,
                select_notes: Vec::new(),
                required_gaps: Vec::new(),
                form_html: "<html><body><form></form></body></html>".into(),
                poster_field: None,
                screenshot_field: None,
            },
            info,
            preflight: Default::default(),
            audit: None,
            audit_note: None,
            carry: None,
        }
    }

    #[test]
    fn dry_run_with_empty_whitelist_drills_all_drafts() {
        // 空白名单 + 演练：应逐条跑门禁（不发布），而不是直接全跳过
        let dir = temp_dir("dry-all");
        let mut cfg = cfg_with_drafts(&dir, 2, 1);
        cfg.coverage.auto_reseed_sites.clear();
        let mut reviewed = 0;
        let mut runner_called = 0;
        let mut no_runner = |_: &Config,
                             _: &reseed::ReseedRequest,
                             _: &mut ContentIndex,
                             _: &mut dyn FnMut(&str),
                             _: Option<&crate::throttle::SiteThrottle>,
                             _: Option<reseed::ReviewReport>| {
            runner_called += 1;
            Ok(reseed::ReseedOutcome::Posted("u".into()))
        };
        let mut logs = Vec::new();
        let summary = run_autopost_with(
            &cfg,
            &AutopostOptions {
                dry_run: true,
                limit: None,
                sites: None,
                ..Default::default()
            },
            None,
            &mut collect_logs(&mut logs),
            &mut |_: &Config,
                  req: &reseed::ReseedRequest,
                  _: &mut dyn FnMut(&str),
                  _: Option<&crate::throttle::SiteThrottle>| {
                reviewed += 1;
                Ok(passing_review(&req.name))
            },
            &mut no_runner,
        )
        .unwrap();
        assert_eq!(reviewed, 1, "空白名单演练必须逐条审阅");
        assert_eq!(runner_called, 0, "演练不能调用发布器");
        assert_eq!(summary.posted, 0);
        assert!(logs.iter().any(|l| l.contains("门禁通过")));
    }

    #[test]
    fn dry_run_with_configured_whitelist_skips_unlisted_site() {
        // 已配白名单时，演练只覆盖会真实发布的站点（预览与真发一致）
        let dir = temp_dir("dry-whitelist");
        let mut cfg = cfg_with_drafts(&dir, 2, 1);
        cfg.coverage.auto_reseed_sites = vec!["luckpt".into()];
        let mut reviewed = 0;
        let mut no_runner = |_: &Config,
                             _: &reseed::ReseedRequest,
                             _: &mut ContentIndex,
                             _: &mut dyn FnMut(&str),
                             _: Option<&crate::throttle::SiteThrottle>,
                             _: Option<reseed::ReviewReport>| {
            Ok(reseed::ReseedOutcome::Posted("u".into()))
        };
        let mut logs = Vec::new();
        let _ = run_autopost_with(
            &cfg,
            &AutopostOptions {
                dry_run: true,
                limit: None,
                sites: None,
                ..Default::default()
            },
            None,
            &mut collect_logs(&mut logs),
            &mut |_: &Config,
                  _: &reseed::ReseedRequest,
                  _: &mut dyn FnMut(&str),
                  _: Option<&crate::throttle::SiteThrottle>| {
                reviewed += 1;
                anyhow::bail!("白名单外的站不该被审阅")
            },
            &mut no_runner,
        )
        .unwrap();
        assert_eq!(reviewed, 0);
        assert!(logs.iter().any(|l| l.contains("没有转种草稿")));
    }

    #[test]
    fn autopost_limit_counts_posted_unresolved_attempts() {
        // 已上传但定位不到详情（posted_unresolved）不记 posted 限额，但仍是
        // 一次真实上传，`--limit` 必须计数；hdbao 实测 --limit 1 连发两条。
        let dir = temp_dir("limit-unresolved");
        let cfg = cfg_with_drafts(&dir, 2, 2);
        let mut runner_calls = 0usize;
        let mut runner = |_: &Config,
                          _: &reseed::ReseedRequest,
                          _: &mut ContentIndex,
                          _: &mut dyn FnMut(&str),
                          _: Option<&crate::throttle::SiteThrottle>,
                          _: Option<reseed::ReviewReport>| {
            runner_calls += 1;
            Ok(reseed::ReseedOutcome::Duplicate(reseed::DuplicateOutcome {
                status: CoverageStatus::ReviewPending,
                url: None,
                note: "posted_unresolved：站上已发布但没定位到详情".into(),
            }))
        };
        let mut logs = Vec::new();
        let summary = run_autopost_with(
            &cfg,
            &AutopostOptions {
                dry_run: false,
                limit: Some(1),
                sites: None,
                ..Default::default()
            },
            None,
            &mut collect_logs(&mut logs),
            &mut |_: &Config,
                  req: &reseed::ReseedRequest,
                  _: &mut dyn FnMut(&str),
                  _: Option<&crate::throttle::SiteThrottle>| {
                Ok(passing_review(&req.name))
            },
            &mut runner,
        )
        .unwrap();
        assert_eq!(runner_calls, 1, "--limit 1 只能上传一次");
        assert_eq!(summary.posted, 0, "posted_unresolved 仍不应占日限额");
    }

    #[test]
    fn dry_run_limit_caps_reviewed_drafts() {
        // 演练不会 posted，limit 必须限制「实际审阅条数」，否则会把全部草稿都跑一遍
        let dir = temp_dir("dry-limit");
        let cfg = cfg_with_drafts(&dir, 2, 3);
        let mut reviewed = 0;
        let mut no_runner = |_: &Config,
                             _: &reseed::ReseedRequest,
                             _: &mut ContentIndex,
                             _: &mut dyn FnMut(&str),
                             _: Option<&crate::throttle::SiteThrottle>,
                             _: Option<reseed::ReviewReport>| {
            Ok(reseed::ReseedOutcome::Posted("u".into()))
        };
        let _ = run_autopost_with(
            &cfg,
            &AutopostOptions {
                dry_run: true,
                limit: Some(1),
                sites: None,
                ..Default::default()
            },
            None,
            &mut |_| {},
            &mut |_: &Config,
                  req: &reseed::ReseedRequest,
                  _: &mut dyn FnMut(&str),
                  _: Option<&crate::throttle::SiteThrottle>| {
                reviewed += 1;
                Ok(passing_review(&req.name))
            },
            &mut no_runner,
        )
        .unwrap();
        assert_eq!(reviewed, 1, "演练 limit 必须限制审阅条数");
    }

    #[test]
    fn autopost_writes_posted_and_blocked_audit() {
        // TEST-L4：门禁通过 → 发布并记 posted；门禁拦截 → 记 blocked（都不触发发布器）
        let dir = temp_dir("audit-branches");
        let cfg = cfg_with_drafts(&dir, 2, 2);
        let mut reviewed = 0usize;
        let mut posted_runner_called = 0;
        let mut runner = |_: &Config,
                          _: &reseed::ReseedRequest,
                          _: &mut ContentIndex,
                          _: &mut dyn FnMut(&str),
                          _: Option<&crate::throttle::SiteThrottle>,
                          review: Option<reseed::ReviewReport>| {
            posted_runner_called += 1;
            // BUG-M7：门禁通过后必须把「审阅的那一份」交给发布器，不能在发布器里再构造
            let rep = review.expect("门禁通过后应把审阅产物交给发布器（同一份字段）");
            assert!(
                rep.prepared
                    .fields
                    .iter()
                    .any(|(n, v)| n == "name" && v == &rep.info.name),
                "发布字段必须来自门禁所见的审阅产物：{:?}",
                rep.prepared.fields
            );
            Ok(reseed::ReseedOutcome::Posted(
                "https://sbpt.link/details.php?id=9".into(),
            ))
        };
        let mut logs = Vec::new();
        let summary = run_autopost_with(
            &cfg,
            &AutopostOptions {
                dry_run: false,
                limit: None,
                sites: None,
                ..Default::default()
            },
            None,
            &mut collect_logs(&mut logs),
            &mut |_: &Config,
                  req: &reseed::ReseedRequest,
                  _: &mut dyn FnMut(&str),
                  _: Option<&crate::throttle::SiteThrottle>| {
                reviewed += 1;
                let mut rep = passing_review(&req.name);
                if reviewed == 1 {
                    Ok(rep)
                } else {
                    // 第二条缺简介/MediaInfo：G2/G3 必拦
                    rep.info.descr.clear();
                    rep.info.mediainfo.clear();
                    rep.prepared
                        .fields
                        .retain(|(n, _)| n != "descr" && n != "media_info" && n != "imdb_url");
                    Ok(rep)
                }
            },
            &mut runner,
        )
        .unwrap();
        assert_eq!(reviewed, 2, "两条草稿都要审阅");
        assert_eq!(posted_runner_called, 1, "只有门禁通过的那条能发布");
        assert_eq!(summary.posted, 1);

        let audit = std::fs::read_to_string(audit_path_of(&cfg)).unwrap();
        let posted = audit
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|v| v["result"] == "posted")
            .expect("要有 posted 记录");
        assert_eq!(posted["url"], "https://sbpt.link/details.php?id=9");
        assert!(posted["gates"].is_array(), "posted 记录要带门禁明细");
        let blocked = audit
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|v| v["result"] == "blocked")
            .expect("要有 blocked 记录");
        assert!(blocked["gates"].is_array(), "blocked 记录要带门禁明细");
    }

    #[test]
    fn reconcile_rewrites_draft_cell_to_seeding() {
        // 下载器已有目标站 100% 种子：对账把草稿格改写 Seeding，本轮不再入队
        let dir = temp_dir("reconcile");
        let cfg = cfg_with_drafts(&dir, 2, 1);
        let torrents = vec![LocalTorrent::new(
            &"a".repeat(40),
            None,
            "Draft 0",
            "",
            None,
            vec![],
            1,
            1.0,
            vec!["sbpt.link".to_string()],
            0,
        )];
        let mut logs = Vec::new();
        let summary = run_autopost_with(
            &cfg,
            &AutopostOptions::default(),
            Some(&torrents),
            &mut collect_logs(&mut logs),
            &mut no_review,
            &mut |_, _, _, _, _, _| Ok(reseed::ReseedOutcome::Posted("u".into())),
        )
        .unwrap();
        assert_eq!(summary.posted, 0, "对账改写后草稿已不在本轮队列");
        assert!(logs.iter().any(|l| l.contains("对账：1 条草稿已挂种")));
        let idx = ContentIndex::load(&cfg.coverage_state_path());
        let cell = idx
            .contents
            .get("m:test0000000000000000")
            .unwrap()
            .coverage
            .get("sbpt")
            .unwrap();
        assert_eq!(cell.status, CoverageStatus::Seeding);
        assert_eq!(cell.note.as_deref(), Some("下载器已挂该站 tracker"));
    }

    #[test]
    fn skip_keyword_draft_cell_settles_to_skipped() {
        // 命中黑名单：真实路径落 Skipped 终态格，第二轮起不再入队；审阅器不应被调用
        let dir = temp_dir("skipped");
        let mut cfg = cfg_with_drafts(&dir, 2, 1);
        cfg.coverage.skip_keywords = vec!["draft".into()];
        let mut logs = Vec::new();
        let summary = run_autopost_with(
            &cfg,
            &AutopostOptions::default(),
            None,
            &mut collect_logs(&mut logs),
            &mut no_review,
            &mut |_, _, _, _, _, _| Ok(reseed::ReseedOutcome::Posted("u".into())),
        )
        .unwrap();
        assert_eq!(summary.posted, 0);
        assert!(logs
            .iter()
            .any(|l| l.contains("命中内容类型黑名单「draft」")));
        let idx = ContentIndex::load(&cfg.coverage_state_path());
        let cell = idx
            .contents
            .get("m:test0000000000000000")
            .unwrap()
            .coverage
            .get("sbpt")
            .unwrap();
        assert_eq!(cell.status, CoverageStatus::Skipped);
        assert!(cell.note.as_deref().unwrap().contains("draft"));
    }
}
