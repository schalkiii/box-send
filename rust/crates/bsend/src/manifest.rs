//! 内容指纹：逐文件清单的归一化与摘要。
//! 与 Swift 版 `AssistMatcher.comparableKey` / `AssistPipeline.digest` 对齐。

use std::collections::BTreeSet;

use crate::models::TorrentFingerprint;

/// 归一化单条路径：`\` 换 `/`、去根目录名（多段时）、去 `.` 段、小写。
/// 站点重新打包常改最外层目录名——那是路径问题不是内容问题，比内容前先剥掉。
/// （分隔符必须先统一：`Root\a\b.mkv` 不先换杠的话整条是一个「段」，根目录剥不掉）
pub fn normalized_path(path: &str, root: &str) -> String {
    let unified = path.replace('\\', "/");
    let mut parts: Vec<String> = unified.split('/').map(|s| s.to_string()).collect();
    if parts.is_empty() {
        parts.push(unified);
    }
    if parts.len() > 1 && parts[0] == root {
        parts.remove(0);
    }
    parts.retain(|p| p != ".");
    parts.join("/").to_lowercase()
}

/// 可比对的清单键集合：每条 `"归一化路径|字节数"`。集合比较，站点条目顺序不可信。
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub root: String,
    pub total_size: i64,
    pub keys: BTreeSet<String>,
}

impl Manifest {
    pub fn count(&self) -> usize {
        self.keys.len()
    }
}

/// 本地/站点指纹都过这道归一化
pub fn comparable_key(fp: &TorrentFingerprint) -> Manifest {
    let keys = fp
        .files
        .iter()
        .map(|(p, sz)| format!("{}|{}", normalized_path(p, &fp.root), sz))
        .collect();
    Manifest {
        root: fp.root.clone(),
        total_size: fp.total_size,
        keys,
    }
}

/// FNV-1a 64：跨进程稳定（不能拿 std 的 Hasher 当身份——每次运行换种子）
pub fn fnv1a64(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// 内容摘要：排序后的逐文件键依次喂 FNV-1a（键间加 0x1f 分隔）。
/// 同一段文件无论各站怎么重新打包，摘要一致；不同文件清单摘要不同。
pub fn manifest_digest(m: &Manifest) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for k in &m.keys {
        h = fnv1a64(h, k.as_bytes());
        h = fnv1a64(h, &[0x1f]);
    }
    format!("{:016x}", h)
}

/// 一组文件路径共有的最外层目录（下载器只报文件路径时反推根目录名）。
/// 路径都在同一层时没有共同根，返回空串。
pub fn common_root(files: &[(String, i64)]) -> String {
    let Some(first) = files.first() else {
        return String::new();
    };
    let Some(head) = first.0.split('/').next() else {
        return String::new();
    };
    let pre = format!("{}/", head);
    if files.iter().all(|(p, _)| p.starts_with(&pre)) {
        head.to_string()
    } else {
        String::new()
    }
}

/// 人类可读的大小
pub fn size_text(bytes: i64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 1.0 {
        format!("{:.2} GB", gb)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(root: &str, files: &[(&str, i64)]) -> TorrentFingerprint {
        TorrentFingerprint::new(
            root.to_string(),
            files.iter().map(|(p, s)| (p.to_string(), *s)).collect(),
        )
    }

    #[test]
    fn normalized_path_strips_root_and_dots() {
        assert_eq!(normalized_path("Root/a/b.mkv", "Root"), "a/b.mkv");
        assert_eq!(normalized_path("Root/./a.mkv", "Root"), "a.mkv");
        assert_eq!(normalized_path("Root\\a\\b.mkv", "Root"), "a/b.mkv");
        assert_eq!(
            normalized_path("singlefile.mkv", "whatever"),
            "singlefile.mkv"
        );
        // 单段路径不去首段（单文件种子的路径就是文件名本身）
        assert_eq!(normalized_path("Root", "Root"), "root");
    }

    #[test]
    fn same_content_same_digest_across_roots() {
        // 同一段文件，站点改了根目录名、加了点路径段：摘要必须一致
        let a = comparable_key(&fp(
            "MyRelease",
            &[("MyRelease/a.mkv", 100), ("MyRelease/s/ch.srt", 5)],
        ));
        let b = comparable_key(&fp(
            "SitePack",
            &[("SitePack/./a.mkv", 100), ("SitePack\\s\\ch.srt", 5)],
        ));
        assert_eq!(a.keys, b.keys);
        assert_eq!(manifest_digest(&a), manifest_digest(&b));
    }

    #[test]
    fn different_content_different_digest() {
        let a = comparable_key(&fp("R", &[("R/a.mkv", 100)]));
        let b = comparable_key(&fp("R", &[("R/a.mkv", 101)]));
        let c = comparable_key(&fp("R", &[("R/a.mkv", 100), ("R/extra.nfo", 1)]));
        assert_ne!(manifest_digest(&a), manifest_digest(&b));
        assert_ne!(manifest_digest(&a), manifest_digest(&c));
    }

    #[test]
    fn common_root_detection() {
        assert_eq!(common_root(&[("R/a".into(), 1), ("R/b".into(), 1)]), "R");
        assert_eq!(common_root(&[("a".into(), 1), ("b".into(), 1)]), "");
        assert_eq!(common_root(&[]), "");
    }
}
