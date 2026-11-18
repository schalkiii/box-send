//! 推送种子的落盘布局决策：让新种子复用已在盘上的文件。
//!
//! qB 的 `savepath` 是下载根：多文件种子会落到 `<savepath>/<info.name>/`。
//! 同一内容在两个站的根目录名经常不同（`Show.S01.1080p-GRP` vs `Show S01 1080p GRP`），
//! 直接沿用旧 `savepath` 会落到一个新文件夹里——校验一遍发现文件不存在，回滚重下。
//!
//! 这里按「根名是否一致 + 去根后的文件清单是否一致」决定推种参数：
//! - 根名一致：原样沿用既有种子的 `savepath`；
//! - 根名不同、文件清单一致：`savepath` 指到既有内容目录 + `contentLayout=NoSubfolder`
//!   （qB 4.4+：不建根子目录，把新种的文件直接映射进既有目录）；
//! - 文件清单也不同：不推，明确报错（省一次下载器往返与校验回滚）。

/// 推种时的落盘决策
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddLayout {
    /// 直接沿用既有种子的 savepath
    Same { save_path: String },
    /// 复用既有内容目录：savepath = 既有内容目录，contentLayout = NoSubfolder
    Remap {
        save_path: String,
        content_layout: &'static str,
    },
    /// 内容布局对不上，不能辅（含原因）
    Mismatch(String),
}

impl AddLayout {
    /// 给 qB `torrents/add` 的 savepath
    pub fn save_path(&self) -> Option<&str> {
        match self {
            AddLayout::Same { save_path } | AddLayout::Remap { save_path, .. } => Some(save_path),
            AddLayout::Mismatch(_) => None,
        }
    }

    /// 给 qB `torrents/add` 的 contentLayout（只有需要重映射时才传）
    pub fn content_layout(&self) -> Option<&'static str> {
        match self {
            AddLayout::Remap { content_layout, .. } => Some(content_layout),
            _ => None,
        }
    }

    /// 人看的一行（写日志/格子备注）
    pub fn describe(&self) -> String {
        match self {
            AddLayout::Same { save_path } => format!("savepath={save_path}"),
            AddLayout::Remap { save_path, .. } => {
                format!(
                    "savepath={save_path} + contentLayout=NoSubfolder（根名不同，复用既有目录）"
                )
            }
            AddLayout::Mismatch(why) => format!("布局对不上：{why}"),
        }
    }
}

/// 路径统一成 `/` 分隔用于比较（qB 在 Windows 上给的是 `I:/Download`）
fn norm_sep(s: &str) -> String {
    s.replace('\\', "/")
}

/// 目录拼接：去掉尾部分隔符再接
fn join_dir(dir: &str, name: &str) -> String {
    let d = norm_sep(dir);
    if d.is_empty() {
        name.to_string()
    } else if d.ends_with('/') {
        format!("{d}{name}")
    } else {
        format!("{d}/{name}")
    }
}

/// 去掉路径首段的根目录（多文件种子的 file_list 首段是 info.name）
fn strip_root(path: &str, root: &str) -> Option<String> {
    let p = norm_sep(path);
    let prefix = format!("{root}/");
    if p == root {
        Some(root.to_string())
    } else {
        p.strip_prefix(&prefix).map(|s| s.to_string())
    }
}

/// 文件清单（路径+大小）是否一致（顺序无关，按路径排序比较）
fn same_files(a: &[(String, i64)], b: &[(String, i64)]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut va: Vec<(String, i64)> = a.iter().map(|(p, s)| (norm_sep(p), *s)).collect();
    let mut vb: Vec<(String, i64)> = b.iter().map(|(p, s)| (norm_sep(p), *s)).collect();
    va.sort();
    vb.sort();
    va == vb
}

/// 决策：新种子能不能复用既有种子的落盘位置。
///
/// - `new_files` / `existing_files` 都应是「相对于下载根」的路径（多文件含根段，
///   单文件即文件名），与 `bencode::file_list` / qB `torrents/files` 的形态一致；
/// - 拿不到/解析不了既有清单时不敢乱改，退回「原样沿用 savepath」（校验不通过会回滚，
///   与旧行为一致）；只有两边清单都拿到、且明确不一致时才拒绝。
pub fn plan_add_layout(
    new_name: &str,
    new_files: &[(String, i64)],
    existing_name: &str,
    existing_save_path: &str,
    existing_files: &[(String, i64)],
) -> AddLayout {
    if existing_save_path.trim().is_empty() {
        return AddLayout::Mismatch("既有种子没有保存路径（索引里 savePath 为空）".into());
    }
    let new_root = norm_sep(new_name);
    let old_root = norm_sep(existing_name);
    // 根名一致：qB 会落到同一个目录，原样沿用
    if new_root == old_root || old_root.is_empty() {
        return AddLayout::Same {
            save_path: existing_save_path.to_string(),
        };
    }
    // 根名不同：拿清单判断内层是否一致；拿不到就不敢映射，退回原样沿用
    let new_inner: Vec<(String, i64)> = new_files
        .iter()
        .filter_map(|(p, s)| strip_root(p, &new_root).map(|q| (q, *s)))
        .collect();
    let old_inner: Vec<(String, i64)> = existing_files
        .iter()
        .filter_map(|(p, s)| strip_root(p, &old_root).map(|q| (q, *s)))
        .collect();
    if new_inner.is_empty() || old_inner.is_empty() {
        return AddLayout::Same {
            save_path: existing_save_path.to_string(),
        };
    }
    if !same_files(&new_inner, &old_inner) {
        return AddLayout::Mismatch(format!(
            "根名不同且内层文件清单不一致（新 `{new_root}` {} 个文件 / 旧 `{old_root}` {} 个文件）",
            new_inner.len(),
            old_inner.len()
        ));
    }
    // 单文件：内层只有一个文件名，没法「复用目录」——文件名不同就不能直接放
    let single = new_root == norm_sep(&new_files[0].0);
    if single {
        return AddLayout::Mismatch(format!(
            "单文件种子且文件名不同（新 `{new_root}` ≠ 旧 `{old_root}`），需先改名"
        ));
    }
    AddLayout::Remap {
        save_path: join_dir(existing_save_path, &old_root),
        content_layout: "NoSubfolder",
    }
}

/// 推种前落盘决策（带下载器现场取证）：优先用既有种子的实时 savepath/根名，
/// 根名不同才去拉文件清单判断内层是否一致。拿不到实时记录时退回索引里的值。
pub fn resolve_for_torrent(
    qb: &crate::qbittorrent::QBittorrent,
    rep_hash: &str,
    data: &[u8],
    fallback_name: &str,
    fallback_save_path: &str,
) -> AddLayout {
    let live = qb.torrent_meta(rep_hash).ok().flatten();
    let (existing_name, save_path) = match &live {
        Some(m) if !m.save_path.trim().is_empty() => (m.name.clone(), m.save_path.clone()),
        _ => (fallback_name.to_string(), fallback_save_path.to_string()),
    };
    let new_name = crate::bencode::info_name(data).unwrap_or_else(|| fallback_name.to_string());
    let new_files = crate::bencode::file_list(data).unwrap_or_default();
    if existing_name == new_name || existing_name.is_empty() {
        return plan_add_layout(&new_name, &new_files, &existing_name, &save_path, &[]);
    }
    let existing_files = qb.torrent_files(rep_hash).unwrap_or_default();
    plan_add_layout(
        &new_name,
        &new_files,
        &existing_name,
        &save_path,
        &existing_files,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(p: &str, s: i64) -> (String, i64) {
        (p.to_string(), s)
    }

    #[test]
    fn same_root_keeps_save_path() {
        let new_files = vec![f("Show.S01-GRP/a.mkv", 100), f("Show.S01-GRP/b.nfo", 1)];
        let old_files = vec![f("Show.S01-GRP/a.mkv", 100), f("Show.S01-GRP/b.nfo", 1)];
        let l = plan_add_layout(
            "Show.S01-GRP",
            &new_files,
            "Show.S01-GRP",
            "I:/Download",
            &old_files,
        );
        assert_eq!(
            l,
            AddLayout::Same {
                save_path: "I:/Download".into()
            }
        );
        assert_eq!(l.save_path(), Some("I:/Download"));
        assert!(l.content_layout().is_none());
    }

    #[test]
    fn different_root_same_files_remaps() {
        // 实测形态：两站根名不同（点分隔 vs 空格），内层文件一致
        let new_files = vec![
            f("Show S01 1080p-GRP/ep1.mkv", 100),
            f("Show S01 1080p-GRP/ep2.mkv", 200),
        ];
        let old_files = vec![
            f("Show.S01.1080p-GRP/ep1.mkv", 100),
            f("Show.S01.1080p-GRP/ep2.mkv", 200),
        ];
        let l = plan_add_layout(
            "Show S01 1080p-GRP",
            &new_files,
            "Show.S01.1080p-GRP",
            "I:/Download",
            &old_files,
        );
        assert_eq!(
            l,
            AddLayout::Remap {
                save_path: "I:/Download/Show.S01.1080p-GRP".into(),
                content_layout: "NoSubfolder",
            }
        );
        assert_eq!(l.content_layout(), Some("NoSubfolder"));
    }

    #[test]
    fn different_files_is_mismatch() {
        let new_files = vec![f("NewRoot/a.mkv", 100)];
        let old_files = vec![f("OldRoot/a.mkv", 999)];
        let l = plan_add_layout("NewRoot", &new_files, "OldRoot", "I:/Download", &old_files);
        assert!(matches!(l, AddLayout::Mismatch(_)), "{l:?}");
        assert!(l.save_path().is_none());
    }

    #[test]
    fn missing_existing_files_keeps_save_path() {
        // 拿不到既有清单：不敢重映射，退回原样沿用（旧行为，校验不通过会回滚）
        let new_files = vec![f("NewRoot/a.mkv", 100)];
        let l = plan_add_layout("NewRoot", &new_files, "OldRoot", "I:/Download", &[]);
        assert_eq!(
            l,
            AddLayout::Same {
                save_path: "I:/Download".into()
            }
        );
    }

    #[test]
    fn empty_save_path_is_mismatch() {
        let l = plan_add_layout("R", &[f("R/a", 1)], "R", "", &[f("R/a", 1)]);
        assert!(matches!(l, AddLayout::Mismatch(_)), "{l:?}");
    }

    #[test]
    fn single_file_different_name_refuses() {
        let l = plan_add_layout(
            "Show.mkv",
            &[f("Show.mkv", 100)],
            "Show.1080p-GRP.mkv",
            "I:/Download",
            &[f("Show.1080p-GRP.mkv", 100)],
        );
        assert!(matches!(l, AddLayout::Mismatch(_)), "{l:?}");
    }

    #[test]
    fn windows_backslashes_normalize() {
        let l = plan_add_layout(
            "A",
            &[f("A/x.mkv", 1)],
            "B",
            "I:\\Download",
            &[f("B\\x.mkv", 1)],
        );
        assert_eq!(
            l,
            AddLayout::Remap {
                save_path: "I:/Download/B".into(),
                content_layout: "NoSubfolder",
            }
        );
    }
}
