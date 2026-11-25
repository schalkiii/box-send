//! 覆盖报告：内容索引 → 一份人看/程序读/给 GUI 吃的结构。
//!
//! 索引本身是「内容 → 各站格子」的账本；报告回答的是运维视角的问题：
//! 每个站覆盖了多少、还差多少、哪段内容缺得最多、有哪些草稿与待找源。
//! 与 Swift 版「辅种页」的信息结构（方案第十二节）对齐，也是 R3 daemon 的 `/api/report` 雏形。

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;

use crate::config::SiteConf;
use crate::index::{ContentIndex, CoverageStatus};
use crate::models::LocalTorrent;

/// 索引总量
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub contents: usize,
    pub member_torrents: usize,
    /// 下载器里已消失、归档倒计时中的内容
    pub gone_contents: usize,
    pub resolved_sources: usize,
    /// 草稿条数（按目标站展开：同一内容转多个站各算一条）
    pub drafts: usize,
    /// 待找源内容数（按内容名去重，与 `wait_source` 清单长度同口径）
    pub wait_source: usize,
}

/// 单站覆盖统计
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteStats {
    pub site_id: String,
    pub name: String,
    /// 已有结论的格子数：该站在账本里被判定过一次的内容数（未判定的内容不计入）
    pub cells: usize,
    /// 已覆盖 = 已在做种 + 已辅种 + 已转种
    pub covered: usize,
    /// 未判定内容数（全库内容总数 − 格子数）；未判定 ≠ 可转种，首轮会很大属正常
    pub pending: usize,
    /// 判定覆盖率 = covered / cells，只回答「已判过的格子里覆盖得怎样」；
    /// cells == 0 时记 0，不记 100%。**不能**拿它当站点铺满度：分母排除了未判定内容。
    pub covered_ratio: f64,
    /// 全库覆盖率 = covered / 全库内容总数（`totals.contents`），回答「站点在全库铺到多少」；
    /// contents == 0 时记 0.0，绝不记 100%，也不出 NaN。
    pub library_ratio: f64,
    /// qB 真值口径：qB 里有该站 tracker 的内容数。读取时即时派生，不需要站点搜索。
    pub qb_covered: usize,
    /// qB 真值口径的缺口 = 全库内容数 − qb_covered。
    pub qb_missing: usize,
    /// qB 真值覆盖率 = qb_covered / 全库内容数；contents == 0 时记 0.0。
    pub qb_ratio: f64,
    pub by_status: BTreeMap<CoverageStatus, usize>,
}

/// 未覆盖最多的一段内容
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UncoveredItem {
    pub key: String,
    pub name: String,
    pub total_size: i64,
    pub missing_sites: Vec<String>,
}

/// 转种草稿（等人工确认）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftRow {
    pub content_key: String,
    pub name: String,
    pub target_site: String,
    pub source_site: String,
    pub source_url: String,
}

/// 一份完整报告
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageReport {
    pub generated_at: i64,
    pub index_path: String,
    pub totals: Totals,
    pub sites: Vec<SiteStats>,
    /// 缺站最多的内容（`top` 条）
    pub uncovered: Vec<UncoveredItem>,
    pub drafts: Vec<DraftRow>,
    /// 待找源清单（源站也都没找到的内容名）
    pub wait_source: Vec<String>,
}

impl CoverageReport {
    /// 用下载器的实时种子列表重新派生草稿：只认「qB 里没有目标站 tracker」的缺口，
    /// 并且要求源种已定位、内容仍在下载器里、没带禁转标记。
    ///
    /// `allowed` 是自动发种白名单；`per_site_limit` 是每站本轮最多保留的草稿数
    /// （通常传当站今日剩余额度）。同一条内容不会重复出现；`Skipped` / `Rejected` /
    /// `SameHashSkip` / `Verifying` 这类终态或人工态不会被重新排队；
    /// 站点审核相关（`PendingOffers` / `PendingReview` / `ReviewRejected` / `GoneFromSite`）
    /// 同样不排：种子已经在站上，再发只会得到重复种，等回访收尾。
    /// 返回清点后的草稿数。
    pub fn derive_qb_gap_drafts(
        &mut self,
        index: &ContentIndex,
        sites: &[SiteConf],
        allowed: &HashSet<String>,
        torrents: &[LocalTorrent],
        per_site_limit: &HashMap<String, usize>,
    ) -> usize {
        let h2k = index.hash_to_key();
        let mut live_hosts: HashMap<String, Vec<String>> = HashMap::new();
        for t in torrents {
            let Some(key) = h2k.get(&t.hash) else {
                continue;
            };
            let hosts = live_hosts.entry(key.clone()).or_default();
            for h in &t.tracker_hosts {
                if !hosts.iter().any(|x| x.eq_ignore_ascii_case(h)) {
                    hosts.push(h.clone());
                }
            }
        }
        self.rebuild_qb_gap_drafts(index, sites, allowed, &live_hosts, per_site_limit)
    }

    /// 不连下载器时的同口径版本：用索引里最后一次扫描到的 tracker 并集。
    /// 只给 `--dry-run` 预览用；真实发布必须用上面的实时版本。
    pub fn derive_qb_gap_drafts_from_index(
        &mut self,
        index: &ContentIndex,
        sites: &[SiteConf],
        allowed: &HashSet<String>,
        per_site_limit: &HashMap<String, usize>,
    ) -> usize {
        let live_hosts: HashMap<String, Vec<String>> = index
            .contents
            .iter()
            .map(|(k, e)| (k.clone(), e.tracker_hosts.clone()))
            .collect();
        self.rebuild_qb_gap_drafts(index, sites, allowed, &live_hosts, per_site_limit)
    }

    fn rebuild_qb_gap_drafts(
        &mut self,
        index: &ContentIndex,
        sites: &[SiteConf],
        allowed: &HashSet<String>,
        live_hosts: &HashMap<String, Vec<String>>,
        per_site_limit: &HashMap<String, usize>,
    ) -> usize {
        let mut candidates: Vec<(&String, &crate::index::ContentEntry)> = index
            .contents
            .iter()
            .filter(|(_, e)| {
                !e.forbid_reseed
                    && !e.torrents.is_empty()
                    && e.source.as_ref().is_some_and(|s| s.is_resolved())
            })
            .collect();
        // 大体积优先，同体积按名字，保证每次运行顺序稳定
        candidates.sort_by(|a, b| {
            b.1.total_size
                .cmp(&a.1.total_size)
                .then_with(|| a.1.name.cmp(&b.1.name))
                .then_with(|| a.0.cmp(b.0))
        });

        let mut drafts = Vec::new();
        let mut per_site: HashMap<String, usize> = HashMap::new();
        for (key, entry) in candidates {
            let src = entry.source.as_ref().expect("上面已过滤");
            let src_site = src.site_id.as_str();
            let hosts = live_hosts.get(key).map(|v| v.as_slice()).unwrap_or(&[]);
            for site in sites {
                if !allowed.contains(&site.id) || site.id == src_site {
                    continue;
                }
                if hosts.iter().any(|h| site.matches_tracker(h)) {
                    continue;
                }
                if let Some(cell) = entry.coverage.get(&site.id) {
                    if matches!(
                        cell.status,
                        CoverageStatus::Skipped
                            | CoverageStatus::Rejected
                            | CoverageStatus::SameHashSkip
                            | CoverageStatus::Verifying
                            // 站点要人走候选入口：再排一次只会再被同一规则挡住
                            | CoverageStatus::PendingOffers
                            // 等站点审核 / 审核没过 / 站上已找不到：种子在站上（或曾经在），
                            // 再排一次就是重复上传；等回访收尾，由人决定要不要重发。
                            | CoverageStatus::PendingReview
                            | CoverageStatus::ReviewRejected
                            | CoverageStatus::GoneFromSite
                    ) {
                        continue;
                    }
                }
                let used = per_site.entry(site.id.clone()).or_insert(0);
                let limit = per_site_limit.get(&site.id).copied().unwrap_or(usize::MAX);
                if *used >= limit {
                    continue;
                }
                *used += 1;
                drafts.push(DraftRow {
                    content_key: key.clone(),
                    name: entry.name.clone(),
                    target_site: site.id.clone(),
                    source_site: src_site.to_string(),
                    source_url: src.detail_url.clone(),
                });
            }
        }

        self.drafts = drafts;
        self.totals.drafts = self.drafts.len();
        self.totals.drafts
    }

    /// 生成报告。`sites` 是参与站（配置里的顺序），没格子的站也占一行（0 格可见）。
    pub fn build(
        index: &ContentIndex,
        sites: &[SiteConf],
        top: usize,
        now: i64,
        index_path: &str,
    ) -> Self {
        let mut totals = Totals {
            contents: index.contents.len(),
            ..Default::default()
        };

        // 站点 → 统计（先建空行，保证「一格都没有」的站也在表里）
        let mut stats: Vec<SiteStats> = sites
            .iter()
            .map(|s| SiteStats {
                site_id: s.id.clone(),
                name: s.name.clone(),
                cells: 0,
                covered: 0,
                pending: 0,
                covered_ratio: 0.0,
                library_ratio: 0.0,
                qb_covered: 0,
                qb_missing: 0,
                qb_ratio: 0.0,
                by_status: BTreeMap::new(),
            })
            .collect();
        // 站点 id → 行号（键用 owned String：借用 stats 里的字段会挡住后面的可变访问）
        let mut index_of: BTreeMap<String, usize> = BTreeMap::new();
        for (i, s) in stats.iter().enumerate() {
            index_of.insert(s.site_id.clone(), i);
        }

        let mut uncovered: Vec<UncoveredItem> = Vec::new();
        let mut drafts: Vec<DraftRow> = Vec::new();
        let mut wait_source: Vec<String> = Vec::new();

        for (key, entry) in &index.contents {
            totals.member_torrents += entry.torrents.len();
            if entry.torrents.is_empty() {
                totals.gone_contents += 1;
            }
            if entry.source.as_ref().is_some_and(|s| s.is_resolved()) {
                totals.resolved_sources += 1;
            }
            let mut missing: Vec<String> = Vec::new();
            for site in sites {
                // qB 真值：只要这段内容在 qB 里有该站 tracker，就算站上已有，
                // 不看账本格子，也不发站点搜索。
                if entry.tracker_hosts.iter().any(|h| site.matches_tracker(h)) {
                    if let Some(i) = index_of.get(site.id.as_str()) {
                        stats[*i].qb_covered += 1;
                    }
                }
                let Some(cell) = entry.coverage.get(&site.id) else {
                    missing.push(site.id.clone());
                    continue;
                };
                if let Some(i) = index_of.get(site.id.as_str()) {
                    let st = &mut stats[*i];
                    st.cells += 1;
                    *st.by_status.entry(cell.status).or_insert(0) += 1;
                    if is_covered(cell.status) {
                        st.covered += 1;
                    }
                }
                if !is_covered(cell.status) {
                    missing.push(site.id.clone());
                }
                if cell.status == CoverageStatus::ReseedDraft {
                    let src = entry.source.clone();
                    drafts.push(DraftRow {
                        content_key: key.clone(),
                        name: entry.name.clone(),
                        target_site: site.id.clone(),
                        source_site: src.as_ref().map(|s| s.site_id.clone()).unwrap_or_default(),
                        source_url: src.map(|s| s.detail_url).unwrap_or_default(),
                    });
                }
                if cell.status == CoverageStatus::WaitSource {
                    wait_source.push(entry.name.clone());
                }
            }
            if !missing.is_empty() {
                uncovered.push(UncoveredItem {
                    key: key.clone(),
                    name: entry.name.clone(),
                    total_size: entry.total_size,
                    missing_sites: missing,
                });
            }
        }

        for st in &mut stats {
            st.pending = totals.contents.saturating_sub(st.cells);
            st.covered_ratio = if st.cells == 0 {
                0.0
            } else {
                st.covered as f64 / st.cells as f64
            };
            st.library_ratio = if totals.contents == 0 {
                0.0
            } else {
                st.covered as f64 / totals.contents as f64
            };
            st.qb_missing = totals.contents.saturating_sub(st.qb_covered);
            st.qb_ratio = if totals.contents == 0 {
                0.0
            } else {
                st.qb_covered as f64 / totals.contents as f64
            };
        }
        totals.drafts = drafts.len();

        // 缺站多的优先，同缺站数按体积倒序
        uncovered.sort_by(|a, b| {
            b.missing_sites
                .len()
                .cmp(&a.missing_sites.len())
                .then(b.total_size.cmp(&a.total_size))
        });
        uncovered.truncate(top);
        drafts.sort_by(|a, b| a.name.cmp(&b.name));
        wait_source.sort();
        wait_source.dedup();
        // Count after dedup so this total matches the wait_source list length.
        totals.wait_source = wait_source.len();

        Self {
            generated_at: now,
            index_path: index_path.to_string(),
            totals,
            sites: stats,
            uncovered,
            drafts,
            wait_source,
        }
    }

    /// JSON 导出（camelCase，原子写：先 tmp 再 rename）
    pub fn write_json(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let body = serde_json::to_string_pretty(self)?;
        crate::atomic_write(path, body.as_bytes())
    }
}

/// 「这个站已经铺到了」的状态集合（覆盖率、未覆盖清单都按同一口径）
pub fn is_covered(status: CoverageStatus) -> bool {
    matches!(
        status,
        CoverageStatus::Seeding | CoverageStatus::Assisted | CoverageStatus::Reseeded
    )
}

/// Number of drafts targeting the given site id.
pub fn draft_count_for_site(report: &CoverageReport, site_id: &str) -> usize {
    report
        .drafts
        .iter()
        .filter(|d| d.target_site == site_id)
        .count()
}

/// 一条对账命中：草稿对应的内容已在下载器里挂着目标站的 100% 种子，格子该从
/// `ReseedDraft` 改写为 `Seeding`。只承载判定结果，写格子与 save 在调用方。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileMark {
    pub key: String,
    pub site: String,
    /// 落到格子 note 的说明文案
    pub note: String,
    /// 命中目标站的 tracker host
    pub host: String,
}

/// 对账纯函数：哪些草稿对应的内容已经在下载器里挂着目标站的完成种子。
///
/// 命中条件（三条同时满足）：
/// 1. 身份匹配：先按成员 v1/v2 hash 经 `hash_to_key` 映射回该草稿的 content key；
///    目标站那份种子是另一个 hash、账本尚未重扫时，退而按 **name 完全相等**兜底，
///    且要求该 name 在索引里只对应一个内容（重名放弃兜底）、大小一致（索引侧
///    `total_size` 已知时），防同名不同压制；
/// 2. 该种子 `progress >= 1.0`（文件已下完）；
/// 3. 该种子的某个 tracker host 命中目标站 `SiteConf::matches_tracker`
///    （走内置别名与子域匹配，禁止拿 site.url 的 host 裸比）。
///    且当前 (key, site) 格子仍是 `ReseedDraft`（已是 Seeding/Failed 等不动）。
///
/// 真值口径（设计约束，不是残缺视图）：一个种子只允许一个站点 tracker，禁止
/// `addTrackers`。上传新种时在 `info.source` 写入目标站专属标记，让新种子的
/// info hash 与源种不同；发布后从目标站下载 `.torrent` 独立挂回 qB。因此一个种子
/// 只挂一个站点 tracker，qB `torrents/info` 里的 tracker 信息就是完整真值，
/// 对账据此判定，精度是准的。
///
/// 已知限制仅两条：
/// 1. 站点 tracker 换域名时需同步 `extra_announce_hosts`，否则该站会整体算成缺口；
/// 2. 未启用的目标站（如 agsvpt、qilin）通常 qB 里也没有，会正常计入缺口。
///
/// 既然一个种子只挂一个 tracker，「同一条种子同时命中两个站」在 qB 侧不会再出现；
/// 跨站交叉只能通过多份独立种子体现，所以按内容×站点统计必须看全部成员种子的并集。
///
/// 纯判定：不碰下载器、不写账本；改写格子与 `index.save()` 由 `run_autopost` 侧完成。
pub fn reconcile_marks(
    drafts: &[DraftRow],
    torrents: &[LocalTorrent],
    sites: &[SiteConf],
    hash_to_key: &HashMap<String, String>,
    index: &ContentIndex,
) -> Vec<ReconcileMark> {
    // name → key 唯一表：重名内容兜底时直接放弃，避免把另一段内容误认成草稿
    let mut name_to_keys: HashMap<&str, Vec<&String>> = HashMap::new();
    for (key, entry) in &index.contents {
        name_to_keys
            .entry(entry.name.as_str())
            .or_default()
            .push(key);
    }

    let mut marks: Vec<ReconcileMark> = Vec::new();
    for d in drafts {
        let Some(site_conf) = sites.iter().find(|s| s.id == d.target_site) else {
            continue;
        };
        let Some(entry) = index.contents.get(&d.content_key) else {
            continue;
        };
        let Some(cell) = entry.coverage.get(&d.target_site) else {
            continue;
        };
        // 只有仍在草稿态的格子需要对账改写；已是 Seeding/Failed 等终态不动
        if cell.status != CoverageStatus::ReseedDraft {
            continue;
        }
        let hit = torrents.iter().find_map(|t| {
            if t.progress < 1.0 {
                return None;
            }
            // 身份匹配：① hash 命中；② name 唯一兜底（大小一致）
            let by_hash = t
                .all_hashes()
                .iter()
                .find_map(|h| hash_to_key.get(h))
                .map(|k| k == &d.content_key)
                .unwrap_or(false);
            let by_name = if !by_hash {
                name_to_keys
                    .get(t.name.as_str())
                    .map(|ks| ks.len() == 1 && ks[0] == &d.content_key)
                    .unwrap_or(false)
                    && (entry.total_size == 0 || t.total_size == entry.total_size)
            } else {
                false
            };
            if !by_hash && !by_name {
                return None;
            }
            t.tracker_hosts
                .iter()
                .find(|h| site_conf.matches_tracker(h))
                .cloned()
        });
        if let Some(host) = hit {
            marks.push(ReconcileMark {
                key: d.content_key.clone(),
                site: d.target_site.clone(),
                note: "下载器已挂该站 tracker".to_string(),
                host,
            });
        }
    }
    marks
}

/// 把报告草稿按自动发种白名单拆成（已启用，未启用）两组。
/// 未启用组只计数、不改格子：以后加白名单这些草稿还能用，不算待发。
pub fn split_drafts_by_allow<'a>(
    report: &'a CoverageReport,
    allow: &HashSet<String>,
) -> (Vec<&'a DraftRow>, Vec<&'a DraftRow>) {
    let mut enabled: Vec<&'a DraftRow> = Vec::new();
    let mut disabled: Vec<&'a DraftRow> = Vec::new();
    for d in &report.drafts {
        if allow.contains(&d.target_site) {
            enabled.push(d);
        } else {
            disabled.push(d);
        }
    }
    (enabled, disabled)
}

/// Whitelist site ids that currently have zero draft in this report.
///
/// Drafts only come from `reseed_draft` cells produced by a `bsend run` round;
/// listing a site in the whitelist by itself never generates one. A whitelisted
/// site absent from the report altogether (no judged cell yet) is also returned.
/// Output keeps the whitelist order and removes duplicates.
pub fn whitelist_without_drafts(report: &CoverageReport, whitelist: &[String]) -> Vec<String> {
    let draft_sites: HashSet<&str> = report
        .drafts
        .iter()
        .map(|d| d.target_site.as_str())
        .collect();
    let mut out: Vec<String> = Vec::new();
    for id in whitelist {
        if !draft_sites.contains(id.as_str()) && !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Audit report: read-only daily × site aggregation over auto-audit.jsonl.
// Pure functions here do no IO; the CLI reads the file and renders the result.
// ---------------------------------------------------------------------------

const AUDIT_UTC8: i64 = 8 * 3600;

/// Per-site counts for one Beijing day.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteAuditCounts {
    pub posted: usize,
    pub blocked: usize,
    pub failed: usize,
    pub duplicate: usize,
    pub other_version: usize,
    /// Unrecognized `result` value, kept so nothing is silently dropped.
    pub other: usize,
    /// 站点要求走候选入口（`pending_offers`）：等人工提交候选，不是失败
    pub pending_offers: usize,
    /// 站上已发布但要先过站点审核（`pending_review`）：等回访，不是失败
    pub pending_review: usize,
    /// 站点明写审核不通过（`review_rejected`）：要人改种子/简介，不是失败
    pub review_rejected: usize,
    /// Latest record timestamp (Unix sec) for this site/day; 0 when empty.
    pub last_at: i64,
}

/// One Beijing-day slice of the audit log, grouped by target site.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditDay {
    /// YYYY-MM-DD, Beijing day boundary (UTC+8)
    pub date: String,
    pub by_site: BTreeMap<String, SiteAuditCounts>,
}

/// Totals across the filtered window.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditTotals {
    pub posted: usize,
    pub blocked: usize,
    pub failed: usize,
    pub duplicate: usize,
    pub other_version: usize,
    pub other: usize,
    /// 站点要求走候选入口（`pending_offers`）
    pub pending_offers: usize,
    /// 等站点审核（`pending_review`）
    pub pending_review: usize,
    /// 站点审核不通过（`review_rejected`）
    pub review_rejected: usize,
    /// Records parsed and counted into the buckets above.
    pub records: usize,
    /// Lines that failed to parse or missed required fields.
    pub malformed: usize,
}

/// Read-only audit report (day × site) over the auto-audit JSONL text.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditReport {
    pub generated_at: i64,
    pub days: Vec<AuditDay>,
    pub totals: AuditTotals,
}

/// Map an audit `result` string onto a report bucket.
///
/// - `posted` → 发布
/// - `blocked` → 拦截（门禁拦截，转人工）
/// - `failed`, `review_failed` → 失败（审阅/发布阶段失败；失败冷却口径与 failed 相同）
/// - `skipped_duplicate`, `existed_pending`, `same_hash_skipped` → 重复（站内已有条目 / 同 hash）
/// - `other_version` → 其他版本（单列，不计入重复）
/// - `pending_offers` → 待走候选入口（等人工提交候选）
/// - `pending_review` / `review_rejected` → 等站点审核 / 审核不通过（等回访或人工处理）
/// - anything else → `other`
fn audit_bucket(result: &str) -> &'static str {
    match result {
        "posted" => "posted",
        "blocked" => "blocked",
        "failed" | "review_failed" => "failed",
        "skipped_duplicate" | "existed_pending" | "same_hash_skipped" => "duplicate",
        "other_version" => "other_version",
        "pending_offers" => "pending_offers",
        "pending_review" => "pending_review",
        "review_rejected" => "review_rejected",
        _ => "other",
    }
}

/// Beijing day index (days since epoch, UTC+8), same boundary as `parse_audit`.
fn beijing_day_index(at: i64) -> i64 {
    (at + AUDIT_UTC8).div_euclid(86400)
}

/// Convert a day index to `YYYY-MM-DD` (proleptic Gregorian, Hinnant's algorithm).
fn day_index_to_date(day: i64) -> String {
    let z = day + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Aggregate audit JSONL text into a day × site report. Pure: no IO.
///
/// `now` is the reference Unix sec (for the "last N days" window, Beijing
/// boundary); `days` keeps the most recent N days including today; `site`
/// filters by target site id (exact match).
pub fn audit_daily(text: &str, now: i64, days: usize, site: Option<&str>) -> AuditReport {
    let days = days.max(1);
    let today_idx = beijing_day_index(now);
    let min_idx = today_idx - (days as i64 - 1);

    let mut by_day: BTreeMap<i64, AuditDay> = BTreeMap::new();
    let mut totals = AuditTotals::default();

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                totals.malformed += 1;
                continue;
            }
        };
        let Some(result) = v.get("result").and_then(|r| r.as_str()) else {
            totals.malformed += 1;
            continue;
        };
        let Some(at) = v.get("at").and_then(|a| a.as_i64()) else {
            totals.malformed += 1;
            continue;
        };
        let Some(site_id) = v.get("site").and_then(|s| s.as_str()) else {
            totals.malformed += 1;
            continue;
        };
        if let Some(filter) = site {
            if site_id != filter {
                continue;
            }
        }
        let day_idx = beijing_day_index(at);
        if day_idx < min_idx {
            continue;
        }
        let bucket = audit_bucket(result);
        let day = by_day.entry(day_idx).or_insert_with(|| AuditDay {
            date: day_index_to_date(day_idx),
            by_site: BTreeMap::new(),
        });
        let counts = day.by_site.entry(site_id.to_string()).or_default();
        match bucket {
            "posted" => counts.posted += 1,
            "blocked" => counts.blocked += 1,
            "failed" => counts.failed += 1,
            "duplicate" => counts.duplicate += 1,
            "other_version" => counts.other_version += 1,
            "pending_offers" => counts.pending_offers += 1,
            "pending_review" => counts.pending_review += 1,
            "review_rejected" => counts.review_rejected += 1,
            _ => counts.other += 1,
        }
        if at > counts.last_at {
            counts.last_at = at;
        }
        match bucket {
            "posted" => totals.posted += 1,
            "blocked" => totals.blocked += 1,
            "failed" => totals.failed += 1,
            "duplicate" => totals.duplicate += 1,
            "other_version" => totals.other_version += 1,
            "pending_offers" => totals.pending_offers += 1,
            "pending_review" => totals.pending_review += 1,
            "review_rejected" => totals.review_rejected += 1,
            _ => totals.other += 1,
        }
        totals.records += 1;
    }

    AuditReport {
        generated_at: now,
        days: by_day.into_values().collect(),
        totals,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::SiteCoverage;
    use crate::sites::Framework;

    fn site(id: &str, name: &str) -> SiteConf {
        SiteConf {
            id: id.to_string(),
            name: name.to_string(),
            url: format!("https://{id}.example/"),
            framework: Framework::NexusPHP,
            search_url: None,
            api_key: None,
            announce_hosts: vec![format!("{id}.example")],
            announce_url: None,
            poster_field: None,
            screenshot_field: None,
        }
    }

    fn index_with(cells: &[(&str, CoverageStatus)]) -> ContentIndex {
        let mut idx = ContentIndex::in_memory();
        idx.contents.insert(
            "m:abc".into(),
            crate::index::ContentEntry {
                name: "Show.S01.1080p".into(),
                total_size: 100,
                file_count: 2,
                manifest_digest: "abc".into(),
                torrents: BTreeMap::new(),
                tracker_hosts: vec![],
                first_seen: 0,
                gone_since: None,
                coverage: cells
                    .iter()
                    .map(|(s, st)| {
                        (
                            s.to_string(),
                            SiteCoverage {
                                status: *st,
                                at: 0,
                                url: None,
                                note: None,
                                verify_hash: None,
                            },
                        )
                    })
                    .collect(),
                source: None,
                forbid_reseed: false,
            },
        );
        idx
    }

    #[test]
    fn report_counts_covered_and_missing() {
        let idx = index_with(&[
            ("a", CoverageStatus::Seeding),
            ("b", CoverageStatus::Assisted),
            ("c", CoverageStatus::Miss),
        ]);
        let sites = vec![
            site("a", "甲"),
            site("b", "乙"),
            site("c", "丙"),
            site("d", "丁"),
        ];
        let r = CoverageReport::build(&idx, &sites, 10, 1000, "coverage.json");
        assert_eq!(r.totals.contents, 1);
        assert_eq!(r.sites.len(), 4, "没格子的站也占一行");
        assert_eq!(r.sites[0].covered, 1);
        assert_eq!(r.sites[0].covered_ratio, 1.0);
        assert_eq!(r.sites[2].covered, 0);
        assert_eq!(r.sites[3].cells, 0);
        assert_eq!(r.sites[3].pending, 1, "从没判过的站：待判定 = 内容总数");
        // 缺站：c（站内没有）+ d（没格子）
        assert_eq!(r.uncovered.len(), 1);
        assert_eq!(r.uncovered[0].missing_sites, vec!["c", "d"]);
    }

    #[test]
    fn zero_cell_site_has_zero_ratios_not_full() {
        let idx = index_with(&[("a", CoverageStatus::Seeding)]);
        let sites = vec![site("a", "甲"), site("d", "丁")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let d = &r.sites[1];
        assert_eq!(d.cells, 0);
        assert_eq!(d.covered_ratio, 0.0);
        assert_eq!(d.library_ratio, 0.0);
        assert!(!d.library_ratio.is_nan());
    }

    #[test]
    fn qb_tracker_truth_counts_without_coverage_cells() {
        let mut idx = ContentIndex::in_memory();
        let hosts = [
            vec!["a.example".to_string()],
            vec!["sub.a.example".to_string()],
            vec!["other.example".to_string()],
        ];
        for (i, h) in hosts.into_iter().enumerate() {
            idx.contents.insert(
                format!("m:{i}"),
                crate::index::ContentEntry {
                    name: format!("Show.{i}"),
                    total_size: 100,
                    file_count: 1,
                    manifest_digest: format!("d{i}"),
                    torrents: BTreeMap::new(),
                    tracker_hosts: h,
                    first_seen: 0,
                    gone_since: None,
                    coverage: BTreeMap::new(),
                    source: None,
                    forbid_reseed: false,
                },
            );
        }
        let sites = vec![site("a", "甲")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let a = &r.sites[0];
        assert_eq!(a.qb_covered, 2, "子域 tracker 也算该站");
        assert_eq!(a.qb_missing, 1);
        assert!((a.qb_ratio - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(a.cells, 0, "qB 真值不依赖账本格子");
    }

    #[test]
    fn library_ratio_divides_by_total_contents_not_cells() {
        let mut idx = ContentIndex::in_memory();
        for i in 0..4 {
            let cells: BTreeMap<String, SiteCoverage> = if i < 2 {
                [(
                    "a".to_string(),
                    SiteCoverage {
                        status: CoverageStatus::Seeding,
                        at: 0,
                        url: None,
                        note: None,
                        verify_hash: None,
                    },
                )]
                .into_iter()
                .collect()
            } else {
                BTreeMap::new()
            };
            idx.contents.insert(
                format!("m:{i}"),
                crate::index::ContentEntry {
                    name: format!("Show.{i}"),
                    total_size: 100,
                    file_count: 2,
                    manifest_digest: format!("{i}"),
                    torrents: BTreeMap::new(),
                    tracker_hosts: vec![],
                    first_seen: 0,
                    gone_since: None,
                    coverage: cells,
                    source: None,
                    forbid_reseed: false,
                },
            );
        }
        let sites = vec![site("a", "甲")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let a = &r.sites[0];
        assert_eq!(a.cells, 2);
        assert_eq!(a.covered, 2);
        assert_eq!(a.covered_ratio, 1.0, "判定覆盖：2/2 格子都覆盖");
        assert_eq!(a.library_ratio, 0.5, "全库覆盖：2/4 段");
    }

    #[test]
    fn empty_index_reports_zero_library_ratio_without_nan() {
        let idx = ContentIndex::in_memory();
        let sites = vec![site("a", "甲")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let a = &r.sites[0];
        assert_eq!(r.totals.contents, 0);
        assert_eq!(a.covered_ratio, 0.0);
        assert_eq!(a.library_ratio, 0.0);
        assert!(!a.library_ratio.is_nan());
    }

    #[test]
    fn drafts_and_wait_source_are_listed() {
        let mut idx = index_with(&[
            ("a", CoverageStatus::ReseedDraft),
            ("b", CoverageStatus::WaitSource),
            ("c", CoverageStatus::Failed),
        ]);
        let e = idx.contents.get_mut("m:abc").unwrap();
        e.source = Some(crate::index::SourceRecord {
            site_id: "src".into(),
            detail_url: "https://src.example/details.php?id=1".into(),
            resolved_at: 10,
            miss_at: None,
        });
        let r = CoverageReport::build(&idx, &[site("a", "甲"), site("b", "乙")], 10, 1, "x");
        assert_eq!(r.totals.drafts, 1);
        assert_eq!(r.drafts[0].source_site, "src");
        assert_eq!(r.totals.wait_source, 1);
        assert_eq!(r.wait_source, vec!["Show.S01.1080p".to_string()]);
        // JSON 里状态是 snake_case，字段是 camelCase（GUI/daemon 直接吃）
        let js = serde_json::to_string(&r).unwrap();
        assert!(js.contains("\"coveredRatio\""), "{js}");
        assert!(js.contains("\"reseed_draft\""), "{js}");
    }

    #[test]
    fn qb_gap_drafts_skip_sites_already_in_qb() {
        let mut idx = ContentIndex::in_memory();
        let mut members = BTreeMap::new();
        members.insert(
            "h1".to_string(),
            crate::index::TorrentMeta {
                save_path: "/data".into(),
                category: None,
                last_seen: 1,
                tracker_hosts: vec!["src.example".into()],
                hash_v2: None,
                added_on: 0,
            },
        );
        idx.contents.insert(
            "m:1".into(),
            crate::index::ContentEntry {
                name: "Show.S01.1080p".into(),
                total_size: 100,
                file_count: 1,
                manifest_digest: "d".into(),
                torrents: members,
                tracker_hosts: vec!["src.example".into()],
                first_seen: 0,
                gone_since: None,
                coverage: BTreeMap::new(),
                source: Some(crate::index::SourceRecord {
                    site_id: "src".into(),
                    detail_url: "https://src.example/details.php?id=1".into(),
                    resolved_at: 1,
                    miss_at: None,
                }),
                forbid_reseed: false,
            },
        );
        let sites = vec![site("a", "甲"), site("b", "乙"), site("src", "源")];
        let allowed: HashSet<String> = ["a", "b", "src"].iter().map(|s| s.to_string()).collect();
        let live = vec![LocalTorrent {
            hash: "h1".into(),
            hash_v2: None,
            name: "Show.S01.1080p".into(),
            save_path: "/data".into(),
            category: None,
            tags: vec![],
            total_size: 100,
            progress: 1.0,
            tracker_hosts: vec!["src.example".into()],
            added_on: 0,
        }];
        let mut r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let limits = HashMap::new();
        assert_eq!(
            r.derive_qb_gap_drafts(&idx, &sites, &allowed, &live, &limits),
            2,
            "源站不转自己；a/b 两个缺口各一条"
        );
        let mut live_b = live.clone();
        live_b[0].tracker_hosts.push("b.example".into());
        assert_eq!(
            r.derive_qb_gap_drafts(&idx, &sites, &allowed, &live_b, &limits),
            1,
            "qB 已有 b 的 tracker，只剩 a"
        );
        assert_eq!(r.drafts[0].target_site, "a");
        assert_eq!(
            r.derive_qb_gap_drafts_from_index(&idx, &sites, &allowed, &limits),
            2
        );
    }

    #[test]
    fn wait_source_dedupes_same_content_across_sites() {
        // One content in WaitSource on two sites: the report lists it once,
        // not once per site cell.
        let idx = index_with(&[
            ("a", CoverageStatus::WaitSource),
            ("b", CoverageStatus::WaitSource),
        ]);
        let sites = vec![site("a", "甲"), site("b", "乙")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        assert_eq!(r.wait_source, vec!["Show.S01.1080p".to_string()]);
        assert_eq!(r.totals.wait_source, 1);
    }

    #[test]
    fn draft_count_counts_only_the_target_site() {
        let idx = index_with(&[
            ("a", CoverageStatus::ReseedDraft),
            ("b", CoverageStatus::ReseedDraft),
        ]);
        let sites = vec![site("a", "甲"), site("b", "乙"), site("c", "丙")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        assert_eq!(draft_count_for_site(&r, "a"), 1);
        assert_eq!(draft_count_for_site(&r, "b"), 1);
        assert_eq!(draft_count_for_site(&r, "c"), 0);
    }

    #[test]
    fn whitelist_without_drafts_excludes_site_that_has_a_draft() {
        // a has a draft cell; b has a judged cell but no draft; c has no cell at all.
        let idx = index_with(&[
            ("a", CoverageStatus::ReseedDraft),
            ("b", CoverageStatus::WaitSource),
        ]);
        let sites = vec![site("a", "甲"), site("b", "乙"), site("c", "丙")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let missing =
            whitelist_without_drafts(&r, &["a".into(), "b".into(), "c".into(), "a".into()]);
        assert_eq!(missing, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn whitelist_without_drafts_empty_when_all_have_drafts_or_list_is_empty() {
        let idx = index_with(&[("a", CoverageStatus::ReseedDraft)]);
        let sites = vec![site("a", "甲")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        assert!(whitelist_without_drafts(&r, &["a".into()]).is_empty());
        assert!(whitelist_without_drafts(&r, &[]).is_empty());
    }

    fn audit_line(at: i64, s: &str, result: &str) -> String {
        format!(r#"{{"at":{at},"site":"{s}","key":"m:x","result":"{result}"}}"#)
    }

    #[test]
    fn audit_day_boundary_uses_beijing_midnight() {
        // `now` is some instant; today_start mirrors parse_audit's Beijing boundary.
        let now = 1_763_890_000;
        let today_start = now - (now + AUDIT_UTC8).rem_euclid(86400);
        let yesterday_last = today_start - 1;
        let today_idx = beijing_day_index(now);
        let text = [
            audit_line(yesterday_last, "sbpt", "posted"),
            audit_line(today_start, "sbpt", "posted"),
        ]
        .join("\n");
        let r = audit_daily(&text, now, 7, None);
        assert_eq!(r.days.len(), 2);
        assert_eq!(r.days[0].date, day_index_to_date(today_idx - 1));
        assert_eq!(r.days[1].date, day_index_to_date(today_idx));
        assert_eq!(r.days[0].by_site["sbpt"].posted, 1);
        assert_eq!(r.days[1].by_site["sbpt"].posted, 1);
    }

    #[test]
    fn audit_site_filter_keeps_only_target_site() {
        let now = 1_763_890_000;
        let text = [
            audit_line(now, "sbpt", "posted"),
            audit_line(now, "hdsky", "blocked"),
            audit_line(now, "sbpt", "failed"),
        ]
        .join("\n");
        let r = audit_daily(&text, now, 7, Some("sbpt"));
        assert_eq!(r.totals.posted, 1);
        assert_eq!(r.totals.failed, 1);
        assert_eq!(r.totals.blocked, 0);
        assert_eq!(r.days[0].by_site.len(), 1);
    }

    #[test]
    fn audit_unknown_result_goes_to_other() {
        let now = 1_763_890_000;
        let text = audit_line(now, "sbpt", "mysterious_new_result");
        let r = audit_daily(&text, now, 7, None);
        assert_eq!(r.totals.other, 1);
        assert_eq!(r.totals.records, 1);
        assert_eq!(r.days[0].by_site["sbpt"].other, 1);
    }

    #[test]
    fn audit_malformed_lines_are_counted_and_ignore_blank_lines() {
        let now = 1_763_890_000;
        let text = [
            audit_line(now, "sbpt", "posted"),
            "{not json".to_string(),
            r#"{"at":1700000000,"site":"sbpt"}"#.to_string(), // missing result
            String::new(),
            "   ".to_string(),
        ]
        .join("\n");
        let r = audit_daily(&text, now, 7, None);
        assert_eq!(r.totals.malformed, 2);
        assert_eq!(r.totals.posted, 1);
        assert_eq!(r.totals.records, 1);
    }

    #[test]
    fn audit_buckets_map_known_results() {
        let now = 1_763_890_000;
        let text = [
            audit_line(now, "a", "review_failed"),
            audit_line(now, "a", "failed"),
            audit_line(now, "a", "skipped_duplicate"),
            audit_line(now, "a", "existed_pending"),
            audit_line(now, "a", "same_hash_skipped"),
            audit_line(now, "a", "other_version"),
            audit_line(now, "a", "blocked"),
            audit_line(now, "a", "posted"),
        ]
        .join("\n");
        let r = audit_daily(&text, now, 7, None);
        let c = &r.days[0].by_site["a"];
        assert_eq!(c.failed, 2);
        assert_eq!(c.duplicate, 3);
        assert_eq!(c.other_version, 1);
        assert_eq!(c.blocked, 1);
        assert_eq!(c.posted, 1);
        assert_eq!(r.totals.records, 8);
    }

    #[test]
    fn audit_daily_drops_records_outside_window() {
        let now = 1_763_890_000;
        let old = now - 30 * 86400;
        let text = [
            audit_line(old, "a", "posted"),
            audit_line(now, "a", "posted"),
        ]
        .join("\n");
        let r = audit_daily(&text, now, 7, None);
        assert_eq!(r.totals.posted, 1);
        assert_eq!(r.days.len(), 1);
    }

    // ---- 对账（reconcile_marks）----

    fn sample_draft(target: &str) -> DraftRow {
        DraftRow {
            content_key: "m:abc".into(),
            name: "Show.S01.1080p".into(),
            target_site: target.into(),
            source_site: String::new(),
            source_url: String::new(),
        }
    }

    fn sample_torrent(
        hash: &str,
        name: &str,
        size: i64,
        progress: f64,
        hosts: &[&str],
    ) -> LocalTorrent {
        LocalTorrent::new(
            hash,
            None,
            name,
            "",
            None,
            vec![],
            size,
            progress,
            hosts.iter().map(|s| s.to_string()).collect(),
            0,
        )
    }

    #[test]
    fn reconcile_marks_matches_finished_torrent_on_target_tracker() {
        let idx = index_with(&[("a", CoverageStatus::ReseedDraft)]);
        let sites = vec![site("a", "甲")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let h = "ffffffffffffffffffffffffffffffffffffffff";
        let mut h2k = HashMap::new();
        h2k.insert(h.to_string(), "m:abc".to_string());
        let torrents = vec![sample_torrent(
            h,
            "Show.S01.1080p",
            100,
            1.0,
            &["a.example"],
        )];
        let marks = reconcile_marks(&r.drafts, &torrents, &sites, &h2k, &idx);
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].key, "m:abc");
        assert_eq!(marks[0].site, "a");
        assert_eq!(marks[0].host, "a.example");
    }

    #[test]
    fn reconcile_marks_skips_partial_progress() {
        let idx = index_with(&[("a", CoverageStatus::ReseedDraft)]);
        let sites = vec![site("a", "甲")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let h = "ffffffffffffffffffffffffffffffffffffffff";
        let mut h2k = HashMap::new();
        h2k.insert(h.to_string(), "m:abc".to_string());
        let torrents = vec![sample_torrent(
            h,
            "Show.S01.1080p",
            100,
            0.5,
            &["a.example"],
        )];
        assert!(reconcile_marks(&r.drafts, &torrents, &sites, &h2k, &idx).is_empty());
    }

    #[test]
    fn reconcile_marks_keeps_non_draft_cell_untouched() {
        // 格子已是 Seeding：即使下载器里有完成种子也不改写（幂等，第二遍不再动）
        let idx = index_with(&[("a", CoverageStatus::Seeding)]);
        let sites = vec![site("a", "甲")];
        let h = "ffffffffffffffffffffffffffffffffffffffff";
        let mut h2k = HashMap::new();
        h2k.insert(h.to_string(), "m:abc".to_string());
        let torrents = vec![sample_torrent(
            h,
            "Show.S01.1080p",
            100,
            1.0,
            &["a.example"],
        )];
        let drafts = vec![sample_draft("a")];
        assert!(reconcile_marks(&drafts, &torrents, &sites, &h2k, &idx).is_empty());
    }

    #[test]
    fn reconcile_marks_rejects_foreign_tracker_host() {
        let idx = index_with(&[("a", CoverageStatus::ReseedDraft)]);
        let sites = vec![site("a", "甲")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let h = "ffffffffffffffffffffffffffffffffffffffff";
        let mut h2k = HashMap::new();
        h2k.insert(h.to_string(), "m:abc".to_string());
        let torrents = vec![sample_torrent(
            h,
            "Show.S01.1080p",
            100,
            1.0,
            &["foreign.example"],
        )];
        assert!(reconcile_marks(&r.drafts, &torrents, &sites, &h2k, &idx).is_empty());
    }

    #[test]
    fn reconcile_marks_name_fallback_matches_when_hash_absent() {
        // 目标站那份种子是另一个 hash、账本没重扫：按 name 唯一 + 大小相等兜底
        let idx = index_with(&[("a", CoverageStatus::ReseedDraft)]);
        let sites = vec![site("a", "甲")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let torrents = vec![sample_torrent(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "Show.S01.1080p",
            100,
            1.0,
            &["a.example"],
        )];
        let h2k = HashMap::new();
        let marks = reconcile_marks(&r.drafts, &torrents, &sites, &h2k, &idx);
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].host, "a.example");
    }

    #[test]
    fn reconcile_marks_name_fallback_rejects_size_mismatch() {
        let idx = index_with(&[("a", CoverageStatus::ReseedDraft)]);
        let sites = vec![site("a", "甲")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let torrents = vec![sample_torrent(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "Show.S01.1080p",
            999,
            1.0,
            &["a.example"],
        )];
        let h2k = HashMap::new();
        assert!(reconcile_marks(&r.drafts, &torrents, &sites, &h2k, &idx).is_empty());
    }

    #[test]
    fn reconcile_marks_name_fallback_abandons_duplicate_name() {
        // 同名对应两段内容：兜底放弃，避免误标
        let mut idx = index_with(&[("a", CoverageStatus::ReseedDraft)]);
        idx.contents.insert(
            "m:dup".into(),
            crate::index::ContentEntry {
                name: "Show.S01.1080p".into(),
                total_size: 100,
                file_count: 2,
                manifest_digest: "dup".into(),
                torrents: BTreeMap::new(),
                tracker_hosts: vec![],
                first_seen: 0,
                gone_since: None,
                coverage: BTreeMap::new(),
                source: None,
                forbid_reseed: false,
            },
        );
        let sites = vec![site("a", "甲")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let torrents = vec![sample_torrent(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "Show.S01.1080p",
            100,
            1.0,
            &["a.example"],
        )];
        let h2k = HashMap::new();
        assert!(reconcile_marks(&r.drafts, &torrents, &sites, &h2k, &idx).is_empty());
    }

    // ---- 草稿按白名单拆分（split_drafts_by_allow）----

    #[test]
    fn split_drafts_empty_allow_lists_all_as_disabled() {
        let idx = index_with(&[("a", CoverageStatus::ReseedDraft)]);
        let sites = vec![site("a", "甲")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let allow: HashSet<String> = HashSet::new();
        let (enabled, disabled) = split_drafts_by_allow(&r, &allow);
        assert_eq!(enabled.len(), 0);
        assert_eq!(disabled.len(), 1);
    }

    #[test]
    fn split_drafts_partial_allow_groups_correctly() {
        let mut idx = index_with(&[("a", CoverageStatus::ReseedDraft)]);
        idx.contents.insert(
            "m:two".into(),
            crate::index::ContentEntry {
                name: "Show.Two".into(),
                total_size: 100,
                file_count: 2,
                manifest_digest: "two".into(),
                torrents: BTreeMap::new(),
                tracker_hosts: vec![],
                first_seen: 0,
                gone_since: None,
                coverage: [(
                    "b".to_string(),
                    SiteCoverage {
                        status: CoverageStatus::ReseedDraft,
                        at: 0,
                        url: None,
                        note: None,
                        verify_hash: None,
                    },
                )]
                .into_iter()
                .collect(),
                source: None,
                forbid_reseed: false,
            },
        );
        let sites = vec![site("a", "甲"), site("b", "乙")];
        let r = CoverageReport::build(&idx, &sites, 10, 1, "x");
        let allow: HashSet<String> = ["a".to_string()].into_iter().collect();
        let (enabled, disabled) = split_drafts_by_allow(&r, &allow);
        assert_eq!(enabled.len(), 1);
        assert_eq!(disabled.len(), 1);
        assert_eq!(enabled[0].target_site, "a");
        assert_eq!(disabled[0].target_site, "b");
    }
}
