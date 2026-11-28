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

use std::path::{Path, PathBuf};
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
use crate::sites::upload::{classify_upload_page, permission_keyword, UploadPageKind};
use crate::throttle::{BudgetKind, SiteThrottle};
use crate::torrent_rewrite;

/// 上传成功后立刻把站点种子下回来挂进 qB 是硬步骤，没有配置开关：
/// 站点刚建种时 download.php 可能先回 “torrent not registered”，过几十秒才正常。
/// 这里固定最多试 4 轮、每轮间隔 15 秒；拿不到才落待人工，回访还能用 `bsend pull` 补。
const POST_UPLOAD_PULL_ATTEMPTS: usize = 4;
const POST_UPLOAD_PULL_INTERVAL_SEC: u64 = 15;

mod audit;
mod form;
mod post;
mod title;
mod upload;

use audit::*;
pub use audit::{
    analyze_prepared, drill_release, drill_upload_form, drill_upload_form_with_rules,
    pre_audit_supported, run_pre_audit, AuditDetail, AuditVerdict, FormDrill,
};
pub(crate) use form::image_urls_from_descr;
use form::*;
pub use form::{
    apply_site_rules, build_upload_fields, build_upload_fields_for_site, drill_select_options,
    image_split_counts, parse_form_fields, prepare_category_fields, prune_dead_images_with,
    prune_dead_images_with_site, submitted_descr, upload_form_block, DescrOrderCtx, Field,
    PreparedUpload, UploadResult,
};
use post::*;
pub use post::{extract_posted_links, review_reseed, verify_posted, PostedLinks, ReviewReport};
use title::{norm_post_title, strip_site_title_badges};
use upload::*;
pub use upload::{prepare_upload, submit_upload, upload_release};

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
    // 青蛙实测：媒介为 Remux 时必须同时勾「Remux」标签；表单有该复选框的站
    // 由 match_checkboxes 自动勾上，没有的站不会硬造字段。
    if title_hay.to_ascii_lowercase().contains("remux") {
        out.push("Remux");
    }
    // 真蓝光原盘（不是 Remux/BDRip）勾「原生原盘」；站上没有这个标签就不勾。
    if is_bluray_release(&title_hay) && !title_hay.to_ascii_lowercase().contains("remux") {
        out.push("原生原盘");
    }
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

/// 站点专属标签：通用推导后再补该站审核明确要求的标签。
/// LongPT 的自动审核会点名“60FPS 要勾高帧”“HDR 要勾 HDR”；
/// 1080p HDR 不在通用 `HDR10` 分支里，所以只在这里补，避免影响其他站。
fn wanted_tags_for_site(site_id: &str, info: &ReleaseInfo) -> Vec<&'static str> {
    let mut out = wanted_tags(info);
    if site_id != "longpt" {
        return out;
    }
    let hay = format!("{} {} {}", info.name, info.subtitle, info.mediainfo).to_lowercase();
    let high_frame = hay.contains("60fps")
        || hay.contains("59.94")
        || hay.contains("frame rate : 60")
        || hay.contains("帧率 : 60");
    if high_frame && !out.contains(&"高帧") {
        out.push("高帧");
    }
    if hay.contains("hdr") && !out.contains(&"HDR10") && !out.contains(&"HDR") {
        out.push("HDR");
    }
    out
}

pub fn preflight(
    info: &mut ReleaseInfo,
    tags: &[String],
    rules: &crate::gates::SiteRules,
    site_id: &str,
) -> Preflight {
    let mut pf = Preflight::default();

    // ③ MediaInfo：站点规则明确不要（requireMediaInfo=false）时降为警告，
    // 否则「门禁放行、预检仍阻断」会让自动发种永远发不出去。
    // 除了判空，还要判结构：源页把「参数块」当 MediaInfo 时，站点仍会按错误退回。
    if info.mediainfo.trim().is_empty() || !mediainfo_is_complete(&info.mediainfo) {
        let msg = if info.mediainfo.trim().is_empty() {
            "MediaInfo 为空：源站详情页没有 MediaInfo/BDInfo 块，先在源站补齐或手动粘贴后再转种"
        } else {
            "MediaInfo/BDInfo 不完整：缺 General 时长、Video 宽高或 BDInfo 的 Playlist/Video 段，先补齐再转种"
        };
        if rules.require_mediainfo() {
            pf.errors.push(msg.into());
        } else {
            pf.warnings
                .push(format!("{msg}（站点规则不要 MediaInfo，已放宽）"));
        }
    }
    // 青蛙蓝光/原盘只收 BDInfo。MediaInfo 看着完整也会被判“工具使用错误”。
    if site_id == "qingwa" && is_bluray_release(&info.name) && !is_bdinfo_block(&info.mediainfo) {
        pf.errors.push(
            "青蛙蓝光/原盘必须填 BDInfo（Disc Label / Playlist / Video / Audio / Subtitle），不能交 MediaInfo；源页没有就先补 BDInfo".into(),
        );
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

    // ① 中字标签：LuckAudit 会把“有中文字幕但没勾中字”直接判不通过。
    // 幸运站按硬错误处理；其他站没有 LuckAudit，仍只提示，避免把没有该复选框的站全拦死。
    if info.has_chinese_subs() && !tags.iter().any(|t| t.contains("中字")) {
        let msg = "检测到中文字幕，必须勾上「中字」标签（LuckAudit 会按未选中字退回）";
        if site_id == "luckpt" {
            pf.errors.push(msg.into());
        } else {
            pf.warnings
                .push(format!("{msg}；发布时自动勾选，表单没有该复选框才会漏"));
        }
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
        // p 前面是数字时直接排除：`1080p.HDTV` 的 p 后面也能看到 HDTV 的连续字母，
        // 只靠 6 字符窗口会把 p 当成英文段开头，标题只剩「p HDTV…」。
        if chars[i].is_ascii_alphabetic()
            && (i == 0 || !chars[i - 1].is_ascii_digit())
            // `S01-S07` 里的第二个 S 前面是连字符：它是季范围的后半段，不是英文标题开头。
            // 不排除会把前缀截成「广告狂人S01-」，范围只剩 S07。
            && !(i > 0
                && chars[i - 1] == '-'
                && matches!(chars[i], 'S' | 'E')
                && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()))
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
    // 源标题只有中文名、后面直接跟 1080p/HDTV/x265 这类技术词时，不能当成
    // “中文前缀 + 英文名”把中文名整段裁掉；否则标题会只剩技术参数。
    let first_word = suffix
        .split(['.', ' ', '_', '-', '['])
        .find(|w| !w.is_empty())
        .unwrap_or("");
    if tech_rank(first_word).is_some() {
        return name.to_string();
    }
    if suffix.chars().filter(|c| c.is_ascii_alphanumeric()).count() >= 8
        && suffix.chars().any(|c| c.is_ascii_digit())
    {
        // 中文前缀里常带合集季范围（「广告狂人S01-S07.Mad.Men…」）。
        // 裁掉中文名时要把这段范围搬进英文标题，否则会从 S01-S07 掉成 S07。
        let suffix = match season_range(&prefix) {
            Some(range) if season_range(suffix).as_deref() != Some(range.as_str()) => {
                insert_season_range(suffix, &range)
            }
            _ => suffix.to_string(),
        };
        suffix
    } else {
        name.to_string()
    }
}

/// 提取 `S01-S07` / `E01-E21` 这种连续范围（不是单个 S01）。
fn season_range(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut i = 0;
    while i + 4 < bytes.len() {
        if (bytes[i] == b'S' || bytes[i] == b'E') && bytes[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if bytes.get(j) == Some(&b'-')
                && matches!(bytes.get(j + 1), Some(b'S') | Some(b'E'))
                && bytes.get(j + 2).is_some_and(|b| b.is_ascii_digit())
            {
                let mut k = j + 2;
                while k < bytes.len() && bytes[k].is_ascii_digit() {
                    k += 1;
                }
                return Some(name[i..k].to_string());
            }
        }
        i += 1;
    }
    None
}

/// 把季范围插到英文名和年份/技术参数之间：`Mad.Men.2007...` → `Mad.Men.S01-S07.2007...`。
fn insert_season_range(suffix: &str, range: &str) -> String {
    let sep = if suffix.contains('.') { '.' } else { ' ' };
    let parts: Vec<&str> = suffix.split(sep).collect();
    let at = parts
        .iter()
        .position(|t| {
            let low = t.to_ascii_lowercase();
            tech_rank(t).is_some()
                || (low.len() == 4 && low.chars().all(|c| c.is_ascii_digit()))
                || low
                    .split('-')
                    .next()
                    .is_some_and(|y| y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()))
        })
        .unwrap_or(parts.len());
    let mut out: Vec<&str> = parts[..at].to_vec();
    out.push(range);
    out.extend_from_slice(&parts[at..]);
    out.join(&sep.to_string())
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

/// 站点对标题片源写法的硬性要求。青蛙实测：Remux 资源必须写 `BluRay`，
/// 不能带 `Blu-ray` 的横杠；只改目标站标题，不改源站页和其他站口径。
/// 大小写（`Blu-ray`/`Blu-Ray`/`BLU-RAY`/`blu-ray`）、连字符/空格（`Blu ray`）、
/// 无分隔（`BLURAY`）变体都归一到 `BluRay`。
fn normalize_title_for_site(name: &str, site_id: &str) -> String {
    if site_id != "qingwa" {
        return name.to_string();
    }
    let lower = name.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut out = String::with_capacity(name.len());
    let mut i = 0usize;
    while i < lb.len() {
        if lb[i..].starts_with(b"blu") {
            let sep = if matches!(lb.get(i + 3), Some(b'-') | Some(b' ')) {
                1
            } else {
                0
            };
            let ray_at = i + 3 + sep;
            if lb.len() >= ray_at + 3 && &lb[ray_at..ray_at + 3] == b"ray" {
                out.push_str("BluRay");
                i = ray_at + 3;
                continue;
            }
        }
        let Some(ch) = name[i..].chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
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

/// 青蛙蓝光/原盘只认 BDInfo；从源页里单独捞 Disc Label/Playlist 那一段。
/// 不能复用 `extract_mediainfo`：同一页可能先出现 MediaInfo，先到先得会选错。
fn extract_bdinfo(html: &str, descr: &str) -> String {
    let mut blocks: Vec<String> = Vec::new();
    collect_tag_blocks(html, "pre", &mut blocks);
    collect_tag_blocks(html, "fieldset", &mut blocks);
    collect_tag_blocks(html, "code", &mut blocks);
    collect_tag_blocks(html, "textarea", &mut blocks);
    for b in blocks {
        let text = strip_tags(&b);
        if is_bdinfo_block(&text) {
            return text.trim().to_string();
        }
    }
    let text = strip_tags(descr);
    if is_bdinfo_block(&text) {
        return text.trim().to_string();
    }
    String::new()
}

fn is_bdinfo_block(text: &str) -> bool {
    let has_disc = text.contains("Disc Label") || text.contains("DISC INFO");
    let has_playlist = text.contains("Playlist:");
    let has_video = text.contains("Video:");
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    lines >= 5 && has_disc && has_playlist && has_video
}

/// 蓝光原盘/Remux 资源：青蛙这类站要求交 BDInfo，而不是 MediaInfo。
/// 不把 `BDRip` 算进来：BDRip 是重编码，MediaInfo 才合适。
fn is_bluray_release(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    low.contains("blu-ray")
        || low.contains("bluray")
        || low.contains("remux")
        || low.contains("bd25")
        || low.contains("bd50")
        || low.contains("bd66")
        || low.contains("bd100")
}

fn is_mediainfo_block(text: &str) -> bool {
    if is_bdinfo_block(text) {
        return true;
    }
    const KEYS: [&str; 7] = [
        "Unique ID",
        "Format",
        "Bit rate",
        "Width",
        "DISC INFO",
        "VIDEO:",
        "Video:",
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
    // 站点常把促销角标和审核提示一起挂在标题尾部，例如
    // `Liar Game ... [50%] (通过)`；先按角标位置裁掉，再走尾部循环。
    // 带「剩余时间」的交给下面按倒计时判断，避免把标题正文里的
    // 「剩余时间剧情解析」误当促销尾巴裁掉。
    let name = if name.contains("剩余时间") || name.contains("剩餘時間") {
        name
    } else {
        strip_site_title_badges(name)
    };
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
            let Some((_, close)) = stack.pop() else {
                break;
            };
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

/// 找 `<div>` 上 `id` 或 `class` 等于 `key` 的元素内层 HTML。
/// NexusPHP 错误框有的站写 id、有的写 class，两种都认；认不出返回 None。
fn div_inner_by_key(html: &str, key: &str) -> Option<String> {
    div_inner(html, key).or_else(|| div_class_inner(html, key))
}

/// 按 `class="key"` / `class='key'` 定位 `<div>` 并取内层 HTML。
fn div_class_inner(html: &str, key: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    for pat in [format!("class=\"{key}\""), format!("class='{key}'")] {
        let Some(p) = lower.find(&pat.to_ascii_lowercase()) else {
            continue;
        };
        let open_start = html[..p].rfind("<div")?;
        let content_start = html[open_start..].find('>').map(|i| open_start + i + 1)?;
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
    }
    None
}

/// 站点规则配了 `descrOrder` 时，把简介按站点要求的段标题重新装配。
/// 没配该规则、或上下文缺失时直接返回，简介一字不动。
/// 装配前死图已经在 `apply_site_rules` 开头剪掉；这里只复用留下的可用图。
fn assemble_descr_by_order(
    prep: &mut PreparedUpload,
    rules: &crate::gates::SiteRules,
    ctx: Option<&DescrOrderCtx>,
) {
    if rules.descr_order.is_empty() {
        return;
    }
    let Some(ctx) = ctx else {
        return;
    };
    let Some(field) = ["descr", "release_desc", "description", "content"]
        .iter()
        .find(|f| field_lookup(&prep.fields, f).is_some())
        .copied()
    else {
        return;
    };
    let original = field_lookup(&prep.fields, field).unwrap_or("").to_string();

    // 源简介已经六段齐全且顺序正确：原样保留，不重复包段（幂等）。
    let (missing, ooo) = crate::gates::descr_order_gaps(&original, &rules.descr_order);
    if missing.is_empty() && ooo.is_empty() {
        return;
    }

    // 图片在剪死图之后收集：装配不往段里塞死图。
    let images = image_urls_from_descr(&original);
    let poster = prep
        .poster_field
        .as_deref()
        .and_then(|f| field_lookup(&prep.fields, f))
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
        .or_else(|| images.first().cloned());
    let shots: Vec<String> = match poster.as_deref() {
        Some(p) => images.iter().filter(|u| u.as_str() != p).cloned().collect(),
        None => images.clone(),
    };

    let plain = descr_plain_text(&original, &rules.descr_order);
    let intro = if plain.is_empty() {
        ctx.info.name.clone()
    } else {
        plain
    };

    let media = prep
        .fields
        .iter()
        .find(|(n, _)| {
            ["technical_info", "media_info", "media_bdinfo", "mediainfo"].contains(&n.as_str())
        })
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| ctx.info.mediainfo.clone());

    // 发布组从标题末尾取；取不到就写目标站名，不留空段。
    let group =
        release_group_from_title(&ctx.info.name).unwrap_or_else(|| ctx.target_label.clone());

    // 输出格式跟随原简介：源是 HTML 就用 `<img>`，否则用 BBCode `[img]`。
    let html_like = original.to_ascii_lowercase().contains("<img");
    let render_img = |u: &str| {
        if html_like {
            format!("<img src=\"{u}\" />")
        } else {
            format!("[img]{u}[/img]")
        }
    };

    let mut sections: Vec<(String, String)> = Vec::new();
    for label in &rules.descr_order {
        let body = match label.as_str() {
            "【转载出处】" => {
                if ctx.source_url.trim().is_empty() {
                    ctx.source_label.clone()
                } else {
                    format!("{} {}", ctx.source_label, ctx.source_url)
                }
            }
            "【制作说明】" => group.clone(),
            "【海报】" => poster.as_deref().map(render_img).unwrap_or_default(),
            "【简介】" => intro.clone(),
            "【info】" | "【MediaInfo】" => media.clone(),
            "【截图】" => shots
                .iter()
                .map(|u| render_img(u))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        sections.push((label.clone(), body.trim().to_string()));
    }

    let assembled = render_ordered_sections(&sections);

    // 装配后自查：段齐全且顺序对、图数达站点下限、MediaInfo 不缺。
    let (m2, o2) = crate::gates::descr_order_gaps(&assembled, &rules.descr_order);
    let img_n = crate::gates::image_count(&assembled);
    let floor = rules.require_images.unwrap_or(0);
    let media_ok = !rules.require_mediainfo() || !media.trim().is_empty();

    let mut gaps: Vec<String> = Vec::new();
    if !m2.is_empty() {
        gaps.push(format!("简介分段缺标记：{}", m2.join("/")));
    }
    if !o2.is_empty() {
        gaps.push(format!("简介分段顺序错误：{}", o2.join("/")));
    }
    if img_n < floor {
        gaps.push(format!(
            "简介图片仅 {img_n} 张 < 站点要求 {floor}（先补可达海报/截图）"
        ));
    }
    if !media_ok {
        gaps.push("简介/独立栏 MediaInfo 为空：先补技术信息再发布".into());
    }

    if gaps.is_empty() {
        set_field(&mut prep.fields, field, assembled);
        prep.select_notes
            .push("简介：按站点 descrOrder 装配分段".into());
    } else {
        // 不带着缺段的简介提交：保留原简介，把自查缺口落到阻断清单。
        prep.required_gaps
            .push(format!("简介分段装配未通过自查：{}", gaps.join("；")));
    }
}

/// 把「段标题 → 正文」按规则顺序拼成最终简介；空正文只留标题行。
fn render_ordered_sections(sections: &[(String, String)]) -> String {
    let mut out = String::new();
    for (label, body) in sections {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(label);
        if !body.is_empty() {
            out.push('\n');
            out.push_str(body);
        }
    }
    out
}

/// 取简介纯文本：去掉图片（HTML `<img>` / BBCode `[img]`）和已有段标题标记，
/// 其余 HTML 标签剥成文字，空行折叠。
fn descr_plain_text(descr: &str, order: &[String]) -> String {
    let mut s = descr.to_string();
    // 删 BBCode 图片
    loop {
        let lower = s.to_ascii_lowercase();
        let Some(start) = lower.find("[img]") else {
            break;
        };
        let Some(close) = lower[start..].find("[/img]") else {
            break;
        };
        s.replace_range(start..start + close + "[/img]".len(), "");
    }
    // 删 HTML `<img ...>`
    let lower = s.to_ascii_lowercase();
    let mut i = 0;
    let mut cleaned = String::new();
    while let Some(p) = lower[i..].find("<img") {
        cleaned.push_str(&s[i..i + p]);
        let end = lower[i + p..]
            .find('>')
            .map(|j| i + p + j + 1)
            .unwrap_or(s.len());
        i = end;
    }
    cleaned.push_str(&s[i..]);
    s = cleaned;
    for label in order {
        s = s.replace(label, "");
    }
    s = strip_html_tags(&s);
    let lines: Vec<String> = s
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    lines.join("\n")
}

/// 剥掉其余 HTML 标签（属性不保留），文本原样留下。
fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// 从发布名末尾取制作组：最后一个 `-` 后的词，排除编码/片源/分辨率等技术词。
fn release_group_from_title(name: &str) -> Option<String> {
    let last = name.rsplit('-').next()?.trim_matches('.');
    if last.is_empty() || last.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    const BAD: &[&str] = &[
        "WEB-DL", "WEBRip", "BluRay", "Blu-ray", "REMUX", "HDTV", "BDRip", "BRRip", "x264", "x265",
        "H264", "H265", "AVC", "HEVC", "AAC", "DD+", "DDP", "TrueHD", "DTS", "1080p", "720p",
        "2160p", "4K",
    ];
    if BAD.iter().any(|b| last.eq_ignore_ascii_case(b)) {
        return None;
    }
    if !(2..=12).contains(&last.chars().count()) {
        return None;
    }
    Some(last.to_string())
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

/// 上传页归类后的中止文案；静态表单返回 None，继续走字段构造。
/// 前缀（`[no_permission]` / `[two_fa]` / `[challenge_gate]` / `[spa_api]` 等）
/// 会被 drill_upload_form 解析成 error_kind，也由 `site_block_kind` 识别为站侧阻塞分类。
fn gate_error_message(
    kind: UploadPageKind,
    site_id: &str,
    form_url: &str,
    html: &str,
) -> Option<String> {
    match kind {
        UploadPageKind::StaticForm => None,
        UploadPageKind::NoPermission => {
            let kw = permission_keyword(html).unwrap_or("权限不足");
            Some(format!(
                "[no_permission] 该账号在 {site_id} 无发布权限（页面提示含「{kw}」）；本站条目自动发种中止，待人工确认权限后再处理：{form_url}"
            ))
        }
        UploadPageKind::LoginExpired => {
            Some(format!("[login] 登录态失效（上传页是登录表单）：{form_url}"))
        }
        UploadPageKind::TwoFa => Some(format!(
            "[two_fa] 站点要求先过二次验证（页面脚本跳到 2FA 中间页）：请在前台真实浏览器完成二次验证，会话带过来后再自动发种；纯 HTTP 复现不了 2FA：{form_url}"
        )),
        UploadPageKind::ChallengeGate => Some(format!(
            "[challenge_gate] 站点被人机验证/区域时段拦截：请在前台真实浏览器打开 {form_url} 人工过一次验证（标签页必须在前台，不能最小化或切到后台）；纯 HTTP 复现不了验证结果，当前不自动发种。"
        )),
        UploadPageKind::SpaApi => Some(format!(
            "[spa_api] 站点上传界面是纯前端 SPA/JSON API（{form_url}）：表单控件由网页 JS 渲染，纯 HTTP 拿不到字段名与提交接口；自动发种暂不支持。缺：上传表单控件定义、提交接口地址、浏览器会话鉴权。"
        )),
        UploadPageKind::NoForm => Some(format!(
            "[no-form] 目标站没有可解析的静态上传表单（JS/自定义上传界面，或 cookie 失效），v1 暂不支持：{form_url}"
        )),
    }
}

/// 站侧阻塞四类的审计分类名。审阅/发布返回的错误消息带 `[token]` 前缀时，
/// 这里把它映射成独立的审计 result（与 `formcheck.json` 的 `errorKind` 同名）：
/// `no_permission` / `two_fa` / `challenge_gate` / `spa_api`。
///
/// 这四类是站点侧条件（没权限、要过 2FA、被人机验证拦、纯前端界面），不是我们发错了：
/// 不进失败冷却、覆盖记录不落 `failed`、程序不自动重发。其余前缀（登录失效、
/// 认不出的表单、网络错误）返回 None，仍走原来的失败路径。
pub fn site_block_kind(error: &str) -> Option<&'static str> {
    let token = error
        .strip_prefix('[')
        .and_then(|rest| rest.split(']').next())?;
    match token {
        "no_permission" => Some("no_permission"),
        "two_fa" => Some("two_fa"),
        "challenge_gate" => Some("challenge_gate"),
        "spa_api" => Some("spa_api"),
        _ => None,
    }
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
    /// 种子是否已挂回 qB。`PendingReview` / `ReviewRejected` 也可能为 true
    /// （站上已发布、种子已挂回做种，只是审核结论未定）；`reseed-log` 的 `seeded`
    /// 据此判断，不再靠 note 里有没有“已挂回 qB”这几个字。
    pub mounted: bool,
}

/// 一次转种的结果：发布成功带详情页，或判重命中带明确分类。
/// 自动发种据此写审计（posted / posted_verifying / skipped_duplicate / other_version / failed /
/// existed_pending / posted_unresolved）。
///
/// 「上传成功但回包定位不到新种」复用 `Duplicate(ReviewPending)`：note 以
/// `posted_unresolved` 开头，审计词据此区分；它不记 failed、不进失败冷却表、不计 posted 限额。
#[derive(Debug, Clone)]
pub enum ReseedOutcome {
    Posted(String),
    /// 站上已发布、种子已加入下载器，但本轮校验没跑完；不能当“已在做种”。
    PostedVerifying(String),
    Duplicate(DuplicateOutcome),
}

impl ReseedOutcome {
    /// 审计 result 词表（auto-audit.jsonl / OPERATIONS 同步）
    pub fn audit_result(&self) -> &'static str {
        match self {
            ReseedOutcome::Posted(_) => "posted",
            // 上传成功、种子已加入 qB，但校验还没到 100%：算发布额度，界面不能写“转种完成”。
            ReseedOutcome::PostedVerifying(_) => "posted_verifying",
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
                CoverageStatus::GoneFromSite => "gone_from_site",
                CoverageStatus::ReviewPending => {
                    if d.note.starts_with("posted_unresolved") {
                        // 定位不到 ≠ 发种失败：不进 blocked/failed/review_failed 失败冷却表
                        "posted_unresolved"
                    } else {
                        "existed_pending"
                    }
                }
                // 这些状态不该出现在 Duplicate 里；显式列出而不是用 `_` 兜底，
                // 以后新增 CoverageStatus 时编译器会提醒补映射。
                CoverageStatus::Rejected
                | CoverageStatus::Miss
                | CoverageStatus::WaitSource
                | CoverageStatus::Failed
                | CoverageStatus::ReseedDraft
                | CoverageStatus::Skipped
                | CoverageStatus::Reseeded
                | CoverageStatus::Verifying => "failed",
            },
        }
    }

    /// 这次结果对应的站点详情页（判重命中或发布成功时可能有）。
    pub fn url(&self) -> Option<&str> {
        match self {
            ReseedOutcome::Posted(url) | ReseedOutcome::PostedVerifying(url) => Some(url),
            ReseedOutcome::Duplicate(d) => d.url.as_deref(),
        }
    }

    /// 这次结果的说明文字（审计 note 用；发布成功返回空）。
    pub fn note(&self) -> &str {
        match self {
            ReseedOutcome::Posted(_) | ReseedOutcome::PostedVerifying(_) => "",
            ReseedOutcome::Duplicate(d) => &d.note,
        }
    }

    /// 人工可读的一句话（CLI/GUI 共用）
    pub fn display(&self) -> String {
        match self {
            ReseedOutcome::Posted(url) => format!("转种完成：{url}"),
            ReseedOutcome::PostedVerifying(url) => format!(
                "已发布，种子已加入下载器，校验还没跑完（不能算已在做种，下一轮接着收尾）：{url}"
            ),
            ReseedOutcome::Duplicate(d) => format!("转种结束（{}）：{}", d.status.label(), d.note),
        }
    }
}

/// 发后挂回遇到“同 hash、但原种不是只挂目标站 tracker”：这不是发布成功。
/// 不能加第二条、也不能给原种补挂 tracker，只能留给人处理；审计词仍是
/// `same_hash_skipped`，不要包成 `posted`。
fn same_hash_skip_after_push(url: String) -> ReseedOutcome {
    ReseedOutcome::Duplicate(DuplicateOutcome {
        mounted: false,
        status: CoverageStatus::SameHashSkip,
        url: Some(url),
        note: "站上已发布，但下载器里同 info hash 的种子没有只挂目标站 tracker；qB 不允许同 hash 存两条，按策略不改原种，当前没做种，待人工处理".to_string(),
    })
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
/// BDInfo 是另一套格式，有 Disc Label/Playlist/Video 就算完整，不能拿 MediaInfo 的字段硬套。
fn mediainfo_is_complete(text: &str) -> bool {
    if is_bdinfo_block(text) {
        return true;
    }
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
    src.log_proxy(log);
    tgt.log_proxy(log);

    // ---- 1 源站详情页 → 发布要素 ----
    log(&format!("拉源站详情页：{}", req.source_url));
    let page: TorrentPage = src.fetch_torrent_page(&req.source_url)?;
    let html = src.fetch_html(&req.source_url, src.base.as_str())?;
    let mut info = parse_release_page(&html, &page.name);
    if info.name.is_empty() {
        info.name = req.name.clone();
    }
    // 源站详情页可能只指向合集里的某一季（Mad Men 页 h1 是 S07），但索引里的
    // 内容名写着 S01-S07。用合集范围修正标题，避免把整包发成单季。
    if let Some(range) = season_range(&req.name) {
        if !info.name.to_ascii_uppercase().contains(&range) {
            let candidate = normalize_release_title(&req.name);
            if !candidate.is_empty() && season_range(&candidate).is_some() {
                log(&format!(
                    "标题按合集范围修正：{} → {}",
                    info.name, candidate
                ));
                info.name = candidate;
            }
        }
    }
    // 目标站标题规范先落地，后续预检、审计和提交字段看到的是同一版标题。
    info.name = normalize_title_for_site(&info.name, &req.target_site);
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

    // ---- 1.2 青蛙蓝光/原盘：技术栏要用源页 BDInfo，不能沿用 MediaInfo ----
    if req.target_site == "qingwa" && is_bluray_release(&info.name) {
        let bdinfo = extract_bdinfo(&html, &info.descr);
        if !bdinfo.is_empty() && !is_bdinfo_block(&info.mediainfo) {
            info.mediainfo = bdinfo;
            log(&format!(
                "青蛙蓝光：技术栏改用源页 BDInfo（{} 字节）",
                info.mediainfo.len()
            ));
        }
    }

    // ---- 1.5 源站没有 MediaInfo 时，从本地文件生成（luckpt 等站发种硬性要求）----
    // 青蛙蓝光/原盘不生成 MediaInfo：那里要 BDInfo，生成本地 MediaInfo 只会把错误格式
    // 送上去；源页没有 BDInfo 就交给预检拦下。
    let local_media_ok = !(req.target_site == "qingwa" && is_bluray_release(&info.name));
    if local_media_ok
        && (info.mediainfo.trim().is_empty() || !mediainfo_is_complete(&info.mediainfo))
    {
        if let Ok(Some(mi)) = disk_mediainfo(cfg, req, log) {
            info.mediainfo = mi;
            log(&format!(
                "MediaInfo 已由本地文件生成（{} 字节）",
                info.mediainfo.len()
            ));
        }
    }

    // ---- 2 LuckAudit 预检（三查）----
    let tags: Vec<String> = wanted_tags_for_site(&req.target_site, &info)
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let site_rules = cfg.coverage.rules_for(&req.target_site);
    let pf = preflight(&mut info, &tags, &site_rules, &req.target_site);
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
#[allow(clippy::too_many_arguments)] // 命令入口：配置、下载器、站点、详情页、可选种子文件都要显式传入
pub fn pull_published(
    cfg: &Config,
    qb: &QBittorrent,
    site_id: &str,
    detail_url: &str,
    content_key: Option<&str>,
    torrent_file: Option<&Path>,
    index: &mut ContentIndex,
    log: &mut dyn FnMut(&str),
) -> Result<(CoverageStatus, String)> {
    let tgt = adapter_for(cfg, site_id)?;
    tgt.log_proxy(log);
    let tdata = match torrent_file {
        Some(path) => {
            log(&format!(
                "使用本地已下载的站点种子：{}（不再请求站点下载页）",
                path.display()
            ));
            std::fs::read(path).with_context(|| format!("读本地种子失败：{}", path.display()))?
        }
        None => {
            log(&format!("拉取目标种：{detail_url}"));
            let page = tgt.fetch_torrent_page(detail_url)?;
            tgt.download_torrent(&page, detail_url)?
        }
    };
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
///
/// 先用目标种的总大小和文件数把候选缩到个位数，再让每条候选去 qB 核对分片。
/// 不能直接遍历全部内容：每个候选都要查 qB 进度、导出种子，内容一多就会跑成几小时。
fn find_content_key_by_pieces(
    qb: &QBittorrent,
    index: &ContentIndex,
    tdata: &[u8],
) -> Result<String> {
    let files = bencode::file_list(tdata).context("目标种缺少文件清单，无法按大小找内容")?;
    let total_size: i64 = files.iter().map(|(_, size)| *size).sum();
    let file_count = files.len();
    let candidates = candidate_keys_by_shape(index, total_size, file_count);
    for key in candidates {
        if matching_local_member(qb, &index.contents[&key], tdata)?.is_some() {
            return Ok(key);
        }
    }
    bail!(
        "索引里找不到分片一致的内容（按总大小 {total_size} 字节、{file_count} 个文件没有匹配）；确认 qB 里有本地源种，或用 --key 指定 contentKey"
    )
}

/// 按总大小和文件数筛候选内容键；只用于缩小 qB 核对范围，不作为等价判据。
fn candidate_keys_by_shape(
    index: &ContentIndex,
    total_size: i64,
    file_count: usize,
) -> Vec<String> {
    index
        .contents
        .iter()
        .filter(|(_, entry)| entry.total_size == total_size && entry.file_count == file_count)
        .map(|(key, _)| key.clone())
        .collect()
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
    let dup = match target_has_imdb(&rel.tgt, imdb_for_search, &rel.info.name, log) {
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
                            CoverageStatus::Seeding => (
                                CoverageStatus::Seeding,
                                format!(
                                    "目标站已有同分片「{title}」，下载器里已有同 hash 种子且只挂目标站 tracker，正在做种"
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
                            mounted: false,
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
                        mounted: false,
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
                    mounted: false,
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
    let mut prep = match prepared_from_review {
        Some(prep) => {
            log("复用审阅产物：提交字段与门禁判定为同一份（跳过上传页/官方预审二次抓取）");
            prep
        }
        None => {
            let rules = cfg.coverage.rules_for(&req.target_site);
            let mut prep = prepare_upload(&tgt, &info)?;
            prepare_category_fields(&tgt, &mut prep, &info, &rules)?;
            let ctx = DescrOrderCtx {
                source_label: req.source_site.clone(),
                source_url: req.source_url.clone(),
                target_label: tgt.site_name.clone(),
                info: info.clone(),
            };
            apply_site_rules(&mut prep, &rules, Some(&ctx));
            enforce_chinese_sub_tag(&mut prep, &info, &req.target_site);
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

    // 本地规则缺口（例如简介图片被剪成 0 张）必须在真提交前硬拦；
    // 审阅页会显示，但 `--yes`/自动发种如果只看预检就会漏过去。
    if !prep.required_gaps.is_empty() {
        bail!("本地规则阻断，未上传：{}", prep.required_gaps.join("；"));
    }

    // 杏坛站规要求封面走官方图床：正式提交前把简介第一张图转存；
    // 审阅（dry-run）不做外部写操作，只有真上传这一条路会走到这里。
    if req.target_site == "xingtan" {
        crate::imagebed::rehost_xingtan_cover(cfg, &req.target_site, &mut prep, log)?;
    }

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
                mounted: false,
                status: CoverageStatus::PendingOffers,
                url: None,
                note,
            }));
        }
        Err(e) => return Err(e),
    };
    log(&format!("上传结果：{}", up.message));
    if up.uncertain {
        log("上传结果未确认：不直接判失败、不重投，先按名称+总大小站内核对是否已经收下");
    }

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
                mounted: false,
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
                            CoverageStatus::Seeding => (
                                CoverageStatus::Seeding,
                                "上传回包 existed：下载器里已有同 hash 种子且只挂目标站 tracker，正在做种"
                                    .to_string(),
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
                            mounted: false,
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
                    mounted: false,
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
                    mounted: false,
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
                current_uid: None,
                current_name: None,
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
            mounted: false,
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
        // 同 hash 跳过不是“发布成功”：站上已有条目，但 qB 里那条不是只挂目标站
        // tracker，不能加第二条，也没补挂原种，当前并没有做种。旧版把它包成
        // Posted，日志里出现 posted + same_hash_skip，用户会以为已经在做种。
        Ok(CoverageStatus::SameHashSkip) => {
            let outcome = same_hash_skip_after_push(reported_url);
            log(&format!("发后挂回·未做种：{}", outcome.display()));
            Ok(outcome)
        }
        // 上传成功、种子已加入 qB，但还没确认做种：可能校验没到 100%，
        // 也可能校验到了 100% 但 tracker 还没回包。旧版和普通发布一起走
        // `Ok(other) => Posted`，界面写“转种完成”，实际站点还没看见做种。
        Ok(CoverageStatus::Verifying) => {
            if let Some((mark, why)) = &page_mark {
                log(&format!(
                    "站点页面标着审核结论（{}：{why}）；本轮还没确认做种，等回访时再记",
                    mark.label()
                ));
            }
            log("发后挂回·尚未确认做种：站上已发布，种子已加入 qB，tracker/校验还没确认；下一轮复核");
            Ok(ReseedOutcome::PostedVerifying(reported_url))
        }
        Ok(CoverageStatus::Seeding) => {
            if let Some((mark, why)) = &page_mark {
                log(&format!(
                    "站点页面标着审核结论（{}：{why}）；本轮格子是 {}，等回访时再记",
                    mark.label(),
                    CoverageStatus::Seeding.label()
                ));
            }
            Ok(ReseedOutcome::Posted(reported_url))
        }
        // 当前只有 Seeding 会走到这里。以后新增挂回状态必须在这里显式处理，
        // 不能再用 `Ok(other) => Posted` 把未知结果包成发布成功。
        Ok(other) => {
            let why = format!("挂回 qB 返回未预期状态：{}", other.label());
            log(&format!("发后挂回·{why}"));
            unresolved_after_site(
                index,
                &req.content_key,
                &req.target_site,
                "posted_unresolved",
                Some(reported_url),
                &why,
            )
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

/// 转种动作日志里的 `seeded`：发布完成，或判重结果明确记了种子已挂回 qB。
/// 不再看 note 里有没有“已挂回 qB”这类人写的字：`DuplicateOutcome.mounted` 才是事实源。
fn outcome_seeded(result: &Result<ReseedOutcome>) -> bool {
    match result {
        // `Posted` 只在挂回成功后产生（上传后站点种子已加入 qB 并只挂目标站 tracker）。
        Ok(ReseedOutcome::Posted(_)) => true,
        // 校验没跑完：种子在 qB 里，但还不能算在做种。
        Ok(ReseedOutcome::PostedVerifying(_)) => false,
        Ok(ReseedOutcome::Duplicate(d)) => {
            d.mounted || matches!(d.status, CoverageStatus::Assisted | CoverageStatus::Seeding)
        }
        Err(_) => false,
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
        Ok(ReseedOutcome::PostedVerifying(u)) => (
            "posted_verifying",
            Some(u.as_str()),
            Some("站上已发布，种子已加入 qB，但 tracker/校验还没确认做种；下一轮复核".to_string()),
        ),
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
    let seeded = outcome_seeded(result);
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
                last_seed_check: None,
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
        mounted: false,
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

/// 同 info hash 已在 qB 时，判断这条种子是不是已经在给目标站做种。
/// 只认「progress ≥ 0.999、tracker 非空且每条都属于目标站、至少一条 status=2」；
/// 读不到 tracker、还没下完或 tracker 没回包时按否处理，交给调用方落
/// `same_hash_skip`，不把本地校验完成当成站点已看见做种。
fn same_hash_on_target(cfg: &Config, qb: &QBittorrent, hash: &str, target_site: &str) -> bool {
    let Some(site) = cfg.sites.iter().find(|s| s.id == target_site) else {
        return false;
    };
    let Ok(Some((progress, _))) = qb.torrent_progress(hash) else {
        return false;
    };
    if progress < 0.999 {
        return false;
    }
    let Ok(infos) = qb.tracker_info(hash) else {
        return false;
    };
    let urls: Vec<String> = infos.iter().map(|t| t.url.clone()).collect();
    if ensure_trackers_on_site(site, &urls).is_err() {
        return false;
    }
    // progress=1 只是本地校验完；tracker status=2 才是站点已回包。
    crate::qbittorrent::tracker_confirms_seeding(&infos)
}

/// 把目标站 `.torrent` 推进下载器并校验：发布成功、或目标站已有同分片条目
/// （existed / 站内判重命中）时共用——「新站已有条目」也要下载目标种挂到本地数据上，
/// 否则站点上永远只有发布记录、下载器里没有这个 tracker。
/// 返回最终格子状态：`Reseeded`（校验 100% 且目标 tracker 已回包）/
/// `Verifying`（已发布挂回但 tracker 还没确认，下一轮继续复核）/
/// `Assisted`（下载器里早已有，已补挂 tracker）/ `Seeding`（同 hash 已在做种）；
/// 校验失败返回 Err（格子已写 Failed）。
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
    // 同 hash 不能 add 第二条，但可以先看原种：如果它只挂目标站 tracker 且
    // progress=1，就是已经在给目标站做种，不能误记成“跳过”。
    if matches!(qb.torrent_progress(&target_hash), Ok(Some(_))) {
        if same_hash_on_target(cfg, qb, &target_hash, &req.target_site) {
            let note =
                "下载器里已有同 info hash 种子，且只挂目标站 tracker；已在该站做种，不重复添加";
            write_cell(
                index,
                &req.content_key,
                &req.target_site,
                CoverageStatus::Seeding,
                Some(detail_url.to_string()),
                note,
            );
            index.save()?;
            log(note);
            return Ok(CoverageStatus::Seeding);
        }
        let note = "目标种与下载器已有种子 info hash 相同，但不是“只挂目标站 tracker 且 progress=1”；按策略不改动原种子，跳过该站";
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
        // 同 hash：qB 不允许同 hash 两条；先看原种到底挂的是不是目标站，
        // 只挂目标站就是已在做种，只挂别站/没有 tracker 才是真没挂上。
        if same_hash_on_target(cfg, qb, &target_hash, &req.target_site) {
            let note =
                "下载器里已有同 info hash 种子，且只挂目标站 tracker；已在该站做种，不重复添加";
            write_cell(
                index,
                &req.content_key,
                &req.target_site,
                CoverageStatus::Seeding,
                Some(detail_url.to_string()),
                note,
            );
            index.save()?;
            log(note);
            return Ok(CoverageStatus::Seeding);
        }
        let note = "目标种与下载器已有种子 info hash 相同，但不是“只挂目标站 tracker 且 progress=1”；按策略不改动原种子，跳过该站";
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
                let (confirmed, tracker_note) = match qb.tracker_info(&site_hash) {
                    Ok(infos) => (
                        crate::qbittorrent::tracker_confirms_seeding(&infos),
                        crate::qbittorrent::tracker_status_note(&infos),
                    ),
                    Err(e) => (false, format!("tracker 状态读不到：{e}")),
                };
                // progress=1 只说明本地校验完成；tracker 没回包时不能落“已转种”，
                // 留在“校验中”让下一轮覆盖扫描继续复核，直到站点真的回包。
                let (status, note) = if confirmed {
                    (
                        CoverageStatus::Reseeded,
                        format!("转种成功：校验到 100%，{tracker_note}"),
                    )
                } else {
                    (
                        CoverageStatus::Verifying,
                        format!(
                            "{}校验到 100%，但{tracker_note}；还没确认站点看见做种，下一轮复核",
                            if expected_hash.is_some() {
                                "本站新发种"
                            } else {
                                "补挂种子"
                            }
                        ),
                    )
                };
                log(&format!("校验到 100%，{tracker_note}"));
                write_cell(
                    index,
                    &req.content_key,
                    &req.target_site,
                    status,
                    Some(detail_url.to_string()),
                    &note,
                );
                // 100% 但 tracker 还没回包时也要留下收尾凭据，否则下一轮
                // collect_verifying 没有 hash 可查，会永远卡在“校验中”。
                if status == CoverageStatus::Verifying {
                    if let Some(e) = index.contents.get_mut(&req.content_key) {
                        if let Some(c) = e.coverage.get_mut(&req.target_site) {
                            c.verify_hash = Some(site_hash.clone());
                        }
                    }
                }
                index.save()?;
                return Ok(status);
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
    a.upload_timeout_sec = cfg.coverage.rules_for(&s.id).upload_timeout_sec();
    let a = a
        .with_announce_hosts(s.announce_hosts.clone())
        .with_announce_url(s.announce_url.clone())
        .with_image_fields(s.poster_field.clone(), s.screenshot_field.clone())
        .with_proxy(s.proxy.as_deref())
        .with_context(|| format!("站点 {site_id} 代理配置不合法"))?;
    Ok(a)
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
mod tests;
