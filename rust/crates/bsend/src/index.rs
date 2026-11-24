//! 内容索引：辅种去重的核心（方案第四节）。
//! 一段「内容」的身份不依赖任何站点：v1 hash、v2 hash、逐文件清单摘要三者任一相同
//! 即同一内容（并查集合并）。各站重新打包（改 source/private 标记）hash 不同、manifest
//! 全等 → 合并；同名不同压制 manifest 不同 → 天然分开。
//!
//! 独立 `coverage.json` 落盘（与 GUI 版 state.json 分开，不互相撑大）；种子从下载器
//! 消失后条目保留 `archiveDays` 再清，保住「已辅/已转」记录——同内容重新入下载器时
//! 不用从头铺。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::manifest::{comparable_key, manifest_digest};
use crate::models::{LocalTorrent, TorrentFingerprint};

/// 内容 × 站点 格子的结论（统一决策表）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    /// 已在做种（tracker hosts 含该站）
    Seeding,
    /// 已辅种成功
    Assisted,
    /// 已转种成功
    Reseeded,
    /// 上一轮推送还没等到校验结果
    Verifying,
    /// 站内检索命中但清单对不上（C 级），待人工确认
    ReviewPending,
    /// 目标站已有同片其他版本（pieces 不一致），程序化判定，不再人工确认
    OtherVersion,
    /// 目标种与下载器已有种子 info hash 相同：qB 不允许同 hash 两条，
    /// 按策略**不改动原种子**（不补挂 tracker、不打标签），跳过该站
    SameHashSkip,
    /// 人工否决（不是同一内容）
    Rejected,
    /// 站内没有（负缓存，missTTL 天内不再重搜）
    Miss,
    /// 站内没有且源站也没定位到源种，进待找源清单
    WaitSource,
    /// 推送/上传失败
    Failed,
    /// 转种臂生成的待发草稿（v1 默认：转种只出报告）
    ReseedDraft,
    /// 命中内容类型黑名单（教育/成人等）等按策略不再自动发的草稿：终态，第二轮起不再入队
    Skipped,
    /// 站点明确要求改走候选入口（NexusPHP 的 offers.php，例如判定种子超过体积上限）。
    /// 这不是“已跳过”，而是要人后续去候选页提交：覆盖记录留着、`status` 单列“待跟进”，
    /// 但程序不自动重试（重试只会再被同一规则挡住）。
    PendingOffers,
    /// 站上已经发布，但站点还没放行：详情页标着未审，或下载种子时回“尚未通过审核，请稍后再试”。
    /// 种子在站上，只是拿不到、挂不了 qB。不算失败（不进失败冷却）、不重发（重发只会得到重复种）；
    /// 也不做站内检索兜底（只会命中同名旧种）。等回访（`bsend reviewcheck` / 自动发种每轮开头）
    /// 看到审核标记消失，再把站点种子下回来挂进 qB。
    PendingReview,
    /// 站点明写审核不通过（附原因）。种子可能仍在 qB 做种；要人改简介/种子后在站上处理，
    /// 程序不重发、不自动重试。
    ReviewRejected,
    /// 回访时详情页报“没有该ID的种子”（或 HTTP 404）：站上已经没有这条。
    /// qB 里那条种子可能还挂着该站 tracker，但站上没有对应种子了；由人决定删不删、要不要重发。
    /// 这条结论比“qB 里有该站 tracker”更直接，扫描时的做种标记不覆盖它。
    GoneFromSite,
}

impl CoverageStatus {
    pub fn label(&self) -> &'static str {
        match self {
            CoverageStatus::Seeding => "已在做种",
            CoverageStatus::Assisted => "已辅种",
            CoverageStatus::Reseeded => "已转种",
            CoverageStatus::Verifying => "校验中",
            CoverageStatus::ReviewPending => "待人工确认",
            CoverageStatus::OtherVersion => "其他版本",
            CoverageStatus::SameHashSkip => "同 hash 跳过",
            CoverageStatus::Rejected => "人工否决",
            CoverageStatus::Miss => "站内没有",
            CoverageStatus::WaitSource => "待找源",
            CoverageStatus::Failed => "失败",
            CoverageStatus::ReseedDraft => "转种草稿",
            CoverageStatus::Skipped => "已跳过（内容类型）",
            CoverageStatus::PendingOffers => "待走候选入口",
            CoverageStatus::PendingReview => "等站点审核",
            CoverageStatus::ReviewRejected => "站点审核没过",
            CoverageStatus::GoneFromSite => "站上已找不到",
        }
    }

    /// 零请求跳过的终态/暂态（重跑不重复）。
    /// ReseedDraft 算终态：草稿已生成，人工确认前重复检索只是白花请求
    /// （确认动作会把状态改写成 Reseeded/Rejected）。
    /// OtherVersion 也算终态：站点已有同片其他版本是程序化结论，重查只会再次命中
    /// 同一个目标种；真出现同版本时由人工/其他站点的辅种臂覆盖，不再周期性入队。
    pub fn is_settled(&self) -> bool {
        matches!(
            self,
            CoverageStatus::Seeding
                | CoverageStatus::Assisted
                | CoverageStatus::Reseeded
                | CoverageStatus::Verifying
                | CoverageStatus::Rejected
                | CoverageStatus::ReseedDraft
                | CoverageStatus::OtherVersion
                | CoverageStatus::SameHashSkip
                | CoverageStatus::Skipped
                // 等站点候选入口：程序重试只会再被同一规则挡住，所以不入队；
                // 但它不是“完事”，`status` / `report` 会单列成待跟进。
                | CoverageStatus::PendingOffers
                // 等站点审核 / 审核没过 / 站上已找不到：都在等站点侧或人来处理，
                // 程序再排一次只会重复上传；同样单列成待跟进，由回访（reviewcheck）收尾。
                | CoverageStatus::PendingReview
                | CoverageStatus::ReviewRejected
                | CoverageStatus::GoneFromSite
        )
    }

    /// 站点侧审核相关的三个状态：要回访详情页才能收尾。
    pub fn is_review_related(&self) -> bool {
        matches!(
            self,
            CoverageStatus::PendingReview
                | CoverageStatus::ReviewRejected
                | CoverageStatus::GoneFromSite
        )
    }
}

/// 内容条目里每个成员种子的记账
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentMeta {
    pub save_path: String,
    pub category: Option<String>,
    pub last_seen: i64,
    /// 该种子自己挂的 tracker host（小写；条目的 trackerHosts 是并集）
    pub tracker_hosts: Vec<String>,
    pub hash_v2: Option<String>,
    /// 进下载器的时间（老索引没有这条，缺省 0）
    #[serde(default)]
    pub added_on: i64,
}

/// 内容 × 站点 的一个格子
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteCoverage {
    pub status: CoverageStatus,
    /// 结论时间（unix 秒）；miss 负缓存 TTL 也看它
    pub at: i64,
    /// 命中种子/新发种的详情页
    pub url: Option<String>,
    /// 给人看的备注（判定原因/失败原因）
    pub note: Option<String>,
    /// 校验中格子专属：上一轮推送暂停在下载器里那条种子的 hash（收尾 pass 的凭据）
    #[serde(default)]
    pub verify_hash: Option<String>,
}

/// 源种定位结果（方案第五节）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecord {
    pub site_id: String,
    pub detail_url: String,
    pub resolved_at: i64,
    /// 定位失败的负缓存时间（TTL 更长：源站新发种比目标站慢，没必要常回去问）
    pub miss_at: Option<i64>,
}

impl SourceRecord {
    /// 定位失败的负缓存记录：site_id/detail_url 留空，判据是 miss_at
    pub fn miss(at: i64) -> Self {
        Self {
            site_id: String::new(),
            detail_url: String::new(),
            resolved_at: 0,
            miss_at: Some(at),
        }
    }

    /// 是否已定位到源种（负缓存记录不算）
    pub fn is_resolved(&self) -> bool {
        !self.detail_url.is_empty()
    }
}

/// 一段内容
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentEntry {
    pub name: String,
    pub total_size: i64,
    pub file_count: usize,
    /// 逐文件清单摘要（判同比对时现算，不存逐文件清单本身）
    pub manifest_digest: String,
    /// 成员种子：v1 hash → 记账
    pub torrents: BTreeMap<String, TorrentMeta>,
    /// 各成员种子的 tracker host 并集
    pub tracker_hosts: Vec<String>,
    pub first_seen: i64,
    /// 全部成员种子都从下载器消失的时间（保留 archiveDays 天再归档）
    pub gone_since: Option<i64>,
    /// 内容 × 站点 决策表
    pub coverage: BTreeMap<String, SiteCoverage>,
    pub source: Option<SourceRecord>,
    /// 源种带禁转标记 → 该内容永不进转种臂（辅种不算转载发布，不受影响）
    pub forbid_reseed: bool,
}

impl ContentEntry {
    /// 全部身份 hash（v1 ∪ v2）
    pub fn all_hashes(&self) -> Vec<String> {
        let mut v: Vec<String> = self.torrents.keys().cloned().collect();
        for m in self.torrents.values() {
            if let Some(h2) = &m.hash_v2 {
                v.push(h2.clone());
            }
        }
        v
    }

    /// 任取一条成员种子的记账（代表；检索用名字/路径就从它拿）
    pub fn pick_meta(&self) -> Option<(&String, &TorrentMeta)> {
        self.torrents.iter().next()
    }

    /// 成员种子里最晚进下载器的时间：辅种优先级「新入库优先」用
    pub fn newest_added(&self) -> i64 {
        self.torrents
            .values()
            .map(|m| m.added_on)
            .max()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScanDelta {
    pub new_contents: usize,
    /// 已有内容新并入的种子数（跨站重新打包被并查集合上的）
    pub merged: usize,
    pub removed_torrents: usize,
    pub purged_contents: usize,
    /// 跳过的未完成种子（文件不全，不该进索引）
    pub skipped_incomplete: usize,
}

#[derive(Serialize, Deserialize)]
struct IndexFile {
    contents: BTreeMap<String, ContentEntry>,
}

/// 落盘用的借用视图：save 不为序列化把整个索引深拷贝一遍
#[derive(Serialize)]
struct IndexFileRef<'a> {
    contents: &'a BTreeMap<String, ContentEntry>,
}

pub struct ContentIndex {
    path: PathBuf,
    pub contents: BTreeMap<String, ContentEntry>,
}

impl ContentIndex {
    /// 纯内存索引（测试/报告计算用，不落盘）
    pub fn in_memory() -> Self {
        Self {
            path: PathBuf::new(),
            contents: BTreeMap::new(),
        }
    }

    pub fn load(path: &Path) -> Self {
        let contents = match std::fs::read_to_string(path) {
            // 没有文件 = 初次运行，正常给空索引
            Err(_) => BTreeMap::new(),
            Ok(text) => match serde_json::from_str::<IndexFile>(&text) {
                Ok(f) => f.contents,
                // 损坏（半截写入一类）：把原文件挪开留证，别让下一次 save 无声覆盖掉——
                // 「已辅/已转」的记录还能从备份里捞回来
                Err(_) => {
                    let backup = path.with_extension("json.corrupt");
                    if std::fs::rename(path, &backup).is_err() {
                        let _ = std::fs::copy(path, &backup);
                    }
                    BTreeMap::new()
                }
            },
        };
        Self {
            path: path.to_path_buf(),
            contents,
        }
    }

    /// 原子落盘（先写 tmp 再 rename）
    pub fn save(&self) -> Result<()> {
        let body = serde_json::to_string_pretty(&IndexFileRef {
            contents: &self.contents,
        })
        .context("索引序列化失败")?;
        // 统一走 lib::atomic_write（tmp + rename），与报告/配置/缓存同一套实现
        crate::atomic_write(&self.path, body.as_bytes())
    }

    /// v1/v2 hash → contentKey 的映射（检索命中后反查内容用）
    pub fn hash_to_key(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        for (key, e) in &self.contents {
            for h in e.all_hashes() {
                m.entry(h).or_insert_with(|| key.clone());
            }
        }
        m
    }

    pub fn digest_to_key(&self) -> HashMap<String, String> {
        self.contents
            .iter()
            .filter(|(_, e)| !e.manifest_digest.is_empty())
            .map(|(k, e)| (e.manifest_digest.clone(), k.clone()))
            .collect()
    }

    /// 增量合并一次扫描。`files_of` 只对「新出现的 hash」或「还没算过摘要的条目」调用
    /// （qB 的 torrents/files 是逐种一次请求，增量只花在新种子上）。
    pub fn merge_scan(
        &mut self,
        torrents: &[LocalTorrent],
        mut files_of: impl FnMut(&LocalTorrent) -> Option<Vec<(String, i64)>>,
        now: i64,
        archive_days: i64,
    ) -> ScanDelta {
        let mut delta = ScanDelta::default();
        let mut current: HashSet<String> = HashSet::new();
        // 身份映射本地维护：扫内新建/并入的 hash 立刻可查，不用每条种子重建全表
        let mut h2k = self.hash_to_key();
        let mut d2k = self.digest_to_key();

        for t in torrents {
            if t.progress < 1.0 {
                delta.skipped_incomplete += 1;
                continue;
            }
            current.insert(t.hash.clone());
            let hashes = t.all_hashes();
            let mut target_key = hashes.iter().find_map(|h| h2k.get(h).cloned());
            let mut created = false;
            // 本轮内 files_of 已对该种子失败过就别再问（qB API 抖动时请求会翻倍）
            let mut files_failed = false;

            if target_key.is_none() {
                // hash 查不到不急着建新条目：先算 manifest，可能是别的站的重新打包
                if let Some(files) = files_of(t) {
                    let fp = TorrentFingerprint::from_local_files(&files);
                    let digest = manifest_digest(&comparable_key(&fp));
                    target_key = d2k.get(&digest).cloned();
                    if target_key.is_none() {
                        let key = format!("m:{digest}");
                        self.contents
                            .insert(key.clone(), Self::new_entry(t, &fp, &digest, now));
                        d2k.insert(digest, key.clone());
                        delta.new_contents += 1;
                        created = true;
                        target_key = Some(key);
                    }
                } else {
                    // 拿不到文件清单（下载器 API 出问题）：退回 hash-only 条目，摘要下次扫描补
                    files_failed = true;
                    let key = format!("h:{}", t.hash);
                    let placeholder = TorrentFingerprint::new(String::new(), Vec::new());
                    self.contents
                        .insert(key.clone(), Self::new_entry(t, &placeholder, "", now));
                    delta.new_contents += 1;
                    created = true;
                    target_key = Some(key);
                }
            }
            let Some(key) = target_key else { continue };
            let already_member;
            {
                let entry = self.contents.get_mut(&key).unwrap();
                already_member = entry.torrents.contains_key(&t.hash);
                entry.gone_since = None;
                if entry.name.is_empty() {
                    entry.name = t.name.clone();
                }
                // 老条目还没有摘要（hash-only 建的）而这次拿到了文件清单 → 补上
                if entry.manifest_digest.is_empty() && !files_failed {
                    if let Some(files) = files_of(t) {
                        let fp = TorrentFingerprint::from_local_files(&files);
                        entry.manifest_digest = manifest_digest(&comparable_key(&fp));
                        entry.total_size = fp.total_size;
                        entry.file_count = fp.file_count();
                    }
                }
                // 记账/更新（savePath、分类变化直接覆盖）
                entry.torrents.insert(
                    t.hash.clone(),
                    TorrentMeta {
                        save_path: t.save_path.clone(),
                        category: t.category.clone(),
                        last_seen: now,
                        tracker_hosts: t.tracker_hosts.clone(),
                        hash_v2: t.hash_v2.clone(),
                        added_on: t.added_on,
                    },
                );
            }
            if !created && !already_member {
                delta.merged += 1;
            }
            for h in &hashes {
                h2k.insert(h.clone(), key.clone());
            }
        }

        // 种子消失：从条目里摘除；条目空了标 gone_since，过期归档
        let before = self.contents.len();
        for entry in self.contents.values_mut() {
            let vanished: Vec<String> = entry
                .torrents
                .keys()
                .filter(|h| !current.contains(*h))
                .cloned()
                .collect();
            if !vanished.is_empty() {
                delta.removed_torrents += vanished.len();
                for h in &vanished {
                    entry.torrents.remove(h);
                }
            }
            if entry.torrents.is_empty() && entry.gone_since.is_none() {
                entry.gone_since = Some(now);
            }
            // tracker host 并集重算
            let hosts: HashSet<String> = entry
                .torrents
                .values()
                .flat_map(|m| m.tracker_hosts.iter().cloned())
                .collect();
            entry.tracker_hosts = {
                let mut v: Vec<String> = hosts.into_iter().collect();
                v.sort();
                v
            };
        }
        let cutoff = now - archive_days * 86_400;
        self.contents
            .retain(|_, e| e.gone_since.map(|g| g >= cutoff).unwrap_or(true));
        delta.purged_contents = before - self.contents.len();
        delta
    }

    fn new_entry(
        t: &LocalTorrent,
        fp: &TorrentFingerprint,
        digest: &str,
        now: i64,
    ) -> ContentEntry {
        ContentEntry {
            name: t.name.clone(),
            total_size: if fp.total_size > 0 {
                fp.total_size
            } else {
                t.total_size
            },
            file_count: fp.file_count(),
            manifest_digest: digest.to_string(),
            torrents: BTreeMap::new(),
            tracker_hosts: Vec::new(),
            first_seen: now,
            gone_since: None,
            coverage: BTreeMap::new(),
            source: None,
            forbid_reseed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torrent(hash: &str, hash_v2: Option<&str>, size: i64, hosts: &[&str]) -> LocalTorrent {
        LocalTorrent::new(
            hash,
            hash_v2,
            &format!("Show.{hash}"),
            "/data/",
            None,
            vec![],
            size,
            1.0,
            hosts.iter().map(|h| h.to_string()).collect(),
            1000,
        )
    }

    fn files_of(
        files: Vec<(String, i64)>,
    ) -> impl FnMut(&LocalTorrent) -> Option<Vec<(String, i64)>> {
        move |_| Some(files.clone())
    }

    #[test]
    fn other_version_is_terminal() {
        // 「目标站已有其他版本」是程序化结论：重复入队只会再命中同一个目标种
        assert!(CoverageStatus::OtherVersion.is_settled());
        assert!(!CoverageStatus::ReviewPending.is_settled());
        assert!(!CoverageStatus::Failed.is_settled());
        // 等站点候选入口：不入队重试，但也不算“完事”（`status` 单列待跟进）
        assert!(CoverageStatus::PendingOffers.is_settled());
        // 等站点审核 / 审核没过 / 站上已找不到：都不入队重发，由回访收尾
        for s in [
            CoverageStatus::PendingReview,
            CoverageStatus::ReviewRejected,
            CoverageStatus::GoneFromSite,
        ] {
            assert!(s.is_settled(), "{s:?} 应该是不入队重发的状态");
            assert!(s.is_review_related());
        }
        assert!(!CoverageStatus::PendingOffers.is_review_related());
        assert!(!CoverageStatus::Reseeded.is_review_related());
    }

    #[test]
    fn same_content_merges_across_repacks() {
        let mut idx = ContentIndex {
            path: PathBuf::from("/dev/null"),
            contents: BTreeMap::new(),
        };
        // 两条种子：hash 不同（站点重新打包），manifest 全等 → 同一段内容
        let files = vec![
            ("Pack/a.mkv".to_string(), 100),
            ("Pack/s/ch.srt".to_string(), 5),
        ];
        let d = idx.merge_scan(
            &[
                torrent("aaaa", None, 105, &["hdsky.me"]),
                torrent("bbbb", None, 105, &["pterclub.net"]),
            ],
            files_of(files),
            1000,
            30,
        );
        assert_eq!(d.new_contents, 1);
        assert_eq!(d.merged, 1);
        assert_eq!(idx.contents.len(), 1);
        let (key, entry) = idx.contents.iter().next().unwrap();
        assert!(key.starts_with("m:"), "key = {key}");
        assert_eq!(entry.torrents.len(), 2, "两条种子并进同一段内容");
        assert!(entry.tracker_hosts.contains(&"hdsky.me".to_string()));
        assert!(entry.tracker_hosts.contains(&"pterclub.net".to_string()));
        assert!(!entry.manifest_digest.is_empty());
    }

    #[test]
    fn v2_hash_union() {
        let mut idx = ContentIndex {
            path: PathBuf::from("/dev/null"),
            contents: BTreeMap::new(),
        };
        // 一条 v1/v2 混合、一条 v2 相同（qB 换了代表种子）→ 并查集合上
        let d = idx.merge_scan(
            &[
                torrent("aaaa", Some("v2hash"), 100, &[]),
                torrent("bbbb", Some("v2hash"), 100, &[]),
            ],
            files_of(vec![("Pack/a.mkv".to_string(), 100)]),
            1000,
            30,
        );
        assert_eq!(d.new_contents, 1);
        assert_eq!(d.merged, 1);
        assert_eq!(idx.contents.len(), 1);
        assert_eq!(idx.contents.values().next().unwrap().torrents.len(), 2);
    }

    #[test]
    fn different_content_stays_apart() {
        let mut idx = ContentIndex {
            path: PathBuf::from("/dev/null"),
            contents: BTreeMap::new(),
        };
        let d = idx.merge_scan(
            &[
                torrent("aaaa", None, 100, &[]),
                torrent("bbbb", None, 90, &[]),
            ],
            |t| {
                Some(vec![(
                    "Pack/a.mkv".to_string(),
                    if t.hash == "aaaa" { 100 } else { 90 },
                )])
            },
            1000,
            30,
        );
        assert_eq!(d.new_contents, 2);
        assert_eq!(idx.contents.len(), 2, "大小不同 → manifest 不同 → 两个内容");
    }

    #[test]
    fn rescan_is_incremental_and_handles_disappearance() {
        let mut idx = ContentIndex {
            path: PathBuf::from("/dev/null"),
            contents: BTreeMap::new(),
        };
        let files = vec![("Pack/a.mkv".to_string(), 100)];
        idx.merge_scan(
            &[torrent("aaaa", None, 100, &[])],
            files_of(files.clone()),
            1000,
            30,
        );
        // 重扫同一条：无新内容，不调文件清单（用计数器证明）
        let mut calls = 0;
        let d = idx.merge_scan(
            &[torrent("aaaa", None, 100, &[])],
            |_| {
                calls += 1;
                Some(files.clone())
            },
            2000,
            30,
        );
        assert_eq!(d.new_contents, 0);
        assert_eq!(d.merged, 0);
        assert_eq!(calls, 0, "已知 hash 不再调文件清单");
        // 种子消失：条目保留（gone_since），不立刻删
        let d = idx.merge_scan(&[], |_| None, 3000, 30);
        assert_eq!(d.removed_torrents, 1);
        assert_eq!(idx.contents.len(), 1);
        assert_eq!(idx.contents.values().next().unwrap().gone_since, Some(3000));
        // 超过归档期（30 天）→ 清掉
        let d = idx.merge_scan(&[], |_| None, 3000 + 31 * 86_400, 30);
        assert_eq!(d.purged_contents, 1);
        assert!(idx.contents.is_empty());
    }

    #[test]
    fn hash_only_entry_gets_digest_later() {
        let mut idx = ContentIndex {
            path: PathBuf::from("/dev/null"),
            contents: BTreeMap::new(),
        };
        // 第一轮拿不到文件清单：hash-only 条目
        idx.merge_scan(&[torrent("aaaa", None, 100, &[])], |_| None, 1000, 30);
        let (k, e) = idx.contents.iter().next().unwrap();
        assert!(k.starts_with("h:"));
        assert!(e.manifest_digest.is_empty());
        // 第二轮拿到了：补摘要（条目还是那个，不重建）
        idx.merge_scan(
            &[torrent("aaaa", None, 100, &[])],
            files_of(vec![("Pack/a.mkv".to_string(), 100)]),
            2000,
            30,
        );
        assert_eq!(idx.contents.len(), 1);
        let e = idx.contents.values().next().unwrap();
        assert!(!e.manifest_digest.is_empty());
        assert_eq!(e.file_count, 1);
        // 第三轮另一站重新打包的种子进来：manifest 现在能对上，并入同一内容
        let d = idx.merge_scan(
            &[torrent("bbbb", None, 100, &[])],
            files_of(vec![("OtherPack/a.mkv".to_string(), 100)]),
            3000,
            30,
        );
        assert_eq!(d.new_contents, 0);
        assert_eq!(d.merged, 1);
    }

    #[test]
    fn incomplete_torrents_skipped() {
        let mut idx = ContentIndex {
            path: PathBuf::from("/dev/null"),
            contents: BTreeMap::new(),
        };
        let mut t = torrent("aaaa", None, 100, &[]);
        t.progress = 0.5;
        let d = idx.merge_scan(&[t], files_of(vec![]), 1000, 30);
        assert_eq!(d.skipped_incomplete, 1);
        assert!(idx.contents.is_empty());
    }

    #[test]
    fn persistence_roundtrip() {
        let dir = std::env::temp_dir().join(format!("bsend-idx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("coverage.json");
        let _ = std::fs::remove_file(&path);
        {
            let mut idx = ContentIndex::load(&path);
            idx.merge_scan(
                &[torrent("aaaa", None, 100, &["hdsky.me"])],
                files_of(vec![("Pack/a.mkv".to_string(), 100)]),
                1000,
                30,
            );
            let e = idx.contents.values_mut().next().unwrap();
            e.coverage.insert(
                "hdsky".to_string(),
                SiteCoverage {
                    status: CoverageStatus::Assisted,
                    at: 1234,
                    url: Some("https://hdsky.me/details.php?id=1".to_string()),
                    note: Some("info hash 一致".to_string()),
                    verify_hash: None,
                },
            );
            idx.save().unwrap();
        }
        let idx2 = ContentIndex::load(&path);
        assert_eq!(idx2.contents.len(), 1);
        let e = idx2.contents.values().next().unwrap();
        assert_eq!(e.coverage["hdsky"].status, CoverageStatus::Assisted);
        assert_eq!(e.tracker_hosts, vec!["hdsky.me"]);
        // 落盘是明文 JSON 且字段名是驼峰（方案文档的格式，人也要能直接看）
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"assisted\""),
            "text head: {}",
            &text[..text.len().min(200)]
        );
        assert!(text.contains("\"manifestDigest\""));
        // 坏文件容错（不炸，给空索引；原文件挪成 .corrupt 留证，不被新 save 覆盖）
        std::fs::write(&path, b"not json").unwrap();
        assert!(ContentIndex::load(&path).contents.is_empty());
        let backup = dir.join("coverage.json.corrupt");
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "not json",
            "坏文件要留备份"
        );
        // 原文件已挪走：再 load 还是空（不会把备份当索引读）
        assert!(ContentIndex::load(&path).contents.is_empty());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&backup);
    }

    #[test]
    fn files_of_failure_is_called_once_per_torrent() {
        let mut idx = ContentIndex {
            path: PathBuf::from("/dev/null"),
            contents: BTreeMap::new(),
        };
        // 下载器 API 出问题（拿不到文件清单）：同一次扫描里对同一条种子只许问一次
        let mut calls = 0;
        idx.merge_scan(
            &[torrent("aaaa", None, 100, &[])],
            |_| {
                calls += 1;
                None
            },
            1000,
            30,
        );
        assert_eq!(calls, 1, "hash-only 失败路径不该二次调 files_of");
        assert!(idx.contents.contains_key("h:aaaa"));
    }
}
