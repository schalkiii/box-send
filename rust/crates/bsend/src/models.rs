//! 数据模型：本地种子、文件指纹、判定级别。与 Swift 版 `Assist/AssistModels.swift` 对齐。

use serde::{Deserialize, Serialize};

use crate::manifest;

/// 下载器里的一条种子。字段取自 qBittorrent `/torrents/info`（Transmission 的映射在 Swift 版，
/// Rust 版 v1 只做 qB——辅种扫描的前提本来就是 qB）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalTorrent {
    /// v1 info hash（小写 40 位）
    pub hash: String,
    /// v2 种子的 v2 hash（qB 的 infohash_v2）：站点那份多半只有 v1，比对时两个都要带上
    pub hash_v2: Option<String>,
    pub name: String,
    /// 种子内容所在目录（qB 的 save_path）：辅种必须沿用，否则下载器找不到文件会重下
    pub save_path: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub total_size: i64,
    /// 已完成比例（0…1）；没下完的种子文件不全，不该去辅
    pub progress: f64,
    /// 该种子现有的 tracker host（小写）：判断「这份文件已经在哪些站做种」
    pub tracker_hosts: Vec<String>,
    /// 加进下载器的时间（qB 的 added_on）：辅种优先级「新入库优先」看它
    pub added_on: i64,
}

impl LocalTorrent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hash: &str,
        hash_v2: Option<&str>,
        name: &str,
        save_path: &str,
        category: Option<&str>,
        tags: Vec<String>,
        total_size: i64,
        progress: f64,
        tracker_hosts: Vec<String>,
        added_on: i64,
    ) -> Self {
        Self {
            hash: hash.to_lowercase(),
            hash_v2: hash_v2.map(|h| h.to_lowercase()),
            name: name.to_string(),
            save_path: save_path.to_string(),
            category: category.filter(|c| !c.is_empty()).map(|c| c.to_string()),
            tags,
            total_size,
            progress,
            tracker_hosts: tracker_hosts
                .into_iter()
                .map(|h| h.to_lowercase())
                .collect(),
            added_on,
        }
    }

    /// 比对可用的全部 hash（v1 + v2）
    pub fn all_hashes(&self) -> Vec<String> {
        let mut v = vec![self.hash.clone()];
        if let Some(h2) = &self.hash_v2 {
            v.push(h2.clone());
        }
        v
    }
}

/// 一条种子的文件指纹：逐文件的相对路径 + 字节数
#[derive(Debug, Clone)]
pub struct TorrentFingerprint {
    /// 内容根目录名（.torrent 的 info.name）：决定下载器去哪儿找文件
    pub root: String,
    pub files: Vec<(String, i64)>,
    pub total_size: i64,
}

impl TorrentFingerprint {
    pub fn new(root: String, files: Vec<(String, i64)>) -> Self {
        let total_size = files.iter().map(|(_, s)| s).sum();
        Self {
            root,
            files,
            total_size,
        }
    }

    /// 下载器报的文件路径反推指纹（qB 的路径就是它期望的相对路径）
    pub fn from_local_files(files: &[(String, i64)]) -> Self {
        let root = manifest::common_root(files);
        Self::new(root, files.to_vec())
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

/// 判定级别：站点那条种子与本地这条是什么关系（Swift `AssistLevel` 对齐）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JudgeLevel {
    /// 站点 .torrent 的 info hash 与本地一致（v1 或 v2）：同一种子，直接辅
    SameHash { hash: String },
    /// hash 不同但逐文件清单完全一致：站点重新打包（改了 source/private 标记），可以辅
    SameFiles,
    /// 只有名字像，清单对不上：不动下载器，交人工
    NameOnly { site_name: String },
    /// 站内没检索到，或这条根本没法辅（原因给人看）
    Skip { why: String },
}

/// 目录布局比对：内容相同但布局对不上时，下载器找不到文件会去重下一整份，
/// 所以「内容是否一致」与「布局是否对得上」必须分开判。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// 完全一致，直接加
    Same,
    /// 只差最外面一层目录名：内容没问题，改站点种子期望的路径就能成
    RootDiffers,
    /// 结构不同，别加
    Differs,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_torrent_normalizes_and_filters() {
        let t = LocalTorrent::new(
            "ABCDEF0123456789ABCDEF0123456789ABCDEF01",
            Some("FEDCBA9876543210FEDCBA9876543210"),
            "Pack",
            "/data",
            Some(""),
            vec!["Boxsend".into()],
            105,
            1.0,
            vec!["TRACKER.Example.COM".into()],
            42,
        );
        assert_eq!(
            t.hash, "abcdef0123456789abcdef0123456789abcdef01",
            "v1 hash 必须小写归一"
        );
        assert_eq!(
            t.hash_v2.as_deref(),
            Some("fedcba9876543210fedcba9876543210"),
            "v2 hash 同样归一"
        );
        assert_eq!(t.category, None, "空 category 不能保留空串");
        assert_eq!(t.tracker_hosts, vec!["tracker.example.com"]);
        assert_eq!(
            t.all_hashes(),
            vec![t.hash.clone(), t.hash_v2.clone().unwrap()]
        );
    }

    #[test]
    fn fingerprint_sums_files_and_derives_root() {
        let fp = TorrentFingerprint::from_local_files(&[
            ("Pack/a.mkv".into(), 100),
            ("Pack/s/ch.srt".into(), 5),
        ]);
        assert_eq!(fp.root, "Pack");
        assert_eq!(fp.total_size, 105);
        assert_eq!(fp.file_count(), 2);

        let empty = TorrentFingerprint::new(String::new(), Vec::new());
        assert_eq!(empty.total_size, 0);
        assert_eq!(empty.file_count(), 0);
        assert_eq!(empty.root, "");
    }

    #[test]
    fn judge_level_uses_kind_tag_and_snake_case() {
        let v = serde_json::to_value(JudgeLevel::SameFiles).unwrap();
        assert_eq!(v["kind"], "same_files");
        let v = serde_json::to_value(JudgeLevel::SameHash { hash: "A".into() }).unwrap();
        assert_eq!(v["kind"], "same_hash");
        assert_eq!(v["hash"], "A");
        let v = serde_json::to_value(JudgeLevel::NameOnly {
            site_name: "站名".into(),
        })
        .unwrap();
        assert_eq!(v["kind"], "name_only");
        assert_eq!(v["site_name"], "站名");
    }
}
