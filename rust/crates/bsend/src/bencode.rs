//! 极简 bencode 读取器：只解析 .torrent 顶层结构，取 info hash / 文件清单 / 发布名 / tracker。
//! 与 Swift 版 `Sources/BoxSendKit/Util/Bencode.swift` 逐函数对齐；
//! v2 种子只算 info hash（sha256），文件清单不解析（判级时落到「名字像」，与 Swift 一致）。

use sha1::{Digest as _, Sha1};
use sha2::Sha256;

/// `<len>:<bytes>`。bencode 字节串按 UTF-8 宽松解码（站点种子偶有非 UTF-8 段）。
pub fn read_string(data: &[u8], mut i: usize) -> Option<(String, usize)> {
    let start = i;
    while i < data.len() && data[i].is_ascii_digit() {
        i += 1;
    }
    if i >= data.len() || i == start || data[i] != b':' {
        return None;
    }
    let n: usize = std::str::from_utf8(&data[start..i]).ok()?.parse().ok()?;
    let s = i + 1;
    if s.checked_add(n)? > data.len() {
        return None;
    }
    Some((String::from_utf8_lossy(&data[s..s + n]).into_owned(), s + n))
}

/// `i<int>e`
pub fn read_int(data: &[u8], i0: usize) -> Option<(i64, usize)> {
    if i0 >= data.len() || data[i0] != b'i' {
        return None;
    }
    let mut i = i0 + 1;
    while i < data.len() && data[i] != b'e' {
        i += 1;
    }
    if i >= data.len() {
        return None;
    }
    let v = std::str::from_utf8(&data[i0 + 1..i]).ok()?.parse().ok()?;
    Some((v, i + 1))
}

/// 真实种子的嵌套深度远小于这个数；超深嵌套只可能是恶意/损坏数据，用它挡住栈溢出
const MAX_DEPTH: usize = 64;

/// 递归跳过一个完整值，返回结束位置（不含）。嵌套超过 64 层返回 None。
pub fn skip_value(data: &[u8], i0: usize) -> Option<usize> {
    skip_value_depth(data, i0, 0)
}

fn skip_value_depth(data: &[u8], i0: usize, depth: usize) -> Option<usize> {
    if i0 >= data.len() || depth > MAX_DEPTH {
        return None;
    }
    match data[i0] {
        b'd' => {
            let mut i = i0 + 1;
            while i < data.len() {
                if data[i] == b'e' {
                    return Some(i + 1);
                }
                let (_, ni) = read_string(data, i)?;
                i = skip_value_depth(data, ni, depth + 1)?;
            }
            None
        }
        b'l' => {
            let mut i = i0 + 1;
            while i < data.len() {
                if data[i] == b'e' {
                    return Some(i + 1);
                }
                i = skip_value_depth(data, i, depth + 1)?;
            }
            None
        }
        b'i' => read_int(data, i0).map(|(_, e)| e),
        _ => read_string(data, i0).map(|(_, e)| e),
    }
}

/// info 值在原始数据中的区间 [start, end)
pub fn info_range(data: &[u8]) -> Option<(usize, usize)> {
    if data.is_empty() || data[0] != b'd' {
        return None;
    }
    let mut i = 1;
    while i < data.len() {
        if data[i] == b'e' {
            break;
        }
        let (key, ni) = read_string(data, i)?;
        i = ni;
        if key == "info" {
            let end = skip_value(data, i)?;
            return Some((i, end));
        }
        i = skip_value(data, i)?;
    }
    None
}

/// `<len>:<bytes>` 的原始字节（不做 UTF-8 解码；pieces 是二进制，lossy 会破坏内容）。
pub fn read_bytes(data: &[u8], mut i: usize) -> Option<(&[u8], usize)> {
    let start = i;
    while i < data.len() && data[i].is_ascii_digit() {
        i += 1;
    }
    if i >= data.len() || i == start || data[i] != b':' {
        return None;
    }
    let n: usize = std::str::from_utf8(&data[start..i]).ok()?.parse().ok()?;
    let s = i + 1;
    if s.checked_add(n)? > data.len() {
        return None;
    }
    Some((&data[s..s + n], s + n))
}

/// info 里的 `pieces`（拼接的 20 字节 piece SHA-1）。v2-only 种子没有这个键 → None。
pub fn pieces(data: &[u8]) -> Option<&[u8]> {
    let (s, e) = info_range(data)?;
    let mut i = s + 1; // 跳过 'd'
    while i < e && i < data.len() {
        if data[i] == b'e' {
            break;
        }
        let (k, ni) = read_string(data, i)?;
        i = ni;
        if k == "pieces" {
            return read_bytes(data, i).map(|(v, _)| v);
        }
        i = skip_value(data, i)?;
    }
    None
}

/// `pieces` 的 SHA-1（40 位小写 hex）——PT 站 `/api/pieces-hash` 的查询键
/// （与 SeedHound 的 `compute_pieces_hash` 一致：sha1(info["pieces"] 原始字节)）。
pub fn pieces_hash(data: &[u8]) -> Option<String> {
    let p = pieces(data)?;
    if p.is_empty() {
        return None;
    }
    let mut h = Sha1::new();
    h.update(p);
    Some(format!("{:x}", h.finalize()))
}

/// v1 info hash：info 区间原始字节的 SHA-1（40 位小写 hex）。
/// 对区间直接散列而不是重编码——重排键序会改变 hash，必须用原始字节。
pub fn info_hash_v1(data: &[u8]) -> Option<String> {
    let (s, e) = info_range(data)?;
    let mut h = Sha1::new();
    h.update(&data[s..e]);
    Some(format!("{:x}", h.finalize()))
}

/// v2 info hash（sha256，64 位小写 hex）。只有 info 里带 `file tree`（v2/hybrid）才算，
/// v1 种子算出来的 sha256 没有比对意义。
pub fn info_hash_v2(data: &[u8]) -> Option<String> {
    let (s, e) = info_range(data)?;
    if !info_has_key(data, s, e, "file tree") {
        return None;
    }
    let mut h = Sha256::new();
    h.update(&data[s..e]);
    Some(format!("{:x}", h.finalize()))
}

/// info 字典一层键扫描（在 [start,end) 区间内）
fn info_has_key(data: &[u8], start: usize, end: usize, want: &str) -> bool {
    let mut i = start + 1; // 跳过 'd'
    while i < end && i < data.len() {
        if data[i] == b'e' {
            break;
        }
        match read_string(data, i) {
            Some((k, ni)) => {
                if k == want {
                    return true;
                }
                match skip_value(data, ni) {
                    Some(ni2) => i = ni2,
                    None => break,
                }
            }
            None => break,
        }
    }
    false
}

/// info.name（发布名的权威来源）
pub fn info_name(data: &[u8]) -> Option<String> {
    let (s, e) = info_range(data)?;
    let mut i = s + 1;
    while i < e && i < data.len() {
        if data[i] == b'e' {
            break;
        }
        let (k, ni) = read_string(data, i)?;
        i = ni;
        if k == "name" {
            return read_string(data, i).map(|(v, _)| v);
        }
        i = skip_value(data, i)?;
    }
    None
}

/// info 内的文件清单。多文件：根目录名补成路径首段（`root/rel/path`）；单文件：路径 = info.name。
/// v2 的 file tree 不解析（与 Swift 版一致），解析不出返回 None。
pub fn file_list(data: &[u8]) -> Option<Vec<(String, i64)>> {
    let (s, e) = info_range(data)?;
    let mut name: Option<String> = None;
    let mut single: Option<i64> = None;
    let mut multi: Option<Vec<(String, i64)>> = None;
    let mut i = s + 1;
    while i < e && i < data.len() {
        if data[i] == b'e' {
            break;
        }
        let (key, ni) = read_string(data, i)?;
        i = ni;
        match key.as_str() {
            "name" => {
                if let Some((v, ni2)) = read_string(data, i) {
                    name = Some(v);
                    i = ni2;
                } else {
                    i = skip_value(data, i)?;
                }
            }
            "length" => {
                if let Some((v, ni2)) = read_int(data, i) {
                    single = Some(v);
                    i = ni2;
                } else {
                    i = skip_value(data, i)?;
                }
            }
            "files" => {
                let (list, ni2) = read_files(data, i)?;
                multi = Some(list);
                i = ni2;
            }
            _ => i = skip_value(data, i)?,
        }
    }
    if let Some(m) = multi {
        if !m.is_empty() {
            let root = name?;
            return Some(
                m.into_iter()
                    .map(|(p, sz)| (format!("{}/{}", root, p), sz))
                    .collect(),
            );
        }
    }
    if let (Some(n), Some(l)) = (name, single) {
        return Some(vec![(n, l)]);
    }
    None
}

/// files = [ {length, path: [seg,…]}, … ]。unpaired 编码的 path 段可能是
/// `[utf8, 原字节]` 双元素列表：取首项当名字、整段跳过（只跳一个元素会把内层 'e' 错当 path 结尾）。
fn read_files(data: &[u8], i0: usize) -> Option<(Vec<(String, i64)>, usize)> {
    if i0 >= data.len() || data[i0] != b'l' {
        return None;
    }
    let mut i = i0 + 1;
    let mut out = Vec::new();
    while i < data.len() && data[i] != b'e' {
        if data[i] != b'd' {
            return None;
        }
        let mut j = i + 1;
        let mut size: Option<i64> = None;
        let mut parts: Vec<String> = Vec::new();
        while j < data.len() && data[j] != b'e' {
            let (k, nj) = read_string(data, j)?;
            j = nj;
            match k.as_str() {
                "length" => {
                    let (v, nj2) = read_int(data, j)?;
                    size = Some(v);
                    j = nj2;
                }
                "path" => {
                    let mut segs: Vec<String> = Vec::new();
                    let mut m = j + 1;
                    while m < data.len() && data[m] != b'e' {
                        if data[m] == b'l' {
                            // unpaired 段：[utf8, raw]
                            let (first, _) = read_string(data, m + 1)?;
                            segs.push(first);
                            m = skip_value(data, m)?;
                        } else {
                            let (one, nm) = read_string(data, m)?;
                            segs.push(one);
                            m = nm;
                        }
                    }
                    if m >= data.len() {
                        return None;
                    }
                    parts = segs;
                    j = m + 1;
                }
                _ => j = skip_value(data, j)?,
            }
        }
        if j >= data.len() {
            return None;
        }
        if parts.is_empty() {
            return None;
        }
        out.push((parts.join("/"), size?));
        i = j + 1;
    }
    if i >= data.len() {
        return None;
    }
    Some((out, i + 1))
}

/// 种子总数据量。单文件取 `length`（太小的值多半不是文件长度，忽略），多文件取 `files` 之和；
/// 个别站的 v2 变体用 `lengths` 列表。
pub fn total_length(data: &[u8]) -> Option<i64> {
    let (s, e) = info_range(data)?;
    let mut i = s + 1;
    let mut total: Option<i64> = None;
    while i < e && i < data.len() {
        if data[i] == b'e' {
            break;
        }
        let (key, ni) = read_string(data, i)?;
        i = ni;
        match key.as_str() {
            "length" => {
                let (v, ni2) = read_int(data, i)?;
                if v > 65536 {
                    total = Some(v);
                }
                i = ni2;
            }
            "lengths" => {
                let (list, ni2) = read_int_list(data, i)?;
                let sum: i64 = list.iter().sum();
                if sum > 0 {
                    total = Some(sum);
                }
                i = ni2;
            }
            "files" => {
                let (sum, ni2) = files_total(data, i)?;
                if sum > 0 {
                    total = Some(sum);
                }
                i = ni2;
            }
            _ => i = skip_value(data, i)?,
        }
    }
    total
}

fn files_total(data: &[u8], i0: usize) -> Option<(i64, usize)> {
    if i0 >= data.len() || data[i0] != b'l' {
        return None;
    }
    let mut i = i0 + 1;
    let mut sum: i64 = 0;
    while i < data.len() && data[i] != b'e' {
        if data[i] != b'd' {
            return None;
        }
        let mut j = i + 1;
        while j < data.len() && data[j] != b'e' {
            let (k, nj) = read_string(data, j)?;
            j = nj;
            if k == "length" {
                let (v, nj2) = read_int(data, j)?;
                sum += v;
                j = nj2;
            } else {
                j = skip_value(data, j)?;
            }
        }
        if j >= data.len() {
            return None;
        }
        i = j + 1;
    }
    if i >= data.len() {
        return None;
    }
    Some((sum, i + 1))
}

fn read_int_list(data: &[u8], i0: usize) -> Option<(Vec<i64>, usize)> {
    if i0 >= data.len() || data[i0] != b'l' {
        return None;
    }
    let mut i = i0 + 1;
    let mut out = Vec::new();
    while i < data.len() && data[i] != b'e' {
        let (v, ni) = read_int(data, i)?;
        out.push(v);
        i = ni;
    }
    if i >= data.len() {
        return None;
    }
    Some((out, i + 1))
}

/// 顶层 announce / announce-list 里的 tracker URL（按出现顺序去重）。
/// 同 hash 种子已在下载器时，用这些 URL 给它补挂本站 tracker。
pub fn announce_urls(data: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: &str| {
        let t = s.trim();
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    };
    if data.is_empty() || data[0] != b'd' {
        return out;
    }
    let mut i = 1;
    while i < data.len() {
        if data[i] == b'e' {
            break;
        }
        let Some((key, ni)) = read_string(data, i) else {
            return out;
        };
        i = ni;
        if key == "announce" {
            if let Some((v, ni2)) = read_string(data, i) {
                add(&v);
                i = ni2;
            } else {
                return out;
            }
        } else if key == "announce-list" && i < data.len() && data[i] == b'l' {
            let mut j = i + 1;
            while j < data.len() && data[j] != b'e' {
                if data[j] == b'l' {
                    let mut k = j + 1;
                    while k < data.len() && data[k] != b'e' {
                        if let Some((v, nk)) = read_string(data, k) {
                            add(&v);
                            k = nk;
                        } else {
                            break;
                        }
                    }
                    j = if k < data.len() { k + 1 } else { data.len() };
                } else if let Some(nj) = skip_value(data, j) {
                    j = nj;
                } else {
                    break;
                }
            }
            i = if j < data.len() { j + 1 } else { data.len() };
        } else if let Some(ni2) = skip_value(data, i) {
            i = ni2;
        } else {
            return out;
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 测试用 bencode 编码器（只服务测试，不进生产路径）
    pub(crate) fn enc(v: &BVal) -> Vec<u8> {
        let mut out = Vec::new();
        enc_into(v, &mut out);
        out
    }
    pub(crate) enum BVal {
        S(String),
        I(i64),
        L(Vec<BVal>),
        D(Vec<(String, BVal)>),
    }
    impl BVal {
        pub(crate) fn s(v: &str) -> BVal {
            BVal::S(v.to_string())
        }
        /// 字典构造辅助：键用字面量写，内部转 String
        pub(crate) fn d(pairs: Vec<(&str, BVal)>) -> BVal {
            BVal::D(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        }
    }
    fn enc_into(v: &BVal, out: &mut Vec<u8>) {
        match v {
            BVal::S(s) => {
                out.extend_from_slice(format!("{}:", s.len()).as_bytes());
                out.extend_from_slice(s.as_bytes());
            }
            BVal::I(n) => out.extend_from_slice(format!("i{}e", n).as_bytes()),
            BVal::L(items) => {
                out.push(b'l');
                for it in items {
                    enc_into(it, out);
                }
                out.push(b'e');
            }
            BVal::D(pairs) => {
                out.push(b'd');
                for (k, val) in pairs {
                    out.extend_from_slice(format!("{}:", k.len()).as_bytes());
                    out.extend_from_slice(k.as_bytes());
                    enc_into(val, out);
                }
                out.push(b'e');
            }
        }
    }

    fn torrent_single() -> Vec<u8> {
        enc(&BVal::d(vec![
            ("announce", BVal::s("https://tracker.example.com/announce")),
            (
                "info",
                BVal::d(vec![
                    ("length", BVal::I(700_000_000)),
                    ("name", BVal::s("Some.Movie.2020.1080p.BluRay.x264-GRP")),
                    ("piece length", BVal::I(262144)),
                    ("pieces", BVal::s("aaaaaaaaaaaaaaaaaaaa")),
                ]),
            ),
        ]))
    }

    fn torrent_multi() -> Vec<u8> {
        enc(&BVal::d(vec![
            ("announce", BVal::s("https://a.example.com/announce")),
            (
                "announce-list",
                BVal::L(vec![
                    BVal::L(vec![BVal::s("https://a.example.com/announce")]),
                    BVal::L(vec![BVal::s("https://b.example.com/announce")]),
                ]),
            ),
            (
                "info",
                BVal::d(vec![
                    (
                        "files",
                        BVal::L(vec![
                            BVal::d(vec![
                                ("length", BVal::I(100)),
                                ("path", BVal::L(vec![BVal::s("video.mkv")])),
                            ]),
                            BVal::d(vec![
                                ("length", BVal::I(50)),
                                ("path", BVal::L(vec![BVal::s("subs"), BVal::s("ch.srt")])),
                            ]),
                        ]),
                    ),
                    ("name", BVal::s("Show.S01.1080p")),
                    ("piece length", BVal::I(262144)),
                    ("pieces", BVal::s("bbbbbbbbbbbbbbbbbbbb")),
                ]),
            ),
        ]))
    }

    #[test]
    fn single_file_torrent() {
        let d = torrent_single();
        assert_eq!(
            info_name(&d).unwrap(),
            "Some.Movie.2020.1080p.BluRay.x264-GRP"
        );
        assert_eq!(
            file_list(&d).unwrap(),
            vec![(
                "Some.Movie.2020.1080p.BluRay.x264-GRP".to_string(),
                700_000_000
            )]
        );
        assert_eq!(total_length(&d), Some(700_000_000));
        assert_eq!(
            announce_urls(&d),
            vec!["https://tracker.example.com/announce"]
        );
        let h = info_hash_v1(&d).unwrap();
        assert_eq!(h.len(), 40);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        // v1 种子没有 file tree，不算 v2 hash
        assert!(info_hash_v2(&d).is_none());
        // 稳定性：同数据同 hash
        assert_eq!(info_hash_v1(&d).unwrap(), h);
    }

    #[test]
    fn multi_file_torrent_root_prefix() {
        let d = torrent_multi();
        assert_eq!(info_name(&d).unwrap(), "Show.S01.1080p");
        assert_eq!(
            file_list(&d).unwrap(),
            vec![
                ("Show.S01.1080p/video.mkv".to_string(), 100),
                ("Show.S01.1080p/subs/ch.srt".to_string(), 50),
            ]
        );
        assert_eq!(total_length(&d), Some(150));
        assert_eq!(
            announce_urls(&d),
            vec![
                "https://a.example.com/announce",
                "https://b.example.com/announce",
            ]
        );
    }

    #[test]
    fn junk_is_rejected() {
        assert!(info_range(b"not bencode").is_none());
        assert!(info_hash_v1(b"d3:fooi1ee").is_none()); // 无 info
        let truncated = &torrent_multi()[..40];
        assert!(file_list(truncated).is_none());
    }

    #[test]
    fn deep_nesting_is_rejected() {
        // 恶意/损坏数据：超深嵌套列表直接拒绝，不会栈溢出也不会误判
        let mut data = vec![b'l'; 300];
        data.extend_from_slice(b"1:a");
        data.extend_from_slice(&vec![b'e'; 300]);
        assert!(skip_value(&data, 0).is_none());
        // 正常深度照常解析
        let ok = enc(&BVal::d(vec![(
            "info",
            BVal::d(vec![
                (
                    "files",
                    BVal::L(vec![BVal::d(vec![("length", BVal::I(1))])]),
                ),
                ("name", BVal::s("x")),
            ]),
        )]));
        assert!(skip_value(&ok, 0).is_some());
    }

    #[test]
    fn v2_hash_only_with_file_tree() {
        let d = enc(&BVal::d(vec![(
            "info",
            BVal::d(vec![
                ("file tree", BVal::d(vec![])),
                ("length", BVal::I(1000)),
                ("name", BVal::s("v2torrent")),
            ]),
        )]));
        let h2 = info_hash_v2(&d).unwrap();
        assert_eq!(h2.len(), 64);
        assert!(info_hash_v1(&d).is_some()); // hybrid 仍有 v1 结构可算
    }

    #[test]
    fn pieces_hash_matches_golden() {
        // pieces = 拼接的 20 字节 SHA-1；夹具用 ASCII 字节，golden 由独立实现（hashlib）给出
        let t = enc(&BVal::d(vec![
            ("announce", BVal::s("http://tracker.example/announce")),
            (
                "info",
                BVal::d(vec![
                    ("name", BVal::s("Pack")),
                    ("piece length", BVal::I(262144)),
                    ("pieces", BVal::s("0123456789abcdefghij")),
                ]),
            ),
        ]));
        assert_eq!(pieces(&t).map(|p| p.len()), Some(20));
        assert_eq!(
            pieces_hash(&t).as_deref(),
            Some("7c8e1dc5a4fd22f1311a7a1f3e3401215c0ccab3")
        );
        // v2-only 形态没有 pieces → None（调用方回退名字检索）
        let v2 = enc(&BVal::d(vec![(
            "info",
            BVal::d(vec![
                ("file tree", BVal::d(vec![])),
                ("name", BVal::s("Pack")),
            ]),
        )]));
        assert!(pieces_hash(&v2).is_none());
    }

    #[test]
    fn malformed_lengths_and_empty_file_list_are_rejected() {
        // 长度超过剩余字节 / 缺长度 / 缺结束符，都必须 None 而不是 panic
        assert!(read_string(b"5:abc", 0).is_none());
        assert!(read_string(b":abc", 0).is_none());
        assert!(read_string(b"0:", 0).is_some());
        assert!(read_int(b"i12", 0).is_none());
        assert!(read_int(b"12e", 0).is_none());
        assert_eq!(read_int(b"i-5e", 0), Some((-5, 4)));

        // 空 files 清单不能误判成单文件种（没有 length）
        let d = enc(&BVal::d(vec![(
            "info",
            BVal::d(vec![
                ("files", BVal::L(vec![])),
                ("name", BVal::s("empty-pack")),
            ]),
        )]));
        assert!(file_list(&d).is_none(), "空 files 清单应返回 None");
        assert!(total_length(&d).is_none(), "空清单总长度不可信");
    }
}
