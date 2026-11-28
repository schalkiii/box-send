//! 上传表单解析、字段构造、分类/图片/区域匹配与站点规则落地。
//!
//! 从 `reseed.rs` 拆出；所有函数仍在父模块的命名空间里复用同一套类型。

use super::*;

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

pub(super) fn select_default(block: &str) -> String {
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

pub(super) fn attr(tag: &str, name: &str) -> Option<String> {
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
    // 标题按目标站规范改写后再参与选档；否则 qingwa 的 `Blu-ray` 会被站点
    // 审核要求改成 `BluRay`，而且改写后仍要用同一份标题匹配下拉/标签。
    let mut site_info = info.clone();
    site_info.name = title_for_site(info, site_id);
    let (mut fields, picked, mut notes) = build_upload_fields(&site_info, page, tags_wanted);
    if site_id == "qingwa" && is_bdinfo_block(&site_info.mediainfo) {
        // 青蛙站规：蓝光原盘/DIY 交 BDInfo 时，音频下拉要选 Other；具体音轨信息让审核看 BDInfo。
        if let Some((field, label)) =
            force_select_label(&mut fields, page, "audiocodec_sel", &["Other", "其他"])
        {
            notes.push(format!("{field}：按青蛙蓝光规则选「{label}」"));
        }
    }
    if site_id == "ubits" {
        fill_ubits_quality_fields(&mut fields, &site_info, page);
    }
    if site_id == "lajidui" {
        fill_lajidui_quality_fields(&mut fields, &site_info, page);
    }
    if site_id == "ptzone" {
        fill_ptzone_quality_fields(&mut fields, page);
    }
    (fields, picked, notes)
}

/// 目标站实际提交的标题：先按站点写法规整，再按站点习惯补年份。
/// 审阅/门禁也用这一个函数，避免“表单已改写、门禁还拿旧标题比对”。
pub(super) fn title_for_site(info: &ReleaseInfo, site_id: &str) -> String {
    let mut site_info = info.clone();
    site_info.name = normalize_title_for_site(&site_info.name, site_id);
    if site_id == "ptzone" {
        site_info.name = add_ptzone_release_year(&site_info);
    }
    site_info.name
}

/// ptzone 的标题习惯带发行年份；源标题没有年份时，从副标题/简介/MediaInfo
/// 抽一个四位年份，插在第一个技术参数前。抽不到就保持原样，不硬编年份。
fn add_ptzone_release_year(info: &ReleaseInfo) -> String {
    let name = info.name.trim();
    if name.is_empty() || name.contains('[') || has_release_year(name) {
        return name.to_string();
    }
    let Some(year) = release_year(info) else {
        return name.to_string();
    };
    let sep = if name.contains(' ') {
        ' '
    } else if name.contains('.') {
        '.'
    } else {
        return name.to_string();
    };
    let mut parts: Vec<String> = name
        .split(sep)
        .filter(|t| !t.is_empty())
        .map(ToString::to_string)
        .collect();
    let Some(at) = parts.iter().position(|t| super::tech_rank(t).is_some()) else {
        return name.to_string();
    };
    parts.insert(at, year.to_string());
    parts.join(&sep.to_string())
}

/// ptzone 的 `team_sel` 只有少数驻站组；源组不在列表里时不能留“请选择一项”，
/// 按站内选项选 Other，避免冒充驻站组。
pub(super) fn fill_ptzone_quality_fields(fields: &mut Vec<Field>, page: &str) {
    if let Some(name) = select_field_names(fields, "team_sel").into_iter().next() {
        if field_lookup(fields, &name).is_some_and(|v| v.is_empty() || v == "0") {
            fill_select_by_label(fields, page, "team_sel", "Other");
        }
    }
}

/// UBits 的 `team_sel`（制作组）没有默认值：源组不是 UBits 自家组，按 Other 提交，
/// 避免冒充驻站组。`source_sel`（地区）由通用地区匹配回填。
pub(super) fn fill_ubits_quality_fields(fields: &mut Vec<Field>, _info: &ReleaseInfo, _page: &str) {
    if fields.iter().any(|(n, _)| n == "team_sel[4]") {
        set_field(fields, "team_sel[4]", "5".to_string());
    }
}

/// lajidui 的 `processing_sel` 是文件容器（MP4/MKV），`team_sel` 配不上落 Other；
/// `source_sel`（地区）由通用地区匹配回填。
/// 2026-11-26 两条误发种子（85084/85085）都卡在这三项。
pub(super) fn fill_lajidui_quality_fields(fields: &mut Vec<Field>, info: &ReleaseInfo, page: &str) {
    let container_hay = format!("{} {}", info.name, info.mediainfo).to_ascii_lowercase();
    let container = if container_hay.contains(".mp4") || container_hay.contains("mpeg-4") {
        Some("MP4")
    } else if container_hay.contains(".mkv") || container_hay.contains("matroska") {
        Some("MKV")
    } else {
        None
    };
    if let Some(container) = container {
        fill_select_by_label(fields, page, "processing_sel", container);
    }

    let matched = release_group(&info.name)
        .map(|group| fill_select_by_label(fields, page, "team_sel", &group))
        .unwrap_or(false);
    if !matched {
        // 站点没有这个组时不能冒充驻站组；有 Other 就选 Other。
        fill_select_by_label(fields, page, "team_sel", "Other");
    }
}

/// 只填当前为空/0 的同前缀下拉；选项文案必须真实存在。
pub(super) fn fill_select_by_label(
    fields: &mut Vec<Field>,
    page: &str,
    prefix: &str,
    label: &str,
) -> bool {
    let Some(name) = select_field_names(fields, prefix).into_iter().next() else {
        return false;
    };
    let cur = field_lookup(fields, &name).unwrap_or("").to_string();
    if !cur.is_empty() && cur != "0" {
        return true;
    }
    let options = select_options(page, &name);
    if let Some(value) = match_option(&options, label) {
        set_field(fields, &name, value);
        return true;
    }
    false
}

/// 剧集类型/地区归类线索（标题/简介命中即归类；归不出走兜底「其他」）。
pub(super) const SERIES_REGIONS: &[&str] =
    &["大陆", "港台", "美剧", "日剧", "韩剧", "英剧", "泰剧"];

/// 按站点规则补分类下 JS 异步注入的附加必填字段。
///
/// 静态上传页里没有这些控件（页面 JS 再从分类片段接口拉）；规则由
/// [`SiteRules.category_rules`](crate::gates::SiteRules) 声明。能拉到分类片段时，
/// 以片段里的真实控件名为准；派生不出中文名/英文名就记缺口落人工，不发空值。
pub fn prepare_category_fields(
    target: &NexusPhp,
    prep: &mut PreparedUpload,
    info: &ReleaseInfo,
    rules: &crate::gates::SiteRules,
) -> Result<()> {
    let Some(type_val) = field_lookup(&prep.fields, "type") else {
        return Ok(());
    };
    let Some(rule) = rules.category_rules.iter().find(|r| r.category == type_val) else {
        return Ok(());
    };
    // 拉分类控件定义片段；拉不到就不猜字段名，记缺口落人工。
    let fragment: Option<String> = match &rule.detail_endpoint {
        Some(template) => {
            let rel = template.replace("{catid}", type_val);
            let url = resolve_url(&rel, &target.base);
            match target.fetch_html(&url, &prep.form_url) {
                Ok(html) => Some(html),
                Err(e) => {
                    prep.required_gaps.push(format!(
                        "必填缺失：分类 {type_val} 的附加字段定义片段拉取失败（{e:#}），落人工核对"
                    ));
                    return Ok(());
                }
            }
        }
        None => None,
    };
    let controls_html = fragment.as_deref().unwrap_or(&prep.form_html);
    apply_category_rule_fields(
        &mut prep.fields,
        &mut prep.required_gaps,
        info,
        controls_html,
        rule,
    );
    Ok(())
}

/// 纯逻辑部分：按规则把派生值补进字段；控件不在片段/页面里、或派生不出值就记缺口。
pub(super) fn apply_category_rule_fields(
    fields: &mut Vec<Field>,
    gaps: &mut Vec<String>,
    info: &ReleaseInfo,
    controls_html: &str,
    rule: &crate::gates::CategoryRule,
) {
    let hay = format!("{} {} {}", info.subtitle, info.descr, info.name);
    for f in &rule.fields {
        // 只认真实控件；片段/静态页里都没有这个字段名就不臆造。
        if !form_has(controls_html, &f.field) {
            gaps.push(format!(
                "必填缺失：{}（分类 {} 的异步控件里找不到该字段）",
                f.field, rule.category
            ));
            continue;
        }
        let cur = field_lookup(fields, &f.field)
            .unwrap_or("")
            .trim()
            .to_string();
        match f.kind {
            crate::gates::CategoryFieldKind::ChineseName => {
                if !cur.is_empty() {
                    continue;
                }
                let cname = first_bracket_name(&info.subtitle).or_else(|| {
                    info.descr
                        .split(['●', '◎', '\n'])
                        .find(|s| s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                });
                match cname {
                    Some(v) => set_field(fields, &f.field, v),
                    None => gaps.push(format!(
                        "必填缺失：{}（派生不出中文名，落人工核对）",
                        f.field
                    )),
                }
            }
            crate::gates::CategoryFieldKind::EnglishName => {
                if !cur.is_empty() {
                    continue;
                }
                let ename = info.name.trim().to_string();
                if ename.is_empty() {
                    gaps.push(format!(
                        "必填缺失：{}（主标题为空，派生不出英文名，落人工核对）",
                        f.field
                    ));
                } else {
                    set_field(fields, &f.field, ename);
                }
            }
            crate::gates::CategoryFieldKind::SpecificCat => {
                if !cur.is_empty() {
                    continue;
                }
                let region = SERIES_REGIONS.iter().find(|r| hay.contains(**r));
                let value = region
                    .map(|r| r.to_string())
                    .or_else(|| f.default.clone())
                    .unwrap_or_else(|| "其他".to_string());
                set_field(fields, &f.field, value);
            }
            crate::gates::CategoryFieldKind::Checkbox => {
                if fields.iter().any(|(n, _)| n == &f.field) {
                    continue;
                }
                let value = f.default.clone().unwrap_or_else(|| f.field.clone());
                fields.push((f.field.clone(), value));
            }
        }
    }
}

pub(super) fn first_bracket_name(s: &str) -> Option<String> {
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
pub(super) fn set_field(fields: &mut Vec<Field>, name: &str, value: String) {
    fields.retain(|(n, _)| n != name);
    fields.push((name.to_string(), value));
}

/// 表单控件里有没有这个名字的字段（input/textarea/select 的 name 属性）。
/// 只认真控件：`<meta name="description">` 与 JS 里的同名字符串不算（hdcity 实测踩过）。
pub(super) fn form_has(page: &str, name: &str) -> bool {
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

/// 从简介里按出现顺序抽图片 URL：HTML `<img src>` 与 BBCode `[img]…[/img]` 都认。
/// 只接受绝对 http(s) 地址；重复图只留第一处，避免海报和截图字段重复提交。
pub(crate) fn image_urls_from_descr(descr: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    let mut push = |u: &str| {
        let u = u.trim();
        if (u.starts_with("http://") || u.starts_with("https://")) && !urls.iter().any(|x| x == u) {
            urls.push(u.to_string());
        }
    };
    let lower = descr.to_ascii_lowercase();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<img") {
        let start = i + p;
        let end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(descr.len());
        if let Some(src) = attr(&descr[start..end], "src") {
            push(&src);
        }
        i = end;
    }
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find("[img]") {
        let content_start = pos + rel + "[img]".len();
        let Some(end) = lower[content_start..].find("[/img]") else {
            break;
        };
        push(&descr[content_start..content_start + end]);
        pos = content_start + end + "[/img]".len();
    }
    urls
}

/// 上传页把海报/截图放在独立字段时，从简介图片里抽出来填入。
/// 只填页面真实存在的字段；配置里的旧字段名（站点改版后已消失）不会硬塞。
pub(super) fn fill_image_fields(
    fields: &mut Vec<Field>,
    page: &str,
    descr: &str,
    poster_field: Option<&str>,
    screenshot_field: Option<&str>,
) {
    let urls = image_urls_from_descr(descr);
    if urls.is_empty() {
        if let Some(name) = poster_field {
            if form_has(page, name) {
                fields.retain(|(n, _)| n != name);
            }
        }
        if let Some(name) = screenshot_field {
            if form_has(page, name) {
                fields.retain(|(n, _)| n != name);
            }
        }
        return;
    }
    let mut poster_used = false;
    if let Some(name) = poster_field {
        if form_has(page, name) {
            set_field(fields, name, urls[0].clone());
            poster_used = true;
        }
    }
    if let Some(name) = screenshot_field {
        if form_has(page, name) {
            let shots = if poster_used { &urls[1..] } else { &urls[..] };
            if !shots.is_empty() {
                set_field(fields, name, shots.join("\n"));
            } else {
                fields.retain(|(n, _)| n != name);
            }
        }
    }
}

/// 从构造结果拆出（海报张数，截图张数）：独立字段有值按字段算；没有独立字段时
/// 第一张简介图算海报，其余算截图。门禁按站点底线分别核对。
pub fn image_split_counts(prep: &PreparedUpload) -> (usize, usize) {
    let descr_imgs = image_urls_from_descr(submitted_descr(prep, "")).len();
    let poster = prep
        .poster_field
        .as_deref()
        .and_then(|f| field_lookup(&prep.fields, f))
        .filter(|v| !v.trim().is_empty())
        .map(|_| 1)
        .unwrap_or(usize::from(descr_imgs > 0));
    let shots = prep
        .screenshot_field
        .as_deref()
        .and_then(|f| field_lookup(&prep.fields, f))
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(descr_imgs.saturating_sub(poster));
    (poster, shots)
}

// ---------------- 选档映射（发布名/副标题 → 分类与质量下拉） ----------------

/// 媒介词元（最长优先；重叠的短词自动跳过：WEB-DL 吃掉 WEB）
pub(super) const MEDIUM_KEYWORDS: &[&str] = &[
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

pub(super) const CODEC_KEYWORDS: &[&str] = &[
    "H.265", "H.264", "H265", "H264", "x265", "x264", "HEVC", "AVC", "XviD", "DivX", "AV1", "VC-1",
    "VC1", "MPEG-2", "MPEG2", "MPEG4", "VP9",
];

pub(super) const AUDIO_KEYWORDS: &[&str] = &[
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

pub(super) const STANDARD_KEYWORDS: &[&str] = &[
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

pub(super) const PROCESSING_KEYWORDS: &[&str] =
    &["REMUX", "REPACK", "PROPER", "INTERNAL", "UNCUT", "RESTORED"];

/// 地区下拉：标题/简介里的地区线索词 → 选项标签同义词。
/// 顺序即归类优先级，先命中先归；归不出再落 Other/其它。
pub(super) const REGION_GROUPS: &[(&[&str], &[&str])] = &[
    (
        &["国产", "大陆", "内地", "中国大陆"],
        &["大陆", "内地", "中国大陆", "国产", "chn"],
    ),
    (&["香港", "港剧", "港片"], &["香港", "hkg", "hk"]),
    (&["台湾", "台剧", "台片"], &["台湾", "twn", "tw"]),
    (&["日本", "日剧", "日影", "日漫"], &["日本", "jpn"]),
    (&["韩国", "韩剧", "韩影"], &["韩国", "kor"]),
    (
        &[
            "欧美",
            "美国",
            "英国",
            "美剧",
            "英剧",
            "法国",
            "德国",
            "加拿大",
            "澳大利亚",
        ],
        &["欧美", "美国", "英国", "west", "usa", "uk"],
    ),
    (&["印度"], &["印度", "ind"]),
    (&["俄国", "俄罗斯"], &["俄国", "俄罗斯", "rus"]),
    (&["泰国", "泰剧"], &["泰国", "tha"]),
];

/// 选项标签属于哪一组地区；不属于任何地区组返回 None。
pub(super) fn label_region_group(label: &str) -> Option<usize> {
    REGION_GROUPS
        .iter()
        .position(|(_, syns)| syns.iter().any(|s| label_relates(label, s)))
}

/// 选项集是不是地区下拉：至少一个选项对得上地区同义词，且没有媒介片源标签
/// （单选项地区下拉也认；片源下拉的 Blu-ray/WEB-DL 一类不误判）。
pub(super) fn options_are_regions(options: &[(String, String)]) -> bool {
    let real: Vec<&(String, String)> = options.iter().filter(|(v, _)| v != "0").collect();
    if real.is_empty() {
        return false;
    }
    let region_n = real
        .iter()
        .filter(|(_, l)| label_region_group(l).is_some())
        .count();
    let medium_n = real
        .iter()
        .filter(|(_, l)| MEDIUM_KEYWORDS.iter().any(|k| label_relates(l, k)))
        .count();
    region_n >= 1 && medium_n == 0
}

/// 从标题/简介线索判定属于哪一组地区；归不出返回 None。
pub(super) fn detect_region_group(hay: &str) -> Option<usize> {
    let h = hay.to_ascii_lowercase();
    REGION_GROUPS
        .iter()
        .position(|(clues, _)| clues.iter().any(|c| h.contains(&c.to_ascii_lowercase())))
}

/// 地区下拉配对：先按线索找同组选项；找不到同组就落 Other/其它；都没有留空。
pub(super) fn match_region_option(
    options: &[(String, String)],
    hay: &str,
) -> Option<(String, String)> {
    if let Some(gi) = detect_region_group(hay) {
        let syns = REGION_GROUPS[gi].1;
        if let Some(hit) = options
            .iter()
            .find(|(_, l)| syns.iter().any(|s| label_relates(l, s)))
        {
            return Some(hit.clone());
        }
    }
    options
        .iter()
        .find(|(_, l)| matches!(norm_token(l).as_str(), "other" | "其它" | "其他"))
        .cloned()
}

/// 年代档选项标签：四位年份（19xx/20xx）或「xx 年代」一类。
pub(super) fn label_is_year_option(label: &str) -> bool {
    let t = label.trim();
    if t.contains("年代") {
        return true;
    }
    t.len() == 4
        && t.chars().all(|c| c.is_ascii_digit())
        && (t.starts_with("19") || t.starts_with("20"))
}

/// 选项集是不是发行年代下拉：至少两个真实选项是年份/年代档。
pub(super) fn options_are_years(options: &[(String, String)]) -> bool {
    let real: Vec<&(String, String)> = options.iter().filter(|(v, _)| v != "0").collect();
    real.len() >= 2 && real.iter().filter(|(_, l)| label_is_year_option(l)).count() >= 2
}

/// 从一段文本里抽四位年份（19xx/20xx）；手动扫 ASCII 数字，不引正则。
pub(super) fn extract_year(text: &str) -> Option<u16> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            let digits = &text[i..i + 4];
            if digits.starts_with("19") || digits.starts_with("20") {
                return digits.parse().ok();
            }
        }
        i += 1;
    }
    None
}

/// 发行年份：按标题 → 副标题 → 简介 → MediaInfo 顺序抽，先命中为准。
pub(super) fn release_year(info: &ReleaseInfo) -> Option<u16> {
    [&info.name, &info.subtitle, &info.descr, &info.mediainfo]
        .into_iter()
        .find_map(|hay| extract_year(hay))
}

/// 发布名里的词元：最长优先贪心去重叠，带词边界（大小写不敏感）
pub(super) fn find_keywords(hay: &str, keywords: &[&str]) -> Vec<String> {
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
pub(super) fn norm_token(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// 词元 → 选项的配对结论
pub(super) enum Pairing {
    /// 唯一命中：选项 value
    Hit(String),
    /// 多个候选（含同名重复项）：不猜
    Ambiguous,
    /// 与选项无关
    Miss,
}

/// 在选项里找词元：规范化全等优先，其次标签里的分隔子串；命中多个不猜
pub(super) fn pair_option(options: &[(String, String)], token: &str) -> Pairing {
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
            // 同名重复项按文档顺序取首个（muxuege 实测：普通 AAC 值 6 在普通编码组，
            // 同名 AAC 值 14 在全景声同组；靠前的普通组才是无全景声音轨该选的档）。
            return Pairing::Hit(exact[0].0.clone());
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

pub(super) fn match_option(options: &[(String, String)], token: &str) -> Option<String> {
    match pair_option(options, token) {
        Pairing::Hit(v) => Some(v),
        _ => None,
    }
}

/// 重编码片的媒介档：优先英文「Encode」，没有就认中文「压制 / 重编码」。
/// hdfans 的选项是「1080P/i压制」「720P压制」，还同时有两档；必须按标题分辨率
/// 选对应档，不能看到「压制」就随便挑一个。
pub(super) fn pick_encode_option(options: &[(String, String)], hay: &str) -> Option<String> {
    if let Some(v) = match_option(options, "Encode") {
        return Some(v);
    }
    let lower = hay.to_ascii_lowercase();
    let resolution = if ["2160", "4k", "uhd"].iter().any(|t| lower.contains(t)) {
        Some("2160")
    } else if ["1080"].iter().any(|t| lower.contains(t)) {
        Some("1080")
    } else if ["720"].iter().any(|t| lower.contains(t)) {
        Some("720")
    } else {
        None
    };
    let candidates: Vec<&(String, String)> = options
        .iter()
        .filter(|(_, label)| {
            let l = label.to_ascii_lowercase();
            l.contains("压制") || l.contains("重编码") || l.contains("重編碼")
        })
        .collect();
    if let Some(res) = resolution {
        if let Some((v, _)) = candidates
            .iter()
            .find(|(_, label)| label.to_ascii_lowercase().contains(res))
        {
            return Some(v.clone());
        }
    }
    if candidates.len() == 1 {
        return Some(candidates[0].0.clone());
    }
    None
}

/// Remux 的介质档要优先命中「Remux」本身，不能先按 Blu-ray/BD 泛匹配。
/// hdfans/kufei 有「BD Remux」，ptzone/hdvideo/city13 只有「Remux」，
/// ptsbao 没有 Remux 档（但 processing/source 已标 Remux）则回退 BluRay。
/// 4K 优先 UHD Remux；站点没有 UHD Remux 时选通用 Remux，避免误选 UHD 原盘。
pub(super) fn pick_remux_option(options: &[(String, String)], hay: &str) -> Option<String> {
    let lower = hay.to_ascii_lowercase();
    let uhd = lower.contains("uhd") || lower.contains("2160") || lower.contains("4k");
    let candidates: &[&str] = if uhd {
        &["UHD Remux", "Remux", "UHD Blu-ray", "UHD BluRay"]
    } else {
        &[
            "BD Remux",
            "Blu-ray Remux",
            "BluRay Remux",
            "Bluray Remux",
            "Remux",
            "BluRay",
            "Blu-ray",
            "BD",
        ]
    };
    for c in candidates {
        if let Some(v) = match_option(options, c) {
            return Some(v);
        }
    }
    None
}

/// 选项标签与词元是否有关系（规范化全等或任一方向的子串）：域判定用，宁可偏宽
pub(super) fn label_relates(label: &str, token: &str) -> bool {
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
pub(super) fn options_in_domain(options: &[(String, String)], kws: &[&str]) -> bool {
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
pub(super) fn looks_like_secondary_category(options: &[(String, String)]) -> bool {
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
pub(super) fn token_aliases(token: &str) -> &'static [&'static str] {
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
        // iloli 的表单把顺序写成 AC3/DD，不能只认 DD/AC3 一种写法。
        "ac3" => &["DD/AC3", "DD/AC-3", "AC3/DD", "AC3"],
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
pub(super) fn compound_medium_token(tokens: &[String]) -> Option<String> {
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
pub(super) fn select_options(page: &str, field: &str) -> Vec<(String, String)> {
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

/// 分类联动隐藏行里的下拉名集合：`<tr class="mode_<分类id>" relation="mode_<分类id>">`
/// 这类行按所选分类由页面 JS 显隐（xingtan 实测 328 行，只有当前分类那行可见）。
/// 静态演练时其余分类行不可见，它们的空下拉不该记成缺口。
pub(super) fn hidden_mode_select_names(page: &str) -> std::collections::HashSet<String> {
    let page = upload_form_block(page).unwrap_or(page);
    let lower = page.to_ascii_lowercase();
    let mut out = std::collections::HashSet::new();
    let mut from = 0;
    while let Some(p) = lower[from..].find("<tr") {
        let start = from + p;
        let tag_end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(page.len());
        let tag = &page[start..tag_end];
        let tag_l = tag.to_ascii_lowercase();
        let is_mode_row = tag_l.contains("class=\"mode_") || tag_l.contains("relation=\"mode_");
        let close = lower[start..]
            .find("</tr>")
            .map(|j| start + j)
            .unwrap_or(page.len());
        if is_mode_row {
            let mut s = tag_end;
            while let Some(q) = lower[s..close].find("<select") {
                let sel_start = s + q;
                let sel_end = lower[sel_start..]
                    .find('>')
                    .map(|j| sel_start + j + 1)
                    .unwrap_or(close);
                if let Some(name) = attr(&page[sel_start..sel_end], "name") {
                    out.insert(name);
                }
                s = sel_end;
            }
        }
        from = close;
    }
    out
}

/// 表单里全部 `<select name=…>` 的名字（演练核对缺口用；只看上传表单块）
pub(super) fn select_names(page: &str) -> Vec<String> {
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
pub(super) fn select_field_names(fields: &[Field], prefix: &str) -> Vec<String> {
    fields
        .iter()
        .map(|(n, _)| n)
        .filter(|n| *n == prefix || (n.starts_with(prefix) && n[prefix.len()..].starts_with('[')))
        .cloned()
        .collect()
}

/// 按页面真实选项文案强制改下拉：用于站点规则明确要求的“Other/其他”。
/// 文案匹配不区分大小写；选项不存在就不写，返回 None（不硬造值）。
pub(super) fn force_select_label(
    fields: &mut Vec<Field>,
    page: &str,
    prefix: &str,
    labels: &[&str],
) -> Option<(String, String)> {
    for name in select_field_names(fields, prefix) {
        for (value, label) in select_options(page, &name) {
            if labels
                .iter()
                .any(|want| label.trim().eq_ignore_ascii_case(want))
            {
                set_field(fields, &name, value.clone());
                return Some((name, label));
            }
        }
    }
    None
}

/// 发布组：名尾 `-GROUP`（`x264-JING` → JING）；常见非组词缀剔除
pub(super) fn release_group(name: &str) -> Option<String> {
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
pub(super) fn marker_contains(hay_lower: &str, marker: &str) -> bool {
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
pub(super) fn has_episode_marker(hay: &str) -> bool {
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
pub(super) fn infer_type_candidates(
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

/// 源站简介里「类型: 喜剧 / 动画 / 短片」这种明确题材行，优先于片名年份推断。
/// 只认动画，避免把简介正文里偶尔出现的「动画」误当分类。
pub(super) fn infer_type_from_descr(
    descr: &str,
) -> Option<(Vec<&'static str>, &'static [&'static str])> {
    const ANIME: &[&str] = &["动画", "动漫", "Anime", "Animation"];
    let plain = strip_html_tags(descr).to_lowercase();
    let start = plain.find("类型")?;
    let tail = &plain[start..];
    let end = ["制片国家", "语言", "上映日期", "片长", "又名", "imdb"]
        .iter()
        .filter_map(|marker| tail.find(marker))
        .min()
        .unwrap_or(tail.len().min(120));
    let genre_line = &tail[..end];
    if ANIME
        .iter()
        .any(|word| genre_line.contains(&word.to_lowercase()))
    {
        Some((vec!["动画", "动漫", "Anime", "Animation"], ANIME))
    } else {
        None
    }
}

/// 发布名里是否带 19xx/20xx 年份（要求两侧非数字）
pub(super) fn has_release_year(h: &str) -> bool {
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
pub(super) fn type_niche_miss(options: &[(String, String)], info: &ReleaseInfo) -> bool {
    let Some((_, syns)) = infer_type_candidates(&info.name, &info.subtitle) else {
        return false;
    };
    !options
        .iter()
        .any(|(_, l)| syns.iter().any(|s| label_relates(l, s)))
}

/// 上传表单是 JS 自定义界面：只有种子文件与隐藏字段，没有可静态回填的文本框
/// （hdcity 实测：name 都是 hidden，简介/MediaInfo 由网页 JS 延后生成）
pub(super) fn js_driven_form(form: &str) -> bool {
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
pub(super) fn mediainfo_dimension(mediainfo: &str, key: &str) -> Option<u32> {
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
pub(super) fn resolution_from_mediainfo(mediainfo: &str) -> Option<&'static str> {
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

pub(super) fn pick_resolution_option(options: &[(String, String)], hay: &str) -> Option<String> {
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
pub(super) fn fill_selects(fields: &mut Vec<Field>, page: &str, info: &ReleaseInfo) -> Vec<String> {
    let mut notes = Vec::new();
    let hay = format!("{} {}", info.name, info.subtitle);
    let audio_hay = format!("{} {}", hay, info.mediainfo);
    // 地区线索常写在副标题/简介里（「国产剧」「中国大陆」），地区下拉单独带简介一起判
    let region_hay = format!("{} {} {}", info.name, info.subtitle, info.descr);
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
            // 地区下拉（iloli/ubits/lajidui 实测）：按标题/简介地区词配对，归不出落 Other。
            if *prefix == "source_sel" && options_are_regions(&options) {
                if let Some((v, _)) = match_region_option(&options, &region_hay) {
                    set_field(fields, fname, v);
                    filled_seen = true;
                }
                continue;
            }
            // 年代档下拉（muxuege 实测 processing_sel 是发行年代 2010=14…2015=19）：
            // 从标题/简介抽年份配对；抽不到年份就留空，不按 REMUX/REPACK 这类处理标记硬填。
            if *prefix == "processing_sel" && options_are_years(&options) {
                if let Some(year) = release_year(info) {
                    if let Some(v) = match_option(&options, &year.to_string()) {
                        set_field(fields, fname, v);
                        filled_seen = true;
                    }
                }
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
            // Remux 介质档优先精确命中「Remux」/「BD Remux」，再回退 BluRay；
            // 不能让 Blu-ray 泛匹配把「BD 原盘 / BD DIY / BD Remux」算成歧义而留空。
            if picked.is_none() && is_remux && *prefix == "medium_sel" {
                picked = pick_remux_option(&options, &hay);
            }
            if reencoded && disc_source && !is_remux {
                picked = pick_encode_option(&options, &hay);
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
                picked = pick_encode_option(&options, &hay);
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
                    picked = pick_encode_option(&options, &hay);
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
            let inferred = infer_type_from_descr(&info.descr)
                .or_else(|| infer_type_candidates(&info.name, &info.subtitle));
            if let Some((phrases, syns)) = inferred {
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
pub(super) fn fill_secondary_category(
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
pub(super) fn secondary_category_label(
    type_label: &str,
    info: &ReleaseInfo,
) -> Option<&'static str> {
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

pub(super) fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        let u = c as u32;
        (0x3400..=0x4DBF).contains(&u) || (0x4E00..=0x9FFF).contains(&u)
    })
}

/// 选档摘要：`分类=剧集 媒介=HDTV 编码=x264 …`（只列已选中的下拉）
pub(super) fn select_summary(prep: &PreparedUpload) -> String {
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
pub(super) const NON_UPLOAD_ENDPOINTS: &[&str] = &[
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
pub(super) fn non_upload_endpoint(action: &str, base: &Url) -> Option<&'static str> {
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
pub(super) fn form_action(page: &str, base: &Url) -> Option<String> {
    let scope = upload_form_block(page)?;
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
pub(super) fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// checkbox 文案匹配：`<input type=checkbox …>` 后面 60 个字符里含关键词就算选中
pub(super) fn match_checkboxes(page: &str, keyword: &str) -> Vec<Field> {
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
    /// 回包认不出来或 POST 网络中断：站点可能已经收下，也可能只是拒绝页。
    /// 不直接记失败、也不原样重投；交给发后定位按名称+总大小站内核对，
    /// 核对不到落 `posted_unresolved`。
    pub uncertain: bool,
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
    /// 上传页独立海报字段名（配置 `posterField`）
    pub poster_field: Option<String>,
    /// 上传页独立截图字段名（配置 `screenshotField`）
    pub screenshot_field: Option<String>,
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

/// 按站点 `descrOrder` 装配简介所需的源站上下文。
/// 只有规则配了分段顺序的站才用得到；其余站传 `None`，简介与旧版逐字节一致。
#[derive(Debug, Clone)]
pub struct DescrOrderCtx {
    /// 源站名（【转载出处】段正文）
    pub source_label: String,
    /// 源详情页 URL
    pub source_url: String,
    /// 目标站名（制作说明取不到组名时兜底）
    pub target_label: String,
    /// 发布要素（标题/简介/MediaInfo 等）
    pub info: ReleaseInfo,
}

/// 已知在站点侧打不开的图床：本机探测可能 200，但站点服务器会判死图。
/// 这些域名即使探测可达也直接剪掉，避免“本机预检通过、站点审核退回”。
pub(super) fn blocked_image_host(src: &str, site_url: &str) -> bool {
    let lower = src.to_ascii_lowercase();
    let global = ["i.imgur.com", "imgur.com", "hdsky.me", "img.hdsky.me"];
    if global.iter().any(|h| lower.contains(*h)) {
        return true;
    }
    // UBits 审核明确报过 imgbox 图床域名失效；该站只认 pixhost。
    if site_url.contains("ubits") && lower.contains("imgbox.com") {
        return true;
    }
    false
}

/// 本机抽帧后上传到 pixhost 的图是发布时注入的可信资产，不参与死图探测。
pub(super) fn trusted_image_host(src: &str) -> bool {
    let lower = src.to_ascii_lowercase();
    lower.contains("pixhost.to/") || lower.contains("pixhost.cc/")
}

/// 剪掉简介里不可访问的 `<img>`（预审逐图校验；只处理绝对 URL）。
/// 先剪 HTML，再剪 BBCode；`site_url` 用于按站点追加禁用图床。
pub fn prune_dead_images_with_site(
    html: &str,
    site_url: &str,
    probe: &mut dyn FnMut(&str) -> bool,
) -> (String, usize) {
    let (html_out, html_removed) = prune_dead_html_images_with_site(html, site_url, &mut *probe);
    let (bb_out, bb_removed) = prune_dead_bbcode_images_with_site(&html_out, site_url, &mut *probe);
    (bb_out, html_removed + bb_removed)
}

pub(super) fn prune_dead_html_images_with_site(
    html: &str,
    site_url: &str,
    probe: &mut dyn FnMut(&str) -> bool,
) -> (String, usize) {
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
        if image_should_keep(&src, site_url, &mut *probe) {
            out.push_str(tag);
        } else {
            removed += 1;
        }
        i = end;
    }
    out.push_str(&html[i..]);
    (out, removed)
}

/// BBCode 图片（`[img]…[/img]`、`[img=尺寸]…[/img]`）也逐张探测，
/// 避免只处理 HTML 时源站热链图原样带到目标站。
pub(super) fn prune_dead_bbcode_images_with_site(
    text: &str,
    site_url: &str,
    probe: &mut dyn FnMut(&str) -> bool,
) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    let mut removed = 0usize;
    let lower = text.to_ascii_lowercase();
    while let Some(p) = lower[i..].find("[img") {
        let start = i + p;
        let open_end = text[start..].find(']').map(|j| start + j + 1);
        let Some(open_end) = open_end else {
            break;
        };
        let open_tag = &text[start..open_end];
        // `[imgs]` 不是图片标签，跳过。
        let rest = &open_tag[4..];
        if !rest.starts_with(']') && !rest.starts_with('=') {
            out.push_str(&text[i..open_end]);
            i = open_end;
            continue;
        }
        let Some(close_rel) = lower[open_end..].find("[/img]") else {
            out.push_str(&text[i..open_end]);
            i = open_end;
            continue;
        };
        let close_end = open_end + close_rel + "[/img]".len();
        let src = text[open_end..open_end + close_rel].trim();
        out.push_str(&text[i..start]);
        if image_should_keep(src, site_url, &mut *probe) {
            out.push_str(&text[start..close_end]);
        } else {
            removed += 1;
        }
        i = close_end;
    }
    out.push_str(&text[i..]);
    (out, removed)
}

pub(super) fn image_should_keep(
    src: &str,
    site_url: &str,
    probe: &mut dyn FnMut(&str) -> bool,
) -> bool {
    !src.is_empty()
        && src.starts_with("http")
        && !blocked_image_host(src, site_url)
        && (trusted_image_host(src) || probe(src))
}

/// 兼容旧调用的无站点版本（单测和不带站点上下文的探测用）。
pub fn prune_dead_images_with(html: &str, probe: &mut dyn FnMut(&str) -> bool) -> (String, usize) {
    prune_dead_images_with_site(html, "", probe)
}

/// 图片响应是否可当图床资源：2xx 且 content-type 是图片（或二进制流）。
/// 防盗链图床常回 2xx 但正文是 text/html 验证页/418 页，只看状态码会误判成可达；
/// 这里按最终响应的 content-type 排除 HTML/JSON 拦截页。CT 缺失时放行（部分图床不回 CT）。
pub(super) fn content_type_is_image(ct: Option<&str>) -> bool {
    match ct.map(|s| s.trim().to_ascii_lowercase()) {
        Some(c) => c.starts_with("image/") || c == "application/octet-stream",
        None => true,
    }
}

/// 图片可达性探测器；一次构造复用连接，Referer 按目标站上传页/详情页带。
pub(super) struct ImageProbe {
    client: reqwest::blocking::Client,
}

impl ImageProbe {
    pub(super) fn new(referer: &str) -> Option<Self> {
        let mut builder = reqwest::blocking::Client::builder()
            .user_agent(crate::sites::nexusphp::DEFAULT_UA)
            .no_proxy()
            .timeout(std::time::Duration::from_secs(8));
        // 防盗链图床（豆瓣图床实测裸请求 418 回 HTML，带 Referer 才出图）：
        // 探测时带上目标站上传页/详情页 Referer，与浏览器从页面插图的行为一致。
        if !referer.is_empty() {
            let mut headers = HeaderMap::new();
            if let Ok(v) = HeaderValue::from_str(referer) {
                headers.insert(REFERER, v);
            }
            builder = builder.default_headers(headers);
        }
        builder.build().ok().map(|client| Self { client })
    }

    pub(super) fn reachable(&self, url: &str) -> bool {
        self.client
            .get(url)
            .send()
            .map(|r| {
                r.status().is_success()
                    && content_type_is_image(
                        r.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()),
                    )
            })
            .unwrap_or(false)
    }
}

pub(super) fn prune_dead_images(html: &str, referer: &str) -> (String, usize) {
    let Some(probe) = ImageProbe::new(referer) else {
        return (html.to_string(), 0);
    };
    prune_dead_images_with_site(html, referer, &mut |url| probe.reachable(url))
}

/// 标签归一：小写、去空白与标点（保留中文等 Unicode 字母数字；与 `norm_token` 不同，后者只留 ASCII）
pub(super) fn norm_label(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// 标签同义词组：同一控件在不同站点的写法（中字/简中、国语/国配…）
pub(super) fn label_aliases(label: &str) -> &'static [&'static str] {
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
pub(super) fn label_matches(page_label: &str, want: &str) -> bool {
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
pub(super) fn input_controls(
    page: &str,
    prefix: &str,
    kind: &str,
) -> Vec<(String, String, String, bool)> {
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
pub(super) fn prefix_control_names(page: &str, prefix: &str) -> Vec<String> {
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
pub(super) fn prefix_selected(fields: &[Field], page: &str, prefix: &str) -> bool {
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
pub fn apply_site_rules(
    prep: &mut PreparedUpload,
    rules: &crate::gates::SiteRules,
    ctx: Option<&DescrOrderCtx>,
) {
    // 剪死图是硬规则：不分站点、不看配置。站点官方预审逐图校验，死链会直接退回整条种子。
    if let Some(field) = ["descr", "release_desc", "description", "content"]
        .iter()
        .find(|f| field_lookup(&prep.fields, f).is_some())
        .copied()
    {
        // 跨站复制简介会把源站的免责声明、保种提示一起带过来；这些是源站广告，
        // 目标站规则通常明确禁止。先洗一遍 BBCode；HTML 源站等 `apply_site_rules_tail`
        // 转成 BBCode 后会再洗一遍，避免 1PTBA 这类 HTML 简介漏掉。
        strip_cross_site_boilerplate_field(prep);
        let cur = field_lookup(&prep.fields, field).unwrap_or("").to_string();
        let before_images = crate::gates::image_count(&cur);
        let (next, removed) = prune_dead_images(&cur, &prep.form_url);
        if removed > 0 {
            let after_images = crate::gates::image_count(&next);
            set_field(&mut prep.fields, field, next);
            prep.select_notes.push(format!(
                "简介剪掉 {removed} 张不可用图片（{before_images}→{after_images}；逐图探测 + 已知失效图床，硬规则）"
            ));
            // 剪前有图、剪后 0 张：即使站点没配图数门槛也不能发。
            // 2026-11-26 实测白色巨塔 #100 和 Disguiser #54 都被剪成 0 图仍过预检。
            if before_images > 0 && after_images == 0 {
                prep.required_gaps
                    .push("简介图片全被剪掉（0 张可用）：先补可达的海报/截图再发布".into());
            }
        }
    }
    apply_site_rules_tail(prep, rules);
    // 配了 descrOrder 的站：按站点段标题装配简介（没配的站这里直接返回，一字不动）。
    assemble_descr_by_order(prep, rules, ctx);
    // 财神这类站的结构硬规则：MediaInfo 独立栏、简介不带 MediaInfo、图片不包 URL。
    // 放在装配之后检查，确保看的是最终要提交的简介。
    enforce_structural_rules(prep, rules);
    // 剪死图 / 转 BBCode 之后再抽一次独立图片字段：尽量只提交最终简介里真正留下的图。
    let descr = submitted_descr(prep, "").to_string();
    fill_image_fields(
        &mut prep.fields,
        &prep.form_html,
        &descr,
        prep.poster_field.as_deref(),
        prep.screenshot_field.as_deref(),
    );
}

/// 幸运站（LuckPT）LuckAudit 硬断言：有中文字幕就必须真的勾上「中字」。
/// `wanted_tags` 只是“想勾”，表单可能没有该复选框或勾选失败；最终以 `picked_tags`
/// 为准，没勾上就写 `required_gaps`，上传前硬拦。
pub fn enforce_chinese_sub_tag(prep: &mut PreparedUpload, info: &ReleaseInfo, site_id: &str) {
    if site_id != "luckpt" || !info.has_chinese_subs() {
        return;
    }
    if !prep.picked_tags.iter().any(|t| t.contains("中字")) {
        prep.required_gaps
            .push("必填缺失：LuckPT 检测到中文字幕，必须勾「中字」标签（LuckAudit 会退回）".into());
    }
}

/// 站点结构硬规则（目前财神启用）：MediaInfo 必须落独立栏、简介里不能带
/// MediaInfo/BDInfo 块、图片外面不能包 `[url]`/`<a>`。检查最终提交值，缺了就写
/// `required_gaps`；手动 `reseed` 和自动发种都会在上传前中止。
pub(super) fn enforce_structural_rules(prep: &mut PreparedUpload, rules: &crate::gates::SiteRules) {
    if rules.separate_mediainfo() {
        let has_separate = ["technical_info", "media_info", "media_bdinfo", "mediainfo"]
            .iter()
            .any(|f| {
                field_lookup(&prep.fields, f)
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false)
            });
        if !has_separate {
            prep.required_gaps
                .push("必填缺失：MediaInfo 必须填在独立栏，不能只放简介".into());
        }
    }
    let descr = submitted_descr(prep, "").to_string();
    if rules.descr_no_mediainfo() && crate::gates::descr_has_mediainfo_block(&descr) {
        prep.required_gaps
            .push("必填缺失：简介里仍带 MediaInfo/BDInfo 块（技术信息只放独立栏）".into());
    }
    if rules.descr_no_linked_images() && crate::gates::has_url_wrapped_image(&descr) {
        prep.required_gaps
            .push("必填缺失：简介里有图片外层再包 [url]/<a>（站点要求图片裸放）".into());
    }
}

/// 门禁里不需要联网的部分（转 BBCode、选档、必填校验）。单测走这里，避免打外网图。
pub(super) fn apply_site_rules_tail(prep: &mut PreparedUpload, rules: &crate::gates::SiteRules) {
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
    // HTML 简介转成 BBCode 之后，源站 `[quote]...[/quote]` 才现形；这里必须再洗一次。
    strip_cross_site_boilerplate_field(prep);
    if rules.poster_first.unwrap_or(false) {
        apply_poster_first(prep);
    }
    for (prefix, want) in &rules.select_picks {
        for name in select_field_names(&prep.fields, prefix) {
            let cur = field_lookup(&prep.fields, &name).unwrap_or("").to_string();
            // 分类是站点级硬口径（如 iloli 只收动画），允许显式覆盖自动推断；
            // 但只纠正「动漫」这个含糊分支，不能把电影/剧集也硬改成动画。
            // 质量档仍只在空/0 时填，避免站点规则把已正确推出来的档位改坏。
            if prefix == "type" {
                if !cur.is_empty() && cur != "0" {
                    let label = select_options(&prep.form_html, &name)
                        .into_iter()
                        .find(|(v, _)| v == &cur)
                        .map(|(_, l)| l)
                        .unwrap_or_default();
                    if !label.contains("动漫") {
                        prep.select_notes.push(format!(
                            "{name}：自动分类「{label}」不是动漫类，拒绝套用站点 type 覆盖"
                        ));
                        continue;
                    }
                }
            } else if !cur.is_empty() && cur != "0" {
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

    // G1 分类白名单：手动 `reseed` 不走 gates::evaluate，必须在构造字段时就落成
    // 必填缺口，否则「电影→405」这类绕过会直接从审阅走到提交。自动发种也共用这里。
    if !rules.allowed_types.is_empty() {
        if let Some(name) = select_field_names(&prep.fields, "type").into_iter().next() {
            let cur = field_lookup(&prep.fields, &name).unwrap_or("").to_string();
            if !cur.is_empty() && cur != "0" {
                let label = select_options(&prep.form_html, &name)
                    .into_iter()
                    .find(|(v, _)| v == &cur)
                    .map(|(_, l)| l)
                    .unwrap_or_default();
                if !rules
                    .allowed_types
                    .iter()
                    .any(|t| label.contains(t.as_str()))
                {
                    prep.required_gaps.push(format!(
                        "分类「{label}」不在站点白名单（{}）",
                        rules.allowed_types.join("/")
                    ));
                }
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

    // requiredTags：配置声明上传必须勾选的标签（如 agsvpt ⛵方舟 value=52；
    // 13 是杜比、51 是合并）。代码不预设某站所有种子必勾某个标签，是否需要由配置决定。
    // 先按页面真实控件自动勾上；构造完仍未选中就记缺口，门禁阻断，不带空值 POST。
    for req in &rules.required_tags {
        let controls = input_controls(&prep.form_html, &req.field, "checkbox");
        let hit = controls.iter().find(|(_, value, label, _)| {
            req.value.as_deref().is_some_and(|v| v == value)
                || req
                    .label
                    .as_deref()
                    .is_some_and(|l| label_matches(label, l))
        });
        match hit {
            Some((name, value, label, _)) => {
                if !prep.fields.iter().any(|(n, v)| n == name && v == value) {
                    prep.fields.push((name.clone(), value.clone()));
                }
                prep.select_notes.push(format!(
                    "{name}：按规则勾选必需标签「{label}」（requiredTags）"
                ));
            }
            None => {
                let want = req
                    .label
                    .clone()
                    .or_else(|| req.value.clone())
                    .unwrap_or_else(|| req.field.clone());
                prep.required_gaps.push(format!(
                    "必填缺失：{}（必需标签「{}」未勾选）",
                    req.field, want
                ));
            }
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

/// 把简介里的第一张 BBCode 图片移到最前面，其余内容原样接在后面。
/// 只处理 `[img]...[/img]` / `[img=...]...[/img]`；第一张图已经在开头时返回 None，
/// 保证合规简介不被无意义改写。青蛙、AGSVPT 的审核明确要求海报在文字之前。
fn promote_first_image(descr: &str) -> Option<String> {
    let lower = descr.to_ascii_lowercase();
    let start = lower.find("[img]").or_else(|| lower.find("[img="))?;
    let end_rel = lower[start..].find("[/img]")?;
    let end = start + end_rel + "[/img]".len();
    let image = &descr[start..end];
    let before = descr[..start].trim();
    if before.is_empty() {
        return None;
    }
    let after = descr[end..].trim();
    let rest = if after.is_empty() {
        before.to_string()
    } else {
        format!("{before}\n\n{after}")
    };
    Some(format!("{image}\n\n{rest}"))
}

/// 从当前简介字段里洗掉跨站复制过来的源站免责声明/保种提示。
/// 单独抽出来是因为 HTML 简介要先转 BBCode，源站引用块才会现形；调用点要能洗两次。
fn strip_cross_site_boilerplate_field(prep: &mut PreparedUpload) {
    let Some(field) = ["descr", "release_desc", "description", "content"]
        .iter()
        .find(|f| field_lookup(&prep.fields, f).is_some())
        .copied()
    else {
        return;
    };
    let cur = field_lookup(&prep.fields, field).unwrap_or("").to_string();
    let (next, removed) = strip_cross_site_boilerplate(&cur, &prep.form_url);
    if removed == 0 {
        return;
    }
    set_field(&mut prep.fields, field, next);
    prep.select_notes.push(format!(
        "简介删掉 {removed} 段源站免责声明/保种提示（跨站复制清洗）"
    ));
}

/// 删掉从别的站点复制过来的免责声明/保种提示。只处理 `[quote]...[/quote]`
/// 里同时出现“本站(/您的保种是(/免责声明/小提示”和另一个站点域名标签的块；
/// 字幕说明、普通引用和本目标站自己的提示不删。
pub(super) fn strip_cross_site_boilerplate(descr: &str, form_url: &str) -> (String, usize) {
    let Some(target_host) = crate::sites::url_host(form_url) else {
        return (descr.to_string(), 0);
    };
    let target_label = target_host
        .trim_start_matches("www.")
        .split('.')
        .next()
        .unwrap_or("");
    let mut out = String::with_capacity(descr.len());
    let mut rest = descr;
    let mut removed = 0usize;
    while let Some(start) = rest.find("[quote]") {
        let Some(end_rel) = rest[start..].find("[/quote]") else {
            break;
        };
        let end = start + end_rel + "[/quote]".len();
        let block = &rest[start..end];
        let lower = block.to_lowercase();
        let has_promo = ["本站(", "您的保种是(", "免责声明", "小提示"]
            .iter()
            .any(|p| lower.contains(p));
        let mut foreign = false;
        if has_promo {
            for site in crate::sites::builtin_sites() {
                let Some(host) = crate::sites::url_host(site.url) else {
                    continue;
                };
                if host == target_host {
                    continue;
                }
                let label = host
                    .trim_start_matches("www.")
                    .split('.')
                    .next()
                    .unwrap_or("");
                if label.len() >= 4 && label != target_label && lower.contains(label) {
                    foreign = true;
                    break;
                }
            }
        }
        if foreign {
            out.push_str(&rest[..start]);
            removed += 1;
        } else {
            out.push_str(&rest[..end]);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    if removed > 0 {
        while out.contains("\n\n\n") {
            out = out.replace("\n\n\n", "\n\n");
        }
        out = out.trim().to_string();
    }
    (out, removed)
}

/// 按 `posterFirst` 规则把第一张图提到简介最前；找不到简介字段或图已在最前就不动。
fn apply_poster_first(prep: &mut PreparedUpload) {
    let Some(field) = ["descr", "release_desc", "description", "content"]
        .iter()
        .find(|f| field_lookup(&prep.fields, f).is_some())
        .copied()
    else {
        return;
    };
    let cur = field_lookup(&prep.fields, field).unwrap_or("").to_string();
    if let Some(next) = promote_first_image(&cur) {
        set_field(&mut prep.fields, field, next);
        prep.select_notes
            .push("简介：第一张图已提到正文最前（posterFirst）".into());
    }
}
