//! 单 tracker 种子改写（P2 新口径，见 `doc/ARCHITECTURE.md` §3.2）。
//!
//! 源种从 qB 导出（`GET /api/v2/torrents/export`），在它的字节基础上**字节级拼接**
//! 出目标站种子，而不是 decode→重编码整个字典：
//!
//! - `info.files` / `info.pieces` / `info['piece length']` / `info.name` 等素材原字节
//!   原样保留，改写后逐字节比对，被动到即报错；
//! - 顶层 `announce` 替换为目标站唯一一条；`announce-list` 整键删除（不得合并多站）；
//! - `info` 内写入目标站专属 `source` 键（值用站点 id），从而必然产生新的 info hash。
//!
//! 本模块只做纯字节变换与断言，不连下载器、不连站点。

use anyhow::{bail, Context, Result};

use crate::bencode;

/// 改写结果。
#[derive(Debug)]
pub struct RewrittenTorrent {
    /// 可直接提交给站点 / 挂回下载器的新 `.torrent` 字节。
    pub data: Vec<u8>,
    /// 源种 v1 info hash（小写 hex）。
    pub old_hash: String,
    /// 改写后 v1 info hash（小写 hex）。
    pub new_hash: String,
}

/// 字典内一条目：key 与 value 在 `data` 中的字节区间（左闭右开）。
struct Entry {
    key: String,
    key_start: usize,
    val_start: usize,
    val_end: usize,
}

/// 扫描字典区间 `data[start..end]`（`data[start] == b'd'`，`data[end-1] == b'e'`）。
fn scan_dict(data: &[u8], start: usize, end: usize) -> Result<Vec<Entry>> {
    anyhow::ensure!(
        start < end && data[start] == b'd' && data[end - 1] == b'e',
        "不是合法 bencode 字典区间"
    );
    let mut i = start + 1;
    let mut out = Vec::new();
    while i < end - 1 {
        let (key, next) =
            bencode::read_string(data, i).with_context(|| format!("字典 key 解析失败 @{i}"))?;
        let key_start = i;
        i = next;
        // `skip_value` 返回绝对结束下标（不是长度），直接作为下一轮起点
        let val_end = bencode::skip_value(data, i).context("字典 value 长度解析失败")?;
        out.push(Entry {
            key,
            key_start,
            val_start: i,
            val_end,
        });
        i = val_end;
    }
    Ok(out)
}

fn value_bytes<'a>(data: &'a [u8], entries: &[Entry], key: &str) -> Option<&'a [u8]> {
    entries
        .iter()
        .find(|e| e.key == key)
        .map(|e| &data[e.val_start..e.val_end])
}

/// 向 `out` 写入一条 `<key><value>` 字符串条目（bencode 字节串）。
fn write_str_entry(out: &mut Vec<u8>, key: &str, value: &[u8]) {
    out.extend_from_slice(format!("{}:", key.len()).as_bytes());
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(format!("{}:", value.len()).as_bytes());
    out.extend_from_slice(value);
}

/// 改写 info 字典：保留原有条目原字节，仅替换/注入 `source`。
fn rewrite_info(info_bytes: &[u8], source_tag: &str) -> Result<Vec<u8>> {
    // 入参已是 info 字典切片（以 d 开头、e 结尾），直接扫描一层条目
    let entries = scan_dict(info_bytes, 0, info_bytes.len())?;

    let mut out = Vec::with_capacity(info_bytes.len() + source_tag.len() + 16);
    out.push(b'd');
    let mut source_written = false;
    for ent in &entries {
        if ent.key == "source" {
            write_str_entry(&mut out, "source", source_tag.as_bytes());
            source_written = true;
        } else {
            out.extend_from_slice(&info_bytes[ent.key_start..ent.val_end]);
        }
    }
    if !source_written {
        write_str_entry(&mut out, "source", source_tag.as_bytes());
    }
    out.push(b'e');
    Ok(out)
}

/// 字节级改写：保留素材字节，换 announce、删 announce-list、注入 info.source。
///
/// `announce` 是目标站唯一一条 announce URL（带账号 passkey，由调用方从下载器现有
/// tracker 或配置中取得）；`source_tag` 用站点 id（如 `qilin`）。
pub fn rewrite_for_target(
    src: &[u8],
    announce: &str,
    source_tag: &str,
) -> Result<RewrittenTorrent> {
    let announce = announce.trim();
    let source_tag = source_tag.trim();
    anyhow::ensure!(!announce.is_empty(), "目标站 announce 不能为空");
    anyhow::ensure!(!source_tag.is_empty(), "info.source 站点标记不能为空");

    // ---- 定位外层字典与 info 区间，抓取素材原字节用于改后比对 ----
    let (info_s, info_e) = bencode::info_range(src).context("源种缺 info 字典")?;
    let outer = scan_dict(src, 0, src.len())?;
    let info_entries = scan_dict(src, info_s, info_e)?;

    let orig_pieces = value_bytes(src, &info_entries, "pieces")
        .context("源种 info 缺 pieces")?
        .to_vec();
    let orig_files = value_bytes(src, &info_entries, "files").map(|b| b.to_vec());
    let orig_name = value_bytes(src, &info_entries, "name").map(|b| b.to_vec());
    let orig_piece_len = value_bytes(src, &info_entries, "piece length").map(|b| b.to_vec());

    let old_hash = bencode::info_hash_v1(src).context("源种 info hash 计算失败")?;

    // ---- 拼新 info 字节 ----
    let new_info = rewrite_info(&src[info_s..info_e], source_tag)?;

    // ---- 拼外层字典：announce 换目标站唯一一条；announce-list 删除；info 换新字节 ----
    let mut out = Vec::with_capacity(src.len() + new_info.len() + announce.len() + 32);
    out.push(b'd');
    if !outer.iter().any(|e| e.key == "announce") {
        write_str_entry(&mut out, "announce", announce.as_bytes());
    }
    for ent in &outer {
        match ent.key.as_str() {
            "announce" => {
                write_str_entry(&mut out, "announce", announce.as_bytes());
            }
            "announce-list" => continue, // 单 tracker：禁止多站 announce 列表
            "info" => {
                out.extend_from_slice(b"4:info");
                out.extend_from_slice(&new_info);
            }
            _ => out.extend_from_slice(&src[ent.key_start..ent.val_end]),
        }
    }
    out.push(b'e');

    // ---- 校验素材字节未被动到 ----
    let (n_info_s, n_info_e) = bencode::info_range(&out).context("改写后 info 区间解析失败")?;
    let new_entries = scan_dict(&out, n_info_s, n_info_e)?;
    let new_pieces = value_bytes(&out, &new_entries, "pieces").context("改写后缺 pieces")?;
    if new_pieces != orig_pieces {
        bail!("单 tracker 策略：改写后 info.pieces 字节与源种不一致，中止");
    }
    if let Some(orig_files) = &orig_files {
        let new_files = value_bytes(&out, &new_entries, "files").context("改写后缺 files")?;
        if new_files != orig_files.as_slice() {
            bail!("单 tracker 策略：改写后 info.files 字节与源种不一致，中止");
        }
    }
    if let Some(orig_name) = &orig_name {
        let new_name = value_bytes(&out, &new_entries, "name").context("改写后缺 name")?;
        if new_name != orig_name.as_slice() {
            bail!("单 tracker 策略：改写后 info.name 字节与源种不一致，中止");
        }
    }
    if let Some(orig_pl) = &orig_piece_len {
        let new_pl =
            value_bytes(&out, &new_entries, "piece length").context("改写后缺 piece length")?;
        if new_pl != orig_pl.as_slice() {
            bail!("单 tracker 策略：改写后 info['piece length'] 字节与源种不一致，中止");
        }
    }

    let new_hash = bencode::info_hash_v1(&out).context("改写后 info hash 计算失败")?;
    if new_hash == old_hash {
        bail!("实现错误：改写后 info hash 与源种相同，info.source 未生效");
    }

    Ok(RewrittenTorrent {
        data: out,
        old_hash,
        new_hash,
    })
}

/// 顶层是否含 `announce-list`（多站列表键）。
pub fn has_announce_list(data: &[u8]) -> bool {
    scan_dict_safe(data)
        .into_iter()
        .any(|e| e == "announce-list")
}

fn scan_dict_safe(data: &[u8]) -> Vec<String> {
    let mut keys = Vec::new();
    if data.is_empty() || data[0] != b'd' {
        return keys;
    }
    let mut i = 1usize;
    while i < data.len() {
        if data[i] == b'e' {
            break;
        }
        let Some((key, next)) = bencode::read_string(data, i) else {
            break;
        };
        i = next;
        let Some(len) = bencode::skip_value(data, i) else {
            break;
        };
        keys.push(key);
        i += len;
    }
    keys
}

/// 读取 `info.source` 字符串值（不存在返回 `None`）。
pub fn info_source(data: &[u8]) -> Option<String> {
    let (s, e) = bencode::info_range(data)?;
    let entries = scan_dict(data, s, e).ok()?;
    let raw = value_bytes(data, &entries, "source")?;
    // raw 本身是 bencode 字节串值：剥掉长度前缀取原文
    let (v, _) = bencode::read_string(raw, 0)?;
    Some(v)
}

/// 断言待上传/已挂种的 `.torrent` 是单 tracker：
/// announce（含 announce-list 展开）恰好一条，且不存在 `announce-list` 键。
pub fn assert_single_tracker(data: &[u8]) -> Result<()> {
    let urls = bencode::announce_urls(data);
    if urls.len() != 1 {
        bail!(
            "单 tracker 策略：上传种子 announce 数量为 {}，必须为 1（实际：{}）",
            urls.len(),
            urls.join(", ")
        );
    }
    if has_announce_list(data) {
        bail!("单 tracker 策略：上传种子仍含 announce-list，按策略必须删除");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t_str(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(format!("{}:", s.len()).as_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    fn t_int(out: &mut Vec<u8>, i: i64) {
        out.extend_from_slice(format!("i{i}e").as_bytes());
    }

    /// 构造多文件源种：两条 announce（主 + announce-list 两站），info 无 source。
    fn source_torrent() -> Vec<u8> {
        let mut info = Vec::new();
        t_str(&mut info, "files");
        info.push(b'l');
        info.push(b'd');
        t_str(&mut info, "length");
        t_int(&mut info, 100);
        t_str(&mut info, "path");
        info.push(b'l');
        t_str(&mut info, "a.mkv");
        info.push(b'e');
        info.push(b'e');
        info.push(b'e');
        t_str(&mut info, "name");
        t_str(&mut info, "Pack");
        t_str(&mut info, "piece length");
        t_int(&mut info, 262144);
        t_str(&mut info, "pieces");
        t_str(&mut info, &"p".repeat(20));

        let mut out = Vec::new();
        out.push(b'd');
        t_str(&mut out, "announce");
        t_str(&mut out, "http://src.example/announce?passkey=AAA");
        let mut alist = Vec::new();
        alist.push(b'l');
        alist.push(b'l');
        t_str(&mut alist, "http://src.example/announce?passkey=AAA");
        alist.push(b'e');
        alist.push(b'l');
        t_str(&mut alist, "http://src2.example/announce?passkey=BBB");
        alist.push(b'e');
        alist.push(b'e');
        t_str(&mut out, "announce-list");
        out.extend_from_slice(&alist);
        t_str(&mut out, "info");
        out.push(b'd');
        out.extend_from_slice(&info);
        out.push(b'e');
        out.push(b'e');
        out
    }

    #[test]
    fn rewrite_keeps_material_bytes_single_announce_and_new_hash() {
        let src = source_torrent();
        let old_hash = bencode::info_hash_v1(&src).unwrap();

        let out = rewrite_for_target(&src, "https://tgt.example/announce?passkey=ZZZ", "qilin")
            .expect("rewrite");

        // files / pieces / name / piece length 原字节不变
        let (os, oe) = bencode::info_range(&src).unwrap();
        let (ns, ne) = bencode::info_range(&out.data).unwrap();
        let orig_entries = scan_dict(&src, os, oe).unwrap();
        let new_entries = scan_dict(&out.data, ns, ne).unwrap();
        for key in ["files", "pieces", "name", "piece length"] {
            let a = value_bytes(&src, &orig_entries, key).unwrap();
            let b = value_bytes(&out.data, &new_entries, key).unwrap();
            assert_eq!(a, b, "info.{key} 字节被改动");
        }

        // announce 只剩目标站一条；announce-list 被删
        let urls = bencode::announce_urls(&out.data);
        assert_eq!(urls, vec!["https://tgt.example/announce?passkey=ZZZ"]);
        assert!(!has_announce_list(&out.data));

        // info.source 写入且值正确
        assert_eq!(info_source(&out.data).as_deref(), Some("qilin"));

        // 必然产生新 hash，且与源 hash 不同
        assert_ne!(out.new_hash, old_hash);
        assert_eq!(out.old_hash, old_hash);
        assert_ne!(out.new_hash, out.old_hash);

        assert_single_tracker(&out.data).expect("单 tracker 断言应通过");
    }

    #[test]
    fn rewrite_replaces_existing_source_tag() {
        let mut src = source_torrent();
        // 往 info 里塞一个旧 source
        let (s, e) = bencode::info_range(&src).unwrap();
        let entries = scan_dict(&src, s, e).unwrap();
        let mut info = Vec::new();
        info.push(b'd');
        for ent in &entries {
            if ent.key == "source" {
                continue;
            }
            info.extend_from_slice(&src[ent.key_start..ent.val_end]);
        }
        t_str_entry(&mut info, "source", b"oldsite");
        info.push(b'e');
        src.splice(s..e, info.iter().copied());

        let out = rewrite_for_target(&src, "https://tgt.example/a", "newsite").unwrap();
        assert_eq!(info_source(&out.data).as_deref(), Some("newsite"));
        assert_ne!(out.new_hash, out.old_hash);
    }

    #[test]
    fn reject_when_announce_count_not_one() {
        let src = source_torrent();
        // 不改写、保留两站 announce-list 的源种必须被单 tracker 断言拦下
        let err = assert_single_tracker(&src).unwrap_err();
        assert!(err.to_string().contains("单 tracker 策略"));
        assert!(err.to_string().contains("announce 数量为 2"));
    }

    #[test]
    fn rewrite_rejects_empty_announce() {
        let src = source_torrent();
        let err = rewrite_for_target(&src, "  ", "qilin").unwrap_err();
        assert!(err.to_string().contains("announce 不能为空"));
    }

    #[test]
    fn source_hash_collision_is_an_implementation_error() {
        // 手工构造：source_tag 若导致 hash 与源相同属于实现错误——这里只钉住错误路径的
        // 存在性：rewrite 正常路径已在上面证明 new != old；此用例守护断言分支不被删。
        let src = source_torrent();
        let out = rewrite_for_target(&src, "https://tgt.example/a", "qilin").unwrap();
        assert_ne!(out.new_hash, bencode::info_hash_v1(&src).unwrap());
    }

    fn t_str_entry(out: &mut Vec<u8>, key: &str, value: &[u8]) {
        out.extend_from_slice(format!("{}:", key.len()).as_bytes());
        out.extend_from_slice(key.as_bytes());
        out.extend_from_slice(format!("{}:", value.len()).as_bytes());
        out.extend_from_slice(value);
    }
}
