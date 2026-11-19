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
    fn level_c_name_only_with_reasons() {
        let st = site_torrent(&[("a.mkv", 100), ("s/ch.srt", 5)], "Pack");
        let local = local_fp(&[("Pack/a.mkv", 100)]);
        let (level, why) = judge(&["deadbeef".to_string()], &local, &st, "Pack");
        match level {
            JudgeLevel::NameOnly { site_name } => assert_eq!(site_name, "Pack"),
            other => panic!("expected NameOnly, got {:?}", other),
        }
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
}
