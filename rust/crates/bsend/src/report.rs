//! 覆盖报告：内容索引 → 一份人看/程序读/给 GUI 吃的结构。
//!
//! 索引本身是「内容 → 各站格子」的账本；报告回答的是运维视角的问题：
//! 每个站覆盖了多少、还差多少、哪段内容缺得最多、有哪些草稿与待找源。
//! 与 Swift 版「辅种页」的信息结构（方案第十二节）对齐，也是 R3 daemon 的 `/api/report` 雏形。

use std::collections::BTreeMap;

use serde::Serialize;

use crate::config::SiteConf;
use crate::index::{ContentIndex, CoverageStatus};

/// 索引总量
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub contents: usize,
    pub member_torrents: usize,
    /// 下载器里已消失、归档倒计时中的内容
    pub gone_contents: usize,
    pub resolved_sources: usize,
    pub drafts: usize,
    pub wait_source: usize,
}

/// 单站覆盖统计
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteStats {
    pub site_id: String,
    pub name: String,
    /// 已有结论的格子数
    pub cells: usize,
    /// 已覆盖 = 已在做种 + 已辅种 + 已转种
    pub covered: usize,
    /// 还没判定过的内容数（内容总数 − 格子数）：首轮会很大，属正常
    pub pending: usize,
    /// covered / cells（0 格时记 0，不记 100%）
    pub covered_ratio: f64,
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
        }
        totals.drafts = drafts.len();
        totals.wait_source = wait_source.len();

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
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp, body.as_bytes())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// 「这个站已经铺到了」的状态集合（覆盖率、未覆盖清单都按同一口径）
pub fn is_covered(status: CoverageStatus) -> bool {
    matches!(
        status,
        CoverageStatus::Seeding | CoverageStatus::Assisted | CoverageStatus::Reseeded
    )
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
}
