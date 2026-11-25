//! Kimi WebBridge 检索通道。
//!
//! 大部分 PT 站可以直接用带 cookie 的 HTTP 请求检索；audiences 的检索页额外挂了
//! Cloudflare Turnstile，脚本请求会被判“搜索人机验证未通过”。这里改成驱动用户
//! 已经登录的真实 Chrome：先打开站点种子列表页，等页面拿到 Turnstile token，再在
//! 同源页面里 `fetch` 搜索 URL。这样请求带的是真实浏览器的登录态和验证结果。
//!
//! 人机验证本身仍由用户在浏览器里完成一次；daemon 不可用或 token 一直为空时
//! 返回明确错误，不伪造“站内没有”。

use std::thread::sleep;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:10086/command";

pub struct KimiBrowser {
    endpoint: String,
    session: String,
    client: Client,
}

impl KimiBrowser {
    pub fn new(session: &str) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .no_proxy()
            .build()
            .context("构建 Kimi WebBridge HTTP 客户端失败")?;
        Ok(Self {
            endpoint: std::env::var("KIMI_WEBBRIDGE_URL")
                .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string()),
            session: session.to_string(),
            client,
        })
    }

    fn command(&self, action: &str, args: Value) -> Result<Value> {
        let body = json!({
            "action": action,
            "args": args,
            "session": self.session,
        });
        let payload = serde_json::to_string(&body).context("Kimi 请求体转 JSON 失败")?;
        let resp = self
            .client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .body(payload)
            .send()
            .with_context(|| format!("Kimi WebBridge 没连上：{}", self.endpoint))?;
        let status = resp.status();
        let text = resp
            .text()
            .with_context(|| format!("Kimi WebBridge 返回内容读不出来（HTTP {status}）"))?;
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("Kimi WebBridge 返回的不是 JSON（HTTP {status}）：{text}"))?;
        if value.get("ok") != Some(&Value::Bool(true)) {
            let err = value
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("未知错误");
            return Err(anyhow!("Kimi WebBridge 执行失败：{err}"));
        }
        Ok(value.get("data").cloned().unwrap_or(Value::Null))
    }

    fn evaluate_string(&self, code: &str) -> Result<String> {
        let data = self.command("evaluate", json!({ "code": code }))?;
        let value = data.get("value").cloned().unwrap_or(Value::Null);
        match value {
            Value::String(s) => Ok(s),
            other => Err(anyhow!("Kimi evaluate 返回类型异常：{other}")),
        }
    }

    /// 找当前会话里用户正在看的同站标签页，优先复用它，避免每次新建后台标签页。
    fn active_tab_for(&self, base: &str) -> Option<String> {
        let data = self.command("list_tabs", json!({})).ok()?;
        let tabs = data.get("tabs")?.as_array()?;
        tabs.iter()
            .find(|t| {
                t.get("active").and_then(|v| v.as_bool()) == Some(true)
                    && t.get("url")
                        .and_then(|v| v.as_str())
                        .map(|u| u.starts_with(base))
                        .unwrap_or(false)
            })
            .and_then(|t| t.get("url").and_then(|v| v.as_str()).map(str::to_string))
    }

    /// 打开站点种子列表页，等待 Cloudflare Turnstile token 就绪。
    /// 验证码本身不能由脚本代点；用户第一次在这个浏览器窗口点一下即可。
    pub fn prepare_search_page(&self, base_url: &str) -> Result<()> {
        let base = base_url.trim_end_matches('/');
        let list_url = format!("{base}/torrents.php");
        if let Some(active_url) = self.active_tab_for(base) {
            let _ = self.command("find_tab", json!({ "url": active_url, "active": true }));
        }
        let state_code = r#"(() => {
            const token = document.querySelector('input[name=cf-turnstile-response]');
            const challenge = document.querySelector('.torrent-search-box__captcha');
            return JSON.stringify({
                ready: document.readyState === 'complete',
                token: token ? token.value : null,
                challenge: !!challenge,
                visible: document.visibilityState,
                focused: document.hasFocus(),
                href: location.href
            });
        })()"#;
        // 先看当前标签页是否已经在同一站点且 token 还在。用户点过一次后，
        // 后续判重不要再 navigate 刷新页面，否则可能把已验证的 token 刷掉。
        let mut last = self.evaluate_string(state_code).unwrap_or_default();
        if let Ok(state) = serde_json::from_str::<Value>(&last) {
            let ready = state.get("ready") == Some(&Value::Bool(true));
            let token = state.get("token").and_then(|v| v.as_str()).unwrap_or("");
            let href = state.get("href").and_then(|v| v.as_str()).unwrap_or("");
            if ready && !token.is_empty() && href.starts_with(base) {
                return Ok(());
            }
        }

        let nav = self.command("navigate", json!({ "url": list_url, "newTab": false }))?;
        if nav.get("success") != Some(&Value::Bool(true)) {
            return Err(anyhow!("Kimi 打开 {list_url} 失败：{nav}"));
        }

        for _ in 0..40 {
            last = self.evaluate_string(state_code)?;
            let state: Value = serde_json::from_str(&last).unwrap_or(Value::Null);
            let ready = state.get("ready") == Some(&Value::Bool(true));
            let challenge = state.get("challenge") == Some(&Value::Bool(true));
            let token = state.get("token").and_then(|v| v.as_str()).unwrap_or("");
            if ready && (!challenge || !token.is_empty()) {
                return Ok(());
            }
            sleep(Duration::from_millis(500));
        }
        let hidden = serde_json::from_str::<Value>(&last)
            .ok()
            .and_then(|v| {
                v.get("visible")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
            })
            .as_deref()
            == Some("hidden");
        if hidden {
            return Err(anyhow!(
                "Kimi 打开的 audiences 标签页被浏览器判为后台（document.visibilityState=hidden），Cloudflare Turnstile 不会生成验证框；请把该标签页切到前台、让页面可见后重试（当前状态：{last}）"
            ));
        }
        Err(anyhow!(
            "audiences 搜索需要人工完成一次 Cloudflare 人机验证：请在 Kimi 打开的浏览器窗口里勾选“请验证您是真人”，然后重试（当前状态：{last}）"
        ))
    }

    /// 在已准备好的站点页面里，用浏览器登录态 fetch 搜索 URL。
    pub fn fetch_search_html(&self, url: &str) -> Result<String> {
        let url_json = serde_json::to_string(url).context("搜索 URL 无法转 JSON")?;
        let code = format!(
            r#"(async () => {{
                const r = await fetch({url_json}, {{ credentials: 'include' }});
                const text = await r.text();
                return JSON.stringify({{ status: r.status, url: r.url, html: text }});
            }})()"#
        );
        let raw = self.evaluate_string(&code)?;
        let value: Value = serde_json::from_str(&raw).context("Kimi fetch 返回不是 JSON")?;
        let status = value.get("status").and_then(|v| v.as_u64()).unwrap_or(0);
        let html = value
            .get("html")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if status != 200 {
            let head: String = html.chars().take(120).collect();
            return Err(anyhow!("HTTP {status} 检索 {url}：{head}"));
        }
        if html.contains("搜索人机验证未通过") {
            return Err(anyhow!(
                "audiences 搜索人机验证仍未通过：请先在 Kimi 浏览器里完成一次“请验证您是真人”，再重试"
            ));
        }
        Ok(html)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// 极简 Kimi WebBridge 假服务：读一次请求体，按 handler 返回 JSON 正文。
    fn mock_bridge<F>(handler: F) -> String
    where
        F: Fn(&str) -> String + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock bridge");
        let addr = listener.local_addr().expect("mock bridge addr");
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        break;
                    }
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            content_length = value.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut body = vec![0u8; content_length];
                let _ = reader.read_exact(&mut body);
                let request = String::from_utf8_lossy(&body).to_string();
                let response = handler(&request);
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                );
                let mut stream = stream;
                let _ = stream.write_all(out.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    fn browser_at(endpoint: &str) -> KimiBrowser {
        let mut b = KimiBrowser::new("bsend-test").unwrap();
        b.endpoint = endpoint.to_string();
        b
    }

    #[test]
    fn browser_session_name_is_stable() {
        let b = KimiBrowser::new("bsend-audiences").unwrap();
        assert_eq!(b.session, "bsend-audiences");
        assert!(b.endpoint.starts_with("http://127.0.0.1:"));
    }

    #[test]
    fn command_rejects_bridge_error_and_non_json() {
        let fail = mock_bridge(|_| r#"{"ok":false,"error":"bridge says no"}"#.into());
        let err = browser_at(&fail)
            .evaluate_string("x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("bridge says no"), "{err}");

        let bad = mock_bridge(|_| "not-json".into());
        let err = browser_at(&bad)
            .evaluate_string("x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("不是 JSON"), "{err}");
    }

    #[test]
    fn evaluate_string_rejects_non_string_value() {
        let endpoint = mock_bridge(|_| json!({"ok": true, "data": {"value": 123}}).to_string());
        let err = browser_at(&endpoint)
            .evaluate_string("x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("类型异常"), "{err}");
    }

    #[test]
    fn fetch_search_html_rejects_non_200_and_captcha_text() {
        let status = mock_bridge(|_| {
            json!({
                "ok": true,
                "data": {"value": json!({
                    "status": 500,
                    "url": "https://audiences.example/search",
                    "html": "<p>bad</p>"
                }).to_string()}
            })
            .to_string()
        });
        let err = browser_at(&status)
            .fetch_search_html("https://audiences.example/search")
            .unwrap_err()
            .to_string();
        assert!(err.contains("HTTP 500"), "{err}");

        let captcha = mock_bridge(|_| {
            json!({
                "ok": true,
                "data": {"value": json!({
                    "status": 200,
                    "url": "https://audiences.example/search",
                    "html": "搜索人机验证未通过"
                }).to_string()}
            })
            .to_string()
        });
        let err = browser_at(&captcha)
            .fetch_search_html("https://audiences.example/search")
            .unwrap_err()
            .to_string();
        assert!(err.contains("人机验证仍未通过"), "{err}");
    }

    #[test]
    fn active_tab_prefers_same_site_active_tab() {
        let endpoint = mock_bridge(|_| {
            json!({
                "ok": true,
                "data": {"tabs": [
                    {"active": false, "url": "https://audiences.example/torrents.php"},
                    {"active": true, "url": "https://other.example/x"},
                    {"active": true, "url": "https://audiences.example/torrents.php"}
                ]}
            })
            .to_string()
        });
        assert_eq!(
            browser_at(&endpoint).active_tab_for("https://audiences.example"),
            Some("https://audiences.example/torrents.php".into())
        );
    }

    #[test]
    fn fetch_search_html_returns_body_on_success() {
        let endpoint = mock_bridge(|_| {
            json!({
                "ok": true,
                "data": {"value": json!({
                    "status": 200,
                    "url": "https://audiences.example/search",
                    "html": "<table>hit</table>"
                }).to_string()}
            })
            .to_string()
        });
        let html = browser_at(&endpoint)
            .fetch_search_html("https://audiences.example/search")
            .expect("200 要返回正文");
        assert_eq!(html, "<table>hit</table>");
    }
}
