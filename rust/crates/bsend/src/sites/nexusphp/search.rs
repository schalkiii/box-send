//! NexusPHP 查询词与名称判定：查询词逐级放宽、名称互含、归一化。
//! 从 `nexusphp.rs` 拆出，行为不变。

use std::collections::HashSet;

use super::{decode_entities, strip_tags};

// ---------- 查询词阶梯 ----------

/// 候选 (查询词, 匹配基准)：全名 → 去组标签名 → 去标点词元查询 → 前 32 字截短 → 副标题（中文名）。
/// 已有种子的组标签/细节/语言可能不同，逐级放宽；严格模式只取前三条（命中要求都高，多查白等）。
pub fn query_pairs(name: &str, subtitle: &str, relaxed: bool) -> Vec<(String, String)> {
    let mut pairs = vec![(name.to_string(), name.to_string())];
    // 名字末段的组标签（-GROUP）：位置在 10 字符之后才剥，避免把标题里正常的连字符段当组标签
    if let Some(dash) = name.rfind('-') {
        if name[..dash].chars().count() > 10 {
            pairs.push((name[..dash].to_string(), name.to_string()));
        }
    }
    // 站点检索多是按词匹配，标题里的点/连字符会把一个词劈成两半：站内把 Blu-ray 写成
    // BluRay 时整名查询一条也查不到。补一条「去标点、只取前几个词」的查询词，
    // 命中与否仍由后面的名称匹配判，不会因为查询变宽就认错种子。
    if let Some(soft) = word_query(name, 6, 3, 60) {
        pairs.push((soft, name.to_string()));
    }
    if name.chars().count() > 32 {
        pairs.push((name.chars().take(32).collect(), name.to_string()));
    }
    // 副标题截到第一个括号类字符（副标题常带【中字】一类的备注尾巴）
    let mut sub = subtitle.to_string();
    for pat in ['[', '【', '（', '('] {
        if let Some(i) = sub.find(pat) {
            sub.truncate(i);
        }
    }
    let sub = sub.trim();
    if sub.chars().count() >= 6 && sub != name {
        pairs.push((sub.to_string(), sub.to_string()));
    }
    if !relaxed {
        pairs.truncate(3);
    }
    pairs
}

/// 参与名称复核的名字：查询词本身 + 发布名（归一化后同一个就不用列两遍）
pub fn match_names(queried: &str, release_name: &str) -> Vec<String> {
    if normalize_search_name(queried) == normalize_search_name(release_name) {
        vec![release_name.to_string()]
    } else {
        vec![queried.to_string(), release_name.to_string()]
    }
}

/// 查询词：只取前几个词、去掉所有标点（`wordQuery`）
pub fn word_query(
    name: &str,
    max_words: usize,
    min_words: usize,
    max_length: usize,
) -> Option<String> {
    let words: Vec<String> = name_tokens(name)
        .into_iter()
        .filter(|w| w.chars().count() >= 3)
        .collect();
    if words.len() < min_words {
        return None;
    }
    let q = words
        .iter()
        .take(max_words)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if q.chars().count() < 10 {
        return None;
    }
    Some(q.chars().take(max_length).collect())
}

// ---------- 名称判定 ----------

/// 两个名称是否指同一种子：归一化互含为准，`token_tolerant` 时补一条词元包含判定
pub fn search_names_match(candidate: &str, release_name: &str, token_tolerant: bool) -> bool {
    let c = normalize_search_name(candidate);
    let t = normalize_search_name(release_name);
    if c.chars().count() < 8 || t.chars().count() < 8 {
        return false;
    }
    if c.contains(&t) || (t.chars().count() >= 15 && t.contains(&c)) {
        return true;
    }
    if !token_tolerant {
        return false;
    }
    name_tokens_contained(release_name, candidate)
}

/// 发布名的每个词元都能在候选名里找到（允许缺 1 个非关键字元）。
/// 站点常见差异：截断尾部（丢组名）、标题里多插 2Audios 一类备注——都不改变「同一种子」的事实。
pub fn name_tokens_contained(name: &str, other: &str) -> bool {
    let target = comparable_name_tokens(name);
    let cand: HashSet<String> = comparable_name_tokens(other).into_iter().collect();
    if target.len() < 4 || cand.len() < 4 {
        return false;
    }
    let missing: Vec<&String> = target.iter().filter(|t| !cand.contains(*t)).collect();
    missing.len() <= 1 && !missing.iter().any(|t| is_discriminating_token(t))
}

/// 发布名分词：连字符不算分隔符（Blu-ray / WEB-DL / DTS-HD 各站写法不一），
/// 丢掉长度 <2 的词元（"2.0" 拆出的 "2"、"0" 是噪声）
pub fn name_tokens(s: &str) -> Vec<String> {
    let joined = strip_tags(s).replace('-', "");
    joined
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2)
        .map(|t| t.to_string())
        .collect()
}

/// 比对用的词元（Lagoon 压成 lagon 就查不到站点了，所以只在比对时压）
pub fn comparable_name_tokens(s: &str) -> Vec<String> {
    name_tokens(s)
        .into_iter()
        .map(|t| fold_doubled_letters(&t))
        .collect()
}

/// 站点对同一介质常有两种写法：Blu-ray / Blu Ray / BluRay。去掉分隔符后前两种都是
/// bluray，最后一种成了 blurray，「关键字元对不上」会把同一种子判成两个版本。
/// 比对用的串里压掉连续重复的字母（两边同样处理；数字不压，2006、1080p 保持原样）。
pub fn fold_doubled_letters(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev = '\0';
    for c in s.chars() {
        if c == prev && c.is_ascii_alphabetic() {
            continue;
        }
        out.push(c);
        prev = c;
    }
    out
}

/// 关键字元：季/集/年份/分辨率/编码音频对不上，说明是两个不同种子，不能算「已存在」
const DISCRIMINATING_TOKENS: &[&str] = &[
    "x264",
    "x265",
    "hevc",
    "h264",
    "h265",
    "avc",
    "vc1",
    "mpeg2",
    "av1",
    "vp9",
    "vp8",
    "xvid",
    "divx",
    "prores",
    "bluray",
    "remux",
    "webdl",
    "webrip",
    "hdtv",
    "bdrip",
    "dvdrip",
    "dvd",
    "hdcam",
    "flac",
    "lpcm",
    "pcm",
    "wav",
    "aac",
    "ac3",
    "eac3",
    "dts",
    "dtshd",
    "dtsma",
    "truehd",
    "atmos",
    "mp3",
    "opus",
    "hdr10",
    "dolbyvision",
    "imax",
    "3d",
];

pub fn is_discriminating_token(t: &str) -> bool {
    if DISCRIMINATING_TOKENS.contains(&t) {
        return true;
    }
    is_shape_token(t)
}

/// 形状字元：`^(?:s\d{1,2}|e\d{1,3}|\d{1,2}x\d{1,3}|19\d{2}|20\d{2}|\d{3,4}p|4k|uhd|sdr|hdr)$`
/// （Swift 版是正则，这里手写等价判定——不引 regex 依赖）
fn is_shape_token(t: &str) -> bool {
    if matches!(t, "4k" | "uhd" | "sdr" | "hdr") {
        return true;
    }
    let b = t.as_bytes();
    if b.is_empty() {
        return false;
    }
    let digits = |s: &[u8]| s.iter().all(|c| c.is_ascii_digit());
    // s\d{1,2} / e\d{1,3}
    if ((b[0] == b's' && b.len() <= 3) || (b[0] == b'e' && b.len() <= 4))
        && b.len() >= 2
        && digits(&b[1..])
    {
        return true;
    }
    // 19\d{2} | 20\d{2}
    if b.len() == 4 && (t.starts_with("19") || t.starts_with("20")) && digits(&b[2..]) {
        return true;
    }
    // \d{3,4}p
    if (b.len() == 4 || b.len() == 5) && b[b.len() - 1] == b'p' && digits(&b[..b.len() - 1]) {
        return true;
    }
    // \d{1,2}x\d{1,3}
    if let Some(x) = b.iter().position(|c| *c == b'x') {
        let tail = b.len() - x - 1;
        if (1..=2).contains(&x) && (1..=3).contains(&tail) && digits(&b[..x]) && digits(&b[x + 1..])
        {
            return true;
        }
    }
    false
}

/// 搜索名称归一化：去标签/实体、小写、只留字母数字（CJK 按字母保留）、压连续重复字母
pub fn normalize_search_name(s: &str) -> String {
    let t = decode_entities(&strip_tags(s)).to_lowercase();
    let filtered: String = t.chars().filter(|c| c.is_alphanumeric()).collect();
    fold_doubled_letters(&filtered)
}
