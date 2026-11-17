//! 转种上传（Rust 原生全套，不再交接 GUI 版）。
//!
//! 流水线：源站详情页 → 发布要素（标题/副标题/简介/MediaInfo/链接）→ LuckAudit 三查
//! （MediaInfo 非空、简介带 IMDb/豆瓣/TMDB、中字标签）→ 目标站 upload.php 表单提交
//! → 新种详情页回推下载器 → 校验 → 格子落 Reseeded。
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
    let mut info = ReleaseInfo {
        name: first_tag_text(html, "h1").unwrap_or_else(|| name_fallback.to_string()),
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
        info.descr = html.to_string();
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
    let key = format!("name=\"{name}\"");
    let p = html.find(&key)?;
    // 从该位置向前后找 value="…"
    let seg = &html[p..html.len().min(p + 600)];
    let vkey = "value=\"";
    let vp = seg.find(vkey)?;
    let rest = &seg[vp + vkey.len()..];
    let end = rest.find('"')?;
    Some(decode_entities(&rest[..end]))
}

/// 配对 div 内容（支持一层嵌套）。字节扫描 + ASCII 大小写不敏感比较：
/// 索引始终落在字符边界上（UTF-8 多字节序列不含 ASCII 字节，逐字节安全）
fn div_inner(html: &str, id: &str) -> Option<String> {
    let key = format!("id=\"{id}\"");
    let p = html.find(&key)?;
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
    // select 默认值（selected 项，无则首项）
    let mut rest = page;
    while let Some(p) = rest.to_ascii_lowercase().find("<select") {
        let after = &rest[p..];
        let close = after
            .to_ascii_lowercase()
            .find("</select>")
            .unwrap_or(after.len());
        let block = &after[..close.min(after.len())];
        if let Some(name) = attr(block, "name") {
            if !name.contains('[') && !fields.iter().any(|(n, _)| *n == name) {
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

/// 提交到目标站 upload.php。multipart 一次提交 + 「已丢失 POST」重投一次
/// （Swift 版实测吐鲁番连续提交会偶发丢 POST，同字节重投即可）。
pub fn upload_release(
    target: &NexusPhp,
    info: &ReleaseInfo,
    torrent_data: &[u8],
    filename: &str,
) -> Result<UploadResult> {
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
    // 表单里找不到「中字」复选框：只警告不阻断（有的站把标签放发布后编辑页）
    let tag_warn = if tags_wanted
        .iter()
        .any(|t| !picked_tags.iter().any(|p| p.contains(t)))
    {
        Some(format!(
            "[{}] 表单里没找到「中字」类标签复选框，请发布后手动补",
            target.site_id
        ))
    } else {
        None
    };

    let action = form_action(&form_html, &target.base).unwrap_or_else(|| form_url.clone());
    // 成功消息末尾追加标签警告（若有）
    let note_suffix = tag_warn
        .as_deref()
        .map(|w| format!("；{w}"))
        .unwrap_or_default();
    let mut last_err = String::new();
    for attempt in 0..2 {
        let resp = target
            .post_upload(&action, &fields, "file", filename, torrent_data, &form_url)
            .with_context(|| format!("提交上传失败（第 {} 次）", attempt + 1))?;
        let body = resp.body;
        // 「已存在」先判：站点常跳回已有种子详情页，先判成功会误报转种成功
        if has_exist_marker(&body) {
            let dup_url = sole_detail_url(&body, &resp.final_url, &action);
            return Ok(UploadResult {
                detail_url: dup_url,
                message: format!("站点提示种子已存在（视为已覆盖，不算本次转种）{note_suffix}"),
                duplicate: true,
            });
        }
        // 成功：跳到详情页 / 发布成功提示
        if let Some(u) = redirect_detail_url(&resp.final_url, &action) {
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
            let u = sole_detail_url(&body, &resp.final_url, &action);
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

// ---------------- 转种流水线 ----------------

pub struct ReseedRequest {
    pub content_key: String,
    pub source_site: String,
    pub source_url: String,
    pub target_site: String,
    pub name: String,
}

/// 转种一条龙：解析源页 → 预检 → 上传 → 推种校验 → 格子落账。
/// `log` 收进度行；返回新种详情页链接。
pub fn run_reseed(
    cfg: &Config,
    qb: &QBittorrent,
    req: &ReseedRequest,
    index: &mut ContentIndex,
    log: &mut dyn FnMut(&str),
) -> Result<String> {
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
    if !pf.ok() {
        for e in &pf.errors {
            log(&format!("预检·阻断：{e}"));
        }
        bail!("预检未通过（{}）：补齐后重试", pf.summary());
    }
    log(&format!("预检通过（{}）", pf.summary()));

    // ---- 3 下载源种 + 上传目标站 ----
    let torrent_data = src.download_torrent(&page, &req.source_url)?;
    log(&format!(
        "源种已下载（{} bytes），提交到 {} 上传页…",
        torrent_data.len(),
        req.target_site
    ));
    let filename = format!("{}.torrent", info.name);
    let up = upload_release(&tgt, &info, &torrent_data, &filename)?;
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
}
