//! NexusPHP 文本工具：剥标签、HTML 实体解码、下载提示页提取。
//! 从 `nexusphp.rs` 拆出，行为不变。

use scraper::{Html, Selector};
use url::Url;

use super::collapse_spaces;

// ---------- 文本工具 ----------

/// 剥标签：`<br>`/`</p>` 换行，其余标签直接去掉，再解码实体
pub fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        match rest[i..].find('>') {
            Some(j) => {
                out.push_str(&rest[..i]);
                let tag = rest[i + 1..i + j].trim().to_lowercase();
                // <br> 与 </p> 都算换行
                if tag == "/p"
                    || tag == "br"
                    || tag.starts_with("br ")
                    || tag.starts_with("br/")
                    || tag.starts_with("br\t")
                {
                    out.push('\n');
                }
                rest = &rest[i + j + 1..];
            }
            None => {
                out.push_str(rest);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    decode_entities(&out)
}

/// HTML 实体解码：常用命名实体 + `&#NN;` 数字实体
pub fn decode_entities(s: &str) -> String {
    let mut out = s.to_string();
    for (k, v) in [
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#039;", "'"),
        ("&#39;", "'"),
        ("&apos;", "'"),
        ("&mdash;", "—"),
        ("&ndash;", "–"),
        ("&hellip;", "…"),
        ("&laquo;", "«"),
        ("&raquo;", "»"),
        ("&rarr;", "→"),
        ("&larr;", "←"),
        ("&plusmn;", "±"),
        ("&times;", "×"),
    ] {
        out = out.replace(k, v);
    }
    replace_numeric_entities(&out)
}

/// 单趟实体解码：从左到右每个实体只解一次（`&amp;lt;` → `&lt;` 而不是 `<`）。
/// 还原「被整体转义一层」的 HTML 简介用；`decode_entities` 的逐表 replace 会把
/// `&amp;` 先解掉造成二次解码，不能用于这种场景。
pub fn decode_entities_once(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        if let Some(e) = after.find(';').filter(|&e| e <= 12) {
            let ent = &after[..=e];
            if let Some(v) = named_entity(ent).or_else(|| numeric_entity(ent)) {
                out.push_str(&v);
                rest = &after[e + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

fn named_entity(ent: &str) -> Option<String> {
    let v = match ent {
        "&nbsp;" => " ",
        "&amp;" => "&",
        "&lt;" => "<",
        "&gt;" => ">",
        "&quot;" => "\"",
        "&#039;" | "&#39;" | "&apos;" => "'",
        "&mdash;" => "—",
        "&ndash;" => "–",
        "&hellip;" => "…",
        "&laquo;" => "«",
        "&raquo;" => "»",
        "&rarr;" => "→",
        "&larr;" => "←",
        "&plusmn;" => "±",
        "&times;" => "×",
        _ => return None,
    };
    Some(v.to_string())
}

fn numeric_entity(ent: &str) -> Option<String> {
    let body = ent.strip_prefix("&#")?.strip_suffix(';')?;
    let code = if let Some(hex) = body.strip_prefix(['x', 'X']) {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        body.parse::<u32>().ok()?
    };
    char::from_u32(code).map(|c| c.to_string())
}

fn replace_numeric_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("&#") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && after[digits.len()..].starts_with(';') {
            if let Some(ch) = digits.parse::<u32>().ok().and_then(char::from_u32) {
                out.push(ch);
                rest = &after[digits.len() + 1..];
                continue;
            }
        }
        out.push_str("&#");
        rest = after;
    }
    out.push_str(rest);
    out
}

/// NexusPHP 首次下载确认页（downloadnotice.php?type=firsttime）：提取确认表单。
/// 标记是提交按钮 `id="continuedownload"`（NexusPHP 通用）；`action="?"` 表示
/// POST 回当前页（final_url）。返回 (提交地址, 字段表)；checkbox/radio 只取 checked 的。
/// POST 确认后站点记会话标记，再请求原下载链接即回种子。
pub fn parse_download_notice_form(
    html: &str,
    final_url: &Url,
) -> Option<(String, Vec<(String, String)>)> {
    let doc = Html::parse_document(html);
    let form_sel = Selector::parse("form").ok()?;
    let input_sel = Selector::parse("input").ok()?;
    for form in doc.select(&form_sel) {
        if !form.inner_html().contains("continuedownload") {
            continue;
        }
        let action = form.value().attr("action").unwrap_or("");
        let target = if action.is_empty() || action == "?" {
            final_url.clone()
        } else {
            final_url.join(action).ok()?
        };
        let mut fields = Vec::new();
        for input in form.select(&input_sel) {
            let Some(name) = input.value().attr("name") else {
                continue;
            };
            let ty = input.value().attr("type").unwrap_or("text").to_lowercase();
            if (ty == "checkbox" || ty == "radio") && input.value().attr("checked").is_none() {
                continue;
            }
            fields.push((
                name.to_string(),
                input.value().attr("value").unwrap_or("").to_string(),
            ));
        }
        return Some((target.to_string(), fields));
    }
    None
}

/// 下载 .torrent 却拿回 HTML 提示页时，提取页面正文里最像原因的一句话（待审核/未通过/请登录…）。
/// 站方也回纯文本提示（hdtime 的「下载过于频繁，请等待 1 秒后再下载。」整页没有标签），
/// 不能因为不像 HTML 就把提示丢掉。
pub fn download_notice(html: &str) -> String {
    let text = collapse_spaces(&strip_tags(html));
    let chars: Vec<char> = text.chars().collect();
    const DELIMS: &[char] = &['。', '！', '？', '；', ';', '.', '\n'];
    for kw in [
        "审核",
        "未通过",
        "驳回",
        "尚未",
        "已被屏蔽",
        "请登录",
        "无权",
        "不存在",
        "已到期",
        "频繁",
        "稍后",
        "失败",
    ] {
        let kwc: Vec<char> = kw.chars().collect();
        let Some(pos) = find_subsequence(&chars, &kwc) else {
            continue;
        };
        // 只取关键词所在的一句，别把整页正文倒进错误提示
        let start = chars[..pos]
            .iter()
            .rposition(|c| DELIMS.contains(c))
            .map(|i| i + 1)
            .unwrap_or(0);
        let end = chars[pos + kwc.len()..]
            .iter()
            .position(|c| DELIMS.contains(c))
            .map(|i| pos + kwc.len() + i)
            .unwrap_or(chars.len());
        let snippet: String = chars[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
        if !snippet.is_empty() {
            return snippet.chars().take(80).collect();
        }
    }
    String::new()
}

fn find_subsequence(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()] == *needle)
}
