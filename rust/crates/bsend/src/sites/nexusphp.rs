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
use scraper::{Html, Selector};
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
    pub user_agent: String,
    client: Client,
}

/// 检索命中：站内已有这条种子的详情页
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub detail_url: String,
    pub title: String,
}

/// 详情页解析结果：发布名 + 下载链接
#[derive(Debug, Clone)]
pub struct TorrentPage {
    pub name: String,
    pub download_url: String,
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
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("构建 HTTP 客户端失败")?;
        Ok(Self {
            site_id: site_id.to_string(),
            site_name: site_name.to_string(),
            base,
            search_url_tmpl: search_url_tmpl
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            cookie: cookie.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            user_agent: DEFAULT_UA.to_string(),
            client,
        })
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
        if ["login", "enter.php", "signin", "auth/login"]
            .iter()
            .any(|hint| final_url.contains(hint))
        {
            return Err(anyhow!("被跳到登录页（cookie 可能已失效）：{url}"));
        }
        Ok(resp)
    }

    /// 站内检索找回已存在种子。`relaxed = true` 放宽到「按词元包含」判定：结果页常把长标题
    /// 截断（…LPCM 2.0 2Audios..）、或在同名标题里插备注，严格互含判不出来。只在站点已经
    /// 回答「已存在」、只差找回链接时使用；上传前查重仍走严格判定。
    pub fn search(
        &self,
        name: &str,
        subtitle: &str,
        relaxed: bool,
        imdb: &str,
    ) -> Result<Option<SearchHit>> {
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
            let url = self.build_search_url(&tmpl, &q, imdb);
            let resp = self.get(&url, self.base.as_str())?;
            let status = resp.status().as_u16();
            let html = resp.text().unwrap_or_default();
            if status != 200 {
                let head: String = html.chars().take(120).collect();
                return Err(anyhow!("HTTP {status} 检索 {url}：{head}"));
            }
            if NO_RESULT_MARKERS.iter().any(|m| html.contains(m)) {
                continue;
            }
            // 结果行 = 详情链接的锚文本（各站搜索结果名称在 <a href="details.php?id=..">名称</a> 内）。
            // 用中文副标题查的时候，结果行写的还是英文发布名：发布名也参与复核
            let names = match_names(&target, name);
            let base = Url::parse(&url).unwrap_or_else(|_| self.base.clone());
            if let Some(hit) = search_name_in_results(&html, &names, &base, relaxed) {
                return Ok(Some(hit));
            }
        }
        Ok(None)
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
        let resp = self.get(detail_url, self.base.as_str())?;
        let status = resp.status().as_u16();
        let html = resp.text().unwrap_or_default();
        if status == 404 {
            // 401/403 已在 get 里按 cookie 失效拦下；这里只剩详情页本身不存在
            return Err(anyhow!(
                "详情页不存在（HTTP 404，种子可能已被删除）：{detail_url}"
            ));
        }
        if status != 200 {
            let head: String = html.chars().take(120).collect();
            return Err(anyhow!("HTTP {status} 抓详情 {detail_url}：{head}"));
        }
        let base = Url::parse(detail_url).unwrap_or_else(|_| self.base.clone());
        parse_torrent_page(&html, detail_url, &base)
            .ok_or_else(|| anyhow!("详情页解析不出标题/下载链接：{detail_url}"))
    }

    /// 下载 .torrent（referer 用详情页，站点的防盗链检查需要）
    pub fn download_torrent(&self, page: &TorrentPage, referer: &str) -> Result<Vec<u8>> {
        let resp = self.get(&page.download_url, referer)?;
        let status = resp.status().as_u16();
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
            let notice = download_notice(&text);
            let head: String = text.chars().take(80).collect();
            let reason = if notice.is_empty() {
                format!("下载链接返回的不是种子文件（{} bytes，开头：{head}），可能 cookie 失效或下载链接错误", data.len())
            } else {
                format!("站点提示：{notice}")
            };
            return Err(anyhow!("{reason}：{}", page.download_url));
        }
        Ok(data)
    }
}

/// 检索报错是不是站点在限流（429/503）：调用方据此给该站间隔退避，别按原节奏接着敲
/// （PT 站对无视限流的行为会警告甚至封号）。Swift 版 `SiteThrottle.penalize` 预留了
/// 同一机制但从未接线，Rust 版这里把它接上。
pub fn error_is_rate_limited(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}");
    msg.contains("HTTP 429") || msg.contains("HTTP 503")
}

// ---------- 查询词阶梯 ----------

/// 候选 (查询词, 匹配基准)：全名 → 去组标签名 → 去标点词元查询 → 前 32 字截短 → 副标题（中文名）。
/// 已有种子的组标签/细节/语言可能不同，逐级放宽；严格模式只取前三条（命中要求都高，多查白等）。
pub fn query_pairs(name: &str, subtitle: &str, relaxed: bool) -> Vec<(String, String)> {
    let mut pairs = vec![(name.to_string(), name.to_string())];
    // 名字末段的组标签（-GROUP）：位置在 10 字符之后才剥，避免把标题里正常的连字符段当组标签
    if let Some(dash) = name.rfind('-') {
        if name[..dash].chars().count() > 10 {
            pairs.push((name[..dash].to_string(), name.to_string()));
        }
    }
    // 站点检索多是按词匹配，标题里的点/连字符会把一个词劈成两半：站内把 Blu-ray 写成
    // BluRay 时整名查询一条也查不到。补一条「去标点、只取前几个词」的查询词，
    // 命中与否仍由后面的名称匹配判，不会因为查询变宽就认错种子。
    if let Some(soft) = word_query(name, 6, 3, 60) {
        pairs.push((soft, name.to_string()));
    }
    if name.chars().count() > 32 {
        pairs.push((name.chars().take(32).collect(), name.to_string()));
    }
    // 副标题截到第一个括号类字符（副标题常带【中字】一类的备注尾巴）
    let mut sub = subtitle.to_string();
    for pat in ['[', '【', '（', '('] {
        if let Some(i) = sub.find(pat) {
            sub.truncate(i);
        }
    }
    let sub = sub.trim();
    if sub.chars().count() >= 6 && sub != name {
        pairs.push((sub.to_string(), sub.to_string()));
    }
    if !relaxed {
        pairs.truncate(3);
    }
    pairs
}

/// 参与名称复核的名字：查询词本身 + 发布名（归一化后同一个就不用列两遍）
pub fn match_names(queried: &str, release_name: &str) -> Vec<String> {
    if normalize_search_name(queried) == normalize_search_name(release_name) {
        vec![release_name.to_string()]
    } else {
        vec![queried.to_string(), release_name.to_string()]
    }
}

/// 查询词：只取前几个词、去掉所有标点（`wordQuery`）
pub fn word_query(
    name: &str,
    max_words: usize,
    min_words: usize,
    max_length: usize,
) -> Option<String> {
    let words: Vec<String> = name_tokens(name)
        .into_iter()
        .filter(|w| w.chars().count() >= 3)
        .collect();
    if words.len() < min_words {
        return None;
    }
    let q = words
        .iter()
        .take(max_words)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if q.chars().count() < 10 {
        return None;
    }
    Some(q.chars().take(max_length).collect())
}

// ---------- 名称判定 ----------

/// 两个名称是否指同一种子：归一化互含为准，`token_tolerant` 时补一条词元包含判定
pub fn search_names_match(candidate: &str, release_name: &str, token_tolerant: bool) -> bool {
    let c = normalize_search_name(candidate);
    let t = normalize_search_name(release_name);
    if c.chars().count() < 8 || t.chars().count() < 8 {
        return false;
    }
    if c.contains(&t) || (t.chars().count() >= 15 && t.contains(&c)) {
        return true;
    }
    if !token_tolerant {
        return false;
    }
    name_tokens_contained(release_name, candidate)
}

/// 发布名的每个词元都能在候选名里找到（允许缺 1 个非关键字元）。
/// 站点常见差异：截断尾部（丢组名）、标题里多插 2Audios 一类备注——都不改变「同一种子」的事实。
pub fn name_tokens_contained(name: &str, other: &str) -> bool {
    let target = comparable_name_tokens(name);
    let cand: HashSet<String> = comparable_name_tokens(other).into_iter().collect();
    if target.len() < 4 || cand.len() < 4 {
        return false;
    }
    let missing: Vec<&String> = target.iter().filter(|t| !cand.contains(*t)).collect();
    missing.len() <= 1 && !missing.iter().any(|t| is_discriminating_token(t))
}

/// 发布名分词：连字符不算分隔符（Blu-ray / WEB-DL / DTS-HD 各站写法不一），
/// 丢掉长度 <2 的词元（"2.0" 拆出的 "2"、"0" 是噪声）
pub fn name_tokens(s: &str) -> Vec<String> {
    let joined = strip_tags(s).replace('-', "");
    joined
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2)
        .map(|t| t.to_string())
        .collect()
}

/// 比对用的词元（Lagoon 压成 lagon 就查不到站点了，所以只在比对时压）
pub fn comparable_name_tokens(s: &str) -> Vec<String> {
    name_tokens(s)
        .into_iter()
        .map(|t| fold_doubled_letters(&t))
        .collect()
}

/// 站点对同一介质常有两种写法：Blu-ray / Blu Ray / BluRay。去掉分隔符后前两种都是
/// bluray，最后一种成了 blurray，「关键字元对不上」会把同一种子判成两个版本。
/// 比对用的串里压掉连续重复的字母（两边同样处理；数字不压，2006、1080p 保持原样）。
pub fn fold_doubled_letters(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev = '\0';
    for c in s.chars() {
        if c == prev && c.is_ascii_alphabetic() {
            continue;
        }
        out.push(c);
        prev = c;
    }
    out
}

/// 关键字元：季/集/年份/分辨率/编码音频对不上，说明是两个不同种子，不能算「已存在」
const DISCRIMINATING_TOKENS: &[&str] = &[
    "x264",
    "x265",
    "hevc",
    "h264",
    "h265",
    "avc",
    "vc1",
    "mpeg2",
    "av1",
    "vp9",
    "vp8",
    "xvid",
    "divx",
    "prores",
    "bluray",
    "remux",
    "webdl",
    "webrip",
    "hdtv",
    "bdrip",
    "dvdrip",
    "dvd",
    "hdcam",
    "flac",
    "lpcm",
    "pcm",
    "wav",
    "aac",
    "ac3",
    "eac3",
    "dts",
    "dtshd",
    "dtsma",
    "truehd",
    "atmos",
    "mp3",
    "opus",
    "hdr10",
    "dolbyvision",
    "imax",
    "3d",
];

pub fn is_discriminating_token(t: &str) -> bool {
    if DISCRIMINATING_TOKENS.contains(&t) {
        return true;
    }
    is_shape_token(t)
}

/// 形状字元：`^(?:s\d{1,2}|e\d{1,3}|\d{1,2}x\d{1,3}|19\d{2}|20\d{2}|\d{3,4}p|4k|uhd|sdr|hdr)$`
/// （Swift 版是正则，这里手写等价判定——不引 regex 依赖）
fn is_shape_token(t: &str) -> bool {
    if matches!(t, "4k" | "uhd" | "sdr" | "hdr") {
        return true;
    }
    let b = t.as_bytes();
    if b.is_empty() {
        return false;
    }
    let digits = |s: &[u8]| s.iter().all(|c| c.is_ascii_digit());
    // s\d{1,2} / e\d{1,3}
    if ((b[0] == b's' && b.len() <= 3) || (b[0] == b'e' && b.len() <= 4))
        && b.len() >= 2
        && digits(&b[1..])
    {
        return true;
    }
    // 19\d{2} | 20\d{2}
    if b.len() == 4 && (t.starts_with("19") || t.starts_with("20")) && digits(&b[2..]) {
        return true;
    }
    // \d{3,4}p
    if (b.len() == 4 || b.len() == 5) && b[b.len() - 1] == b'p' && digits(&b[..b.len() - 1]) {
        return true;
    }
    // \d{1,2}x\d{1,3}
    if let Some(x) = b.iter().position(|c| *c == b'x') {
        let tail = b.len() - x - 1;
        if (1..=2).contains(&x) && (1..=3).contains(&tail) && digits(&b[..x]) && digits(&b[x + 1..])
        {
            return true;
        }
    }
    false
}

/// 搜索名称归一化：去标签/实体、小写、只留字母数字（CJK 按字母保留）、压连续重复字母
pub fn normalize_search_name(s: &str) -> String {
    let t = decode_entities(&strip_tags(s)).to_lowercase();
    let filtered: String = t.chars().filter(|c| c.is_alphanumeric()).collect();
    fold_doubled_letters(&filtered)
}

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

/// 搜索结果页里按名称匹配已存在种子：先按归一化互含，`relaxed` 时再按词元包含放宽。
/// 结果行的 title 属性也参与判定（站内常截断锚文本，完整名只在 title 里）。
pub fn search_name_in_results(
    html: &str,
    release_names: &[String],
    base: &Url,
    relaxed: bool,
) -> Option<SearchHit> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("a").unwrap();
    // 扁平候选列表：(href, 文本)，title 更完整时在同锚后面补一条——与 Swift anchorText 的顺序一致
    let mut candidates: Vec<(String, String)> = Vec::new();
    for el in doc.select(&sel) {
        let Some(href) = el.value().attr("href") else {
            continue;
        };
        if !is_detail_href(href) {
            continue;
        }
        let text: String = el.text().collect::<String>().trim().to_string();
        candidates.push((href.to_string(), text.clone()));
        if let Some(t) = el.value().attr("title") {
            let t = t.trim();
            // title 至少 12 字符且比锚文本完整才补（{12,} 且 count > text.count）
            if t.chars().count() >= 12 && t.chars().count() > text.chars().count() {
                candidates.push((href.to_string(), t.to_string()));
            }
        }
    }
    let hit = candidates
        .iter()
        .find(|(_, text)| {
            release_names
                .iter()
                .any(|n| search_names_match(text, n, false))
        })
        .or_else(|| {
            if !relaxed {
                return None;
            }
            candidates.iter().find(|(_, text)| {
                release_names
                    .iter()
                    .any(|n| search_names_match(text, n, true))
            })
        })?;
    Some(SearchHit {
        detail_url: resolve_url(&hit.0, base),
        title: hit.1.clone(),
    })
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
    // 1) <h1 id="top">（LuckPT 等改版 NexusPHP）
    if let Ok(sel) = Selector::parse("h1") {
        for el in doc.select(&sel) {
            if el.value().attr("id") == Some("top") {
                let t: String = collapse_spaces(&el.text().collect::<String>());
                if !t.is_empty() {
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
    Some(TorrentPage { name, download_url })
}

fn collapse_spaces(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let hit = search_name_in_results(html, &names, &base, false).unwrap();
        assert_eq!(hit.detail_url, "https://hdsky.me/details.php?id=1002");
        // 截断的锚文本在 title 里补全：严格互含判不出时靠 title 命中
        let html2 = r#"<a href="details.php?id=1003" title="Black.Lagoon.S01.1080p.Blu-ray.DTS-HD.MA.5.1-GROUP">Black.Lagoon…</a>"#;
        let hit2 = search_name_in_results(html2, &names, &base, false).unwrap();
        assert_eq!(hit2.detail_url, "https://hdsky.me/details.php?id=1003");
        // 查不到时 None
        assert!(search_name_in_results(
            "<a href='details.php?id=1'>Nothing</a>",
            &names,
            &base,
            false
        )
        .is_none());
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

    #[test]
    fn url_encode_unreserved() {
        assert_eq!(url_encode("Black Lagoon"), "Black%20Lagoon");
        assert_eq!(url_encode("a.b-c_d~e"), "a.b-c_d~e");
        assert_eq!(url_encode("黑礁"), "%E9%BB%91%E7%A4%81");
    }
}
