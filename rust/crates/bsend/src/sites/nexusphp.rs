//! NexusPHP 通用端点适配器（站内检索 / 详情解析 / .torrent 下载）。
//! 检索启发式从 Swift `Sites/NexusPHPAdapter.swift` 逐条移植——这套「查询词逐级放宽 +
//! 归一化互含判定」是在真实站点上磨出来的（实测猫站 Black Lagoon：站内把 Blu-ray 写成
//! BluRay，整名查询一条也查不到），别图省事简化。
//!
//! v1 不移植的部分：上传（转种表单填充，GUI 版职责）、海胆式结果页兜底
//! （`matchingTorrentID`/`resultTorrentIDs`——那是 Haidan 框架的版式，不在 v1 框架范围内）。

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::REFERER;
use url::Url;

use crate::bencode;

/// 与 Swift 版默认 userAgent 一致（Config.swift 的 Mac Chrome 串）
pub const DEFAULT_UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 无结果特征（早退；最终判定以名称匹配为准）
const NO_RESULT_MARKERS: &[&str] = &[
    "No torrents found",
    "没有种子",
    "没有相关",
    "no results",
    "No torrents",
];

pub struct NexusPhp {
    pub site_id: String,
    pub site_name: String,
    /// 站点首页（带尾斜杠，`Url` 规范化保证）
    pub base: Url,
    /// boxsend.json overrides.searchURL 的模板（`{name}`/`{imdb}` 占位）
    pub search_url_tmpl: Option<String>,
    /// 手工拼好的 Cookie 头（config 从 cookies.json 按站点 host 组装）
    pub cookie: Option<String>,
    /// 该站 announce 的 host 列表（判断「已在做种」用；默认就是页面 host）
    pub announce_hosts: Vec<String>,
    /// 该站完整 announce 地址（含 passkey）。第一次发种时使用；为空则从 qB 现有 tracker 取。
    pub announce_url: Option<String>,
    /// 上传页的海报字段名（如 `cover`）；为空表示简介里的图就是全部。
    pub poster_field: Option<String>,
    /// 上传页的截图字段名（如 `screenshots`）；提交时一行一个 URL。
    pub screenshot_field: Option<String>,
    /// 上传 POST 的单独超时（秒）：默认 120，可由 `assist.reseedRules.<site>.uploadTimeoutSec` 放宽
    pub upload_timeout_sec: u64,
    pub user_agent: String,
    /// 该站代理的 host:port（配了 `overrides.proxy` 才有）；None = 直连。
    /// 只用于日志和测试断言，不回显带账号口令的完整代理地址。
    pub proxy_endpoint: Option<String>,
    client: Client,
}

/// 构造站点 HTTP 客户端。`proxy_url` 为 None 时维持原来的直连行为（`.no_proxy()`，
/// 不读系统代理）；填了代理地址时只给这一个站的客户端挂代理。
fn build_site_client(proxy_url: Option<&str>) -> Result<Client> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(30))
        // 只走 HTTP/1.1：非浏览器客户端的 HTTP/2 指纹易被 WAF 误伤，
        // 这里是防御性收窄（API POST 曾被「网站防火墙」拦过，见 reseed 官方预审注释）
        .http1_only();
    match proxy_url {
        Some(raw) => {
            let proxy =
                reqwest::Proxy::all(raw).with_context(|| format!("站点代理地址不合法：{raw}"))?;
            builder = builder.proxy(proxy);
        }
        None => {
            // PT 站请求一律直连：passkey 与账号 IP 绑定，走代理既可能连不上也都算异常登录；
            // 另外 Windows 上「系统代理已停用但注册表仍留着 ProxyServer」会让 reqwest
            // 去连一个不存在的 SOCKS，每次请求卡满超时（实机踩到过）。
            builder = builder.no_proxy();
        }
    }
    builder.build().context("构建 HTTP 客户端失败")
}

/// 检索命中：站内已有这条种子的详情页
#[derive(Debug, Clone, Default)]
pub struct SearchHit {
    pub detail_url: String,
    pub title: String,
    /// 结果页列表里的大小；解析不到为 None，后续用详情页复核
    pub size_bytes: Option<i64>,
    /// 结果页大小文本的显示精度（0.01 个显示单位）；用作初筛冗余，最终仍以 pieces 为准
    pub size_slack_bytes: Option<i64>,
}

/// 站点显示的大小是四舍五入过的人类可读值，不能和本地精确字节直接相等。
/// 0.5% 的容差只用于“名字+大小”初筛，最终的 pieces 校验仍是精确比对。
/// 站点有的用 1024 进位标 GB、有的用 1000 进位；同一串 294.27 GB 两种解释会差 7.4%，
/// 所以同时接受 expected 和 expected 的两种单位换算，避免把正确候选误杀。
///
/// 另外站点只显示两位小数时，页面上的 `0.01 GB/GiB` 本身就是一个显示量子；
/// 调用方把该量子传进 `sizes_close_with_slack`，小体积种子也能覆盖显示舍入。
pub fn sizes_close(actual: i64, expected: i64) -> bool {
    sizes_close_with_slack(actual, expected, None)
}

/// 带显示精度余量的大小初筛。`display_slack` 是站点文本里 `0.01` 个显示单位
/// 对应的字节数（例如 `274.04 GiB` 的 0.01 GiB）；None 时退回默认 2 MiB 下限。
pub fn sizes_close_with_slack(actual: i64, expected: i64, display_slack: Option<i64>) -> bool {
    if actual <= 0 || expected <= 0 {
        return false;
    }
    let near = |a: i64, b: i64| {
        // 2 MiB 只作为小体积的绝对下限；同时不超过 b 的 1%，避免 1 MB 的种子
        // 也吃满 2 MiB 容差，把同名不同版本当成同一条。
        let floor = (2 * 1024 * 1024).min(b / 100).max(0);
        let tolerance = (b / 200).max(floor).max(display_slack.unwrap_or(0).max(0));
        (a - b).abs() <= tolerance
    };
    if near(actual, expected) {
        return true;
    }
    let binary_scaled = (expected as f64 * 1.073_741_824).round() as i64;
    let decimal_scaled = (expected as f64 / 1.073_741_824).round() as i64;
    near(actual, binary_scaled) || near(actual, decimal_scaled)
}

/// 详情页解析结果：发布名 + 下载链接 + 发后定位核对字段（大小/上传者）。
/// 核对字段解析不出时为 `None`：比对时该弱维度跳过，不算冲突，也不靠缺字段放行。
#[derive(Debug, Clone)]
pub struct TorrentPage {
    pub name: String,
    pub download_url: String,
    /// 页面解析出的总大小（字节）；解析不出为 None
    pub size_bytes: Option<i64>,
    /// 页面大小文本的显示精度（0.01 个显示单位）；用于初筛冗余，最终仍以 pieces 为准
    pub size_slack_bytes: Option<i64>,
    /// 上传者 uid（`usercp.php?id=` / `userdetails.php?id=` / `details.php?uploader=`）；解析不出为 None
    pub uploader_uid: Option<String>,
    /// 上传者展示名
    pub uploader_name: Option<String>,
}

/// 上传表单 multipart 提交的响应（转种流水线用）
#[derive(Debug, Clone)]
pub struct UploadResponse {
    pub status: u16,
    pub body: String,
    /// 跟随后的最终 URL
    pub final_url: String,
    /// 响应头快照（已剔除 Cookie/Set-Cookie，调试落盘用，不含会话凭据）
    pub headers: Vec<(String, String)>,
}

/// 检索结果。必须区分「站内没有」与「预算不够没查完」——后者绝不能落负缓存。
#[derive(Debug, Clone)]
pub enum SearchOutcome {
    Hit(SearchHit),
    Miss,
    /// 领号失败（本轮预算用完）：调用方本轮跳过这一格，下轮从头查
    BudgetExhausted,
}

impl NexusPhp {
    pub fn new(
        site_id: &str,
        site_name: &str,
        base_url: &str,
        search_url_tmpl: Option<&str>,
        cookie: Option<&str>,
    ) -> Result<Self> {
        let base = Url::parse(base_url).with_context(|| format!("站点 URL 不合法：{base_url}"))?;
        let client = build_site_client(None)?;
        let announce_hosts = base
            .host_str()
            .map(|h| vec![h.to_lowercase()])
            .unwrap_or_default();
        Ok(Self {
            site_id: site_id.to_string(),
            site_name: site_name.to_string(),
            base,
            announce_hosts,
            announce_url: None,
            poster_field: None,
            screenshot_field: None,
            upload_timeout_sec: 120,
            search_url_tmpl: search_url_tmpl
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            cookie: cookie.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            user_agent: DEFAULT_UA.to_string(),
            proxy_endpoint: None,
            client,
        })
    }

    /// 给该站单独挂代理。传 None / 空串 = 保持直连。只影响这一个适配器的客户端，
    /// qB、CookieCloud 和其他站的客户端都不走这里。
    pub fn with_proxy(mut self, proxy_url: Option<&str>) -> Result<Self> {
        let raw = proxy_url
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        match raw.as_deref() {
            Some(url) => {
                self.client = build_site_client(Some(url))?;
                // 日志/断言只用 host:port，不带账号口令
                self.proxy_endpoint = Url::parse(url).ok().and_then(|u| {
                    u.host_str().map(|h| match u.port() {
                        Some(p) => format!("{h}:{p}"),
                        None => h.to_string(),
                    })
                });
            }
            None => {
                self.client = build_site_client(None)?;
                self.proxy_endpoint = None;
            }
        }
        Ok(self)
    }

    /// 配了代理时按日志回调记一行「本站走代理 host:port」；直连不记。
    pub fn log_proxy(&self, log: &mut dyn FnMut(&str)) {
        if let Some(ep) = &self.proxy_endpoint {
            log(&format!("本站走代理 {ep}"));
        }
    }

    /// 设置该站完整 announce 地址（含 passkey）。第一次在该站发种前必须提供，
    /// 否则无法构造只带目标站 tracker 的上传种子。
    pub fn with_announce_url(mut self, url: Option<String>) -> Self {
        self.announce_url = url.filter(|u| !u.trim().is_empty());
        self
    }

    /// 设置上传页的独立图片字段。只在页面确实有该字段时提交，避免把配置里的旧字段名
    /// 强行塞进新版表单。
    pub fn with_image_fields(
        mut self,
        poster_field: Option<String>,
        screenshot_field: Option<String>,
    ) -> Self {
        self.poster_field = poster_field.filter(|v| !v.trim().is_empty());
        self.screenshot_field = screenshot_field.filter(|v| !v.trim().is_empty());
        self
    }

    /// 设置 announce host 列表（页面 host 之外还有别名的站：luckpt / chdbits）
    pub fn with_announce_hosts(mut self, hosts: Vec<String>) -> Self {
        if !hosts.is_empty() {
            self.announce_hosts = hosts;
        }
        self
    }

    fn get(&self, url: &str, referer: &str) -> Result<reqwest::blocking::Response> {
        let mut req = self
            .client
            .get(url)
            .header(REFERER, referer)
            .header("User-Agent", &self.user_agent);
        if let Some(c) = &self.cookie {
            req = req.header("Cookie", c);
        }
        let resp = req.send().with_context(|| format!("请求没通：{url}"))?;
        // Swift fetchHTML 语义：401/403 或被重定向到登录页 = cookie 失效——
        // 这类响应绝不能当「站内没有」落成负缓存，得作为错误冒出去
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return Err(anyhow!("站点回 HTTP {status}（多半是 cookie 失效）：{url}"));
        }
        let final_url = resp.url().as_str().to_lowercase();
        // `returnto=` 是 NexusPHP 被踹回登录页时的跳转参数（实机 pter：index.php?returnto=torrents.php…），
        // 只看 login/signin 会漏掉这一种，把「cookie 失效」误当成「站内没有」
        if ["login", "enter.php", "signin", "auth/login", "returnto="]
            .iter()
            .any(|hint| final_url.contains(hint))
        {
            // 实机 pter：登录态正常（首页显示用户名）但 torrents.php 仍 302 回
            // index.php?returnto=… —— 那是站内公告没点「我已知悉」，或有新手考核。
            // 所以这句不能一口咬定 cookie 失效，两种可能都要摆出来
            return Err(anyhow!(
                "被跳到登录页/首页（cookie 可能已失效，或站内有待确认的公告/考核）：{url}"
            ));
        }
        Ok(resp)
    }

    /// 页面请求失败时只对明确的超时/断连重试；cookie 失效、403、登录墙不重试。
    /// 检索和详情页共用这一条：站点偶发 30 秒超时不应直接把一次判重判成失败。
    fn get_with_retry(&self, url: &str, referer: &str) -> Result<reqwest::blocking::Response> {
        retry_fetch(url, |u| self.get(u, referer))
    }

    /// 抓任意页面的原始 HTML（转种流水线用：上传表单页 / 详情页全文）
    pub fn fetch_html(&self, url: &str, referer: &str) -> Result<String> {
        let resp = self.get_with_retry(url, referer)?;
        let status = resp.status().as_u16();
        let html = resp.text().unwrap_or_default();
        if status != 200 {
            let head: String = html.chars().take(120).collect();
            return Err(anyhow!("HTTP {status} 抓页面 {url}：{head}"));
        }
        Ok(html)
    }

    /// 搜索页抓取：audiences 走 Kimi 真浏览器；其他站仍用带 cookie 的 HTTP。
    /// 搜 IMDb 区的判重也走这里，避免 `target_has_imdb` 单独发无头请求被 Turnstile
    /// 静默返回挑战页，把“验证未通过”误当成“站内没有”。
    pub fn fetch_search_html(&self, url: &str) -> Result<String> {
        if self.uses_kimi_search() {
            let kimi = crate::kimi::KimiBrowser::new("bsend-audiences")?;
            kimi.prepare_search_page(self.base.as_str())?;
            return kimi.fetch_search_html(url);
        }
        self.fetch_html(url, self.base.as_str())
    }

    /// 上传表单 multipart 提交。`file_field` 是种子文件框的 name（NexusPHP 默认 file）。
    /// 返回状态码、响应体、最终 URL（跟跳后）。
    pub fn post_upload(
        &self,
        action: &str,
        fields: &[(String, String)],
        file_field: &str,
        filename: &str,
        torrent: &[u8],
        referer: &str,
    ) -> Result<UploadResponse> {
        let mut form = reqwest::blocking::multipart::Form::new();
        for (n, v) in fields {
            form = form.text(n.clone(), v.clone());
        }
        let part = reqwest::blocking::multipart::Part::bytes(torrent.to_vec())
            .file_name(filename.to_string())
            .mime_str("application/x-bittorrent")
            .context("mime 类型构造失败")?;
        form = form.part(file_field.to_string(), part);

        let mut req = self
            .client
            .post(action)
            // 上传页比列表页慢得多：站点要在服务端解 bencode、算 hash、查重再落库，
            // 北洋园（tjupt）实测 30s 全局超时会在回包前断开（2026-11-24 连挂两次）。
            // 只给上传这一步放宽，列表/检索仍用全局 30s。
            .timeout(std::time::Duration::from_secs(
                self.upload_timeout_sec.max(1),
            ))
            .header(REFERER, referer)
            .header("User-Agent", &self.user_agent)
            .multipart(form);
        if let Some(c) = &self.cookie {
            req = req.header("Cookie", c);
        }
        let resp = req
            .send()
            .with_context(|| format!("上传请求没通：{action}"))?;
        let status = resp.status().as_u16();
        let final_url = resp.url().as_str().to_string();
        // 调试落盘用的头快照：会话凭据类头一律不进快照，避免回包被写进审计/排查目录。
        // 必须在 `resp.text()` 取走响应体之前快照（text 拿走 self 所有权）。
        let headers = resp
            .headers()
            .iter()
            .filter(|(k, _)| {
                let k = k.as_str().to_ascii_lowercase();
                k != "cookie" && k != "set-cookie"
            })
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    v.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let body = resp.text().unwrap_or_default();
        Ok(UploadResponse {
            status,
            body,
            final_url,
            headers,
        })
    }

    /// 表单 POST（x-www-form-urlencoded）给站点 API 用（LuckAudit 预审核
    /// `/api/auto-audit/pre-audit`）。返回状态码与响应体原文；登录态失效按 GET 同款报错。
    ///
    /// 为什么不用 `application/json`：站点 Cloudflare WAF 把 JSON POST 拦成 502
    /// （rustls/openssl/schannel 全中招；`text/json` 等变体还会把源站打炸），
    /// 而普通表单 POST 畅通，应用侧 `$request->input()` 同样能读到嵌套字段
    /// （`type[id]=404` 一类 PHP 数组语法），实测拿到完整预审评分。
    pub fn post_form(
        &self,
        url: &str,
        fields: &[(String, String)],
        referer: &str,
    ) -> Result<(u16, String)> {
        let enc =
            |s: &str| -> String { url::form_urlencoded::byte_serialize(s.as_bytes()).collect() };
        let body = fields
            .iter()
            .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
            .collect::<Vec<_>>()
            .join("&");
        let mut req = self
            .client
            .post(url)
            .header(REFERER, referer)
            .header("User-Agent", &self.user_agent)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .header("X-Requested-With", "XMLHttpRequest")
            // 浏览器同款 Origin（fetch POST 会带；WAF 参与打分）
            .header("Origin", origin_of(url))
            .body(body);
        if let Some(c) = &self.cookie {
            req = req.header("Cookie", c);
        }
        let resp = req.send().with_context(|| format!("API 请求没通：{url}"))?;
        let status = resp.status().as_u16();
        // WAF 拦截与登录态失效长得不一样：把响应体与关键头带进报错，方便一眼归因
        if status == 401 || status == 403 {
            let sniff = resp
                .headers()
                .get("cf-ray")
                .or_else(|| resp.headers().get("server"))
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let body_head: String = resp.text().unwrap_or_default().chars().take(200).collect();
            return Err(anyhow!(
                "站点回 HTTP {status}（cf/server 头：{sniff}；多半是 cookie 失效或 WAF 拦截）：{url}\n响应体：{body_head}"
            ));
        }
        let final_url = resp.url().as_str().to_lowercase();
        if ["login", "enter.php", "signin", "returnto="]
            .iter()
            .any(|hint| final_url.contains(hint))
        {
            return Err(anyhow!("被跳到登录页（cookie 可能已失效）：{url}"));
        }
        Ok((status, resp.text().unwrap_or_default()))
    }

    /// 站内检索找回已存在种子。`relaxed = true` 放宽到「按词元包含」判定：结果页常把长标题
    /// 截断（…LPCM 2.0 2Audios..）、或在同名标题里插备注，严格互含判不出来。只在站点已经
    /// 回答「已存在」、只差找回链接时使用；上传前查重仍走严格判定。
    ///
    /// 查询词阶梯每级都是一次真实 HTTP 请求，实机一轮里一个「站内没有」要花 3 次（严格）
    /// 再加上至多 5 次（宽松）。所以每个请求前都过一次 `gate`（预算 + 站间隔）：不接的话
    /// 几个查询会在 1 秒内连发，PT 站会按爬虫处理。
    pub fn search(
        &self,
        name: &str,
        subtitle: &str,
        relaxed: bool,
        imdb: &str,
        gate: &mut dyn FnMut() -> bool,
    ) -> Result<SearchOutcome> {
        self.search_sized(name, subtitle, relaxed, imdb, None, gate)
    }

    /// 与 `search` 相同，但用本地总大小初筛结果行：同名不同版本很多时，
    /// 只凭名字会先抓错候选；大小对不上的候选不进详情页下载。
    #[allow(clippy::too_many_arguments)]
    pub fn search_sized(
        &self,
        name: &str,
        subtitle: &str,
        relaxed: bool,
        imdb: &str,
        expected_size: Option<i64>,
        gate: &mut dyn FnMut() -> bool,
    ) -> Result<SearchOutcome> {
        if self.uses_kimi_search() {
            return self.search_sized_browser(name, subtitle, relaxed, imdb, expected_size, gate);
        }
        self.search_sized_with(name, subtitle, relaxed, imdb, expected_size, gate, |url| {
            let resp = self.get(url, self.base.as_str())?;
            Ok((resp.status().as_u16(), resp.text().unwrap_or_default()))
        })
    }

    /// audiences 的搜索页挂了 Cloudflare Turnstile，HTTP 脚本请求会被判“搜索人机验证
    /// 未通过”。改走 Kimi WebBridge 的真实 Chrome：先打开种子列表页，等 token；
    /// 再在同源页面里 `fetch` 搜索 URL。人机验证仍需用户在浏览器里点一次。
    fn search_sized_browser(
        &self,
        name: &str,
        subtitle: &str,
        relaxed: bool,
        imdb: &str,
        expected_size: Option<i64>,
        gate: &mut dyn FnMut() -> bool,
    ) -> Result<SearchOutcome> {
        let kimi = crate::kimi::KimiBrowser::new("bsend-audiences")?;
        kimi.prepare_search_page(self.base.as_str())?;
        self.search_sized_with(name, subtitle, relaxed, imdb, expected_size, gate, |url| {
            let html = kimi.fetch_search_html(url)?;
            Ok((200, html))
        })
    }

    /// 检索查询词阶梯的公共部分：调用方只负责把 URL 变成 `(HTTP 状态, HTML)`。
    /// 这样 HTTP 与 Kimi 浏览器两条通道共用同一套名称匹配和大小初筛，不会漂移。
    #[allow(clippy::too_many_arguments)]
    fn search_sized_with(
        &self,
        name: &str,
        subtitle: &str,
        relaxed: bool,
        imdb: &str,
        expected_size: Option<i64>,
        gate: &mut dyn FnMut() -> bool,
        mut fetch: impl FnMut(&str) -> Result<(u16, String)>,
    ) -> Result<SearchOutcome> {
        // 未配 searchURL 时退回 torrents.php?search=（NexusPHP 家族都有这个通用端点）
        let tmpl = self
            .search_url_tmpl
            .clone()
            .unwrap_or_else(|| "torrents.php?search={name}&notnewword=1".to_string());
        let mut seen: HashSet<String> = HashSet::new();
        for (q, target) in query_pairs(name, subtitle, relaxed) {
            if !seen.insert(normalize_search_name(&q)) {
                continue;
            }
            if !gate() {
                return Ok(SearchOutcome::BudgetExhausted);
            }
            let url = self.build_search_url(&tmpl, &q, imdb);
            let (status, html) = retry_fetch(&url, &mut fetch)?;
            if status != 200 {
                let head: String = html.chars().take(120).collect();
                return Err(anyhow!("HTTP {status} 检索 {url}：{head}"));
            }
            if let Some(notice) = block_notice(&html) {
                return Err(anyhow!("站点限流（{notice}）：{url}"));
            }
            if NO_RESULT_MARKERS.iter().any(|m| html.contains(m)) {
                continue;
            }
            // 结果行 = 详情链接的锚文本（各站搜索结果名称在 <a href="details.php?id=..">名称</a> 内）。
            // 用中文副标题查的时候，结果行写的还是英文发布名：发布名也参与复核
            let names = match_names(&target, name);
            let base = Url::parse(&url).unwrap_or_else(|_| self.base.clone());
            if let Some(hit) = search_name_in_results(&html, &names, &base, relaxed, expected_size)
            {
                return Ok(SearchOutcome::Hit(hit));
            }
        }
        Ok(SearchOutcome::Miss)
    }

    /// 这个站点是否必须走 Kimi WebBridge 的真实浏览器检索。
    /// 目前只有 audiences 在搜索页挂了 Turnstile；其他站直接 HTTP 更快。
    pub fn uses_kimi_search(&self) -> bool {
        self.site_id == "audiences"
    }

    fn build_search_url(&self, tmpl: &str, name: &str, imdb: &str) -> String {
        let q = tmpl
            .replace("{imdb}", imdb)
            .replace("{name}", &url_encode(name));
        if q.starts_with("http") {
            q
        } else {
            // Swift 是 site.url + q 纯拼接（site.url 带尾斜杠）；两边各归一一次，
            // 保证 base 和模板之间恰好一个 '/'
            format!(
                "{}/{}",
                self.base.as_str().trim_end_matches('/'),
                q.trim_start_matches('/')
            )
        }
    }

    /// 抓详情页并解析出发布名 + 下载链接
    pub fn fetch_torrent_page(&self, detail_url: &str) -> Result<TorrentPage> {
        self.fetch_torrent_page_marked(detail_url)
            .map(|(page, _)| page)
    }

    /// 抓详情页：解析结果 + 页面上明写的审核标记（未审 / 审核不通过）。
    /// 详情页明说“没有这条种子”（或 HTTP 404）时返回 [`NoSuchTorrent`]，
    /// 调用方据此把它和网络故障、登录失效区分开。
    pub fn fetch_torrent_page_marked(
        &self,
        detail_url: &str,
    ) -> Result<(TorrentPage, Option<(ReviewMark, String)>)> {
        let resp = self.get_with_retry(detail_url, self.base.as_str())?;
        let status = resp.status().as_u16();
        let html = resp.text().unwrap_or_default();
        if status == 404 {
            // 401/403 已在 get 里按 cookie 失效拦下；这里只剩详情页本身不存在
            return Err(anyhow::Error::new(NoSuchTorrent {
                detail_url: detail_url.to_string(),
                what: "HTTP 404，页面不存在".to_string(),
            }));
        }
        if status != 200 {
            let head: String = html.chars().take(120).collect();
            return Err(anyhow!("HTTP {status} 抓详情 {detail_url}：{head}"));
        }
        if let Some(notice) = block_notice(&html) {
            return Err(anyhow!("站点限流（{notice}）：{detail_url}"));
        }
        let base = Url::parse(detail_url).unwrap_or_else(|_| self.base.clone());
        let Some(page) = parse_torrent_page(&html, detail_url, &base) else {
            // NexusPHP 对不存在的 id 回的是 HTTP 200 的报错页（实测熊猫：错误 没有该ID的种子），
            // 不是 404；解析不出标题时先看页面是不是在说“没有这条”。
            if let Some(what) = no_such_torrent_mark(&html) {
                return Err(anyhow::Error::new(NoSuchTorrent {
                    detail_url: detail_url.to_string(),
                    what: format!("详情页写：{what}"),
                }));
            }
            return Err(anyhow!("详情页解析不出标题/下载链接：{detail_url}"));
        };
        let review = review_mark(&html, &page.name);
        Ok((page, review))
    }

    /// 下载 .torrent（referer 用详情页，站点的防盗链检查需要）
    /// 直接按 URL 下载 .torrent（pieces-hash 通道用：已知 torrent id 与直链，
    /// 不需要先读详情页；校验与 `download_torrent` 同一条路径）。
    pub fn download_bytes(&self, url: &str, referer: &str) -> Result<Vec<u8>> {
        let page = TorrentPage {
            name: String::new(),
            download_url: url.to_string(),
            size_bytes: None,
            size_slack_bytes: None,
            uploader_uid: None,
            uploader_name: None,
        };
        self.download_torrent(&page, referer)
    }

    pub fn download_torrent(&self, page: &TorrentPage, referer: &str) -> Result<Vec<u8>> {
        let resp = self.get(&page.download_url, referer)?;
        let status = resp.status().as_u16();
        let final_url = resp.url().clone();
        let data = resp.bytes().unwrap_or_default().to_vec();
        if status == 404 {
            // 401/403 已在 get 里按 cookie 失效拦下
            return Err(anyhow!(
                "下载种子被拒（HTTP 404，链接可能已失效）：{}",
                page.download_url
            ));
        }
        if status >= 400 {
            return Err(anyhow!("HTTP {status} 下载种子：{}", page.download_url));
        }
        // 校验是有效 bencode 且含 info（防止误抓 HTML 提示页，如下载链接过期/失效时）
        if bencode::info_hash_v1(&data).is_none() {
            let text = String::from_utf8_lossy(&data).to_string();
            if let Some(notice) = block_notice(&text) {
                return Err(anyhow!("站点限流（{notice}）：{}", page.download_url));
            }
            // 首次下载确认页：POST 确认表单一次（站点记会话标记），再重试原下载链接
            if let Some((action, fields)) = parse_download_notice_form(&text, &final_url) {
                if self.post_form(&action, &fields, final_url.as_str()).is_ok() {
                    let retry = self.get(&page.download_url, referer)?;
                    let retry_data = retry.bytes().unwrap_or_default().to_vec();
                    if bencode::info_hash_v1(&retry_data).is_some() {
                        return Ok(retry_data);
                    }
                }
            }
            let notice = download_notice(&text);
            let head: String = text.chars().take(80).collect();
            let reason = if notice.is_empty() {
                format!("下载链接返回的不是种子文件（{} bytes，开头：{head}），可能 cookie 失效或下载链接错误", data.len())
            } else {
                format!("站点提示：{notice}")
            };
            // 只认提示里那一句的审核结论：整页（可能带导航/列表）里别的种子的“未审”不算数
            let review = if notice.is_empty() {
                None
            } else {
                // 不给下载的提示里不会有“通过”；真出现了也只当普通提示
                review_mark_in(&notice).filter(|(m, _)| *m != ReviewMark::Approved)
            };
            return Err(anyhow::Error::new(SiteNotice {
                text: format!("{reason}：{}", page.download_url),
                review,
            }));
        }
        Ok(data)
    }
}

mod upload;
use upload::retry_fetch;
#[allow(unused_imports)] // 只在 `mod tests` 里直接按旧名字调用
use upload::retryable_page_error;
pub use upload::*;

mod detail;
mod search;
use detail::collapse_spaces;
pub use detail::*;
#[allow(unused_imports)] // 只在 `mod tests` 里直接按旧名字调用
use detail::{page_size_display, parse_human_size_display};
pub use search::*;

mod text;
pub use text::*;

/// 站点对一条已发布种子的审核结论（只认页面上明写的字样，不猜）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewMark {
    /// 站点还没放行：详情页标着未审，或下载种子时回“尚未通过审核，请稍后再试”
    Pending,
    /// 站点明写审核不通过（多半附原因）
    Rejected,
    /// 详情页的审核图标标着“通过”（藏宝阁实测：`title="通过"`，绿色对勾）
    Approved,
}

impl ReviewMark {
    /// 人话标签
    pub fn label(self) -> &'static str {
        match self {
            ReviewMark::Pending => "等站点审核",
            ReviewMark::Rejected => "审核不通过",
            ReviewMark::Approved => "审核通过",
        }
    }
}

/// 「还没审完」的字样。必须先于「没通过」比对：`尚未通过审核` 里含 `未通过审核`，
/// 顺序反了会把“还没审”判成“没通过”。都是实见样本：tey 的 `title="未审"`、
/// 站点下载提示“尚未通过审核，请稍后再试”。
const REVIEW_PENDING_MARKS: &[&str] = &[
    "尚未通过审核",
    "尚未审核",
    "等待审核",
    "正在审核",
    "待审核",
    "未审核",
    "未审",
    "待审",
];

/// 「审核没过」的字样（正文里写的）。实见样本：麒麟详情页红字“审核不通过，原因：缺少海报或截图（图片少于 2 张）”；
/// agsvpt 标题旁的 `(禁止)` / `（禁止）` 徽标也是审核没过的标记。
const REVIEW_REJECTED_MARKS: &[&str] = &[
    "审核不通过",
    "审核未通过",
    "未通过审核",
    "审核没有通过",
    "审核被拒",
    "(禁止)",
    "（禁止）",
];

/// 审核图标主题下的“不通过”形态。同一套图标（tey `title="未审"`、藏宝阁 `title="通过"`）
/// 的不通过写法没见过实例，按同一位置、同一种写法推断；只认 `title="…"` 属性形式，
/// 不会和正文里的普通字样混淆。
const REVIEW_REJECTED_ATTR_MARKS: &[&str] = &[
    "title=\"不通过\"",
    "title=\"未通过\"",
    "title=\"拒绝\"",
    "title=\"驳回\"",
    "title='不通过'",
    "title='未通过'",
];

/// 审核图标主题下的“通过”形态。实见样本：藏宝阁 43994 `title="通过"`。
/// 只认属性形式——正文里的“通过”（比如考核结果“通過”）和审核无关。
const REVIEW_APPROVED_ATTR_MARKS: &[&str] = &[
    "title=\"通过\"",
    "title=\"审核通过\"",
    "title='通过'",
    "title='审核通过'",
];

/// `title="…"` 形式的标记里引号中间那几个字
fn attr_title(mark: &str) -> String {
    let open = mark.find(['\'', '"']).map(|i| i + 1).unwrap_or(0);
    let close = mark.rfind(['\'', '"']).unwrap_or(mark.len());
    mark[open..close.max(open)].to_string()
}

/// 「没有这条种子」的字样。实测熊猫对不存在的 id 回 HTTP 200 报错页“错误 没有该ID的种子”。
const NO_SUCH_TORRENT_MARKS: &[&str] = &[
    "没有该ID的种子",
    "没有该 ID 的种子",
    "没有这个ID的种子",
    "No torrent with this ID",
    "Torrent does not exist",
    "Torrent not found",
];

/// 页面是不是在说“没有这条种子”。命中返回页面上那句原话。
pub fn no_such_torrent_mark(html: &str) -> Option<&'static str> {
    NO_SUCH_TORRENT_MARKS
        .iter()
        .copied()
        .find(|m| html.contains(m))
}

/// ASCII 大小写不敏感的子串查找（只认 ASCII 字母，字节偏移可直接用于切片）
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| {
        h[i..i + n.len()]
            .iter()
            .zip(n)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

/// 不落在多字节字符中间的切片下界
fn floor_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 详情页里“标题到下载链接”之间的一段：审核角标就挂在这里（tey 的 `title="未审"` 在标题
/// `<h1>` 里，麒麟的“审核不通过，原因：…”在标题和基本信息表之间）。只在这一段里找，
/// 免得把导航栏/公告里的“审核”字样算到这条种子头上。
/// 发布名在页面上找不到时返回 `None`（多半是站点回了提示页），由调用方决定要不要整页找。
///
/// 返回的文本已经解码过 HTML 实体：发布名里的 `&`、引号在页面上是 `&amp;` 之类，
/// 不解码就对不上；角标属性里的 `&#26410;&#23457;` 解码后也才认得出“未审”。
fn title_region(html: &str, torrent_name: &str) -> Option<String> {
    // 只在正文里找标题：`<title>` 里也有发布名，但它在 `<head>`，后面跟着整页导航
    let body = match find_ci(html, "</head>") {
        Some(i) => &html[i + "</head>".len()..],
        None => html,
    };
    let body = decode_entities(body);
    // 发布名开头取长一点最好认；太短容易在导航里先撞上，所以只在长的那几次都没找到时才退到 4 个字
    let mut start = None;
    for take in [12usize, 6, 4] {
        let probe: String = torrent_name.chars().take(take).collect();
        let found = find_ci(&body, &probe).filter(|_| probe.trim().chars().count() >= 3);
        if found.is_some() {
            start = found;
            break;
        }
    }
    let start = floor_char(&body, start?.saturating_sub(160));
    // 角标也可能排在标题前面，往前多留一点；往后到下载链接为止，最长 6KB
    let end = body[start..]
        .find("download.php?id=")
        .map(|i| start + i)
        .unwrap_or(body.len())
        .min(start + 6000);
    let end = floor_char(&body, end);
    Some(body[start..end].to_string())
}

/// 从一句站点原话里截出审核没过的原因：以句号类标点为界，最多 100 个字。
fn sentence_from(text: &str) -> String {
    let cut = text
        .find(['。', '！', '？', '；', ';'])
        .unwrap_or(text.len());
    let s = &text[..cut];
    s.chars().take(100).collect()
}

/// 可见文字里有没有“审核中”这个说法。“审核中心”是导航项，后面紧跟“心”的不算。
fn mentions_review_in_progress(visible: &str) -> bool {
    let mut rest = visible;
    while let Some(i) = rest.find("审核中") {
        let after = &rest[i + "审核中".len()..];
        if !after.starts_with('心') {
            return true;
        }
        rest = after;
    }
    false
}

/// 审核没过的原因：标记后面到下一个标签为止就是站点给的原因；
/// 中间被标签拆开时只剩标记本身，再退到整句。
fn rejected_reason(text: &str, visible: &str, at: usize, m: &str) -> String {
    let tail = &text[at..];
    let stop = tail[m.len()..]
        .find('<')
        .map(|j| m.len() + j)
        .unwrap_or(tail.len());
    let same_run = decode_entities(&tail[..stop]);
    if same_run.chars().count() > m.chars().count() + 1 {
        sentence_from(&collapse_spaces(&same_run))
    } else {
        let at = visible.find(m).unwrap_or(0);
        sentence_from(&visible[at..])
    }
}

/// 在一段页面文本里找审核标记，返回（结论，站点原话）。
///
/// 顺序：明写的“还没审完” → 明写的“没通过” → 含糊的“还没定”（请稍后再试 / 审核中）→
/// 属性形式的不通过 → 属性形式的通过。明写的永远压过含糊的。
/// 等审核的字样连 HTML 属性一起找（tey 的角标是 `<span title="未审">`，剥了标签就看不见）；
/// 没通过的原因取标记所在的同一小段文字（麒麟是 `&nbsp;审核不通过，原因：…</div>`）。
pub fn review_mark_in(text: &str) -> Option<(ReviewMark, String)> {
    // 角标属性可能把汉字写成 `&#26410;&#23457;`，先解码再找（后面的偏移都在解码后的文本里算）
    let text: &str = &decode_entities(text);
    let visible = collapse_spaces(&strip_tags(text));
    // 1. 明写的“审核不通过”先判。拒绝页常同时带一句“修改后会自动进入待审队列”，
    //    如果先扫“待审”就会把已经拒绝的种子误判成还在审。
    let hard_reject = |m: &'static str| text.find(m).map(|i| (i, m));
    if let Some((i, m)) = REVIEW_REJECTED_MARKS
        .iter()
        .copied()
        .filter(|m| *m != "未通过审核")
        .find_map(hard_reject)
    {
        return Some((ReviewMark::Rejected, rejected_reason(text, &visible, i, m)));
    }
    // 2. 明写的“还没审完”
    if let Some(m) = REVIEW_PENDING_MARKS.iter().find(|m| text.contains(**m)) {
        return Some((ReviewMark::Pending, (*m).to_string()));
    }
    // 3. 含糊的“还没定”：请稍后再试 / 审核中（导航项“审核中心”不算）。
    //    “稍后”只在很短的一句话里才算数——详情页那一段里随便一个“稍后”不该推翻结论。
    let in_progress = mentions_review_in_progress(&visible)
        || (visible.contains("稍后") && visible.chars().count() <= 200);
    if in_progress && visible.contains("审核") {
        return Some((ReviewMark::Pending, "审核中".to_string()));
    }
    if let Some((i, m)) = hard_reject("未通过审核") {
        return Some((ReviewMark::Rejected, rejected_reason(text, &visible, i, m)));
    }
    if let Some(m) = REVIEW_REJECTED_ATTR_MARKS
        .iter()
        .find(|m| text.contains(**m))
    {
        return Some((ReviewMark::Rejected, attr_title(m)));
    }
    if let Some(m) = REVIEW_APPROVED_ATTR_MARKS
        .iter()
        .find(|m| text.contains(**m))
    {
        return Some((ReviewMark::Approved, attr_title(m)));
    }
    None
}

/// 详情页上的审核标记。`torrent_name` 是解析出来的发布名，用来定位“标题到下载链接”那一段；
/// 页面上找不到发布名（站点回了提示页）且整页很短时才整页找一次。
pub fn review_mark(html: &str, torrent_name: &str) -> Option<(ReviewMark, String)> {
    if let Some(region) = title_region(html, torrent_name) {
        return review_mark_in(&region);
    }
    // 页面上找不到发布名：多半是站点回了提示页。整页很短才整页找一次，
    // 免得把首页/列表页里别的种子的“未审”角标算到这条头上。
    if strip_tags(html).chars().count() < 600 {
        return review_mark_in(html);
    }
    None
}

/// 查询词百分号编码（未保留字符直通，其余 %XX；空格 → %20）
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 从完整 URL 取 `scheme://host[:port]` 作为 Origin 头（fetch POST 同款；WAF 参与打分）。
/// 解析失败退化为空串（宁可缺头也不错头）。
fn origin_of(url: &str) -> String {
    Url::parse(url)
        .ok()
        .map(|u| {
            let host = u.host_str().unwrap_or_default();
            match u.port() {
                Some(port) => format!("{}://{}:{}", u.scheme(), host, port),
                None => format!("{}://{}", u.scheme(), host),
            }
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audiences_uses_kimi_search() {
        let a = NexusPhp::new(
            "audiences",
            "Audiences",
            "https://audiences.me/",
            None,
            None,
        )
        .unwrap();
        assert!(a.uses_kimi_search());
        let b = NexusPhp::new("luckpt", "LuckPT", "https://pt.luckpt.de/", None, None).unwrap();
        assert!(!b.uses_kimi_search());
    }

    #[test]
    fn adapter_proxy_defaults_direct_and_only_attaches_when_configured() {
        // 不配代理：直连（proxy_endpoint 为 None），行为与原来一致。
        let direct = NexusPhp::new("plain", "Plain", "https://plain.example/", None, None).unwrap();
        assert_eq!(direct.proxy_endpoint, None);

        // 配了代理：该站客户端挂代理；endpoint 只留 host:port，不带账号口令。
        let proxied = NexusPhp::new("gtk", "GTK", "https://pt.gtkpw.xyz/", None, None)
            .unwrap()
            .with_proxy(Some("http://127.0.0.1:7890"))
            .unwrap();
        assert_eq!(proxied.proxy_endpoint.as_deref(), Some("127.0.0.1:7890"));

        // 显式传 None / 空串：回到直连。
        let back = proxied.with_proxy(None).unwrap();
        assert_eq!(back.proxy_endpoint, None);
        let back_empty = NexusPhp::new("e", "E", "https://e.example/", None, None)
            .unwrap()
            .with_proxy(Some("   "))
            .unwrap();
        assert_eq!(back_empty.proxy_endpoint, None);

        // 代理地址不合法：构造直接报错，不静默退回直连。
        assert!(
            NexusPhp::new("bad", "Bad", "https://bad.example/", None, None)
                .unwrap()
                .with_proxy(Some("not a url"))
                .is_err()
        );
    }

    #[test]
    fn entities_decode_single_pass() {
        // 存储层整体转义一层的简介：一趟解码还原成裸 HTML
        assert_eq!(
            decode_entities_once(
                "&lt;fieldset&gt;&lt;legend&gt;摘&lt;/legend&gt;x &amp; y&lt;/fieldset&gt;"
            ),
            "<fieldset><legend>摘</legend>x & y</fieldset>"
        );
        // 二次转义只解一层：&amp;quot; → &quot;，不得变成裸引号
        assert_eq!(
            decode_entities_once("&amp;quot;&amp;lt;br&amp;gt;"),
            "&quot;&lt;br&gt;"
        );
        // 数字实体十进制/十六进制都认
        assert_eq!(decode_entities_once("&#65;&#x42;&#x26;"), "AB&");
        // 未知实体与孤立 & 原样保留
        assert_eq!(
            decode_entities_once("&unknownthing; a & b &#zz;"),
            "&unknownthing; a & b &#zz;"
        );
        // 裸 HTML 不受影响
        assert_eq!(decode_entities_once("<b>已发布</b>"), "<b>已发布</b>");
    }

    #[test]
    fn normalize_and_fold() {
        // 过滤非字母数字后压连续重复字母：blacklagoon 的 oo → o（2006 的数字不压）
        assert_eq!(
            normalize_search_name("Black.Lagoon.S01.1080p.Blu-ray"),
            "blacklagons011080pbluray"
        );
        // BluRay → bluray → 压重复字母后与 Blu-ray 一致
        assert_eq!(fold_doubled_letters("blurray"), "bluray");
        assert_eq!(fold_doubled_letters("2006"), "2006"); // 数字不压
                                                          // 去标签 + 实体
        assert_eq!(normalize_search_name("<b>A&nbsp;Title</b>"), "atitle");
    }

    #[test]
    fn search_names_match_ladder() {
        let name = "Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP";
        // 互含（完全一致 / 站内加了前缀备注 / 丢了尾部组名——前缀仍是子串，都算命中）
        assert!(search_names_match(name, name, false));
        assert!(search_names_match(
            "[置顶] Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP",
            name,
            false
        ));
        assert!(search_names_match(
            "Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1",
            name,
            false
        ));
        // 标题中间插备注（2Audios）：互含判不出（头插的话整串仍是子串，中插才断开），词元包含能判出
        assert!(!search_names_match(
            "Black.Lagoon.S01.2Audios.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP",
            name,
            false
        ));
        assert!(search_names_match(
            "Black.Lagoon.S01.2Audios.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP",
            name,
            true
        ));
        // 编码是裸词元时（空格隔开），x264 vs x265 是关键差异，词元包含也不许过
        assert!(!search_names_match(
            "Show S01 1080p WEB-DL x265 GROUP",
            "Show S01 1080p WEB-DL x264 GROUP",
            true
        ));
        // Swift 原版行为：连字符直接删除，`x265-GROUP` 融合成 `x265group` 一个词元，
        // 不在关键字元表里 → 宽容匹配放行（移植保持一致，不改判）
        assert!(search_names_match(
            "Show.S01.1080p.WEB-DL.x265-GROUP",
            "Show.S01.1080p.WEB-DL.x264-GROUP",
            true
        ));
        assert!(!search_names_match(
            "Show.S01.1080p.WEB-DL.x265-GROUP",
            "Show.S01.1080p.WEB-DL.x264-GROUP",
            false
        ));
        // 太短的名字（<8）直接不判
        assert!(!search_names_match("abc", "abc", false));
    }

    #[test]
    fn search_names_match_rejects_conflicting_discriminating_tokens() {
        let name = "Movie.2023.1080p.BluRay.x264-GROUP";
        // 年份不同：即使其余部分完全一致，宽容模式也不能当同一条
        assert!(!search_names_match(
            "Movie.2024.1080p.BluRay.x264-GROUP",
            name,
            true
        ));
        // 分辨率不同：1080p vs 720p 必须判为两个版本
        assert!(!search_names_match(
            "Movie.2023.720p.BluRay.x264-GROUP",
            name,
            true
        ));
        // 分隔符写法不同但词元一致：仍然命中
        assert!(search_names_match(
            "Movie 2023 1080p BluRay x264 GROUP",
            name,
            true
        ));
    }

    #[test]
    fn discriminating_shapes() {
        assert!(is_discriminating_token("x264"));
        assert!(is_discriminating_token("s01"));
        assert!(is_discriminating_token("e12"));
        assert!(is_discriminating_token("1080p"));
        assert!(is_discriminating_token("2006"));
        assert!(is_discriminating_token("4k"));
        assert!(is_discriminating_token("uhd"));
        assert!(is_discriminating_token("1x03"));
        assert!(!is_discriminating_token("lagoon"));
        assert!(!is_discriminating_token("group"));
        assert!(!is_discriminating_token("108"));
    }

    #[test]
    fn query_ladder_and_dedupe() {
        let pairs = query_pairs("Black.Lagoon.S01.1080p.Blu-ray.DTS-HD-GROUP", "", false);
        // 严格模式取前三：全名、去组标签、词元查询
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0].0, "Black.Lagoon.S01.1080p.Blu-ray.DTS-HD-GROUP");
        assert_eq!(pairs[1].0, "Black.Lagoon.S01.1080p.Blu-ray.DTS-HD");
        // 词元查询：≥3 字符的词、≥3 个、取前 6 个、≥10 字符
        let soft = pairs[2].0.clone();
        assert!(soft.contains("black lagoon"), "soft = {soft}");
        // relaxed 模式加上 32 字截短和副标题
        let relaxed = query_pairs(
            "Black.Lagoon.S01.1080p.Blu-ray.DTS-HD-GROUP",
            "黑礁 第一季【中字】",
            true,
        );
        assert!(relaxed.len() >= 4);
        let sub = relaxed.last().unwrap();
        assert_eq!(sub.0, "黑礁 第一季");
        assert_eq!(sub.1, "黑礁 第一季");
    }

    #[test]
    fn block_pages_are_detected() {
        // 实机 hdsky：HTTP 200，正文是 403 提示页（限流 60 秒/次）
        let hdsky = "<html><head><title>403</title></head><body><h3>请求过于频繁，限制访问间隔 60 秒/次</h3></body></html>";
        assert_eq!(block_notice(hdsky), Some("请求过于频繁"));
        // 短 403 页（只剩标题）也要认
        assert!(block_notice("<html><title>403 Forbidden</title></html>").is_some());
        // 正常的结果页不能被误判成拦截页
        assert!(block_notice("<html><title>搜索结果</title><p>没有种子</p></html>").is_none());
        // JS 人机挑战页（sbpt 实测判重曾把这类页当 Miss）：必须认成拦截
        let cf = "<html><head><title>Just a moment...</title></head><body><script src=\"/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1\"></script></body></html>";
        assert!(block_notice(cf).is_some());
        assert!(block_notice("<div class=\"cf-turnstile\"></div>").is_some());
        // 电影名撞「Just a moment」不算挑战页（只认 title 里的）
        assert!(block_notice("<a href=\"details.php?id=1\">Just a Moment 2024</a>").is_none());
        // 限流页转成的错误要让调用方走退避
        let e = anyhow!("站点限流（请求过于频繁）：https://hdsky.me/torrents.php");
        assert!(error_is_rate_limited(&e));
    }

    #[test]
    fn rate_limited_error_detection() {
        let e = |m: &str| anyhow::anyhow!("{m}");
        assert!(error_is_rate_limited(&e(
            "HTTP 429 检索 https://x.example/torrents.php：Too Many Requests"
        )));
        assert!(error_is_rate_limited(&e(
            "HTTP 503 检索 https://x.example/：维护中"
        )));
        // 非限流错误不算
        assert!(!error_is_rate_limited(&e(
            "HTTP 404 检索 https://x.example/：不存在"
        )));
        assert!(!error_is_rate_limited(&e("请求没通：connection refused")));
    }

    #[test]
    fn word_query_gate() {
        assert_eq!(word_query("A.Bb.Ccc", 6, 3, 60), None); // 词不够
        assert_eq!(word_query("Aa.Bb.Cc", 6, 3, 60), None); // 长度 <10
        assert_eq!(
            word_query("Alpha.Beta.Gamma.Delta.Epsilon.Zeta.Eta", 6, 3, 60).unwrap(),
            "alpha beta gamma delta epsilon zeta"
        );
    }

    #[test]
    fn detail_href_detection() {
        assert!(is_detail_href("details.php?id=123"));
        assert!(is_detail_href("https://hdsky.me/details.php?id=123&hit=1"));
        assert!(is_detail_href("/torrents.php?id=456"));
        assert!(is_detail_href("https://x/torrents.php?id=456&togroup=1"));
        // userdetails.php 是用户主页，不能当种子链接
        assert!(!is_detail_href("userdetails.php?id=123"));
        assert!(!is_detail_href("https://x/userdetails.php?id=123"));
        assert!(!is_detail_href("details.php?id=abc"));
        assert!(!is_detail_href("download.php?id=123"));
    }

    #[test]
    fn search_hits_in_result_page() {
        let html = r#"
        <html><body>
        <!-- 注释里的 details.php?id=999 不该算 -->
        <table>
        <tr><td><a href="details.php?id=1001" title="Other.Release.2020.1080p.Blu-ray.x264-GROUP">Other.Release.2020…</a></td></tr>
        <tr><td><a href="details.php?id=1002">Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1</a></td></tr>
        </table></body></html>"#;
        let base = Url::parse("https://hdsky.me/torrents.php?search=x").unwrap();
        let names = vec!["Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP".to_string()];
        let hit = search_name_in_results(html, &names, &base, false, None).unwrap();
        assert_eq!(hit.detail_url, "https://hdsky.me/details.php?id=1002");
        // 截断的锚文本在 title 里补全：严格互含判不出时靠 title 命中
        let html2 = r#"<a href="details.php?id=1003" title="Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP">Black.Lagoon…</a>"#;
        let hit2 = search_name_in_results(html2, &names, &base, false, None).unwrap();
        assert_eq!(hit2.detail_url, "https://hdsky.me/details.php?id=1003");
        // 查不到时 None
        assert!(search_name_in_results(
            "<a href='details.php?id=1'>Nothing</a>",
            &names,
            &base,
            false,
            None
        )
        .is_none());
    }

    #[test]
    fn search_retries_transient_timeout_then_hits() {
        let adapter = NexusPhp::new("test", "Test", "https://test.example/", None, None).unwrap();
        let name = "Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP";
        let html = format!(r#"<a href="details.php?id=42">{name}</a>"#);
        let mut calls = 0usize;
        let outcome = adapter
            .search_sized_with(name, "", false, "", None, &mut || true, |_url| {
                calls += 1;
                if calls == 1 {
                    Err(anyhow::anyhow!("请求没通：operation timed out"))
                } else {
                    Ok((200, html.clone()))
                }
            })
            .unwrap();
        assert!(matches!(outcome, SearchOutcome::Hit(_)));
        assert_eq!(calls, 2, "一次超时后应重试并命中");
    }

    #[test]
    fn page_retry_only_matches_transient_network_errors() {
        assert!(retryable_page_error("请求没通：operation timed out"));
        assert!(retryable_page_error("connection reset by peer"));
        assert!(!retryable_page_error(
            "站点回 HTTP 403（多半是 cookie 失效）"
        ));
        assert!(!retryable_page_error("站点限流（请求过于频繁）"));
    }

    #[test]
    fn search_prefers_closest_candidate_among_matches() {
        let base = Url::parse("https://hdsky.me/torrents.php?search=x").unwrap();
        let names = vec!["Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP".to_string()];
        // 两个候选都能命中（一个多 "Extended"、一个精确），精确者优先
        let html = r#"
        <a href="details.php?id=2001">Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1.Extended-GROUP</a>
        <a href="details.php?id=2002">Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP</a>"#;
        let hit = search_name_in_results(html, &names, &base, false, None).unwrap();
        assert_eq!(hit.detail_url, "https://hdsky.me/details.php?id=2002");
        // 同档按覆盖比例：越接近发布名越优先（顺序反过来也取 2002）
        let html2 = r#"
        <a href="details.php?id=2002">Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP</a>
        <a href="details.php?id=2001">Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1.Extended-GROUP</a>"#;
        let hit2 = search_name_in_results(html2, &names, &base, false, None).unwrap();
        assert_eq!(hit2.detail_url, "https://hdsky.me/details.php?id=2002");
    }

    #[test]
    fn search_result_size_filters_same_name_other_version() {
        let base = Url::parse("https://hdsky.me/torrents.php?search=x").unwrap();
        let names = vec!["Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP".to_string()];
        let html = r#"
        <table>
        <tr><td><a href="details.php?id=3001">Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP</a></td><td>1.00 GB</td></tr>
        <tr><td><a href="details.php?id=3002">Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP</a></td><td>2.00 GB</td></tr>
        </table>"#;
        let expected = 2 * 1024 * 1024 * 1024i64;
        let hit = search_name_in_results(html, &names, &base, false, Some(expected)).unwrap();
        assert_eq!(hit.detail_url, "https://hdsky.me/details.php?id=3002");
        assert_eq!(hit.size_bytes, Some(expected));

        // expected 1 GB 时只能选 3001，不能因为名字一样就抓 2 GB 的 3002
        let hit_one =
            search_name_in_results(html, &names, &base, false, Some(1024 * 1024 * 1024)).unwrap();
        assert_eq!(hit_one.detail_url, "https://hdsky.me/details.php?id=3001");

        // 页面里只有 2 GB 版本、expected 是 1 GB：宁可返回 None，也不抓错版本
        let only_two = r#"<tr><td><a href="details.php?id=3002">Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP</a></td><td>2.00 GB</td></tr>"#;
        assert!(
            search_name_in_results(only_two, &names, &base, false, Some(1024 * 1024 * 1024))
                .is_none()
        );
    }

    #[test]
    fn sizes_close_allows_display_rounding_but_not_other_version() {
        let expected = 294_270_271_759i64;
        assert!(sizes_close(expected, expected));
        // 同一串「294.27 GB」按 1024 进位解析会大 7.4%，也要接受，避免误杀正确候选
        let binary_display = (expected as f64 * 1.073_741_824).round() as i64;
        assert!(sizes_close(binary_display, expected));
        // 同名但差一个数量级/明显不同版本：不能过
        assert!(!sizes_close(100 * 1024 * 1024 * 1024, expected));
        assert!(!sizes_close(0, expected));
    }

    #[test]
    fn sizes_close_covers_one_hundredth_of_displayed_unit() {
        let one_gib = 1024 * 1024 * 1024i64;
        let display = parse_human_size_display("1.00 GB").expect("1.00 GB must parse");
        assert_eq!(display.bytes, one_gib);
        assert_eq!(display.slack_bytes, (one_gib as f64 * 0.01).round() as i64);

        // 差 10 MiB：默认 0.5% 容差不够，但 0.01 GiB 的显示余量必须放行。
        let rounded = one_gib - 10 * 1024 * 1024;
        assert!(!sizes_close(rounded, one_gib));
        assert!(sizes_close_with_slack(
            rounded,
            one_gib,
            Some(display.slack_bytes)
        ));

        // 小体积种子按自己的单位算余量，不套用 10 MiB 的固定下限。
        let one_mib = 1024 * 1024i64;
        let small = parse_human_size_display("1.00 MB").expect("1.00 MB must parse");
        assert_eq!(small.slack_bytes, 10_486);
        assert!(sizes_close_with_slack(
            one_mib - 10_000,
            one_mib,
            Some(small.slack_bytes)
        ));
        assert!(!sizes_close_with_slack(
            one_mib - 12_000,
            one_mib,
            Some(small.slack_bytes)
        ));
    }

    #[test]
    fn detail_page_carries_display_size_slack() {
        let base = Url::parse("https://hdsky.me/details.php?id=1002").unwrap();
        let html = r#"
        <html><body><h1 id="top">Black.Lagoon.S01.1080p.Blu-ray.GROUP</h1>
        <table><tr><td>大小：</td><td>1.00 GB</td></tr></table>
        <a href="download.php?id=1002">下载种子</a>
        </body></html>"#;
        let page = parse_torrent_page(html, "https://hdsky.me/details.php?id=1002", &base).unwrap();
        assert_eq!(page.size_bytes, Some(1024 * 1024 * 1024));
        assert_eq!(page.size_slack_bytes, Some(10_737_418));
    }

    #[test]
    fn parse_detail_page_ladder() {
        let base = Url::parse("https://hdsky.me/details.php?id=1002").unwrap();
        // h1#top + download 锚（带同 id）
        let html = r#"
        <html><body><h1 id="top">Black.Lagoon.S01.1080p.Blu-ray.GROUP</h1>
        <a href="download.php?id=1002">下载种子</a>
        </body></html>"#;
        let page = parse_torrent_page(html, "https://hdsky.me/details.php?id=1002", &base).unwrap();
        assert_eq!(page.name, "Black.Lagoon.S01.1080p.Blu-ray.GROUP");
        assert_eq!(page.download_url, "https://hdsky.me/download.php?id=1002");
        // 无 h1：title 清洗 + 空文本的 download.php（纯图标锚）
        let html2 = r#"
        <html><head><title>HDSky :: 种子详情 &quot;Some.Show.S02.720p-GROUP&quot; - Powered by NexusPHP</title></head>
        <body><div id="hdarea"><a href="details.php?id=55"><b>Some.Show.S02.720p</b></a></div>
        <a href="download.php?id=55"><img src="dl.gif"></a></body></html>"#;
        let page2 = parse_torrent_page(html2, "https://hdsky.me/details.php?id=55", &base).unwrap();
        assert_eq!(page2.name, "Some.Show.S02.720p");
        assert_eq!(page2.download_url, "https://hdsky.me/download.php?id=55");
    }

    #[test]
    fn existed_notice_h1_falls_back_to_title_tag() {
        let base = Url::parse("https://hdfans.org/details.php?id=287874").unwrap();
        let html = r#"
        <html><head><title>HDFans :: 种子详情 &quot;Liar Game S01 2007 2160p WEB-DL H.264 AAC-ADWeb    [50%] (通过)&quot; - Powered by NexusPHP</title></head>
        <body><h1 id="top">该种子已存在！</h1>
        <a href="download.php?id=287874">下载种子</a></body></html>"#;
        let page = parse_torrent_page(
            html,
            "https://hdfans.org/details.php?id=287874&existed=1",
            &base,
        )
        .unwrap();
        assert_eq!(
            page.name,
            "Liar Game S01 2007 2160p WEB-DL H.264 AAC-ADWeb [50%] (通过)"
        );
        assert_eq!(
            page.download_url,
            "https://hdfans.org/download.php?id=287874"
        );
    }

    #[test]
    fn notice_extraction() {
        // 站点纯文本提示也能提取（整页没有标签）
        assert!(
            download_notice("下载过于频繁，请等待 1 秒后再下载。其他正文。")
                .contains("下载过于频繁")
        );
        assert!(
            download_notice("<html>对不起，该种子尚未通过审核，请稍后再试。</html>")
                .contains("审核")
        );
        assert_eq!(download_notice("一切正常"), "");
    }

    // ---- 审核标记：样本照 2026-11-24 抓到的真实页面结构缩写（长路径、无关内容删掉） ----

    /// tey 623：审核图标在标题 `<h1>` 里，`title="未审"`
    const TEY_PENDING: &str = "<html><head><title>Tey :: 种子详情 \"The Disguiser Complete HDTV 1080p AAC x264-JING\"</title></head><body>\
<aside class=\"n8-ubar\"><a href=\"userdetails.php?id=1\"><span>UID 10444</span><svg viewBox=\"0 0 24 24\"><path d=\"M9 6l6 6-6 6\"/></svg></a></aside>\
<div class=\"n8-content\" id=\"outer\">\
<div class=\"n8-promo\"><b><a href=\"torrents.php\"><font color=\"white\">全站 [Free] 生效中！</font></a></b></div>\
<h1 align=\"center\" id=\"top\">The Disguiser Complete HDTV 1080p AAC x264-JING&nbsp;&nbsp;&nbsp; <b>[<font class='free' >免费</font>]</b><span style=\"margin-left: 6px\" title=\"未审\"><svg class=\"icon\" viewBox=\"0 0 1024 1024\" width=\"16\" height=\"16\"><path d=\"M450.267 772.245\"/></svg></span></h1>\
<table width=\"97%\"><tr><td class=\"rowhead\">下载</td><td class=\"rowfollow\"><a class=\"index\" href=\"download.php?id=623\">[Tey].torrent</a></td></tr></table>\
</div></body></html>";

    /// 麒麟 479362：标题下面的红底横幅写“审核不通过，原因：…”；导航里有“审核中心 / 一键审种”
    const QILIN_REJECTED: &str = "<html><head><title>HD-Kylin :: 种子详情 \"Cinderella Man 2005\"</title></head><body>\
<div id=\"nav\">首页 论坛 审核中心 一键审种</div>\
<div style=\"text-align: center;margin-bottom: 10px\"><h1 id=\"top\">Cinderella Man 2005 1080p x265-BeiTai&nbsp;&nbsp; <b>[<font class='free'>免费</font>]</b></h1>\
<div style=\"display: flex;background-color: black; color: white;font-weight: bold; padding: 10px 100px\"><svg viewBox=\"0 0 1024 1024\" width=\"16\" height=\"16\"><path d=\"M963.5 856.8\" fill=\"#d81e06\"/></svg>&nbsp;审核不通过，原因：缺少海报或截图（图片少于 2 张）</div></div>\
<table width=\"97%\"><tr><td class=\"rowhead\">下载</td><td class=\"rowfollow\"><a class=\"index\" href=\"download.php?id=479362\">[HD-Kylin].Cinderella.torrent</a></td></tr></table>\
</body></html>";

    /// 藏宝阁 43994：同一套审核图标，这条标的是 `title="通过"`
    const CANGBAO_APPROVED: &str = "<html><head><title>藏宝阁 :: 种子详情 \"Quick Change 1990 BDRip X264 iNT-TLF\"</title></head><body>\
<table><tr><td><b><a href=\"messages.php\"><font color=\"white\">你有1条新短讯！点击查看</font></a></b></td></tr></table><br />\
<h1 align=\"center\" id=\"top\">Quick Change 1990 BDRip X264 iNT-TLF&nbsp;&nbsp;&nbsp; <b>[<font class='free' >免费</font>]</b> <font color='#0000FF'>剩余时间：<span title=\"2026-11-25 14:44:48\">18时27分钟</span></font><span style=\"margin-left: 6px\" title=\"通过\"><svg viewBox=\"0 0 1413 1024\" width=\"16\" height=\"16\"><path d=\"M1381.8\" fill=\"#1afa29\"/></svg></span></h1>\
<table><tr><td class=\"rowhead\">下载</td><td class=\"rowfollow\"><a class=\"index\" href=\"download.php?id=43994\">x.torrent</a></td></tr></table>\
</body></html>";

    /// sbpt 21683：没有审核标记。标题带 H&R 图标（`title="H&R"`）、书签带 `title="收藏"`，
    /// 上方还有考核结果“結果：通過！”
    const SBPT_CLEAR: &str = "<html><head><title>SB :: 种子详情 \"Agatha Christies Poirot S01-S13 Collection Bluray 1080p x265 10bit MNHD-FRDS\"</title></head><body>\
<table><tr><td><b><a href=\"rules.php\"><font>要求：8 , 當前：32, 結果：通過！</font></a></b></td></tr></table><br />\
<h1 align=\"center\" id=\"top\">Agatha Christies Poirot S01-S13 Collection Bluray 1080p x265 10bit MNHD-FRDS<img class=\"hitandrun\" src=\"pic/trans.gif\" alt=\"H&R\" title=\"H&R\" /></h1>\
<table width=\"97%\"><tr><td class=\"rowhead\">下載</td><td class=\"rowfollow\"><a class=\"index\" href=\"download.php?id=21683\">[SB].torrent</a>&nbsp;&nbsp;<a id=\"bookmark0\" href=\"javascript: bookmark(21683,0);\"><img class=\"delbookmark\" src=\"pic/trans.gif\" title=\"收藏\" /></a></td></tr></table>\
</body></html>";

    #[test]
    fn review_mark_on_real_page_shapes() {
        assert_eq!(
            review_mark(
                TEY_PENDING,
                "The Disguiser Complete HDTV 1080p AAC x264-JING"
            ),
            Some((ReviewMark::Pending, "未审".to_string()))
        );
        assert_eq!(
            review_mark(QILIN_REJECTED, "Cinderella Man 2005 1080p x265-BeiTai"),
            Some((
                ReviewMark::Rejected,
                "审核不通过，原因：缺少海报或截图（图片少于 2 张）".to_string()
            ))
        );
        assert_eq!(
            review_mark(CANGBAO_APPROVED, "Quick Change 1990 BDRip X264 iNT-TLF"),
            Some((ReviewMark::Approved, "通过".to_string()))
        );
        // 导航里的“审核中心”、考核结果的“通過”、H&R / 收藏的 title 都不是审核标记
        assert_eq!(
            review_mark(
                SBPT_CLEAR,
                "Agatha Christies Poirot S01-S13 Collection Bluray 1080p x265 10bit MNHD-FRDS"
            ),
            None
        );
    }

    #[test]
    fn review_mark_order_pending_before_rejected() {
        // “尚未通过审核”里含“未通过审核”：必须判成还没审，不能判成没过
        assert_eq!(
            review_mark_in("对不起，该种子尚未通过审核，请稍后再试。"),
            Some((ReviewMark::Pending, "尚未通过审核".to_string()))
        );
        // 带“请稍后再试”的“未通过审核”也还没定
        assert_eq!(
            review_mark_in("该种子未通过审核，请稍后再试"),
            Some((ReviewMark::Pending, "审核中".to_string()))
        );
        assert_eq!(
            review_mark_in("审核不通过，原因：缺少海报或截图"),
            Some((
                ReviewMark::Rejected,
                "审核不通过，原因：缺少海报或截图".to_string()
            ))
        );
        // 明写的“不通过”压过含糊的“稍后”：改完再提交的话术，不是还在审
        assert_eq!(
            review_mark_in(
                "审核不通过，原因：缺少海报或截图（图片少于 2 张）。请修改后稍后再提交审核。"
            ),
            Some((
                ReviewMark::Rejected,
                "审核不通过，原因：缺少海报或截图（图片少于 2 张）".to_string()
            ))
        );
        // 青蛙拒绝页同时说“修改完成后会自动进入待审队列”：拒绝要压过这句
        assert_eq!(
            review_mark_in(
                "审核不通过，原因：MediaInfo 残缺，未检测到任何音频轨。修改完成后会自动进入待审队列，耐心等待即可。"
            ),
            Some((
                ReviewMark::Rejected,
                "审核不通过，原因：MediaInfo 残缺，未检测到任何音频轨".to_string()
            ))
        );
        // 导航项“审核中心”不是“审核中”
        assert_eq!(review_mark_in("首页 论坛 审核中心 一键审种"), None);
    }

    #[test]
    fn review_mark_attribute_and_split_forms() {
        // 同一套审核图标的不通过形态（只认属性形式）
        assert_eq!(
            review_mark_in("<span class=\"x\" title=\"不通过\"><svg/></span>"),
            Some((ReviewMark::Rejected, "不通过".to_string()))
        );
        // 实体写法的未审
        assert_eq!(
            review_mark_in("<span title=\"&#26410;&#23457;\"><svg/></span>"),
            Some((ReviewMark::Pending, "未审".to_string()))
        );
        // 正文里的“通过”和繁体“通過”都不算通过，只有 title="通过" 算
        assert_eq!(review_mark_in("考核结果：通过！"), None);
        assert_eq!(review_mark_in("結果：通過！"), None);
        assert_eq!(
            review_mark_in("<span title=\"通过\"><svg/></span>"),
            Some((ReviewMark::Approved, "通过".to_string()))
        );
        // 原因被标签拆开：退到整句
        assert_eq!(
            review_mark_in("提示：<b>审核不通过</b>，原因：缺图，请补图后再提交。后面是别的话"),
            Some((
                ReviewMark::Rejected,
                "审核不通过，原因：缺图，请补图后再提交".to_string()
            ))
        );
    }

    #[test]
    fn review_mark_banned_badge_counts_as_rejected() {
        // 标题旁的 `(禁止)` / `（禁止）` 徽标是审核没过，不能当成正常做种
        for badge in ["(禁止)", "（禁止）"] {
            let page = format!("Some Title 2024 1080p {badge}");
            let mark = review_mark_in(&page);
            assert_eq!(
                mark.map(|(m, _)| m),
                Some(ReviewMark::Rejected),
                "{badge} 应判审核被拒"
            );
        }
        // 详情页标题区域里的禁徽标也要认
        let html = "<html><body><h1>Some Title 2024 1080p (禁止)</h1>\
            <table><tr><td><a href=\"download.php?id=9\">t.torrent</a></td></tr></table></body></html>";
        assert_eq!(
            review_mark(html, "Some Title 2024 1080p").map(|(m, _)| m),
            Some(ReviewMark::Rejected)
        );
        // 正文明写“审核不通过”仍是拒绝
        assert_eq!(
            review_mark_in("审核不通过，原因：缺少海报或截图").map(|(m, _)| m),
            Some(ReviewMark::Rejected)
        );
    }

    #[test]
    fn review_mark_outside_title_area_does_not_count() {
        // 发布名不在页面上（比如回的是首页）：别的种子的“未审”角标不能算到这条头上
        let listing = format!(
            "<html><head><title>首页</title></head><body><h1>最新种子</h1><p>{}</p>\
             {SBPT_CLEAR}{CANGBAO_APPROVED}{TEY_PENDING}</body></html>",
            "这里是站点公告和列表页的长篇文字，和这条种子没有关系。".repeat(12)
        );
        assert_eq!(review_mark(&listing, "Some Other Release 2024 1080p"), None);
        // 发布名里的 `&` 在页面上是 `&amp;`：照样能定位到标题那一段
        let amp = "<html><body><div id=\"nav\">首页 一键审种</div>\
            <h1 id=\"top\">Tom &amp; Jerry 2020 1080p WEB-DL<span title=\"未审\"><svg/></span></h1>\
            <table><tr><td><a href=\"download.php?id=9\">t.torrent</a></td></tr></table></body></html>";
        assert_eq!(
            review_mark(amp, "Tom & Jerry 2020 1080p WEB-DL"),
            Some((ReviewMark::Pending, "未审".to_string()))
        );
    }

    #[test]
    fn no_such_torrent_page_is_recognized() {
        // 熊猫对不存在的 id 回的是 HTTP 200 报错页
        assert_eq!(
            no_such_torrent_mark("<table><tr><td class=\"text\">错误</td></tr><tr><td class=\"text\">没有该ID的种子</td></tr></table>"),
            Some("没有该ID的种子")
        );
        assert_eq!(no_such_torrent_mark(TEY_PENDING), None);
    }

    #[test]
    fn url_encode_unreserved() {
        assert_eq!(url_encode("Black Lagoon"), "Black%20Lagoon");
        assert_eq!(url_encode("a.b-c_d~e"), "a.b-c_d~e");
        assert_eq!(url_encode("黑礁"), "%E9%BB%91%E7%A4%81");
    }

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../Tests/BoxSendTests/Fixtures")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    #[test]
    fn human_size_requires_explicit_unit() {
        let gib = 1024f64.powi(3);
        let bytes = |text: &str| parse_human_size_display(text).map(|s| s.bytes);
        // 数字前可以带不含数字的标签，单位可以和数字粘连
        for text in ["77.43 GB", "大小：77.43 GB", "77.43GB", "大小：77.43GB"] {
            assert_eq!(bytes(text), Some((77.43 * gib).round() as i64), "{text}");
        }
        assert_eq!(bytes("1,024 MB"), Some(1024 * 1024 * 1024));
        // 纯数字是日期、做种数、分辨率，不是大小
        for text in [
            "8549",
            "上传于 2026.11.23",
            "做种 12",
            "2024-11-23 5",
            "1080p",
            "10bit",
            "86189256212 bytes",
            "S01E02 2",
        ] {
            assert_eq!(bytes(text), None, "{text}");
        }
    }

    #[test]
    fn page_size_prefers_labelled_cell() {
        let gib = 1024f64.powi(3);
        // 标准 NexusPHP：大小和类型、媒介挤在同一格，远超 40 字；页面里还有纯数字和 MediaInfo 表格
        let html = r#"<html><body>
        <span>8549</span>
        <div class="all-mediainfo"><div class="base-mediainfo"><table><tbody>
        <tr><td>Container: Matroska</td></tr><tr><td>Size: 7.99 GiB</td></tr>
        </tbody></table></div></div>
        <table><tr><td class="rowfollow"><b><b>大小：</b></b>77.43 GB&nbsp;&nbsp;&nbsp;<b>类型:</b>动漫 / Anime&nbsp;&nbsp;<b>媒介:</b>Remux</td></tr></table>
        </body></html>"#;
        assert_eq!(
            page_size_display(html).unwrap().bytes,
            (77.43 * gib).round() as i64
        );

        // MediaInfo 文本块和文件列表（每行一个文件的体积）都不是种子总大小
        let html = r#"<html><body>
        <div>General<br>Complete name : Show.S01E01.mkv<br><span>Size: 80.27 GiB</span><br>Duration : 24 min</div>
        <table><tr><td>01.mkv</td><td>82,196.48 MB</td></tr><tr><td>02.mkv</td><td>300.00 MB</td></tr></table>
        <table><tr><td class="rowhead">大小</td><td>33.19 GB</td></tr></table>
        </body></html>"#;
        assert_eq!(
            page_size_display(html).unwrap().bytes,
            (33.19 * gib).round() as i64
        );

        // 只有计数、日期、分辨率：取不到就是取不到，不能编一个字节数出来
        let counts =
            r#"<html><body><span>4</span><b>1080p</b><td>上传于 2026.11.23</td></body></html>"#;
        assert_eq!(page_size_display(counts), None);
    }

    #[test]
    fn fixture_detail_pages_report_the_labelled_size() {
        let gib = 1024f64.powi(3);
        // 真实站点详情页：大小标签都和类型、媒介挤在同一格，以前取到的是纯数字或文件列表里的单文件体积
        for (name, labelled) in [
            ("detail-crabpt-theirs.html", 77.43),
            ("detail-qilin-theirs.html", 77.43),
            ("detail-hdclone-mine.html", 77.43),
            ("detail-ubits-theirs.html", 77.43),
            ("hdsky-detail.html", 33.19),
            ("pter-detail.html", 1.89),
            ("zmpt-details-115285-webdl.html", 14.15),
        ] {
            let got = page_size_display(&fixture(name))
                .unwrap_or_else(|| panic!("{name} must report its labelled size"));
            assert_eq!(got.bytes, (labelled * gib).round() as i64, "{name}");
        }
    }

    #[test]
    fn fixture_detail_pages_never_report_bare_counts() {
        // 没有「大小」标签的版式可以取不到（取不到就不参与大小比对），但绝不能把计数当成字节数
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../Tests/BoxSendTests/Fixtures");
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("detail") && n.ends_with(".html"))
            .collect();
        assert!(
            names.len() >= 14,
            "expected the saved detail pages, got {names:?}"
        );
        for name in names {
            if let Some(size) = page_size_display(&fixture(&name)) {
                assert!(
                    size.bytes >= 1024 * 1024,
                    "{name} reported {} bytes",
                    size.bytes
                );
            }
        }
    }

    #[test]
    fn page_size_ignores_header_stats_and_description_mediainfo() {
        let gib = 1024f64.powi(3);
        // hdvideo 78856 实测结构：页眉先是账户统计，信息表下半截的简介里带 MediaInfo
        let html = r#"<html><body>
        <table><tr><td>上传量:1.353 TB</td><td>下载量:30.85 GB</td><td>4.945 TB 100.15 GB 50.557</td></tr></table>
        <table border="1"><tr><td class="rowhead">基本信息</td>
        <td class="rowfollow"><b><b>大小：</b></b>11.56 GB&nbsp;&nbsp;&nbsp;<b>类型:</b>电影&nbsp;&nbsp;<b>媒介:</b>Encode</td></tr>
        <tr><td class="rowhead">简介</td><td class="rowfollow"><div>General<br>Complete name : Movie.2024.mkv<br>
        <span>Size: 80.27 GiB</span><br>Duration : 1 h 30 min<br>Bit rate : 30.0 Mb/s</div></td></tr></table>
        </body></html>"#;
        assert_eq!(
            page_size_display(html).unwrap().bytes,
            (11.56 * gib).round() as i64
        );

        // 没有大小标签、只有页眉账户统计：宁可取不到
        let stats = r#"<html><body><table><tr><td>上传量:1.353 TB</td><td>4.945 TB 100.15 GB</td></tr></table></body></html>"#;
        assert_eq!(page_size_display(stats), None);

        // ubits 实测结构：MediaInfo 逐行渲染成表格、没有 mediainfo 类名，但整块以 General 开头
        let html = r#"<html><body>
        <table><tbody><tr><td>General:</td></tr><tr><td>Container: Matroska</td></tr><tr><td>Size: 7.99 GiB</td></tr></tbody></table>
        <table><tr><td class="rowhead">大小</td><td>33.19 GB</td></tr></table>
        </body></html>"#;
        assert_eq!(
            page_size_display(html).unwrap().bytes,
            (33.19 * gib).round() as i64
        );
    }

    #[test]
    fn download_notice_form_parsed() {
        // sbpt 实测：action="?" POST 回当前页；checked 的 hidenotice 入字段
        let html = r#"<html><body><form action="?" method="post">
            <input type="checkbox" name="hidenotice" id="hidenotice" value="1" checked="checked" />
            <input type="hidden" name="id" value="21653" />
            <input type="hidden" name="type" value="firsttime" />
            <input type="checkbox" name="unchecked" value="9" />
            <input type="submit" name="submit" id="continuedownload" value="下載種子檔案" />
        </form></body></html>"#;
        let url = Url::parse("https://sbpt.link/downloadnotice.php?torrentid=21653&type=firsttime")
            .unwrap();
        let (action, fields) = parse_download_notice_form(html, &url).unwrap();
        assert_eq!(action, url.as_str());
        assert!(fields.contains(&("hidenotice".to_string(), "1".to_string())));
        assert!(fields.contains(&("id".to_string(), "21653".to_string())));
        assert!(fields.contains(&("type".to_string(), "firsttime".to_string())));
        assert!(fields.contains(&("submit".to_string(), "下載種子檔案".to_string())));
        assert!(
            !fields.iter().any(|(k, _)| k == "unchecked"),
            "未勾选的 checkbox 不提交"
        );
        // 相对 action 按当前页解析
        let html2 = html.replace("action=\"?\"", "action=\"confirm.php\"");
        let (action2, _) = parse_download_notice_form(&html2, &url).unwrap();
        assert_eq!(action2, "https://sbpt.link/confirm.php");
        // 没有确认按钮的页面不当确认页
        assert!(parse_download_notice_form(
            "<form action=\"?\"><input name=\"id\" value=\"1\"></form>",
            &url
        )
        .is_none());
    }
}
