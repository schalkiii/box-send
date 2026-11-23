//! 「站点这条和本地这条是不是同一段文件」的判定（Swift `AssistMatcher` 对齐）。
//! 名字会因站点改写、组标签、结果页截断而失真，所以名字只用来挑候选，
//! 定性一律靠下载来的 .torrent：先比 info hash（v1/v2 都算），再比逐文件清单。

use std::collections::{BTreeMap, HashSet};

use crate::bencode;
use crate::manifest::{comparable_key, size_text, Manifest};
use crate::models::{JudgeLevel, Layout, TorrentFingerprint};

/// 判定。`site_torrent` 是站点的 .torrent 原始字节。
pub fn judge(
    local_hashes: &[String],
    local: &TorrentFingerprint,
    site_torrent: &[u8],
    site_name: &str,
) -> (JudgeLevel, String) {
    let hashes: HashSet<String> = local_hashes.iter().map(|h| h.to_lowercase()).collect();
    if let Some(h) = bencode::info_hash_v1(site_torrent) {
        let hl = h.to_lowercase();
        if hashes.contains(&hl) {
            let short: String = hl.chars().take(12).collect();
            return (
                JudgeLevel::SameHash { hash: hl },
                format!("info hash 一致（{}…）", short),
            );
        }
    }
    // qB 的 v2 hash 只在 v2/hybrid 种子上有；站点种子带 file tree 时也比一遍
    if let Some(h2) = bencode::info_hash_v2(site_torrent) {
        let h2l = h2.to_lowercase();
        if hashes.contains(&h2l) {
            let short: String = h2l.chars().take(12).collect();
            return (
                JudgeLevel::SameHash { hash: h2l },
                format!("v2 info hash 一致（{}…）", short),
            );
        }
    }
    let Some(site_files) = bencode::file_list(site_torrent) else {
        return (
            JudgeLevel::NameOnly {
                site_name: site_name.to_string(),
            },
            "站点种子解析不出文件清单，只能靠名字".to_string(),
        );
    };
    let root = bencode::info_name(site_torrent).unwrap_or_default();
    let site = TorrentFingerprint::new(root, site_files);
    let a = comparable_key(local);
    let b = comparable_key(&site);
    if a.keys == b.keys {
        return (
            JudgeLevel::SameFiles,
            format!(
                "逐文件清单全等（{} 个文件、{}），站点重新打包过所以 hash 不同",
                b.count(),
                size_text(b.total_size)
            ),
        );
    }
    // 总大小不同 = 不可能是同一段文件（逐文件清单全等时大小必然相等）。
    // 典型场景：分集 vs 全集、1080p vs 2160p、不同压制组。这类补不了，
    // 硬挂上去只会重下一整份，所以直接判「其他版本」，不再塞进人工确认队列。
    if a.total_size != b.total_size {
        return (
            JudgeLevel::Different,
            format!(
                "总大小 {} vs {}；文件数 {} vs {}",
                size_text(a.total_size),
                size_text(b.total_size),
                a.count(),
                b.count()
            ),
        );
    }
    // 差异说明（界面「待人工确认」里显示）
    let paths_local: BTreeMap<&str, i64> = a
        .keys
        .iter()
        .map(|k| (path_part(k), size_part(k).unwrap_or(-1)))
        .collect();
    let paths_site: BTreeMap<&str, i64> = b
        .keys
        .iter()
        .map(|k| (path_part(k), size_part(k).unwrap_or(-1)))
        .collect();
    let mut reasons: Vec<String> = Vec::new();
    if a.count() != b.count() {
        reasons.push(format!("文件数 {} vs {}", a.count(), b.count()));
    }
    // 同一路径两边都有、大小不同——这是「不是一段文件」最直接的证据
    let mismatched = paths_local
        .keys()
        .filter(|p| {
            paths_site
                .get(*p)
                .map(|s| *s != paths_local[*p])
                .unwrap_or(false)
        })
        .count();
    if mismatched > 0 {
        reasons.push(format!("{} 个同路径文件大小不同", mismatched));
    }
    let only_local = a.keys.difference(&b.keys).count();
    let only_site = b.keys.difference(&a.keys).count();
    if only_local > 0 || only_site > 0 {
        reasons.push(format!(
            "只有本地有 {} 个、只有站点有 {} 个",
            only_local, only_site
        ));
    }
    let why = if reasons.is_empty() {
        "清单对不上".to_string()
    } else {
        reasons.join("；")
    };
    (
        JudgeLevel::NameOnly {
            site_name: site_name.to_string(),
        },
        why,
    )
}

/// 目录布局比对。`local_paths` 取下载器自己报的文件路径，`site_paths` 取站点 .torrent 里写的。
pub fn layout(local_paths: &HashSet<String>, site_paths: &HashSet<String>) -> Layout {
    let a = normalize_set(local_paths);
    let b = normalize_set(site_paths);
    if a == b {
        return Layout::Same;
    }
    if strip_root(&a) == strip_root(&b) {
        return Layout::RootDiffers;
    }
    Layout::Differs
}

/// 从站点 .torrent 构指纹（judge 之外的独立入口：源种定位也用）
pub fn fingerprint_of_torrent(data: &[u8]) -> Option<TorrentFingerprint> {
    let root = bencode::info_name(data)?;
    let files = bencode::file_list(data)?;
    if files.is_empty() {
        return None;
    }
    Some(TorrentFingerprint::new(root, files))
}

/// 站点 .torrent 的清单（比对用）
pub fn manifest_of_torrent(data: &[u8]) -> Option<Manifest> {
    let fp = fingerprint_of_torrent(data)?;
    Some(comparable_key(&fp))
}

fn normalize_set(paths: &HashSet<String>) -> HashSet<String> {
    paths
        .iter()
        .map(|p| {
            p.to_lowercase()
                .replace('\\', "/")
                .split('/')
                .filter(|s| !s.is_empty() && *s != ".")
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

fn strip_root(paths: &HashSet<String>) -> HashSet<String> {
    // 只有一层的路径（单文件种子）没有根目录可剥，原样留着
    paths
        .iter()
        .map(|p| {
            let parts: Vec<&str> = p.split('/').collect();
            if parts.len() > 1 {
                parts[1..].join("/")
            } else {
                p.clone()
            }
        })
        .collect()
}

/// `"path|123"` 的路径段
fn path_part(k: &str) -> &str {
    match k.rfind('|') {
        Some(i) => &k[..i],
        None => k,
    }
}

/// `"path|123"` 的字节数段
fn size_part(k: &str) -> Option<i64> {
    let i = k.rfind('|')?;
    k[i + 1..].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::tests::{enc, BVal};

    fn site_torrent(files: &[(&str, i64)], root: &str) -> Vec<u8> {
        let file_dicts: Vec<BVal> = files
            .iter()
            .map(|(p, s)| {
                let segs: Vec<BVal> = p.split('/').map(BVal::s).collect();
                BVal::d(vec![("length", BVal::I(*s)), ("path", BVal::L(segs))])
            })
            .collect();
        enc(&BVal::d(vec![(
            "info",
            BVal::d(vec![
                ("files", BVal::L(file_dicts)),
                ("name", BVal::s(root)),
                ("piece length", BVal::I(262144)),
                ("pieces", BVal::s("pppppppppppppppppppp")),
            ]),
        )]))
    }

    fn local_fp(files: &[(&str, i64)]) -> TorrentFingerprint {
        TorrentFingerprint::from_local_files(
            &files
                .iter()
                .map(|(p, s)| (p.to_string(), *s))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn level_a_same_hash() {
        let st = site_torrent(&[("a.mkv", 100)], "Pack");
        let hash = bencode::info_hash_v1(&st).unwrap();
        let local = local_fp(&[("Pack/a.mkv", 100)]);
        let (level, why) = judge(&[hash], &local, &st, "Pack");
        assert!(matches!(level, JudgeLevel::SameHash { .. }));
        assert!(why.contains("info hash"));
    }

    #[test]
    fn level_b_same_files_repacked() {
        // 站点重新打包：根目录名不同、内容逐文件一致
        let st = site_torrent(&[("a.mkv", 100), ("s/ch.srt", 5)], "SitePack");
        let local = local_fp(&[("MyPack/a.mkv", 100), ("MyPack/s/ch.srt", 5)]);
        let (level, why) = judge(&["deadbeef".to_string()], &local, &st, "SitePack");
        assert_eq!(level, JudgeLevel::SameFiles);
        assert!(why.contains("重新打包"));
    }

    #[test]
    fn level_c_size_differs_is_other_version() {
        // 站点多一条字幕、总大小不一致：不是同一段文件，应判「其他版本」而非人工确认
        let st = site_torrent(&[("a.mkv", 100), ("s/ch.srt", 5)], "Pack");
        let local = local_fp(&[("Pack/a.mkv", 100)]);
        let (level, why) = judge(&["deadbeef".to_string()], &local, &st, "Pack");
        assert_eq!(level, JudgeLevel::Different);
        assert!(why.contains("总大小"), "why = {}", why);
        assert!(why.contains("文件数 1 vs 2"), "why = {}", why);
    }

    #[test]
    fn level_c_name_only_when_totals_match() {
        // 总大小相等但路径拼不上：信息不足，才交人工
        let st = site_torrent(&[("b.mkv", 100)], "Pack");
        let local = local_fp(&[("Pack/a.mkv", 100)]);
        let (level, why) = judge(&["deadbeef".to_string()], &local, &st, "Pack");
        match level {
            JudgeLevel::NameOnly { site_name } => assert_eq!(site_name, "Pack"),
            other => panic!("expected NameOnly, got {:?}", other),
        }
        assert!(why.contains("只有本地有 1 个"), "why = {}", why);
        assert!(why.contains("只有站点有 1 个"), "why = {}", why);
    }

    #[test]
    fn unparseable_site_torrent_falls_to_name_only() {
        let local = local_fp(&[("Pack/a.mkv", 100)]);
        let (level, why) = judge(&["h".to_string()], &local, b"junk", "whatever");
        assert!(matches!(level, JudgeLevel::NameOnly { .. }));
        assert!(why.contains("解析不出"));
    }

    #[test]
    fn layout_variants() {
        let mk = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<HashSet<_>>();
        assert_eq!(
            layout(&mk(&["Pack/a.mkv"]), &mk(&["Pack/a.mkv"])),
            Layout::Same
        );
        assert_eq!(
            layout(&mk(&["Pack/a.mkv"]), &mk(&["Other/a.mkv"])),
            Layout::RootDiffers
        );
        assert_eq!(
            layout(&mk(&["Pack/a.mkv"]), &mk(&["b.mkv"])),
            Layout::Differs
        );
        // 反斜杠与大小写归一化
        assert_eq!(
            layout(&mk(&["PACK\\A.MKV"]), &mk(&["Pack/a.mkv"])),
            Layout::Same
        );
    }

    fn path_set(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Build a torrent whose `info` dict holds the given pairs (test-only encoder).
    fn torrent_raw(info: Vec<(&str, BVal)>) -> Vec<u8> {
        enc(&BVal::d(vec![("info", BVal::d(info))]))
    }

    // ---------- judge symmetry ----------

    #[test]
    fn judge_symmetry_same_hash() {
        // SameHash is directional by design: only the site torrent's hash is
        // probed against the local hash list. Swapping the two sides still yields
        // SameHash when each side's own torrent hash is supplied on the list.
        let st = site_torrent(&[("a.mkv", 100)], "Pack");
        let h = bencode::info_hash_v1(&st).unwrap();
        let local = local_fp(&[("Pack/a.mkv", 100)]);
        let (level, _) = judge(&[h], &local, &st, "Pack");
        assert!(matches!(level, JudgeLevel::SameHash { .. }));

        let st2 = site_torrent(&[("a.mkv", 100)], "LocalPack");
        let h2 = bencode::info_hash_v1(&st2).unwrap();
        let mirrored = local_fp(&[("Pack/a.mkv", 100)]);
        let (level2, _) = judge(&[h2], &mirrored, &st2, "LocalPack");
        assert!(matches!(level2, JudgeLevel::SameHash { .. }));
    }

    #[test]
    fn judge_symmetry_same_files() {
        let files = &[("a.mkv", 100), ("s/ch.srt", 5)];
        let site_t = site_torrent(files, "SitePack");
        let local = local_fp(&[("MyPack/a.mkv", 100), ("MyPack/s/ch.srt", 5)]);
        let (level, _) = judge(&["0000".into()], &local, &site_t, "SitePack");
        assert_eq!(level, JudgeLevel::SameFiles);

        let local_t = site_torrent(files, "MyPack");
        let mirrored = local_fp(&[("SitePack/a.mkv", 100), ("SitePack/s/ch.srt", 5)]);
        let (level2, _) = judge(&["0000".into()], &mirrored, &local_t, "MyPack");
        assert_eq!(level2, JudgeLevel::SameFiles);
    }

    #[test]
    fn judge_symmetry_different_version() {
        // Forward: local 1 file (100) vs site 2 files (105) -> Different.
        let site_t = site_torrent(&[("a.mkv", 100), ("s/ch.srt", 5)], "Pack");
        let local = local_fp(&[("Pack/a.mkv", 100)]);
        let (level, _) = judge(&["0000".into()], &local, &site_t, "Pack");
        assert_eq!(level, JudgeLevel::Different);

        // Mirrored: site 1 file (100) vs local 2 files (105) -> still Different.
        let local_t = site_torrent(&[("a.mkv", 100)], "Pack");
        let mirrored = local_fp(&[("Pack/a.mkv", 100), ("Pack/s/ch.srt", 5)]);
        let (level2, _) = judge(&["0000".into()], &mirrored, &local_t, "Pack");
        assert_eq!(level2, JudgeLevel::Different);
    }

    // ---------- degraded torrent paths ----------

    #[test]
    fn fingerprint_missing_pieces_still_resolves() {
        // Solidified current behavior: fingerprint_of_torrent reads only
        // info.name + the file list; `pieces` is not consulted here (pieces
        // feeds the pieces-hash source lookup in bencode::pieces_hash). A
        // torrent without `pieces` still resolves to a manifest.
        let d = torrent_raw(vec![
            (
                "files",
                BVal::L(vec![BVal::d(vec![
                    ("length", BVal::I(100)),
                    ("path", BVal::L(vec![BVal::s("a.mkv")])),
                ])]),
            ),
            ("name", BVal::s("Pack")),
        ]);
        let fp = fingerprint_of_torrent(&d).expect("missing pieces must not drop the fingerprint");
        assert_eq!(fp.file_count(), 1);
        let m = manifest_of_torrent(&d).unwrap();
        assert_eq!(m.count(), 1);
        assert_eq!(m.total_size, 100);
    }

    #[test]
    fn fingerprint_single_file_torrent_manifest() {
        // Single-file torrent: only `length` + `name`, no `files` list.
        let d = torrent_raw(vec![
            ("length", BVal::I(700_000_000)),
            ("name", BVal::s("Movie.2020.mkv")),
        ]);
        let fp = fingerprint_of_torrent(&d).unwrap();
        assert_eq!(fp.files, vec![("Movie.2020.mkv".to_string(), 700_000_000)]);
        assert_eq!(fp.total_size, 700_000_000);
        let m = manifest_of_torrent(&d).unwrap();
        assert_eq!(m.count(), 1);
        assert_eq!(m.total_size, 700_000_000);
        assert!(m.keys.contains("movie.2020.mkv|700000000"));
    }

    #[test]
    fn fingerprint_illegal_bencode_both_none() {
        let raws: [&[u8]; 3] = [b"junk", b"", b"d3:fooi1ee"];
        for raw in raws {
            assert!(fingerprint_of_torrent(raw).is_none());
            assert!(manifest_of_torrent(raw).is_none());
        }
    }

    #[test]
    fn fingerprint_v2_only_and_empty_files_both_none() {
        // v2-only layout: `file tree`, no v1 files/length -> list unreadable.
        let v2 = torrent_raw(vec![
            ("file tree", BVal::d(vec![])),
            ("name", BVal::s("v2pack")),
        ]);
        assert!(fingerprint_of_torrent(&v2).is_none());
        assert!(manifest_of_torrent(&v2).is_none());

        // Explicitly empty files list must not be misread as a single-file torrent.
        let empty = torrent_raw(vec![
            ("files", BVal::L(vec![])),
            ("name", BVal::s("empty-pack")),
        ]);
        assert!(fingerprint_of_torrent(&empty).is_none());
        assert!(manifest_of_torrent(&empty).is_none());
    }

    // ---------- layout file-set boundaries ----------

    #[test]
    fn layout_empty_and_single_file_sets() {
        let empty: HashSet<String> = HashSet::new();
        let one = path_set(&["a.mkv"]);
        assert_eq!(layout(&empty, &empty), Layout::Same);
        assert_eq!(layout(&empty, &one), Layout::Differs);
        assert_eq!(layout(&one, &empty), Layout::Differs);
        // Two single-file torrents with different names have no root to strip.
        assert_eq!(
            layout(&path_set(&["a.mkv"]), &path_set(&["b.mkv"])),
            Layout::Differs
        );
        assert_eq!(
            layout(&path_set(&["a.mkv"]), &path_set(&["a.mkv"])),
            Layout::Same
        );
    }

    #[test]
    fn layout_nested_root_differs_and_deep_mismatch() {
        // Only the outer root differs; the inner tree matches -> RootDiffers.
        assert_eq!(
            layout(
                &path_set(&["Pack/S01/a.mkv"]),
                &path_set(&["Other/S01/a.mkv"])
            ),
            Layout::RootDiffers
        );
        // Inner directory also differs -> Differs.
        assert_eq!(
            layout(&path_set(&["Pack/a/b.mkv"]), &path_set(&["Other/c/b.mkv"])),
            Layout::Differs
        );
        // Multi-file set: root differs on every entry, inner paths match.
        assert_eq!(
            layout(
                &path_set(&["Pack/a.mkv", "Pack/s/ch.srt"]),
                &path_set(&["Other/a.mkv", "Other/s/ch.srt"])
            ),
            Layout::RootDiffers
        );
    }

    #[test]
    fn layout_case_unicode_and_separator_normalization() {
        // Case-insensitive after lowercase normalization.
        assert_eq!(
            layout(&path_set(&["Root/A.MKV"]), &path_set(&["root/a.mkv"])),
            Layout::Same
        );
        // Backslash/forward-slash mixed, nested path.
        assert_eq!(
            layout(
                &path_set(&["Root\\S01\\A.MKV"]),
                &path_set(&["root/s01/a.mkv"])
            ),
            Layout::Same
        );
        // Unicode segments and spaces are preserved verbatim on both sides.
        assert_eq!(
            layout(
                &path_set(&["Pack/第 01 集.mkv"]),
                &path_set(&["Pack/第 01 集.mkv"])
            ),
            Layout::Same
        );
        // Unicode root differs -> RootDiffers.
        assert_eq!(
            layout(
                &path_set(&["字幕组/第 01 集.mkv"]),
                &path_set(&["其他组/第 01 集.mkv"])
            ),
            Layout::RootDiffers
        );
    }

    // ---------- private key parsers ----------

    #[test]
    fn path_and_size_part_parsing_edges() {
        // path_part: split on the last `|`; colon in the path survives untouched.
        assert_eq!(path_part("a/b.mkv|100"), "a/b.mkv");
        assert_eq!(path_part("plain.mkv"), "plain.mkv");
        assert_eq!(path_part("a|b|100"), "a|b");
        assert_eq!(path_part("C:/dir/a.mkv|10"), "C:/dir/a.mkv");

        // size_part: only an i64 after the last `|` parses; anything else -> None.
        assert_eq!(size_part("a.mkv|100"), Some(100));
        assert_eq!(size_part("a.mkv"), None);
        assert_eq!(size_part("a.mkv|"), None);
        assert_eq!(size_part("a.mkv|abc"), None);
        assert_eq!(size_part("a.mkv|12.5"), None);
        assert_eq!(size_part("a.mkv|-5"), Some(-5));
    }
}
