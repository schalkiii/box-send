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

use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use url::Url;

use crate::bencode;
use crate::config::Config;
use crate::coverage::now_unix;
use crate::index::{ContentIndex, CoverageStatus, SiteCoverage};
use crate::qbittorrent::QBittorrent;
use crate::sites::nexusphp::{decode_entities, resolve_url, strip_tags, NexusPhp, TorrentPage};

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
        MARKERS.iter().any(|m| hay.contains(m))
    }

    /// 完结剧集特征：完结 / 全集 / Complete（拉丁词边界）
    pub fn is_completed_series(&self) -> bool {
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
    out
}

pub fn preflight(info: &mut ReleaseInfo, tags: &[String]) -> Preflight {
    let mut pf = Preflight::default();

    // ③ MediaInfo
    if info.mediainfo.trim().is_empty() {
        pf.errors.push(
            "MediaInfo 为空：源站详情页没有 MediaInfo/BDInfo 块，先在源站补齐或手动粘贴后再转种"
                .into(),
        );
    }

    // ② 链接：有 id 就自动补进简介；三个 id 都缺才是阻断
    if !info.has_media_links() {
        if info.ensure_media_links() {
            pf.fixes
                .push("简介缺条目链接，已按源页 IMDb/豆瓣/TMDB 补上".into());
        } else {
            pf.errors.push(
                "简介缺少 IMDb/豆瓣/TMDB 链接，且源页解析不出条目 id：请手动把链接写进简介".into(),
            );
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
    // MediaInfo：<pre>、引用框、code 块里含 Unique ID / DISC TITLE 特征的段
    info.mediainfo = extract_mediainfo(html, &info.descr);
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
            || matches!(low.as_str(), "免费" | "折扣" | "促销" | "半价")
            || (low.ends_with('x')
                && low.len() <= 4
                && low[..low.len() - 1].chars().all(|c| c.is_ascii_digit()));
        if !promo {
            break;
        }
        s = t[..open].trim_end().to_string();
    }
    s
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
    "H.265", "H.264", "x265", "x264", "HEVC", "AVC", "XviD", "DivX", "AV1", "VC-1", "VC1",
    "MPEG-2", "MPEG2", "MPEG4", "VP9",
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
    "Atmos",
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
            let post_ok = end >= h.len() || !h.as_bytes()[end].is_ascii_alphanumeric();
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
    if sub.len() > 1 {
        return Pairing::Ambiguous;
    }
    Pairing::Miss
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
        "eac3" | "ddp" | "dd+" => &["DD+", "E-AC-3", "EAC3", "DDP"],
        "dtshdma" => &["DTS-HD MA", "DTS-HD", "DTS"],
        "dtshd" => &["DTS-HD", "DTS"],
        "webdl" => &["WEB-DL", "WEB"],
        "webrip" => &["WEBRip", "WEB"],
        "bluray" | "blurayremux" => &["BluRay", "Blu-ray"],
        _ => &[],
    }
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

/// 选档回填：发布名/副标题词元 → 分类与质量下拉。只动当前值为 0/空的下拉，
/// 词元与选项唯一配对才填；有词元配不上、或推断不唯一，留提示不猜。
/// 同前缀多组下拉（`medium_sel[4]`/`medium_sel[9]`）各自独立配对全填。
/// 返回提示清单（审阅/演练里展示）。
fn fill_selects(fields: &mut Vec<Field>, page: &str, info: &ReleaseInfo) -> Vec<String> {
    let mut notes = Vec::new();
    let hay = format!("{} {}", info.name, info.subtitle);
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
    let reencoded = codec_tokens.iter().any(|t| {
        matches!(
            t.to_ascii_lowercase().as_str(),
            "x264" | "x265" | "h264" | "h265" | "avc" | "hevc"
        )
    });
    for (prefix, kws) in cats {
        let names = select_field_names(fields, prefix);
        if names.is_empty() {
            continue;
        }
        let tokens = find_keywords(&hay, kws);
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
            if !options_in_domain(&options, kws) {
                continue;
            }
            relevant_seen = true;
            let mut picked = None;
            // 原盘/光盘源 + 有损重编码 → 站点规范通常归「Encode」而非「Blu-ray 原盘」
            // （luckpt 官方预审实测：x265 10bit 标 Blu-ray 会报「媒介与标题不匹配」）。
            // 只在站点确实提供 Encode 选项时生效；REMUX/HDTV 不受影响。
            let disc_source = tokens.iter().any(|t| {
                let l = t.to_ascii_lowercase();
                l.contains("blu") || l.contains("bluray") || l == "dvd"
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
    notes
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

/// 表单 action 绝对地址（站外绝对地址照收：城市等站把种子 POST 到独立域名；
/// 相对路径按站点根解析成 takeupload.php 一类的提交地址）
fn form_action(page: &str, base: &Url) -> Option<String> {
    let lower = page.to_ascii_lowercase();
    let p = lower.find("<form")?;
    let end = lower[p..]
        .find('>')
        .map(|j| p + j + 1)
        .unwrap_or(page.len());
    let tag = &page[p..end];
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
            if label.contains(&kw) {
                if let Some(name) = attr(tag, "name") {
                    let value = attr(tag, "value").unwrap_or_else(|| "yes".into());
                    out.push((name, value));
                }
            }
        }
        i = end;
    }
    out
}

// ---------------- 上传提交 ----------------

pub struct UploadResult {
    pub detail_url: Option<String>,
    pub message: String,
    /// 站点回「种子已存在」：视为已覆盖，不算本次转种
    pub duplicate: bool,
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
    /// 表单页原文（官方预审要从中取 LUCK_AUDIT_USER / 选项文案）
    pub form_html: String,
}

/// 应用逐站显式选档覆盖（`reseedRules.<site>.selectPicks`）：只在字段当前为空/0 时写入，
/// 且 value 必须是该站表单真实存在的选项（防旧规则在新版表单上写无效值）。
pub fn apply_site_rules(prep: &mut PreparedUpload, rules: &crate::gates::SiteRules) {
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
    let (fields, picked_tags, select_notes) = build_upload_fields(info, &form_html, &tags_wanted);
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
    Ok(PreparedUpload {
        action,
        form_url,
        fields,
        picked_tags,
        tag_warn,
        select_notes,
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
) -> Result<UploadResult> {
    let prep = prepare_upload(target, info)?;
    submit_upload(target, &prep, torrent_data, filename)
}

/// 用已构造好的字段提交（`prepare_upload` 的产出）。官方预审、审阅与
/// 正式转种共用同一份字段，永不漂移。
pub fn submit_upload(
    target: &NexusPhp,
    prep: &PreparedUpload,
    torrent_data: &[u8],
    filename: &str,
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
        let body = resp.body;
        // 「已存在」先判：站点常跳回已有种子详情页，先判成功会误报转种成功
        if has_exist_marker(&body) {
            let dup_url = sole_detail_url(&body, &resp.final_url, &prep.action);
            return Ok(UploadResult {
                detail_url: dup_url,
                message: format!("站点提示种子已存在（视为已覆盖，不算本次转种）{note_suffix}"),
                duplicate: true,
            });
        }
        // 成功：跳到详情页 / 发布成功提示
        if let Some(u) = redirect_detail_url(&resp.final_url, &prep.action) {
            return Ok(UploadResult {
                detail_url: Some(u),
                message: format!("发布成功（跳转详情页）{note_suffix}"),
                duplicate: false,
            });
        }
        if body.contains("发布成功")
            || body.contains("new torrent")
            || body.contains("Torrent added")
        {
            let u = sole_detail_url(&body, &resp.final_url, &prep.action);
            return Ok(UploadResult {
                detail_url: u.clone(),
                message: if u.is_some() {
                    format!("发布成功{note_suffix}")
                } else {
                    format!("发布成功（响应页没找到详情链接，需手动确认新种）{note_suffix}")
                },
                duplicate: false,
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
    if let Some(t) = div_inner(body, "errorbox") {
        let t = strip_tags(&t).trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    for marker in ["已经上传", "已存在", "被人上传", "上传失败", "发布失败"] {
        if let Some(p) = body.find(marker) {
            let seg = strip_tags(&body[p..(p + 120).min(body.len())]);
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

fn has_exist_marker(s: &str) -> bool {
    [
        "已经上传",
        "已存在",
        "被人上传",
        "already exists",
        "Torrent already",
    ]
    .iter()
    .any(|m| s.contains(m))
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

fn sole_detail_url(body: &str, final_url: &str, action: &str) -> Option<String> {
    if let Some(u) = redirect_detail_url(final_url, action) {
        return Some(u);
    }
    // 响应体里唯一的 details.php?id=N
    let mut found: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(p) = rest.find("details.php?id=") {
        let after = &rest[p + "details.php?id=".len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            let url = resolve_url(
                &format!("details.php?id={digits}"),
                &Url::parse(action).ok()?,
            );
            if !found.contains(&url) {
                found.push(url);
            }
        }
        rest = &after[digits.len()..];
    }
    if found.len() == 1 {
        Some(found.pop().unwrap())
    } else {
        None
    }
}

// ---------------- 审阅模式（不提交） ----------------

/// 审阅结果：将要提交的全部内容摆出来，不 POST、不落账
pub struct ReviewReport {
    pub info: ReleaseInfo,
    pub preflight: Preflight,
    pub prepared: PreparedUpload,
    /// 站点官方预审（LuckAudit）结论——支持该接口的站才有
    pub audit: Option<AuditVerdict>,
    /// 官方预审没跑通时的原因（只提示，不阻断审阅）
    pub audit_note: Option<String>,
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
            out.push_str(&format!("  {n} = {}\n", truncate_chars(v, max)));
        }
        out
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
pub fn review_reseed(
    cfg: &Config,
    req: &ReseedRequest,
    log: &mut dyn FnMut(&str),
) -> Result<ReviewReport> {
    let rel = prepare_released(cfg, req, log)?;
    log("抓目标站上传表单，构造提交字段…");
    let mut prepared = prepare_upload(&rel.tgt, &rel.info)?;
    if let Some(rules) = cfg.coverage.reseed_rules.get(&req.target_site) {
        apply_site_rules(&mut prepared, rules);
    }
    log(&format!(
        "审阅完成：{} 个字段 → {}（未提交）",
        prepared.fields.len(),
        prepared.action
    ));
    // 站点官方预审（LuckAudit）：把将要提交的字段交给站方打分，结论进审阅报告
    let (audit, audit_note) = if pre_audit_supported(&prepared.form_html) {
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

/// 发后校正：上传回包给的详情页可能是中转/错误 id（luckpt 实测回包 id=10230，
/// 真实种子 61140）。按发布标题站内检索 + 逐条核对详情页标题定位真实链接。
fn relocate_posted_detail(tgt: &NexusPhp, name: &str, log: &mut dyn FnMut(&str)) -> Option<String> {
    let q = url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>();
    let base = tgt.base.as_str().trim_end_matches('/');
    let url = format!("{base}/torrents.php?search={q}&search_area=0&incldead=1");
    let html = match tgt.fetch_html(&url, tgt.base.as_str()) {
        Ok(h) => h,
        Err(e) => {
            log(&format!("发后检索失败：{e:#}"));
            return None;
        }
    };
    let mut ids: Vec<String> = Vec::new();
    let mut rest = html.as_str();
    while let Some(pos) = rest.find("details.php?id=") {
        let after = &rest[pos + "details.php?id=".len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && !ids.contains(&digits) {
            ids.push(digits);
        }
        rest = after;
    }
    ids.truncate(8);
    log(&format!("发后检索到 {} 个候选详情页", ids.len()));
    let want = norm_post_title(name);
    for id in ids {
        let u = format!("{base}/details.php?id={id}");
        if let Ok(page) = tgt.fetch_torrent_page(&u) {
            if norm_post_title(&page.name) == want {
                return Some(u);
            }
        }
    }
    None
}

pub fn verify_posted(target: &NexusPhp, detail_url: &str, info: &ReleaseInfo) -> Vec<String> {
    match target.fetch_html(detail_url, target.base.as_str()) {
        Ok(html) => verify_posted_html(&html, info),
        Err(e) => vec![format!("抓新种详情页失败（{e:#}），请手动核对发布结果")],
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

/// 源站解析 + 预检（`review_reseed` 与 `run_reseed` 共用）。
/// 预检不通过不拦在这里，由调用方决定（审阅要出报告，正式转种要拦）。
struct Released {
    src: NexusPhp,
    tgt: NexusPhp,
    page: TorrentPage,
    info: ReleaseInfo,
    preflight: Preflight,
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
    log(&format!(
        "发布要素：标题「{}」｜副标题「{}」｜MediaInfo {} 字节｜IMDb {:?}｜豆瓣 {:?}",
        info.name,
        info.subtitle,
        info.mediainfo.len(),
        info.imdb,
        info.douban
    ));

    // ---- 1.5 源站没有 MediaInfo 时，从本地文件生成（luckpt 等站发种硬性要求）----
    if info.mediainfo.trim().is_empty() {
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
    let pf = preflight(&mut info, &tags);
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
pub fn run_reseed(
    cfg: &Config,
    qb: &QBittorrent,
    req: &ReseedRequest,
    index: &mut ContentIndex,
    log: &mut dyn FnMut(&str),
) -> Result<String> {
    // 真实发种总闸：缺省关闭，任何调用方（CLI --yes / autopost / GUI）都过这里。
    crate::config::ensure_publish_enabled(cfg)?;
    let rel = prepare_released(cfg, req, log)?;
    if !rel.preflight.ok() {
        for e in &rel.preflight.errors {
            log(&format!("预检·阻断：{e}"));
        }
        bail!("预检未通过（{}）：补齐后重试", rel.preflight.summary());
    }
    let Released {
        src,
        tgt,
        page,
        info,
        ..
    } = rel;

    // ---- 3 构造上传字段 + 站点官方预审（LuckAudit）----
    let mut prep = prepare_upload(&tgt, &info)?;
    if let Some(rules) = cfg.coverage.reseed_rules.get(&req.target_site) {
        apply_site_rules(&mut prep, rules);
    }
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

    // ---- 4 下载源种 + 上传目标站 ----
    let torrent_data = src.download_torrent(&page, &req.source_url)?;
    log(&format!(
        "源种已下载（{} bytes），提交到 {} 上传页…",
        torrent_data.len(),
        req.target_site
    ));
    let filename = format!("{}.torrent", info.name);
    let up = submit_upload(&tgt, &prep, &torrent_data, &filename)?;
    log(&format!("上传结果：{}", up.message));

    let detail_url = match up.detail_url {
        Some(u) => u,
        None => {
            if up.duplicate {
                // 已存在但没解析出链接：记覆盖，不再推种
                write_cell(
                    index,
                    &req.content_key,
                    &req.target_site,
                    CoverageStatus::Assisted,
                    None,
                    &format!("上传时站点提示已存在（{}）", up.message),
                );
                index.save()?;
                return Ok(String::new());
            }
            bail!("上传成功但没拿到详情页链接，请手动到目标站确认后把链接补进格子");
        }
    };
    log(&format!("新种详情页：{detail_url}"));

    // ---- 发后自检：回读新种页核对标题/MediaInfo/链接（只警告不推翻）----
    // 站点回包给的详情页可能是中转/错误 id；先按标题校正，再做自检与回推
    let detail_url = match tgt.fetch_torrent_page(&detail_url) {
        Ok(_) => detail_url,
        Err(e) => {
            log(&format!(
                "回包详情页打不开（{e:#}），按标题检索校正真实详情页…"
            ));
            match relocate_posted_detail(&tgt, &info.name, log) {
                Some(u) => {
                    log(&format!("发后校正：{u}"));
                    u
                }
                None => detail_url,
            }
        }
    };
    for w in verify_posted(&tgt, &detail_url, &info) {
        log(&format!("发后自检·{w}"));
    }

    // ---- 4 回推下载器并校验 ----
    // 发布本身已成功；若站点随即删除/转待审导致详情页回读不到，不能再把整轮判失败
    // （luckpt 实测：上传回包给的中转 id 不是真实种子 id；已先按标题校正）。
    let tpage = match tgt.fetch_torrent_page(&detail_url) {
        Ok(p) => p,
        Err(e) => {
            log(&format!(
                "发后回读详情页失败（{e:#}）——发布已成功，格子记 Reseeded 并提示人工确认（不重发）"
            ));
            write_cell(
                index,
                &req.content_key,
                &req.target_site,
                CoverageStatus::Reseeded,
                Some(detail_url.clone()),
                "发布成功，但回包详情页与站内检索都定位失败（回包 id 可能有误），请人工确认；勿重发",
            );
            index.save()?;
            return Ok(detail_url);
        }
    };
    let tdata = tgt.download_torrent(&tpage, &detail_url)?;
    let (rep_hash, meta_save_path, category) = {
        let entry = index
            .contents
            .get(&req.content_key)
            .ok_or_else(|| anyhow!("索引里找不到内容 {}", req.content_key))?;
        let (h, m) = entry
            .pick_meta()
            .ok_or_else(|| anyhow!("内容 {} 没有成员种子，不知道保存路径", req.content_key))?;
        (h.clone(), m.save_path.clone(), m.category.clone())
    };
    let limit = cfg
        .downloader
        .site_up_limits
        .get(&req.target_site)
        .copied()
        .unwrap_or(cfg.downloader.default_up_limit);
    // 落盘布局：按成员种子实时 savepath；根名不同的同内容新种用 NoSubfolder 复用既有目录
    let layout =
        crate::layout::resolve_for_torrent(qb, &rep_hash, &tdata, &info.name, &meta_save_path);
    if let crate::layout::AddLayout::Mismatch(why) = &layout {
        bail!("新种布局对不上，不推下载器：{why}");
    }
    let save_path = layout.save_path().unwrap_or(&meta_save_path).to_string();
    let fname = format!("{}.torrent", info.name);
    let add = qb.add_torrent(
        &tdata,
        &fname,
        &save_path,
        category.as_deref(),
        false,
        limit,
        true,
        layout.content_layout(),
    )?;
    if !add.note.is_empty() {
        log(&format!("下载器回执：{}", add.note));
        write_cell(
            index,
            &req.content_key,
            &req.target_site,
            CoverageStatus::Assisted,
            Some(detail_url.clone()),
            &format!("已加入下载器（{}）", add.note),
        );
        index.save()?;
        return Ok(detail_url);
    }
    let site_hash = if add.id.is_empty() {
        bencode::info_hash_v1(&tdata).unwrap_or_default()
    } else {
        add.id
    };
    let _ = qb.set_paused(&[&site_hash], false);
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
                    Some(detail_url.clone()),
                    "转种成功：校验到 100%，已在做种",
                );
                index.save()?;
                return Ok(detail_url);
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
                    Some(detail_url.clone()),
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
        Some(detail_url.clone()),
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
    Ok(detail_url)
}

fn write_cell(
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

fn adapter_for(cfg: &Config, site_id: &str) -> Result<NexusPhp> {
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
    Ok(a.with_announce_hosts(s.announce_hosts.clone()))
}

/// CLI/调试用：把上传字段清单展开成人能读的行
pub fn dump_fields(fields: &[Field]) -> String {
    let mut out = String::new();
    for (n, v) in fields {
        let flat = v.replace('\n', "\\n");
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
        assert_eq!(info.name, "The.Disguiser.Complete.HDTV.1080p.AAC.x264-JING");
        assert_eq!(info.subtitle, "伪装者 全41集");
        assert!(info.mediainfo.contains("Unique ID"));
        assert_eq!(info.imdb.as_deref(), Some("tt4823720"));
        assert_eq!(info.douban.as_deref(), Some("26302614"));
    }

    #[test]
    fn preflight_blocks_empty_mediainfo() {
        let mut info = ReleaseInfo {
            name: "x".into(),
            descr: "IMDb: https://www.imdb.com/title/tt1/".into(),
            mediainfo: String::new(),
            ..Default::default()
        };
        let pf = preflight(&mut info, &[]);
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
        let pf = preflight(&mut info, &tags);
        assert!(pf.ok(), "{:?}", pf.errors);
        assert!(info.descr.contains("imdb.com"));
        assert!(!pf.fixes.is_empty());
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
        // 内容标签保留
        assert_eq!(strip_promo_suffix("Movie.2024 [中字]"), "Movie.2024 [中字]");
        assert_eq!(
            strip_promo_suffix("Movie.2024 [中字] [50%]"),
            "Movie.2024 [中字]"
        );
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
}
