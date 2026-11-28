//! 杏坛官方图床（`img.xingtan.one`）封面自动转存。
//!
//! 杏坛站规要求封面必须来自官方图床。引擎在正式提交前把简介第一张图下载下来，
//! 用 `cookies.json` 里 `img.xingtan.one` 的登录态上传到官方图床，再把简介里的
//! 旧地址换成新地址；不是人工在浏览器里点上传。
//!
//! 上传走站内网页同源接口 `POST /upload`（Lsky Pro 的网页上传入口），需要
//! 登录 Cookie 和页面 `meta[name=csrf-token]`；接口路径与字段名以页面实际
//! 返回为准，解析失败会明确报错，不会静默保留外链封面。

use anyhow::{bail, Context, Result};
use reqwest::blocking::{multipart, Client};
use serde_json::Value;
use std::time::Duration;

use crate::config::Config;
use crate::reseed::{image_urls_from_descr, PreparedUpload};

const IMAGE_BED_HOST: &str = "img.xingtan.one";
const IMAGE_BED_BASE: &str = "https://img.xingtan.one";
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// 一次封面转存的结果；`None` 表示第一张图已经在官方图床，无需上传。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverRehost {
    pub old_url: String,
    pub new_url: String,
    pub upload_id: Option<u64>,
}

/// 只处理杏坛；其他站点原样返回 `None`。
pub fn rehost_xingtan_cover(
    cfg: &Config,
    site_id: &str,
    prep: &mut PreparedUpload,
    log: &mut dyn FnMut(&str),
) -> Result<Option<CoverRehost>> {
    if site_id != "xingtan" {
        return Ok(None);
    }
    let Some(descr) = descr_field_mut(&mut prep.fields) else {
        bail!("杏坛上传表单没有简介字段，无法确认封面");
    };
    let Some(old_url) = image_urls_from_descr(descr).into_iter().next() else {
        bail!("杏坛简介没有图片，无法生成官方封面；请先补海报/截图");
    };
    if host_of(&old_url).as_deref() == Some(IMAGE_BED_HOST) {
        log("杏坛封面已在官方图床，跳过转存");
        return Ok(None);
    }

    log(&format!("杏坛封面不在官方图床，开始转存：{old_url}"));
    let cookie = cfg.cookie_header_for(IMAGE_BED_HOST).with_context(|| {
        format!(
            "杏坛图床没有登录 Cookie；先在浏览器登录 {IMAGE_BED_BASE}/login，\
             再执行 `bsend cookies --from cookiecloud --write` 刷新"
        )
    })?;
    let client = image_client(
        cfg.site_conf(site_id)
            .and_then(|site| site.proxy.as_deref()),
    )?;
    let (bytes, filename) = download_image(&client, &old_url)?;
    let page = fetch_upload_page(&client, &cookie)?;
    let csrf = csrf_from_page(&page)
        .context("杏坛图床上传页没有 csrf-token；登录态可能已失效，重新登录后再试")?;
    let strategy_id = strategy_id_from_page(&page);
    let uploaded = post_upload(&client, &cookie, &csrf, strategy_id, bytes, &filename)?;

    let replaced = descr.replacen(&old_url, &uploaded.url, 1);
    if replaced == *descr {
        bail!("杏坛简介里的封面地址替换失败：{old_url}");
    }
    *descr = replaced;
    for (_, value) in prep.fields.iter_mut() {
        if value.contains(&old_url) {
            *value = value.replace(&old_url, &uploaded.url);
        }
    }
    log(&format!(
        "杏坛封面已转存官方图床：{} → {}",
        old_url, uploaded.url
    ));
    Ok(Some(CoverRehost {
        old_url,
        new_url: uploaded.url,
        upload_id: uploaded.id,
    }))
}

fn descr_field_mut(fields: &mut [(String, String)]) -> Option<&mut String> {
    for name in ["descr", "release_desc", "description", "content"] {
        if let Some(pos) = fields.iter().position(|(n, _)| n == name) {
            return Some(&mut fields[pos].1);
        }
    }
    None
}

fn host_of(raw: &str) -> Option<String> {
    url::Url::parse(raw)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

fn image_client(proxy_url: Option<&str>) -> Result<Client> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) BoxSend/1.0");
    match proxy_url {
        Some(raw) => {
            // 图床单独挂在 xingtan 的 `overrides.proxy` 上：直连被网络挡下时
            // 可以只给这个站/图床走代理，不影响其他站。
            let proxy =
                reqwest::Proxy::all(raw).with_context(|| format!("图床代理地址不合法：{raw}"))?;
            builder = builder.proxy(proxy);
        }
        None => {
            // 默认直连，避免系统代理残留把请求拖死。
            builder = builder.no_proxy();
        }
    }
    builder.build().context("构建杏坛图床 HTTP 客户端失败")
}

fn download_image(client: &Client, url: &str) -> Result<(Vec<u8>, String)> {
    let resp = client
        .get(url)
        .send()
        .with_context(|| format!("下载封面失败：{url}"))?;
    let status = resp.status();
    if !status.is_success() {
        bail!("下载封面失败：{url} 返回 HTTP {status}");
    }
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_IMAGE_BYTES {
            bail!(
                "封面超过 {} MB，拒绝上传：{url}",
                MAX_IMAGE_BYTES / 1024 / 1024
            );
        }
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if content_type.contains("text/html") {
        bail!("封面地址返回的是网页，不是图片：{url}");
    }
    let bytes = resp
        .bytes()
        .with_context(|| format!("读取封面内容失败：{url}"))?
        .to_vec();
    if bytes.len() > MAX_IMAGE_BYTES {
        bail!(
            "封面超过 {} MB，拒绝上传：{url}",
            MAX_IMAGE_BYTES / 1024 / 1024
        );
    }
    if !content_type.starts_with("image/") {
        // 有些图床不回 Content-Type，能拿到字节就继续；明确是 HTML/文本才拒绝。
        if content_type.contains("json") || content_type.contains("text/") {
            bail!("封面地址返回的不是图片（Content-Type={content_type}）：{url}");
        }
    }
    let filename = filename_for(url, &content_type);
    Ok((bytes, filename))
}

fn filename_for(url: &str, content_type: &str) -> String {
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some(name) = parsed.path_segments().and_then(|mut s| s.next_back()) {
            let name = name.trim();
            if !name.is_empty() && name.contains('.') {
                return name.to_string();
            }
        }
    }
    let ext = if content_type.contains("png") {
        "png"
    } else if content_type.contains("webp") {
        "webp"
    } else if content_type.contains("gif") {
        "gif"
    } else {
        "jpg"
    };
    format!("xingtan-cover.{ext}")
}

fn fetch_upload_page(client: &Client, cookie: &str) -> Result<String> {
    let resp = client
        .get(format!("{IMAGE_BED_BASE}/upload"))
        .header(reqwest::header::COOKIE, cookie)
        .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
        .send()
        .context("打开杏坛图床上传页失败")?;
    let status = resp.status();
    if !status.is_success() {
        bail!("杏坛图床上传页返回 HTTP {status}");
    }
    let html = resp.text().context("读取杏坛图床上传页失败")?;
    if is_login_page(&html) {
        bail!(
            "杏坛图床登录态已失效：上传页跳回登录页。请重新登录 {IMAGE_BED_BASE}/login，\
             再执行 `bsend cookies --from cookiecloud --write` 刷新 Cookie"
        );
    }
    Ok(html)
}

fn post_upload(
    client: &Client,
    cookie: &str,
    csrf: &str,
    strategy_id: i64,
    bytes: Vec<u8>,
    filename: &str,
) -> Result<UploadedImage> {
    let part = multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(guess_mime(filename))
        .context("设置杏坛封面 MIME 失败")?;
    let form = multipart::Form::new()
        .part("file", part)
        .text("_token", csrf.to_string())
        .text("strategy_id", strategy_id.to_string())
        .text("permission", "1");

    let resp = client
        .post(format!("{IMAGE_BED_BASE}/upload"))
        .header(reqwest::header::COOKIE, cookie)
        .header("X-CSRF-TOKEN", csrf)
        .header("X-Requested-With", "XMLHttpRequest")
        .header(reqwest::header::REFERER, format!("{IMAGE_BED_BASE}/upload"))
        .header(reqwest::header::ORIGIN, IMAGE_BED_BASE)
        .header(reqwest::header::ACCEPT, "application/json")
        .multipart(form)
        .send()
        .context("提交杏坛图床上传失败")?;
    let status = resp.status();
    let body = resp.text().context("读取杏坛图床上传回包失败")?;
    if !status.is_success() {
        bail!(
            "杏坛图床上传返回 HTTP {status}：{}",
            body.chars().take(300).collect::<String>()
        );
    }
    if is_login_page(&body) {
        bail!("杏坛图床上传被跳回登录页：登录态已失效");
    }
    parse_upload_response(&body)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UploadedImage {
    url: String,
    id: Option<u64>,
}

fn parse_upload_response(body: &str) -> Result<UploadedImage> {
    let value: Value = serde_json::from_str(body).with_context(|| {
        format!(
            "杏坛图床上传回包不是 JSON：{}",
            body.chars().take(300).collect::<String>()
        )
    })?;
    if value.get("status").and_then(Value::as_bool) != Some(true) {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("上传失败");
        bail!("杏坛图床上传失败：{message}");
    }
    let data = value.get("data").unwrap_or(&Value::Null);
    let raw_url = data
        .pointer("/links/url")
        .and_then(Value::as_str)
        .or_else(|| data.get("url").and_then(Value::as_str))
        .map(str::to_string);
    let pathname = data.get("pathname").and_then(Value::as_str);
    let url = match (raw_url, pathname) {
        (Some(u), _) if u.starts_with("http://") || u.starts_with("https://") => u,
        (Some(u), _) => format!("{IMAGE_BED_BASE}{}", ensure_leading_slash(&u)),
        (None, Some(p)) => format!("{IMAGE_BED_BASE}/i/{}", p.trim_start_matches('/')),
        _ => bail!(
            "杏坛图床上传回包没有图片地址：{}",
            body.chars().take(300).collect::<String>()
        ),
    };
    if host_of(&url).as_deref() != Some(IMAGE_BED_HOST) {
        bail!("杏坛图床回包地址不属于官方图床：{url}");
    }
    let id = data
        .get("id")
        .or_else(|| data.get("key"))
        .and_then(Value::as_u64);
    Ok(UploadedImage { url, id })
}

fn ensure_leading_slash(s: &str) -> String {
    if s.starts_with('/') {
        s.to_string()
    } else {
        format!("/{s}")
    }
}

fn guess_mime(filename: &str) -> &'static str {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else {
        "image/jpeg"
    }
}

fn is_login_page(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    // 登录页特征：表单 action 指向 /login，且带 _token 隐藏字段。
    let has_login_action = lower.contains("action=\"https://img.xingtan.one/login\"")
        || lower.contains("action='https://img.xingtan.one/login'")
        || lower.contains("action=\"/login\"")
        || lower.contains("action='login'");
    has_login_action && lower.contains("name=\"_token\"")
}

fn csrf_from_page(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let pos = lower.find("name=\"csrf-token\"")?;
    let after = &html[pos..];
    let content_pos = after.to_ascii_lowercase().find("content=\"")?;
    let start = content_pos + "content=\"".len();
    let end = after[start..].find('"')?;
    Some(after[start..start + end].to_string())
}

fn strategy_id_from_page(html: &str) -> i64 {
    let lower = html.to_ascii_lowercase();
    let Some(pos) = lower.find("name=\"strategy_id\"") else {
        return 1;
    };
    let after = &html[pos..];
    if let Some(value_pos) = after.to_ascii_lowercase().find("value=\"") {
        let start = value_pos + "value=\"".len();
        if let Some(end) = after[start..].find('"') {
            if let Ok(id) = after[start..start + end].parse::<i64>() {
                return id;
            }
        }
    }
    // select 的 option value 在 name 后面（页面通常只有一个“默认本地策略”）
    if let Some(option_pos) = after.to_ascii_lowercase().find("<option") {
        let option = &after[option_pos..];
        if let Some(value_pos) = option.to_ascii_lowercase().find("value=\"") {
            let start = value_pos + "value=\"".len();
            if let Some(end) = option[start..].find('"') {
                if let Ok(id) = option[start..start + end].parse::<i64>() {
                    return id;
                }
            }
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_csrf_meta() {
        let html = r#"<meta name="csrf-token" content="LEiPjGCeaVPoPNhZ7mBJWUZxh5nFSULuhnPaNGwA">"#;
        assert_eq!(
            csrf_from_page(html).as_deref(),
            Some("LEiPjGCeaVPoPNhZ7mBJWUZxh5nFSULuhnPaNGwA")
        );
    }

    #[test]
    fn detects_login_page() {
        let login = r#"<form method="POST" action="https://img.xingtan.one/login"><input type="hidden" name="_token" value="x">"#;
        assert!(is_login_page(login));
        let upload =
            r#"<meta name="csrf-token" content="x"><form action="https://img.xingtan.one/upload">"#;
        assert!(!is_login_page(upload));
    }

    #[test]
    fn reads_strategy_from_hidden_or_select() {
        assert_eq!(
            strategy_id_from_page(r#"<input name="strategy_id" value="2">"#),
            2
        );
        assert_eq!(
            strategy_id_from_page(
                r#"<select name="strategy_id"><option value="7">默认</option></select>"#
            ),
            7
        );
        assert_eq!(strategy_id_from_page("<form>"), 1);
    }

    #[test]
    fn parses_upload_response_links_url() {
        let body = r#"{"status":true,"message":"上传成功","data":{"id":91772,"pathname":"2026/11/28/6b0a2f2e597b3.png","links":{"url":"https://img.xingtan.one/i/2026/11/28/6b0a2f2e597b3.png"}}}"#;
        let up = parse_upload_response(body).expect("解析成功");
        assert_eq!(up.id, Some(91772));
        assert_eq!(
            up.url,
            "https://img.xingtan.one/i/2026/11/28/6b0a2f2e597b3.png"
        );
    }

    #[test]
    fn parses_upload_response_pathname_fallback() {
        let body = r#"{"status":true,"data":{"pathname":"2026/11/28/a.png"}}"#;
        let up = parse_upload_response(body).expect("解析成功");
        assert_eq!(up.url, "https://img.xingtan.one/i/2026/11/28/a.png");
    }

    #[test]
    fn rejects_foreign_url_and_failed_status() {
        let foreign = r#"{"status":true,"data":{"links":{"url":"https://evil.example/x.png"}}}"#;
        assert!(parse_upload_response(foreign).is_err());
        let failed = r#"{"status":false,"message":"登录已失效"}"#;
        assert!(parse_upload_response(failed).is_err());
    }

    #[test]
    fn filename_falls_back_to_content_type() {
        assert_eq!(filename_for("https://x/y.png", "image/png"), "y.png");
        assert_eq!(
            filename_for("https://x/noext", "image/webp"),
            "xingtan-cover.webp"
        );
    }
}
