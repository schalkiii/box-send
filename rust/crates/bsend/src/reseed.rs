//! 转种上传（Rust 原生全套，不再交接 GUI 版）。
//!
//! 流水线：源站详情页 → 发布要素（标题/副标题/简介/MediaInfo/链接）→ LuckAudit 三查
//! （MediaInfo 非空、简介带 IMDb/豆瓣/TMDB、中字标签）→ 站点官方预审（LuckAudit
//! pre-audit，支持该接口的站把将提交字段交给站方打分，不过不发）→ 目标站
//! upload.php 表单提交 → 发后自检（回读新种页核对标题/MediaInfo/链接）→ 新种详情页
//! 回推下载器 → 校验 → 格子落 Reseeded。
//!
//! 另有审阅模式（`review_reseed`）：同一套解析/预检/字段构造跑到「将要提交什么」为止，
//! 不 POST、不落账，官方预审结论一并进报告——先看清字段再决定发不发
//! （CLI `bsend reseed` 默认、GUI「审阅」按钮）。
//!
//! 表单构造与 Swift 版 `NexusPHPAdapter.buildUploadFields` 同思路（hidden 透传、
//! select 默认值、业务字段覆盖、标签动态匹配），但只做站点无关的通用层：
//! 逐站 overrides（quality 下拉/mode 字段/两步上传）不在 v1 范围，遇到特殊站会
//! 在预检里报「表单不认识的必填项」而不是硬着头皮提交。

use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, REFERER};
use url::Url;

use crate::bencode;
use crate::config::{Config, SiteConf};
use crate::coverage::now_unix;
use crate::index::{ContentIndex, CoverageStatus, SiteCoverage};
use crate::qbittorrent::QBittorrent;
use crate::sites::nexusphp::{
    decode_entities, decode_entities_once, is_detail_href, resolve_url, review_mark,
    search_names_match, sizes_close_with_slack, strip_tags, NexusPhp, ReviewMark, SearchOutcome,
    SiteNotice, TorrentPage,
};
use crate::throttle::{BudgetKind, SiteThrottle};
use crate::torrent_rewrite;

/// 上传成功后立刻把站点种子下回来挂进 qB 是硬步骤，没有配置开关：
/// 站点刚建种时 download.php 可能先回 “torrent not registered”，过几十秒才正常。
/// 这里固定最多试 4 轮、每轮间隔 15 秒；拿不到才落待人工，回访还能用 `bsend pull` 补。
const POST_UPLOAD_PULL_ATTEMPTS: usize = 4;
const POST_UPLOAD_PULL_INTERVAL_SEC: u64 = 15;

/// 从源站详情页解析出的发布要素（Swift `ReleaseInfo` 的通用子集）
#[derive(Debug, Clone, Default)]
pub struct ReleaseInfo {
    pub name: String,
    pub subtitle: String,
    /// 简介 HTML（源页原样；上传前会补链接段）
    pub descr: String,
    /// MediaInfo/BDInfo 原文
    pub mediainfo: String,
    /// `tt1234567`
    pub imdb: Option<String>,
    /// 豆瓣 subject id
    pub douban: Option<String>,
    /// TMDB 规范链接
    pub tmdb: Option<String>,
    /// 标题清洗时已经判定为完结剧集（去掉标题里的 Complete 后仍要能勾「完结」标签）
    pub completed_hint: bool,
}

impl ReleaseInfo {
    /// 中文字幕/中字特征（发布名、副标题、简介任一处命中即可）
    pub fn has_chinese_subs(&self) -> bool {
        const MARKERS: [&str; 14] = [
            "中字",
            "简体",
            "繁体",
            "简繁",
            "简中",
            "繁中",
            "中英",
            "chs",
            "cht",
            "gb",
            "big5",
            "chinese",
            "简体中文",
            "繁體中文",
        ];
        let hay = format!(
            "{} {} {}",
            self.name.to_lowercase(),
            self.subtitle.to_lowercase(),
            self.descr.to_lowercase()
        );
        // 常见「中英字幕/简繁字幕」不等于连续「中字」
        if ["中英字幕", "中文字幕", "简繁字幕", "中英双字", "简繁中字"]
            .iter()
            .any(|m| hay.contains(m))
        {
            return true;
        }
        if MARKERS.iter().any(|m| hay.contains(m)) {
            return true;
        }
        // MediaInfo 的 Text 段 Language=Chinese / Title 带简繁，也是中字的实证
        let mut section = String::new();
        for line in self.mediainfo.lines() {
            let l = line.trim();
            let low = l.to_ascii_lowercase();
            if ["text", "audio", "video", "general", "menu", "chapters"]
                .iter()
                .any(|p| low.starts_with(p))
            {
                section = low.split('#').next().unwrap_or("").to_string();
            }
            if section.starts_with("text")
                && ((low.starts_with("language")
                    && (low.contains("chinese")
                        || low.contains("zh")
                        || low.contains("chi")
                        || low.contains("zho")))
                    || l.contains("简体")
                    || l.contains("繁体")
                    || l.contains("中字"))
            {
                return true;
            }
        }
        false
    }

    /// 完结剧集特征：完结 / 全集 / Complete（拉丁词边界）
    pub fn is_completed_series(&self) -> bool {
        if self.completed_hint {
            return true;
        }
        let hay = format!("{} {}", self.name, self.subtitle).to_ascii_lowercase();
        if hay.contains("完结") || hay.contains("全集") || hay.contains("全剧终") {
            return true;
        }
        // 全41集 这类「全N集」也算完结
        for (i, c) in hay.char_indices() {
            if c == '全' {
                let rest = &hay[i + c.len_utf8()..];
                let digits: String = rest.chars().take_while(|x| x.is_ascii_digit()).collect();
                if !digits.is_empty() && rest[digits.len()..].starts_with('集') {
                    return true;
                }
            }
        }
        let mut from = 0;
        while let Some(p) = hay[from..].find("complete") {
            let at = from + p;
            let end = at + "complete".len();
            let pre_ok = at == 0 || !hay.as_bytes()[at - 1].is_ascii_alphanumeric();
            let post_ok = end >= hay.len() || !hay.as_bytes()[end].is_ascii_alphanumeric();
            if pre_ok && post_ok {
                return true;
            }
            from = at + 1;
        }
        false
    }

    /// MediaInfo 里是否有中文音轨（Audio 段的 Language=Chinese 或 国语/普通话/国配/Mandarin）
    /// 英语音轨：MediaInfo Audio Language=English / Title 带 English/英语
    pub fn has_english_audio(&self) -> bool {
        let mut section = String::new();
        for line in self.mediainfo.lines() {
            let l = line.trim();
            let low = l.to_ascii_lowercase();
            if ["audio", "video", "text", "general", "menu"]
                .iter()
                .any(|p| low.starts_with(p))
            {
                section = low.split('#').next().unwrap_or("").to_string();
            }
            if section.starts_with("audio")
                && (low.contains("english") || l.contains("英语") || l.contains("英語"))
            {
                return true;
            }
        }
        false
    }

    /// 粤语音轨：MediaInfo 的 Audio Title/Language 出现 Cantonese/粤语
    pub fn has_cantonese_audio(&self) -> bool {
        let hay = format!("{} {} {}", self.name, self.subtitle, self.descr).to_lowercase();
        if hay.contains("cantonese")
            || hay.contains("粤语")
            || hay.contains("粵語")
            || hay.contains("国粤")
            || hay.contains("國粵")
        {
            return true;
        }
        let mut section = String::new();
        for line in self.mediainfo.lines() {
            let l = line.trim();
            let low = l.to_ascii_lowercase();
            if ["audio", "video", "text", "general", "menu"]
                .iter()
                .any(|p| low.starts_with(p))
            {
                section = low.split('#').next().unwrap_or("").to_string();
            }
            if section.starts_with("audio")
                && (low.contains("cantonese") || l.contains("粤语") || l.contains("粵語"))
            {
                return true;
            }
        }
        false
    }

    pub fn has_chinese_audio(&self) -> bool {
        let mut section = String::new();
        for line in self.mediainfo.lines() {
            let l = line.trim();
            let low = l.to_ascii_lowercase();
            if ["audio", "text", "general", "video", "chapters", "menu"]
                .iter()
                .any(|p| low.starts_with(p))
            {
                section = low.split('#').next().unwrap_or("").to_string();
            }
            if section.starts_with("audio")
                && ((low.starts_with("language") && low.contains("chinese"))
                    || low.contains("mandarin")
                    || l.contains("国语")
                    || l.contains("普通话")
                    || l.contains("国配"))
            {
                return true;
            }
        }
        false
    }

    /// 简介里是否已带 IMDb/豆瓣/TMDB 任一链接
    pub fn has_media_links(&self) -> bool {
        let d = self.descr.to_lowercase();
        d.contains("imdb.com") || d.contains("douban.com") || d.contains("themoviedb.org")
    }

    /// 按已知 id 补一段链接（LuckAudit 要求简介带 IMDb/豆瓣/TMDB）。
    /// 返回是否改写了简介。
    pub fn ensure_media_links(&mut self) -> bool {
        if self.has_media_links() {
            return false;
        }
        let mut lines: Vec<String> = Vec::new();
        if let Some(tt) = &self.imdb {
            lines.push(format!("IMDb：https://www.imdb.com/title/{tt}/"));
        }
        if let Some(db) = &self.douban {
            lines.push(format!("豆瓣：https://movie.douban.com/subject/{db}/"));
        }
        if let Some(tm) = &self.tmdb {
            lines.push(format!("TMDB：{tm}"));
        }
        if lines.is_empty() {
            return false;
        }
        self.descr = format!(
            "{}\n\n[quote][b]条目链接[/b]\n{}[/quote]",
            self.descr.trim_end(),
            lines.join("\n")
        );
        true
    }
}

/// 上传前体检（LuckAudit 81 分被拒的三类问题在这里拦下）
#[derive(Debug, Default)]
pub struct Preflight {
    /// 阻断项：有它就不许提交
    pub errors: Vec<String>,
    /// 提醒项：可提交但要在日志里说清
    pub warnings: Vec<String>,
    /// 已自动修掉的问题
    pub fixes: Vec<String>,
}

impl Preflight {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// 人看的一行摘要
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.errors.is_empty() {
            parts.push(format!("阻断 {}", self.errors.len()));
        }
        if !self.warnings.is_empty() {
            parts.push(format!("警告 {}", self.warnings.len()));
        }
        if !self.fixes.is_empty() {
            parts.push(format!("已自动补 {}", self.fixes.len()));
        }
        if parts.is_empty() {
            "预检通过".into()
        } else {
            parts.join("、")
        }
    }
}

/// LuckAudit 三查：① 中文字幕要选中字标签 ② 简介要带 IMDb/豆瓣/TMDB ③ MediaInfo 不能为空
/// 想勾的标签：中字 + 完结（站点没有对应复选框时匹配不到，不报错）
fn wanted_tags(info: &ReleaseInfo) -> Vec<&'static str> {
    let mut out = Vec::new();
    if info.has_chinese_subs() {
        out.push("中字");
    }
    if info.is_completed_series() {
        out.push("完结");
    }
    if info.has_chinese_audio() {
        out.push("国语");
    }
    if info.has_cantonese_audio() {
        out.push("粤语");
    }
    let title_hay = format!("{} {}", info.name, info.subtitle);
    let has_cjk = title_hay
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
    if info.has_english_audio()
        && !info.has_chinese_audio()
        && !info.has_cantonese_audio()
        && !has_cjk
        && !info.has_chinese_subs()
    {
        out.push("英语");
    }
    let hdr_hay = title_hay.to_lowercase();
    let is_uhd = ["2160p", "4k", "uhd"].iter().any(|x| hdr_hay.contains(x));
    if is_uhd && (hdr_hay.contains("hdr10") || hdr_hay.contains("hdr")) {
        out.push("HDR10");
    }
    out
}

pub fn preflight(
    info: &mut ReleaseInfo,
    tags: &[String],
    rules: &crate::gates::SiteRules,
) -> Preflight {
    let mut pf = Preflight::default();

    // ③ MediaInfo：站点规则明确不要（requireMediaInfo=false）时降为警告，
    // 否则「门禁放行、预检仍阻断」会让自动发种永远发不出去
    if info.mediainfo.trim().is_empty() {
        let msg =
            "MediaInfo 为空：源站详情页没有 MediaInfo/BDInfo 块，先在源站补齐或手动粘贴后再转种";
        if rules.require_mediainfo() {
            pf.errors.push(msg.into());
        } else {
            pf.warnings
                .push(format!("{msg}（站点规则不要 MediaInfo，已放宽）"));
        }
    }

    // ② 链接：有 id 就自动补进简介；三个 id 都缺时按站点规则决定阻断/警告
    if !info.has_media_links() {
        if info.ensure_media_links() {
            pf.fixes
                .push("简介缺条目链接，已按源页 IMDb/豆瓣/TMDB 补上".into());
        } else {
            let msg = "简介缺少 IMDb/豆瓣/TMDB 链接，且源页解析不出条目 id：请手动把链接写进简介";
            if rules.require_links() {
                pf.errors.push(msg.into());
            } else {
                pf.warnings
                    .push(format!("{msg}（站点规则不要条目链接，已放宽）"));
            }
        }
    }

    // ① 中字标签
    if info.has_chinese_subs() && !tags.iter().any(|t| t.contains("中字")) {
        pf.warnings.push(
            "检测到中文字幕，确认表单里勾上「中字」类标签（发布时自动勾选；表单没有该复选框才会漏）".into(),
        );
    }
    pf
}

// ---------------- 源站详情页解析 ----------------

/// 上传标题清洗：站点 LuckAudit 不允许中文主标题，但很多源页标题是
/// 「中文名.英文名.年份...」。只有在前缀含中文且后面存在 >=2 个连续 ASCII
/// 字母的正式英文段时才去掉中文前缀；纯中文标题原样保留。
fn normalize_release_title(name: &str) -> String {
    if !name.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
        return name.to_string();
    }
    let chars: Vec<char> = name.chars().collect();
    let mut start = None;
    let mut i = 0;
    while i + 1 < chars.len() {
        // 从首个 ASCII 词起，且后面 6 个字符内存在连续字母；这样 I.Am... 的
        // 单字母 I 不会被误当噪声，而 1080p 末尾的孤立 p 也不会触发。
        if chars[i].is_ascii_alphabetic()
            && chars[i..chars.len().min(i + 6)]
                .windows(2)
                .any(|w| w[0].is_ascii_alphabetic() && w[1].is_ascii_alphabetic())
        {
            start = Some(i);
            break;
        }
        i += 1;
    }
    let Some(i) = start else {
        return name.to_string();
    };
    let prefix: String = chars[..i].iter().collect();
    if !prefix
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
    {
        return name.to_string();
    }
    let suffix: String = chars[i..].iter().collect();
    let suffix = suffix.trim_start_matches(['.', '-', '_', '[', ']', ' ']);
    if suffix.chars().filter(|c| c.is_ascii_alphanumeric()).count() >= 8
        && suffix.chars().any(|c| c.is_ascii_digit())
    {
        suffix.to_string()
    } else {
        name.to_string()
    }
}

/// 标题技术参数的固定顺序：分辨率 → 帧率/HDR → 片源 → 视频编码 → 位深 → 音频 → 压制组。
/// 站点审核反复指出「HDTV 1080p」「AAC x264」这类倒序，发布前统一排好。
fn tech_rank(token: &str) -> Option<u8> {
    let t = token.to_ascii_lowercase();
    const RES: [&str; 9] = [
        "2160p", "1080p", "1080i", "720p", "576p", "480p", "4k", "8k", "uhd",
    ];
    const FPS_HDR: [&str; 8] = [
        "60fps", "50fps", "48fps", "30fps", "25fps", "24fps", "hdr", "hdr10",
    ];
    const SOURCE: [&str; 15] = [
        "bluray", "blu-ray", "web-dl", "webdl", "webrip", "hdtv", "hdrip", "bdrip", "dvdrip",
        "tvrip", "remux", "dvd", "bd", "hdtvrip", "uhd",
    ];
    const REGION: [&str; 18] = [
        "usa", "us", "uk", "jpn", "jp", "chn", "ger", "de", "fra", "fr", "hk", "tw", "kor", "kr",
        "ita", "esp", "rus", "can",
    ];
    const VIDEO: [&str; 12] = [
        "x264", "x265", "h264", "h.264", "h265", "h.265", "hevc", "avc", "vc-1", "mpeg2", "mpeg-2",
        "av1",
    ];
    const DEPTH: [&str; 5] = ["10bit", "8bit", "12bit", "10-bit", "8-bit"];
    const AUDIO: [&str; 18] = [
        "aac",
        "ac3",
        "eac3",
        "e-ac3",
        "dts",
        "dts-hd",
        "dtshd",
        "dts-hd.ma",
        "truehd",
        "flac",
        "mp3",
        "opus",
        "atmos",
        "lpcm",
        "pcm",
        "dd",
        "dd+",
        "ddp",
    ];
    if RES.contains(&t.as_str()) {
        Some(0)
    } else if FPS_HDR.contains(&t.as_str()) {
        Some(1)
    } else if SOURCE.contains(&t.as_str()) {
        Some(2)
    } else if REGION.contains(&t.as_str()) {
        Some(3)
    } else if VIDEO.contains(&t.as_str()) {
        Some(4)
    } else if DEPTH.contains(&t.as_str()) {
        Some(5)
    } else if AUDIO.contains(&t.as_str()) {
        Some(6)
    } else if is_audio_detail(&t) {
        Some(7)
    } else {
        None
    }
}

fn is_audio_detail(token: &str) -> bool {
    let t = token.to_ascii_lowercase();
    matches!(
        t.as_str(),
        "ma" | "2audio" | "2audios" | "5.1" | "7.1" | "2.0" | "5.1ch" | "7.1ch"
    )
}

fn has_series_marker(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    let bytes = low.as_bytes();
    if low.contains("season") || low.contains("complete series") {
        return true;
    }
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b's' && bytes[i + 1].is_ascii_digit() {
            let is_word_start = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            if is_word_start {
                return true;
            }
        }
    }
    false
}

fn explicit_series(name: &str, subtitle: &str) -> bool {
    let hay = format!("{} {}", name, subtitle);
    if has_series_marker(&hay) || hay.contains("完结") || hay.contains("全集") {
        return true;
    }
    let chars: Vec<char> = hay.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == '全' {
            let digits: String = chars[i + 1..]
                .iter()
                .take_while(|x| x.is_ascii_digit())
                .collect();
            if !digits.is_empty() && chars.get(i + 1 + digits.len()) == Some(&'集') {
                return true;
            }
        }
    }
    false
}

/// 按站点通行规范重排标题里的技术参数；只有识别出至少两个技术词才动，
/// 且遇到 `[...]` 前缀的中文标题直接原样返回，避免打乱方括号结构。
fn reorder_release_title(name: &str, completed: bool) -> String {
    if name.contains('[') || name.contains(']') {
        return name.to_string();
    }
    let sep = if name.contains(' ') {
        ' '
    } else if name.contains('.') {
        '.'
    } else {
        return name.to_string();
    };
    let tokens: Vec<&str> = name.split(sep).filter(|t| !t.is_empty()).collect();
    let mut base: Vec<&str> = Vec::new();
    let mut tech: Vec<(u8, usize, &str)> = Vec::new();
    let mut group: Option<&str> = None;
    let mut tail: Vec<&str> = Vec::new();
    let mut seen_tech = false;
    let mut dropped_complete = false;
    for (idx, token) in tokens.iter().enumerate() {
        if completed && token.eq_ignore_ascii_case("complete") {
            dropped_complete = true;
            continue;
        }
        if let Some(rank) = tech_rank(token) {
            seen_tech = true;
            tech.push((rank, idx, *token));
            continue;
        }
        // `x264-JING` / `AAC-GRP`：左边是技术词、右边是压制组，拆开各归各位。
        if let Some((left, right)) = token.rsplit_once('-') {
            if !left.is_empty() && !right.is_empty() {
                if let Some(rank) = tech_rank(left) {
                    seen_tech = true;
                    tech.push((rank, idx, left));
                    group = Some(right);
                    continue;
                }
            }
        }
        // 技术词后面剩下的内容（如 `AC3￡cXcY@FRDS`、`2Audio`、`MNHD-FRDS`）
        // 按原顺序整体留在最后，不要塞回标题主体前面。
        if seen_tech {
            tail.push(*token);
            continue;
        }
        base.push(*token);
    }
    if tech.len() < 2 && !dropped_complete {
        return name.to_string();
    }
    tech.sort_by_key(|(rank, idx, _)| (*rank, *idx));
    let mut out = base.join(&sep.to_string());
    let tech_join = tech
        .iter()
        .map(|(_, _, t)| *t)
        .collect::<Vec<_>>()
        .join(&sep.to_string());
    if !tech_join.is_empty() {
        if !out.is_empty() {
            out.push(sep);
        }
        out.push_str(&tech_join);
    }
    if let Some(g) = group {
        out.push('-');
        out.push_str(g.trim_start_matches('-'));
    }
    if !tail.is_empty() {
        if !out.is_empty() {
            out.push(sep);
        }
        out.push_str(&tail.join(&sep.to_string()));
    }
    out
}

fn normalize_release_title_for(name: &str, completed: bool) -> String {
    let cleaned = normalize_release_title(name);
    reorder_release_title(&cleaned, completed)
}

/// 详情页解析：发布名/副标题/简介/MediaInfo/条目链接（Swift `parse` 的通用子集）
pub fn parse_release_page(html: &str, name_fallback: &str) -> ReleaseInfo {
    let raw_name = first_tag_text(html, "h1").unwrap_or_else(|| name_fallback.to_string());
    let mut info = ReleaseInfo {
        name: strip_promo_suffix(&raw_name),
        ..Default::default()
    };
    // 副标题：行内「副标题」表行，或 small_descr 输入框
    info.subtitle = detail_row_value(html, "副标题")
        .or_else(|| detail_row_value(html, "副標題"))
        .or_else(|| input_value(html, "small_descr"))
        .unwrap_or_default();
    let completed = explicit_series(&raw_name, &info.subtitle);
    info.name = normalize_release_title_for(&info.name, completed);
    info.completed_hint = completed;
    // 简介容器（配对 div，常见 id）
    for id in ["kdescr", "largedescribe", "kt_d", "descr", "description"] {
        if let Some(d) = div_inner(html, id) {
            info.descr = d;
            break;
        }
    }
    if info.descr.is_empty() {
        // 退而求其次：「简介」表行 → 正文纯文本。绝不把整页 HTML 当简介提交
        // （实测有站长页面容器不叫上面任何 id，兜底兜成 58KB 源码差点发出去）
        info.descr = detail_row_value(html, "简介")
            .or_else(|| detail_row_value(html, "簡介"))
            .unwrap_or_else(|| body_text(html));
    }
    info.descr = unescape_descr(&info.descr);
    // MediaInfo：<pre>、引用框、code 块里含 Unique ID / DISC TITLE 特征的段
    info.mediainfo = normalize_mediainfo(&extract_mediainfo(html, &info.descr));
    // 条目链接
    let hay = format!("{}\n{}", info.descr, html);
    info.imdb = find_tt(&hay);
    info.douban = find_douban(&hay);
    info.tmdb = find_tmdb(&hay);
    info
}

/// `tt` + 5–13 位数字（IMDb id）
fn find_tt(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut i = 0;
    while i + 2 < b.len() {
        if (b[i] == b't' || b[i] == b'T') && (b[i + 1] == b't' || b[i + 1] == b'T') {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let n = j - (i + 2);
            if (5..=13).contains(&n) {
                return Some(s[i..j].to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    None
}

fn find_douban(s: &str) -> Option<String> {
    let key = "douban.com/subject/";
    let p = s.find(key)?;
    let rest = &s[p + key.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 4 {
        Some(digits)
    } else {
        None
    }
}

fn find_tmdb(s: &str) -> Option<String> {
    for key in ["themoviedb.org/movie/", "themoviedb.org/tv/"] {
        if let Some(p) = s.find(key) {
            let start = p + key.len();
            let digits: String = s[start..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                let kind = if key.contains("/movie/") {
                    "movie"
                } else {
                    "tv"
                };
                return Some(format!("https://www.themoviedb.org/{kind}/{digits}"));
            }
        }
    }
    None
}

/// MediaInfo/BDInfo 块：先扫 `<pre>`，再扫引用框/code 块，要求「大部分行是键值对」
fn extract_mediainfo(html: &str, descr: &str) -> String {
    let mut blocks: Vec<String> = Vec::new();
    collect_tag_blocks(html, "pre", &mut blocks);
    collect_tag_blocks(html, "fieldset", &mut blocks);
    collect_tag_blocks(html, "code", &mut blocks);
    collect_tag_blocks(html, "textarea", &mut blocks);
    for b in blocks {
        let text = strip_tags(&b);
        if is_mediainfo_block(&text) {
            return text.trim().to_string();
        }
    }
    // 简介整段也可能就是 MediaInfo（纯键值对）
    let text = strip_tags(descr);
    if is_mediainfo_block(&text) {
        return text.trim().to_string();
    }
    String::new()
}

fn is_mediainfo_block(text: &str) -> bool {
    const KEYS: [&str; 6] = [
        "Unique ID",
        "Format",
        "Bit rate",
        "Width",
        "DISC INFO",
        "VIDEO:",
    ];
    let hits = KEYS.iter().filter(|k| text.contains(**k)).count();
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    hits >= 2 && lines >= 5
}

fn collect_tag_blocks(html: &str, tag: &str, out: &mut Vec<String>) {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut rest = html;
    // to_ascii_lowercase 不改变字节长度：find 出来的偏移可以直接切原串
    while let Some(p) = rest.to_ascii_lowercase().find(&open) {
        let after = &rest[p..];
        match after.to_ascii_lowercase().find(&close) {
            Some(q) => {
                out.push(after[..q + close.len()].to_string());
                rest = &after[q + close.len()..];
            }
            None => break,
        }
    }
}

fn first_tag_text(html: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let p = html.to_ascii_lowercase().find(&open)?;
    let after = &html[p..];
    let q = after.to_ascii_lowercase().find(&close)?;
    let inner = &after[after.find('>').map(|i| i + 1).unwrap_or(0)..q];
    let t = strip_tags(inner).trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// `<tr><td>标签</td><td>值</td></tr>` 行取值（直接在 HTML 上找，strip_tags 会把
/// `</td><td>` 拼死、丢掉列分隔）
fn detail_row_value(html: &str, label: &str) -> Option<String> {
    let p = html.find(label)?;
    let after_label = &html[p + label.len()..];
    // 标签单元格收尾后的下一个 <td> 里是值
    let td_close = after_label.find("</td>")?;
    let rest = &after_label[td_close + 5..];
    let td_open = rest.find("<td")?;
    let val_start = rest[td_open..].find('>').map(|i| td_open + i + 1)?;
    let val_end = rest[val_start..].find("</td>")? + val_start;
    let v = strip_tags(&rest[val_start..val_end]).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

fn input_value(html: &str, name: &str) -> Option<String> {
    // name="…" 或 name='…'（实测有站用单引号，只认双引号会整段解析丢）
    let p = html
        .find(&format!("name=\"{name}\""))
        .or_else(|| html.find(&format!("name='{name}'")))?;
    // 从该位置向前后找 value="…" / value='…'
    let seg = &html[p..html.len().min(p + 600)];
    for (open, close) in [("value=\"", '"'), ("value='", '\'')] {
        if let Some(vp) = seg.find(open) {
            let rest = &seg[vp + open.len()..];
            if let Some(end) = rest.find(close) {
                return Some(decode_entities(&rest[..end]));
            }
        }
    }
    None
}

/// 标题尾部促销角标（`[50%]`/`[FREE]`/`[2x]`/`[免费]`…）：站点折扣标记混进发布名要剥掉，
/// 但 `[中字]` 这类内容标签保留
fn strip_promo_suffix(name: &str) -> String {
    let mut s = name.trim_end().to_string();
    // 免费促销常带倒计时尾巴（luckpt 实测「[免费] 剩余时间：6天18时」）：
    // 先剥倒计时，[免费] 才落到尾部能被方括号循环剥掉
    for marker in ["剩余时间", "剩餘時間"] {
        if let Some(p) = s.rfind(marker) {
            let tail = s[p + marker.len()..].trim_start_matches(['：', ':', ' ']);
            let is_countdown = !tail.is_empty()
                && tail.chars().all(|c| {
                    c.is_ascii_digit()
                        || matches!(
                            c,
                            '天' | '时'
                                | '時'
                                | '小'
                                | '分'
                                | '钟'
                                | '鐘'
                                | '秒'
                                | '年'
                                | '月'
                                | ' '
                        )
                });
            if is_countdown {
                s = s[..p].trim_end().to_string();
            }
            break;
        }
    }
    loop {
        let t = s.trim_end();
        let Some(open) = t.rfind('[') else { break };
        if !t.ends_with(']') {
            break;
        }
        let inner = t[open + 1..t.len() - 1].trim();
        if inner.is_empty() {
            break;
        }
        let low = inner.to_lowercase();
        let promo = low
            .chars()
            .all(|c| c.is_ascii_digit() || c == '%' || c == '.')
            || low == "free"
            || matches!(
                low.as_str(),
                "免费" | "免費" | "折扣" | "促销" | "半价" | "未审" | "未审核"
            )
            || (low.ends_with('x')
                && low.len() <= 4
                && low[..low.len() - 1].chars().all(|c| c.is_ascii_digit()))
            || low.split_once('x').is_some_and(|(n, tail)| {
                !n.is_empty()
                    && n.chars().all(|c| c.is_ascii_digit())
                    && matches!(tail, "免费" | "免費" | "free")
            });
        if !promo {
            break;
        }
        s = t[..open].trim_end().to_string();
    }
    s
}

/// 源站把简介 HTML 整体转义后存储（luckpt 实测 kdescr 里是 `&lt;fieldset&gt;…`，
/// 站点只补了 nl2br 的裸 `<br />`）；也有站是「裸 `<div>` + 转义块」混排（hdvideo
/// 实测），或转义两层（`&amp;lt;fieldset`）。按块判断，不再要求整段转义多于裸标签。
fn unescape_descr(html: &str) -> String {
    let count_tags = |s: &str, prefix: &str| {
        let b = s.as_bytes();
        let mut n = 0usize;
        let mut from = 0usize;
        while let Some(p) = s[from..].find(prefix) {
            let at = from + p + prefix.len();
            if at < b.len() && (b[at].is_ascii_alphabetic() || b[at] == b'/' || b[at] == b'!') {
                n += 1;
            }
            from = from + p + prefix.len();
        }
        n
    };
    let rich_markers = [
        "fieldset",
        "img",
        "span",
        "font",
        "div",
        "table",
        "center",
        "iframe",
        "video",
        "audio",
        "blockquote",
        "pre",
        "code",
    ];
    let mut cur = html.to_string();
    for _ in 0..2 {
        let escaped = count_tags(&cur, "&lt;");
        let double = count_tags(&cur, "&amp;lt;");
        let raw = count_tags(&cur, "<");
        let lower = cur.to_ascii_lowercase();
        let rich = rich_markers
            .iter()
            .filter(|m| {
                lower.contains(&format!("&lt;{m}")) || lower.contains(&format!("&amp;lt;{m}"))
            })
            .count();
        let should_decode = escaped + double >= 3 && (rich >= 1 || (raw == 0 && escaped >= 5));
        if !should_decode {
            break;
        }
        cur = decode_entities_once(&cur);
    }
    cur
}

/// 目标站关闭 HTML、只吃 BBCode 时，把源站简介转成 BBCode。
/// NexusPHP 的 BBCode 会把 `[quote]` 渲染成 `<fieldset><legend>引用</legend>`；
/// 直接提交 HTML 会被站点转义成字面 `<fieldset>`，审核端看不到图也看不到排版。
pub(crate) fn html_to_bbcode(html: &str) -> String {
    fn newline(out: &mut String) {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
    }
    fn open(
        out: &mut String,
        stack: &mut Vec<(&'static str, String)>,
        key: &'static str,
        text: &str,
        close: &str,
    ) {
        out.push_str(text);
        stack.push((key, close.to_string()));
    }
    fn close(out: &mut String, stack: &mut Vec<(&'static str, String)>, key: &str) {
        let Some(pos) = stack.iter().rposition(|(k, _)| *k == key) else {
            return;
        };
        while stack.len() > pos {
            let (_, close) = stack.pop().expect("stack has pos");
            out.push_str(&close);
        }
    }
    fn clean_url(raw: &str) -> String {
        let mut u = raw.trim().to_string();
        loop {
            if let Some(s) = u.strip_suffix("%2522") {
                u = s.to_string();
            } else if let Some(s) = u.strip_suffix("%22") {
                u = s.to_string();
            } else {
                break;
            }
        }
        while let Some(s) = u.strip_prefix("%22") {
            u = s.to_string();
        }
        u.trim_matches('"').trim().replace(']', "%5D")
    }
    fn css_color(style: &str) -> Option<String> {
        let lower = style.to_ascii_lowercase();
        let p = lower.find("color:")?;
        let rest = &style[p + "color:".len()..];
        let end = rest.find([';', '!']).unwrap_or(rest.len());
        let v = rest[..end].trim().trim_matches('"').trim();
        (!v.is_empty()).then(|| v.to_string())
    }
    fn font_size(raw: &str) -> Option<u8> {
        let n: u8 = raw.trim().parse().ok()?;
        (1..=7).contains(&n).then_some(n)
    }
    fn tidy(s: &str) -> String {
        let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
        let mut out = String::with_capacity(normalized.len());
        let mut blank = 0usize;
        for line in normalized.lines() {
            let line = line.trim_end();
            if line.trim().is_empty() {
                blank += 1;
                if blank <= 1 {
                    out.push('\n');
                }
            } else {
                blank = 0;
                out.push_str(line);
                out.push('\n');
            }
        }
        while out.ends_with('\n') {
            out.pop();
        }
        out
    }

    let mut out = String::with_capacity(html.len());
    let mut stack: Vec<(&'static str, String)> = Vec::new();
    let mut i = 0usize;
    while i < html.len() {
        let Some(rel) = html[i..].find('<') else {
            out.push_str(&decode_entities(&html[i..]));
            break;
        };
        let start = i + rel;
        if start > i {
            out.push_str(&decode_entities(&html[i..start]));
        }
        let rest = &html[start..];
        if rest.starts_with("<!--") {
            i = rest
                .find("-->")
                .map(|p| start + p + 3)
                .unwrap_or(html.len());
            continue;
        }
        // `<` 后必须是标签起始字符；数学式子里的 `2 < 3` 原样保留
        if !rest
            .as_bytes()
            .get(1)
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'/' || *b == b'!')
        {
            out.push('<');
            i = start + 1;
            continue;
        }
        let Some(tag_end) = rest.find('>') else {
            out.push_str(&decode_entities(rest));
            break;
        };
        let tag = &rest[..tag_end + 1];
        i = start + tag_end + 1;
        let inner = &tag[1..tag.len() - 1];
        let closing = inner.trim_start().starts_with('/');
        let name: String = inner
            .trim_start_matches('/')
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }

        if (name == "script" || name == "style") && !closing {
            let lower = html[i..].to_ascii_lowercase();
            let close = format!("</{name}>");
            if let Some(p) = lower.find(&close) {
                i += p + close.len();
            }
            continue;
        }
        // [quote] 自带「引用」legend，源里的 legend 文案不再重复
        if name == "legend" && !closing {
            let lower = html[i..].to_ascii_lowercase();
            if let Some(p) = lower.find("</legend>") {
                i += p + "</legend>".len();
            }
            continue;
        }

        match name.as_str() {
            "br" => newline(&mut out),
            "hr" => {
                newline(&mut out);
                newline(&mut out);
            }
            "img" if !closing => {
                if let Some(src) = attr(tag, "src") {
                    let url = clean_url(&src);
                    if url.starts_with("http://") || url.starts_with("https://") {
                        out.push_str("[img]");
                        out.push_str(&url);
                        out.push_str("[/img]");
                    }
                }
            }
            "a" => {
                if closing {
                    close(&mut out, &mut stack, "a");
                } else if let Some(href) = attr(tag, "href") {
                    let href = href.trim().replace(']', "%5D");
                    if !href.is_empty() && href != "#" {
                        open(
                            &mut out,
                            &mut stack,
                            "a",
                            &format!("[url={href}]"),
                            "[/url]",
                        );
                    }
                }
            }
            "b" | "strong" => {
                if closing {
                    close(&mut out, &mut stack, "b");
                } else {
                    open(&mut out, &mut stack, "b", "[b]", "[/b]");
                }
            }
            "i" | "em" => {
                if closing {
                    close(&mut out, &mut stack, "i");
                } else {
                    open(&mut out, &mut stack, "i", "[i]", "[/i]");
                }
            }
            "u" => {
                if closing {
                    close(&mut out, &mut stack, "u");
                } else {
                    open(&mut out, &mut stack, "u", "[u]", "[/u]");
                }
            }
            "font" => {
                if closing {
                    close(&mut out, &mut stack, "font");
                } else {
                    let color = attr(tag, "color").filter(|c| !c.trim().is_empty());
                    let size = attr(tag, "size").and_then(|s| font_size(&s));
                    let mut opens = String::new();
                    let mut closes = String::new();
                    if let Some(c) = color {
                        opens.push_str(&format!("[color={}]", c.trim()));
                        closes.push_str("[/color]");
                    }
                    if let Some(n) = size {
                        opens.push_str(&format!("[size={n}]"));
                        closes.insert_str(0, "[/size]");
                    }
                    if !opens.is_empty() {
                        open(&mut out, &mut stack, "font", &opens, &closes);
                    }
                }
            }
            "span" => {
                if closing {
                    close(&mut out, &mut stack, "span");
                } else if let Some(style) = attr(tag, "style") {
                    if let Some(color) = css_color(&style) {
                        open(
                            &mut out,
                            &mut stack,
                            "span",
                            &format!("[color={color}]"),
                            "[/color]",
                        );
                    }
                }
            }
            "blockquote" | "fieldset" => {
                if closing {
                    newline(&mut out);
                    close(&mut out, &mut stack, "quote");
                } else {
                    newline(&mut out);
                    open(&mut out, &mut stack, "quote", "[quote]\n", "\n[/quote]");
                }
            }
            "pre" => {
                newline(&mut out);
            }
            "code" => {
                if closing {
                    close(&mut out, &mut stack, "code");
                } else {
                    open(&mut out, &mut stack, "code", "[code]", "[/code]");
                }
            }
            "div" | "p" | "center" | "table" | "tbody" | "thead" | "ul" | "ol" => {
                newline(&mut out);
            }
            "tr" => newline(&mut out),
            "td" | "th" => {
                if !out.ends_with('\n') && !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            "li" if !closing => {
                newline(&mut out);
                out.push_str("• ");
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if closing {
                    newline(&mut out);
                    close(&mut out, &mut stack, "h");
                } else {
                    newline(&mut out);
                    open(&mut out, &mut stack, "h", "[b]", "[/b]");
                }
            }
            _ => {}
        }
    }
    tidy(&out)
}

/// 判断简介是不是 HTML（避免把纯文本或已有 BBCode 误转）
fn looks_like_html(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    [
        "<fieldset",
        "<img",
        "<br",
        "<b>",
        "<strong",
        "<font",
        "<span",
        "<div",
        "<p>",
        "<a ",
        "<center",
        "<table",
        "<blockquote",
        "<pre",
        "<code",
        "<i>",
        "<u>",
    ]
    .iter()
    .any(|t| l.contains(t))
}

/// `<body>` 内的纯文本（简介容器都找不到时的兜底；绝不返回标签源码）
fn body_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let start = match lower.find("<body") {
        Some(p) => lower[p..].find('>').map(|i| p + i + 1).unwrap_or(0),
        None => 0,
    };
    let end = lower.find("</body>").unwrap_or(html.len());
    strip_tags(&html[start..end.min(html.len())])
}

/// 配对 div 内容（支持一层嵌套）。字节扫描 + ASCII 大小写不敏感比较：
/// 索引始终落在字符边界上（UTF-8 多字节序列不含 ASCII 字节，逐字节安全）
fn div_inner(html: &str, id: &str) -> Option<String> {
    // id="…" 或 id='…'（hdsky 等站用单引号）
    let p = html
        .find(&format!("id=\"{id}\""))
        .or_else(|| html.find(&format!("id='{id}'")))?;
    let open_end = html[..p].rfind('<')?;
    let content_start = html[open_end..].find('>').map(|i| open_end + i + 1)?;
    let b = html.as_bytes();
    let mut depth = 1usize;
    let mut i = content_start;
    while i < b.len() {
        if tag_at(b, i, b"<div") {
            depth += 1;
            i += 4;
        } else if tag_at(b, i, b"</div>") {
            depth -= 1;
            if depth == 0 {
                return Some(html[content_start..i].to_string());
            }
            i += 6;
        } else {
            i += 1;
        }
    }
    None
}

/// 字节序列 ASCII 大小写不敏感前缀匹配（i 必须在字符边界上：只在 ASCII 标签位调用）
fn tag_at(b: &[u8], i: usize, pat: &[u8]) -> bool {
    i + pat.len() <= b.len() && b[i..i + pat.len()].eq_ignore_ascii_case(pat)
}

// ---------------- 目标站上传表单 ----------------

/// 上传表单里要提交的一对键值
pub type Field = (String, String);

/// 上传表单块：action 含 takeupload / 含文件上传框的 `<form>…</form>`。
/// 页面导航条常带搜索表单（search_area 一类），字段解析必须限定在上传表单里。
/// 找不到候选返回 None——页面上根本没有静态上传表单。
pub fn upload_form_block(page: &str) -> Option<&str> {
    let lower = page.to_ascii_lowercase();
    let mut best: Option<(usize, usize, u8)> = None;
    let mut i = 0;
    while let Some(p) = lower[i..].find("<form") {
        let start = i + p;
        let open_end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(page.len());
        let close = lower[open_end..]
            .find("</form>")
            .map(|j| open_end + j)
            .unwrap_or(page.len());
        let bl = &lower[start..close];
        let score = if bl.contains("type=\"file\"") || bl.contains("type='file'") {
            3u8
        } else if bl.contains("takeupload") {
            2
        } else if bl.contains("upload") {
            1
        } else {
            0
        };
        if score > 0 && best.map(|(_, _, s)| score > s).unwrap_or(true) {
            best = Some((start, close, score));
        }
        i = open_end;
    }
    best.map(|(s, e, _)| &page[s..e])
}

/// 解析上传表单页：hidden 透传 + select 默认值（模拟浏览器首轮提交）。
/// 只看上传表单块内（导航搜索表单的 search_area 等不掺进来）。
pub fn parse_form_fields(page: &str) -> Vec<Field> {
    let page = upload_form_block(page).unwrap_or(page);
    let mut fields: Vec<Field> = Vec::new();
    // hidden inputs（to_ascii_lowercase 找偏移、切原串：字节长度一致才安全）
    let lower = page.to_ascii_lowercase();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<input") {
        let start = i + p;
        let end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(page.len());
        let tag = &page[start..end];
        let tl = tag.to_ascii_lowercase();
        if tl.contains("type=\"hidden\"") || tl.contains("type='hidden'") {
            if let Some(name) = attr(tag, "name") {
                let value = attr(tag, "value").unwrap_or_default();
                if !fields.iter().any(|(n, _)| *n == name) {
                    fields.push((name, value));
                }
            }
        }
        i = end;
    }
    // select 默认值（selected 项，无则首项）。带 [n] 下标的质量下拉
    // （medium_sel[4] 一类）浏览器同样会提交，这里一并透传
    let mut rest = page;
    while let Some(p) = rest.to_ascii_lowercase().find("<select") {
        let after = &rest[p..];
        let close = after
            .to_ascii_lowercase()
            .find("</select>")
            .unwrap_or(after.len());
        let block = &after[..close.min(after.len())];
        if let Some(name) = attr(block, "name") {
            if !fields.iter().any(|(n, _)| *n == name) {
                let value = select_default(block);
                fields.push((name, value));
            }
        }
        rest = &after[(close + 9).min(after.len())..];
    }
    fields
}

fn select_default(block: &str) -> String {
    let mut first: Option<String> = None;
    let mut selected: Option<String> = None;
    let mut rest = block;
    while let Some(p) = rest.to_ascii_lowercase().find("<option") {
        let after = &rest[p..];
        let end = after.find('>').map(|j| j + 1).unwrap_or(after.len());
        let tag = &after[..end];
        let v = attr(tag, "value").unwrap_or_default();
        if first.is_none() {
            first = Some(v.clone());
        }
        if tag.to_ascii_lowercase().contains("selected") {
            selected = Some(v);
            break;
        }
        rest = &after[end..];
    }
    selected.or(first).unwrap_or_default()
}

fn attr(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let key = format!("{name}={quote}");
        if let Some(p) = tag.find(&key) {
            let rest = &tag[p + key.len()..];
            let end = rest.find(quote)?;
            return Some(decode_entities(&rest[..end]));
        }
    }
    None
}

/// 业务字段：在 hidden/select 默认值之上覆盖标题/副标题/简介/MediaInfo/链接/标签，
/// 并按发布名做选档回填（分类/媒介/编码/音频/分辨率）。返回
/// (提交字段, 实际勾选的标签值, 选档提示)。
pub fn build_upload_fields(
    info: &ReleaseInfo,
    page: &str,
    tags_wanted: &[&str],
) -> (Vec<Field>, Vec<String>, Vec<String>) {
    let mut fields = parse_form_fields(page);

    // 标题 / 副标题（Gazelle 系如 dicmusic 用 title/subtitle）
    let (title_field, title_val) = if form_has(page, "name") {
        ("name", info.name.clone())
    } else if form_has(page, "title") {
        ("title", info.name.clone())
    } else {
        ("", String::new())
    };
    if !title_field.is_empty() {
        set_field(&mut fields, title_field, title_val);
    }
    if !info.subtitle.is_empty() {
        if form_has(page, "small_descr") {
            set_field(&mut fields, "small_descr", info.subtitle.clone());
        } else if form_has(page, "subtitle") {
            set_field(&mut fields, "subtitle", info.subtitle.clone());
        }
    }

    // MediaInfo 独立文本域（新版 NexusPHP）；没有就内嵌简介
    let media_field = ["technical_info", "media_info", "media_bdinfo", "mediainfo"]
        .iter()
        .find(|f| form_has(page, f))
        .copied();
    let embed_media = match media_field {
        Some(f) if !info.mediainfo.is_empty() => {
            set_field(&mut fields, f, info.mediainfo.clone());
            false
        }
        _ => !info.mediainfo.is_empty(),
    };
    let descr = if embed_media && !info.mediainfo.is_empty() {
        format!(
            "{}\n\n[quote][b]MediaInfo[/b]\n[code]{}[/code][/quote]",
            info.descr.trim_end(),
            info.mediainfo
        )
    } else {
        info.descr.clone()
    };
    // 简介字段：NexusPHP 是 descr；Gazelle 系（dicmusic）是 release_desc
    if let Some(f) = ["descr", "release_desc", "description", "content"]
        .iter()
        .find(|f| form_has(page, f))
        .copied()
    {
        // WAF 实测：简介里的内联事件属性（onload/onclick 等）按出现次数计分，
        // 真实上传同样会被拦（luckpt 实测 HTTP 403）——提交前统一剥掉，正文不动。
        set_field(&mut fields, f, strip_event_handlers(&descr));
    }

    // 条目链接字段
    if let Some(tt) = &info.imdb {
        for f in ["imdbid", "imdb_url", "url"] {
            if form_has(page, f) {
                let v = if f == "imdbid" {
                    tt.clone()
                } else {
                    format!("https://www.imdb.com/title/{tt}/")
                };
                set_field(&mut fields, f, v);
                break;
            }
        }
    }
    if let Some(db) = &info.douban {
        for f in ["douban", "douban_id", "douban_url"] {
            if form_has(page, f) {
                let v = if f.ends_with("url") {
                    format!("https://movie.douban.com/subject/{db}/")
                } else {
                    db.clone()
                };
                set_field(&mut fields, f, v);
                break;
            }
        }
    }

    // 标签：checkbox 组（tags[] 一类）按文案匹配
    let mut picked: Vec<String> = Vec::new();
    for want in tags_wanted {
        for (name, value) in match_checkboxes(page, want) {
            if !fields.iter().any(|(n, v)| n == &name && v == &value) {
                fields.push((name, value.clone()));
                picked.push(value);
            }
        }
    }

    // 匿名发布默认 no（TTG 等必填）
    if page.to_lowercase().contains("<select")
        && page.contains("name=\"anonymity\"")
        && !fields.iter().any(|(n, _)| n == "anonymity")
    {
        set_field(&mut fields, "anonymity", "no".into());
    }
    // 选档回填：分类/媒介/编码/音频/分辨率（词元唯一配对才填）
    let notes = fill_selects(&mut fields, page, info);
    (fields, picked, notes)
}

/// 站点专属的静态字段补齐。tjupt 的剧集分类在 `showcategorydetail.php?catid=402`
/// 动态插入 `cname` / `ename` / `specificcat`，上传页静态 HTML 里没有这三个控件；
/// 只提交通用字段会被 takeupload.php 以“上传失败！”拒绝。
pub fn build_upload_fields_for_site(
    info: &ReleaseInfo,
    page: &str,
    tags_wanted: &[&str],
    site_id: &str,
) -> (Vec<Field>, Vec<String>, Vec<String>) {
    let (mut fields, picked, notes) = build_upload_fields(info, page, tags_wanted);
    if site_id == "tjupt" {
        fill_tjupt_series_fields(&mut fields, info, page);
    }
    if site_id == "ubits" {
        fill_ubits_quality_fields(&mut fields, info, page);
    }
    (fields, picked, notes)
}

/// UBits 的 `source_sel`（地区）和 `team_sel`（制作组）没有默认值，
/// 通用选档只填媒介/编码/音频/分辨率；不补这两项会被 takeupload 拒绝：
/// “一个或多个质量区域未选择”。
fn fill_ubits_quality_fields(fields: &mut Vec<Field>, info: &ReleaseInfo, _page: &str) {
    let hay = format!("{} {} {}", info.subtitle, info.descr, info.name);
    let region = if hay.contains("中国大陆") || hay.contains("大陆") {
        "1"
    } else if hay.contains("香港") {
        "2"
    } else if hay.contains("台湾") {
        "3"
    } else if hay.contains("日本") {
        "5"
    } else if hay.contains("韩国") {
        "6"
    } else if hay.contains("泰国") {
        "7"
    } else {
        "11" // 其它
    };
    if fields.iter().any(|(n, _)| n == "source_sel[4]") {
        set_field(fields, "source_sel[4]", region.to_string());
    }
    if fields.iter().any(|(n, _)| n == "team_sel[4]") {
        // 源组不是 UBits 自家组，按 Other 提交，避免冒充驻站组。
        set_field(fields, "team_sel[4]", "5".to_string());
    }
}

fn fill_tjupt_series_fields(fields: &mut Vec<Field>, info: &ReleaseInfo, _page: &str) {
    let type_val = fields
        .iter()
        .find(|(n, _)| n == "type")
        .map(|(_, v)| v.clone());
    let Some(type_val) = type_val else {
        return;
    };
    if type_val != "402" {
        return;
    }
    // 剧集必填：中文名、英文名、剧集类型。
    if let Some(cname) = first_bracket_name(&info.subtitle).or_else(|| {
        info.descr
            .split(['●', '◎', '\n'])
            .find(|s| s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }) {
        set_field(fields, "cname", cname);
    }
    if !info.name.is_empty() {
        set_field(fields, "ename", info.name.clone());
    }
    const SERIES_REGIONS: &[&str] = &["大陆", "港台", "美剧", "日剧", "韩剧", "英剧", "泰剧"];
    let hay = format!("{} {} {}", info.subtitle, info.descr, info.name);
    if let Some(region) = SERIES_REGIONS.iter().find(|r| hay.contains(**r)) {
        set_field(fields, "specificcat", (*region).to_string());
        // 页面上的复选框会同时提交同名 value；服务端两种都认。
        if !fields.iter().any(|(n, _)| n == region) {
            fields.push(((*region).to_string(), (*region).to_string()));
        }
    } else {
        // 页面有该必填项但内容没线索时留空会造成静默退回，直接用“其他”。
        set_field(fields, "specificcat", "其他".to_string());
    }
}

fn first_bracket_name(s: &str) -> Option<String> {
    for (open, close) in [('【', '】'), ('[', ']'), ('《', '》')] {
        if let Some(start) = s.find(open) {
            let body_start = start + open.len_utf8();
            if let Some(end) = s[body_start..].find(close) {
                let name = s[body_start..body_start + end].trim();
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

/// 覆盖式设置字段：同名先删再推
fn set_field(fields: &mut Vec<Field>, name: &str, value: String) {
    fields.retain(|(n, _)| n != name);
    fields.push((name.to_string(), value));
}

/// 表单控件里有没有这个名字的字段（input/textarea/select 的 name 属性）。
/// 只认真控件：`<meta name="description">` 与 JS 里的同名字符串不算（hdcity 实测踩过）。
fn form_has(page: &str, name: &str) -> bool {
    let lower = page.to_ascii_lowercase();
    for tag_name in ["<input", "<textarea", "<select"] {
        let mut i = 0;
        while let Some(p) = lower[i..].find(tag_name) {
            let start = i + p;
            let end = lower[start..]
                .find('>')
                .map(|j| start + j + 1)
                .unwrap_or(page.len());
            if attr(&page[start..end], "name").as_deref() == Some(name) {
                return true;
            }
            i = end;
        }
    }
    false
}

// ---------------- 选档映射（发布名/副标题 → 分类与质量下拉） ----------------

/// 媒介词元（最长优先；重叠的短词自动跳过：WEB-DL 吃掉 WEB）
const MEDIUM_KEYWORDS: &[&str] = &[
    "UHD Blu-ray",
    "UHD BluRay",
    "Blu-ray REMUX",
    "BluRay REMUX",
    "WEB-DL",
    "WEBRip",
    "Blu-ray",
    "BluRay",
    "BDRip",
    "BRRip",
    "DVDRip",
    "HDTV",
    "REMUX",
    "WEB",
    "UHD",
    "DVD",
];

const CODEC_KEYWORDS: &[&str] = &[
    "H.265", "H.264", "H265", "H264", "x265", "x264", "HEVC", "AVC", "XviD", "DivX", "AV1", "VC-1",
    "VC1", "MPEG-2", "MPEG2", "MPEG4", "VP9",
];

const AUDIO_KEYWORDS: &[&str] = &[
    "DTS-HD MA",
    "DTS-HD",
    "DTS-X",
    "TrueHD",
    "E-AC-3",
    "DD+",
    "DDP",
    "EAC3",
    "EC3",
    "Atmos",
    "AC-3",
    "FLAC",
    "LPCM",
    "ALAC",
    "AAC",
    "AC3",
    "DTS",
    "APE",
    "WAV",
    "MP3",
    "Opus",
    "DD",
];

const STANDARD_KEYWORDS: &[&str] = &[
    "3840x2160",
    "1920x1080",
    "1280x720",
    "2160p",
    "1080p",
    "1080i",
    "720p",
    "576p",
    "480p",
    "8K",
    "4K",
];

const PROCESSING_KEYWORDS: &[&str] =
    &["REMUX", "REPACK", "PROPER", "INTERNAL", "UNCUT", "RESTORED"];

/// 发布名里的词元：最长优先贪心去重叠，带词边界（大小写不敏感）
fn find_keywords(hay: &str, keywords: &[&str]) -> Vec<String> {
    let h = hay.to_ascii_lowercase();
    let mut hits: Vec<(usize, &str)> = Vec::new();
    for kw in keywords {
        let k = kw.to_ascii_lowercase();
        let mut from = 0;
        while let Some(p) = h[from..].find(&k) {
            let at = from + p;
            let end = at + k.len();
            let pre_ok = at == 0 || !h.as_bytes()[at - 1].is_ascii_alphanumeric();
            let post_ok = end >= h.len()
                || !h.as_bytes()[end].is_ascii_alphanumeric()
                // 音频码常紧跟声道：DDP5.1 / DTS5.1 / AAC2.0
                || h.as_bytes()[end].is_ascii_digit();
            if pre_ok && post_ok {
                hits.push((at, kw));
                from = end;
            } else {
                from = at + 1;
            }
        }
    }
    // 长词优先，位置次之；与已取区间重叠的短词跳过
    hits.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for (at, kw) in hits {
        let end = at + kw.len();
        if taken.iter().any(|(s, e)| at < *e && end > *s) {
            continue;
        }
        taken.push((at, end));
        // 同一关键词出现多次也只算一个词元
        if !out.iter().any(|k| k.eq_ignore_ascii_case(kw)) {
            out.push(kw.to_string());
        }
    }
    out
}

/// 规范化词元：小写 + 只留 ASCII 字母数字（`WEB-DL` → `webdl`；标签带中文尾缀也全等）
fn norm_token(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// 词元 → 选项的配对结论
enum Pairing {
    /// 唯一命中：选项 value
    Hit(String),
    /// 多个候选（含同名重复项）：不猜
    Ambiguous,
    /// 与选项无关
    Miss,
}

/// 在选项里找词元：规范化全等优先，其次标签里的分隔子串；命中多个不猜
fn pair_option(options: &[(String, String)], token: &str) -> Pairing {
    let tn = norm_token(token);
    if !tn.is_empty() {
        let exact: Vec<&(String, String)> = options
            .iter()
            .filter(|(_, l)| norm_token(l) == tn)
            .collect();
        if exact.len() == 1 {
            return Pairing::Hit(exact[0].0.clone());
        }
        if exact.len() > 1 {
            return Pairing::Ambiguous;
        }
    }
    // 分隔子串（中文词元没有 ASCII 边界概念，子串即命中）
    let tl = token.to_ascii_lowercase();
    let ascii_kw = tl
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b' ' || b == b'-' || b == b'.' || b == b'+');
    let sub: Vec<&(String, String)> = options
        .iter()
        .filter(|(_, l)| {
            let ll = l.to_ascii_lowercase();
            let mut from = 0;
            while let Some(p) = ll[from..].find(&tl) {
                let at = from + p;
                let end = at + tl.len();
                if !ascii_kw {
                    return true;
                }
                let pre_ok = at == 0 || !ll.as_bytes()[at - 1].is_ascii_alphanumeric();
                let post_ok = end >= ll.len() || !ll.as_bytes()[end].is_ascii_alphanumeric();
                if pre_ok && post_ok {
                    return true;
                }
                from = at + 1;
            }
            false
        })
        .collect();
    if sub.len() == 1 {
        return Pairing::Hit(sub[0].0.clone());
    }
    // 同名重复项（两个 AAC 一类）不猜
    let dup_label = sub.iter().any(|o| {
        sub.iter()
            .any(|x| x.1 != o.1 && norm_token(x.1.as_str()) == norm_token(o.1.as_str()))
    });
    if dup_label {
        return Pairing::Ambiguous;
    }
    if sub.is_empty() {
        return Pairing::Miss;
    }
    // 嵌套标签：一个规范化标签包含另一个（如「UHD Blu-ray」含「Blu-ray」），
    // 取最具体的最长标签；互不包含的并列候选仍判歧义，不猜。
    let nested = sub.iter().find(|o| {
        let on = norm_token(o.1.as_str());
        sub.iter().all(|x| {
            let xn = norm_token(x.1.as_str());
            x.1 == o.1 || (on != xn && on.contains(&xn))
        })
    });
    match nested {
        Some(o) => Pairing::Hit(o.0.clone()),
        None => Pairing::Ambiguous,
    }
}

fn match_option(options: &[(String, String)], token: &str) -> Option<String> {
    match pair_option(options, token) {
        Pairing::Hit(v) => Some(v),
        _ => None,
    }
}

/// 选项标签与词元是否有关系（规范化全等或任一方向的子串）：域判定用，宁可偏宽
fn label_relates(label: &str, token: &str) -> bool {
    let ln = norm_token(label);
    let tn = norm_token(token);
    if !ln.is_empty() && ln == tn {
        return true;
    }
    let ll = label.to_ascii_lowercase();
    let tl = token.to_ascii_lowercase();
    if tl.is_empty() || ll.is_empty() {
        return false;
    }
    ll.contains(&tl) || tl.contains(&ll)
}

/// 选项集是否属于该词表的域：防「source_sel 是地区」「audiocodec_sel 是电子书格式」
/// 这类同名不同义的下拉被误配/误报
fn options_in_domain(options: &[(String, String)], kws: &[&str]) -> bool {
    options.iter().any(|(_, l)| {
        kws.iter().any(|kw| {
            std::iter::once(*kw)
                .chain(token_aliases(kw).iter().copied())
                .any(|t| label_relates(l, t))
        })
    })
}

/// `source_sel` 的选项是不是「大类-子类」二级分类（ptsbao 实测：电影-WEB-DL /
/// 剧集-美剧 / 音乐-无损）。按‘-’前的中文大类判断，至少两个不同大类才算；
/// 这样 `Blu-ray` 的 `Blu-ray` 前缀、`UHD Blu-ray` 不会被误认成分类。
fn looks_like_secondary_category(options: &[(String, String)]) -> bool {
    const CATEGORY_PREFIXES: &[&str] = &[
        "电影",
        "剧集",
        "电视剧",
        "音乐",
        "动漫",
        "动画",
        "综艺",
        "纪录片",
        "体育",
        "游戏",
        "软件",
        "其他",
        "其它",
        "书籍",
        "期刊",
        "教育",
        "学习",
        "资料",
        "写真",
        "禁片",
        "MV",
        "晚会",
        "讲座",
        "个人",
    ];
    let mut prefixes = std::collections::BTreeSet::new();
    for (_, label) in options {
        let Some((prefix, _)) = label.split_once('-') else {
            continue;
        };
        let prefix = prefix.trim();
        if CATEGORY_PREFIXES.contains(&prefix) {
            prefixes.insert(prefix.to_string());
        }
    }
    prefixes.len() >= 2
}

/// 词元直配不上时的别名回退（同一制式的不同写法；含实测站点的简写与中文标签）
fn token_aliases(token: &str) -> &'static [&'static str] {
    match norm_token(token).as_str() {
        "x264" | "h264" | "avc" => &["x264", "H.264", "AVC", "H264"],
        "x265" | "h265" | "hevc" => &["x265", "H.265", "HEVC", "H265"],
        "2160p" | "4k" | "uhd" => &["2160p", "2160", "4K", "UHD"],
        // ptsbao 实测分辨率选项是 480/576/720/1080/2K/4K 裸数字
        "1080p" => &["1080p", "1920x1080", "1080"],
        "1080i" => &["1080i", "1920x1080", "1080"],
        "720p" => &["720p", "1280x720", "720"],
        "576p" => &["576p", "576"],
        "480p" => &["480p", "480"],
        // aling/ptcafe 实测媒介选项写「TV」，繁体站写「電視」
        "hdtv" => &["TV", "TVRip", "HDTVRip", "Television", "电视", "電視"],
        "eac3" | "ddp" | "dd+" | "dd" | "ec3" => &[
            "DD+",
            "E-AC-3",
            "EAC3",
            "EC3",
            "DDP",
            "DDP/E-AC3",
            "DDP/EAC3",
            "DDP Atmos/EAC3",
        ],
        // AC3 不能靠子串：会同时命中 DD/AC3 与 DDP/E-AC3
        "ac3" => &["DD/AC3", "DD/AC-3", "AC3"],
        "truehd" => &["TrueHD", "TrueHD Atmos", "TrueHD_Atmos"],
        "dtshdma" => &["DTS-HD MA", "DTS-HD", "DTS_HD|MA", "DTS"],
        "dtshd" => &["DTS-HD", "DTS_HD|HR", "DTS"],
        "webdl" => &["WEB-DL", "WEB"],
        "webrip" => &["WEBRip", "WEB"],
        "bluray" => &["BluRay", "Blu-ray"],
        "uhdbluray" => &["UHD Blu-ray", "UHD BluRay"],
        // 原盘 REMUX：站点有 Remux 选项时归 Remux，不归 Blu-ray
        "blurayremux" | "remux" => &["Remux", "REMUX"],
        _ => &[],
    }
}

/// UHD + Blu-ray 同时出现时合成「UHD Blu-ray」复合词元，优先命中最具体的
/// UHD 蓝光标签（而不是让 UHD/Blu-ray 各自子串命中多个标签判歧义留空）。
fn compound_medium_token(tokens: &[String]) -> Option<String> {
    let has_uhd = tokens
        .iter()
        .any(|t| t.to_ascii_lowercase().contains("uhd"));
    let has_bd = tokens.iter().any(|t| {
        let l = t.to_ascii_lowercase();
        l.contains("bluray") || l.contains("blu-ray") || l.contains("bdrip") || l.contains("brrip")
    });
    (has_uhd && has_bd).then(|| "UHD Blu-ray".to_string())
}

/// `<select name=field>` 的全部 (value, label)；跳过 value 空/0 的占位项。
/// 同名多块（分类分支的第二套 type）合并读——选项 value 是全局分类 id，合并配对安全。
fn select_options(page: &str, field: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for block in select_blocks(page, field) {
        let mut rest = block;
        while let Some(p) = rest.to_ascii_lowercase().find("<option") {
            let after = &rest[p..];
            let end = after.find('>').map(|j| j + 1).unwrap_or(after.len());
            let tag = &after[..end];
            let v = attr(tag, "value").unwrap_or_default();
            let text_end = after[end..]
                .find('<')
                .map(|i| end + i)
                .unwrap_or(after.len());
            let label = strip_tags(&after[end..text_end]).trim().to_string();
            if !v.is_empty() && v != "0" {
                out.push((v, label));
            }
            rest = &after[end..];
        }
    }
    out
}

/// 表单里全部 `<select name=…>` 的名字（演练核对缺口用；只看上传表单块）
fn select_names(page: &str) -> Vec<String> {
    let page = upload_form_block(page).unwrap_or(page);
    let lower = page.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<select") {
        let start = i + p;
        let end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(page.len());
        if let Some(name) = attr(&page[start..end], "name") {
            if !out.contains(&name) {
                out.push(name);
            }
        }
        i = end;
    }
    out
}

/// 演练选项表：上传表单每个下拉的全量 (value, label)（供 formcheck.json 落盘）。
/// 只看上传表单块；导航搜索表单的 search_area 一类不掺进来。
pub fn drill_select_options(page: &str) -> Vec<(String, Vec<(String, String)>)> {
    select_names(page)
        .into_iter()
        .map(|name| {
            let opts = select_options(page, &name);
            (name, opts)
        })
        .filter(|(_, opts)| !opts.is_empty())
        .collect()
}

/// 前缀匹配 select 字段名：`medium_sel` 或 `medium_sel[4]`
/// 所有匹配前缀的下拉名：NexusPHP 参数化质量下拉按分类分组会出现多套
/// （`medium_sel[4]` 视频分支与 `medium_sel[9]` AV 分支要各自独立配对）
fn select_field_names(fields: &[Field], prefix: &str) -> Vec<String> {
    fields
        .iter()
        .map(|(n, _)| n)
        .filter(|n| *n == prefix || (n.starts_with(prefix) && n[prefix.len()..].starts_with('[')))
        .cloned()
        .collect()
}

/// 发布组：名尾 `-GROUP`（`x264-JING` → JING）；常见非组词缀剔除
fn release_group(name: &str) -> Option<String> {
    const NOT_GROUP: &[&str] = &[
        "10bit", "8bit", "hdr", "hdr10", "dv", "remux", "repack", "proper", "complete", "internal",
        "uncut", "restored", "subs", "multi", "dual",
    ];
    let p = name.rfind('-')?;
    let g = &name[p + 1..];
    if !(2..=20).contains(&g.len()) || !g.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if NOT_GROUP.iter().any(|x| x.eq_ignore_ascii_case(g)) {
        return None;
    }
    Some(g.to_string())
}

/// 大小写不敏感的标记命中：拉丁词要求词边界，CJK 子串即命中
fn marker_contains(hay_lower: &str, marker: &str) -> bool {
    let m = marker.to_ascii_lowercase();
    let ascii_kw = m
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b' ' || b == b'-' || b == b'.');
    let mut from = 0;
    while let Some(p) = hay_lower[from..].find(&m) {
        let at = from + p;
        let end = at + m.len();
        if !ascii_kw {
            return true;
        }
        let pre_ok = at == 0 || !hay_lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let post_ok = end >= hay_lower.len() || !hay_lower.as_bytes()[end].is_ascii_alphanumeric();
        if pre_ok && post_ok {
            return true;
        }
        from = at + 1;
    }
    false
}

/// 集数/季标记：S01 / E01 / 全41集 / 第N季
fn has_episode_marker(hay: &str) -> bool {
    let b = hay.as_bytes();
    for i in 0..b.len() {
        let c = b[i] | 0x20;
        if c == b's' || c == b'e' {
            if i > 0 && b[i - 1].is_ascii_alphanumeric() {
                continue;
            }
            if i + 2 < b.len()
                && b[i + 1].is_ascii_digit()
                && b[i + 2].is_ascii_digit()
                && (i + 3 >= b.len() || !b[i + 3].is_ascii_alphanumeric())
            {
                return true;
            }
        }
    }
    for (i, c) in hay.char_indices() {
        if c == '全' || c == '第' {
            let rest = &hay[i + c.len_utf8()..];
            let digits: String = rest.chars().take_while(|x| x.is_ascii_digit()).collect();
            if !digits.is_empty()
                && (rest[digits.len()..].starts_with('集')
                    || rest[digits.len()..].starts_with('季'))
            {
                return true;
            }
        }
    }
    false
}

/// 分类推断：返回 (依次尝试的选项短语, 类别同义词兜底)。没把握返回 None——不猜。
fn infer_type_candidates(
    name: &str,
    subtitle: &str,
) -> Option<(Vec<&'static str>, &'static [&'static str])> {
    const TV: &[&str] = &[
        "电视剧",
        "剧集",
        "连续剧",
        "電視劇",
        "TV Series",
        "TVSeries",
        "TV",
        "Drama",
    ];
    const MOVIE: &[&str] = &["电影", "影片", "Movie", "Film"];
    const ANIME: &[&str] = &["动画", "动漫", "Anime", "Animation"];
    const DOC: &[&str] = &["纪录", "Documentary"];
    const VARIETY: &[&str] = &["综艺", "Variety", "真人秀", "脱口秀"];
    const MUSIC: &[&str] = &["音乐", "MV", "Concert", "演唱会"];
    const SPORT: &[&str] = &["体育", "Sport", "赛事"];
    // (名里的标记, 选项短语, 类别同义词)：短语唯一命中优先，类别词唯一命中兜底
    const MARKERS: &[(&str, &[&str], &[&str])] = &[
        ("国产剧", &["国产剧", "华语剧"], TV),
        ("港台剧", &["港台剧", "港剧", "台剧"], TV),
        ("国产电影", &["国产电影", "华语电影", "大陆电影"], MOVIE),
        ("电视剧", &["电视剧", "剧集"], TV),
        ("美剧", &["美剧"], TV),
        ("日剧", &["日剧"], TV),
        ("韩剧", &["韩剧"], TV),
        ("英剧", &["英剧"], TV),
        ("泰剧", &["泰剧"], TV),
        ("tv series", &["电视剧", "剧集"], TV),
        ("tvshow", &["电视剧", "剧集"], TV),
        ("纪录片", &["纪录片", "纪录"], DOC),
        ("documentary", &["纪录片", "纪录"], DOC),
        ("国家地理", &["纪录片", "纪录"], DOC),
        ("剧场版", &["剧场版"], ANIME),
        ("动漫", &["动漫", "动画"], ANIME),
        ("动画", &["动漫", "动画"], ANIME),
        ("anime", &["动漫", "动画"], ANIME),
        ("ova", &["动漫", "动画"], ANIME),
        ("综艺", &["综艺"], VARIETY),
        ("脱口秀", &["综艺"], VARIETY),
        ("真人秀", &["综艺"], VARIETY),
        ("variety", &["综艺"], VARIETY),
        ("演唱会", &["演唱会", "音乐", "MV"], MUSIC),
        ("concert", &["演唱会", "音乐", "MV"], MUSIC),
        ("体育", &["体育"], SPORT),
        ("nba", &["体育"], SPORT),
        ("ufc", &["体育"], SPORT),
        ("movie", &["电影"], MOVIE),
        ("film", &["电影"], MOVIE),
        ("剧情片", &["电影"], MOVIE),
        ("电影", &["电影"], MOVIE),
    ];
    let h = format!("{name} {subtitle}").to_ascii_lowercase();
    for (marker, phrases, syns) in MARKERS {
        if marker_contains(&h, marker) {
            return Some((phrases.to_vec(), syns));
        }
    }
    if has_episode_marker(&h) {
        return Some((vec!["电视剧", "剧集"], TV));
    }
    // 电影惯用命名：`Title.1998.1080p.BluRay...`——有年份、无集数/季标记。
    // 属强信号但仍只是推断：官方预审与站点规范门禁是最终闸门。
    if has_release_year(&h) {
        return Some((vec!["电影", "Movie"], MOVIE));
    }
    None
}

/// 发布名里是否带 19xx/20xx 年份（要求两侧非数字）
fn has_release_year(h: &str) -> bool {
    let b = h.as_bytes();
    for i in 0..b.len().saturating_sub(3) {
        if !b[i].is_ascii_digit()
            || !b[i + 1].is_ascii_digit()
            || !b[i + 2].is_ascii_digit()
            || !b[i + 3].is_ascii_digit()
        {
            continue;
        }
        if i > 0 && b[i - 1].is_ascii_digit() {
            continue;
        }
        if i + 4 < b.len() && b[i + 4].is_ascii_digit() {
            continue;
        }
        let y = (b[i] - b'0') as u32 * 1000
            + (b[i + 1] - b'0') as u32 * 100
            + (b[i + 2] - b'0') as u32 * 10
            + (b[i + 3] - b'0') as u32;
        if (1900..=2099).contains(&y) {
            return true;
        }
    }
    false
}

/// 样本类型在该站分类里没有对应项（垂直站，如曲艺/有声书/音乐站）：
/// 留空不算表单缺口。推断不出样本类型不算垂直站——那是名字信息不足，仍是硬缺口。
fn type_niche_miss(options: &[(String, String)], info: &ReleaseInfo) -> bool {
    let Some((_, syns)) = infer_type_candidates(&info.name, &info.subtitle) else {
        return false;
    };
    !options
        .iter()
        .any(|(_, l)| syns.iter().any(|s| label_relates(l, s)))
}

/// 上传表单是 JS 自定义界面：只有种子文件与隐藏字段，没有可静态回填的文本框
/// （hdcity 实测：name 都是 hidden，简介/MediaInfo 由网页 JS 延后生成）
fn js_driven_form(form: &str) -> bool {
    let lower = form.to_ascii_lowercase();
    if !(lower.contains("type=\"file\"") || lower.contains("type='file'")) {
        return false;
    }
    if lower.contains("<textarea") {
        return false;
    }
    const SKIP: &[&str] = &[
        "type=\"hidden\"",
        "type='hidden'",
        "type=\"file\"",
        "type='file'",
        "type=\"submit\"",
        "type='submit'",
        "type=\"button\"",
        "type='button'",
        "type=\"image\"",
        "type='image'",
        "type=\"reset\"",
        "type='reset'",
        "type=\"checkbox\"",
        "type='checkbox'",
        "type=\"radio\"",
        "type='radio'",
    ];
    let mut i = 0;
    while let Some(p) = lower[i..].find("<input") {
        let start = i + p;
        let end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(form.len());
        let tag = &lower[start..end];
        if !SKIP.iter().any(|s| tag.contains(s)) {
            return false;
        }
        i = end;
    }
    true
}

/// MediaInfo 文本里的视频宽/高（`Width : 1 290 pixels`；只认第一个 Width/Height）
fn mediainfo_dimension(mediainfo: &str, key: &str) -> Option<u32> {
    mediainfo.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        if !k.trim().eq_ignore_ascii_case(key) {
            return None;
        }
        let digits: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    })
}

/// 由 MediaInfo 宽高反推站点标准分辨率档；BDRip 常裁掉黑边（1290x696），
/// 用长边分档比按高度更稳（1920x800 仍是 1080p）。
fn resolution_from_mediainfo(mediainfo: &str) -> Option<&'static str> {
    let w = mediainfo_dimension(mediainfo, "Width")?;
    let h = mediainfo_dimension(mediainfo, "Height")?;
    let long = w.max(h);
    Some(match long {
        3200.. => "2160p",
        2200..=3199 => "1440p",
        1600..=2199 => "1080p",
        1100..=1599 => "720p",
        900..=1099 => "576p",
        600..=899 => "480p",
        _ => return None,
    })
}

fn pick_resolution_option(options: &[(String, String)], hay: &str) -> Option<String> {
    let low = hay.to_ascii_lowercase();
    let res = [
        "2160p", "1080p", "1080i", "720p", "576p", "480p", "4320p", "8k", "4k",
    ]
    .iter()
    .find(|r| low.contains(**r))?;
    let fps = [
        "120fps", "60fps", "50fps", "48fps", "30fps", "25fps", "24fps",
    ]
    .iter()
    .find(|f| low.contains(**f));
    let candidates: Vec<&(String, String)> = options
        .iter()
        .filter(|(_, label)| label.to_ascii_lowercase().contains(res))
        .collect();
    if let Some(fps) = fps {
        if let Some((value, _)) = candidates
            .iter()
            .find(|(_, label)| label.to_ascii_lowercase().contains(fps))
        {
            return Some(value.clone());
        }
    }
    // 没写帧率时优先不带帧率的普通档，避免默认落到 60Fps/120Fps。
    if let Some((value, _)) = candidates.iter().find(|(_, label)| {
        let ll = label.to_ascii_lowercase();
        ![
            "120fps", "60fps", "50fps", "48fps", "30fps", "25fps", "24fps",
        ]
        .iter()
        .any(|f| ll.contains(f))
    }) {
        return Some(value.clone());
    }
    candidates.first().map(|(value, _)| value.clone())
}

/// 选档回填：发布名/副标题词元 → 分类与质量下拉。只动当前值为 0/空的下拉，
/// 词元与选项唯一配对才填；有词元配不上、或推断不唯一，留提示不猜。
/// 同前缀多组下拉（`medium_sel[4]`/`medium_sel[9]`）各自独立配对全填。
/// 返回提示清单（审阅/演练里展示）。
fn fill_selects(fields: &mut Vec<Field>, page: &str, info: &ReleaseInfo) -> Vec<String> {
    let mut notes = Vec::new();
    let hay = format!("{} {}", info.name, info.subtitle);
    let audio_hay = format!("{} {}", hay, info.mediainfo);
    // source_sel 在不同站分别是来源媒介或地区，域无关的下拉自动跳过不误报
    let cats: &[(&str, &[&str])] = &[
        ("source_sel", MEDIUM_KEYWORDS),
        ("medium_sel", MEDIUM_KEYWORDS),
        ("codec_sel", CODEC_KEYWORDS),
        ("audiocodec_sel", AUDIO_KEYWORDS),
        ("standard_sel", STANDARD_KEYWORDS),
        ("processing_sel", PROCESSING_KEYWORDS),
    ];
    let codec_tokens = find_keywords(&hay, CODEC_KEYWORDS);
    // 只有 x264/x265 明确是重编码；AVC/HEVC 也可能是原盘（Bitter Moon 实测：
    // Blu-ray AVC DTS-HD MA 标 Encode 被预审判「媒介与标题不匹配」）。
    let reencoded = codec_tokens
        .iter()
        .any(|t| matches!(t.to_ascii_lowercase().as_str(), "x264" | "x265"));
    for (prefix, kws) in cats {
        let names = select_field_names(fields, prefix);
        if names.is_empty() {
            continue;
        }
        let title_tokens = find_keywords(&hay, kws);
        // 音频：标题里已有码（DTS/DD+/AAC…）就以标题为准；不要再把 MediaInfo
        // 的多音轨混进来，否则会因多个候选而留空（国际市场/玻璃樽实测）。
        let mut tokens = if *prefix == "audiocodec_sel" && title_tokens.is_empty() {
            find_keywords(&audio_hay, kws)
        } else {
            title_tokens
        };
        // 分辨率：标题/副标题常只有 BDRip、没有 1080p/720p。此时从 MediaInfo
        // 的 Width/Height 反推标准档（BDRip 1290x696 → 720p），不凭标题猜。
        if *prefix == "standard_sel" && tokens.is_empty() {
            if let Some(r) = resolution_from_mediainfo(&info.mediainfo) {
                tokens.push(r.to_string());
            }
        }
        if matches!(*prefix, "source_sel" | "medium_sel") {
            if let Some(c) = compound_medium_token(&tokens) {
                tokens.insert(0, c);
                // 短语命中后重叠规则会吃掉独立部件词；补回 BluRay/UHD 部件，
                // 便于站点没有「UHD Blu-ray」标签时回退到普通蓝光标签。
                for part in ["BluRay", "UHD"] {
                    if !tokens.iter().any(|t| t.eq_ignore_ascii_case(part)) {
                        tokens.push(part.to_string());
                    }
                }
            }
        }
        let mut relevant_seen = false;
        let mut filled_seen = false;
        let mut ambiguous_tok: Option<String> = None;
        for fname in &names {
            let cur = field_lookup(fields, fname).unwrap_or("").to_string();
            if !cur.is_empty() && cur != "0" {
                filled_seen = true;
                continue;
            }
            let options = select_options(page, fname);
            if options.is_empty() {
                continue;
            }
            // ptsbao 实测：source_sel 不是片源，是「剧集-美剧 / 电影-WEB-DL」二级分类。
            // 它的选项里也含 WEB-DL/Blu-ray 字样，按片源匹配会错填到「电影-WEB-DL」；
            // 这里先认出来跳过，等 type 选完再由 fill_secondary_category 按父子关系填。
            if *prefix == "source_sel" && looks_like_secondary_category(&options) {
                continue;
            }
            if !options_in_domain(&options, kws) {
                continue;
            }
            relevant_seen = true;
            let mut picked = if *prefix == "standard_sel" {
                pick_resolution_option(&options, &hay)
            } else {
                None
            };
            // 原盘/光盘源 + 有损重编码 → 站点规范通常归「Encode」而非「Blu-ray 原盘」
            // （luckpt 官方预审实测：x265 10bit 标 Blu-ray 会报「媒介与标题不匹配」）。
            // 只在站点确实提供 Encode 选项时生效；REMUX/HDTV 不受影响。
            let disc_source = tokens.iter().any(|t| {
                let l = t.to_ascii_lowercase();
                l.contains("blu")
                    || l.contains("bluray")
                    || l.contains("bdrip")
                    || l.contains("brrip")
                    || l.contains("dvdrip")
                    || l == "dvd"
            });
            let is_remux = tokens
                .iter()
                .any(|t| t.to_ascii_lowercase().contains("remux"));
            if reencoded && disc_source && !is_remux {
                picked = match_option(&options, "Encode");
            }
            // HDTV.x264/x265 属重编码发布：站点若没有 HDTV/TV 类媒介但有 Encode，
            // 归 Encode 是正确的（ptfans 实测：选项 Other/Encode/Blu-ray/Web-DL/…）。
            // 只在带 x264/x265 且有 Encode 选项时生效；音乐站（无 Encode）不受影响。
            let hdtv_source = tokens
                .iter()
                .any(|t| t.to_ascii_lowercase().contains("hdtv"));
            let has_hdtv_option = options.iter().any(|(_, l)| {
                let ll = l.to_ascii_lowercase();
                ll.contains("hdtv")
                    || ll.contains("tvrip")
                    || ll == "tv"
                    || l.contains("电视")
                    || l.contains("電視")
            });
            if picked.is_none() && reencoded && hdtv_source && !is_remux && !has_hdtv_option {
                picked = match_option(&options, "Encode");
            }
            // WEB-DL/WEBRip 重编码：站点没有 WEB-DL 选项但有 Encode 时归 Encode
            // （oshen 实测：媒介只有 Track/CD/DVDR/HDTV/MiniBD/Encode/Remux/HD DVD/Blu-ray，
            //  WEB-DL 4K 留空会被发布检查拦成「medium_sel 未选」）。
            if picked.is_none() && *prefix == "medium_sel" && !is_remux {
                let web_source = tokens
                    .iter()
                    .any(|t| t.to_ascii_lowercase().starts_with("web"));
                let has_web_option = options
                    .iter()
                    .any(|(_, l)| l.to_ascii_lowercase().contains("web"));
                if web_source && !has_web_option {
                    picked = match_option(&options, "Encode");
                }
            }
            'tok: for t in &tokens {
                if picked.is_some() {
                    break 'tok;
                }
                for c in std::iter::once(t.as_str()).chain(token_aliases(t).iter().copied()) {
                    match pair_option(&options, c) {
                        Pairing::Hit(v) => {
                            picked = Some(v);
                            break 'tok;
                        }
                        Pairing::Ambiguous => {
                            ambiguous_tok.get_or_insert_with(|| t.clone());
                        }
                        Pairing::Miss => {}
                    }
                }
            }
            if let Some(v) = picked {
                set_field(fields, fname, v);
                filled_seen = true;
            }
        }
        // 只有一组都没配对时才点名；歧义与「标签没配对」分开说
        if tokens.is_empty() || filled_seen || !relevant_seen {
            continue;
        }
        if let Some(t) = ambiguous_tok {
            notes.push(format!(
                "{prefix}：「{t}」在该站有多个同名候选选项，留空不猜"
            ));
        } else {
            notes.push(format!(
                "{prefix}：名里有「{}」但选项没配对（该站标签可能用了别的写法）",
                tokens.join("/")
            ));
        }
    }
    // 制作组（名尾 -GROUP）→ team_sel；配不上不响（词尾误判代价低）
    if let Some(group) = release_group(&info.name) {
        for fname in select_field_names(fields, "team_sel") {
            let cur = field_lookup(fields, &fname).unwrap_or("").to_string();
            if cur.is_empty() || cur == "0" {
                let options = select_options(page, &fname);
                if let Some(v) = match_option(&options, &group) {
                    set_field(fields, &fname, v);
                }
            }
        }
    }
    // 分类（同名多块的分类分支合并配；value 是全局分类 id，填进 type 即可）
    let type_cur = field_lookup(fields, "type").unwrap_or("").to_string();
    if type_cur.is_empty() || type_cur == "0" {
        let options = select_options(page, "type");
        if !options.is_empty() {
            let mut picked = None;
            if let Some((phrases, syns)) = infer_type_candidates(&info.name, &info.subtitle) {
                for ph in &phrases {
                    if let Some(v) = match_option(&options, ph) {
                        picked = Some(v);
                        break;
                    }
                }
                // 同义词按序逐个试，第一个「唯一命中」就用（短词如 TV 会同时
                // 命中「TV Series/电视剧」与「TV Shows/综艺」，不能混在一起数）
                if picked.is_none() {
                    'syn: for s in syns {
                        let hits: Vec<&(String, String)> =
                            options.iter().filter(|(_, l)| l.contains(*s)).collect();
                        if hits.len() == 1 {
                            picked = Some(hits[0].0.clone());
                            break 'syn;
                        }
                    }
                }
            }
            match picked {
                Some(v) => {
                    set_field(fields, "type", v);
                }
                None => {
                    if type_niche_miss(&options, info) {
                        notes.push(
                            "分类：样本类型在该站分类里没有对应项（垂直站/分类体系不同），留空需人工判断"
                                .into(),
                        );
                    } else {
                        notes.push(
                            "分类：从名/副标题推断不出来或配不上选项，留空等人工选（官方预审会点名）"
                                .into(),
                        );
                    }
                }
            }
        }
    }
    // 少数站（ptsbao 实测）把 `source_sel` 当二级分类：`剧集-美剧` / `电影-1080`。
    // 它跟 type 的选项文案是父子关系，不能按片源词匹配；type 选好后按标题/副标题归子类。
    if let Some(note) = fill_secondary_category(fields, page, info) {
        notes.push(note);
    }
    notes
}

/// `source_sel` 是二级分类时的子类映射（fulfilled type 后调用）。
/// 只处理选项文案形如 `{type_label}-{子类}` 的下拉；普通地区/片源下拉原样跳过。
fn fill_secondary_category(
    fields: &mut Vec<Field>,
    page: &str,
    info: &ReleaseInfo,
) -> Option<String> {
    let type_value = field_lookup(fields, "type").unwrap_or("").to_string();
    if type_value.is_empty() || type_value == "0" {
        return None;
    }
    let type_label = select_option_meta(page, "type", &type_value)?.0;
    let mut saw_category_style = false;
    for fname in select_field_names(fields, "source_sel") {
        let current = field_lookup(fields, &fname).unwrap_or("").to_string();
        let options = select_options(page, &fname);
        let prefix = format!("{type_label}-");
        if !options.iter().any(|(_, l)| l.starts_with(&prefix)) {
            continue;
        }
        saw_category_style = true;
        if !current.is_empty() && current != "0" {
            continue;
        }
        let Some(sub) = secondary_category_label(&type_label, info) else {
            continue;
        };
        let wanted = format!("{prefix}{sub}");
        if let Some((value, _)) = options.iter().find(|(_, label)| label == &wanted) {
            set_field(fields, &fname, value.clone());
            return None;
        }
    }
    saw_category_style
        .then(|| "分类：source_sel 是二级分类，无法从标题/副标题判断子类，留空等人工选".into())
}

/// 二级分类子类：只认明确信号，不凭「名里有中文」乱猜国别。
fn secondary_category_label(type_label: &str, info: &ReleaseInfo) -> Option<&'static str> {
    let hay = format!("{} {}", info.name, info.subtitle).to_ascii_lowercase();
    if type_label.contains("剧集") || type_label.contains("电视剧") || type_label.contains("TV")
    {
        const BRANCHES: &[(&str, &str)] = &[
            ("美剧", "美剧"),
            ("英剧", "英剧"),
            ("日剧", "日剧"),
            ("韩剧", "韩剧"),
            ("港剧", "港台"),
            ("台剧", "港台"),
            ("国产剧", "大陆"),
            ("大陆剧", "大陆"),
            ("内地剧", "大陆"),
            ("tvb", "港台"),
            ("bbc", "英剧"),
            ("nhk", "日剧"),
            ("kbs", "韩剧"),
            ("cctv", "大陆"),
        ];
        for (marker, branch) in BRANCHES {
            if marker_contains(&hay, marker) {
                return Some(branch);
            }
        }
        // `全73集` 是国产剧的强信号；没有这个信号就不猜国别（英文剧的中文副标题不算）。
        if info.is_completed_series() && has_cjk(&info.subtitle) {
            return Some("大陆");
        }
        return None;
    }
    if type_label.contains("电影") || type_label == "Movie" {
        if hay.contains("remux") {
            return Some("Remux");
        }
        if hay.contains("2160") || hay.contains("4k") || hay.contains("uhd") {
            return Some("4K");
        }
        if hay.contains("1080") {
            return Some("1080");
        }
        if hay.contains("720") {
            return Some("720");
        }
        if hay.contains("hdtv") {
            return Some("HDTV");
        }
        if hay.contains("web-dl") || hay.contains("webrip") {
            return Some("WEB-DL");
        }
        if hay.contains("blu") || hay.contains("bdrip") || hay.contains("dvd") {
            return Some("原盘");
        }
    }
    None
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        let u = c as u32;
        (0x3400..=0x4DBF).contains(&u) || (0x4E00..=0x9FFF).contains(&u)
    })
}

/// 选档摘要：`分类=剧集 媒介=HDTV 编码=x264 …`（只列已选中的下拉）
fn select_summary(prep: &PreparedUpload) -> String {
    const KINDS: [(&str, &str); 8] = [
        ("type", "分类"),
        ("source_sel", "来源"),
        ("medium_sel", "媒介"),
        ("codec_sel", "编码"),
        ("standard_sel", "分辨率"),
        ("audiocodec_sel", "音频"),
        ("processing_sel", "处理"),
        ("team_sel", "制作组"),
    ];
    let mut parts = Vec::new();
    for (prefix, label) in KINDS {
        // 多组下拉（medium_sel[4]/[9]）取第一组已选中的展示
        let picked = select_field_names(&prep.fields, prefix)
            .into_iter()
            .find_map(|fname| {
                let v = field_lookup(&prep.fields, &fname).unwrap_or("").to_string();
                if v.is_empty() || v == "0" {
                    None
                } else {
                    Some((fname, v))
                }
            });
        let Some((fname, v)) = picked else {
            continue;
        };
        let name = select_option_meta(&prep.form_html, &fname, &v)
            .map(|(l, _)| l)
            .unwrap_or(v);
        parts.push(format!("{label}={name}"));
    }
    parts.join(" ")
}

/// 明显不是上传处理页的端点 basename：导航搜索/奖励/登录/索引/列表/详情一类。
/// 命中即中止提交——绝不能把种子 POST 到错误接口（longpt 被解析到 bonusrain.php、
/// tangpt 被解析到 search.php 就是这类误配）。
const NON_UPLOAD_ENDPOINTS: &[&str] = &[
    "search.php",
    "bonusrain.php",
    "login.php",
    "index.php",
    "torrents.php",
    "browse.php",
    "details.php",
    "signup.php",
    "register.php",
    "user.php",
    "usercp.php",
    "rules.php",
    "faq.php",
];

/// action 是否落在明显非上传端点上。站外绝对地址照收（城市类站点把种子 POST 到
/// 独立域名）。返回命中的 basename 写进错误提示。
fn non_upload_endpoint(action: &str, base: &Url) -> Option<&'static str> {
    let url = Url::parse(action).ok()?;
    if url.host_str() != base.host_str() {
        return None;
    }
    let name = url.path().rsplit('/').next().unwrap_or("");
    NON_UPLOAD_ENDPOINTS
        .iter()
        .find(|e| name.eq_ignore_ascii_case(e))
        .copied()
}

/// 表单 action 绝对地址（站外绝对地址照收：城市等站把种子 POST 到独立域名；
/// 相对路径按站点根解析成 takeupload.php 一类的提交地址）。
/// 只在上传表单块内取 action：导航条的搜索/登录表单排在前面，整页取第一个
/// 会把提交地址误配成 search.php / bonusrain.php 一类。
fn form_action(page: &str, base: &Url) -> Option<String> {
    let scope = upload_form_block(page).unwrap_or(page);
    let lower = scope.to_ascii_lowercase();
    let p = lower.find("<form")?;
    let end = lower[p..]
        .find('>')
        .map(|j| p + j + 1)
        .unwrap_or(scope.len());
    let tag = &scope[p..end];
    let action = attr(tag, "action")?;
    if action.is_empty() {
        return None;
    }
    Some(resolve_url(&action, base))
}

/// 把切片下标回退到最近的 UTF-8 字符边界（`end+80` 一类定长窗口可能切在汉字中间）
fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// checkbox 文案匹配：`<input type=checkbox …>` 后面 60 个字符里含关键词就算选中
fn match_checkboxes(page: &str, keyword: &str) -> Vec<Field> {
    let mut out = Vec::new();
    let mut exact: Vec<Field> = Vec::new();
    let lower = page.to_ascii_lowercase();
    let kw = keyword.to_lowercase();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<input") {
        let start = i + p;
        let end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(page.len());
        let tag = &page[start..end];
        let tl = tag.to_ascii_lowercase();
        if tl.contains("type=\"checkbox\"") || tl.contains("type='checkbox'") {
            // 文案 = 标签后到下一个 < 之前；窗口遇到下一个 <input 立即截断，
            // 否则密集标签网格里会把邻格文案误配给本格（实机出过 200 连勾）
            let after = &page[end..floor_char_boundary(page, end + 80)];
            let next = after
                .to_ascii_lowercase()
                .find("<input")
                .unwrap_or(after.len());
            let label = strip_tags(&after[..next]).to_lowercase();
            if label.trim() == kw {
                if let Some(name) = attr(tag, "name") {
                    let value = attr(tag, "value").unwrap_or_else(|| "yes".into());
                    exact.push((name, value));
                }
            } else if label.contains(&kw) {
                if let Some(name) = attr(tag, "name") {
                    let value = attr(tag, "value").unwrap_or_else(|| "yes".into());
                    out.push((name, value));
                }
            }
        }
        i = end;
    }
    // 文案全等优先：避免「HDR10」把「HDR10+」也勾上（互斥冲突）。
    if !exact.is_empty() {
        exact
    } else {
        out
    }
}

// ---------------- 上传提交 ----------------

pub struct UploadResult {
    pub detail_url: Option<String>,
    pub message: String,
    /// 站点回「种子已存在」：视为已覆盖，不算本次转种
    pub duplicate: bool,
    /// 回包正文原文（发后定位只从中提取自带链接，不做检索）
    pub body: String,
    /// 最终跳转 URL（跟随重定向后）
    pub final_url: String,
}

/// 上传准备：抓 upload.php 表单 + 构造提交字段（不 POST）。
/// `upload_release`（真提交）与 `review_reseed`（审阅）共用这一段。
pub struct PreparedUpload {
    /// 表单 action 解析出的绝对提交地址
    pub action: String,
    /// upload.php 页面地址（作 Referer）
    pub form_url: String,
    pub fields: Vec<Field>,
    pub picked_tags: Vec<String>,
    pub tag_warn: Option<String>,
    /// 选档映射提示（词元配不上选项、分类没把握等；只提示不阻断）
    pub select_notes: Vec<String>,
    /// 规则判定的必填字段缺口（每条文案含「必填缺失」）；非空时门禁拒绝发布
    pub required_gaps: Vec<String>,
    /// 表单页原文（官方预审要从中取 LUCK_AUDIT_USER / 选项文案）
    pub form_html: String,
}

/// 取最终要提交的简介（`apply_site_rules` 可能已剪死图/转 BBCode）。
/// 门禁必须看最终值，不能看源页原文，否则死图剪掉后仍会按旧图数量误判。
pub fn submitted_descr<'a>(prep: &'a PreparedUpload, fallback: &'a str) -> &'a str {
    for name in ["descr", "release_desc", "description", "content"] {
        if let Some((_, v)) = prep.fields.iter().find(|(n, _)| n == name) {
            return v;
        }
    }
    fallback
}

/// 剪掉简介里不可访问的 `<img>`（预审逐图校验；只处理绝对 URL）。
/// `probe` 返回 true 表示图片可访问，便于单测注入。
pub fn prune_dead_images_with(html: &str, probe: &mut dyn FnMut(&str) -> bool) -> (String, usize) {
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    let mut removed = 0usize;
    let lower = html.to_ascii_lowercase();
    while let Some(p) = lower[i..].find("<img") {
        let start = i + p;
        out.push_str(&html[i..start]);
        let end = html[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(html.len());
        let tag = &html[start..end];
        let src = attr(tag, "src").unwrap_or_default();
        // 本机截帧上传的 Imgur 图是发布时注入的可信资产，不参与死图探测；
        // 源页原有图片仍逐张探测。
        let trusted = src.contains("i.imgur.com/");
        let keep = src.starts_with("http") && (trusted || probe(&src));
        if keep {
            out.push_str(tag);
        } else {
            removed += 1;
        }
        i = end;
    }
    out.push_str(&html[i..]);
    (out, removed)
}

/// 图片响应是否可当图床资源：2xx 且 content-type 是图片（或二进制流）。
/// 防盗链图床常回 2xx 但正文是 text/html 验证页/418 页，只看状态码会误判成可达；
/// 这里按最终响应的 content-type 排除 HTML/JSON 拦截页。CT 缺失时放行（部分图床不回 CT）。
fn content_type_is_image(ct: Option<&str>) -> bool {
    match ct.map(|s| s.trim().to_ascii_lowercase()) {
        Some(c) => c.starts_with("image/") || c == "application/octet-stream",
        None => true,
    }
}

fn prune_dead_images(html: &str, referer: &str) -> (String, usize) {
    let mut builder = reqwest::blocking::Client::builder()
        .user_agent(crate::sites::nexusphp::DEFAULT_UA)
        .no_proxy()
        .timeout(std::time::Duration::from_secs(8));
    // 防盗链图床（豆瓣图床实测裸请求 418 回 HTML，带 Referer 才出图）：
    // 探测时带上目标站上传页 Referer，与浏览器从上传页插图的行为一致。
    if !referer.is_empty() {
        let mut headers = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(referer) {
            headers.insert(REFERER, v);
        }
        builder = builder.default_headers(headers);
    }
    let Ok(client) = builder.build() else {
        return (html.to_string(), 0);
    };
    prune_dead_images_with(html, &mut |url| {
        client
            .get(url)
            .send()
            .map(|r| {
                r.status().is_success()
                    && content_type_is_image(
                        r.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()),
                    )
            })
            .unwrap_or(false)
    })
}

/// 标签归一：小写、去空白与标点（保留中文等 Unicode 字母数字；与 `norm_token` 不同，后者只留 ASCII）
fn norm_label(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// 标签同义词组：同一控件在不同站点的写法（中字/简中、国语/国配…）
fn label_aliases(label: &str) -> &'static [&'static str] {
    match norm_label(label).as_str() {
        "中字" | "简中" | "简体" | "中文字幕" | "简繁中字" => {
            &["中字", "简中", "简体", "中文字幕", "简繁中字"]
        }
        "国语" | "国配" | "普通话" | "国语配音" | "普通话配音" => {
            &["国语", "国配", "普通话", "国语配音", "普通话配音"]
        }
        _ => &[],
    }
}

/// 页面标签与配置标签归一匹配：大小写/全半角/空白标点不敏感；全等优先，其次双向子串，再走别名
fn label_matches(page_label: &str, want: &str) -> bool {
    let pl = norm_label(page_label);
    let wl = norm_label(want);
    if pl.is_empty() || wl.is_empty() {
        return false;
    }
    if pl == wl || pl.contains(&wl) || wl.contains(&pl) {
        return true;
    }
    label_aliases(want).iter().any(|a| {
        let an = norm_label(a);
        !an.is_empty() && (pl == an || pl.contains(&an) || an.contains(&pl))
    })
}

/// 表单里某前缀下的 input 控件（radio/checkbox）：(name, value, 标签文案, 是否 checked)
fn input_controls(page: &str, prefix: &str, kind: &str) -> Vec<(String, String, String, bool)> {
    let page = upload_form_block(page).unwrap_or(page);
    let lower = page.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<input") {
        let start = i + p;
        let end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(page.len());
        let tag = &page[start..end];
        let tl = tag.to_ascii_lowercase();
        let type_attr = format!("type=\"{kind}\"");
        let type_attr_q = format!("type='{kind}'");
        if !(tl.contains(&type_attr) || tl.contains(&type_attr_q)) {
            i = end;
            continue;
        }
        if let Some(name) = attr(tag, "name") {
            if name == prefix || (name.starts_with(prefix) && name[prefix.len()..].starts_with('['))
            {
                let after = &page[end..floor_char_boundary(page, end + 80)];
                let next = after
                    .to_ascii_lowercase()
                    .find("<input")
                    .unwrap_or(after.len());
                let label = strip_tags(&after[..next]).trim().to_string();
                // 浏览器对无 value 的复选框/单选框默认提交 `on`；真实站点多带数字编码 value
                let value = attr(tag, "value").unwrap_or_else(|| "on".into());
                let checked = tl.contains("checked");
                out.push((name, value, label, checked));
            }
        }
        i = end;
    }
    out
}

/// 前缀下在页面上出现过的全部控件名（select + radio/checkbox），用于缺口点名
fn prefix_control_names(page: &str, prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = select_names(page)
        .into_iter()
        .filter(|n| n == prefix || (n.starts_with(prefix) && n[prefix.len()..].starts_with('[')))
        .collect();
    for kind in ["radio", "checkbox"] {
        for (n, _, _, _) in input_controls(page, prefix, kind) {
            if !names.contains(&n) {
                names.push(n);
            }
        }
    }
    names
}

/// 前缀下是否已有已选值：fields 里非空/非 0，或页面控件带 checked 且 value 非空
fn prefix_selected(fields: &[Field], page: &str, prefix: &str) -> bool {
    if select_field_names(fields, prefix).iter().any(|n| {
        let v = field_lookup(fields, n).unwrap_or("");
        !v.is_empty() && v != "0"
    }) {
        return true;
    }
    for kind in ["radio", "checkbox"] {
        if input_controls(page, prefix, kind)
            .iter()
            .any(|(_, v, _, c)| *c && !v.is_empty() && v != "0")
        {
            return true;
        }
    }
    false
}

/// 应用逐站显式选档覆盖（`reseedRules.<site>.selectPicks`）：只在字段当前为空/0 时写入，
/// 且 value 必须是该站表单真实存在的选项（防旧规则在新版表单上写无效值）。
/// `multiPicks` 按页面标签勾选复选框/多选组；`requiredFields` 校验必填前缀，
/// 未选即记 `required_gaps`，由门禁拒绝发布（不带空值 POST）。
pub fn apply_site_rules(prep: &mut PreparedUpload, rules: &crate::gates::SiteRules) {
    // 剪死图是硬规则：不分站点、不看配置。站点官方预审逐图校验，死链会直接退回整条种子。
    if let Some(field) = ["descr", "release_desc", "description", "content"]
        .iter()
        .find(|f| field_lookup(&prep.fields, f).is_some())
        .copied()
    {
        let cur = field_lookup(&prep.fields, field).unwrap_or("").to_string();
        let (next, removed) = prune_dead_images(&cur, &prep.form_url);
        if removed > 0 {
            set_field(&mut prep.fields, field, next);
            prep.select_notes.push(format!(
                "简介剪掉 {removed} 张不可访问图片（逐图探测，硬规则）"
            ));
        }
    }
    apply_site_rules_tail(prep, rules);
}

/// 门禁里不需要联网的部分（转 BBCode、选档、必填校验）。单测走这里，避免打外网图。
fn apply_site_rules_tail(prep: &mut PreparedUpload, rules: &crate::gates::SiteRules) {
    // 站点只吃 BBCode 时（NexusPHP 的简介框是 BBCode 编辑器），源站 HTML 原样提交
    // 会被转义成字面 `<fieldset>`，审核端既看不到排版也看不到图。缺省就转 BBCode；
    // 只有规则显式 descrFormat=html 才保持 HTML。先剪死图，再转。
    if rules.descr_is_bbcode() {
        if let Some(field) = ["descr", "release_desc", "description", "content"]
            .iter()
            .find(|f| field_lookup(&prep.fields, f).is_some())
            .copied()
        {
            let cur = field_lookup(&prep.fields, field).unwrap_or("").to_string();
            if looks_like_html(&cur) {
                let next = html_to_bbcode(&cur);
                if next != cur {
                    set_field(&mut prep.fields, field, next);
                    prep.select_notes
                        .push("简介：HTML 已转 BBCode（descrFormat）".into());
                }
            }
        }
    }
    for (prefix, want) in &rules.select_picks {
        for name in select_field_names(&prep.fields, prefix) {
            let cur = field_lookup(&prep.fields, &name).unwrap_or("").to_string();
            if !cur.is_empty() && cur != "0" {
                continue;
            }
            let valid = select_options(&prep.form_html, &name)
                .iter()
                .any(|(v, _)| v == want);
            if valid {
                set_field(&mut prep.fields, &name, want.clone());
                prep.select_notes
                    .push(format!("{name}：按站点显式规则选 value={want}"));
            }
        }
    }

    // multiPicks：复选框/多选组按页面真实标签勾选；匹配不到不臆造、保持未选
    let mut unmatched: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (prefix, wants) in &rules.multi_picks {
        let controls = input_controls(&prep.form_html, prefix, "checkbox");
        let mut miss: Vec<String> = Vec::new();
        for want in wants {
            let hit = controls
                .iter()
                .find(|(_, _, label, _)| label_matches(label, want));
            match hit {
                Some((name, value, label, _)) => {
                    if !prep.fields.iter().any(|(n, v)| n == name && v == value) {
                        prep.fields.push((name.clone(), value.clone()));
                    }
                    prep.select_notes
                        .push(format!("{name}：按规则勾选「{label}」（multiPicks）"));
                }
                None => miss.push(want.clone()),
            }
        }
        if !miss.is_empty() {
            unmatched.insert(prefix.clone(), miss);
        }
    }

    // requiredFields：必填前缀任一未选即记缺口；门禁据此拒绝发布，绝不带空值 POST
    for prefix in &rules.required_fields {
        if prefix_selected(&prep.fields, &prep.form_html, prefix) {
            continue;
        }
        let name = prefix_control_names(&prep.form_html, prefix)
            .into_iter()
            .next()
            .unwrap_or_else(|| prefix.clone());
        let gap = match unmatched.get(prefix) {
            Some(miss) if !miss.is_empty() => {
                format!("必填缺失：{name}（配置标签未命中：{}）", miss.join("/"))
            }
            _ => format!("必填缺失：{name}"),
        };
        prep.required_gaps.push(gap);
    }
}

/// qB 中与内容分片一致的本地成员：(hash, savePath, category)
type MatchedMember = (String, String, Option<String>);
/// 判重复核产物：目标站 .torrent 字节 + 本地同分片成员
type DupCheck = Option<(Vec<u8>, MatchedMember)>;

/// 源种必须与 qB 里某个**已完成**的本地成员分片结构一致（pieces hash 相同），
/// 且返回该成员的 savePath/category 供推种复用。只看 info hash 属于内容组不够：
/// 同文件不同分片长度的重新打包 hash 不同，无法跨种（浪潮实测）。
fn matching_local_member(
    qb: &QBittorrent,
    entry: &crate::index::ContentEntry,
    source_data: &[u8],
) -> Result<Option<MatchedMember>> {
    let src_pieces = bencode::pieces_hash(source_data)
        .ok_or_else(|| anyhow!("源种缺少 pieces，无法确认分片结构"))?;
    for (hash, meta) in &entry.torrents {
        let Some((progress, _)) = qb.torrent_progress(hash).ok().flatten() else {
            continue;
        };
        if progress < 0.999 {
            continue;
        }
        let Ok(local) = qb.export_torrent(hash) else {
            continue;
        };
        if bencode::pieces_hash(&local).as_deref() == Some(src_pieces.as_str()) {
            return Ok(Some((
                hash.clone(),
                meta.save_path.clone(),
                meta.category.clone(),
            )));
        }
    }
    Ok(None)
}

/// Pick one completed local member, export its raw `.torrent` from qB.
/// Source seed for the rewritten upload comes from here, never re-downloaded
/// from the source site.
fn pick_exported_member(
    qb: &QBittorrent,
    entry: &crate::index::ContentEntry,
) -> Result<(MatchedMember, Vec<u8>)> {
    for (hash, meta) in &entry.torrents {
        let Some((progress, _)) = qb.torrent_progress(hash).ok().flatten() else {
            continue;
        };
        if progress < 0.999 {
            continue;
        }
        let Ok(data) = qb.export_torrent(hash) else {
            continue;
        };
        if bencode::pieces_hash(&data).is_none() {
            continue;
        }
        return Ok((
            (hash.clone(), meta.save_path.clone(), meta.category.clone()),
            data,
        ));
    }
    bail!(
        "本地 qB 中找不到已完成的内容成员（{}），拒绝上传",
        entry.name
    );
}

/// Derive the sibling `details.php?id=N` link from a `download.php?id=N` URL.
fn detail_from_download(download_url: &str, base: &Url) -> Option<String> {
    let marker = "download.php?id=";
    let pos = download_url.find(marker)?;
    let after = &download_url[pos + marker.len()..];
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    Some(resolve_url(&format!("details.php?id={digits}"), base))
}

pub fn prepare_upload(target: &NexusPhp, info: &ReleaseInfo) -> Result<PreparedUpload> {
    let form_url = resolve_url("upload.php", &target.base);
    let form_html = target
        .fetch_html(&form_url, target.base.as_str())
        .context("抓目标站上传表单失败")?;
    if upload_form_block(&form_html).is_none() {
        if let Some(why) = upload_denied_reason(&form_html) {
            bail!("{why}：{form_url}");
        }
        bail!("目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：{form_url}");
    }
    if let Some(notice) = crate::sites::nexusphp::block_notice(&form_html) {
        bail!("目标站限流（{notice}）：{form_url}");
    }

    let tags_wanted = wanted_tags(info);
    let (fields, picked_tags, select_notes) =
        build_upload_fields_for_site(info, &form_html, &tags_wanted, &target.site_id);
    // 想勾的标签一个都没匹配上才警告（复选框 value 多是数字编码，
    // 拿 value 里含不含关键词来判会把勾上了的误报成漏勾）
    let tag_warn = if !tags_wanted.is_empty() && picked_tags.is_empty() {
        Some(format!(
            "[{}] 表单里没找到「{}」类标签复选框，请发布后手动补",
            target.site_id,
            tags_wanted.join("/")
        ))
    } else {
        None
    };

    let action = form_action(&form_html, &target.base).unwrap_or_else(|| form_url.clone());
    if let Some(bad) = non_upload_endpoint(&action, &target.base) {
        bail!("上传表单 action 不是上传处理页：{action}（命中 {bad}），已中止提交");
    }
    Ok(PreparedUpload {
        action,
        form_url,
        fields,
        picked_tags,
        tag_warn,
        select_notes,
        required_gaps: vec![],
        form_html,
    })
}

/// 提交到目标站 upload.php。multipart 一次提交 + 「已丢失 POST」重投一次
/// （Swift 版实测吐鲁番连续提交会偶发丢 POST，同字节重投即可）。
pub fn upload_release(
    target: &NexusPhp,
    info: &ReleaseInfo,
    torrent_data: &[u8],
    filename: &str,
    dump_dir: Option<&Path>,
) -> Result<UploadResult> {
    let prep = prepare_upload(target, info)?;
    submit_upload(target, &prep, torrent_data, filename, dump_dir)
}

/// 上传回包调试落盘开关：仅 `BSEND_DUMP_UPLOAD=1/true/on` 时开启，默认关闭。
/// 抽成纯函数便于单测，开关判定不直接读环境变量本身。
fn dump_upload_enabled(env: Option<&str>) -> bool {
    matches!(
        env.map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("on")
    )
}

fn upload_dump_enabled() -> bool {
    dump_upload_enabled(std::env::var("BSEND_DUMP_UPLOAD").ok().as_deref())
}

/// 把上传回包（状态 / 最终 URL / 非凭据响应头 / 提交字段名 / 正文）写到数据目录
/// `upload-debug/<site>-<unix>-a<attempt>.html`。开关关闭或目录不可用时静默跳过，
/// 不影响主流程；快照已剔除 Cookie/Set-Cookie，正文不含 passkey。
fn dump_upload_response(
    dump_dir: Option<&Path>,
    site_id: &str,
    resp: &crate::sites::nexusphp::UploadResponse,
    action: &str,
    fields: &[Field],
    attempt: u8,
) {
    if !upload_dump_enabled() {
        return;
    }
    let Some(dir) = dump_dir else { return };
    let dir = dir.join("upload-debug");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let stamp = now_unix();
    let file = dir.join(format!("{site_id}-{stamp}-a{attempt}.html"));
    let field_names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
    let mut preamble = String::new();
    preamble.push_str("<!-- BoxSend upload debug dump\n");
    preamble.push_str(&format!("site: {site_id}\n"));
    preamble.push_str(&format!("time_unix: {stamp}\n"));
    preamble.push_str(&format!("attempt: {attempt}\n"));
    preamble.push_str(&format!("action: {action}\n"));
    preamble.push_str(&format!("status: {}\n", resp.status));
    preamble.push_str(&format!("final_url: {}\n", resp.final_url));
    preamble.push_str("headers:\n");
    for (k, v) in &resp.headers {
        preamble.push_str(&format!("  {k}: {v}\n"));
    }
    preamble.push_str(&format!(
        "submitted_field_names: {}\n",
        field_names.join(", ")
    ));
    preamble.push_str("-->\n");
    let body = format!("{preamble}{}", resp.body);
    let _ = std::fs::write(&file, body);
}

/// 用已构造好的字段提交（`prepare_upload` 的产出）。官方预审、审阅与
/// 正式转种共用同一份字段，永不漂移。
pub fn submit_upload(
    target: &NexusPhp,
    prep: &PreparedUpload,
    torrent_data: &[u8],
    filename: &str,
    dump_dir: Option<&Path>,
) -> Result<UploadResult> {
    // 成功消息末尾追加标签警告（若有）
    let note_suffix = prep
        .tag_warn
        .as_deref()
        .map(|w| format!("；{w}"))
        .unwrap_or_default();
    let mut last_err = String::new();
    for attempt in 0..2 {
        let resp = target
            .post_upload(
                &prep.action,
                &prep.fields,
                "file",
                filename,
                torrent_data,
                &prep.form_url,
            )
            .with_context(|| format!("提交上传失败（第 {} 次）", attempt + 1))?;
        dump_upload_response(
            dump_dir,
            &target.site_id,
            &resp,
            &prep.action,
            &prep.fields,
            attempt as u8,
        );
        let body = resp.body.clone();
        let final_url = resp.final_url.clone();
        // 「已存在」先判：站点常跳回已有种子详情页，先判成功会误报转种成功。
        // 只认结构性证据（existed=1 跳转 / errorbox），整页自由文本不能当重复，
        // 否则普通校验失败页里的模板措辞会被误判成「站点已有」。
        if let Some(marker) = exist_marker(&final_url, &body) {
            let dup_url = sole_detail_url(&body, &final_url, &prep.action);
            return Ok(UploadResult {
                detail_url: dup_url,
                message: format!(
                    "站点提示种子已存在（证据：{marker}；视为已覆盖，不算本次转种）{note_suffix}"
                ),
                duplicate: true,
                body,
                final_url,
            });
        }
        // 成功：跳到详情页 / 发布成功提示
        if let Some(u) = redirect_detail_url(&final_url, &prep.action) {
            return Ok(UploadResult {
                detail_url: Some(u),
                message: format!("发布成功（跳转详情页）{note_suffix}"),
                duplicate: false,
                body,
                final_url,
            });
        }
        if body.contains("发布成功")
            || body.contains("new torrent")
            || body.contains("Torrent added")
        {
            let u = sole_detail_url(&body, &final_url, &prep.action);
            return Ok(UploadResult {
                detail_url: u.clone(),
                message: if u.is_some() {
                    format!("发布成功{note_suffix}")
                } else {
                    format!("发布成功（响应页没找到详情链接，需手动确认新种）{note_suffix}")
                },
                duplicate: false,
                body,
                final_url,
            });
        }
        // 部分站点成功后不发 Location 跳转，回包正文挂新种详情页链接（kufei 实测：
        // 提交后 final_url 仍是上传页，正文带 details.php?id=）。回包不是上传表单
        // 重填页、且正文只有唯一 details.php?id=N 时视为成功。这里只放宽「成功
        // 识别方式」：existed 判定与判重口径不变。
        if let Some(u) = body_detail_success_url(&body, &final_url, &prep.action) {
            return Ok(UploadResult {
                detail_url: Some(u),
                message: format!("发布成功（回包正文详情链接）{note_suffix}"),
                duplicate: false,
                body,
                final_url,
            });
        }
        last_err = extract_upload_error(&body, resp.status);
        if looks_like_lost_post(&last_err) && attempt == 0 {
            thread::sleep(Duration::from_secs(2));
            continue; // 同字节重投
        }
        break;
    }
    Err(anyhow!("上传未成功：{last_err}"))
}

/// 上传未成功时提取错误（errorbox → 常见错误特征 → 兜底）
fn extract_upload_error(body: &str, status: u16) -> String {
    // 种子过大：NexusPHP 系不回错误框，而是回一个 `torrent_too_large` 表单自动跳
    // offers.php（候选区）。按普通“未识别返回”记会把站点明确的结论埋掉。
    if body.contains("torrent_too_large") || body.contains("add_offer=1") {
        return "站点判定种子过大（torrent_too_large），要求改走候选（offers.php）；不自动重试"
            .into();
    }
    if let Some(t) = div_inner(body, "errorbox") {
        let t = strip_tags(&t).trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    for marker in ["已经上传", "已存在", "被人上传", "上传失败", "发布失败"] {
        if let Some(p) = body.find(marker) {
            let seg = strip_tags(&body[p..floor_char_boundary(body, p + 120)]);
            let line = seg.lines().next().unwrap_or("").trim().to_string();
            if !line.is_empty() {
                return line;
            }
        }
    }
    if body.to_lowercase().contains("takelogin.php") || body.contains("signinhandler") {
        return "会话失效（返回登录页），请重新导入该站 cookie".into();
    }
    if body.contains("<form") && body.contains("upload.php") {
        return "服务器未跳转到详情页（必填字段缺失或校验失败）".into();
    }
    format!("HTTP {status} 未识别的返回")
}

/// 上传错误信息是不是“站点嫌种子太大、要求走候选”（终态，不重试）。
fn site_rejects_size(msg: &str) -> bool {
    msg.contains("torrent_too_large") || msg.contains("种子过大")
}

/// 站点「种子已存在」判定。返回命中的证据描述（写进审计，便于回溯）。
///
/// 只认两类结构性证据：
/// 1. 跳转落到 `details.php?id=…&existed=1`（NexusPHP 重复上传的标准动作）；
/// 2. `errorbox` 里出现重复措辞。
///
/// 过去是整页 `contains`，结果把导航/表单说明/帮助文案里的「已存在」也算命中：
/// 一旦上传页因必填项校验失败被原样返回，就会被记成「站点已有」，凭空造出一条
/// 并不存在的详情页链接。注意：`existed=1` 也只证明「站点认为重复」，不证明
/// 那个 id 是本次内容的条目——回包 id 在实测里 6/6 不可信，仍需逐条核对。
fn exist_marker(final_url: &str, body: &str) -> Option<&'static str> {
    if final_url.to_ascii_lowercase().contains("existed=1") {
        return Some("跳转 URL 带 existed=1");
    }
    const MARKERS: &[&str] = &[
        "已经上传",
        "已存在",
        "被人上传",
        "already exists",
        "Torrent already",
    ];
    // 错误框里的明确重复措辞优先：有的站会把错误框和上传表单一起返回（重填页），
    // 不能因为看到 multipart 就忽略站点说过的「已存在」。
    if let Some(boxed) = div_inner(body, "errorbox") {
        if let Some(m) = MARKERS.iter().find(|m| boxed.contains(**m)) {
            return Some(*m);
        }
    }
    // 返回体还是上传表单（multipart）：这是校验失败重填，不是重复提示
    if body.contains("multipart/form-data") {
        return None;
    }
    // 提示页通常很短；长页面里的零散措辞多半是模板/简介文本
    if body.len() <= 8192 {
        if let Some(m) = MARKERS.iter().find(|m| body.contains(**m)) {
            return Some(*m);
        }
    }
    None
}

fn looks_like_lost_post(msg: &str) -> bool {
    msg.contains("请填写必填项目") || msg.contains("必填字段缺失")
}

/// 最终 URL / 响应体里的详情页链接（details.php?id=N）
fn redirect_detail_url(final_url: &str, action: &str) -> Option<String> {
    if final_url == action {
        return None;
    }
    let lower = final_url.to_lowercase();
    if lower.contains("details.php?id=") || lower.contains("torrents.php?id=") {
        // 不能是登录页/上传页
        if !lower.contains("upload") && !lower.contains("login") {
            return Some(final_url.to_string());
        }
    }
    None
}

/// 成功但没跳转：从回包正文取唯一详情页链接。上传表单重填页（校验失败回显，
/// multipart 或仍含文件上传框）不参与——侧边栏挂的旧种链接不能当新种证据。
fn body_detail_success_url(body: &str, final_url: &str, action: &str) -> Option<String> {
    if body.contains("multipart/form-data") || upload_form_block(body).is_some() {
        return None;
    }
    sole_detail_url(body, final_url, action)
}

fn sole_detail_url(body: &str, final_url: &str, action: &str) -> Option<String> {
    if let Some(u) = redirect_detail_url(final_url, action) {
        return Some(u);
    }
    // 响应体里唯一的 details.php?id=N；`userdetails.php?id=` 是用户主页，不算。
    let base = Url::parse(action).ok()?;
    let mut found: Vec<(String, String)> = Vec::new();
    collect_id_links(body, &base, "details.php?id=", &mut found);
    if found.len() == 1 {
        Some(found.pop().unwrap().1)
    } else {
        None
    }
}

// ---------------- 审阅模式（不提交） ----------------

/// 审阅时已取到的发布侧载荷：源/目标适配器与源页。
/// 发布器直接复用它们，既不再抓源页/上传页，也保证提交字段与门禁判定是同一份。
pub(crate) struct ReviewCarry {
    src: NexusPhp,
    tgt: NexusPhp,
    page: TorrentPage,
}

/// 审阅结果：将要提交的全部内容摆出来，不 POST、不落账
pub struct ReviewReport {
    pub info: ReleaseInfo,
    pub preflight: Preflight,
    pub prepared: PreparedUpload,
    /// 站点官方预审（LuckAudit）结论——支持该接口的站才有
    pub audit: Option<AuditVerdict>,
    /// 官方预审没跑通时的原因（只提示，不阻断审阅）
    pub audit_note: Option<String>,
    /// 发布器复用（`run_reseed_reviewed`）；渲染与单测不需要
    pub(crate) carry: Option<ReviewCarry>,
}

impl ReviewReport {
    /// 人看的审阅文本：预检结论 + 官方预审 + 逐字段值（长值截断）
    pub fn render(&self, max: usize) -> String {
        let mut out = String::new();
        out.push_str("==== 转种审阅（只审不发）====\n");
        out.push_str(&format!("提交地址：{}\n", self.prepared.action));
        out.push_str(&format!("预检：{}\n", self.preflight.summary()));
        for e in &self.preflight.errors {
            out.push_str(&format!("  阻断：{e}\n"));
        }
        for w in &self.preflight.warnings {
            out.push_str(&format!("  警告：{w}\n"));
        }
        for f in &self.preflight.fixes {
            out.push_str(&format!("  已自动补：{f}\n"));
        }
        if let Some(w) = &self.prepared.tag_warn {
            out.push_str(&format!("  警告：{w}\n"));
        }
        for n in &self.prepared.select_notes {
            out.push_str(&format!("  选档提示：{n}\n"));
        }
        for g in &self.prepared.required_gaps {
            out.push_str(&format!("  阻断：{g}\n"));
        }
        match (&self.audit, &self.audit_note) {
            (Some(a), _) => {
                out.push_str(&format!(
                    "官方预审（LuckAudit）：{} {}\n",
                    if a.passed { "✅" } else { "❌" },
                    a.summary()
                ));
                for d in &a.details {
                    out.push_str(&format!("  {}\n", d.line()));
                }
                for s in &a.suggestions {
                    out.push_str(&format!("  建议：{s}\n"));
                }
            }
            (None, Some(n)) => out.push_str(&format!("官方预审（LuckAudit）：没跑通（{n}）\n")),
            (None, None) => {}
        }
        if !self.prepared.picked_tags.is_empty() {
            out.push_str(&format!(
                "标签勾选：{}\n",
                self.prepared.picked_tags.join("、")
            ));
        }
        let picks = select_summary(&self.prepared);
        if !picks.is_empty() {
            out.push_str(&format!("选档：{picks}\n"));
        }
        out.push_str("将提交的字段：\n");
        for (n, v) in &self.prepared.fields {
            out.push_str(&format!(
                "  {n} = {}\n",
                truncate_chars(&mask_field_value(n, v), max)
            ));
        }
        out
    }
}

/// 审阅渲染的字段脱敏：按**字段名**判定（不按值猜），命中凭据类一律隐藏，
/// 只保留「有这个字段 + 长度」，审阅仍能确认 passkey 等是否透传成功。
/// 泄漏面：render/dump_fields 会进终端、GUI 日志与反馈截图，passkey 等同账号凭据。
fn mask_field_value(name: &str, value: &str) -> String {
    const SECRET_HINTS: [&str; 8] = [
        "passkey", "token", "auth", "cookie", "secure", "uid", "secret", "password",
    ];
    let n = name.to_ascii_lowercase();
    if SECRET_HINTS.iter().any(|h| n.contains(h)) {
        format!("<已隐藏 len={}>", value.len())
    } else {
        value.to_string()
    }
}

/// 长值截断（按字符切，中文不切半个）
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…（共 {} 字节）", s.len())
}

/// 审阅一条龙：源页解析 → 预检 → 构造上传字段，但不 POST。
/// 预检不通过也照常返回报告（审阅就是要看清问题），调用方看 `preflight.ok()`。
/// `throttle` 给 autopost 用：源页/上传页/官方预审都是真实站点请求，必须领号；
/// 单条人工审阅传 `None`（没有共享节流器，不额外限速）。
pub fn review_reseed(
    cfg: &Config,
    req: &ReseedRequest,
    log: &mut dyn FnMut(&str),
    throttle: Option<&SiteThrottle>,
) -> Result<ReviewReport> {
    if let Some(t) = throttle {
        if !t.acquire(&req.source_site, BudgetKind::Source) {
            bail!("审阅预算用完（源站定位），本轮跳过该草稿");
        }
    }
    let rel = prepare_released(cfg, req, log)?;
    if let Some(t) = throttle {
        if !t.acquire(&req.target_site, BudgetKind::Search) {
            bail!("审阅预算用完（目标站上传页），本轮跳过该草稿");
        }
    }
    log("抓目标站上传表单，构造提交字段…");
    let mut prepared = prepare_upload(&rel.tgt, &rel.info)?;
    let rules = cfg.coverage.rules_for(&req.target_site);
    apply_site_rules(&mut prepared, &rules);
    log(&format!(
        "审阅完成：{} 个字段 → {}（未提交）",
        prepared.fields.len(),
        prepared.action
    ));
    // 站点官方预审（LuckAudit）：把将要提交的字段交给站方打分，结论进审阅报告
    let (audit, audit_note) = if pre_audit_supported(&prepared.form_html) {
        if let Some(t) = throttle {
            if !t.acquire(&req.target_site, BudgetKind::Search) {
                bail!("审阅预算用完（官方预审），本轮跳过该草稿");
            }
        }
        log("调用站点官方预审（LuckAudit）…");
        match run_pre_audit(&rel.tgt, &rel.info, &prepared) {
            Ok(v) => {
                log(&format!("官方预审：{}", v.summary()));
                for d in &v.details {
                    log(&format!("官方预审·{}", d.line()));
                }
                (Some(v), None)
            }
            Err(e) => {
                let note = format!("{e:#}");
                log(&format!("官方预审接口异常（{note}），审阅里只做提示"));
                (None, Some(note))
            }
        }
    } else {
        (None, None)
    };
    Ok(ReviewReport {
        info: rel.info,
        preflight: rel.preflight,
        prepared,
        audit,
        audit_note,
        carry: Some(ReviewCarry {
            src: rel.src,
            tgt: rel.tgt,
            page: rel.page,
        }),
    })
}

// ---------------- 发后自检 ----------------

/// 发后自检：重读新种详情页，核对标题 / MediaInfo / 条目链接真的落上了。
/// 只出警告不推翻上传结论（抓不到页面也算一条警告）。
/// 标题归一化：大小写 + 非字母数字统一成点（站点常把 `.`/空格互换写法）
fn norm_post_title(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '.' })
        .collect::<String>()
        .split('.')
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}

/// 目标站 IMDb 区判重：很多站按条目判重（同片不同版本也算 existed）。
/// 返回 (详情页链接, 页面标题)；无 IMDb/无命中返回 None。
fn target_has_imdb(
    target: &NexusPhp,
    imdb: Option<&str>,
    log: &mut dyn FnMut(&str),
) -> Result<Option<(String, String)>> {
    let Some(tt) = imdb else {
        return Ok(None);
    };
    if !tt.starts_with("tt") {
        return Ok(None);
    }
    let url = format!(
        "{}/torrents.php?search={}&search_area=4&notnewword=1",
        target.base.as_str().trim_end_matches('/'),
        tt
    );
    let html = match target.fetch_html(&url, target.base.as_str()) {
        Ok(h) => h,
        Err(e) => {
            log(&format!("IMDb 区判重失败：{e:#}"));
            return Err(e);
        }
    };
    // 只认搜索结果锚点：<a title="..." href="details.php?id=N...">。
    // 侧栏/用户信息里也有 details.php?id=，不能当判重结果（曾误抓 id=10230）。
    let lower = html.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(p) = lower[from..].find("<a") {
        let start = from + p;
        let end = html[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(html.len());
        let tag = &html[start..end];
        if let Some(href) = attr(tag, "href") {
            let path = href.split(['?', '#']).next().unwrap_or("");
            let last_seg = path.rsplit('/').next().unwrap_or("");
            if last_seg == "details.php" {
                let Some(pos) = href.find("details.php?id=") else {
                    from = end;
                    continue;
                };
                let after = &href[pos + "details.php?id=".len()..];
                let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    let detail = format!(
                        "{}/details.php?id={digits}",
                        target.base.as_str().trim_end_matches('/')
                    );
                    let title = attr(tag, "title").unwrap_or_default().to_string();
                    if !title.is_empty() {
                        return Ok(Some((detail, title)));
                    }
                }
            }
        }
        from = end;
    }
    Ok(None)
}

// ---------------- 发后定位（只认上传回包自带链接，不做站内检索） ----------------

/// 上传回包里解析出的链接：详情页与下载直链各取首条。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostedLinks {
    pub detail: Option<String>,
    pub download: Option<String>,
}

fn collect_id_links(haystack: &str, base: &Url, marker: &str, out: &mut Vec<(String, String)>) {
    let mut rest = haystack;
    while let Some(p) = rest.find(marker) {
        // `userdetails.php?id=` 里也包含 `details.php?id=`：marker 前面还是字母
        // 或下划线时说明它是更长的文件名，不能当种子详情链接。
        if rest[..p]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            rest = &rest[p + marker.len()..];
            continue;
        }
        let after = &rest[p + marker.len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            let href = format!("{marker}{digits}");
            // details 链接要躲过 userdetails.php?id= 这类用户主页锚（前缀守卫）
            if marker == "details.php?id=" && !is_detail_href(&href) {
                rest = after;
                continue;
            }
            let u = resolve_url(&href, base);
            if !out.iter().any(|(id, url)| id == &digits && url == &u) {
                out.push((digits, u));
            }
        }
        rest = after;
    }
}

/// 从上传回包正文与最终跳转 URL 提取 `details.php?id=` / `download.php?id=` 链接。
/// 纯解析：不发检索请求、不读「我发布的列表」（QB-TRUTH-MODE §6）。
///
/// 成功页常把侧边栏/相关推荐里的旧种一起带上，不能盲取第一条。取链接的顺序：
/// 1. 最终跳转 URL 是详情页 → 以它为准；下载链接只在 id 对得上时才用；
/// 2. 正文里详情和下载链接 id 相同 → 用这一对；
/// 3. 只有唯一详情或唯一下载 → 用唯一的一个；
/// 4. 多个链接但 id 对不上 → 两个都留空，交给 `posted_unresolved` 人工确认，
///    宁可不挂也不能把旧种挂进 qB。
pub fn extract_posted_links(body: &str, final_url: &str, base: &Url) -> PostedLinks {
    let mut body_details: Vec<(String, String)> = Vec::new();
    let mut body_downloads: Vec<(String, String)> = Vec::new();
    collect_id_links(body, base, "details.php?id=", &mut body_details);
    collect_id_links(body, base, "download.php?id=", &mut body_downloads);

    let mut final_details: Vec<(String, String)> = Vec::new();
    let mut final_downloads: Vec<(String, String)> = Vec::new();
    collect_id_links(final_url, base, "details.php?id=", &mut final_details);
    collect_id_links(final_url, base, "download.php?id=", &mut final_downloads);

    if let Some((fid, furl)) = final_details.first() {
        if let Some((_, durl)) = final_downloads
            .iter()
            .chain(body_downloads.iter())
            .find(|(id, _)| id == fid)
        {
            return PostedLinks {
                detail: Some(furl.clone()),
                download: Some(durl.clone()),
            };
        }
        return PostedLinks {
            detail: Some(furl.clone()),
            download: None,
        };
    }
    if let [(did, durl)] = final_downloads.as_slice() {
        if let Some((_, detail)) = body_details.iter().find(|(id, _)| id == did) {
            return PostedLinks {
                detail: Some(detail.clone()),
                download: Some(durl.clone()),
            };
        }
        return PostedLinks {
            detail: None,
            download: Some(durl.clone()),
        };
    }

    // 正文里 id 对得上的详情/下载对。唯一一对才可信。
    let pairs: Vec<_> = body_downloads
        .iter()
        .filter_map(|d| {
            body_details
                .iter()
                .find(|(id, _)| id == &d.0)
                .map(|detail| (detail, d))
        })
        .collect();
    if let [(detail, download)] = pairs.as_slice() {
        return PostedLinks {
            detail: Some(detail.1.clone()),
            download: Some(download.1.clone()),
        };
    }
    if !pairs.is_empty() {
        return PostedLinks::default();
    }

    // 只有一类链接时，唯一的那条还能用；多条就分不清新旧。
    if body_downloads.is_empty() && body_details.len() == 1 {
        return PostedLinks {
            detail: Some(body_details[0].1.clone()),
            download: None,
        };
    }
    if body_details.is_empty() && body_downloads.len() == 1 {
        return PostedLinks {
            detail: None,
            download: Some(body_downloads[0].1.clone()),
        };
    }
    PostedLinks::default()
}

/// 发后定位的站内检索兜底：按发布名 + 本地总大小找刚发布的新种。
///
/// 只在 takeupload.php 回包/跳转没有给出详情或下载链接时使用。命中后先核对详情页标题，
/// 再核对详情页大小（结果行大小解析不出来时也要过这一关），最后只下载站点生成的
/// `.torrent`；不会拿本地上传构造的种子顶替站种，避免把审核没过的废种挂进 qB。
/// `held` 是出参：候选页面找到了、但站点因为审核没放行不给下载时，记下（详情页，审核结论），
/// 调用方据此落“等站点审核”，而不是“定位不到新种”。
fn locate_posted_by_search(
    tgt: &NexusPhp,
    info: &ReleaseInfo,
    expected_size: Option<i64>,
    relaxed_retry: bool,
    throttle: &SiteThrottle,
    log: &mut dyn FnMut(&str),
    held: &mut Option<(String, ReviewMark, String)>,
) -> Option<(String, Vec<u8>)> {
    for relaxed in [false, true] {
        if relaxed && !relaxed_retry {
            break;
        }
        let mut gate = || throttle.acquire(&tgt.site_id, BudgetKind::Search);
        let outcome = match tgt.search_sized(
            &info.name,
            "",
            relaxed,
            info.imdb.as_deref().unwrap_or(""),
            expected_size,
            &mut gate,
        ) {
            Ok(SearchOutcome::Hit(h)) => h,
            Ok(SearchOutcome::Miss) => continue,
            Ok(SearchOutcome::BudgetExhausted) => {
                log("发后定位·检索预算用完，转 posted_unresolved");
                return None;
            }
            Err(e) => {
                log(&format!("发后定位·站内检索失败（{e:#}）"));
                return None;
            }
        };

        if !throttle.acquire(&tgt.site_id, BudgetKind::Search) {
            log("发后定位·详情页预算用完，转 posted_unresolved");
            return None;
        }
        let page = match tgt.fetch_torrent_page(&outcome.detail_url) {
            Ok(p) => p,
            Err(e) => {
                log(&format!("发后定位·候选详情页读不下来（{e:#}）"));
                continue;
            }
        };
        let page_title = strip_site_title_badges(&page.name);
        if !search_names_match(page_title, &info.name, true)
            && norm_post_title(page_title) != norm_post_title(&info.name)
        {
            log(&format!(
                "发后定位·候选标题不符：页面「{}」，期望「{}」",
                page.name, info.name
            ));
            continue;
        }
        if let Some(expected) = expected_size {
            match page.size_bytes {
                Some(actual)
                    if !sizes_close_with_slack(actual, expected, page.size_slack_bytes) =>
                {
                    log(&format!(
                        "发后定位·候选大小不符：页面 {} 字节，期望 {} 字节",
                        actual, expected
                    ));
                    continue;
                }
                None => log("发后定位·详情页大小解析不到，继续用 pieces 核对"),
                _ => {}
            }
        }

        if !throttle.acquire(&tgt.site_id, BudgetKind::Search) {
            log("发后定位·下载预算用完，转 posted_unresolved");
            return None;
        }
        match tgt.download_torrent(&page, &outcome.detail_url) {
            Ok(data) => {
                log(&format!(
                    "发后定位·站内检索命中：{}（{}）",
                    outcome.detail_url, page.name
                ));
                return Some((outcome.detail_url, data));
            }
            Err(e) => match site_review_hold(&e) {
                // 新种找到了，只是站点要先审才给下载：这就是刚发的那条，记下交给调用方落格
                Some((mark, why)) => {
                    *held = Some((outcome.detail_url.clone(), mark, why));
                    return None;
                }
                None => log(&format!("发后定位·候选站点种下载失败（{e:#}）")),
            },
        }
    }
    None
}

/// 下载失败是不是因为站点审核没放行（`SiteNotice`，且提示里认出了审核结论）。
/// “通过”不算：站点标着通过却不给下载，那是别的问题，走普通的下载失败路径。
fn site_review_hold(err: &anyhow::Error) -> Option<(ReviewMark, String)> {
    err.downcast_ref::<SiteNotice>()?
        .review
        .clone()
        .filter(|(m, _)| *m != ReviewMark::Approved)
}

/// 站点收下了种子，但审核没放行（要先审，或已经判了不通过）：拿不到站点种子，挂不了 qB。
/// 这不是发种失败：不记 failed、不进失败冷却、不重发（重发只会得到重复种）；
/// 落专门的格子，等回访（`bsend followup` / 自动发种每轮开头）收尾。
fn review_hold_outcome(
    index: &mut ContentIndex,
    key: &str,
    site: &str,
    url: Option<String>,
    mark: ReviewMark,
    why: &str,
    log: &mut dyn FnMut(&str),
) -> Result<ReseedOutcome> {
    let (status, note) = match mark {
        ReviewMark::Pending => (
            CoverageStatus::PendingReview,
            format!("站上已发布，等站点审核（站点原话：{why}）；审完前拿不到站点种子，没挂回 qB"),
        ),
        ReviewMark::Rejected => (
            CoverageStatus::ReviewRejected,
            format!("站点审核没过（{why}）；没挂回 qB。改简介或种子后在站上处理，程序不重发"),
        ),
        // site_review_hold 已经把“通过”滤掉了；真走到这里，说明不是审核在等，按原文记下来让人看
        ReviewMark::Approved => (
            CoverageStatus::ReviewPending,
            format!("posted_unresolved：站点页面标着“通过”却不给下载（{why}），待人工确认；勿重发"),
        ),
    };
    log(&format!("站点审核·{}：{}", status.label(), note));
    write_cell(index, key, site, status, url.clone(), &note);
    index.save()?;
    Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
        status,
        url,
        note,
    }))
}

/// 站点让下载、种子也挂回 qB 在做种了，但页面上还标着审核结论（tey：`title="未审"`；
/// 麒麟：“审核不通过，原因：…”）。做种不受影响，可这条种子留不留得下来要看审核，
/// 所以格子按审核结论落，等回访收尾。
fn review_seeding_outcome(
    index: &mut ContentIndex,
    key: &str,
    site: &str,
    detail_url: &str,
    mark: ReviewMark,
    why: &str,
    log: &mut dyn FnMut(&str),
) -> Result<ReseedOutcome> {
    let (status, note) = match mark {
        ReviewMark::Pending => (
            CoverageStatus::PendingReview,
            format!("站上已发布，种子已挂回 qB 做种；站点页面还标着“{why}”，等审核结果"),
        ),
        ReviewMark::Rejected => (
            CoverageStatus::ReviewRejected,
            format!("站点审核没过：{why}；种子已挂回 qB，仍在做种"),
        ),
        // 调用前已经把“通过”滤掉了；标着通过就是正常发布完成，格子不动
        ReviewMark::Approved => return Ok(ReseedOutcome::Posted(detail_url.to_string())),
    };
    log(&format!("站点审核·{}：{}", status.label(), note));
    write_cell(
        index,
        key,
        site,
        status,
        Some(detail_url.to_string()),
        &note,
    );
    index.save()?;
    Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
        status,
        url: Some(detail_url.to_string()),
        note,
    }))
}

/// existed 回包详情页标题核对：只采信回包给的 hint 页面，标题一致才返回。
/// 这里不做站内检索校正；新发后的无链接兜底检索由 `locate_posted_by_search` 负责。
/// 不用 IMDb 区兜底：IMDb 命中只说明同一部片子，不能说明同一条种子。
fn locate_existing_target(
    tgt: &NexusPhp,
    info: &ReleaseInfo,
    hint: &str,
    log: &mut dyn FnMut(&str),
) -> Option<String> {
    if hint.is_empty() {
        return None;
    }
    match tgt.fetch_torrent_page(hint) {
        Ok(p) => {
            let page_title = strip_site_title_badges(&p.name);
            if norm_post_title(page_title) == norm_post_title(&info.name) {
                Some(hint.to_string())
            } else {
                log(&format!(
                    "existed 回包详情页标题不符（页面「{}」，用于比对「{}」），弃用该线索",
                    p.name, page_title
                ));
                None
            }
        }
        Err(e) => {
            log(&format!("existed 回包详情页打不开（{e:#}），弃用该线索"));
            None
        }
    }
}

/// 详情页标题常被站点追加促销/剩余时间等后缀（如 `[免费] 剩余时间：23时59分钟`）。
/// 这些不是标题正文，比对 existed 线索前先裁掉，避免把真重复误判成“标题不符”。
fn strip_site_title_badges(title: &str) -> &str {
    const BADGES: [&str; 12] = [
        "[免费]",
        "[免費]",
        "[Free]",
        "[FREE]",
        "[2x]",
        "[2X]",
        "[50%]",
        "[促销]",
        "[Promotion]",
        "[HR]",
        "[官方]",
        "剩余时间",
    ];
    // 裁在标题里最早出现的角标上，而不是内置列表里靠前的那个：
    // 标题是 `X [2X] [Free]` 时若先按列表顺序裁 [Free]，会留下 `X [2X]` 仍带着角标。
    let mut cuts: Vec<usize> = BADGES.iter().filter_map(|b| title.find(b)).collect();
    cuts.sort_unstable();
    cuts.dedup();
    for pos in cuts {
        let core = title[..pos].trim();
        if !core.is_empty() {
            return core;
        }
    }
    title.trim()
}

/// 发后自检：回读新种页核对标题/MediaInfo/链接。同一次请求顺带带回页面上明写的审核结论
/// （未审 / 审核不通过），不额外打站点。抓不到页面时只返回一条警告，审核结论为 `None`。
pub fn verify_posted(
    target: &NexusPhp,
    detail_url: &str,
    info: &ReleaseInfo,
) -> (Vec<String>, Option<(ReviewMark, String)>) {
    match target.fetch_html(detail_url, target.base.as_str()) {
        Ok(html) => (
            verify_posted_html(&html, info),
            review_mark(&html, &info.name),
        ),
        Err(e) => (
            vec![format!("抓新种详情页失败（{e:#}），请手动核对发布结果")],
            None,
        ),
    }
}

/// 发后自检的纯文本比对（单测友好）
fn verify_posted_html(html: &str, info: &ReleaseInfo) -> Vec<String> {
    let mut warns = Vec::new();
    let posted = parse_release_page(html, "");
    // 标题
    let want = info.name.trim();
    let got = posted.name.trim();
    if got.is_empty() {
        warns.push("新种页没解析出标题，无法核对".into());
    } else if !want.is_empty() && got != want {
        warns.push(format!("标题与预期不一致：预期「{want}」页面是「{got}」"));
    }
    // MediaInfo：独立块没有就退化成「页面里出现了原文行」
    if !info.mediainfo.trim().is_empty() {
        let landed = !posted.mediainfo.trim().is_empty()
            || info
                .mediainfo
                .lines()
                .map(str::trim)
                .filter(|l| l.len() >= 8)
                .take(5)
                .any(|l| html.contains(l));
        if !landed {
            warns.push("MediaInfo 没落到新种页：确认技术信息/简介里带上了".into());
        }
    }
    // 条目链接（按 id 本体找，页面展示形式可能带完整 URL）
    if let Some(tt) = &info.imdb {
        if !html.contains(tt) {
            warns.push(format!("IMDb id {tt} 没在新种页出现"));
        }
    }
    if let Some(db) = &info.douban {
        if !html.contains(db) {
            warns.push(format!("豆瓣 id {db} 没在新种页出现"));
        }
    }
    if let Some(tm) = &info.tmdb {
        let tail = tm.rsplit('/').next().unwrap_or("");
        if !tail.is_empty() && !html.contains(tail) {
            warns.push("TMDB 链接没在新种页出现".into());
        }
    }
    warns
}

// ---------------- 发种表格演练（只审不发，全站体检） ----------------

/// 发种表格演练结果：逐站核对 upload.php 表单与字段构造的完备性
#[derive(Debug, Default)]
pub struct FormDrill {
    pub site_id: String,
    pub site_name: String,
    /// 表单没拿到/构造失败的原因（登录墙、限流、无上传权限…）
    pub error: Option<String>,
    pub field_count: usize,
    pub select_count: usize,
    pub title_set: bool,
    /// 标题输入框在表单里不存在（tjupt 实测：标题由种子文件名自动生成）
    pub title_auto: bool,
    pub subtitle_set: bool,
    pub descr_bytes: usize,
    /// MediaInfo 落到哪：独立字段名，或「内嵌简介」
    pub mediainfo_field: Option<String>,
    /// 条目链接落到的字段名
    pub link_fields: Vec<String>,
    pub picked_tags: Vec<String>,
    pub tag_warn: Option<String>,
    /// 自动选档结果：(字段, 选项文案)
    pub select_picks: Vec<(String, String)>,
    /// 仍为 0/空的下拉（缺口与否由 core_select_gaps 归类）
    pub open_selects: Vec<String>,
    /// 硬缺口下拉（前缀级折叠；域无关与垂直站分类不算）
    pub core_select_gaps: Vec<String>,
    pub notes: Vec<String>,
    /// 上传页是否带官方预审面板（LuckAudit）
    pub audit_panel: bool,
    /// 表单解析出的提交地址（绝对 URL）；构造失败时为 None
    pub action: Option<String>,
    /// 每个上传下拉的全量选项：(下拉名, [(value, label)])，供 formcheck 落盘核对缺口
    pub select_options: Vec<(String, Vec<(String, String)>)>,
}

impl FormDrill {
    /// 硬缺口：分类 + 核心质量下拉仍为 0/空，或名称/简介/MediaInfo 没落上。
    /// 标题由站点自动生成（无标题输入框）、垂直站分类不适配不算缺口。
    pub fn core_gaps(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(e) = &self.error {
            out.push(e.clone());
            return out;
        }
        if !self.title_set && !self.title_auto {
            out.push("标题未落上".into());
        }
        if self.descr_bytes == 0 {
            out.push("简介为空".into());
        }
        if self.mediainfo_field.is_none() {
            out.push("MediaInfo 无处可落".into());
        }
        for s in &self.core_select_gaps {
            out.push(format!("{s} 未选"));
        }
        out
    }

    /// 软缺口：制作组/处理/地区等次要或域无关下拉未选、选档提示
    pub fn soft_gaps(&self) -> Vec<String> {
        let mut out = Vec::new();
        for s in &self.open_selects {
            // 已按前缀点名进硬缺口的不重复（多组同前缀只报一次）
            let covered = self
                .core_select_gaps
                .iter()
                .any(|c| s == c || (s.starts_with(c) && s[c.len()..].starts_with('[')));
            if !covered {
                out.push(format!("{s} 未选"));
            }
        }
        out.extend(self.notes.iter().cloned());
        if let Some(w) = &self.tag_warn {
            out.push(w.clone());
        }
        out
    }
}

/// 演练样本：覆盖 中字/国产剧/HDTV/1080p/x264/AAC/制作组/IMDb/豆瓣/MediaInfo 全要素
pub fn drill_release() -> ReleaseInfo {
    ReleaseInfo {
        name: "The.Disguiser.Complete.1080p.HDTV.x264.AAC-JING".into(),
        subtitle: "国产剧--伪装者[无台标完整版 全41集1080P][中字]".into(),
        descr: "演练样本简介（中字）。IMDb：https://www.imdb.com/title/tt5154296/\n豆瓣：https://movie.douban.com/subject/25823432/\n截图：<img src=\"https://img.example/shot01.jpg\"><img src=\"https://img.example/shot02.jpg\"><img src=\"https://img.example/shot03.jpg\">".into(),
        mediainfo: "General\nUnique ID : 1\nFormat : Matroska\nDuration : 47mn 17s\nOverall bit rate : 6000 kb/s\n\nVideo\nFormat : AVC\nFormat/Info : Advanced Video Codec\nWidth : 1920 pixels\nHeight : 1080 pixels\n\nAudio\nLanguage : Chinese\nFormat : AAC\nChannel(s) : 2 channels".into(),
        imdb: Some("tt5154296".into()),
        douban: Some("25823432".into()),
        tmdb: None,
        completed_hint: true,
    }
}

/// 逐站演练：与真实转种同一套字段构造（prepare_upload），但绝不 POST
pub fn drill_upload_form(target: &NexusPhp) -> FormDrill {
    let info = drill_release();
    match prepare_upload(target, &info) {
        Ok(prep) => analyze_form(&target.site_id, &target.site_name, &prep, &info),
        Err(e) => FormDrill {
            site_id: target.site_id.clone(),
            site_name: target.site_name.clone(),
            error: Some(format!("{e:#}")),
            ..Default::default()
        },
    }
}

/// 上传页拿不到表单时的细分原因（权限/登录）；None = 看不出所以然
fn upload_denied_reason(html: &str) -> Option<String> {
    // 简繁都要：实测繁体站用「沒有發布種子的許可權」一类写法（許可權 ≠ 權限）
    const DENIED: &[&str] = &[
        "没有发布种子的权限",
        "沒有發布種子的許可權",
        "沒有發佈種子的許可權",
        "没有发布权限",
        "沒有發布權限",
        "沒有發佈權限",
        "没有上传权限",
        "沒有上傳權限",
        "没有发布",
        "沒有發布",
        "沒有發佈",
        "上传权限不足",
        "上傳權限不足",
        "权限不足",
        "權限不足",
        "許可權不足",
        "等级不足",
        "等級不足",
        "不允许上传",
        "不允許上傳",
        "禁止上传",
        "禁止上傳",
    ];
    for kw in DENIED {
        if html.contains(kw) {
            return Some(format!("该账号无发布权限（页面提示含「{kw}」）"));
        }
    }
    let lower = html.to_ascii_lowercase();
    if lower.contains("type=\"password\"") || lower.contains("type='password'") {
        return Some("登录态失效（上传页是登录表单）".into());
    }
    None
}

/// 供门禁/自动发种复用：按演练同一套逻辑分析「已构造好的上传字段」
pub fn analyze_prepared(
    site_id: &str,
    site_name: &str,
    prep: &PreparedUpload,
    info: &ReleaseInfo,
) -> FormDrill {
    analyze_form(site_id, site_name, prep, info)
}

/// 演练分析（纯函数，单测友好）
fn analyze_form(
    site_id: &str,
    site_name: &str,
    prep: &PreparedUpload,
    info: &ReleaseInfo,
) -> FormDrill {
    let f = |k: &str| field_lookup(&prep.fields, k).unwrap_or("").to_string();
    let mut d = FormDrill {
        site_id: site_id.into(),
        site_name: site_name.into(),
        field_count: prep.fields.len(),
        select_count: select_names(&prep.form_html).len(),
        ..Default::default()
    };
    let title = if !f("name").is_empty() {
        f("name")
    } else {
        f("title")
    };
    d.title_set = title == info.name;
    // 无标题输入框（tjupt 实测）：标题由种子文件名自动生成，不算缺口
    d.title_auto =
        !d.title_set && !form_has(&prep.form_html, "name") && !form_has(&prep.form_html, "title");
    d.subtitle_set = f("small_descr") == info.subtitle || f("subtitle") == info.subtitle;
    // 简介：NexusPHP descr / Gazelle 系 release_desc
    let descr_text = ["descr", "release_desc", "description", "content"]
        .iter()
        .find_map(|k| {
            let v = f(k);
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        })
        .unwrap_or_default();
    d.descr_bytes = descr_text.len();
    for cand in ["technical_info", "media_info", "media_bdinfo", "mediainfo"] {
        if !f(cand).is_empty() {
            d.mediainfo_field = Some(cand.to_string());
            break;
        }
    }
    if d.mediainfo_field.is_none() && descr_text.contains("MediaInfo") {
        d.mediainfo_field = Some("内嵌简介".into());
    }
    for l in [
        "imdbid",
        "imdb_url",
        "url",
        "douban",
        "douban_id",
        "douban_url",
    ] {
        if !f(l).is_empty() {
            d.link_fields.push(l.to_string());
        }
    }
    d.picked_tags = prep.picked_tags.clone();
    d.tag_warn = prep.tag_warn.clone();
    d.notes = prep.select_notes.clone();
    // JS 驱动的自定义上传界面（hdcity 实测）：简介/MediaInfo 静态填不进
    let form = upload_form_block(&prep.form_html).unwrap_or(&prep.form_html);
    if js_driven_form(form) {
        d.notes.push(
            "该站上传界面是 JS 驱动（表单仅种子+隐藏字段），简介/MediaInfo 需上传后在网页里补"
                .into(),
        );
    }
    d.audit_panel = pre_audit_supported(&prep.form_html);
    d.action = Some(prep.action.clone());
    d.select_options = drill_select_options(&prep.form_html);
    for name in select_names(&prep.form_html) {
        // BBCode 编辑器的 color/font/size 装饰下拉与发种完备性无关
        if matches!(name.as_str(), "color" | "font" | "size") {
            continue;
        }
        let v = f(&name);
        if v.is_empty() || v == "0" {
            d.open_selects.push(name);
        } else if name == "type" || name.contains("_sel") {
            let label = select_option_meta(&prep.form_html, &name, &v)
                .map(|(l, _)| l)
                .unwrap_or_else(|| v.clone());
            d.select_picks.push((name, label));
        }
    }
    // 硬缺口下拉：域相关 + 前缀折叠（多组分类分支只要一组填上就不算）；
    // 垂直站分类与样本不适配不算表单缺口
    const CORE_QUALITY: &[(&str, &[&str])] = &[
        ("medium_sel", MEDIUM_KEYWORDS),
        ("codec_sel", CODEC_KEYWORDS),
        ("standard_sel", STANDARD_KEYWORDS),
        ("audiocodec_sel", AUDIO_KEYWORDS),
    ];
    if d.open_selects.iter().any(|s| s == "type") {
        let opts = select_options(&prep.form_html, "type");
        if !type_niche_miss(&opts, info) {
            d.core_select_gaps.push("type".into());
        }
    }
    for (prefix, kws) in CORE_QUALITY {
        let hit = |n: &String| {
            n == prefix || (n.starts_with(prefix) && n[prefix.len()..].starts_with('['))
        };
        if !d.open_selects.iter().any(&hit) {
            continue;
        }
        if d.select_picks.iter().any(|(n, _)| hit(n)) {
            continue;
        }
        // 域无关的同名下拉（如书站 audiocodec_sel 实为电子书格式）不算质量缺口
        if d.open_selects.iter().any(|s| {
            hit(s) && {
                let opts = select_options(&prep.form_html, s);
                !opts.is_empty() && options_in_domain(&opts, kws)
            }
        }) {
            d.core_select_gaps.push((*prefix).to_string());
        }
    }
    d
}

// ---------------- 站点官方预审（LuckAudit pre-audit） ----------------

/// 官方预审的一条问题（level：error / warning / suspicious / …）
#[derive(Debug, Clone)]
pub struct AuditDetail {
    pub level: String,
    pub message: String,
    /// 扣分（正数）
    pub score: i64,
    pub suggestion: Option<String>,
}

impl AuditDetail {
    /// 人看的一行：❌ 消息（-8 分）→ 建议
    pub fn line(&self) -> String {
        let icon = match self.level.to_ascii_uppercase().as_str() {
            "ERROR" => "❌",
            "WARNING" => "⚠️",
            "SUSPICIOUS" => "🔍",
            _ => "·",
        };
        let mut s = format!("{icon} {}（-{} 分）", self.message, self.score);
        if let Some(a) = &self.suggestion {
            if !a.is_empty() {
                s.push_str(&format!(" → {a}"));
            }
        }
        s
    }
}

/// 站点官方预审结论（`POST /api/auto-audit/pre-audit`，luckaudit.js 同款协议）
#[derive(Debug, Clone)]
pub struct AuditVerdict {
    pub passed: bool,
    pub status: String,
    pub total_score: i64,
    pub error_count: i64,
    pub warning_count: i64,
    pub suspicious_count: i64,
    pub details: Vec<AuditDetail>,
    pub suggestions: Vec<String>,
}

impl AuditVerdict {
    /// 人看的一行摘要
    pub fn summary(&self) -> String {
        format!(
            "{}（总分 {}/100｜❌ {} ⚠️ {} 🔍 {}）",
            self.status,
            self.total_score,
            self.error_count,
            self.warning_count,
            self.suspicious_count
        )
    }
}

/// 上传页带 LuckAudit 面板（`luckaudit.js`）的站才有官方预审接口
pub fn pre_audit_supported(form_html: &str) -> bool {
    form_html.to_ascii_lowercase().contains("luckaudit")
}

/// 调站点官方预审核（LuckAudit）：把将提交的字段原样交给它打分。
/// 只应在 `pre_audit_supported` 为真时调用。
///
/// 传输走表单编码（`post_form`）而非 JSON：站点 Cloudflare WAF 拦 JSON POST，
/// 表单畅通且应用 `$request->input()` 能读 PHP 嵌套语法（`type[id]=…`）。
pub fn run_pre_audit(
    target: &NexusPhp,
    info: &ReleaseInfo,
    prep: &PreparedUpload,
) -> Result<AuditVerdict> {
    let payload = build_audit_payload(info, prep);
    let form = json_to_form(&payload);
    let url = resolve_url("/api/auto-audit/pre-audit", &target.base);
    let (status, body) = target
        .post_form(&url, &form, &prep.form_url)
        .context("调用官方预审接口失败")?;
    parse_audit_response(status, &body)
}

/// JSON 值摊平成 PHP 数组语法的表单字段对：
/// `{"type":{"id":"404"}}` → `type[id]=404`；`tags:[{id:23}]` → `tags[0][id]=23`。
/// null 与空对象/空数组不出字段（服务端把缺字段当「未选」处理，正是预审要判的）。
fn json_to_form(v: &serde_json::Value) -> Vec<Field> {
    let mut out = Vec::new();
    flatten_form(v, "", &mut out);
    out
}

fn flatten_form(v: &serde_json::Value, prefix: &str, out: &mut Vec<Field>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}[{k}]")
                };
                flatten_form(val, &key, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, val) in arr.iter().enumerate() {
                flatten_form(val, &format!("{prefix}[{i}]"), out);
            }
        }
        serde_json::Value::Null => {}
        serde_json::Value::String(s) => out.push((prefix.to_string(), s.clone())),
        other => out.push((prefix.to_string(), other.to_string())),
    }
}

fn parse_audit_response(status: u16, body: &str) -> Result<AuditVerdict> {
    if !body.trim_start().starts_with('{') {
        let head: String = body.chars().take(80).collect();
        bail!("官方预审返回了非 JSON（HTTP {status}，可能登录态失效）：{head}");
    }
    let v: serde_json::Value = serde_json::from_str(body).context("官方预审响应解析失败")?;
    if v.get("success").and_then(|s| s.as_bool()) == Some(false) {
        let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
        let err = v.get("errors").map(|e| e.to_string()).unwrap_or_default();
        bail!("官方预审接口报错：{msg}{err}");
    }
    let data = v
        .get("data")
        .ok_or_else(|| anyhow!("官方预审响应缺 data 段"))?;
    let details = data
        .get("details")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .map(|d| AuditDetail {
                    level: d.get("level").and_then(|x| x.as_str()).unwrap_or("").into(),
                    message: d
                        .get("message")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .into(),
                    score: d.get("score").and_then(|x| x.as_i64()).unwrap_or(0),
                    suggestion: d
                        .get("suggestion")
                        .and_then(|x| x.as_str())
                        .map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();
    let suggestions = data
        .get("suggestions")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(AuditVerdict {
        passed: data
            .get("passed")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        status: data
            .get("status")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .into(),
        total_score: data.get("totalScore").and_then(|x| x.as_i64()).unwrap_or(0),
        error_count: data.get("errorCount").and_then(|x| x.as_i64()).unwrap_or(0),
        warning_count: data
            .get("warningCount")
            .and_then(|x| x.as_i64())
            .unwrap_or(0),
        suspicious_count: data
            .get("suspiciousCount")
            .and_then(|x| x.as_i64())
            .unwrap_or(0),
        details,
        suggestions,
    })
}

/// 官方预审载荷：与 luckaudit.js `collectTorrentInfo` 同构——如实反映将提交的字段
/// （简介含自动补的链接段、质量下拉含 `X_sel[n]` 下标形式）
fn build_audit_payload(info: &ReleaseInfo, prep: &PreparedUpload) -> serde_json::Value {
    let f = |key: &str| -> String {
        field_lookup(&prep.fields, key)
            .unwrap_or_default()
            .to_string()
    };
    // 简介剥掉内联事件属性再送审：站点 WAF 对表单体里的 on* 事件属性计异常分
    // （实测 2 处放行、4 处即 403「网站防火墙」），NexusPHP 截图段天然带一串
    // onload/onclick；对预审打分毫无意义，只改预审载荷，真实上传一字不动。
    let description = strip_event_handlers(&f("descr"));
    let technical_info = ["technical_info", "media_info", "media_bdinfo", "mediainfo"]
        .iter()
        .map(|n| f(n))
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| info.mediainfo.clone());

    let mut external = serde_json::Map::new();
    if let Some(tt) = &info.imdb {
        external.insert(
            "imdb".into(),
            format!("https://www.imdb.com/title/{tt}/").into(),
        );
    }
    if let Some(db) = &info.douban {
        external.insert(
            "douban".into(),
            format!("https://movie.douban.com/subject/{db}/").into(),
        );
    }
    if let Some(tm) = &info.tmdb {
        external.insert("tmdb".into(), tm.clone().into());
    }

    // 分类：value 0/空 = 未选（luckaudit.js 同款判法）
    let type_val = f("type");
    let type_obj = if type_val.is_empty() || type_val == "0" {
        serde_json::Value::Null
    } else {
        let (label, class_name) =
            select_option_meta(&prep.form_html, "type", &type_val).unwrap_or_default();
        serde_json::json!({
            "id": type_val,
            "name": label,
            "mode": select_mode(&prep.form_html, "type").unwrap_or_default(),
            "class_name": class_name,
        })
    };

    // 质量：source/medium/codec/standard/processing/audiocodec/team（`X_sel` 或 `X_sel[n]`）
    let mut quality = serde_json::Map::new();
    for key in [
        "source",
        "medium",
        "codec",
        "standard",
        "processing",
        "audiocodec",
        "team",
    ] {
        let prefix = format!("{key}_sel");
        let Some((fname, val)) = prep.fields.iter().find(|(n, _)| {
            n == &prefix || (n.starts_with(&prefix) && n[prefix.len()..].starts_with('['))
        }) else {
            continue;
        };
        if val.is_empty() || val == "0" {
            continue;
        }
        let (label, _) = select_option_meta(&prep.form_html, fname, val).unwrap_or_default();
        quality.insert(
            key.to_string(),
            serde_json::json!({ "id": val, "name": label }),
        );
    }

    // 标签：value + 复选框文案
    let tags: Vec<serde_json::Value> = prep
        .picked_tags
        .iter()
        .map(|v| serde_json::json!({ "id": v, "name": checkbox_label(&prep.form_html, v) }))
        .collect();

    let now = iso_utc(now_unix());
    serde_json::json!({
        "name": info.name.trim(),
        "small_descr": info.subtitle.trim(),
        "pt_gen_url": f("pt_gen"),
        "imdb_url": external.get("imdb").cloned().unwrap_or_default(),
        "external_links": external,
        "description": description.trim(),
        "technical_info": technical_info.trim(),
        "type": type_obj,
        "quality": quality,
        "tags": tags,
        "collected_at": now,
        "export_time": now,
        "page_url": prep.form_url,
        "user_info": extract_luck_audit_user(&prep.form_html)
            .unwrap_or(serde_json::Value::Null),
    })
}

/// 剥掉 HTML 标签内的内联事件属性（`onload="…"` / `onclick='…'`，大小写不敏感）。
/// 只动标签内部，正文里出现的 "on=…" 字样不受影响；多字节 UTF-8 原样保留。
/// 站点 WAF 对表单体里的 `on*=` 按出现次数异常计分（实测 2 处放行、4 处即
/// 403「网站防火墙」），NexusPHP 简介截图段天然带一串——对预审打分无意义。
fn strip_event_handlers(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut seg_start = 0;
    let mut in_tag = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => in_tag = true,
            b'>' => in_tag = false,
            _ => {}
        }
        // 属性名起笔：标签内、空白后的 onXxx=（至少 2 个字母，避免误伤 on=）
        if in_tag
            && (bytes[i] | 0x20) == b'o'
            && i > 0
            && bytes[i - 1].is_ascii_whitespace()
            && bytes.get(i + 1).map(|b| b | 0x20) == Some(b'n')
        {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                j += 1;
            }
            if j >= i + 4 && bytes.get(j) == Some(&b'=') {
                j += 1;
                match bytes.get(j) {
                    Some(&q @ (b'"' | b'\'')) => {
                        j += 1;
                        while j < bytes.len() && bytes[j] != q {
                            j += 1;
                        }
                        if j < bytes.len() {
                            j += 1;
                        }
                    }
                    _ => {
                        while j < bytes.len() && !bytes[j].is_ascii_whitespace() && bytes[j] != b'>'
                        {
                            j += 1;
                        }
                    }
                }
                // 连同属性前的空格一起吞掉，不留双空格
                let mut seg_end = i;
                while seg_end > seg_start && bytes[seg_end - 1] == b' ' {
                    seg_end -= 1;
                }
                out.push_str(&html[seg_start..seg_end]);
                seg_start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&html[seg_start..]);
    out
}

/// 字段查找：精确名，或 `key[...]` 下标形式（medium_sel[4]）
fn field_lookup<'a>(fields: &'a [Field], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(n, _)| n == key || (n.starts_with(key) && n[key.len()..].starts_with('[')))
        .map(|(_, v)| v.as_str())
}

/// `<select name=field>` 里指定 value 的选项文案与 class-name（同名多块全找）
fn select_option_meta(page: &str, field: &str, value: &str) -> Option<(String, String)> {
    for block in select_blocks(page, field) {
        let mut rest = block;
        while let Some(p) = rest.to_ascii_lowercase().find("<option") {
            let after = &rest[p..];
            let end = after.find('>').map(|j| j + 1).unwrap_or(after.len());
            let tag = &after[..end];
            if attr(tag, "value").as_deref() == Some(value) {
                let text_end = after[end..]
                    .find('<')
                    .map(|i| end + i)
                    .unwrap_or(after.len());
                let label = strip_tags(&after[end..text_end]).trim().to_string();
                let class_name = attr(tag, "data-class-name").unwrap_or_default();
                return Some((label, class_name));
            }
            rest = &after[end..];
        }
    }
    None
}

/// `<select name=field>` 的 data-mode 属性（分类联动模式）
fn select_mode(page: &str, field: &str) -> Option<String> {
    let block = select_block(page, field)?;
    let end = block.find('>')? + 1;
    attr(&block[..end], "data-mode")
}

/// 取 `<select name=field>` 整块（含开闭标签内文）；同名多块取第一块
fn select_block<'a>(page: &'a str, field: &str) -> Option<&'a str> {
    select_blocks(page, field).into_iter().next()
}

/// 同名 `<select>` 的全部块（分类分支的第二套下拉与首套同名）
fn select_blocks<'a>(page: &'a str, field: &str) -> Vec<&'a str> {
    let lower = page.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<select") {
        let start = i + p;
        let end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(page.len());
        if attr(&page[start..end], "name").as_deref() == Some(field) {
            let close = lower[start..]
                .find("</select>")
                .map(|j| start + j)
                .unwrap_or(page.len());
            out.push(&page[start..close]);
        }
        i = end;
    }
    out
}

/// 复选框 value 对应的文案：input 标签后到下一个 `<` 之前（包装标签时退 80 字符窗口）
fn checkbox_label(page: &str, value: &str) -> String {
    let lower = page.to_ascii_lowercase();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<input") {
        let start = i + p;
        let end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(page.len());
        let tag = &page[start..end];
        if tag.to_ascii_lowercase().contains("checkbox")
            && attr(tag, "value").as_deref() == Some(value)
        {
            let after = &page[end..floor_char_boundary(page, end + 80)];
            // 先取到下一个标签为止的纯文本；input 后直接跟标签（<label>…）再退窗口。
            // 下一个 <input 处截断，密集标签网格不误配邻格
            let next_input = after
                .to_ascii_lowercase()
                .find("<input")
                .unwrap_or(after.len());
            let cut = after[..next_input].find('<').unwrap_or(next_input);
            let inline = strip_tags(&after[..cut]).trim().to_string();
            if !inline.is_empty() {
                return inline;
            }
            return strip_tags(&after[..next_input]).trim().to_string();
        }
        i = end;
    }
    String::new()
}

/// 页面内嵌 `window.LUCK_AUDIT_USER = {...}`（官方预审 user_info 段）
fn extract_luck_audit_user(form_html: &str) -> Option<serde_json::Value> {
    let p = form_html.find("LUCK_AUDIT_USER")?;
    let brace = form_html[p..].find('{')? + p;
    let b = form_html.as_bytes();
    let mut depth = 0usize;
    let mut i = brace;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&form_html[brace..=i]).ok();
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// unix 秒 → `2026-11-18T07:50:00.000Z`（预审载荷时间戳；民用历换算不引 chrono）
fn iso_utc(unix: i64) -> String {
    let secs = unix.rem_euclid(86400);
    let days = unix.div_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

// ---------------- 转种流水线 ----------------

pub struct ReseedRequest {
    pub content_key: String,
    pub source_site: String,
    pub source_url: String,
    pub target_site: String,
    pub name: String,
}

/// 判重命中后的分类结果（不再把四种语义混成「空 URL」）
#[derive(Debug, Clone)]
pub struct DuplicateOutcome {
    /// `assisted`（同分片已覆盖）/ `other_version` / `failed`（复核失败）/ `review_pending`（existed）
    pub status: CoverageStatus,
    pub url: Option<String>,
    pub note: String,
}

/// 一次转种的结果：发布成功带详情页，或判重命中带明确分类。
/// 自动发种据此写审计（posted / skipped_duplicate / other_version / failed /
/// existed_pending / posted_unresolved）。
///
/// 「上传成功但回包定位不到新种」复用 `Duplicate(ReviewPending)`：note 以
/// `posted_unresolved` 开头，审计词据此区分；它不记 failed、不进失败冷却表、不计 posted 限额。
#[derive(Debug, Clone)]
pub enum ReseedOutcome {
    Posted(String),
    Duplicate(DuplicateOutcome),
}

impl ReseedOutcome {
    /// 审计 result 词表（auto-audit.jsonl / OPERATIONS 同步）
    pub fn audit_result(&self) -> &'static str {
        match self {
            ReseedOutcome::Posted(_) => "posted",
            ReseedOutcome::Duplicate(d) => match d.status {
                CoverageStatus::Assisted | CoverageStatus::Seeding => "skipped_duplicate",
                CoverageStatus::OtherVersion => "other_version",
                CoverageStatus::SameHashSkip => "same_hash_skipped",
                // 站点要人走候选入口：既不是失败（不进失败冷却），也不是“已结束”，
                // 单独一个审计词，周报里能数出来。
                CoverageStatus::PendingOffers => "pending_offers",
                // 站上已发布但要先过站点审核：等回访收尾，不是失败
                CoverageStatus::PendingReview => "pending_review",
                // 站点明写审核不通过：要人改种子/简介，程序不重发
                CoverageStatus::ReviewRejected => "review_rejected",
                CoverageStatus::ReviewPending => {
                    if d.note.starts_with("posted_unresolved") {
                        // 定位不到 ≠ 发种失败：不进 blocked/failed/review_failed 失败冷却表
                        "posted_unresolved"
                    } else {
                        "existed_pending"
                    }
                }
                _ => "failed",
            },
        }
    }

    /// 人工可读的一句话（CLI/GUI 共用）
    pub fn display(&self) -> String {
        match self {
            ReseedOutcome::Posted(url) => format!("转种完成：{url}"),
            ReseedOutcome::Duplicate(d) => format!("转种结束（{}）：{}", d.status.label(), d.note),
        }
    }
}

/// 源站解析 + 预检（`review_reseed` 与 `run_reseed` 共用）。
/// 预检不通过不拦在这里，由调用方决定（审阅要出报告，正式转种要拦）。
struct Released {
    src: NexusPhp,
    tgt: NexusPhp,
    page: TorrentPage,
    info: ReleaseInfo,
    preflight: Preflight,
}

/// 站点预审只认普通空格：源页 MediaInfo 常用 NBSP 对齐，先归一化
fn normalize_mediainfo(text: &str) -> String {
    text.replace(['\u{a0}', '\u{3000}'], " ")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

/// MediaInfo 是否满足站点预审的最低结构：General 有 Duration、Video 有 Width/Height。
fn mediainfo_is_complete(text: &str) -> bool {
    let mut section = String::new();
    let mut has_duration = false;
    let mut has_width = false;
    let mut has_height = false;
    for line in text.lines() {
        let l = line.trim();
        let low = l.to_ascii_lowercase();
        if ["general", "video", "audio", "text", "menu", "chapters"]
            .iter()
            .any(|p| low.starts_with(p))
        {
            section = low.split('#').next().unwrap_or("").to_string();
        }
        if section == "general" && low.starts_with("duration") {
            has_duration = true;
        }
        if section == "video" && low.starts_with("width") {
            has_width = true;
        }
        if section == "video" && low.starts_with("height") {
            has_height = true;
        }
    }
    has_duration && has_width && has_height
}

/// 源站详情页没有 MediaInfo 块时，从本地文件生成：内容 → 成员种子 → 落地路径 → 代表视频。
/// 失败只记提示（预检会如实报「MediaInfo 为空」），不阻断审阅。
fn disk_mediainfo(
    cfg: &Config,
    req: &ReseedRequest,
    log: &mut dyn FnMut(&str),
) -> Result<Option<String>> {
    let index = ContentIndex::load(&cfg.coverage_state_path());
    let Some(entry) = index.contents.get(&req.content_key) else {
        log("本地 MediaInfo：索引里找不到这段内容，跳过");
        return Ok(None);
    };
    let Some((hash, _)) = entry.pick_meta() else {
        log("本地 MediaInfo：内容没有成员种子，跳过");
        return Ok(None);
    };
    let qb = QBittorrent::new(
        &cfg.downloader.url,
        &cfg.downloader.username,
        &cfg.downloader.password,
    )?;
    let path = qb.torrent_content_path(hash).or_else(|| {
        qb.torrent_meta(hash)
            .ok()
            .flatten()
            .map(|m| join_path(&m.save_path, &m.name))
    });
    let Some(path) = path else {
        log("本地 MediaInfo：拿不到内容落地路径，跳过");
        return Ok(None);
    };
    let Some(media) = crate::mediainfo::pick_media_file(std::path::Path::new(&path)) else {
        log(&format!("本地 MediaInfo：目录里没找到视频文件（{path}）"));
        return Ok(None);
    };
    log(&format!("本地 MediaInfo：代表文件 {}", media.display()));
    match crate::mediainfo::generate(&media, cfg.coverage.media_info_dll.as_deref()) {
        Ok(text) => Ok(Some(text)),
        Err(e) => {
            log(&format!("本地 MediaInfo 生成失败：{e:#}"));
            Ok(None)
        }
    }
}

/// 目录拼接（qB 的 save_path 可能以 `/` 或 `\` 结尾）
fn join_path(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches(['/', '\\']), name)
}

/// 读取 dataDir/screenshot-overrides.json：contentKey → 公网截图 URL 列表。
/// 截图由独立工具从本地视频抽帧并上传图床后写入，不在发布时临时下载/伪造。
fn extra_images_for(cfg: &Config, content_key: &str) -> Option<Vec<String>> {
    let path = cfg
        .coverage_state_path()
        .parent()?
        .join("screenshot-overrides.json");
    let text = std::fs::read_to_string(path).ok()?;
    let map: std::collections::BTreeMap<String, Vec<String>> = serde_json::from_str(&text).ok()?;
    map.get(content_key).cloned().filter(|v| !v.is_empty())
}

fn prepare_released(
    cfg: &Config,
    req: &ReseedRequest,
    log: &mut dyn FnMut(&str),
) -> Result<Released> {
    let src = adapter_for(cfg, &req.source_site)?;
    let tgt = adapter_for(cfg, &req.target_site)?;

    // ---- 1 源站详情页 → 发布要素 ----
    log(&format!("拉源站详情页：{}", req.source_url));
    let page: TorrentPage = src.fetch_torrent_page(&req.source_url)?;
    let html = src.fetch_html(&req.source_url, src.base.as_str())?;
    let mut info = parse_release_page(&html, &page.name);
    if info.name.is_empty() {
        info.name = req.name.clone();
    }
    // 源页图片不足时，读取程序化截图清单（key → 公网图片 URL），注入简介。
    if let Some(urls) = extra_images_for(cfg, &req.content_key) {
        let mut add = String::new();
        let n = urls.len();
        for u in urls {
            add.push_str(&format!("<br/><img src=\"{u}\" />"));
        }
        info.descr.push_str(&add);
        log(&format!("注入本机视频截图 {n} 张"));
    }
    log(&format!(
        "发布要素：标题「{}」｜副标题「{}」｜MediaInfo {} 字节｜IMDb {:?}｜豆瓣 {:?}",
        info.name,
        info.subtitle,
        info.mediainfo.len(),
        info.imdb,
        info.douban
    ));

    // ---- 1.5 源站没有 MediaInfo 时，从本地文件生成（luckpt 等站发种硬性要求）----
    if info.mediainfo.trim().is_empty() || !mediainfo_is_complete(&info.mediainfo) {
        if let Ok(Some(mi)) = disk_mediainfo(cfg, req, log) {
            info.mediainfo = mi;
            log(&format!(
                "MediaInfo 已由本地文件生成（{} 字节）",
                info.mediainfo.len()
            ));
        }
    }

    // ---- 2 LuckAudit 预检（三查）----
    let tags: Vec<String> = wanted_tags(&info)
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let site_rules = cfg.coverage.rules_for(&req.target_site);
    let pf = preflight(&mut info, &tags, &site_rules);
    for f in &pf.fixes {
        log(&format!("预检·已自动补：{f}"));
    }
    for w in &pf.warnings {
        log(&format!("预检·警告：{w}"));
    }
    if pf.ok() {
        log(&format!("预检通过（{}）", pf.summary()));
    }
    Ok(Released {
        src,
        tgt,
        page,
        info,
        preflight: pf,
    })
}

/// 转种一条龙：解析源页 → 预检 → 官方预审 → 上传 → 发后自检 → 推种校验 → 格子落账。
/// `log` 收进度行；返回新种详情页链接。
///
/// 所有调用方（CLI / GUI / autopost）都会在这里落一条 `reseed-log.jsonl`：
/// 记录开始时间、源/目标站、结果与错误，用于事后排查「发了但没辅上」这类问题。
pub fn run_reseed(
    cfg: &Config,
    qb: &QBittorrent,
    req: &ReseedRequest,
    index: &mut ContentIndex,
    log: &mut dyn FnMut(&str),
    throttle: Option<&SiteThrottle>,
) -> Result<ReseedOutcome> {
    run_reseed_impl(cfg, qb, req, index, log, throttle, None)
}

/// 用审阅产物直发（BUG-M7）：跳过源页/上传页/官方预审的二次抓取，
/// 提交字段与门禁判定完全同一份，消除「门禁通过的那份 ≠ 真正上传的那份」。
pub fn run_reseed_reviewed(
    cfg: &Config,
    qb: &QBittorrent,
    req: &ReseedRequest,
    index: &mut ContentIndex,
    log: &mut dyn FnMut(&str),
    throttle: Option<&SiteThrottle>,
    review: ReviewReport,
) -> Result<ReseedOutcome> {
    run_reseed_impl(cfg, qb, req, index, log, throttle, Some(review))
}

/// 把站点上已发布的种子下回来挂进 qB（命令 `bsend pull`）：站上已经发布，qB 里还没有这个站的目标种。
/// 只拉详情页下载站点生成的 `.torrent`，按 pieces 在索引里找本地内容，
/// 再走与发后同一条 `push_target_torrent`。绝不重新上传、不占发布额度。
/// 不接受本地构造的种子：站点审核没通过时，挂它只会得到一条没有 tracker 响应的废种。
/// 返回最终格子状态和详情页 URL。
pub fn pull_published(
    cfg: &Config,
    qb: &QBittorrent,
    site_id: &str,
    detail_url: &str,
    content_key: Option<&str>,
    index: &mut ContentIndex,
    log: &mut dyn FnMut(&str),
) -> Result<(CoverageStatus, String)> {
    let tgt = adapter_for(cfg, site_id)?;
    log(&format!("拉取目标种：{detail_url}"));
    let page = tgt.fetch_torrent_page(detail_url)?;
    let tdata = tgt.download_torrent(&page, detail_url)?;
    pull_with_data(
        cfg,
        qb,
        site_id,
        detail_url,
        content_key,
        &tdata,
        index,
        log,
    )
}

/// 站点种子已经下回来了，按它挂进 qB（`pull_published` 的后半段；站点审核回访也走这里）。
/// 返回最终格子状态和详情页 URL。
#[allow(clippy::too_many_arguments)] // 内部管道：挂回需要的全部共享件
pub(crate) fn pull_with_data(
    cfg: &Config,
    qb: &QBittorrent,
    site_id: &str,
    detail_url: &str,
    content_key: Option<&str>,
    tdata: &[u8],
    index: &mut ContentIndex,
    log: &mut dyn FnMut(&str),
) -> Result<(CoverageStatus, String)> {
    let key = match content_key {
        Some(k) => k.to_string(),
        None => find_content_key_by_pieces(qb, index, tdata)?,
    };
    let (name, source_site, source_url) = {
        let entry = index.contents.get(&key).with_context(|| {
            format!("索引里没有内容 {key}；先用 bsend scan 刷新，或用 --key 指定")
        })?;
        (
            entry.name.clone(),
            entry
                .source
                .as_ref()
                .map(|s| s.site_id.clone())
                .unwrap_or_default(),
            entry
                .source
                .as_ref()
                .map(|s| s.detail_url.clone())
                .unwrap_or_default(),
        )
    };
    let member = matching_local_member(qb, &index.contents[&key], tdata)?
        .with_context(|| format!("目标种分片与本地内容 {key} 不一致，拒绝挂回"))?;
    let req = ReseedRequest {
        content_key: key,
        source_site,
        source_url,
        target_site: site_id.to_string(),
        name: name.clone(),
    };
    let status = push_target_torrent(
        cfg, qb, &req, &name, detail_url, tdata, None, member, index, log,
    )?;
    index.save()?;
    Ok((status, detail_url.to_string()))
}

/// 没有给 `--key` 时，按 pieces 在索引里找这段内容；找不到要求用户指定。
fn find_content_key_by_pieces(
    qb: &QBittorrent,
    index: &ContentIndex,
    tdata: &[u8],
) -> Result<String> {
    for (key, entry) in &index.contents {
        if matching_local_member(qb, entry, tdata)?.is_some() {
            return Ok(key.clone());
        }
    }
    bail!("索引里找不到分片一致的内容；确认 qB 里有本地源种，或用 --key 指定 contentKey")
}

fn run_reseed_impl(
    cfg: &Config,
    qb: &QBittorrent,
    req: &ReseedRequest,
    index: &mut ContentIndex,
    log: &mut dyn FnMut(&str),
    throttle: Option<&SiteThrottle>,
    review: Option<ReviewReport>,
) -> Result<ReseedOutcome> {
    let started = now_unix();
    let result = run_reseed_inner(cfg, qb, req, index, log, throttle, review);
    append_reseed_log(cfg, req, started, &result, index);
    result
}

fn run_reseed_inner(
    cfg: &Config,
    qb: &QBittorrent,
    req: &ReseedRequest,
    index: &mut ContentIndex,
    log: &mut dyn FnMut(&str),
    throttle: Option<&SiteThrottle>,
    review: Option<ReviewReport>,
) -> Result<ReseedOutcome> {
    // 真实发种总闸：缺省关闭，任何调用方（CLI --yes / autopost / GUI）都过这里。
    crate::config::ensure_publish_enabled(cfg)?;
    // 调用方传共享节流（autopost 全局预算）就用它；单发一条时本地建一份
    let owned_throttle;
    let throttle = match throttle {
        Some(t) => t,
        None => {
            owned_throttle = SiteThrottle::from_coverage(&cfg.coverage);
            &owned_throttle
        }
    };
    // 自动发种路径：直接复用审阅产物（同一份字段），不再二次抓源页/上传页。
    // 单发/CLI --yes 没有审阅产物，仍走 prepare_released 的现场构造。
    let (rel, prepared_from_review) = match review {
        Some(rep) => {
            let Some(carry) = rep.carry else {
                bail!("审阅产物缺少发布载荷，不能直发（请重新审阅后再发）");
            };
            if carry.src.site_id != req.source_site || carry.tgt.site_id != req.target_site {
                bail!(
                    "审阅产物与请求站点不一致（审阅 {}→{}，请求 {}→{}），拒绝提交",
                    carry.src.site_id,
                    carry.tgt.site_id,
                    req.source_site,
                    req.target_site
                );
            }
            // 官方预审结论随审阅产物一起复用：没过不再重复请求，直接拦。
            if let Some(a) = &rep.audit {
                if !a.passed {
                    let first = a
                        .details
                        .first()
                        .map(|d| d.message.clone())
                        .unwrap_or_default();
                    bail!(
                        "官方预审未通过（{}，总分 {}/100）：{first}。补齐后重试",
                        a.status,
                        a.total_score
                    );
                }
            }
            if let Some(note) = &rep.audit_note {
                log(&format!(
                    "官方预审接口异常（{note}，审阅时；不重复请求），按本地预检继续"
                ));
            }
            let rel = Released {
                src: carry.src,
                tgt: carry.tgt,
                page: carry.page,
                info: rep.info,
                preflight: rep.preflight,
            };
            (rel, Some(rep.prepared))
        }
        None => (prepare_released(cfg, req, log)?, None),
    };
    if !rel.preflight.ok() {
        for e in &rel.preflight.errors {
            log(&format!("预检·阻断：{e}"));
        }
        bail!("预检未通过（{}）：补齐后重试", rel.preflight.summary());
    }
    // ---- 2.5 目标站判重：站点按 IMDb/片名判重，命中可能是「同片不同版本」。
    // qB 没有该站 tracker 并不一定是辅种漏了：文件清单不同（1080p vs 2160p）时
    // 辅种臂会正确拒绝跨种。这里不上传，落 ReviewPending 交人工确认。
    let imdb_for_search = rel.info.imdb.as_deref();
    let dup = match target_has_imdb(&rel.tgt, imdb_for_search, log) {
        Ok(Some(hit)) => Some(hit),
        Ok(None) => match rel.tgt.search_sized(
            &rel.info.name,
            &rel.info.subtitle,
            false,
            imdb_for_search.unwrap_or(""),
            rel.page.size_bytes,
            &mut || throttle.acquire(&rel.tgt.site_id, BudgetKind::Search),
        ) {
            Ok(crate::sites::nexusphp::SearchOutcome::Hit(hit)) => {
                Some((hit.detail_url, hit.title))
            }
            Ok(crate::sites::nexusphp::SearchOutcome::Miss) => None,
            Ok(crate::sites::nexusphp::SearchOutcome::BudgetExhausted) => {
                bail!("目标站判重预算耗尽，拒绝在未确认重复情况的上传");
            }
            Err(e) => {
                bail!("目标站判重检索失败（{e:#}），拒绝在未确认重复情况的上传");
            }
        },
        Err(e) => {
            bail!("目标站 IMDb 判重失败（{e:#}），拒绝在未确认重复情况的上传");
        }
    };
    if let Some((detail_url, title)) = dup {
        // 判重命中不直接假定“其他版本”：下载目标种比较 pieces。
        // 三态：同分片 = 站点已有同一内容（Assisted）；不同分片 = 其他版本（OtherVersion）；
        // 复核失败（网络/下载/索引） = Unknown——不能谎称「pieces 不一致」，
        // 落 Failed 带真实原因，TTL 后重查。
        // 复核要下载目标种，也是目标站请求：按检索预算领号，耗尽就 fail-closed
        if !throttle.acquire(&rel.tgt.site_id, BudgetKind::Search) {
            bail!("目标站判重复核预算用完，拒绝在未确认重复情况的上传");
        }
        let verdict: Result<DupCheck> = (|| {
            let page = rel.tgt.fetch_torrent_page(&detail_url)?;
            let data = rel.tgt.download_torrent(&page, &detail_url)?;
            let entry = index
                .contents
                .get(&req.content_key)
                .context("判重复核：索引缺内容条目")?;
            Ok(matching_local_member(qb, entry, &data)?.map(|m| (data, m)))
        })();
        match verdict {
            // 同分片：目标站已有同一内容，仍要下载目标种挂到本地数据上（跨站辅种）。
            // 只写 Assisted 不推下载器，站点上就只有发布记录、qB 里没有目标站 tracker。
            Ok(Some((data, member))) => {
                match push_target_torrent(
                    cfg,
                    qb,
                    req,
                    &rel.info.name,
                    &detail_url,
                    &data,
                    None,
                    member,
                    index,
                    log,
                ) {
                    Ok(state) => {
                        let (status, note) = match state {
                            CoverageStatus::Reseeded => (
                                CoverageStatus::Assisted,
                                format!(
                                    "目标站已有同分片「{title}」，已下载目标种并加入下载器做种"
                                ),
                            ),
                            CoverageStatus::Assisted => (
                                CoverageStatus::Assisted,
                                format!(
                                    "目标站已有同分片「{title}」，下载器里已存在（已补挂 tracker）"
                                ),
                            ),
                            CoverageStatus::SameHashSkip => (
                                CoverageStatus::SameHashSkip,
                                format!(
                                    "目标站已有同分片「{title}」，但目标种与已有种子同 hash；按策略不改动原种子，跳过"
                                ),
                            ),
                            _ => (
                                CoverageStatus::Assisted,
                                format!(
                                    "目标站已有同分片「{title}」，已加入下载器，校验中（下一轮收尾）"
                                ),
                            ),
                        };
                        log(&format!("目标站判重命中：{title} → {detail_url}；{note}"));
                        return Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
                            status,
                            url: Some(detail_url),
                            note,
                        }));
                    }
                    Err(e) => {
                        let why = format!("站上已有同分片「{title}」，但挂回 qB 失败（{e:#}）");
                        log(&why);
                        return unresolved_after_site(
                            index,
                            &req.content_key,
                            &req.target_site,
                            "existed_unresolved",
                            Some(detail_url),
                            &why,
                        );
                    }
                }
            }
            Ok(None) => {
                // 站点有同名「其他版本」：照样发（分集/全集、不同压制组都算）。
                // 画质高低不由工具裁决——站点自己的门禁/审核会拦，工具多做一层判断
                // 只会把能发的种挡在门外（实测一次 1080p/2160p 误判就废掉一条）。
                if cfg.coverage.allow_other_version {
                    log(&format!(
                        "目标站已有其他版本「{title}」→ {detail_url}；按策略继续发布，画质交由站点门禁判断"
                    ));
                } else {
                    let why = format!("目标站已有其他版本「{title}」；pieces 不一致，按配置不发");
                    log(&format!("目标站判重命中：{title} → {detail_url}；{why}"));
                    write_cell(
                        index,
                        &req.content_key,
                        &req.target_site,
                        CoverageStatus::OtherVersion,
                        Some(detail_url.clone()),
                        &why,
                    );
                    index.save()?;
                    return Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
                        status: CoverageStatus::OtherVersion,
                        url: Some(detail_url),
                        note: why,
                    }));
                }
            }
            Err(e) => {
                let why = format!("目标站已有条目「{title}」，但复核失败（{e:#}），不判其他版本");
                log(&format!(
                    "目标站判重命中：{title} → {detail_url}；复核失败，待重查"
                ));
                write_cell(
                    index,
                    &req.content_key,
                    &req.target_site,
                    CoverageStatus::Failed,
                    Some(detail_url.clone()),
                    &why,
                );
                index.save()?;
                return Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
                    status: CoverageStatus::Failed,
                    url: Some(detail_url),
                    note: why,
                }));
            }
        }
    }
    log("目标站未找到同条目，继续上传");

    let Released { tgt, info, .. } = rel;

    // ---- 3 上传字段：审阅产物直传；否则现场构造 + 官方预审 ----
    let prep = match prepared_from_review {
        Some(prep) => {
            log("复用审阅产物：提交字段与门禁判定为同一份（跳过上传页/官方预审二次抓取）");
            prep
        }
        None => {
            let mut prep = prepare_upload(&tgt, &info)?;
            let rules = cfg.coverage.rules_for(&req.target_site);
            apply_site_rules(&mut prep, &rules);
            if pre_audit_supported(&prep.form_html) {
                log("调用站点官方预审（LuckAudit）…");
                match run_pre_audit(&tgt, &info, &prep) {
                    Ok(v) => {
                        log(&format!("官方预审：{}", v.summary()));
                        for d in &v.details {
                            log(&format!("官方预审·{}", d.line()));
                        }
                        if !v.passed {
                            let first = v
                                .details
                                .first()
                                .map(|d| d.message.clone())
                                .unwrap_or_default();
                            bail!(
                                "官方预审未通过（{}，总分 {}/100）：{first}。补齐后重试",
                                v.status,
                                v.total_score
                            );
                        }
                    }
                    Err(e) => log(&format!("官方预审接口异常（{e:#}），按本地预检继续")),
                }
            }
            prep
        }
    };

    // ---- 4 从 qB 导出源种 → 单 tracker 改写 → 上传目标站 ----
    let entry = index
        .contents
        .get(&req.content_key)
        .ok_or_else(|| anyhow!("索引里找不到内容 {}", req.content_key))?;

    // 4.1 选一个已完成本地成员，导出原始 .torrent（源种不回源站搜索/下载）
    let (matched_member, src_data) = pick_exported_member(qb, entry)?;
    log(&format!(
        "源种取自 qB 导出：本地成员 {}（savePath {}）",
        &matched_member.0[..12.min(matched_member.0.len())],
        matched_member.1
    ));

    // 4.2 目标站 announce（带 passkey）：优先用站点配置（第一次在该站发种时必须配）；
    // 没配则从下载器现有该站 tracker 里取一条。两者都没有就 fail-closed，
    // 不构造无 passkey 的 announce。
    let target_announce = match tgt.announce_url.clone() {
        Some(u) => {
            let host = Url::parse(&u)
                .ok()
                .and_then(|p| p.host_str().map(|h| h.to_lowercase()))
                .unwrap_or_default();
            let host_ok = crate::qbittorrent::announce_host_matches(&host, &tgt.announce_hosts);
            if !host_ok {
                bail!(
                    "站点 {} 配置的 announceURL host（{host}）不在 announceHosts 里，拒绝上传到错误 tracker",
                    tgt.site_id
                );
            }
            u
        }
        None => qb.announce_for_hosts(&tgt.announce_hosts).ok_or_else(|| {
            anyhow!(
                "单 tracker 策略：本地找不到目标站 {} 的 announce（带 passkey）。第一次在该站发种时，请在 boxsend.json 的该站 overrides.announceURL 填入完整 tracker 地址",
                tgt.site_id
            )
        })?,
    };

    // 4.3 字节级改写：换 announce、删 announce-list、注入 info.source（素材字节不动）
    let rewritten = torrent_rewrite::rewrite_for_target(&src_data, &target_announce, &tgt.site_id)?;

    // 4.4 上传前断言：新 hash 不在下载器；新旧 hash 不同；单 tracker
    let existing = qb.existing_hashes()?;
    if existing.contains(&rewritten.new_hash) {
        bail!(
            "单 tracker 策略：新种 info hash {} 已存在于 qB（{}），中止；不做 hash 冲突兜底、不改 piece length",
            rewritten.new_hash,
            req.target_site
        );
    }
    if rewritten.new_hash == rewritten.old_hash {
        bail!("实现错误：改写后 info hash 与源种相同，info.source 未生效");
    }
    torrent_rewrite::assert_single_tracker(&rewritten.data)?;

    log(&format!(
        "源种已改写（{} bytes，新 hash {}），提交到 {} 上传页…",
        rewritten.data.len(),
        &rewritten.new_hash[..12.min(rewritten.new_hash.len())],
        req.target_site
    ));
    let filename = format!("{}.torrent", info.name);
    // 上传是唯一有站点副作用的动作：全局上传预算耗尽必须 fail-closed，
    // 否则多站并发/循环异常时没有任何总量闸（日限额只管单站）
    if !throttle.acquire(&tgt.site_id, BudgetKind::Upload) {
        bail!("上传预算用完（assist.roundBudget / uploadBudget），拒绝继续上传");
    }
    let up = match submit_upload(
        &tgt,
        &prep,
        &rewritten.data,
        &filename,
        cfg.data_dir.as_deref(),
    ) {
        Ok(u) => u,
        // 站点嫌种子太大时不会报错，而是回一个自动跳 offers.php 的表单
        // （`id='torrent_too_large'`、`add_offer=1`）：这不是可重试的失败，
        // 转候选要人工走投票流程。落 PendingOffers（不入队重试，但在待跟进里留痕）。
        Err(e) if site_rejects_size(&format!("{e:#}")) => {
            let note = format!(
                "站点判定种子过大，要求改走候选入口（offers.php）：需人工在候选页提交，程序不自动重试（原始回包：{e:#}）"
            );
            write_cell(
                index,
                &req.content_key,
                &req.target_site,
                CoverageStatus::PendingOffers,
                None,
                &note,
            );
            index.save()?;
            log(&note);
            return Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
                status: CoverageStatus::PendingOffers,
                url: None,
                note,
            }));
        }
        Err(e) => return Err(e),
    };
    log(&format!("上传结果：{}", up.message));

    // 站点回包带 duplicate 标志时，回包链接可能只是 existed 跳转（不可信），
    // 但不能只落 ReviewPending 就完事：目标站已有条目同样要下载目标种挂到本地数据上，
    // 否则站点上有发布记录、qB 里却没有目标站 tracker（跨站辅种缺这一步）。
    if up.duplicate {
        let hint = up.detail_url.unwrap_or_default();
        let Some(detail_url) = locate_existing_target(&tgt, &info, &hint, log) else {
            // 回包 id 只当线索，不当结论：实测 6/6 新种的 details id 都是错的，
            // 这里又定位不到可核对条目，就不能把 hint 写成「站点详情页」，
            // 否则覆盖表会凭空多出一条「站点已有」的假记录。
            let clue = if hint.is_empty() {
                String::new()
            } else {
                format!("（回包给的线索 {hint}，未验证）")
            };
            let note = format!(
                "上传回包标记 existed（{}），但站内定位不到可核对的条目{clue}；未加入下载器，待人工确认",
                up.message
            );
            write_cell(
                index,
                &req.content_key,
                &req.target_site,
                CoverageStatus::ReviewPending,
                None,
                &note,
            );
            index.save()?;
            return Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
                status: CoverageStatus::ReviewPending,
                url: None,
                note,
            }));
        };
        log(&format!(
            "existed 条目已定位：{detail_url}，下载目标种判重…"
        ));
        if !throttle.acquire(&tgt.site_id, BudgetKind::Search) {
            bail!("上传回包 existed，但判重复核预算用完，拒绝继续");
        }
        let verdict: Result<DupCheck> = (|| {
            let page = tgt.fetch_torrent_page(&detail_url)?;
            let data = tgt.download_torrent(&page, &detail_url)?;
            let entry = index
                .contents
                .get(&req.content_key)
                .context("existed 判重复核：索引缺内容条目")?;
            Ok(matching_local_member(qb, entry, &data)?.map(|m| (data, m)))
        })();
        return match verdict {
            Ok(Some((data, member))) => {
                match push_target_torrent(
                    cfg,
                    qb,
                    req,
                    &info.name,
                    &detail_url,
                    &data,
                    None,
                    member,
                    index,
                    log,
                ) {
                    Ok(state) => {
                        let (status, note) = match state {
                            CoverageStatus::Reseeded => (
                                CoverageStatus::Assisted,
                                "上传回包 existed：已下载目标种并加入下载器做种".to_string(),
                            ),
                            CoverageStatus::Assisted => (
                                CoverageStatus::Assisted,
                                "上传回包 existed：下载器里已存在（同 hash，按策略不改动原种子）".to_string(),
                            ),
                            CoverageStatus::SameHashSkip => (
                                CoverageStatus::SameHashSkip,
                                "上传回包 existed：目标种与已有种子同 hash；按策略不改动原种子，跳过"
                                    .to_string(),
                            ),
                            _ => (
                                CoverageStatus::Assisted,
                                "上传回包 existed：已加入下载器，校验中（下一轮收尾）".to_string(),
                            ),
                        };
                        log(&note);
                        Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
                            status,
                            url: Some(detail_url),
                            note,
                        }))
                    }
                    Err(e) => {
                        let why = format!("上传回包 existed，但挂回 qB 失败（{e:#}）");
                        log(&why);
                        unresolved_after_site(
                            index,
                            &req.content_key,
                            &req.target_site,
                            "existed_unresolved",
                            Some(detail_url),
                            &why,
                        )
                    }
                }
            }
            Ok(None) => {
                let why =
                    "上传回包 existed：目标站条目 pieces 与本地不一致（其他版本），未加入下载器";
                log(why);
                write_cell(
                    index,
                    &req.content_key,
                    &req.target_site,
                    CoverageStatus::OtherVersion,
                    Some(detail_url.clone()),
                    why,
                );
                index.save()?;
                Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
                    status: CoverageStatus::OtherVersion,
                    url: Some(detail_url),
                    note: why.to_string(),
                }))
            }
            Err(e) => {
                let why = format!("上传回包 existed，但判重复核失败（{e:#}），未加入下载器");
                log(&why);
                write_cell(
                    index,
                    &req.content_key,
                    &req.target_site,
                    CoverageStatus::Failed,
                    Some(detail_url.clone()),
                    &why,
                );
                index.save()?;
                Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
                    status: CoverageStatus::Failed,
                    url: Some(detail_url),
                    note: why,
                }))
            }
        };
    }

    // ---- 5 发后定位：优先用回包/跳转里的详情 id；拿不到才按名称+大小站内检索 ----
    let links = extract_posted_links(&up.body, &up.final_url, &tgt.base);
    // takeupload.php / 跳转里的详情链接优先：submit_upload 已经单独解析过，
    // 这里不能只信 extract_posted_links 的正文扫描，否则会把已有的 detail_url 丢掉。
    let mut detail_url = up
        .detail_url
        .clone()
        .filter(|u| !u.is_empty())
        .or_else(|| links.detail.clone());
    let mut download_url = links.download.clone();
    let mut tdata: Option<Vec<u8>> = None;
    // 站点页面上明写的审核结论（未审 / 审核不通过）。回包本身就是新种详情页时先看回包；
    // 之后真抓到的新种页（取下载链接、发后自检）以它为准。
    // `page_seen`：这条新种的详情页这一轮是不是已经看过了（看过就不用再抓）。
    let body_is_detail = is_detail_href(&up.final_url);
    let mut page_seen = body_is_detail;
    let mut page_mark = if body_is_detail {
        review_mark(&up.body, &info.name)
    } else {
        None
    };

    // 回包只给详情页时，从详情页拿站点的 download.php；不要自己拼目标站种子。
    if download_url.is_none() {
        if let Some(detail) = detail_url.as_deref() {
            match tgt.fetch_torrent_page_marked(detail) {
                Ok((p, mark)) => {
                    page_seen = true;
                    page_mark = mark.or(page_mark);
                    if !p.download_url.is_empty() {
                        download_url = Some(p.download_url);
                    } else {
                        log(&format!("回包详情页没有下载链接：{detail}"));
                    }
                }
                Err(e) => log(&format!("回包详情页打不开（{e:#}）：{detail}")),
            }
        }
    }

    // 只下载站点生成的 .torrent。上传成功后这一步是**硬步骤**，不给配置开关：
    // 站点刚建种时 download.php 常先回 `torrent not registered`，隔几十秒才好；
    // 一次失败就收工会留下“站上发布了、qB 里没做种”的种子（熊猫候选就这么被删过）。
    // 因此固定重试几轮，拿不到才落待人工，回访时 `bsend pull` 还能补。
    let mut pull_round = 0usize;
    loop {
        pull_round += 1;
        if let Some(url) = download_url.as_deref() {
            let referer = detail_url.as_deref().unwrap_or(url);
            let page = TorrentPage {
                name: String::new(),
                download_url: url.to_string(),
                size_bytes: None,
                size_slack_bytes: None,
                uploader_uid: None,
                uploader_name: None,
            };
            match tgt.download_torrent(&page, referer) {
                Ok(d) => tdata = Some(d),
                Err(e) => match site_review_hold(&e) {
                    // 站点收下了种子，但要先审才给下载：种子就在站上，不用再去站内检索
                    // （检索只会命中同名旧种），直接落“等站点审核 / 审核没过”。
                    Some((mark, why)) => {
                        return review_hold_outcome(
                            index,
                            &req.content_key,
                            &req.target_site,
                            detail_url.clone().or_else(|| up.detail_url.clone()),
                            mark,
                            &why,
                            log,
                        );
                    }
                    None => log(&format!("站点种子下载失败（{e:#}）：{url}")),
                },
            }
        }

        // 回包没给链接、或链接下载失败：用发布名 + 本地总大小检索新种。
        // 只接受站点详情页标题和大小都对上的候选，再下载站点种；绝不拿本地构造种顶替。
        if tdata.is_none() {
            let mut held: Option<(String, ReviewMark, String)> = None;
            if let Some((found_detail, found_data)) = locate_posted_by_search(
                &tgt,
                &info,
                Some(entry.total_size),
                cfg.coverage.relaxed_retry_search,
                throttle,
                log,
                &mut held,
            ) {
                detail_url = Some(found_detail);
                tdata = Some(found_data);
            } else if let Some((found_detail, mark, why)) = held {
                return review_hold_outcome(
                    index,
                    &req.content_key,
                    &req.target_site,
                    Some(found_detail),
                    mark,
                    &why,
                    log,
                );
            }
        }

        if tdata.is_some() || pull_round >= POST_UPLOAD_PULL_ATTEMPTS {
            break;
        }
        log(&format!(
            "上传已成功，站点种子还拿不到；{} 秒后再试（第 {pull_round}/{POST_UPLOAD_PULL_ATTEMPTS} 轮）",
            POST_UPLOAD_PULL_INTERVAL_SEC
        ));
        thread::sleep(Duration::from_secs(POST_UPLOAD_PULL_INTERVAL_SEC));
        // 重抓详情页：刷新下载链接，也顺带看审核标记是否更新
        if let Some(detail) = detail_url.as_deref() {
            match tgt.fetch_torrent_page_marked(detail) {
                Ok((p, mark)) => {
                    page_seen = true;
                    page_mark = mark.or(page_mark);
                    if !p.download_url.is_empty() {
                        download_url = Some(p.download_url);
                    }
                }
                Err(e) => log(&format!("重试·详情页打不开（{e:#}）：{detail}")),
            }
        }
    }

    let Some(tdata) = tdata else {
        // 上传可能已成功：定位不到新种 ≠ 发种失败；落 ReviewPending，不进失败冷却、不计 posted。
        let reason = format!(
            "posted_unresolved：回包无详情/下载链接，站内按名称+总大小也没定位到新种（{}），待人工确认；勿重发",
            up.message
        );
        log(&format!("发后定位·未解决：{reason}"));
        let unresolved_url = detail_url.clone().or_else(|| up.detail_url.clone());
        write_cell(
            index,
            &req.content_key,
            &req.target_site,
            CoverageStatus::ReviewPending,
            unresolved_url.clone(),
            &reason,
        );
        index.save()?;
        return Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
            status: CoverageStatus::ReviewPending,
            url: unresolved_url,
            note: reason,
        }));
    };

    // 详情页链接（自检/汇报用）：以实际下载的 download.php 反推优先，避免回包里
    // 或判重页上的旧种线索（例如「目标站已有其他版本」的链接）盖过真正的新种。
    let detail_url = download_url
        .as_deref()
        .and_then(|u| detail_from_download(u, &tgt.base))
        .or(detail_url);

    // ---- 发后自检：回读新种页核对标题/MediaInfo/链接（只警告不推翻）----
    // 同一次请求顺带看页面上明写的审核结论（未审 / 审核不通过）。
    // 跳过自检时，只有这一轮还没看过新种详情页才为看审核标记单独抓一次。
    if let Some(d) = &detail_url {
        if !cfg.coverage.skip_post_check || (!page_seen && page_mark.is_none()) {
            let (warns, mark) = verify_posted(&tgt, d, &info);
            if !cfg.coverage.skip_post_check {
                for w in warns {
                    log(&format!("发后自检·{w}"));
                }
            }
            page_mark = mark.or(page_mark);
        }
    }
    // 站点已经标“通过”的，和没有审核标记一样：照常算发布完成
    if matches!(page_mark, Some((ReviewMark::Approved, _))) {
        log("站点审核：新种页面上标着“通过”");
    }
    let page_mark = page_mark.filter(|(m, _)| *m != ReviewMark::Approved);

    // ---- 6 已拿到站点 .torrent（下载或检索补定位），作为独立种子挂回 qB（绝不追加 tracker）----
    let reported_url = detail_url
        .clone()
        .or_else(|| download_url.clone())
        .unwrap_or_else(|| up.final_url.clone());
    match push_target_torrent(
        cfg,
        qb,
        req,
        &info.name,
        &reported_url,
        &tdata,
        Some(&rewritten.new_hash),
        matched_member,
        index,
        log,
    ) {
        Ok(CoverageStatus::Reseeded) => match page_mark {
            // 站点让下载、也挂回 qB 在做种了，但页面上还标着审核结论：做种不受影响，
            // 可这条种子留不留得下来要看审核，格子按审核结论落，等回访收尾。
            Some((mark, why)) => review_seeding_outcome(
                index,
                &req.content_key,
                &req.target_site,
                &reported_url,
                mark,
                &why,
                log,
            ),
            None => Ok(ReseedOutcome::Posted(reported_url)),
        },
        Ok(other) => {
            if let Some((mark, why)) = &page_mark {
                log(&format!(
                    "站点页面标着审核结论（{}：{why}）；本轮格子是 {}，等回访时再记",
                    mark.label(),
                    other.label()
                ));
            }
            Ok(ReseedOutcome::Posted(reported_url))
        }
        Err(e) => unresolved_after_site(
            index,
            &req.content_key,
            &req.target_site,
            "posted_unresolved",
            Some(reported_url),
            &format!("站上已发布，但挂回 qB 失败（{e:#}）"),
        ),
    }
}

/// 转种动作日志（JSONL，fail-open）：GUI/CLI/autopost 每次转种都写一行。
/// 与 `auto-audit.jsonl` 分开：后者管日限额/失败冷却，这里只留可追溯的动作现场。
fn append_reseed_log(
    cfg: &Config,
    req: &ReseedRequest,
    started: i64,
    result: &Result<ReseedOutcome>,
    index: &ContentIndex,
) {
    use std::io::Write as _;
    let path = cfg
        .coverage_state_path()
        .parent()
        .map(|p| p.join("reseed-log.jsonl"))
        .unwrap_or_else(|| std::path::PathBuf::from("reseed-log.jsonl"));
    let (word, url, note): (&str, Option<&str>, Option<String>) = match result {
        Ok(ReseedOutcome::Posted(u)) => ("posted", Some(u.as_str()), None),
        Ok(ReseedOutcome::Duplicate(d)) => (
            ReseedOutcome::Duplicate(d.clone()).audit_result(),
            d.url.as_deref(),
            Some(d.note.clone()),
        ),
        Err(e) => ("failed", None, Some(format!("{e:#}"))),
    };
    // 发种后的下载器确认：只有格子落到 Reseeded/Assisted/Seeding 才算「已辅到 qB」
    let cell = index
        .contents
        .get(&req.content_key)
        .and_then(|e| e.coverage.get(&req.target_site));
    let cell_status = cell
        .and_then(|c| serde_json::to_value(c.status).ok())
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let seeded = matches!(
        cell.map(|c| c.status),
        Some(CoverageStatus::Reseeded | CoverageStatus::Assisted | CoverageStatus::Seeding)
    );
    let line = serde_json::json!({
        "at": now_unix(),
        "started_at": started,
        "key": req.content_key,
        "name": req.name,
        "source_site": req.source_site,
        "source_url": req.source_url,
        "target_site": req.target_site,
        "result": word,
        "url": url,
        "note": note,
        "cell_status": cell_status,
        "seeded": seeded,
    });
    // 日志失败不影响发种结论（但 stderr 留一句，便于发现磁盘问题）
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{line}") {
                eprintln!("写 reseed-log.jsonl 失败：{e}");
            }
        }
        Err(e) => eprintln!("打开 reseed-log.jsonl 失败（{}）：{e}", path.display()),
    }
}

pub(crate) fn write_cell(
    index: &mut ContentIndex,
    key: &str,
    site: &str,
    status: CoverageStatus,
    url: Option<String>,
    note: &str,
) {
    if let Some(e) = index.contents.get_mut(key) {
        e.coverage.insert(
            site.to_string(),
            SiteCoverage {
                status,
                at: now_unix(),
                url,
                note: Some(note.to_string()),
                verify_hash: None,
            },
        );
    }
}

/// 站点侧已经有结果（新发成功 / existed），但下载或推 qB 失败：
/// 这不是发种失败，不能记 failed、不能进失败冷却、不能重发；落 ReviewPending，
/// note 前缀供审计区分（`posted_unresolved` / `existed_unresolved`）。
fn unresolved_after_site(
    index: &mut ContentIndex,
    key: &str,
    site: &str,
    prefix: &str,
    url: Option<String>,
    why: &str,
) -> Result<ReseedOutcome> {
    let note = format!("{prefix}：{why}；勿重发，按详情页补挂");
    write_cell(
        index,
        key,
        site,
        CoverageStatus::ReviewPending,
        url.clone(),
        &note,
    );
    index.save()?;
    Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
        status: CoverageStatus::ReviewPending,
        url,
        note,
    }))
}

/// 挂回后的 tracker 必须全部属于目标站。站点自己的种子可能带两条同站 announce
/// （如 hdvideo 的 `tracker.hdvideo.top` 和 `hdvideo.top`），这不算多站合并；
/// 只要出现别站 host 或一条都没有，就拒绝。
fn ensure_trackers_on_site(site: &SiteConf, urls: &[String]) -> Result<()> {
    if urls.is_empty() {
        bail!("新种挂回下载器后没有可用 tracker；中止");
    }
    let foreign: Vec<&String> = urls
        .iter()
        .filter(|u| {
            Url::parse(u)
                .ok()
                .and_then(|p| p.host_str().map(|h| h.to_lowercase()))
                .is_none_or(|h| !site.matches_tracker(&h))
        })
        .collect();
    if !foreign.is_empty() {
        bail!(
            "单站点策略：新种挂回下载器后包含非目标站 tracker {:?}；中止（不追加 announce、不补挂 tracker）",
            foreign
        );
    }
    Ok(())
}

/// 把目标站 `.torrent` 推进下载器并校验：发布成功、或目标站已有同分片条目
/// （existed / 站内判重命中）时共用——「新站已有条目」也要下载目标种挂到本地数据上，
/// 否则站点上永远只有发布记录、下载器里没有这个 tracker。
/// 返回最终格子状态：`Reseeded`（校验 100% 在做种）/ `Assisted`（下载器里早已有，
/// 已补挂 tracker）/ `Verifying`（本轮没验完，下一轮收尾）；校验失败返回 Err（格子已写 Failed）。
#[allow(clippy::too_many_arguments)]
fn push_target_torrent(
    cfg: &Config,
    qb: &QBittorrent,
    req: &ReseedRequest,
    name: &str,
    detail_url: &str,
    tdata: &[u8],
    expected_hash: Option<&str>,
    matched_member: MatchedMember,
    index: &mut ContentIndex,
    log: &mut dyn FnMut(&str),
) -> Result<CoverageStatus> {
    let (rep_hash, meta_save_path, category) = matched_member;
    // 挂回前先核对：下载回来的必须就是刚上传的新种（默认 hash 对得上），
    // 分片结构也必须和本地源种一致；否则宁可落人工，不往 qB 里塞错种。
    // 站点上传阶段常改写种子元信息（info.source 等），hash 必然不同；这类改写不改
    // 分片和 info.name。分片一致 + 标题一致就说明是同一份内容，放行挂回；仍拒绝
    // 分片不一致的种。站点白名单只用来在日志里标注“已实测改写”，不再是放行条件。
    let target_hash = bencode::info_hash_v1(tdata).context("目标种缺少 v1 info hash")?;
    let target_pieces = bencode::pieces_hash(tdata).context("目标种缺少 pieces，无法确认分片")?;
    let local = qb
        .export_torrent(&rep_hash)
        .with_context(|| format!("导出本地源种 {rep_hash} 失败，无法核对分片"))?;
    let local_pieces = bencode::pieces_hash(&local).context("本地源种缺少 pieces，无法核对分片")?;
    if target_pieces != local_pieces {
        bail!("目标种分片与本地源种不一致，无法跨种做种，拒绝挂回 qB");
    }
    // 站点常按自己的标题规范改 info.name（根目录名随之变），文件清单和分片不变。
    // 只比「去掉首段根名后的相对路径 + 文件长度」；布局不同就说明不是同一套文件，拒绝。
    if !bencode::same_file_layout_ignoring_root(tdata, &local) {
        bail!("目标种文件清单与本地源种不一致，无法跨种做种，拒绝挂回 qB");
    }
    if let Some(expected) = expected_hash {
        if target_hash != expected {
            let site_rewrites = crate::sites::allows_target_hash_rewrite(&req.target_site);
            let target_name = bencode::info_name(tdata).unwrap_or_default();
            let same_name = target_name == name;
            log(&format!(
                "站点改写了种子元信息（如 info.source/标题）：目标种 hash {target_hash} 与上传构造 {expected} 不同，标题「{target_name}」/构造「{name}」{}；分片和文件清单一致，放行挂回（站点白名单={site_rewrites}）",
                if same_name { "一致" } else { "不同" }
            ));
        }
    }
    // 同 hash 绝不碰原种子：qB 不允许同 hash 两条，提前判掉，连 add 请求都不发
    if matches!(qb.torrent_progress(&target_hash), Ok(Some(_))) {
        let note = "目标种与下载器已有种子 info hash 相同；按策略不改动原种子，跳过该站";
        write_cell(
            index,
            &req.content_key,
            &req.target_site,
            CoverageStatus::SameHashSkip,
            Some(detail_url.to_string()),
            note,
        );
        index.save()?;
        log(note);
        return Ok(CoverageStatus::SameHashSkip);
    }
    let limit = cfg
        .downloader
        .site_up_limits
        .get(&req.target_site)
        .copied()
        .unwrap_or(cfg.downloader.default_up_limit);
    // 落盘布局：按成员种子实时 savepath；根名不同的同内容新种用 NoSubfolder 复用既有目录
    let layout = crate::layout::resolve_for_torrent(qb, &rep_hash, tdata, name, &meta_save_path);
    if let crate::layout::AddLayout::Mismatch(why) = &layout {
        bail!("新种布局对不上，不推下载器：{why}");
    }
    let save_path = layout.save_path().unwrap_or(&meta_save_path).to_string();
    let fname = format!("{name}.torrent");
    let add = qb.add_torrent_new_only(
        tdata,
        &fname,
        &save_path,
        category.as_deref(),
        cfg.coverage.skip_hash_check,
        limit,
        // 跳过校验时必须直接开始，paused=true 再恢复会让 qB 重新进入校验
        !cfg.coverage.skip_hash_check,
        layout.content_layout(),
    )?;
    if !add.note.is_empty() {
        log(&format!("下载器回执：{}", add.note));
    }
    if add.already {
        // 同 hash：qB 不允许同 hash 两条；按策略绝不改动原种子（不补挂 tracker、
        // 不打标签），只落终态备注，交人工决定是否放弃目标站做种。
        let note = "目标种与下载器已有种子 info hash 相同；按策略不改动原种子，跳过该站";
        write_cell(
            index,
            &req.content_key,
            &req.target_site,
            CoverageStatus::SameHashSkip,
            Some(detail_url.to_string()),
            note,
        );
        index.save()?;
        log(note);
        return Ok(CoverageStatus::SameHashSkip);
    }
    let site_hash = if add.id.is_empty() {
        bencode::info_hash_v1(tdata).unwrap_or_default()
    } else {
        add.id
    };
    let _ = qb.set_paused(&[&site_hash], false);
    // 单站点铁律（ARCH §3.3）：挂回后的 tracker 必须全部属于目标站。
    // 站点自己的种子偶尔带两条同站 announce（例如 hdvideo 的 tracker.hdvideo.top
    // 和 hdvideo.top），这不是多站合并，保留原样；只要出现别的站就中止。
    // 读不到列表（网络/404 形态）重试后只记日志放行；空列表仍视为异常并中止。绝不追加 announce。
    // 刚 add 的种子，qB 偶尔还没建好 tracker 列表，接口会返回 404 或空数组；
    // 短重试三次再判断，避免误跳过单站点核对（railgun 2026-11-23 实发时出现过一次）。
    let mut tracker_check: Option<Vec<String>> = None;
    let mut tracker_error: Option<anyhow::Error> = None;
    let mut tracker_empty = false;
    for attempt in 0..3 {
        match qb.trackers(&site_hash) {
            Ok(urls) if !urls.is_empty() => {
                tracker_check = Some(urls);
                break;
            }
            Ok(_) => {
                tracker_empty = true;
                tracker_error = Some(anyhow!("qB 返回空 tracker 列表"));
            }
            Err(e) => {
                tracker_empty = false;
                tracker_error = Some(e);
            }
        }
        if attempt < 2 {
            thread::sleep(Duration::from_secs(2));
        }
    }
    match tracker_check {
        Some(urls) => {
            let Some(site) = cfg.sites.iter().find(|s| s.id == req.target_site) else {
                bail!("目标站 {} 不在配置里，无法核对 tracker", req.target_site);
            };
            ensure_trackers_on_site(site, &urls)?;
            if urls.len() > 1 {
                log(&format!(
                    "目标种自带 {} 条同站 announce，保留站点原始 tracker 列表",
                    urls.len()
                ));
            }
        }
        None => {
            if let Some(e) = tracker_error {
                log(&format!("tracker 列表核对跳过：{e:#}"));
            }
            if tracker_empty {
                bail!("新种挂回下载器后没有可用 tracker；中止");
            }
        }
    }
    log("已加入下载器并启动校验…");
    let poll = cfg.coverage.verify_poll_sec.max(1);
    let mut waited: u64 = 0;
    while waited < cfg.coverage.verify_wait_sec {
        if let Ok(Some((progress, state))) = qb.torrent_progress(&site_hash) {
            if progress >= 0.999 {
                let _ = qb.set_download_limit(&site_hash, 0);
                log("校验到 100%，已在做种");
                write_cell(
                    index,
                    &req.content_key,
                    &req.target_site,
                    CoverageStatus::Reseeded,
                    Some(detail_url.to_string()),
                    "转种成功：校验到 100%，已在做种",
                );
                index.save()?;
                return Ok(CoverageStatus::Reseeded);
            }
            if waited > 0 && !state.contains("check") && !state.contains("meta") {
                let _ = qb.set_paused(&[&site_hash], true);
                if cfg.coverage.remove_on_verify_fail {
                    let _ = qb.remove_torrent(&site_hash, true);
                }
                write_cell(
                    index,
                    &req.content_key,
                    &req.target_site,
                    CoverageStatus::Failed,
                    Some(detail_url.to_string()),
                    &format!(
                        "校验后只到 {:.0}%：分片结构与本地不同，无法转种",
                        progress * 100.0
                    ),
                );
                index.save()?;
                bail!(
                    "校验后只到 {:.0}%，分片结构与本地不同，无法转种",
                    progress * 100.0
                );
            }
        }
        thread::sleep(Duration::from_secs(poll));
        waited += poll;
    }
    // 超时：落 Verifying，下一轮收尾
    write_cell(
        index,
        &req.content_key,
        &req.target_site,
        CoverageStatus::Verifying,
        Some(detail_url.to_string()),
        &format!(
            "校验还没跑完（等了 {} 秒），下一轮接着收尾",
            cfg.coverage.verify_wait_sec
        ),
    );
    // verify_hash 收尾凭据
    if let Some(e) = index.contents.get_mut(&req.content_key) {
        if let Some(c) = e.coverage.get_mut(&req.target_site) {
            c.verify_hash = Some(site_hash);
        }
    }
    index.save()?;
    log("校验还没跑完，落「校验中」下一轮收尾");
    Ok(CoverageStatus::Verifying)
}

pub(crate) fn adapter_for(cfg: &Config, site_id: &str) -> Result<NexusPhp> {
    let s = cfg
        .sites
        .iter()
        .find(|s| s.id == site_id)
        .ok_or_else(|| anyhow!("配置里没有站点 {site_id}"))?;
    let cookie = cfg.cookie_header_for(s.primary_host());
    let mut a = NexusPhp::new(
        &s.id,
        &s.name,
        &s.url,
        s.search_url.as_deref(),
        cookie.as_deref(),
    )
    .with_context(|| format!("站点配置不合法：{}", s.id))?;
    a.user_agent = cfg.user_agent.clone();
    Ok(a.with_announce_hosts(s.announce_hosts.clone())
        .with_announce_url(s.announce_url.clone()))
}

/// CLI/调试用：把上传字段清单展开成人能读的行
pub fn dump_fields(fields: &[Field]) -> String {
    let mut out = String::new();
    for (n, v) in fields {
        let flat = mask_field_value(n, v).replace('\n', "\\n");
        out.push_str(&format!("{n}={flat}\n"));
    }
    out
}

/// 草稿 → 请求（report::DraftRow 的字段就够）
pub fn request_from_draft(
    content_key: &str,
    name: &str,
    source_site: &str,
    source_url: &str,
    target_site: &str,
) -> ReseedRequest {
    ReseedRequest {
        content_key: content_key.to_string(),
        source_site: source_site.to_string(),
        source_url: source_url.to_string(),
        target_site: target_site.to_string(),
        name: name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"
<html><head><title>x</title></head><body>
<h1>The.Disguiser.Complete.HDTV.1080p.AAC.x264-JING</h1>
<table><tr><td>副标题</td><td>伪装者 全41集</td></tr></table>
<div id="kdescr">
<p>IMDb: https://www.imdb.com/title/tt4823720/</p>
<p>豆瓣: https://movie.douban.com/subject/26302614/</p>
<pre>Unique ID : 123
Format : AVC
Bit rate : 3500 Kbps
Width : 1920
Height : 1080
Frame rate : 25</pre>
</div>
</body></html>
"#;

    #[test]
    fn parses_release_page() {
        let info = parse_release_page(PAGE, "fallback");
        assert_eq!(info.name, "The.Disguiser.1080p.HDTV.x264.AAC-JING");
        assert_eq!(info.subtitle, "伪装者 全41集");
        assert!(info.mediainfo.contains("Unique ID"));
        assert_eq!(info.imdb.as_deref(), Some("tt4823720"));
        assert_eq!(info.douban.as_deref(), Some("26302614"));
    }

    #[test]
    fn exist_marker_accepts_existed_redirect() {
        let body = format!("<html>{}</html>", "x".repeat(30_000));
        let m = exist_marker("https://sbpt.link/details.php?id=21692&existed=1", &body);
        assert_eq!(m, Some("跳转 URL 带 existed=1"));
    }

    #[test]
    fn exist_marker_ignores_rerendered_upload_form() {
        // 必填项校验失败会把 upload.php 原样返回，页面帮助文案里可能带「已存在」
        let body = r#"<form action="takeupload.php" method="post" enctype="multipart/form-data">
<p>请先搜索站内是否已有该资源；重复资源会提示种子已存在。</p></form>"#;
        assert_eq!(exist_marker("https://x/upload.php", body), None);
    }

    #[test]
    fn exist_marker_ignores_long_page_boilerplate() {
        let mut body = String::from("<html><body>");
        body.push_str(&"y".repeat(12_000));
        body.push_str("<p>规则：重复上传会提示已存在</p></body></html>");
        assert_eq!(exist_marker("https://x/details.php?id=1", &body), None);
    }

    #[test]
    fn exist_marker_reads_errorbox_even_when_form_is_rerendered() {
        // pthome 实测：重复上传会把错误框和上传表单一起返回
        let body = r#"<form action="takeupload.php" method="post" enctype="multipart/form-data">
<div id="errorbox">该种子已存在！</div></form>"#;
        assert_eq!(
            exist_marker("https://x/takeupload.php", body),
            Some("已存在")
        );
    }

    #[test]
    fn exist_marker_reads_short_notice() {
        let body = "<html><body><p>该种子已存在，请勿重复上传</p></body></html>";
        assert_eq!(
            exist_marker("https://x/takeupload.php", body),
            Some("已存在")
        );
    }

    #[test]
    fn preflight_blocks_empty_mediainfo() {
        let mut info = ReleaseInfo {
            name: "x".into(),
            descr: "IMDb: https://www.imdb.com/title/tt1/".into(),
            mediainfo: String::new(),
            ..Default::default()
        };
        let pf = preflight(&mut info, &[], &crate::gates::SiteRules::default());
        assert!(!pf.ok());
        assert!(pf.errors.iter().any(|e| e.contains("MediaInfo")));
    }

    #[test]
    fn preflight_autofixes_links() {
        let mut info = ReleaseInfo {
            name: "x 中字".into(),
            descr: "简介没有链接".into(),
            mediainfo: "Format : AVC\nUnique ID : 1\nBit rate : 1\nWidth : 1\nHeight : 1".into(),
            imdb: Some("tt4823720".into()),
            ..Default::default()
        };
        let tags = vec!["中字".to_string()];
        let pf = preflight(&mut info, &tags, &crate::gates::SiteRules::default());
        assert!(pf.ok(), "{:?}", pf.errors);
        assert!(info.descr.contains("imdb.com"));
        assert!(!pf.fixes.is_empty());
    }

    #[test]
    fn preflight_relaxes_mediainfo_when_rules_allow() {
        // requireMediaInfo=false：预检不再阻断，降为警告（否则门禁放行也发不出去）
        let mut info = ReleaseInfo {
            name: "x".into(),
            descr: "IMDb: https://www.imdb.com/title/tt1/".into(),
            mediainfo: String::new(),
            ..Default::default()
        };
        let rules = crate::gates::SiteRules {
            require_mediainfo: Some(false),
            ..Default::default()
        };
        let pf = preflight(&mut info, &[], &rules);
        assert!(
            pf.ok(),
            "requireMediaInfo=false 时不应阻断：{:?}",
            pf.errors
        );
        assert!(pf.warnings.iter().any(|w| w.contains("放宽")));
    }

    #[test]
    fn parses_form_fields() {
        let form = r#"<form action="takeupload.php" method="post">
<input type="hidden" name="passkey" value="abc">
<input type="text" name="name" value="">
<input type="text" name="small_descr" value="">
<select name="type"><option value="1">电影</option><option value="2" selected>剧集</option></select>
<input type="checkbox" name="tags[]" value="cnsub"> 中字
</form>"#;
        let fields = parse_form_fields(form);
        assert!(fields.contains(&("passkey".into(), "abc".into())));
        assert!(fields.contains(&("type".into(), "2".into())));
        let (fs, picked, _notes) = build_upload_fields(
            &ReleaseInfo {
                name: "T".into(),
                subtitle: "s".into(),
                descr: "d".into(),
                mediainfo: "m".into(),
                ..Default::default()
            },
            form,
            &["中字"],
        );
        assert!(fs.contains(&("name".into(), "T".into())));
        assert!(fs.contains(&("small_descr".into(), "s".into())));
        assert!(picked.iter().any(|p| p == "cnsub"));
    }

    #[test]
    fn tjupt_series_fields_are_filled_from_subtitle_and_tags() {
        let form = r#"<form action="takeupload.php" method="post">
<select name="type"><option value="402" selected>剧集</option></select>
<input type="text" name="small_descr" value="">
<textarea name="descr"></textarea>
</form>"#;
        let info = ReleaseInfo {
            name: "Agatha Christies Poirot S01-S13 Collection Bluray 1080p x265 10bit MNHD-FRDS"
                .into(),
            subtitle: "【大侦探波洛】国英双配 外挂中英ass字幕 10bit HEVC版本".into(),
            descr: "◎标　　签　英剧 | 推理".into(),
            ..Default::default()
        };
        let (fields, _, _) = build_upload_fields_for_site(&info, form, &[], "tjupt");
        assert!(fields.contains(&("cname".into(), "大侦探波洛".into())));
        assert!(fields.contains(&("ename".into(), info.name.clone())));
        assert!(fields.contains(&("specificcat".into(), "英剧".into())));
        assert!(fields.contains(&("英剧".into(), "英剧".into())));
    }

    #[test]
    fn ubits_quality_fields_get_region_and_team_defaults() {
        let form = r#"<form action="takeupload.php" method="post">
<select name="source_sel[4]"><option value="0" selected>请选择一项</option><option value="1">中国大陆</option></select>
<select name="team_sel[4]"><option value="0" selected>请选择一项</option><option value="5">Other</option></select>
<select name="medium_sel[4]"><option value="4" selected>WEB-DL</option></select>
<input type="text" name="name" value="">
<textarea name="descr"></textarea>
</form>"#;
        let info = ReleaseInfo {
            name: "The Story Of Ming Lan 2018 WEB-DL 4k H265 60fps 10bit AAC-HDSWEB".into(),
            subtitle: "知否知否应是绿肥红瘦".into(),
            descr: "中国大陆 古装".into(),
            ..Default::default()
        };
        let (fields, _, _) = build_upload_fields_for_site(&info, form, &[], "ubits");
        assert!(fields.contains(&("source_sel[4]".into(), "1".into())));
        assert!(fields.contains(&("team_sel[4]".into(), "5".into())));
    }

    #[test]
    fn bracket_name_handles_multibyte_brackets() {
        assert_eq!(
            first_bracket_name("【大侦探波洛】国英双配"),
            Some("大侦探波洛".to_string())
        );
        assert_eq!(first_bracket_name("no brackets"), None);
    }

    #[test]
    fn chinese_subs_detection() {
        let mut info = ReleaseInfo {
            name: "Show.S01.1080p".into(),
            ..Default::default()
        };
        assert!(!info.has_chinese_subs());
        info.subtitle = "内封简体中文".into();
        assert!(info.has_chinese_subs());
    }

    #[test]
    fn mediainfo_block_detection() {
        let good = "Unique ID : 1\nFormat : AVC\nBit rate : 3500\nWidth : 1920\nHeight : 1080";
        assert!(is_mediainfo_block(good));
        assert!(!is_mediainfo_block(
            "随便一段话\n第二行\n第三行\n第四行\n第五行"
        ));
    }

    #[test]
    fn error_extraction() {
        let body = r#"<div id="errorbox">请填写必填项目</div>"#;
        assert_eq!(extract_upload_error(body, 200), "请填写必填项目");
        assert!(looks_like_lost_post("请填写必填项目"));
    }

    #[test]
    fn error_extraction_does_not_split_utf8() {
        let body = format!("xx上传失败{}", "子".repeat(200));
        let msg = extract_upload_error(&body, 200);
        assert!(msg.contains("上传失败"), "{msg}");
        assert!(!site_rejects_size(&msg), "{msg}");
    }

    #[test]
    fn too_large_torrent_is_a_site_limit_not_an_unknown_response() {
        // qingwa/CarPT 的真实回包：不是错误框，而是一个自动跳 offers.php 的表单
        let body = r#"<form id='torrent_too_large' action='/offers.php?add_offer=1&torrent_too_large=1' method='POST'>
            <input type='hidden' name='name' value='X' />
        </form>"#;
        let msg = extract_upload_error(body, 200);
        assert!(
            msg.contains("种子过大") && msg.contains("offers.php"),
            "{msg}"
        );
        assert!(!msg.contains("未识别的返回"), "{msg}");
        assert!(site_rejects_size(&msg), "{msg}");

        // 这类结果不是“已跳过”，要留成待跟进：审计词单独一个，周报能数出来
        let outcome = ReseedOutcome::Duplicate(DuplicateOutcome {
            status: CoverageStatus::PendingOffers,
            url: None,
            note: msg,
        });
        assert_eq!(outcome.audit_result(), "pending_offers");
    }

    #[test]
    fn find_links() {
        assert_eq!(find_tt("xx tt4823720 yy"), Some("tt4823720".into()));
        assert_eq!(
            find_douban("https://movie.douban.com/subject/26302614/"),
            Some("26302614".into())
        );
        assert_eq!(
            find_tmdb("https://www.themoviedb.org/tv/61888"),
            Some("https://www.themoviedb.org/tv/61888".into())
        );
    }

    #[test]
    fn strips_promo_suffix_not_content_tags() {
        assert_eq!(
            strip_promo_suffix("Show.S01 1080p-JING    [50%]"),
            "Show.S01 1080p-JING"
        );
        assert_eq!(strip_promo_suffix("Movie.2024 [FREE]"), "Movie.2024");
        assert_eq!(strip_promo_suffix("Movie.2024 [2x]"), "Movie.2024");
        assert_eq!(strip_promo_suffix("Movie.2024 [免费]"), "Movie.2024");
        assert_eq!(
            strip_promo_suffix("Movie.2024 [2X免費] 剩餘時間：2天23時"),
            "Movie.2024"
        );
        assert_eq!(strip_promo_suffix("Movie.2024 [未审]"), "Movie.2024");
        assert_eq!(strip_promo_suffix("Movie.2024 [未审核]"), "Movie.2024");
        // 内容标签保留
        assert_eq!(strip_promo_suffix("Movie.2024 [中字]"), "Movie.2024 [中字]");
        assert_eq!(
            strip_promo_suffix("Movie.2024 [中字] [50%]"),
            "Movie.2024 [中字]"
        );
    }

    #[test]
    fn strips_free_promo_countdown_tail() {
        // luckpt 实测 h1：[免费] 后面还挂着倒计时，角标不在尾部
        assert_eq!(
            strip_promo_suffix(
                "Die Welle 2008 FRA BluRay 1080p x264 DTS-CMCT    [免费] 剩余时间：6天18时"
            ),
            "Die Welle 2008 FRA BluRay 1080p x264 DTS-CMCT"
        );
        assert_eq!(
            strip_promo_suffix("Show.S01 1080p-LuckAni [免费] 剩余时间：23时37分钟"),
            "Show.S01 1080p-LuckAni"
        );
        assert_eq!(
            strip_promo_suffix("Movie.2024 [50%] 剩餘時間：2天3小時"),
            "Movie.2024"
        );
        // hdvideo 实测：倒计时按年/月计，前面挂着百分比角标
        assert_eq!(
            strip_promo_suffix(
                "Star.Wars.Episode.IV.1977.1080p.BluRay.x264.MNHDR-FRDS    [50%] 剩余时间：26年4月"
            ),
            "Star.Wars.Episode.IV.1977.1080p.BluRay.x264.MNHDR-FRDS"
        );
        assert_eq!(strip_promo_suffix("Movie.2024 [免費]"), "Movie.2024");
        // 正文里提到「剩余时间」但后面不是倒计时：不动
        assert_eq!(
            strip_promo_suffix("Movie.2024 剩余时间剧情解析"),
            "Movie.2024 剩余时间剧情解析"
        );
    }

    #[test]
    fn escaped_descr_is_decoded_once() {
        // luckpt 实测：kdescr 存的是转义一层的 HTML，站点只补 nl2br 的裸 <br />
        let escaped = "&lt;fieldset&gt;&lt;legend&gt; 引用 &lt;/legend&gt;&lt;b&gt;官组作品&lt;/b&gt;&lt;/fieldset&gt;&lt;br /&gt;<br />\n&lt;img src=&quot;https://x/a.jpg&quot;&gt;<br />\n&lt;span style=&quot;color:blue&quot;&gt;谢谢&lt;/span&gt;<br />";
        let out = unescape_descr(escaped);
        assert!(
            out.starts_with("<fieldset><legend> 引用 </legend>"),
            "{out}"
        );
        assert!(out.contains("<img src=\"https://x/a.jpg\">"), "{out}");
        // 双重转义的引号只解一层：&amp;quot; → &quot;（渲染成引号），不是裸引号
        assert_eq!(
            unescape_descr(
                "&lt;b&gt;&amp;quot;名&amp;quot;&lt;/b&gt;&lt;i&gt;x&lt;/i&gt;&lt;u&gt;y&lt;/u&gt;"
            ),
            "<b>&quot;名&quot;</b><i>x</i><u>y</u>"
        );
        // 正常裸 HTML 简介：原样保留
        let raw =
            "<fieldset><legend> 引用 </legend>正文</fieldset><img src=\"https://x/a.jpg\"><br />";
        assert_eq!(unescape_descr(raw), raw);
        // 纯文本里引用少量 HTML 示例（裸标签只有 nl2br）：不解码
        let text = "用 &lt;b&gt; 加粗，&lt;i&gt; 斜体，&lt;u&gt; 下划线<br /><br />就这样";
        assert_eq!(unescape_descr(text), text);
    }

    #[test]
    fn escaped_descr_decodes_mixed_and_double_escaped_blocks() {
        // hdvideo 实测：裸 <div> 壳 + 转义 <fieldset> 混排，旧逻辑因裸标签多而整段不解码
        let mixed = r#"<div><fieldset><legend>引用</legend>正文</fieldset></div>
&lt;fieldset&gt;&lt;legend&gt; 引用 &lt;/legend&gt;&lt;br /&gt;转自&lt;b&gt;TLF&lt;/b&gt;&lt;/fieldset&gt;<br />"#;
        let out = unescape_descr(mixed);
        assert!(out.starts_with("<div><fieldset>"), "{out}");
        assert!(out.contains("<fieldset><legend> 引用 </legend>"), "{out}");
        assert!(!out.contains("&lt;fieldset"), "{out}");
        // 两层转义：&amp;lt;fieldset → &lt;fieldset → <fieldset
        let double = "&amp;lt;fieldset&amp;gt;&amp;lt;legend&amp;gt;引用&amp;lt;/legend&amp;gt;正文&amp;lt;/fieldset&amp;gt;";
        let out = unescape_descr(double);
        assert_eq!(out, "<fieldset><legend>引用</legend>正文</fieldset>");
    }

    #[test]
    fn html_descr_is_converted_to_bbcode() {
        // 藏宝阁只吃 BBCode：HTML 原样提交会被转义成字面 <fieldset>，
        // 审核端看不到图/排版（Quick Change 43994 实测）。
        let html = r#"<fieldset><legend> 引用 </legend><br />转自<b>TLF</b>，感谢原发布者！</fieldset><br /><br />
<b><font size="5">本种来自TLFBits<br /><br /></font></b><br />
<img style="max-width: 100%" id="" alt="image" src="https://img9.doubanio.com/view/photo/l_ratio_poster/public/p2300931299.jpg" /><br />
◎IMDb链接　<a class="faqlink" href="https://www.imdb.com/title/tt0100449/" target="_blank">https://www.imdb.com/title/tt0100449/</a><br />"#;
        let bb = html_to_bbcode(html);
        assert!(bb.starts_with("[quote]\n转自[b]TLF[/b]"), "{bb}");
        assert!(bb.contains("[quote]"), "{bb}");
        assert!(bb.contains("[/quote]"), "{bb}");
        assert!(bb.contains("[b][size=5]本种来自TLFBits"), "{bb}");
        assert!(
            bb.contains("[img]https://img9.doubanio.com/view/photo/l_ratio_poster/public/p2300931299.jpg[/img]"),
            "{bb}"
        );
        assert!(
            bb.contains("[url=https://www.imdb.com/title/tt0100449/]"),
            "{bb}"
        );
        assert!(!bb.contains("<fieldset"), "{bb}");
        assert!(!bb.contains("<img"), "{bb}");
        assert!(!bb.contains("&lt;"), "{bb}");
    }

    #[test]
    fn html_to_bbcode_keeps_plain_text_and_repaired_img_url() {
        let text = "用 2 < 3 说明，不动";
        assert_eq!(html_to_bbcode(text), text);
        let html = r#"<img src=" https://x/a.jpg%22" />"#;
        assert_eq!(html_to_bbcode(html), "[img]https://x/a.jpg[/img]");
    }

    #[test]
    fn bbcode_rule_rewrites_descr_field() {
        let mut prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: vec![(
                "descr".into(),
                "<fieldset><legend>引用</legend>正文</fieldset><img src=\"https://x/a.jpg\">"
                    .into(),
            )],
            picked_tags: vec![],
            tag_warn: None,
            select_notes: vec![],
            required_gaps: vec![],
            form_html: String::new(),
        };
        let rules = crate::gates::SiteRules {
            descr_format: Some("bbcode".into()),
            ..Default::default()
        };
        apply_site_rules_tail(&mut prep, &rules);
        let d = field_lookup(&prep.fields, "descr").unwrap_or("");
        assert!(d.starts_with("[quote]\n正文"), "{d}");
        assert!(d.contains("[img]https://x/a.jpg[/img]"), "{d}");
        assert!(!d.contains("<fieldset"), "{d}");
        assert!(
            prep.select_notes
                .iter()
                .any(|n| n.contains("HTML 已转 BBCode")),
            "{:?}",
            prep.select_notes
        );
    }

    #[test]
    fn default_rules_convert_html_descr_to_bbcode() {
        let html = "<fieldset><legend>引用</legend>正文</fieldset><img src=\"https://x/a.jpg\">";
        let mut prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: vec![("descr".into(), html.into())],
            picked_tags: vec![],
            tag_warn: None,
            select_notes: vec![],
            required_gaps: vec![],
            form_html: String::new(),
        };
        apply_site_rules_tail(&mut prep, &crate::gates::SiteRules::default());
        let d = field_lookup(&prep.fields, "descr").unwrap_or("");
        assert!(d.starts_with("[quote]\n正文"), "{d}");
        assert!(d.contains("[img]https://x/a.jpg[/img]"), "{d}");
        assert!(!d.contains("<fieldset"), "{d}");
    }

    #[test]
    fn html_rule_keeps_descr_html() {
        let html = "<fieldset><legend>引用</legend>正文</fieldset>";
        let mut prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: vec![("descr".into(), html.into())],
            picked_tags: vec![],
            tag_warn: None,
            select_notes: vec![],
            required_gaps: vec![],
            form_html: String::new(),
        };
        let rules = crate::gates::SiteRules {
            descr_format: Some("html".into()),
            ..Default::default()
        };
        apply_site_rules(&mut prep, &rules);
        assert_eq!(field_lookup(&prep.fields, "descr"), Some(html));
        assert!(prep.select_notes.is_empty(), "{:?}", prep.select_notes);
    }

    #[test]
    fn single_quoted_ids_are_parsed() {
        let page = r#"<html><body>
<h1>T</h1>
<div id='kdescr'>简介内容 <a href="x">链接</a></div>
<input type='text' name='small_descr' value='副标题'>
</body></html>"#;
        let info = parse_release_page(page, "fallback");
        assert_eq!(info.name, "T");
        assert_eq!(info.subtitle, "副标题");
        assert!(info.descr.contains("简介内容"), "{:?}", info.descr);
        assert!(!info.descr.contains("<html"), "简介兜底不能带标签源码");
    }

    #[test]
    fn descr_fallback_is_body_text_not_raw_html() {
        // 无已知简介容器、也没有「简介」表行：兜底给正文纯文本，不给整页 HTML
        let page = r#"<html><head><style>a{}</style></head><body>
<div class="nav">菜单</div>
<div class="post">真正的简介文字</div>
</body></html>"#;
        let info = parse_release_page(page, "fb");
        assert!(info.descr.contains("真正的简介文字"), "{:?}", info.descr);
        assert!(!info.descr.contains("<div"), "{:?}", info.descr);
    }

    #[test]
    fn truncate_chars_keeps_char_boundary() {
        assert_eq!(truncate_chars("短", 10), "短");
        let t = truncate_chars("中文测试字符串", 3);
        assert!(t.starts_with("中文测"), "{t}");
        assert!(t.contains("…（共 "));
    }

    #[test]
    fn review_render_lists_fields() {
        let rep = ReviewReport {
            info: ReleaseInfo::default(),
            preflight: Preflight::default(),
            prepared: PreparedUpload {
                action: "https://x.example/takeupload.php".into(),
                form_url: "https://x.example/upload.php".into(),
                fields: vec![
                    ("name".into(), "T".into()),
                    ("descr".into(), "很长的简介".repeat(40)),
                ],
                picked_tags: vec!["cnsub".into()],
                tag_warn: None,
                select_notes: vec!["分类：留空等人工选".into()],
                required_gaps: vec![],
                form_html: String::new(),
            },
            audit: Some(AuditVerdict {
                passed: false,
                status: "预审核未通过".into(),
                total_score: 81,
                error_count: 2,
                warning_count: 1,
                suspicious_count: 0,
                details: vec![AuditDetail {
                    level: "error".into(),
                    message: "MediaInfo 为空".into(),
                    score: 30,
                    suggestion: Some("补上技术信息".into()),
                }],
                suggestions: vec!["发布前自查".into()],
            }),
            audit_note: None,
            carry: None,
        };
        let t = rep.render(10);
        assert!(t.contains("只审不发"));
        assert!(t.contains("takeupload.php"));
        assert!(t.contains("官方预审（LuckAudit）：❌"));
        assert!(t.contains("总分 81/100"));
        assert!(t.contains("MediaInfo 为空（-30 分）"));
        assert!(t.contains("name = T"));
        assert!(t.contains("descr = "));
        assert!(t.contains("…（共 "), "{t}");
        assert!(t.contains("标签勾选：cnsub"));
    }

    #[test]
    fn verify_posted_full_match_no_warn() {
        let info = ReleaseInfo {
            name: "Show.S01".into(),
            mediainfo:
                "Unique ID : 1\nFormat : AVC\nBit rate : 3500 Kbps\nWidth : 1920\nHeight : 1080"
                    .into(),
            imdb: Some("tt4823720".into()),
            douban: Some("26302614".into()),
            ..Default::default()
        };
        let good = r#"<h1>Show.S01</h1><div id="kdescr">tt4823720 26302614
<pre>Unique ID : 1
Format : AVC
Bit rate : 3500 Kbps
Width : 1920
Height : 1080</pre></div>"#;
        assert!(verify_posted_html(good, &info).is_empty());
    }

    #[test]
    fn verify_posted_detects_missing_parts() {
        let info = ReleaseInfo {
            name: "Show.S01".into(),
            mediainfo:
                "Unique ID : 1\nFormat : AVC\nBit rate : 3500 Kbps\nWidth : 1920\nHeight : 1080"
                    .into(),
            imdb: Some("tt4823720".into()),
            douban: Some("26302614".into()),
            ..Default::default()
        };
        let bad = r#"<h1>别的名字</h1><div id="kdescr">什么都没有</div>"#;
        let w = verify_posted_html(bad, &info);
        assert!(w.iter().any(|x| x.contains("标题与预期不一致")), "{w:?}");
        assert!(w.iter().any(|x| x.contains("MediaInfo")), "{w:?}");
        assert!(w.iter().any(|x| x.contains("tt4823720")), "{w:?}");
        assert!(w.iter().any(|x| x.contains("26302614")), "{w:?}");
    }

    #[test]
    fn parses_official_audit_response() {
        // 真实接口的 level 是大写（ERROR/WARNING/SUSPICIOUS），detail 带 ruleType/errorCode
        let body = r#"{"success":true,"data":{"passed":false,"status":"未通过","totalScore":81,
            "errorCount":2,"warningCount":1,"suspiciousCount":0,
            "details":[{"ruleType":"TECHNICAL","errorCode":"MEDIAINFO_EMPTY","level":"ERROR","message":"MediaInfo 为空","score":30,"suggestion":"补技术信息"},
                       {"ruleType":"TAG","errorCode":"CN_SUB_NOT_TAGGED","level":"WARNING","message":"中字标签未选","score":5}],
            "suggestions":["发布前自查"]}}"#;
        let v = parse_audit_response(200, body).unwrap();
        assert!(!v.passed);
        assert_eq!(v.total_score, 81);
        assert_eq!(v.error_count, 2);
        assert_eq!(v.details.len(), 2);
        assert_eq!(v.details[0].score, 30);
        assert!(v.details[0].line().contains("❌"), "{:?}", v.details[0]);
        assert!(v.details[0].line().contains("MediaInfo 为空（-30 分）"));
        assert!(v.details[1].line().contains("⚠️"));
        assert!(v.summary().contains("总分 81/100"));
        // success=false 走错误路径
        let err = parse_audit_response(200, r#"{"success":false,"message":"登录状态已失效"}"#);
        assert!(err.is_err());
        // 非 JSON（登录墙 HTML）
        assert!(parse_audit_response(200, "<html>login</html>").is_err());
    }

    #[test]
    fn json_payload_flattens_to_php_form() {
        let payload = serde_json::json!({
            "name": "T",
            "type": {"id": "404", "name": "剧集"},
            "quality": {"medium": {"id": "7", "name": "HDTV"}},
            "tags": [{"id": "23", "name": "中字"}],
            "external_links": {"imdb": "https://www.imdb.com/title/tt1/"},
            "note": serde_json::Value::Null,
            "empty_obj": {},
            "empty_arr": [],
        });
        let form = json_to_form(&payload);
        assert!(form.contains(&("name".into(), "T".into())));
        assert!(form.contains(&("type[id]".into(), "404".into())));
        assert!(form.contains(&("type[name]".into(), "剧集".into())));
        assert!(form.contains(&("quality[medium][id]".into(), "7".into())));
        assert!(form.contains(&("tags[0][id]".into(), "23".into())));
        assert!(form.contains(&("tags[0][name]".into(), "中字".into())));
        assert!(form.contains(&(
            "external_links[imdb]".into(),
            "https://www.imdb.com/title/tt1/".into()
        )));
        // null / 空容器不出字段
        assert!(!form.iter().any(|(k, _)| k.starts_with("note")));
        assert!(!form.iter().any(|(k, _)| k.starts_with("empty")));
    }

    #[test]
    fn audit_payload_bits_parse_from_form() {
        // 字段查找：精确 + 下标形式
        let fields: Vec<Field> = vec![
            ("medium_sel[4]".into(), "7".into()),
            ("type".into(), "404".into()),
        ];
        assert_eq!(field_lookup(&fields, "medium_sel"), Some("7"));
        assert_eq!(field_lookup(&fields, "type"), Some("404"));
        assert_eq!(field_lookup(&fields, "codec_sel"), None);

        // select 选项文案 / data-mode
        let page = r#"<select name='type' data-mode='4'>
<option value="0">请选择</option>
<option value="404" data-class-name="cat404">剧集</option></select>
<input type="checkbox" name="tags[4][]" value="23"> 中字
<select name="medium_sel[4]" data-mode="medium_4">
<option value="0">请选择</option><option value="7">HDTV</option></select>"#;
        assert!(pre_audit_supported(&format!(
            "{page}<script src='js/luckaudit.js'></script>"
        )));
        assert!(!pre_audit_supported(page));
        assert_eq!(
            select_option_meta(page, "type", "404"),
            Some(("剧集".into(), "cat404".into()))
        );
        assert_eq!(
            select_option_meta(page, "medium_sel[4]", "7"),
            Some(("HDTV".into(), String::new()))
        );
        assert_eq!(select_mode(page, "type").as_deref(), Some("4"));
        assert_eq!(checkbox_label(page, "23"), "中字");

        // LUCK_AUDIT_USER 抽取
        let with_user = format!(
            "{page}<script>window.LUCK_AUDIT_USER = {{\"id\":42,\"username\":\"u\"}};</script>"
        );
        let u = extract_luck_audit_user(&with_user).expect("user json");
        assert_eq!(u["id"], 42);
        assert!(extract_luck_audit_user(page).is_none());
    }

    #[test]
    fn iso_utc_formats_known_instants() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00.000Z");
        // 951782400 = 2000-02-29（闰年）T00:00:00Z
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso_utc(1), "1970-01-01T00:00:01.000Z");
    }

    #[test]
    fn strips_inline_event_handlers_only_inside_tags() {
        // 双引号、单引号、裸值；src/alt 与中文正文原样
        let s = r#"<img alt="图" src="http://x/1.jpg" onload="Scale(this,700,0);" onclick='Preview(this);' />
正文 on=1 online=x 原样 <b>加粗</b>"#;
        assert_eq!(
            strip_event_handlers(s),
            r#"<img alt="图" src="http://x/1.jpg" />
正文 on=1 online=x 原样 <b>加粗</b>"#
        );
        // 没有事件属性时原样返回
        assert_eq!(strip_event_handlers("纯文本<br />结束"), "纯文本<br />结束");
    }

    #[test]
    fn select_keywords_are_longest_first_and_deduped() {
        // WEB-DL 内部的 WEB 被重叠规则吃掉；独立出现的 WEB 仍是词元
        assert_eq!(
            find_keywords("Show.WEB-DL.1080p", MEDIUM_KEYWORDS),
            vec!["WEB-DL"]
        );
        assert_eq!(
            find_keywords("Show.WEB-DL.WEB.WEB-DL.1080p", MEDIUM_KEYWORDS),
            vec!["WEB-DL", "WEB"]
        );
        // 同一关键词重复出现只算一个
        assert_eq!(
            find_keywords("Show.HDTV.HDTV.1080p.HDTV", MEDIUM_KEYWORDS),
            vec!["HDTV"]
        );
        // 词边界：HDD 里的 DD 不算
        assert!(find_keywords("My.HDD.x264", AUDIO_KEYWORDS).is_empty());
        assert_eq!(
            find_keywords("Show.1080p.HDTV.x264.AAC-GRP", STANDARD_KEYWORDS),
            vec!["1080p"]
        );
    }

    #[test]
    fn match_option_prefers_exact_norm_and_rejects_ambiguity() {
        let opts = vec![
            ("7".into(), "HDTV".into()),
            ("8".into(), "WEB-DL 高清".into()),
            ("9".into(), "1080p/1080i".into()),
            ("10".into(), "720p".into()),
            ("11".into(), "1080p".into()),
        ];
        // 中文尾缀不影响全等
        assert_eq!(match_option(&opts, "WEB-DL").as_deref(), Some("8"));
        // 分隔子串（唯一）
        assert_eq!(match_option(&opts, "1080i").as_deref(), Some("9"));
        // 1080p 全等命中两个（1080p/1080i 规范化不同，只有 9？不——norm(9)=1080p1080i）
        assert_eq!(match_option(&opts, "1080p").as_deref(), Some("11"));
        // 词元不存在
        assert_eq!(match_option(&opts, "x264"), None);
    }

    #[test]
    fn fill_selects_maps_quality_and_type() {
        let form = r#"<form action="takeupload.php">
<input type="hidden" name="passkey" value="k">
<select name="type"><option value="0">请选择</option><option value="401">电影</option><option value="404">剧集</option><option value="405">综艺</option></select>
<select name="medium_sel[4]"><option value="0">请选择</option><option value="7">HDTV</option><option value="8">WEB-DL</option><option value="9">BluRay</option></select>
<select name="codec_sel[4]"><option value="0">请选择</option><option value="1">x264</option><option value="2">x265</option></select>
<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="1">AAC</option><option value="2">FLAC</option></select>
<select name="standard_sel[4]"><option value="0">请选择</option><option value="1">720p</option><option value="2">1080p</option></select>
<select name="team_sel[4]"><option value="0">请选择</option><option value="5">JING</option><option value="6">WiKi</option></select>
</form>"#;
        let (fs, _picked, notes) = build_upload_fields(&drill_release(), form, &["中字"]);
        let f = |k: &str| field_lookup(&fs, k).unwrap_or("").to_string();
        assert_eq!(f("type"), "404", "国产剧/全41集 → 剧集");
        assert_eq!(f("medium_sel"), "7", "HDTV");
        assert_eq!(f("codec_sel"), "1", "x264");
        assert_eq!(f("audiocodec_sel"), "1", "AAC");
        assert_eq!(f("standard_sel"), "2", "1080p");
        assert_eq!(f("team_sel"), "5", "名尾 -JING");
        assert!(notes.is_empty(), "全部配对成功不该有提示：{notes:?}");
    }

    #[test]
    fn fill_secondary_category_for_ptsbao_style() {
        // ptsbao 实测：source_sel 不是片源，是 type 的二级分类（剧集-大陆/美剧/电影-4K）。
        let form = r#"<form action="takeupload.php">
<select name="type"><option value="0">请选择</option><option value="401">电影</option><option value="402">剧集</option></select>
<select name="source_sel[1]"><option value="0">请选择</option><option value="23">剧集-大陆</option><option value="25">剧集-美剧</option><option value="4">电影-WEB-DL</option><option value="92">电影-4K</option><option value="3">电影-1080</option></select>
</form>"#;
        let tv = ReleaseInfo {
            name: "The.Story.Of.Ming.Lan.2018.WEB-DL.4k.H265.60fps.10bit.AAC-HDSWEB".into(),
            subtitle: "知否知否应是绿肥红瘦 全73集|主演：赵丽颖 冯绍峰".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&tv, form, &[]);
        assert_eq!(field_lookup(&fs, "type"), Some("402"), "{notes:?}");
        assert_eq!(
            field_lookup(&fs, "source_sel"),
            Some("23"),
            "国产剧全N集 → 剧集-大陆：{notes:?}"
        );

        // 电影按分辨率归子类，不是按片源
        let movie = ReleaseInfo {
            name: "Movie.2024.2160p.BluRay.x265-GRP".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&movie, form, &[]);
        assert_eq!(field_lookup(&fs, "type"), Some("401"), "{notes:?}");
        assert_eq!(
            field_lookup(&fs, "source_sel"),
            Some("92"),
            "2160p → 电影-4K：{notes:?}"
        );
    }

    #[test]
    fn secondary_category_detection_ignores_medium_hyphens() {
        let medium = vec![
            ("7".into(), "Blu-ray".into()),
            ("8".into(), "UHD Blu-ray".into()),
            ("9".into(), "WEB-DL".into()),
        ];
        assert!(!looks_like_secondary_category(&medium));
        let category = vec![
            ("23".into(), "剧集-大陆".into()),
            ("25".into(), "剧集-美剧".into()),
            ("92".into(), "电影-4K".into()),
        ];
        assert!(looks_like_secondary_category(&category));
    }

    #[test]
    fn secondary_category_skips_region_source_sel() {
        // 别的站 source_sel 是地区（内地/香港），不能按「剧集-」前缀硬套。
        let form = r#"<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<select name="source_sel[4]"><option value="0">请选择</option><option value="1">内地</option><option value="2">香港</option></select>"#;
        let info = ReleaseInfo {
            name: "国产剧.2024.1080p.WEB-DL.x264-GRP".into(),
            subtitle: "全40集".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "source_sel"), Some("0"), "{notes:?}");
        assert!(
            !notes.iter().any(|n| n.contains("二级分类")),
            "地区下拉不该报二级分类：{notes:?}"
        );
    }

    #[test]
    fn fill_selects_notes_when_token_unmatched() {
        let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="9">BluRay</option></select>"#;
        let info = ReleaseInfo {
            name: "Show.S01.HDTV.1080p".into(),
            ..Default::default()
        };
        let (_fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("medium_sel") && n.contains("HDTV")),
            "HDTV 配不上 BluRay 选项要点名：{notes:?}"
        );
        // 没有质量词元的名字不该有提示
        let (_fs, _p, notes2) = build_upload_fields(
            &ReleaseInfo {
                name: "Plain".into(),
                ..Default::default()
            },
            form,
            &[],
        );
        assert!(notes2.is_empty(), "{notes2:?}");
    }

    #[test]
    fn medium_plain_bluray_without_uhd_picks_plain_label() {
        let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="10">UHD Blu-ray</option></select>"#;
        let info = ReleaseInfo {
            name: "The.Machinist.2004.BluRay.1080p.TrueHD.5.1.x265-BeiTai".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("1"), "{notes:?}");
    }

    #[test]
    fn bdrip_x264_uses_mediainfo_for_encode_and_resolution() {
        // 藏宝阁实测拒绝：标题只有 BDRip X264，媒介/分辨率都没填。
        let form = r#"<form action="takeupload.php">
<select name="medium_sel[4]"><option value="0">请选择</option><option value="9">UHD</option><option value="8">Remux</option><option value="7">Blu-ray</option><option value="6">WEB-DL</option><option value="5">UHDTV</option><option value="4">HDTV</option><option value="3">Encode</option><option value="2">DVD</option></select>
<select name="codec_sel[4]"><option value="0">请选择</option><option value="9">AV1</option><option value="8">HEVC</option><option value="6">AVC</option></select>
<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="5">DDP</option><option value="4">AAC</option><option value="3">DTS</option><option value="2">AC3</option></select>
<select name="standard_sel[4]"><option value="0">请选择</option><option value="7">4320p</option><option value="6">2160p</option><option value="5">1440p</option><option value="4">1080p</option><option value="3">720p</option><option value="2">540p</option><option value="1">480P</option></select>
</form>"#;
        let info = ReleaseInfo {
            name: "Quick Change 1990 BDRip X264 iNT-TLF".into(),
            mediainfo: "Video\nWidth                                    : 1 290 pixels\nHeight                                   : 696 pixels\nAudio\nFormat                                   : AC-3\n".into(),
            ..Default::default()
        };
        let (fs, _picked, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("3"), "{notes:?}");
        assert_eq!(field_lookup(&fs, "standard_sel"), Some("3"), "{notes:?}");
        assert_eq!(field_lookup(&fs, "codec_sel"), Some("6"), "{notes:?}");
        assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("2"), "{notes:?}");
    }

    #[test]
    fn resolution_from_mediainfo_handles_cropped_frames() {
        assert_eq!(
            resolution_from_mediainfo("Width : 1 920 pixels\nHeight : 800 pixels"),
            Some("1080p")
        );
        assert_eq!(
            resolution_from_mediainfo("Width : 3 840 pixels\nHeight : 1 600 pixels"),
            Some("2160p")
        );
        assert_eq!(
            resolution_from_mediainfo("Width : 1 280 pixels\nHeight : 720 pixels"),
            Some("720p")
        );
    }

    #[test]
    fn medium_compound_token_fills_uhd_bluray_option() {
        let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="10">UHD Blu-ray</option><option value="3">Remux</option></select>"#;
        let info = ReleaseInfo {
            name: "Star.Wars.Episode.IX.The.Rise.of.Skywalker.2019.UHD.BluRay.1080p.x265.10bit.DDP.7.1.MNHDR-FRDS.mkv".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("10"), "{notes:?}");
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn audio_aliases_match_real_site_labels() {
        // luckpt 实测选项文案
        let opts: Vec<(String, String)> = vec![
            ("11".into(), "TrueHD Atmos".into()),
            ("16".into(), "DTS-HD MA".into()),
            ("14".into(), "TrueHD".into()),
            ("12".into(), "DDP/E-AC3".into()),
            ("8".into(), "DD/AC3".into()),
            ("6".into(), "AAC".into()),
            ("3".into(), "DTS".into()),
        ];
        // DDP → DD+/E-AC3 一类（EC3/EAC3 别名走 fill 路径，见 audio_alias_fill_from_title）
        assert_eq!(match_option(&opts, "DDP").as_deref(), Some("12"));
        // DTS-HD MA → DTS-HD MA
        assert_eq!(match_option(&opts, "DTS-HD MA").as_deref(), Some("16"));
        // TrueHD → TrueHD（不带 Atmos 不抢 Atmos 标签）
        assert_eq!(match_option(&opts, "TrueHD").as_deref(), Some("14"));
    }

    #[test]
    fn audio_alias_fill_from_title() {
        let form = r#"<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="16">DTS-HD MA</option><option value="12">DDP/E-AC3</option><option value="14">TrueHD</option></select>"#;
        let info = ReleaseInfo {
            name: "Dark.City.1998.DC.BluRay.1080p.2Audio.DTS-HD.MA.7.1.x265.10bit-BeiTai".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("16"), "{notes:?}");
        let info2 = ReleaseInfo {
            name: "Show.2020.1080p.BluRay.EC3.5.1-x265-GRP".into(),
            ..Default::default()
        };
        let (fs2, _p2, notes2) = build_upload_fields(&info2, form, &[]);
        assert_eq!(
            field_lookup(&fs2, "audiocodec_sel"),
            Some("12"),
            "{notes2:?}"
        );
    }

    #[test]
    fn resolution_case_and_i_variants_normalize() {
        let opts: Vec<(String, String)> = vec![
            ("8".into(), "1080p".into()),
            ("9".into(), "1080i".into()),
            ("10".into(), "720p".into()),
        ];
        // 大小写归一：1080P / 1080p 同选项
        assert_eq!(match_option(&opts, "1080P").as_deref(), Some("8"));
        assert_eq!(match_option(&opts, "1080p").as_deref(), Some("8"));
        assert_eq!(match_option(&opts, "1080I").as_deref(), Some("9"));
        assert_eq!(match_option(&opts, "1080i").as_deref(), Some("9"));
    }

    #[test]
    fn episode_marker_picks_tv_not_movie() {
        let form = r#"<select name="type"><option value="0">请选择</option><option value="401">电影</option><option value="402">电视剧</option><option value="405">动漫</option></select>"#;
        // 带 E01-E21 集数特征 → 唯一定位电视剧，不判歧义
        let info = ReleaseInfo {
            name: "白色巨塔.E01-E21.2003.1080p.HDTV.x265.AC3-FRDS".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "type"), Some("402"), "{notes:?}");
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn movie_with_year_no_episode_stays_movie() {
        let form = r#"<select name="type"><option value="0">请选择</option><option value="401">电影</option><option value="402">电视剧</option></select>"#;
        // 无集数特征的年份电影 → 电影，不误伤成剧集
        let info = ReleaseInfo {
            name: "Dark.City.1998.DC.BluRay.1080p.DTS-HD.MA.7.1.x265-BeiTai".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "type"), Some("401"), "{notes:?}");
    }

    #[test]
    fn upload_descr_strips_inline_event_handlers() {
        // WAF 实测：真实上传也会被 on* 属性拦（luckpt 403）；提交前统一剥掉
        let form = r#"<form action="takeupload.php"><input type="file" name="file"><textarea name="descr"></textarea></form>"#;
        let info = ReleaseInfo {
            name: "T".into(),
            descr: r#"<img src="http://x/y.jpg" onload="Scale(this)" onclick="Preview(this)">正文"#
                .into(),
            ..Default::default()
        };
        let (fs, _p, _n) = build_upload_fields(&info, form, &[]);
        let d = field_lookup(&fs, "descr").unwrap_or("");
        assert!(!d.contains("onload") && !d.contains("onclick"), "{d}");
        assert!(d.contains("正文") && d.contains("y.jpg"), "{d}");
    }

    #[test]
    fn post_title_normalization() {
        assert_eq!(
            norm_post_title("Dark City 1998 DC BluRay 1080p-BeiTai"),
            norm_post_title("dark.city.1998.dc.bluray.1080p.beitai")
        );
        assert_ne!(
            norm_post_title("Dark City 1998"),
            norm_post_title("Dark City 1999")
        );
    }

    #[test]
    fn site_title_badges_are_stripped_before_matching() {
        let want = "The Disguiser Complete HDTV 1080p AAC x264-JING";
        let got = "The Disguiser Complete HDTV 1080p AAC x264-JING [免费] 剩余时间：23时59分钟";
        assert_eq!(strip_site_title_badges(got), want);
        assert_eq!(
            norm_post_title(strip_site_title_badges(got)),
            norm_post_title(want)
        );
        // 正常不带促销后缀的标题不能误裁
        assert_eq!(strip_site_title_badges(want), want);
        // 多个角标的顺序和内置列表不同时，要裁在标题里最早出现的那个角标上
        assert_eq!(
            strip_site_title_badges("Some Pack 2020 1080p [2X] [Free]"),
            "Some Pack 2020 1080p"
        );
        assert_eq!(
            strip_site_title_badges("Some Pack 2020 1080p [50%] [Promotion] 剩余时间：1时"),
            "Some Pack 2020 1080p"
        );
        // 角标在最前面时没有可返回的标题前缀，保持原样
        assert_eq!(
            strip_site_title_badges("[Free] Some Pack 2020 1080p"),
            "[Free] Some Pack 2020 1080p"
        );
    }

    #[test]
    fn mediainfo_nbsp_is_normalized() {
        let s = normalize_mediainfo("Duration\u{a0}\u{a0}: 2 h\r\nWidth\u{a0}: 1920");
        assert!(s.contains("Duration  : 2 h") && !s.contains('\u{a0}') && !s.contains('\r'));
    }

    #[test]
    fn mediainfo_completeness_requires_duration_and_dimensions() {
        assert!(mediainfo_is_complete(
            "General
Duration : 47 min
Video
Width : 1920 pixels
Height : 1080 pixels"
        ));
        assert!(!mediainfo_is_complete(
            "General
Format : MPEG-4
Video
Format : AVC"
        ));
    }

    #[test]
    fn chinese_text_track_counts_as_subs() {
        let info = ReleaseInfo {
            mediainfo: "Text #1
Language : Chinese
Title : 简体中文"
                .into(),
            ..Default::default()
        };
        assert!(info.has_chinese_subs());
        let only_english = ReleaseInfo {
            mediainfo: "Text #1
Language : English"
                .into(),
            ..Default::default()
        };
        assert!(!only_english.has_chinese_subs());
    }

    #[test]
    fn bluray_remux_maps_to_remux_option() {
        let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="2">Remux</option><option value="3">Encode</option></select>"#;
        let info = ReleaseInfo {
            name: "A Touch of Zen 1971 Blu-ray REMUX 1080p AVC LPCM".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("2"), "{notes:?}");
    }

    #[test]
    fn checkbox_exact_label_beats_prefix_match() {
        let form = r#"<input type="checkbox" name="tags[4][]" value="1" />HDR10<input type="checkbox" name="tags[4][]" value="2" />HDR10+"#;
        let got = match_checkboxes(form, "HDR10");
        assert_eq!(got, vec![("tags[4][]".to_string(), "1".to_string())]);
    }

    #[test]
    fn chinese_prefix_title_is_trimmed_only_when_english_exists() {
        assert_eq!(
            normalize_release_title("[雄狮少年].I.Am.What.I.Am.2021.CHN.BluRay.1080p-GRP"),
            "I.Am.What.I.Am.2021.CHN.BluRay.1080p-GRP"
        );
        assert_eq!(
            normalize_release_title("武状元苏乞儿.King.of.Beggars.1992.BluRay.1080p-GRP"),
            "King.of.Beggars.1992.BluRay.1080p-GRP"
        );
        assert_eq!(
            normalize_release_title("纯中文标题.2018.1080p"),
            "纯中文标题.2018.1080p"
        );
    }

    #[test]
    fn release_tech_tokens_are_reordered_in_place() {
        assert_eq!(
            normalize_release_title_for("The.Disguiser.Complete.HDTV.1080p.AAC.x264-JING", true),
            "The.Disguiser.1080p.HDTV.x264.AAC-JING"
        );
        assert_eq!(
            normalize_release_title_for(
                "The Story Of Ming Lan 2018 WEB-DL 4k H265 60fps 10bit AAC-HDSWEB",
                true
            ),
            "The Story Of Ming Lan 2018 4k 60fps WEB-DL H265 10bit AAC-HDSWEB"
        );
        assert_eq!(
            normalize_release_title_for(
                "Agatha Christies Poirot S01-S13 Collection Bluray 1080p x265 10bit MNHD-FRDS",
                true
            ),
            "Agatha Christies Poirot S01-S13 Collection 1080p Bluray x265 10bit MNHD-FRDS"
        );
        // 技术词之后的末段整体保留在最后，不要被挪到标题主体前面
        assert_eq!(
            normalize_release_title_for(
                "S07 Mad Men 2007-2014 1080p Blu-ray x265 AC3￡cXcY@FRDS",
                true
            ),
            "S07 Mad Men 2007-2014 1080p Blu-ray x265 AC3￡cXcY@FRDS"
        );
        // 地区、声道/音频细节跟着技术顺序走，压制组仍在最后
        assert_eq!(
            normalize_release_title_for(
                "Coraline.2009.1080p.USA.Blu-ray.AVC.DTS-HD.MA.5.1-PIXELPOLICE",
                true
            ),
            "Coraline.2009.1080p.Blu-ray.USA.AVC.DTS-HD.MA.5.1-PIXELPOLICE"
        );
        // 技术词之后不认识的字样（2Audio、MNHD-FRDS）整体留在最后
        assert_eq!(
            normalize_release_title_for(
                "Stephen.Chow.1988-2017.BluRay.1080p.x265.10bit.2Audio.MNHD-FRDS",
                true
            ),
            "Stephen.Chow.1988-2017.1080p.BluRay.x265.10bit.2Audio.MNHD-FRDS"
        );
        // 方括号中文前缀先按既有规则去掉，再重排技术参数
        assert_eq!(
            normalize_release_title_for(
                "[人民的名义].In.the.Name.of.People.2017.S01.Complete.2160p.60fps.WEB-DL.HEVC.10bit.AAC-QHstudIo",
                true
            ),
            "In.the.Name.of.People.2017.S01.2160p.60fps.WEB-DL.HEVC.10bit.AAC-QHstudIo"
        );
    }

    #[test]
    fn hdr10_tag_detected_from_uhd_title() {
        let info = ReleaseInfo {
            name: "Movie.2024.2160p.UHD.BluRay.HDR10.x265-GRP".into(),
            ..Default::default()
        };
        assert!(wanted_tags(&info).contains(&"HDR10"));

        let sdr = ReleaseInfo {
            name: "Movie.2018.1080p.BluRay.x265-GRP".into(),
            mediainfo: "Video\nHDR format : HDR10".into(),
            ..Default::default()
        };
        assert!(!wanted_tags(&sdr).contains(&"HDR10"));
    }

    #[test]
    fn audio_codec_falls_back_to_mediainfo() {
        let form = r#"<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="8">DD/AC3</option><option value="12">DDP/E-AC3</option><option value="3">DTS</option></select>"#;
        let info = ReleaseInfo {
            name: "Movie.2014.1080p.BluRay.x265-GRP".into(),
            mediainfo: "Audio #1
Format : AC-3
Language : English"
                .into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("8"), "{notes:?}");
    }

    #[test]
    fn ddplus_maps_to_ddp_eac3() {
        let form = r#"<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="8">DD/AC3</option><option value="12">DDP/E-AC3</option></select>"#;
        let info = ReleaseInfo {
            name: "Movie.2014.1080p.BluRay.x265.DD+5.1-GRP".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("12"), "{notes:?}");
    }

    #[test]
    fn h265_without_dot_maps_to_hevc_option() {
        let form = r#"<select name="codec_sel[4]"><option value="0">请选择</option><option value="1">H.264/AVC</option><option value="6">H.265/HEVC</option></select>"#;
        let info = ReleaseInfo {
            name: "Show.2018.WEB-DL.4k.H265.60fps.10bit.AAC-GRP".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "codec_sel"), Some("6"), "{notes:?}");
    }

    #[test]
    fn ac3_prefers_dd_ac3_not_eac3() {
        let form = r#"<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="8">DD/AC3</option><option value="12">DDP/E-AC3</option><option value="3">DTS</option></select>"#;
        let info = ReleaseInfo {
            name: "Movie.2023.1080p.BluRay.x264.AC3-GRP".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("8"), "{notes:?}");
    }

    #[test]
    fn cantonese_audio_tag() {
        let info = ReleaseInfo {
            mediainfo: "Audio #1
Title : 粤语
Language : Chinese"
                .into(),
            ..Default::default()
        };
        assert!(info.has_cantonese_audio());
    }

    #[test]
    fn prune_dead_images_keeps_reachable_only() {
        let html = r#"正文<img src="http://ok/1.jpg"><img src="http://dead/2.jpg">尾"#;
        let (out, n) = prune_dead_images_with(html, &mut |u| u.contains("ok"));
        assert_eq!(n, 1);
        assert!(
            out.contains("ok/1.jpg") && !out.contains("dead/2.jpg"),
            "{out}"
        );
    }

    #[test]
    fn image_content_type_rejects_hotlink_pages() {
        assert!(content_type_is_image(Some("image/jpeg")));
        assert!(content_type_is_image(Some("IMAGE/PNG")));
        assert!(content_type_is_image(Some("application/octet-stream")));
        // 防盗链/验证页：HTML、JSON 不算图片
        assert!(!content_type_is_image(Some("text/html; charset=UTF-8")));
        assert!(!content_type_is_image(Some("application/json")));
        // CT 缺失放行（图床不回 CT 时不误剪）
        assert!(content_type_is_image(None));
    }

    #[test]
    fn upload_dump_gate_is_off_by_default_and_env_gated() {
        // 默认关闭：未设置 / 空串 / 0 / 无关值都不落盘
        assert!(!dump_upload_enabled(None));
        assert!(!dump_upload_enabled(Some("")));
        assert!(!dump_upload_enabled(Some("0")));
        assert!(!dump_upload_enabled(Some("no")));
        assert!(!dump_upload_enabled(Some("yes")));
        // 显式开启：大小写/空白不敏感
        assert!(dump_upload_enabled(Some("1")));
        assert!(dump_upload_enabled(Some("true")));
        assert!(dump_upload_enabled(Some(" on ")));
        assert!(dump_upload_enabled(Some("TRUE")));
    }

    #[test]
    fn bluray_avc_is_not_reencode() {
        let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="2">Encode</option></select>"#;
        let info = ReleaseInfo {
            name: "Bitter Moon 1992 1080p Blu-ray AVC DTS-HD MA 2.0".into(),
            ..Default::default()
        };
        let (fs, _p, _n) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("1"));
    }

    #[test]
    fn chinese_audio_detection() {
        let mk = |mi: &str| ReleaseInfo {
            mediainfo: mi.into(),
            ..Default::default()
        };
        assert!(mk("Audio #1
Language : English
Audio #2
Language : Chinese")
        .has_chinese_audio());
        assert!(mk("Audio #1
Title : Mandarin 2.0 国配")
        .has_chinese_audio());
        assert!(!mk("Audio #1
Language : English
Text #1
Language : Chinese")
        .has_chinese_audio());
        let t = wanted_tags(&mk("Audio #1
Language : Chinese"));
        assert!(t.contains(&"国语"), "{t:?}");
    }

    #[test]
    fn completed_series_detection() {
        let mk = |n: &str, s: &str| ReleaseInfo {
            name: n.into(),
            subtitle: s.into(),
            ..Default::default()
        };
        assert!(mk("The.Disguiser.Complete.HDTV.1080p", "").is_completed_series());
        assert!(mk("Show.S01.1080p", "全41集").is_completed_series());
        assert!(mk("Show.S01.1080p", "已完结").is_completed_series());
        assert!(!mk("Show.S01.1080p", "更新至第10集").is_completed_series());
        // wanted_tags：中字 + 完结都要在
        let t = wanted_tags(&mk("Show.Complete", "中字"));
        assert!(t.contains(&"中字") && t.contains(&"完结"), "{t:?}");
    }

    #[test]
    fn type_inference_variants() {
        let g = infer_type_candidates("The Disguiser Complete", "国产剧--伪装者 全41集").unwrap();
        assert!(g.0.contains(&"国产剧"));
        assert!(infer_type_candidates("Show.S01.1080p", "").is_some());
        assert!(infer_type_candidates("Show.第3季", "").is_some());
        // 年份 + 无集数标记 = 电影惯用命名（强信号；仍由官方预审做最终闸门）
        let m = infer_type_candidates("Dune.2021.1080p.BluRay.x264-GRP", "").unwrap();
        assert!(m.0.contains(&"电影"));
        assert!(infer_type_candidates("Plain.Title.1080p", "").is_none());
    }

    #[test]
    fn release_group_takes_tail_suffix() {
        assert_eq!(
            release_group("The.Disguiser.Complete.HDTV.1080p.AAC.x264-JING").as_deref(),
            Some("JING")
        );
        assert_eq!(release_group("Movie.2020.1080p.BluRay.x264-10bit"), None);
        assert_eq!(release_group("Show.S01"), None);
    }

    #[test]
    fn form_parsing_scoped_to_upload_form() {
        // 导航条搜索表单（search_area）不能掺进上传字段
        let page = r#"<form action="search.php"><input name="search"><select name="search_area"><option value="1">全站</option></select></form>
<form action="takeupload.php" method="post"><input type="hidden" name="passkey" value="k">
<select name="type"><option value="0">请选择</option><option value="404">剧集</option></select></form>"#;
        let fields = parse_form_fields(page);
        assert!(fields.contains(&("passkey".into(), "k".into())));
        assert!(fields.contains(&("type".into(), "0".into())));
        assert!(
            !fields.iter().any(|(n, _)| n == "search_area"),
            "{fields:?}"
        );
        assert_eq!(select_names(page), vec!["type".to_string()]);
        assert!(upload_form_block("<p>无表单</p>").is_none());
    }

    #[test]
    fn checkbox_grid_does_not_bleed_into_neighbor() {
        // 密集标签网格：第一个复选框的 80 字节窗口会盖住邻格「中字」，不能误配
        let page = r#"<input type="checkbox" name="tags[]" value="10"> 原创首发高清首发字幕首发
<input type="checkbox" name="tags[]" value="6"> 中字"#;
        assert_eq!(
            match_checkboxes(page, "中字"),
            vec![("tags[]".to_string(), "6".to_string())]
        );
    }

    #[test]
    fn type_syns_match_sequentially_not_pooled() {
        // hdfans 式双语选项：短词 TV 同时命中「TV Series/电视剧」与「TV Shows/综艺」，
        // 逐同义词唯一命中才能落到电视剧
        let form = r#"<select name="type"><option value="0">请选择</option>
<option value="401">Movies/电影</option>
<option value="402">TV Series/电视剧</option>
<option value="416">TV Shows/综艺</option></select>"#;
        let info = ReleaseInfo {
            name: "The.Disguiser.Complete.1080p.HDTV.x264.AAC-JING".into(),
            subtitle: "国产剧--伪装者 全41集".into(),
            ..Default::default()
        };
        let (fs, _p, _n) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "type"), Some("402"));
    }

    #[test]
    fn denial_reason_covers_traditional_chinese() {
        let t = "對不起... 你沒有發布種子的許可權。如果你是普通會員，請到 候選區 提交你的發布請求";
        assert!(upload_denied_reason(t).unwrap().contains("无发布权限"));
        assert!(upload_denied_reason("对不起... 你没有发布种子的权限").is_some());
        assert!(upload_denied_reason("<p>随便看看</p>").is_none());
    }

    #[test]
    fn fixed_windows_do_not_split_utf8() {
        // 任意窗口端点都回退到字符边界，切片不 panic
        let s = "中文标签中字测试";
        for i in 0..=s.len() + 4 {
            let f = floor_char_boundary(s, i);
            assert!(s.is_char_boundary(f));
            let _ = &s[..f];
        }
        // 定长 80 字节窗口切在汉字中间的真实页面（实机 panic 过）
        let page = format!(
            r#"<input type="checkbox" name="tags[4][]" value="23"> 中字{}"#,
            "字".repeat(40)
        );
        assert_eq!(match_checkboxes(&page, "中字").len(), 1);
        assert!(checkbox_label(&page, "23").contains("中字"));
    }

    #[test]
    fn analyze_form_reports_core_and_soft_gaps() {
        let prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: vec![
                ("name".into(), "T".into()),
                ("small_descr".into(), "s".into()),
                ("descr".into(), "d".into()),
                ("technical_info".into(), "m".into()),
                ("imdb_url".into(), "u".into()),
                ("type".into(), "0".into()),
                ("medium_sel[4]".into(), "7".into()),
            ],
            picked_tags: vec![],
            tag_warn: None,
            select_notes: vec!["分类：留空等人工选".into()],
            required_gaps: vec![],
            form_html: r#"<select name="type"><option value="0">请选择</option><option value="404">剧集</option></select>
<select name="medium_sel[4]"><option value="0">请选择</option><option value="7">HDTV</option></select>
<select name="codec_sel[4]"><option value="0">请选择</option><option value="1">x264</option></select>"#
                .into(),
        };
        let info = ReleaseInfo {
            name: "T".into(),
            subtitle: "s".into(),
            ..Default::default()
        };
        let d = analyze_form("sx", "站X", &prep, &info);
        assert!(d.title_set && d.subtitle_set);
        assert_eq!(d.mediainfo_field.as_deref(), Some("technical_info"));
        assert_eq!(d.link_fields, vec!["imdb_url".to_string()]);
        assert_eq!(
            d.select_picks,
            vec![("medium_sel[4]".into(), "HDTV".into())]
        );
        let core = d.core_gaps();
        assert!(core.iter().any(|g| g.contains("type")), "{core:?}");
        assert!(core.iter().any(|g| g.contains("codec_sel")), "{core:?}");
        let soft = d.soft_gaps();
        assert!(soft.iter().any(|g| g.contains("分类")), "{soft:?}");
    }

    #[test]
    fn hdtv_x264_maps_to_encode_when_no_hdtv_option() {
        let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="2">Encode</option><option value="3">Web-DL</option></select>"#;
        let info = ReleaseInfo {
            name: "Show.Complete.HDTV.1080p.x264-GRP".into(),
            ..Default::default()
        };
        let (fs, _p, _notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("2"));
        // 音乐站没有 Encode：不硬塞，保持留空（站点不收该内容）
        let music = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">CD</option><option value="2">SACD</option></select>"#;
        let (fs, _p, _n) = build_upload_fields(&info, music, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("0"));
    }

    #[test]
    fn reencoded_disc_maps_to_encode() {
        let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="2">Encode</option><option value="3">Remux</option></select>"#;
        let info = ReleaseInfo {
            name: "Dark.City.1998.BluRay.1080p.x265.10bit-GRP".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("2"), "{notes:?}");
        // REMUX 仍按原盘归类，不改判 Encode
        let remux = ReleaseInfo {
            name: "Dark.City.1998.BluRay.REMUX.1080p-GRP".into(),
            ..Default::default()
        };
        let (fs, _p, _n) = build_upload_fields(&remux, form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("1"));
    }

    #[test]
    fn picks_2160p_60fps_option_when_option_text_has_both() {
        let options = vec![
            ("5".to_string(), "2160p/4K".to_string()),
            ("7".to_string(), "2160p/4K 60Fps".to_string()),
        ];
        assert_eq!(
            pick_resolution_option(&options, "In the Name of People 2017 2160p 60fps WEB-DL"),
            Some("7".to_string())
        );
        assert_eq!(
            pick_resolution_option(&options, "In the Name of People 2017 2160p WEB-DL"),
            Some("5".to_string())
        );
    }

    #[test]
    fn webdl_maps_to_encode_when_no_web_option() {
        // oshen 实测：没有 WEB-DL，只有 HDTV/Encode/Blu-ray；留空会被发布检查拦。
        let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="5">HDTV</option><option value="7">Encode</option><option value="3">Remux</option><option value="1">Blu-ray</option></select>"#;
        let info = ReleaseInfo {
            name: "Show.WEB-DL.4k.H265.60fps.10bit.AAC-GRP".into(),
            ..Default::default()
        };
        let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("7"), "{notes:?}");
        // REMUX 仍按原盘归类，不改判 Encode
        let remux = ReleaseInfo {
            name: "Show.WEB-DL.REMUX.4k.H265-GRP".into(),
            ..Default::default()
        };
        let (fs, _p, _n) = build_upload_fields(&remux, form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("3"));
    }

    #[test]
    fn hdtv_alias_matches_tv_label() {
        // aling/ptcafe 实测：媒介选项写「TV」而不是「HDTV」
        let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="4">TV</option><option value="5">WEB-DL</option></select>"#;
        let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel"), Some("4"), "{notes:?}");
    }

    #[test]
    fn bare_resolution_labels_match() {
        // ptsbao 实测：分辨率选项是 480/576/720/1080/2K/4K 裸数字
        let form = r#"<select name="standard_sel[1]"><option value="0">请选择一项</option><option value="1">1080</option><option value="2">720</option></select>"#;
        let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
        assert_eq!(field_lookup(&fs, "standard_sel"), Some("1"), "{notes:?}");
    }

    #[test]
    fn multi_group_selects_all_filled() {
        // ptba 实测：medium_sel[4] 与 medium_sel[9] 是两套分类分支，都要独立配对填上
        let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="7">HDTV</option></select>
<select name="medium_sel[9]"><option value="0">请选择</option><option value="3">HDTV</option><option value="8">Remux</option></select>"#;
        let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
        assert_eq!(field_lookup(&fs, "medium_sel[4]"), Some("7"));
        assert_eq!(field_lookup(&fs, "medium_sel[9]"), Some("3"), "{notes:?}");
    }

    #[test]
    fn ambiguous_options_leave_note_not_guess() {
        // muxuege 实测：两个同名 AAC 选项（value 6/14），不猜
        let form = r#"<select name="audiocodec_sel[4]"><option value="0">请选择一项</option><option value="6">AAC</option><option value="14">AAC</option></select>"#;
        let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
        assert_eq!(field_lookup(&fs, "audiocodec_sel").unwrap_or("0"), "0");
        assert!(
            notes
                .iter()
                .any(|n| n.contains("AAC") && n.contains("多个")),
            "{notes:?}"
        );
    }

    #[test]
    fn region_selects_not_treated_as_quality() {
        // aling/ubits 实测：source_sel 是地区下拉，HDTV 不往里配，也不点名「没配对」
        let form = r#"<select name="source_sel[4]"><option value="0">请选择一项</option><option value="1">内地</option><option value="2">香港</option></select>"#;
        let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
        assert_eq!(field_lookup(&fs, "source_sel").unwrap_or("0"), "0");
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn off_domain_selects_not_core_gaps() {
        // soulvoice 实测：audiocodec_sel 选项是电子书格式（PDF/MOBI），不算音频质量缺口
        let prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: vec![
                ("name".into(), "T".into()),
                ("descr".into(), "d".into()),
                ("technical_info".into(), "m".into()),
                ("type".into(), "402".into()),
                ("audiocodec_sel[8]".into(), "0".into()),
            ],
            picked_tags: vec![],
            tag_warn: None,
            select_notes: vec![],
            required_gaps: vec![],
            form_html: r#"<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<select name="audiocodec_sel[8]"><option value="0">请选择</option><option value="1">PDF</option><option value="2">MOBI</option></select>"#
                .into(),
        };
        let info = ReleaseInfo {
            name: "T".into(),
            ..Default::default()
        };
        let d = analyze_form("sv", "声之形", &prep, &info);
        assert!(
            d.core_gaps().iter().all(|g| !g.contains("audiocodec")),
            "{:?}",
            d.core_gaps()
        );
        assert!(d.soft_gaps().iter().any(|g| g.contains("audiocodec_sel")));
    }

    #[test]
    fn vertical_site_type_is_soft_not_core() {
        // haitang/kelu 实测：曲艺/垂直站分类里没有影视类，样本不适配不算表单缺口
        let form = r#"<select name="type"><option value="0">请选择</option><option value="4097">戏曲</option><option value="1">相声</option></select>"#;
        let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
        assert_eq!(field_lookup(&fs, "type").unwrap_or("0"), "0");
        assert!(notes.iter().any(|n| n.contains("分类")), "{notes:?}");
        let prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: fs,
            picked_tags: vec![],
            tag_warn: None,
            select_notes: notes,
            required_gaps: vec![],
            form_html: form.into(),
        };
        let d = analyze_form("ht", "海棠", &prep, &drill_release());
        assert!(
            d.core_gaps().iter().all(|g| !g.contains("type")),
            "{:?}",
            d.core_gaps()
        );
        assert!(d.soft_gaps().iter().any(|g| g.contains("分类")));
    }

    #[test]
    fn meta_tags_are_not_form_fields() {
        // hdcity 实测：<meta name="description"> 不能当成简介字段
        let page = r#"<meta name="description" content="HDCiTY is a platform">
<form action="takeupload.php"><input type="file" name="file"><input type="hidden" name="uid" value="1"></form>"#;
        assert!(!form_has(page, "description"));
        assert!(form_has(page, "file"));
        assert!(form_has(page, "uid"));
    }

    #[test]
    fn gazelle_field_names_fallback() {
        // dicmusic 实测：Gazelle 架构用 title/subtitle/release_desc
        let form = r#"<form action="upload.php"><input type="file" name="file_input">
<input type="text" name="title"><input type="text" name="subtitle">
<textarea name="release_desc"></textarea></form>"#;
        let (fs, _p, _n) = build_upload_fields(&drill_release(), form, &[]);
        let f = |k: &str| field_lookup(&fs, k).unwrap_or("").to_string();
        assert_eq!(
            f("title"),
            "The.Disguiser.Complete.1080p.HDTV.x264.AAC-JING"
        );
        assert_eq!(
            f("subtitle"),
            "国产剧--伪装者[无台标完整版 全41集1080P][中字]"
        );
        assert!(
            f("release_desc").contains("MediaInfo"),
            "MediaInfo 内嵌简介"
        );
    }

    #[test]
    fn missing_title_field_is_note_not_gap() {
        // tjupt 实测：表单没有标题输入框，标题由种子文件名自动生成
        let form = r#"<form action="takeupload.php"><input type="file" name="file">
<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<textarea name="descr"></textarea></form>"#;
        let (fs, _p, _n) = build_upload_fields(&drill_release(), form, &[]);
        assert!(field_lookup(&fs, "name").is_none());
        let prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: fs,
            picked_tags: vec![],
            tag_warn: None,
            select_notes: _n,
            required_gaps: vec![],
            form_html: form.into(),
        };
        let d = analyze_form("tjupt", "北洋", &prep, &drill_release());
        assert!(d.title_auto);
        assert!(
            d.core_gaps().iter().all(|g| !g.contains("标题")),
            "{:?}",
            d.core_gaps()
        );
    }

    #[test]
    fn run_reseed_refuses_when_publish_gate_closed() {
        // 总闸关闭时必须在任何网络动作之前 bail（CLI/GUI/autopost 共用这道闸）
        let dir = std::env::temp_dir().join(format!("bsend-reseed-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("boxsend.json");
        std::fs::write(
            &cfg_path,
            r#"{
              "sourceSites": [{"id": "luckpt", "name": "LuckPT", "url": "https://pt.luckpt.de/", "framework": "NexusPHP", "enabled": true}],
              "targetSites": ["luckpt"],
              "downloader": {"url": "http://127.0.0.1:1", "username": "u", "password": "p"},
              "assist": {"publishEnabled": false}
            }"#,
        )
        .unwrap();
        let cfg = crate::config::Config::load(&cfg_path).unwrap();
        let qb = crate::qbittorrent::QBittorrent::new(&cfg.downloader.url, "u", "p").unwrap();
        let mut index = crate::index::ContentIndex::load(&cfg.coverage_state_path());
        let req = request_from_draft(
            "m:gate",
            "Gate Test",
            "luckpt",
            "https://pt.luckpt.de/details.php?id=1",
            "luckpt",
        );
        let err = run_reseed(&cfg, &qb, &req, &mut index, &mut |_| {}, None).unwrap_err();
        assert!(
            format!("{err:#}").contains("publishEnabled"),
            "错误必须点明总闸：{err:#}"
        );
    }

    #[test]
    fn review_render_masks_secret_field_values() {
        let prepared = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: vec![
                ("name".into(), "Show.Name.2026".into()),
                ("passkey".into(), "abc123".into()),
                ("c_secure_pass".into(), "deadbeef".into()),
                ("descr".into(), "简介".into()),
            ],
            picked_tags: vec![],
            tag_warn: None,
            select_notes: vec![],
            required_gaps: vec![],
            form_html: String::new(),
        };
        let rep = ReviewReport {
            info: ReleaseInfo::default(),
            preflight: Preflight::default(),
            prepared,
            audit: None,
            audit_note: None,
            carry: None,
        };
        let text = rep.render(200);
        assert!(text.contains("name = Show.Name.2026"), "{text}");
        assert!(text.contains("passkey = <已隐藏 len=6>"), "{text}");
        assert!(text.contains("descr = 简介"), "{text}");
        assert!(!text.contains("abc123"), "passkey 值泄漏：{text}");
        assert!(!text.contains("deadbeef"), "secure 值泄漏：{text}");
        // dump_fields 同样走脱敏
        let dump = dump_fields(&[
            ("passkey".into(), "abc123".into()),
            ("name".into(), "Show".into()),
        ]);
        assert!(!dump.contains("abc123"), "{dump}");
        assert!(dump.contains("name=Show"), "{dump}");
    }

    #[test]
    fn form_action_prefers_upload_form_over_nav_search() {
        // 导航搜索表单排在前面，上传表单在后：action 必须取上传块里的 takeupload.php，
        // 不能整页取第一个 <form> 误配成 search.php（tangpt/longpt 实机故障）
        let base = Url::parse("https://x.example/")
            .unwrap()
            .join("upload.php")
            .unwrap();
        let page = r#"<form action="search.php"><input name="search"><select name="search_area"><option value="1">全站</option></select></form>
<form action="takeupload.php" method="post" enctype="multipart/form-data"><input type="file" name="torrent"></form>"#;
        assert_eq!(
            form_action(page, &base).as_deref(),
            Some("https://x.example/takeupload.php")
        );
    }

    #[test]
    fn form_action_keeps_offsite_absolute_endpoint() {
        // 城市类站点把种子 POST 到独立域名：站外绝对地址照收，不按本站根解析
        let base = Url::parse("https://city.example/upload.php").unwrap();
        let page = r#"<form action="https://up.other-domain.cc/takeupload.php" enctype="multipart/form-data"><input type="file" name="torrent"></form>"#;
        assert_eq!(
            form_action(page, &base).as_deref(),
            Some("https://up.other-domain.cc/takeupload.php")
        );
    }

    #[test]
    fn non_upload_endpoint_blocks_navigation_pages() {
        let base = Url::parse("https://x.example/upload.php").unwrap();
        for bad in [
            "search.php",
            "bonusrain.php",
            "login.php",
            "index.php",
            "torrents.php",
            "browse.php",
            "details.php",
        ] {
            let action = resolve_url(bad, &base);
            assert!(
                non_upload_endpoint(&action, &base).is_some(),
                "{bad} 必须被拦截"
            );
        }
        // 上传端点与站外绝对地址放行
        assert!(non_upload_endpoint("https://x.example/takeupload.php", &base).is_none());
        assert!(non_upload_endpoint("https://up.other.cc/takeupload.php", &base).is_none());
    }

    #[test]
    fn drill_select_options_excludes_nav_search() {
        let page = r#"<form action="search.php"><select name="search_area"><option value="1">全站</option></select></form>
<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="torrent">
<select name="type"><option value="0">请选择</option><option value="404">剧集</option></select>
<select name="medium_sel[4]"><option value="0">请选择</option><option value="7">HDTV</option></select></form>"#;
        let opts = drill_select_options(page);
        assert_eq!(opts.len(), 2, "{opts:?}");
        assert_eq!(opts[0].0, "type");
        assert!(opts[0].1.contains(&("404".to_string(), "剧集".to_string())));
        assert_eq!(opts[1].0, "medium_sel[4]");
        assert!(opts[1].1.contains(&("7".to_string(), "HDTV".to_string())));
    }

    fn prep_from_form(form: &str) -> PreparedUpload {
        let (fields, picked, notes) = build_upload_fields(&drill_release(), form, &[]);
        PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields,
            picked_tags: picked,
            tag_warn: None,
            select_notes: notes,
            required_gaps: vec![],
            form_html: form.into(),
        }
    }

    #[test]
    fn multi_picks_checkboxes_match_and_fill() {
        // 标签归一命中（含别名/子串）→ 按站点真实 value 勾上；必填缺口清空
        let form = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<input type="checkbox" name="style_sel[4][]" value="23"> 中字
<input type="checkbox" name="style_sel[4][]" value="24"> 国语配音
</form>"#;
        let mut prep = prep_from_form(form);
        let mut multi = std::collections::BTreeMap::new();
        multi.insert(
            "style_sel".to_string(),
            vec!["中字".to_string(), "国语".to_string()],
        );
        let rules = crate::gates::SiteRules {
            multi_picks: multi,
            required_fields: vec!["style_sel".into()],
            ..Default::default()
        };
        apply_site_rules(&mut prep, &rules);
        assert!(
            prep.fields
                .iter()
                .any(|(n, v)| n == "style_sel[4][]" && v == "23"),
            "{:?}",
            prep.fields
        );
        assert!(
            prep.fields
                .iter()
                .any(|(n, v)| n == "style_sel[4][]" && v == "24"),
            "{:?}",
            prep.fields
        );
        assert!(prep.required_gaps.is_empty(), "{:?}", prep.required_gaps);
    }

    #[test]
    fn multi_picks_miss_keeps_empty_and_reports_required_gap() {
        // 标签配不上 → 不臆造勾选，保持未选；必填门禁点名未命中标签
        let form = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<input type="checkbox" name="style_sel[4][]" value="23"> 内嵌字幕
<input type="checkbox" name="style_sel[4][]" value="24"> 粤语
</form>"#;
        let mut prep = prep_from_form(form);
        let mut multi = std::collections::BTreeMap::new();
        multi.insert(
            "style_sel".to_string(),
            vec!["中字".to_string(), "国语".to_string()],
        );
        let rules = crate::gates::SiteRules {
            multi_picks: multi,
            required_fields: vec!["style_sel".into()],
            ..Default::default()
        };
        apply_site_rules(&mut prep, &rules);
        assert!(
            !prep.fields.iter().any(|(n, _)| n.starts_with("style_sel")),
            "{:?}",
            prep.fields
        );
        assert_eq!(
            prep.required_gaps,
            vec!["必填缺失：style_sel[4][]（配置标签未命中：中字/国语）".to_string()]
        );
    }

    #[test]
    fn required_single_select_empty_blocks() {
        // 必填单选（select / radio）未选 → 阻断；已选（checked/非 0 value）→ 不报缺口
        let select_form = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<select name="region_sel[4]"><option value="0">请选择</option><option value="1">中国大陆</option></select>
</form>"#;
        let mut prep = prep_from_form(select_form);
        let rules = crate::gates::SiteRules {
            required_fields: vec!["region_sel".into()],
            ..Default::default()
        };
        apply_site_rules(&mut prep, &rules);
        assert_eq!(
            prep.required_gaps,
            vec!["必填缺失：region_sel[4]".to_string()]
        );

        let radio_empty = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<input type="radio" name="region_sel[4]" value="1"> 中国大陆
</form>"#;
        let mut prep2 = prep_from_form(radio_empty);
        apply_site_rules(&mut prep2, &rules);
        assert_eq!(
            prep2.required_gaps,
            vec!["必填缺失：region_sel[4]".to_string()]
        );

        let radio_checked = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<input type="radio" name="region_sel[4]" value="1" checked> 中国大陆
</form>"#;
        let mut prep3 = prep_from_form(radio_checked);
        apply_site_rules(&mut prep3, &rules);
        assert!(prep3.required_gaps.is_empty(), "{:?}", prep3.required_gaps);
    }

    #[test]
    fn default_rules_leave_behavior_unchanged() {
        // 旧配置（无 multiPicks / requiredFields）：不写字段、不记缺口、不加提示
        let form = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<input type="checkbox" name="style_sel[4][]" value="23"> 中字
</form>"#;
        let mut prep = prep_from_form(form);
        let before = prep.fields.len();
        let notes_before = prep.select_notes.len();
        apply_site_rules(&mut prep, &crate::gates::SiteRules::default());
        assert_eq!(prep.fields.len(), before);
        assert_eq!(prep.select_notes.len(), notes_before);
        assert!(prep.required_gaps.is_empty());
    }

    #[test]
    fn body_detail_success_url_kufei_style() {
        // kufei 类站点：提交后不发 Location 跳转，回包正文挂新种详情页唯一链接
        let action = "https://x.example/takeupload.php";
        let body = r#"<html><body><p>种子提交成功，请在个人主页查看</p>
<a href="details.php?id=12345">查看新种</a></body></html>"#;
        let u = body_detail_success_url(body, action, action);
        assert_eq!(u.as_deref(), Some("https://x.example/details.php?id=12345"));

        // 校验失败回显的上传表单（multipart）不参与成功识别
        let rerender = r#"<form action="takeupload.php" method="post" enctype="multipart/form-data">
<p>请填写必填项目</p><a href="details.php?id=999">旧种链接</a></form>"#;
        assert!(body_detail_success_url(rerender, action, action).is_none());

        // 回包里多个详情链接（侧边栏/推荐位）不能唯一定位，不算成功
        let two = r#"<html><body>
<a href="details.php?id=111">A</a><a href="details.php?id=222">B</a></body></html>"#;
        assert!(body_detail_success_url(two, action, action).is_none());

        // 用户主页锚不是种子详情页：只有 userdetails 时不算唯一详情
        let user = r#"<html><body><a href="userdetails.php?id=777">上传者</a></body></html>"#;
        assert!(body_detail_success_url(user, action, action).is_none());
        // 用户主页 + 唯一详情 → 仍能定位到详情
        let user_and_detail = r#"<html><body><a href="userdetails.php?id=777">上传者</a>
<a href="details.php?id=12345">新种</a></body></html>"#;
        assert_eq!(
            body_detail_success_url(user_and_detail, action, action).as_deref(),
            Some("https://x.example/details.php?id=12345")
        );
    }

    #[test]
    fn extract_posted_links_uses_matching_id_not_first_link() {
        // hdvideo 实测：成功页先出现新种，后面跟着相关旧种；下载直链只有新种。
        // 即使旧种链接排在前面，也必须按 id 配对，不能取第一条详情。
        let base = Url::parse("https://hdvideo.top/").unwrap();
        let body = r#"<p>发布成功！</p>
<a href="details.php?id=14469">相关旧种</a>
<a href="download.php?id=78856">下载</a>
<a href="details.php?id=78856">新种</a>
<a href="userdetails.php?id=999">上传者</a>"#;
        let links = extract_posted_links(body, "https://hdvideo.top/takeupload.php", &base);
        assert_eq!(
            links.detail.as_deref(),
            Some("https://hdvideo.top/details.php?id=78856")
        );
        assert_eq!(
            links.download.as_deref(),
            Some("https://hdvideo.top/download.php?id=78856")
        );
    }

    #[test]
    fn extract_posted_links_refuses_mismatched_ids() {
        // 详情指向 A、下载指向 B：分不清哪个是新种，宁可留空转人工，也不挂错种。
        let base = Url::parse("https://x.example/").unwrap();
        let body = r#"<a href="details.php?id=111">A</a><a href="download.php?id=222">B</a>"#;
        let links = extract_posted_links(body, "https://x.example/takeupload.php", &base);
        assert!(links.detail.is_none());
        assert!(links.download.is_none());
    }

    #[test]
    fn tracker_check_allows_same_site_aliases_but_rejects_foreign() {
        let site = SiteConf {
            id: "hdvideo".into(),
            name: "HDVideo".into(),
            url: "https://hdvideo.top/".into(),
            announce_hosts: vec!["hdvideo.top".into()],
            announce_url: None,
            framework: crate::sites::Framework::NexusPHP,
            search_url: None,
            api_key: None,
        };
        let same_site = vec![
            "https://tracker.hdvideo.top/announce.php?passkey=a".to_string(),
            "https://hdvideo.top/announce.php?passkey=a".to_string(),
        ];
        assert!(ensure_trackers_on_site(&site, &same_site).is_ok());
        let foreign = vec!["https://tracker.other.example/announce".to_string()];
        assert!(ensure_trackers_on_site(&site, &foreign).is_err());
        assert!(ensure_trackers_on_site(&site, &[]).is_err());
    }

    #[test]
    fn unresolved_after_site_persists_review_pending_not_failed() {
        let path = std::env::temp_dir().join(format!(
            "bsend-unresolved-{}-{}.json",
            std::process::id(),
            now_unix()
        ));
        let seed = serde_json::json!({
            "contents": {
                "m:test": {
                    "name": "x",
                    "totalSize": 1,
                    "fileCount": 0,
                    "manifestDigest": "",
                    "torrents": {},
                    "trackerHosts": [],
                    "firstSeen": 0,
                    "goneSince": null,
                    "coverage": {},
                    "source": null,
                    "forbidReseed": false
                }
            }
        });
        std::fs::write(&path, seed.to_string()).unwrap();
        let mut index = ContentIndex::load(&path);
        let out = unresolved_after_site(
            &mut index,
            "m:test",
            "hdvideo",
            "posted_unresolved",
            Some("https://hdvideo.top/details.php?id=1".into()),
            "站上已发布，但挂回 qB 失败",
        )
        .unwrap();
        match out {
            ReseedOutcome::Duplicate(d) => {
                assert_eq!(d.status, CoverageStatus::ReviewPending);
                assert!(d.note.starts_with("posted_unresolved"));
                assert!(d.note.contains("勿重发"));
            }
            other => panic!("expected duplicate outcome, got {other:?}"),
        }
        let reloaded = ContentIndex::load(&path);
        let cell = &reloaded.contents["m:test"].coverage["hdvideo"];
        assert_eq!(cell.status, CoverageStatus::ReviewPending);
        assert!(cell
            .note
            .as_deref()
            .unwrap_or("")
            .starts_with("posted_unresolved"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn review_hold_and_seeding_outcomes_persist_distinct_cells() {
        let path = std::env::temp_dir().join(format!(
            "bsend-review-hold-{}-{}.json",
            std::process::id(),
            now_unix()
        ));
        let entry = serde_json::json!({
            "name": "x",
            "totalSize": 1,
            "fileCount": 0,
            "manifestDigest": "",
            "torrents": {},
            "trackerHosts": [],
            "firstSeen": 0,
            "goneSince": null,
            "coverage": {},
            "source": null,
            "forbidReseed": false
        });
        let seed = serde_json::json!({ "contents": { "m:held": entry, "m:seed": entry.clone(), "m:ok": entry } });
        std::fs::write(&path, seed.to_string()).unwrap();
        let mut index = ContentIndex::load(&path);
        let mut log = |_l: &str| {};
        let url = "https://www.hdkyl.in/details.php?id=479362";

        // 站点收下了但不给下载（等审核）：落“等站点审核”，不是 posted_unresolved、不算失败
        let out = review_hold_outcome(
            &mut index,
            "m:held",
            "qilin",
            Some(url.to_string()),
            ReviewMark::Pending,
            "尚未通过审核，请稍后再试",
            &mut log,
        )
        .unwrap();
        assert_eq!(out.audit_result(), "pending_review");
        let ReseedOutcome::Duplicate(d) = &out else {
            panic!("expected duplicate outcome, got {out:?}");
        };
        assert_eq!(d.status, CoverageStatus::PendingReview);
        assert!(!d.note.starts_with("posted_unresolved"), "{}", d.note);
        assert!(d.note.contains("尚未通过审核"), "{}", d.note);
        let cell = &index.contents["m:held"].coverage["qilin"];
        assert_eq!(cell.status, CoverageStatus::PendingReview);
        assert_eq!(cell.url.as_deref(), Some(url));

        // 能下载、也挂回 qB 了，但页面标着不通过：落“站点审核没过”，审计词 review_rejected
        let out = review_seeding_outcome(
            &mut index,
            "m:seed",
            "qilin",
            url,
            ReviewMark::Rejected,
            "审核不通过，原因：缺少海报或截图（图片少于 2 张）",
            &mut log,
        )
        .unwrap();
        assert_eq!(out.audit_result(), "review_rejected");
        let cell = &index.contents["m:seed"].coverage["qilin"];
        assert_eq!(cell.status, CoverageStatus::ReviewRejected);
        assert!(cell
            .note
            .as_deref()
            .is_some_and(|n| n.contains("缺少海报或截图")));

        // 页面标着“通过”＝正常发布完成：返回已发布，格子不动
        let out = review_seeding_outcome(
            &mut index,
            "m:ok",
            "qilin",
            url,
            ReviewMark::Approved,
            "通过",
            &mut log,
        )
        .unwrap();
        assert!(matches!(out, ReseedOutcome::Posted(ref u) if u == url));
        assert!(!index.contents["m:ok"].coverage.contains_key("qilin"));

        // “通过”不算审核卡住：站点标着通过却不给下载，走普通的下载失败路径
        let notice = |mark, why: &str| {
            anyhow::Error::new(SiteNotice {
                text: "提示".to_string(),
                review: Some((mark, why.to_string())),
            })
        };
        assert_eq!(
            site_review_hold(&notice(ReviewMark::Pending, "尚未通过审核")).map(|(m, _)| m),
            Some(ReviewMark::Pending)
        );
        assert_eq!(
            site_review_hold(&notice(ReviewMark::Approved, "通过")),
            None
        );
        assert!(site_review_hold(&anyhow::anyhow!("HTTP 500")).is_none());

        index.save().unwrap();
        let reloaded = ContentIndex::load(&path);
        assert_eq!(
            reloaded.contents["m:held"].coverage["qilin"].status,
            CoverageStatus::PendingReview
        );
        assert_eq!(
            reloaded.contents["m:seed"].coverage["qilin"].status,
            CoverageStatus::ReviewRejected
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn extract_posted_links_prefers_final_redirect() {
        let base = Url::parse("https://x.example/").unwrap();
        let body = r#"<a href="details.php?id=111">旧种</a><a href="download.php?id=333">下载</a>"#;
        let links = extract_posted_links(body, "https://x.example/details.php?id=333", &base);
        assert_eq!(
            links.detail.as_deref(),
            Some("https://x.example/details.php?id=333")
        );
        assert_eq!(
            links.download.as_deref(),
            Some("https://x.example/download.php?id=333")
        );
    }
}
