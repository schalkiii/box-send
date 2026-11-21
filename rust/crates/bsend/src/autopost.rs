//! 自动发种核心：草稿 → 审阅 → 门禁 → 发布，白名单 + 日限额 + 退避 + 审计留痕。
//! CLI 与 GUI 共用本模块：调用方只渲染事件，不重复实现流程逻辑。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::config::Config;
use crate::coverage::now_unix;
use crate::gates::{self, GateResult};
use crate::index::ContentIndex;
use crate::qbittorrent::QBittorrent;
use crate::report::CoverageReport;
use crate::reseed;

pub struct AutopostOptions {
    /// 只跑门禁与审阅，不碰站点写接口
    pub dry_run: bool,
    /// 本次最多发布多少条（skipped_duplicate 不计数）
    pub limit: Option<usize>,
    /// 只处理这些目标站的草稿（大小写不敏感；None = 全部）
    pub sites: Option<Vec<String>>,
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

/// 单草稿发布执行器。生产路径是 `reseed::run_reseed`；测试注入假实现即可
/// 在不碰真实站点的前提下验证白名单/日限额/退避/审计等编排安全闸。
pub type ReseedRunner<'a> = dyn FnMut(
        &Config,
        &reseed::ReseedRequest,
        &mut ContentIndex,
        &mut dyn FnMut(&str),
        Option<&crate::throttle::SiteThrottle>,
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
        // 演练只跑审阅+门禁，不碰下载器：下载器停机/换机时也要能做门禁演练
        return run_autopost_with(
            cfg,
            opts,
            emit,
            &mut |cfg, req, log, throttle| reseed::review_reseed(cfg, req, log, throttle),
            &mut |_, _, _, _, _| anyhow::bail!("dry-run 不应调用发布器（内部错误）"),
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
    run_autopost_with(
        cfg,
        opts,
        emit,
        &mut |cfg, req, log, throttle| reseed::review_reseed(cfg, req, log, throttle),
        &mut |cfg, req, index, log, throttle| {
            reseed::run_reseed(cfg, &qb, req, index, log, throttle)
        },
    )
}

/// `run_autopost` 的编排主体（不含下载器连接与总闸）：调用方负责把 reviewer/runner
/// 接到真实 `review_reseed`/`run_reseed` 或测试假实现上。公开给测试与 GUI 复用。
#[doc(hidden)]
pub fn run_autopost_with(
    cfg: &Config,
    opts: &AutopostOptions,
    emit: &mut impl FnMut(AutopostEvent),
    reviewer: &mut ReviewRunner<'_>,
    runner: &mut ReseedRunner<'_>,
) -> Result<AutopostSummary> {
    let mut summary = AutopostSummary::default();
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let report = CoverageReport::build(&index, &cfg.sites, 20, now_unix(), "");
    if report.drafts.is_empty() {
        emit(AutopostEvent::Log(
            "没有转种草稿。先 `bsend run` 跑一轮，站里没有的段落定位到源种后会出草稿。".into(),
        ));
        return Ok(summary);
    }
    let allow: HashSet<String> = cfg.coverage.auto_reseed_sites.iter().cloned().collect();
    if allow.is_empty() {
        emit(AutopostEvent::Log(
            "未配置 assist.autoReseedSites（自动发种白名单）——本次只做门禁演练，不会发布任何东西。"
                .into(),
        ));
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
    for d in &report.drafts {
        // 长任务跨午夜时，退避/日限额要按当前时刻算，不能用本轮开跑时的时间
        let now = now_unix();
        if let Some(l) = opts.limit {
            if summary.posted >= l {
                break;
            }
        }
        let site = &d.target_site;
        if let Some(ss) = &opts.sites {
            if !ss.iter().any(|s| s.eq_ignore_ascii_case(site)) {
                continue;
            }
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
        ) {
            emit(AutopostEvent::Log(format!(
                "跳过 {}：上次拦截/失败还在退避期内（门禁 24h / 失败 6h）",
                d.name
            )));
            continue;
        }
        let rules = cfg
            .coverage
            .reseed_rules
            .get(site)
            .cloned()
            .unwrap_or_default();
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
        let input = gates::GateInput {
            rules: &rules,
            name: &rep.info.name,
            subtitle: &rep.info.subtitle,
            descr: &rep.info.descr,
            mediainfo: &rep.info.mediainfo,
            has_links: rep.info.has_media_links(),
            type_label: type_label.as_deref(),
            core_gaps: &core,
            audit_passed: rep.audit.as_ref().map(|a| a.passed),
        };
        let results = gates::evaluate(&input);
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
        match runner(
            cfg,
            &req,
            &mut index,
            &mut |l| {
                emit(AutopostEvent::Log(format!("   {l}")));
            },
            Some(&throttle),
        ) {
            Ok(reseed::ReseedOutcome::Duplicate(dup)) => {
                index.save()?;
                let result = reseed::ReseedOutcome::Duplicate(dup.clone()).audit_result();
                append_audit(
                    &audit_path,
                    serde_json::json!({"site": site, "key": d.content_key,
                        "result": result, "url": dup.url, "note": dup.note, "gates": gate_json}),
                )?;
                emit(AutopostEvent::Outcome {
                    key: d.content_key.clone(),
                    site: site.clone(),
                    result,
                    url: dup.url,
                    error: if result == "failed" {
                        Some(dup.note)
                    } else {
                        None
                    },
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
/// (内容, 站点) → (最近一次拦截/失败时间, 是否门禁拦截)，退避判定用
pub type LastBad = HashMap<(String, String), (i64, bool)>;

/// 读审计 JSONL：今日各站 posted 计数 + 每 (内容, 站点) 最近一次拦截/失败（退避用）。
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

/// 退避窗口：门禁拦截 24h、失败 6h 内不重试同草稿，避免连击白耗请求
pub fn in_backoff(last: Option<(i64, bool)>, now: i64) -> bool {
    match last {
        Some((at, is_block)) => now - at < if is_block { 86400 } else { 21600 },
        None => false,
    }
}

/// 审计追加（JSONL）。写失败必须中止本轮：posted 记账丢失会让日限额失守、退避失效。
/// `at` 一律取**写入时刻**：循环起点的时间只用于白名单/日限额/退避判定，
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
        assert!(!in_backoff(None, now));
        // 门禁拦截：23h 内退避，25h 后放行
        assert!(in_backoff(Some((now - 23 * 3600, true)), now));
        assert!(!in_backoff(Some((now - 25 * 3600, true)), now));
        // 失败：5h 内退避，7h 后放行
        assert!(in_backoff(Some((now - 5 * 3600, false)), now));
        assert!(!in_backoff(Some((now - 7 * 3600, false)), now));
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
    fn autopost_skips_sites_outside_whitelist() {
        let dir = temp_dir("whitelist");
        let mut cfg = cfg_with_drafts(&dir, 2, 1);
        cfg.coverage.auto_reseed_sites.clear();
        let mut called = 0;
        let mut runner = |_: &Config,
                          _: &reseed::ReseedRequest,
                          _: &mut ContentIndex,
                          _: &mut dyn FnMut(&str),
                          _: Option<&crate::throttle::SiteThrottle>| {
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
            },
            &mut collect_logs(&mut logs),
            &mut no_review,
            &mut runner,
        )
        .unwrap();
        assert_eq!(called, 0, "空白名单不能调用发布器");
        assert_eq!(summary.posted, 0);
        assert!(logs.iter().any(|l| l.contains("不在自动发种白名单")));
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
                          _: Option<&crate::throttle::SiteThrottle>| {
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
            },
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
        let cfg = cfg_with_drafts(&dir, 2, 1);
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
                          _: Option<&crate::throttle::SiteThrottle>| {
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
            },
            &mut collect_logs(&mut logs),
            &mut no_review,
            &mut runner,
        )
        .unwrap();
        assert_eq!(called, 0, "退避期内不能调用发布器");
        assert!(logs.iter().any(|l| l.contains("退避期")));
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
                          _: Option<&crate::throttle::SiteThrottle>| {
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
            },
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
            },
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
            "复核失败必须走失败语义（退避 6h），不能记成重复"
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
                form_html: "<html><body><form></form></body></html>".into(),
            },
            info,
            preflight: Default::default(),
            audit: None,
            audit_note: None,
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
        let mut no_runner =
            |_: &Config,
             _: &reseed::ReseedRequest,
             _: &mut ContentIndex,
             _: &mut dyn FnMut(&str),
             _: Option<&crate::throttle::SiteThrottle>| {
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
            },
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
        let mut no_runner =
            |_: &Config,
             _: &reseed::ReseedRequest,
             _: &mut ContentIndex,
             _: &mut dyn FnMut(&str),
             _: Option<&crate::throttle::SiteThrottle>| {
                Ok(reseed::ReseedOutcome::Posted("u".into()))
            };
        let mut logs = Vec::new();
        let _ = run_autopost_with(
            &cfg,
            &AutopostOptions {
                dry_run: true,
                limit: None,
                sites: None,
            },
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
        assert!(logs.iter().any(|l| l.contains("不在自动发种白名单")));
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
                          _: Option<&crate::throttle::SiteThrottle>| {
            posted_runner_called += 1;
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
            },
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
}
