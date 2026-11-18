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

/// 解析上传表单页：hidden 透传 + select 默认值（模拟浏览器首轮提交）
pub fn parse_form_fields(page: &str) -> Vec<Field> {
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

/// 业务字段：在 hidden/select 默认值之上覆盖标题/副标题/简介/MediaInfo/链接/标签。
/// 返回实际勾选的标签值（预检要核对中字）。
pub fn build_upload_fields(
    info: &ReleaseInfo,
    page: &str,
    tags_wanted: &[&str],
) -> (Vec<Field>, Vec<String>) {
    let mut fields = parse_form_fields(page);

    // 标题 / 副标题
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
    if !info.subtitle.is_empty() && form_has(page, "small_descr") {
        set_field(&mut fields, "small_descr", info.subtitle.clone());
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
    if form_has(page, "descr") {
        set_field(&mut fields, "descr", descr);
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
    (fields, picked)
}

/// 覆盖式设置字段：同名先删再推
fn set_field(fields: &mut Vec<Field>, name: &str, value: String) {
    fields.retain(|(n, _)| n != name);
    fields.push((name.to_string(), value));
}

fn form_has(page: &str, name: &str) -> bool {
    page.contains(&format!("name=\"{name}\"")) || page.contains(&format!("name='{name}'"))
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
            // 文案 = 标签后到下一个 < 之前
            let after = &page[end..(end + 80).min(page.len())];
            let label = strip_tags(after).to_lowercase();
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
    /// 表单页原文（官方预审要从中取 LUCK_AUDIT_USER / 选项文案）
    pub form_html: String,
}

pub fn prepare_upload(target: &NexusPhp, info: &ReleaseInfo) -> Result<PreparedUpload> {
    let form_url = resolve_url("upload.php", &target.base);
    let form_html = target
        .fetch_html(&form_url, target.base.as_str())
        .context("抓目标站上传表单失败")?;
    if !form_html.to_lowercase().contains("<form") {
        bail!("目标站上传页没有表单（cookie 失效或站点拒绝发种），先检测该站登录态");
    }
    if let Some(notice) = crate::sites::nexusphp::block_notice(&form_html) {
        bail!("目标站限流（{notice}）：{form_url}");
    }

    let tags_wanted = if info.has_chinese_subs() {
        vec!["中字"]
    } else {
        vec![]
    };
    let (fields, picked_tags) = build_upload_fields(info, &form_html, &tags_wanted);
    // 想勾的标签一个都没匹配上才警告（复选框 value 多是数字编码，
    // 拿 value 里含不含关键词来判会把勾上了的误报成漏勾）
    let tag_warn = if !tags_wanted.is_empty() && picked_tags.is_empty() {
        Some(format!(
            "[{}] 表单里没找到「中字」类标签复选框，请发布后手动补",
            target.site_id
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
    let prepared = prepare_upload(&rel.tgt, &rel.info)?;
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

/// `<select name=field>` 里指定 value 的选项文案与 class-name
fn select_option_meta(page: &str, field: &str, value: &str) -> Option<(String, String)> {
    let block = select_block(page, field)?;
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
    None
}

/// `<select name=field>` 的 data-mode 属性（分类联动模式）
fn select_mode(page: &str, field: &str) -> Option<String> {
    let block = select_block(page, field)?;
    let end = block.find('>')? + 1;
    attr(&block[..end], "data-mode")
}

/// 取 `<select name=field>` 整块（含开闭标签内文）
fn select_block<'a>(page: &'a str, field: &str) -> Option<&'a str> {
    let lower = page.to_ascii_lowercase();
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
            return Some(&page[start..close]);
        }
        i = end;
    }
    None
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
            let after = &page[end..(end + 80).min(page.len())];
            // 先取到下一个标签为止的纯文本；input 后直接跟标签（<label>…）再退窗口
            let cut = after.find('<').unwrap_or(after.len());
            let inline = strip_tags(&after[..cut]).trim().to_string();
            if !inline.is_empty() {
                return inline;
            }
            return strip_tags(after).trim().to_string();
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

    // ---- 2 LuckAudit 预检（三查）----
    let tags: Vec<String> = if info.has_chinese_subs() {
        vec!["中字".into()]
    } else {
        vec![]
    };
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
    let prep = prepare_upload(&tgt, &info)?;
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
    for w in verify_posted(&tgt, &detail_url, &info) {
        log(&format!("发后自检·{w}"));
    }

    // ---- 4 回推下载器并校验 ----
    let tpage = tgt.fetch_torrent_page(&detail_url)?;
    let tdata = tgt.download_torrent(&tpage, &detail_url)?;
    let entry = index
        .contents
        .get(&req.content_key)
        .ok_or_else(|| anyhow!("索引里找不到内容 {}", req.content_key))?;
    let save_path = entry
        .pick_meta()
        .map(|(_, m)| m.save_path.clone())
        .ok_or_else(|| anyhow!("内容 {} 没有成员种子，不知道保存路径", req.content_key))?;
    let category = entry.pick_meta().and_then(|(_, m)| m.category.clone());
    let limit = cfg
        .downloader
        .site_up_limits
        .get(&req.target_site)
        .copied()
        .unwrap_or(cfg.downloader.default_up_limit);
    let fname = format!("{}.torrent", info.name);
    let add = qb.add_torrent(
        &tdata,
        &fname,
        &save_path,
        category.as_deref(),
        false,
        limit,
        true,
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
        let (fs, picked) = build_upload_fields(
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
}
