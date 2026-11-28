//! NexusPHP 页面请求与错误分类：超时重试、限流/拦截页识别、站点提示页。
//! 从 `nexusphp.rs` 拆出，行为不变。

use std::time::Duration;

use anyhow::Result;

use super::ReviewMark;

/// 只有明确的网络超时/断连才值得重试；登录墙、403、HTTP 业务错误不重试。
pub(super) fn retryable_page_error(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    [
        "timed out",
        "timeout",
        "connection reset",
        "connection closed",
        "broken pipe",
    ]
    .iter()
    .any(|m| lower.contains(m))
}

/// 一次请求失败后最多再试 2 次（共 3 次），只对超时/断连生效。
pub(super) fn retry_fetch<T>(url: &str, mut fetch: impl FnMut(&str) -> Result<T>) -> Result<T> {
    let mut last: Option<anyhow::Error> = None;
    for attempt in 0..3u64 {
        match fetch(url) {
            Ok(value) => return Ok(value),
            Err(err) => {
                if attempt < 2 && retryable_page_error(&format!("{err:#}")) {
                    last = Some(err);
                    std::thread::sleep(Duration::from_millis(500 * (attempt + 1)));
                } else {
                    return Err(err);
                }
            }
        }
    }
    Err(last.ok_or_else(|| anyhow::anyhow!("重试循环结束仍没有记录错误"))?)
}

/// 详情页明说“没有这条种子”（NexusPHP 的报错页）或 HTTP 404。
/// 单独一个类型，是为了让回访能分清“站上删了”和“网络故障 / 登录失效”。
#[derive(Debug)]
pub struct NoSuchTorrent {
    pub detail_url: String,
    /// 页面上的原话（“详情页写：没有该ID的种子” / “HTTP 404，页面不存在”）
    pub what: String,
}

impl std::fmt::Display for NoSuchTorrent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "站上已没有这条种子（{}）：{}",
            self.what, self.detail_url
        )
    }
}

impl std::error::Error for NoSuchTorrent {}

/// 下载 .torrent 时站点回的是提示页而不是种子文件。
/// `text` 是完整提示（日志/覆盖记录用），`review` 是提示里能认出的审核结论。
#[derive(Debug)]
pub struct SiteNotice {
    pub text: String,
    pub review: Option<(ReviewMark, String)>,
}

impl std::fmt::Display for SiteNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for SiteNotice {}

/// 检索报错是不是站点在限流（429/503）：调用方据此给该站间隔退避，别按原节奏接着敲
/// （PT 站对无视限流的行为会警告甚至封号）。Swift 版 `SiteThrottle.penalize` 预留了
/// 同一机制但从未接线，Rust 版这里把它接上。
pub fn error_is_rate_limited(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}");
    msg.contains("HTTP 429")
        || msg.contains("HTTP 503")
        // 站点限流页（HTTP 200 + 提示页）在 block_notice 里转成错误，这里一并认
        || msg.contains("站点限流")
}

/// NexusPHP 站的「限流 / 拦截」页：IRL 见 hdsky——HTTP 200、正文却是
/// `<title>403</title>` +「请求过于频繁，限制访问间隔 60 秒/次」。
/// 这种页绝不能当「站内没有」（会把整站记成 7/30 天负缓存），必须冒成错误并让该站退避。
/// JS 人机挑战页（Cloudflare 等）同理：200 + 挑战脚本、没有任何结果锚点，
/// 判重若当成 Miss 会放行重复上传（sbpt 实测靠 existed 兜底才拦住），一律冒错。
pub fn block_notice(html: &str) -> Option<&'static str> {
    const MARKERS: &[&str] = &[
        "请求过于频繁",
        "限制访问间隔",
        "访问过于频繁",
        "请求太频繁",
        "Too Many Requests",
        "Access Denied",
        "你已被禁止",
        "账号已被禁用",
        // Cloudflare / 同类 WAF 的 JS 挑战页（正常 NexusPHP 页面不会出现这些串）
        "<title>Just a moment",
        "challenge-platform",
        "cf-chl",
        "cf-turnstile",
        "Verify you are human",
        "Checking your browser",
        "DDoS-Guard",
        "人机验证",
    ];
    for m in MARKERS {
        if html.contains(m) {
            return Some(m);
        }
    }
    // 极短的 403 页（标题就写着 403）：别拿它当「没有搜索结果」
    let head: String = html.chars().take(4096).collect::<String>().to_lowercase();
    if head.contains("<title>403") {
        return Some("HTTP 403 页");
    }
    None
}
