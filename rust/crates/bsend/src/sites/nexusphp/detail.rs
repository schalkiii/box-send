//! NexusPHP 结果页/详情页解析：详情链接、种子大小、上传者、审核标记。
//! 从 `nexusphp.rs` 拆出，行为不变。

use scraper::{ElementRef, Html, Selector};
use url::Url;

use super::{
    decode_entities, name_tokens_contained, normalize_search_name, sizes_close_with_slack,
    strip_tags, SearchHit, TorrentPage,
};

// ---------- 结果页 / 详情页解析 ----------

/// 详情链接判定。等价 Swift `(?<![a-zA-Z_])details\.php\?id=\d+|/torrents\.php\?id=\d+`：
/// 「details 前不能是字母/下划线」是必须的——「发布成功」页里的 userdetails.php?id=
/// 是用户主页，不排除会把用户主页当成刚发的种子链接。
pub fn is_detail_href(href: &str) -> bool {
    if let Some(i) = href.find("/torrents.php?id=") {
        if href
            .as_bytes()
            .get(i + 17)
            .is_some_and(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    const D: &str = "details.php?id=";
    for (i, _) in href.match_indices(D) {
        let before_ok = i == 0 || {
            let c = href[..i].chars().next_back().unwrap();
            !(c.is_ascii_alphabetic() || c == '_')
        };
        let after = &href[i + D.len()..];
        if before_ok && after.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// 详情页 id（`id=(\d+)`）
fn detail_id(detail_url: &str) -> Option<u64> {
    let i = detail_url.find("id=")?;
    let rest = &detail_url[i + 3..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// 相对链接补成绝对（结果页常写 details.php?id=123，不补就无法推给下载器）
pub fn resolve_url(reference: &str, base: &Url) -> String {
    match Url::parse(reference) {
        Ok(u) => u.to_string(),
        Err(_) => base
            .join(reference)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| format!("{base}{reference}")),
    }
}

/// 全部 `<a>`（文档序）：href 原值 + 锚文本
fn all_anchors(doc: &Html) -> Vec<(String, String)> {
    let sel = Selector::parse("a").unwrap();
    doc.select(&sel)
        .filter_map(|el| {
            let href = el.value().attr("href")?.to_string();
            let text: String = el.text().collect::<String>();
            Some((href, text))
        })
        .collect()
}

/// 候选匹配打分（越高越优先）：精确相等 3 > 候选包含发布名 2 > 发布名包含候选 1 >
/// 词元容忍 0。同档按覆盖比例（更接近发布名者优先）。None = 不匹配。
/// 多候选择优：同一查询常同时命中同一部片的不同版本/合集，优先取名称最贴近的。
fn candidate_score(candidate: &str, release_name: &str, relaxed: bool) -> Option<(u8, f64)> {
    let c = normalize_search_name(candidate);
    let t = normalize_search_name(release_name);
    if c.chars().count() < 8 || t.chars().count() < 8 {
        return None;
    }
    let c_len = c.chars().count() as f64;
    let t_len = t.chars().count() as f64;
    if c == t {
        return Some((3, 1.0));
    }
    if c.contains(&t) {
        return Some((2, t_len / c_len));
    }
    if t.chars().count() >= 15 && t.contains(&c) {
        return Some((1, c_len / t_len));
    }
    if relaxed && name_tokens_contained(release_name, candidate) {
        return Some((0, 0.0));
    }
    None
}

/// 结果页一行里的大小：先截取详情锚所在 `<tr>` 的上下文，再剥标签找 `12.34 GB`。
/// 版式不固定时找不到就返回 None，后续由详情页大小复核兜底。
fn result_row_size(html: &str, href: &str) -> Option<DisplaySize> {
    let pos = html.find(href)?;
    let start = html[..pos]
        .rfind("<tr")
        .unwrap_or_else(|| pos.saturating_sub(600));
    let end = html[pos..]
        .find("</tr>")
        .map(|i| pos + i + 5)
        .unwrap_or_else(|| (pos + 800).min(html.len()));
    human_size_in_text(&strip_tags(&html[start..end]))
}

fn human_size_in_text(text: &str) -> Option<DisplaySize> {
    let cleaned = text.replace(',', " ");
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        // 结果行剥标签后经常长这样：`...GROUP1.00 GB`，数字和标题粘在一起。
        // 标题里也可能有数字（S01、1080p、5.1），所以词内每个数字串都要试，
        // 只接受数字后面紧接单位、或者数字就是词尾（单位在下一个词）的候选。
        for (num, suffix) in numeric_token_parts(word) {
            let unit = if suffix.is_empty() {
                words.get(i + 1).copied().unwrap_or("")
            } else {
                suffix
            };
            if unit.chars().any(|c| c.is_ascii_alphabetic()) {
                if let Some(v) = parse_human_size_display(&format!("{num} {unit}")) {
                    return Some(v);
                }
            }
        }
    }
    // 兼容 `1.00 GB` 这种数字与单位分成两个词的常规情况
    for w in words.windows(2) {
        if !w[0].chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Some(v) = parse_human_size_display(&format!("{} {}", w[0], w[1])) {
            return Some(v);
        }
    }
    None
}

/// 从 token 里拆出所有“词尾数字 + 后面的单位片段”候选；`GROUP1.00` -> (`1.00`, ``)，
/// `1080p` -> (`1080`, `p`)。标题中的数字不会阻断后面真正的 `1.00 GB`。
fn numeric_token_parts(token: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    for (start, c) in token.char_indices() {
        if !c.is_ascii_digit() {
            continue;
        }
        let mut end = start;
        let mut seen_dot = false;
        for (idx, ch) in token[start..].char_indices() {
            let at = start + idx;
            if ch.is_ascii_digit() {
                end = at + ch.len_utf8();
            } else if ch == '.' && !seen_dot {
                seen_dot = true;
                end = at + 1;
            } else {
                break;
            }
        }
        if end == start {
            continue;
        }
        let num = &token[start..end];
        let rest = &token[end..];
        // 只保留“数字后就是词尾”或“数字后全是字母”的片段；带其他符号说明数字
        // 只是标题的一部分（如 5.1-GROUP），继续找后面的数字串。
        if rest.is_empty() || rest.chars().all(|c| c.is_ascii_alphabetic()) {
            out.push((num, rest));
        }
    }
    out
}

/// 搜索结果页里按名称匹配已存在种子：先按归一化互含，`relaxed` 时再按词元包含放宽。
/// 结果行的 title 属性也参与判定（站内常截断锚文本，完整名只在 title 里）。
/// 多个候选都命中时取 `candidate_score` 最高者，而不是文档序第一个。
pub fn search_name_in_results(
    html: &str,
    release_names: &[String],
    base: &Url,
    relaxed: bool,
    expected_size: Option<i64>,
) -> Option<SearchHit> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("a").unwrap();
    // 扁平候选列表：(href, 文本, 行内大小)，title 更完整时在同锚后面补一条——与 Swift anchorText 的顺序一致
    let mut candidates: Vec<(String, String, Option<DisplaySize>)> = Vec::new();
    for el in doc.select(&sel) {
        let Some(href) = el.value().attr("href") else {
            continue;
        };
        if !is_detail_href(href) {
            continue;
        }
        let size = result_row_size(html, href);
        let text: String = el.text().collect::<String>().trim().to_string();
        candidates.push((href.to_string(), text.clone(), size));
        if let Some(t) = el.value().attr("title") {
            let t = t.trim();
            // title 至少 12 字符且比锚文本完整才补（{12,} 且 count > text.count）
            if t.chars().count() >= 12 && t.chars().count() > text.chars().count() {
                candidates.push((href.to_string(), t.to_string(), size));
            }
        }
    }
    let mut best: Option<(u8, f64, usize)> = None;
    for (idx, cand) in candidates.iter().enumerate() {
        if let (Some(expected), Some(actual)) = (expected_size, cand.2) {
            if !sizes_close_with_slack(actual.bytes, expected, Some(actual.slack_bytes)) {
                continue;
            }
        }
        for name in release_names {
            let Some((rank, ratio)) = candidate_score(&cand.1, name, relaxed) else {
                continue;
            };
            let better = match best {
                None => true,
                Some((br, brt, _)) => rank > br || (rank == br && ratio > brt),
            };
            if better {
                best = Some((rank, ratio, idx));
            }
        }
    }
    let hit = &candidates[best?.2];
    Some(SearchHit {
        detail_url: resolve_url(&hit.0, base),
        title: hit.1.clone(),
        size_bytes: hit.2.map(|s| s.bytes),
        size_slack_bytes: hit.2.map(|s| s.slack_bytes),
    })
}

/// `h1#top` 是提示语而不是种子名：`existed=1` 页面会写“该种子已存在！”。
fn is_notice_h1(text: &str) -> bool {
    let t = text.trim();
    t.contains("该种子已存在")
        || t.contains("种子已存在")
        || t.contains("种子不存在")
        || t.contains("没有该ID")
        || t.contains("错误")
}

/// 详情页解析：标题阶梯（h1#top → #hdarea 首链 → 同 id 详情锚 → `<title>` 清洗）
/// 以及下载链接阶梯（同 id 的 download 锚 → 文本含关键词的 download.php → 空 text 的
/// download.php → .torrent 文本锚（TTG）→ 表单 → 兜底任意 download.php）。
/// Swift 里还有 override 的 torrentLinkPattern/titleStrip（站定制的最后手段），v1 不移植。
pub fn parse_torrent_page(html: &str, detail_url: &str, base: &Url) -> Option<TorrentPage> {
    let doc = Html::parse_document(html);
    let anchors = all_anchors(&doc);
    let id = detail_id(detail_url).filter(|i| *i > 100);

    // ---- 下载链接 ----
    let mut download_url: Option<String> = None;
    let mut name_from_anchor = String::new();
    let clean = |href: &str| resolve_url(href, base);
    // 1) href 同时含 download 与当前详情页 id 的锚点（最可靠，不受版式改版影响）
    if let Some(id) = id {
        for (href, text) in &anchors {
            let t = text.trim();
            if href.contains("download")
                && href.contains(&format!("id={id}"))
                && !t.to_lowercase().starts_with("http")
            {
                name_from_anchor = t.to_string();
                download_url = Some(clean(href));
                break;
            }
        }
    }
    // 2) 文本含关键词的 download.php 锚点
    if download_url.is_none() {
        for (href, text) in &anchors {
            let t = text.trim().to_lowercase();
            if !href.contains("download.php") || t.starts_with("http") {
                continue;
            }
            if t.contains("torrent") || t.contains("种子") || t.contains("下载") {
                name_from_anchor = text.trim().to_string();
                download_url = Some(clean(href));
                break;
            }
        }
    }
    // 3) 纯图标的 download.php 锚点（新版式锚内只有 <img>，text 为空）
    if download_url.is_none() {
        for (href, text) in &anchors {
            if href.contains("download.php") && text.trim().is_empty() {
                download_url = Some(clean(href));
                break;
            }
        }
    }
    // 4) 锚文本就是 .torrent 文件名（TTG 的 /dl/<id>/<随机号> 里没有 download.php）；
    //    zip 附件的锚文本以 .zip 结尾，不会误取
    if download_url.is_none() {
        if let Some(id_str) = detail_id(detail_url).map(|i| i.to_string()) {
            for (href, text) in &anchors {
                let t = text.trim().to_lowercase();
                if t.ends_with(".torrent")
                    && href.contains(&id_str)
                    && !t.starts_with("javascript:")
                {
                    name_from_anchor = text.trim().to_string();
                    download_url = Some(clean(href));
                    break;
                }
            }
        }
    }
    // 5) 兜底：任意含 download.php 的 href
    if download_url.is_none() {
        for (href, _) in &anchors {
            if href.contains("download.php") {
                download_url = Some(clean(href));
                break;
            }
        }
    }
    let download_url = download_url?;

    // ---- 标题 ----
    let mut name: Option<String> = None;
    // 1) <h1 id="top">（LuckPT 等改版 NexusPHP）。`existed=1` 页面会把 h1 换成
    //    “该种子已存在！”这类提示，不能当种子名；跳过并走后面的真实标题兜底。
    if let Ok(sel) = Selector::parse("h1") {
        for el in doc.select(&sel) {
            if el.value().attr("id") == Some("top") {
                let t: String = collapse_spaces(&el.text().collect::<String>());
                if !t.is_empty() && !is_notice_h1(&t) {
                    name = Some(t);
                    break;
                }
            }
        }
    }
    // 2) #hdarea 内第一个链接（经典 NexusPHP）
    if name.is_none() {
        if let Ok(sel) = Selector::parse("#hdarea a") {
            if let Some(el) = doc.select(&sel).next() {
                let t: String = collapse_spaces(&el.text().collect::<String>());
                if !t.is_empty() {
                    name = Some(t);
                }
            }
        }
    }
    // 3) 页面无 h1/#hdarea 时，标题在同 id 的详情锚点内（<a title="名称" href="details.php?id=N"><b>名称</b></a>）
    if name.is_none() {
        if let Some(id) = id {
            let mine = format!("details.php?id={id}");
            let mine2 = format!("/torrents.php?id={id}");
            for (href, text) in &anchors {
                if !(href.starts_with(&mine) || href.starts_with(&mine2)) {
                    continue;
                }
                // 页内锚点（TTG 的 &filelist=1#filelist）文案是「[完整列表]」这类 UI 标签，不是标题
                if href.contains('#') {
                    continue;
                }
                let t = text.trim();
                if t.starts_with('[') && t.ends_with(']') {
                    continue;
                }
                if t.chars().count() > 2 {
                    name = Some(t.to_string());
                    break;
                }
            }
        }
    }
    // 4) <title> 清洗（"站名 :: 种子详情 \"X\" - Powered by NexusPHP" → X）
    if name.is_none() {
        if let Ok(sel) = Selector::parse("title") {
            if let Some(el) = doc.select(&sel).next() {
                let t = clean_title_tag(&el.text().collect::<String>());
                if !t.is_empty() {
                    name = Some(t);
                }
            }
        }
    }
    let name = name.unwrap_or_else(|| name_from_anchor.clone());
    if name.is_empty() {
        return None;
    }
    let (uploader_uid, uploader_name) = page_uploader(html);
    let size = page_size_display(html);
    Some(TorrentPage {
        name,
        download_url,
        size_bytes: size.map(|s| s.bytes),
        size_slack_bytes: size.map(|s| s.slack_bytes),
        uploader_uid,
        uploader_name,
    })
}

/// 人类可读大小 → 字节 + 显示精度（0.01 个显示单位）。支持
/// B / KiB / KB / MiB / MB / GiB / GB / TiB / TB（1024 进制）；解析不出返回 None。
/// 站点详情页大小文本形如 `12.34 GiB` / `1,024 MB`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DisplaySize {
    pub(super) bytes: i64,
    pub(super) slack_bytes: i64,
}

/// 只认“数字 + 显式单位”：数字前可以有不含数字的前缀（`大小：77.43`），
/// 单位可以和数字粘连（`77.43GB`）。没有单位的纯数字是日期、做种数、分辨率这类计数，
/// 一律不当大小；单位只看紧随其后的一个词，下一个词不是单位就说明这个数字不是大小。
pub(super) fn parse_human_size_display(text: &str) -> Option<DisplaySize> {
    let cleaned = text.replace(',', "");
    let tokens: Vec<&str> = cleaned.split_whitespace().collect();
    for (i, token) in tokens.iter().enumerate() {
        let Some((num, glued)) = number_with_optional_unit(token) else {
            continue;
        };
        let unit = if glued.is_empty() {
            tokens.get(i + 1)?.trim_end_matches('.')
        } else {
            glued
        };
        let Some(factor) = unit_factor(unit) else {
            continue;
        };
        let bytes = (num * factor).round() as i64;
        if bytes > 0 {
            // 站点通常只保留两位小数；0.01 个显示单位就是它的舍入粒度。
            let slack_bytes = (factor * 0.01).round() as i64;
            return Some(DisplaySize { bytes, slack_bytes });
        }
    }
    None
}

/// 把一个词拆成“数字 + 紧跟的字母后缀”：`77.43` -> (77.43, ``)，`77.43GB` -> (77.43, `GB`)，
/// `大小：77.43` -> (77.43, ``)。数字前面还带数字的词（`2024-11-23`、`S01E02`）和
/// 数字后面不是纯字母的词（`5.1-GROUP`、`1080p.x264`）都返回 None：那只是标题或日期的一部分。
fn number_with_optional_unit(token: &str) -> Option<(f64, &str)> {
    let glued_len = token
        .bytes()
        .rev()
        .take_while(|b| b.is_ascii_alphabetic())
        .count();
    let head = &token[..token.len() - glued_len];
    let start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit() || *c == '.')
        .last()
        .map(|(i, _)| i)?;
    if token[..start].bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    let num = head[start..].trim_end_matches('.');
    num.parse::<f64>()
        .ok()
        .map(|n| (n, &token[token.len() - glued_len..]))
}

/// 大小单位 → 字节倍数（1024 进制）；不是大小单位返回 None。
fn unit_factor(unit: &str) -> Option<f64> {
    match unit.to_ascii_lowercase().as_str() {
        "b" => Some(1.0),
        "kib" | "kb" => Some(1024.0),
        "mib" | "mb" => Some(1024.0f64.powi(2)),
        "gib" | "gb" => Some(1024.0f64.powi(3)),
        "tib" | "tb" => Some(1024.0f64.powi(4)),
        _ => None,
    }
}

/// 人类可读大小 → 字节；解析不出返回 None。
pub fn parse_human_size(text: &str) -> Option<i64> {
    parse_human_size_display(text).map(|s| s.bytes)
}

/// 从详情页 HTML 提取总大小（字节 + 显示精度）。
///
/// 先找带标签的：以「大小 / 体积 / Size」开头的单元格，取标签后的第一个带单位大小。
/// 标准 NexusPHP 把大小和类型、媒介写在同一格，整格远超下面的长度上限，只靠短单元格取不到。
/// 没有带标签的，再退回“整格只有一个带单位大小”的短单元格（值独占一格的版式）。
/// 没有单位的纯数字是日期、做种数、分辨率，一律不当大小；页眉的“上传量：1.35 TB”、
/// MediaInfo 的 `Size : 7.99 GiB`（单个文件的体积）、文件列表每一行的体积都不是种子总大小，都跳过。
pub(super) fn page_size_display(html: &str) -> Option<DisplaySize> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("td,th,span,div").ok()?;
    let mut bare = None;
    for el in doc.select(&sel) {
        let t: String = el.text().collect::<String>();
        let t = t.trim();
        // 没有数字的文本不可能带大小，先挡掉，省掉后面的解析。
        if t.is_empty() || !t.contains(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let labelled = after_size_label(t).and_then(parse_human_size_display);
        let bare_hit = if bare.is_none() && t.chars().count() <= 40 {
            bare_cell_size(t)
        } else {
            None
        };
        if labelled.is_none() && bare_hit.is_none() {
            continue;
        }
        if in_mediainfo(el) {
            continue;
        }
        if let Some(size) = labelled {
            return Some(size);
        }
        if !in_file_list_row(el) {
            bare = bare_hit;
        }
    }
    bare
}

/// 值独占一格的版式：整格必须以数字开头、就是一个带单位的大小，单位后面不能再有别的数字。
/// `77.43 GB` / `370.69GiB` / `1,024 MB` 算；`上传量:1.353 TB`、`4.945 TB 100.15 GB 50.557`
/// 是页眉的账户统计，不算。
fn bare_cell_size(text: &str) -> Option<DisplaySize> {
    let mut tokens = text.split_whitespace();
    let first = tokens.next()?;
    if !first.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let size = parse_human_size_display(text)?;
    let glued = number_with_optional_unit(first)
        .map(|(_, unit)| !unit.is_empty())
        .unwrap_or(false);
    // 单位和数字分在两个词时，第二个词是单位，不算“后面的内容”。
    if !glued {
        tokens.next();
    }
    if tokens.any(|w| w.chars().any(|c| c.is_ascii_digit())) {
        return None;
    }
    Some(size)
}

/// 单元格文本以「大小 / 体积 / Size」这类总大小标签开头时，返回标签之后的部分。
fn after_size_label(text: &str) -> Option<&str> {
    const LABELS: &[&str] = &["文件大小", "大小", "体积", "容量", "file size", "size"];
    let lower = text.to_ascii_lowercase();
    let label = LABELS.iter().copied().find(|l| lower.starts_with(l))?;
    Some(
        text[label.len()..]
            .trim_start_matches(|c: char| c.is_whitespace() || c == ':' || c == '：'),
    )
}

/// 判断单元格是否落在 MediaInfo 块里。MediaInfo 文本块固定以 `General` 段开头，所以：
/// 往上三层内有以 `General` 开头的容器，或者自身 / 上一层带至少两个 MediaInfo 字段
/// （`Container`、`Bit rate`、`Duration`…），或者往上 8 层内有 class / id 含 `mediainfo` 的容器。
/// 标记只往上看一层：再往上就是整张信息表，简介里的 MediaInfo 也在里面，会把真正的大小那一格误伤。
fn in_mediainfo(el: ElementRef<'_>) -> bool {
    const MARKERS: &[&str] = &[
        "Complete name",
        "Container",
        "Bit rate",
        "Duration",
        "Stream size",
    ];
    let mut node = Some(el);
    for depth in 0..8 {
        let Some(cur) = node else {
            return false;
        };
        let tagged = cur
            .value()
            .classes()
            .chain(cur.value().id())
            .any(|v| v.to_ascii_lowercase().contains("mediainfo"));
        if tagged {
            return true;
        }
        if depth <= 3 {
            let text: String = cur.text().collect();
            let starts_with_general = text.trim_start().starts_with("General");
            let marker_hits = if depth <= 1 {
                MARKERS.iter().filter(|m| text.contains(**m)).count()
            } else {
                0
            };
            if starts_with_general || marker_hits >= 2 {
                return true;
            }
        }
        node = cur.parent().and_then(ElementRef::wrap);
    }
    false
}

/// 文件列表每一行都是「文件名 + 单个体积」，短小又带单位，但不是种子总大小。
/// 只看父元素这一层：同一行里出现视频文件扩展名，就说明这是文件列表。
fn in_file_list_row(el: ElementRef<'_>) -> bool {
    const EXTENSIONS: &[&str] = &[
        ".mkv", ".mp4", ".m2ts", ".ts", ".avi", ".m4v", ".iso", ".mov", ".wmv", ".mpg", ".mpeg",
        ".webm", ".flv",
    ];
    let Some(parent) = el.parent().and_then(ElementRef::wrap) else {
        return false;
    };
    let text: String = parent.text().collect();
    let lower = text.to_ascii_lowercase();
    EXTENSIONS.iter().any(|ext| lower.contains(ext))
}

/// 从详情页 HTML 提取总大小（字节）。只认带单位的大小文本；纯数字不当大小。
pub fn page_size_bytes(html: &str) -> Option<i64> {
    page_size_display(html).map(|s| s.bytes)
}

/// 从详情页 HTML 提取上传者 `(uid, 展示名)`。排除种子详情锚点本身：
/// 只认用户主页链接（`usercp.php?id=` / `userdetails.php?id=` / `details.php?uploader=`）。
pub fn page_uploader(html: &str) -> (Option<String>, Option<String>) {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("a").unwrap();
    for el in doc.select(&sel) {
        let Some(href) = el.value().attr("href") else {
            continue;
        };
        let uid = uploader_uid_from_href(href);
        if let Some(uid) = uid {
            let name: String = el.text().collect::<String>().trim().to_string();
            return (Some(uid), (!name.is_empty()).then_some(name));
        }
    }
    (None, None)
}

/// 从链接 href 里取上传者 uid；种子详情锚（`details.php?id=`）与导航链接不参与。
fn uploader_uid_from_href(href: &str) -> Option<String> {
    const PATTERNS: &[&str] = &[
        "usercp.php?id=",
        "userdetails.php?id=",
        "user.php?id=",
        "details.php?uploader=",
    ];
    for pat in PATTERNS {
        if let Some(p) = href.find(pat) {
            let after = &href[p + pat.len()..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                return Some(digits);
            }
        }
    }
    None
}

/// 从 Cookie 头提取当前账号 uid：NexusPHP 家族常见 `c_secure_uid=` / `uid=` / `myuid=`。
/// 提取不到返回 None（上传者维度跳过核对，不算冲突）。
pub fn current_uid_from_cookie(cookie: &str) -> Option<String> {
    for pair in cookie.split(';') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let k = k.trim().to_ascii_lowercase();
        if matches!(k.as_str(), "c_secure_uid" | "uid" | "myuid") {
            let digits: String = v
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return Some(digits);
            }
        }
    }
    None
}

pub(super) fn collapse_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `<title>` 文本清洗（Swift parseDetail 的 title 兜底段）
fn clean_title_tag(raw: &str) -> String {
    let mut t = collapse_spaces(&decode_entities(raw));
    // "站名 :: 种子详情 \"X\"" / TTG 的 "TTG :: 种子 \"X\" 的详情"：优先取引号里的
    let trimmed = t.trim();
    let quoted = |open: char, close: char| -> Option<String> {
        let s = trimmed.find(open)?;
        let e = trimmed[s + open.len_utf8()..].find(close)? + s + open.len_utf8();
        Some(trimmed[s + open.len_utf8()..e].trim().to_string())
    };
    if let Some(q) = quoted('"', '"').or_else(|| quoted('“', '”')) {
        if !q.is_empty() {
            t = q;
        }
    } else {
        // "站名 :: " 前缀
        if let Some(i) = t.find("::") {
            t = t[i + 2..].trim().to_string();
        }
        if let Some(i) = t.find("种子详情") {
            t = t[i + "种子详情".len()..].trim().to_string();
        }
        if let Some(i) = t.rfind("的详情") {
            t.truncate(i);
        }
        if let Some(i) = t.find("- Powered by NexusPHP") {
            t.truncate(i);
        }
    }
    t.trim_matches(|c: char| matches!(c, '"' | '“' | '”' | '\'' | ' '))
        .trim()
        .to_string()
}
