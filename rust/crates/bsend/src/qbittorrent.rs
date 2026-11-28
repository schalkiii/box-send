//! qBittorrent Web API (v2) 客户端。与 Swift 版 `Downloader/QBittorrent.swift` 逐段对齐，
//! 保留它的全部兼容性补丁（同一条注释里写明为什么）：
//! - 登录 200+"Ok." 或 204（qB 5.2+ 已有会话）都算成功；
//! - `/torrents/files` 参数名 `hashes`→`hash` 两版都试，响应可能是 `[file]` 或 `[[file]]`；
//! - `/torrents/trackers` 参数名 `hash`→`infohash` 两版都试（qB 5.x 只认前者）。
//!
//! 单 tracker 策略（`doc/ARCHITECTURE.md` §3.3）：本客户端**没有**任何写 tracker
//! 的接口；同 hash 已存在的种子一律跳过、不改动。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, REFERER};
use serde_json::Value;

use crate::bencode;
use crate::lock_unpoisoned;
use crate::models::LocalTorrent;

/// qB `torrents/trackers` 里真实 http(s) tracker 的最小快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerInfo {
    pub url: String,
    /// qB 原始状态：0 未启用、1 未联系、2 已工作、3 未工作、4 报错。
    pub status: i32,
    pub message: String,
    pub num_seeds: i64,
    pub num_leeches: i64,
    pub num_downloaded: i64,
}

fn parse_tracker_entries(text: &str) -> Result<Vec<TrackerInfo>> {
    let Value::Array(arr) = serde_json::from_str::<Value>(text)? else {
        bail!("trackers 回包不是数组");
    };
    Ok(arr
        .into_iter()
        .filter_map(|t| {
            let url = t.get("url")?.as_str()?.trim();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return None;
            }
            let mut status = t.get("status").and_then(Value::as_i64).unwrap_or(-1) as i32;
            let mut message = t
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut num_seeds = t.get("num_seeds").and_then(Value::as_i64).unwrap_or(-1);
            let mut num_leeches = t.get("num_leeches").and_then(Value::as_i64).unwrap_or(-1);
            let mut num_downloaded = t
                .get("num_downloaded")
                .and_then(Value::as_i64)
                .unwrap_or(-1);
            // qB 5.3 的顶层 tracker 可能仍是 1（未联系），真正的连接在 endpoints 里：
            // 有些 endpoint 已经 status=2 并带回同伴数。只看顶层会把“已回包”误判成
            // “还没回包”，让种子永远卡在“校验中”。
            if status != 2 {
                if let Some(endpoint) =
                    t.get("endpoints")
                        .and_then(Value::as_array)
                        .and_then(|eps| {
                            eps.iter()
                                .find(|e| e.get("status").and_then(Value::as_i64) == Some(2))
                        })
                {
                    status = 2;
                    message = endpoint
                        .get("msg")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    num_seeds = endpoint
                        .get("num_seeds")
                        .and_then(Value::as_i64)
                        .unwrap_or(num_seeds);
                    num_leeches = endpoint
                        .get("num_leeches")
                        .and_then(Value::as_i64)
                        .unwrap_or(num_leeches);
                    num_downloaded = endpoint
                        .get("num_downloaded")
                        .and_then(Value::as_i64)
                        .unwrap_or(num_downloaded);
                }
            }
            Some(TrackerInfo {
                url: url.to_string(),
                status,
                message,
                num_seeds,
                num_leeches,
                num_downloaded,
            })
        })
        .collect())
}

/// 把 tracker 快照翻成一句人能直接写进审计/覆盖记录的话。
/// 只有 `status=2` 才是 tracker 真回包；`status=1` 只能写“没回包”，
/// 不能因为 qB 里 progress=1 就写成“站点已看见做种”。
pub fn tracker_status_note(infos: &[TrackerInfo]) -> String {
    if let Some(t) = infos.iter().find(|t| t.status == 2) {
        return format!(
            "tracker 已回包（做种 {} / 下载 {}）",
            t.num_seeds.max(0),
            t.num_leeches.max(0)
        );
    }
    if let Some(t) = infos.iter().find(|t| t.status == 4) {
        let why = if t.message.trim().is_empty() {
            "未知错误"
        } else {
            t.message.trim()
        };
        return format!("tracker 报错：{why}");
    }
    "tracker 还没回包".to_string()
}

/// 站点是否真的看见这条做种：只有 qB tracker 明确回报 `status=2` 才算。
/// `progress=1` 只证明本地校验完成，不能替代站点回包。
pub fn tracker_confirms_seeding(infos: &[TrackerInfo]) -> bool {
    infos.iter().any(|t| t.status == 2)
}

pub struct QBittorrent {
    client: Client,
    base_url: String,
    username: String,
    password: String,
    /// 已登录（SID 在 cookie store 里）。免得每个 API 调用都重打一次 /auth/login，
    /// 把对下载器的请求数翻倍
    logged_in: AtomicBool,
    /// `/torrents/files` 认哪个参数名（qB 5.3 只认 `hash`，4.x 认 `hashes`）。
    /// 命中一次就记住，别每次都先撞一发 400——首扫 4 万条种子就是 4 万次白跑。
    files_param: Mutex<Option<&'static str>>,
}

/// 推种结果：`id` 是 v1 info hash（.torrent 解不出时为空），`note` 给人看（已存在/补挂/限速未生效…），
/// `already` 区分「下载器里早有这个种」（409/补挂）与「本次新加」——新加即使带 note 也要走校验流程
#[derive(Debug, Clone)]
pub struct AddTorrentResult {
    pub id: String,
    pub note: String,
    pub already: bool,
}

/// boxsend 添加/接管的种子统一打这个标签：下载器里可一眼筛出「引擎辅种/转种」的条目
pub const BOXSEND_TAG: &str = "Boxsend";

/// `torrents/add` 的表单字段；单独抽出以便单测 qB 5.3 的 seedMode 语义。
fn add_form_fields(
    save_path: &str,
    category: Option<&str>,
    skip_checking: bool,
    up_limit: i64,
    paused: bool,
    content_layout: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut f = vec![
        ("savepath", save_path.to_string()),
        ("category", category.unwrap_or("").to_string()),
        // qB 5.3 / WebAPI 2.16 已移除 skip_checking，改用 seedMode：
        // 只做种、不先校验/下载。两个参数都发，兼容 4.x 与 5.x。
        (
            "skip_checking",
            if skip_checking { "true" } else { "false" }.to_string(),
        ),
        (
            "seedMode",
            if skip_checking { "true" } else { "false" }.to_string(),
        ),
        ("upLimit", up_limit.to_string()),
        // 校验期间 1 B/s 下载兜底：内容对不上时不会开始狂下（通过后由 setDownloadLimit 解除）
        ("dlLimit", "1".to_string()),
        // 启停三套参数名都发：5.x 认 stopped，4.3+/4.4+ 认 started / paused
        ("started", if paused { "false" } else { "true" }.to_string()),
        ("stopped", if paused { "true" } else { "false" }.to_string()),
        // 引擎加进来的种子统一打标签，便于在下载器里筛选「Boxsend 辅种/转种」
        ("tags", BOXSEND_TAG.to_string()),
    ];
    if let Some(layout) = content_layout {
        f.push(("contentLayout", layout.to_string()));
    }
    if paused {
        f.push(("paused", "true".to_string()));
    }
    f
}

impl QBittorrent {
    pub fn new(base_url: &str, username: &str, password: &str) -> Result<Self> {
        let client = Client::builder()
            .cookie_store(true)
            .timeout(Duration::from_secs(60))
            // 下载器是本机/局域网服务，走代理只会有害：实机上系统注册表残留
            // `ProxyServer=socks=127.0.0.1:9090`（ProxyEnable=0 但 reqwest 照样读），
            // 每次请求先去连那个不存在的 SOCKS，单次卡满 30 秒超时。
            .no_proxy()
            .build()
            .context("构建 HTTP 客户端失败")?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            username: username.to_string(),
            password: password.to_string(),
            logged_in: AtomicBool::new(false),
            files_param: Mutex::new(None),
        })
    }

    fn login(&self) -> Result<()> {
        // SID 已在 cookie store 里就别再登。会话失效（qB 重启回 403）由响应侧
        // 清标记兜底，下一次调用自动重登
        if self.logged_in.load(Ordering::Relaxed) {
            return Ok(());
        }
        let resp = self
            .client
            .post(format!("{}/api/v2/auth/login", self.base_url))
            .form(&[
                ("username", self.username.as_str()),
                ("password", self.password.as_str()),
            ])
            .send()
            .context("qBittorrent 登录请求没通，检查 url/网络")?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        // qBittorrent 5.2+: 客户端已有活动会话时返回 204（无内容），同样视为登录成功
        if (status == 200 && body.trim() == "Ok.") || status == 204 {
            // SID 已由 cookie store 自动保存
            self.logged_in.store(true, Ordering::Relaxed);
            return Ok(());
        }
        let head: String = body.chars().take(80).collect();
        Err(anyhow!(
            "qBittorrent 登录失败 (HTTP {status} {head})，检查 url/账号/密码"
        ))
    }

    /// 响应 403 多半是 SID 失效（qB 重启）：清登录标记，下一次调用自动重登
    fn session_died(&self, status: u16) {
        if status == 403 {
            self.logged_in.store(false, Ordering::Relaxed);
        }
    }

    pub fn test_connection(&self) -> Result<String> {
        self.login()?;
        let resp = self
            .client
            .get(format!("{}/api/v2/app/version", self.base_url))
            .headers(self.referer_headers())
            .send()
            .context("GET /app/version 没通")?;
        let status = resp.status().as_u16();
        let text = resp.text().unwrap_or_default();
        if status != 200 {
            self.session_died(status);
            return Err(anyhow!("HTTP {status} GET /app/version"));
        }
        // qB 5.x 起该端点回纯文本（`v5.3.0`），4.x 回 JSON（`{"version":"v4.6.2"}`）——两版都认
        let trimmed = text.trim();
        let version = if trimmed.starts_with('{') {
            serde_json::from_str::<Value>(trimmed)
                .ok()
                .and_then(|v| v.get("version").and_then(|x| x.as_str()).map(String::from))
                .unwrap_or_default()
        } else {
            trimmed.to_string()
        };
        if version.is_empty() {
            return Err(anyhow!("GET /app/version 回包是空的"));
        }
        Ok(format!("qBittorrent {version}，登录成功"))
    }

    fn get_json(&self, path: &str) -> Result<Value> {
        let resp = self
            .client
            .get(format!("{}{}", self.base_url, path))
            .headers(self.referer_headers())
            .send()
            .with_context(|| format!("GET {path} 没通"))?;
        let status = resp.status().as_u16();
        let text = resp.text().unwrap_or_default();
        if status != 200 {
            self.session_died(status);
            let head: String = text.chars().take(200).collect();
            return Err(anyhow!("HTTP {status} GET {path}: {head}"));
        }
        serde_json::from_str(&text).with_context(|| format!("GET {path} 回包不是 JSON"))
    }

    fn referer_headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&self.base_url) {
            h.insert(REFERER, v);
        }
        h
    }

    /// 全量种子列表。qB 的 `torrents/info` 单页最多 50,000 条；现役已接近
    /// 这个上限，只取一页会在超过后静默截断，覆盖判断可能把站上已有看成
    /// 没有。这里按 offset 翻页，直到某一页不足一页或没有新 hash 为止。
    fn all_torrents(&self) -> Result<Vec<Value>> {
        const PAGE: usize = 50_000;
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut offset = 0usize;
        loop {
            let path = format!("/api/v2/torrents/info?limit={PAGE}&offset={offset}");
            let page = self
                .get_json(&path)?
                .as_array()
                .cloned()
                .ok_or_else(|| anyhow!("torrents/info 回包不是数组"))?;
            let got = page.len();
            let before = out.len();
            for t in page {
                let key = t
                    .get("hash")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                if key.is_empty() || seen.insert(key) {
                    out.push(t);
                }
            }
            if got < PAGE {
                break;
            }
            if out.len() == before {
                eprintln!("警告：torrents/info 翻页没有取到新种子，停止在 offset={offset}");
                break;
            }
            offset = offset.saturating_add(got);
            if offset > 10_000_000 {
                eprintln!("警告：torrents/info 翻页超过 1000 万条，停止在 offset={offset}");
                break;
            }
        }
        if out.len() > PAGE {
            eprintln!("提示：torrents/info 已分页取回 {} 条", out.len());
        }
        Ok(out)
    }

    fn post_form_ignoring_errors(&self, path: &str, fields: &[(&str, String)]) {
        // Swift 版 setPaused/removeTorrent 用 try? 吞错（删一条已不存在的种子不算失败）；
        // 这里保持同样语义：尽力发出去，结果不抛
        let _ = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .headers(self.referer_headers())
            .form(fields)
            .send();
    }

    // ---- 辅种要读的三样：种子清单、文件清单、暂停/删除 ----

    /// 种子清单。取全量再逐条解析（`parse_torrent`），字段兼容全在解析里。
    pub fn list_torrents(&self) -> Result<Vec<LocalTorrent>> {
        self.login()?;
        let arr = self.all_torrents()?;
        Ok(arr.iter().filter_map(parse_torrent).collect())
    }

    /// 批量读文件清单：`concurrency` 个线程并发取。首扫描 4 万条种子 = 4 万次
    /// `/torrents/files` 请求，单线程串行要十几分钟；并发后压到分钟级。
    /// 取失败的 hash 不进结果——调用方按「拿不到清单」处理（不许拿残缺清单算身份）。
    /// `on_progress(已完成, 总数, 该条是否成功)` 在主线程按完成顺序回调。
    pub fn torrent_files_many(
        &self,
        hashes: &[String],
        concurrency: usize,
        mut on_progress: impl FnMut(usize, usize, bool),
    ) -> HashMap<String, Vec<(String, i64)>> {
        let total = hashes.len();
        if total == 0 {
            return HashMap::new();
        }
        // 先登一次：并发线程各自 login 会同时打 /auth/login
        let _ = self.login();
        let queue: Mutex<VecDeque<String>> = Mutex::new(hashes.iter().cloned().collect());
        let (tx, rx) = std::sync::mpsc::channel::<(String, Option<Vec<(String, i64)>>)>();
        let n = concurrency.clamp(1, 32).min(total);
        std::thread::scope(|scope| {
            for _ in 0..n {
                let tx = tx.clone();
                let queue = &queue;
                scope.spawn(move || loop {
                    let next = { lock_unpoisoned(queue).pop_front() };
                    let Some(h) = next else { break };
                    let files = self.torrent_files(&h).ok();
                    if tx.send((h, files)).is_err() {
                        break;
                    }
                });
            }
            drop(tx);
            let mut out = HashMap::with_capacity(total);
            let mut done = 0usize;
            for (h, files) in rx {
                done += 1;
                match files {
                    Some(f) => {
                        out.insert(h, f);
                        on_progress(done, total, true);
                    }
                    None => on_progress(done, total, false),
                }
            }
            out
        })
    }

    /// 文件清单。参数名两版都试（qB 5.3 实测只认 `hash`；4.x 认 `hashes`），
    /// 成功的那个参数名会被记住，后续调用不再先撞 400。
    /// 一过性失败（qB 忙时偶发 400/超时）重试一次——单次抖动不该让一格内容失败 7 天。
    pub fn torrent_files(&self, hash: &str) -> Result<Vec<(String, i64)>> {
        self.login()?;
        let mut last = String::new();
        for attempt in 0..2 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(300));
            }
            let preferred = *lock_unpoisoned(&self.files_param);
            let order: [&'static str; 2] = match preferred {
                Some("hash") => ["hash", "hashes"],
                _ => ["hashes", "hash"],
            };
            for param in order {
                let Ok(resp) = self
                    .client
                    .get(format!(
                        "{}/api/v2/torrents/files?{}={}",
                        self.base_url, param, hash
                    ))
                    .headers(self.referer_headers())
                    .send()
                else {
                    last = "请求没通".to_string();
                    continue;
                };
                let st = resp.status().as_u16();
                if st != 200 {
                    self.session_died(st);
                    last = format!("HTTP {st}（{param}）");
                    continue;
                }
                let Ok(text) = resp.text() else { continue };
                let Ok(obj) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                // 多 hash 查询时 qB 回 [[file,…],…]；单 hash 直接回 [file,…]
                let Some(files) = parse_files_value(&obj) else {
                    continue;
                };
                if !files.is_empty() {
                    *lock_unpoisoned(&self.files_param) = Some(param);
                    return Ok(files);
                }
            }
        }
        Err(anyhow!("读不到 {hash} 的文件清单（{last}）"))
    }

    /// 单条种子的完整 info 对象（progress/state/up_limit/…）
    fn torrent_info(&self, hash: &str) -> Option<Value> {
        let resp = self
            .client
            .get(format!(
                "{}/api/v2/torrents/info?hashes={}",
                self.base_url, hash
            ))
            .headers(self.referer_headers())
            .send()
            .ok()?;
        let st = resp.status().as_u16();
        if st != 200 {
            self.session_died(st);
            return None;
        }
        let text = resp.text().ok()?;
        let Value::Array(arr) = serde_json::from_str(&text).ok()? else {
            return None;
        };
        arr.into_iter().next()
    }

    /// 单条种子的记账（save_path/name/category…）：推种前按 hash 现场取，
    /// 保证「沿用已有种子的保存路径」用的是下载器当前值而不是索引里的旧值
    pub fn torrent_meta(&self, hash: &str) -> Result<Option<LocalTorrent>> {
        self.login()?;
        Ok(self.torrent_info(hash).and_then(|v| parse_torrent(&v)))
    }

    /// 全部种子的 v1 info hash 集合（小写）。上传前断言新种 hash 不在下载器里用。
    pub fn existing_hashes(&self) -> Result<HashSet<String>> {
        self.login()?;
        let arr = self.all_torrents()?;
        Ok(arr
            .iter()
            .filter_map(|t| t.get("hash")?.as_str())
            .map(|h| h.to_lowercase())
            .filter(|h| !h.is_empty())
            .collect())
    }

    /// 一条种子当前的 http(s) tracker 详情。qB 的 trackers 接口会带
    /// `** [DHT] **` / `** [PeX] **` / `** [LSD] **` 伪条目，按 scheme 过滤。
    /// `status` 是 qB 的原始值：2 表示 tracker 已回包，1 表示还没联系上，
    /// 4 表示 tracker 报错；只返回 URL 会让调用方把“挂回 qB”误当成“站点已看见”。
    pub fn tracker_info(&self, hash: &str) -> Result<Vec<TrackerInfo>> {
        self.login()?;
        for param in ["hash", "infohash"] {
            let Ok(resp) = self
                .client
                .get(format!(
                    "{}/api/v2/torrents/trackers?{}={}",
                    self.base_url, param, hash
                ))
                .headers(self.referer_headers())
                .send()
            else {
                continue;
            };
            let st = resp.status().as_u16();
            if st != 200 {
                self.session_died(st);
                continue;
            }
            let Ok(text) = resp.text() else { continue };
            if let Ok(arr) = parse_tracker_entries(&text) {
                return Ok(arr);
            }
        }
        Err(anyhow!("读不到 {hash} 的 tracker 列表"))
    }

    /// 一条种子当前的 http(s) tracker URL 列表（兼容只关心 URL 的旧调用方）。
    pub fn trackers(&self, hash: &str) -> Result<Vec<String>> {
        Ok(self
            .tracker_info(hash)?
            .into_iter()
            .map(|t| t.url)
            .collect())
    }

    /// 一条种子的 http(s) tracker 条数（单 tracker 策略挂种后必须 == 1）。
    pub fn tracker_count(&self, hash: &str) -> Result<usize> {
        Ok(self.trackers(hash)?.len())
    }

    /// 从下载器现有种子里，按站点 announce host 取一条完整 announce URL（带 passkey）。
    /// 用于把源种 announce 改写为目标站唯一一条；本地没有该站任何种子时返回 `None`。
    pub fn announce_for_hosts(&self, hosts: &[String]) -> Option<String> {
        self.login().ok()?;
        let arr = self.all_torrents().ok()?;
        for t in arr {
            let Some(u) = t.get("tracker").and_then(|v| v.as_str()) else {
                continue;
            };
            let u = u.trim();
            if !u.starts_with("http://") && !u.starts_with("https://") {
                continue;
            }
            let host = url::Url::parse(u).ok()?.host_str()?.to_lowercase();
            if announce_host_matches(&host, hosts) {
                return Some(u.to_string());
            }
        }
        None
    }

    /// 导出种子原始 .torrent（/api/v2/torrents/export；qB 4.6+）。
    /// 用途：P2 单 tracker 流程的源种取自 qB 导出；也算 pieces_hash 供站内查询。
    pub fn export_torrent(&self, hash: &str) -> Result<Vec<u8>> {
        self.login()?;
        let resp = self
            .client
            .get(format!(
                "{}/api/v2/torrents/export?hash={hash}",
                self.base_url
            ))
            .headers(self.referer_headers())
            .send()
            .context("GET torrents/export 没通")?;
        let status = resp.status().as_u16();
        if status != 200 {
            self.session_died(status);
            bail!("HTTP {status} 导出种子 {hash}");
        }
        let data = resp.bytes().context("读导出种子字节失败")?.to_vec();
        if bencode::info_hash_v1(&data).is_none() {
            bail!("导出内容不是有效 .torrent（{hash}）");
        }
        Ok(data)
    }

    /// 全部种子当前 tracker 的原始 URL（采集站点 passkey 用；只读一次，逐轮可缓存）
    pub fn all_tracker_urls(&self) -> Result<Vec<String>> {
        self.login()?;
        let arr = self.all_torrents()?;
        let mut out = Vec::new();
        for t in arr {
            if let Some(u) = t.get("tracker").and_then(|v| v.as_str()) {
                if !u.is_empty() {
                    out.push(u.to_string());
                }
            }
        }
        Ok(out)
    }

    /// 批量取成员种子的 (名字, 大小)：辅种名字兜底检索用（一次请求）。
    /// 读不到返回空表——调用方按「没有兜底候选」处理，不影响主流程。
    pub fn torrent_names(&self, hashes: &[String]) -> Vec<(String, i64)> {
        if hashes.is_empty() {
            return Vec::new();
        }
        let resp = self
            .client
            .get(format!(
                "{}/api/v2/torrents/info?hashes={}",
                self.base_url,
                hashes.join("|")
            ))
            .headers(self.referer_headers())
            .send()
            .ok();
        let Some(resp) = resp else { return Vec::new() };
        if resp.status().as_u16() != 200 {
            return Vec::new();
        }
        let Ok(text) = resp.text() else {
            return Vec::new();
        };
        let Ok(list) = serde_json::from_str::<Value>(&text) else {
            return Vec::new();
        };
        let Some(arr) = list.as_array() else {
            return Vec::new();
        };
        arr.iter()
            .filter_map(|t| {
                let name = t.get("name")?.as_str()?.to_string();
                let size = t.get("size").and_then(|v| v.as_i64()).unwrap_or(0);
                Some((name, size))
            })
            .collect()
    }

    /// 单条种子的落地路径（qB 的 content_path：多文件=内容根目录，单文件=文件全路径）
    pub fn torrent_content_path(&self, hash: &str) -> Option<String> {
        self.torrent_info(hash).and_then(|v| {
            v.get("content_path")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        })
    }

    /// 单条种子的完成度与状态（辅种等校验用；比整表拉一遍省）
    pub fn torrent_progress(&self, hash: &str) -> Result<Option<(f64, String)>> {
        self.login()?;
        Ok(self.torrent_info(hash).map(|t| {
            (
                t.get("progress").and_then(|v| v.as_f64()).unwrap_or(0.0),
                t.get("state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
        }))
    }

    /// 启停种子。qB 5.x 把 pause/resume 改名 stop/start（5.0 起旧名 404，实机 qB 5.3 验证）——
    /// 只用旧名会静默失败：校验完的种子一直停着不作种、停止态的种子永远不会开始校验。
    /// 新名优先，回 404/405 再试旧名（qB 4.x）。
    pub fn set_paused(&self, hashes: &[&str], paused: bool) -> Result<()> {
        self.login()?;
        let body = [("hashes", hashes.join("|"))];
        let (primary, legacy) = if paused {
            ("stop", "pause")
        } else {
            ("start", "resume")
        };
        if let Some(st) = self.post_form_status(&format!("/api/v2/torrents/{primary}"), &body) {
            if st != 404 && st != 405 {
                return Ok(());
            }
        }
        self.post_form_ignoring_errors(&format!("/api/v2/torrents/{legacy}"), &body);
        Ok(())
    }

    /// 下载限速（B/s，0 = 不限）。辅种校验期间给 1 B/s 兜底：文件不全也不会开始狂下。
    pub fn set_download_limit(&self, hash: &str, limit: i64) -> Result<()> {
        self.login()?;
        self.post_form_ignoring_errors(
            "/api/v2/torrents/setDownloadLimit",
            &[("hashes", hash.to_string()), ("limit", limit.to_string())],
        );
        Ok(())
    }

    /// POST 表单并返回 HTTP 状态码（请求没通返回 None）
    fn post_form_status(&self, path: &str, fields: &[(&str, String)]) -> Option<u16> {
        let resp = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .headers(self.referer_headers())
            .form(fields)
            .send()
            .ok()?;
        let st = resp.status().as_u16();
        self.session_died(st);
        Some(st)
    }

    pub fn remove_torrent(&self, hash: &str, keep_files: bool) -> Result<()> {
        self.login()?;
        self.post_form_ignoring_errors(
            "/api/v2/torrents/delete",
            &[
                ("hashes", hash.to_string()),
                (
                    "deleteFiles",
                    if keep_files { "false" } else { "true" }.to_string(),
                ),
            ],
        );
        Ok(())
    }

    /// 推种。`paused=true` 是辅种安全网：先暂停挂上，校验通过再恢复。
    ///
    /// 单 tracker 策略：同 hash 已存在时**绝不**补挂/改写 tracker，也不接管原种子；
    /// 只回执「已存在」，由上层记 SameHashSkip/外部工具覆盖备注。
    #[allow(clippy::too_many_arguments)]
    pub fn add_torrent(
        &self,
        data: &[u8],
        filename: &str,
        save_path: &str,
        category: Option<&str>,
        skip_checking: bool,
        up_limit: i64,
        paused: bool,
        content_layout: Option<&str>,
    ) -> Result<AddTorrentResult> {
        self.add_torrent_ex(
            data,
            filename,
            save_path,
            category,
            skip_checking,
            up_limit,
            paused,
            content_layout,
        )
    }

    /// 兼容别名：本仓库统一走「不接管已有种子」语义（见 `add_torrent` 注释）。
    #[allow(clippy::too_many_arguments)]
    pub fn add_torrent_new_only(
        &self,
        data: &[u8],
        filename: &str,
        save_path: &str,
        category: Option<&str>,
        skip_checking: bool,
        up_limit: i64,
        paused: bool,
        content_layout: Option<&str>,
    ) -> Result<AddTorrentResult> {
        self.add_torrent(
            data,
            filename,
            save_path,
            category,
            skip_checking,
            up_limit,
            paused,
            content_layout,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn add_torrent_ex(
        &self,
        data: &[u8],
        filename: &str,
        save_path: &str,
        category: Option<&str>,
        skip_checking: bool,
        up_limit: i64,
        paused: bool,
        content_layout: Option<&str>,
    ) -> Result<AddTorrentResult> {
        self.login()?;

        let fields = add_form_fields(
            save_path,
            category,
            skip_checking,
            up_limit,
            paused,
            content_layout,
        );
        let mut form = reqwest::blocking::multipart::Form::new();
        for (k, v) in fields {
            form = form.text(k, v);
        }
        let file_part = reqwest::blocking::multipart::Part::bytes(data.to_vec())
            .file_name(filename.to_string())
            .mime_str("application/x-bittorrent")
            .context("mime 类型构造失败（不可能）")?;
        form = form.part("torrents", file_part);

        let resp = self
            .client
            .post(format!("{}/api/v2/torrents/add", self.base_url))
            .headers(self.referer_headers())
            .multipart(form)
            .send()
            .context("POST torrents/add 没通")?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        let hash = bencode::info_hash_v1(data).unwrap_or_default();

        if status == 409 {
            // 5.2+: 未新增任何种子时返回 409 Conflict（典型场景：种子已存在于下载器）。
            // 单 tracker 策略：不补挂 tracker、不接管原种子，只回执已存在。
            return Ok(self.verify(&hash, up_limit, true));
        }
        if status != 200 {
            self.session_died(status);
            let head: String = body.chars().take(200).collect();
            return Err(anyhow!("HTTP {status} POST torrents/add: {head}"));
        }
        // 5.2+ 成功时返回 JSON（success_count 等），旧版返回 "Ok."。
        // 200 但不是这两种之一（例如隧道返回 HTML 登录页）不能当成功。
        if body.trim() != "Ok." {
            let obj = serde_json::from_str::<Value>(&body).map_err(|e| {
                let head: String = body.chars().take(200).collect();
                anyhow!("HTTP 200 POST torrents/add: 回包既不是 Ok. 也不是 JSON（{e}）：{head}")
            })?;
            let sc = obj
                .get("success_count")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| {
                    let head: String = body.chars().take(200).collect();
                    anyhow!("HTTP 200 POST torrents/add: JSON 里没有 success_count：{head}")
                })?;
            if sc == 0 {
                return Err(anyhow!("HTTP 200 POST torrents/add: {body}"));
            }
        }
        Ok(self.verify(&hash, up_limit, false))
    }

    /// 添加后回查：确认种子存在、限速是否生效（限速端点被隧道挡住时给用户可见提示）
    fn verify(&self, hash: &str, up_limit: i64, already: bool) -> AddTorrentResult {
        if already {
            if hash.is_empty() {
                return AddTorrentResult {
                    id: String::new(),
                    note: "已存在于下载器".into(),
                    already: true,
                };
            }
            // 单 tracker 策略：同 hash 原种子一律不改动（不补挂 tracker、不改标签），
            // 视为已由外部工具覆盖，跳过本站 tracker 写入。
            return AddTorrentResult {
                id: hash.to_string(),
                note:
                    "已存在于下载器（同 hash）；按单 tracker 策略不改动原种子，视为已由外部工具覆盖"
                        .into(),
                already: true,
            };
        }
        if hash.is_empty() {
            return AddTorrentResult {
                id: String::new(),
                note: String::new(),
                already: false,
            };
        }
        let Some(t) = self.torrent_info(hash) else {
            return AddTorrentResult {
                id: hash.to_string(),
                note: String::new(),
                already: false,
            };
        };
        let mut notes: Vec<String> = Vec::new();
        if let Some(ul) = t.get("up_limit").and_then(|v| v.as_i64()) {
            if up_limit > 0 && ul < up_limit {
                let cur = if ul == -1 {
                    "跟随全局".to_string()
                } else {
                    format!("{ul} B/s")
                };
                notes.push(format!("限速未生效（当前 {cur}，请求 {up_limit} B/s）"));
            }
        }
        if t.get("state").and_then(|v| v.as_str()) == Some("missingFiles") {
            notes.push(
                "文件不存在于下载器路径（missingFiles）：放置对应文件或 qB 里重新定位后才开始上传"
                    .to_string(),
            );
        }
        AddTorrentResult {
            id: hash.to_string(),
            note: notes.join("；"),
            already: false,
        }
    }
}

/// 站点 upLimit 的查询表（boxsend.json 的 siteUpLimits；Rust 版 v1 由 coverage 配置直接给每站限速）
pub type SiteUpLimits = HashMap<String, i64>;

// ---------- /torrents/info 记录解析 ----------

/// announce URL 的 host（小写）。qB 用 `** [DHT] **` / `** [PeX] **` / `** [LSD] **`
/// 占位符表示非 HTTP tracker，解析不出 host 直接丢。
fn host_of_announce(url: &str) -> Option<String> {
    url::Url::parse(url.trim())
        .ok()?
        .host_str()
        .map(|h| h.to_lowercase())
}

/// 一条记录的 tracker host 列表。字段有三个版本：
/// - `tracker_hosts`：数组，qB 4.6+（**qB 5.x 已不再返回**）；
/// - `trackers`：`|` 分隔的 announce URL 串，更老的版本；
/// - `tracker`：当前那条 announce URL，qB 5.x 实测只剩这个。
///
/// 实机（qB 5.3.0，41,885 条种子）里前两者都不存在。只认前两者会让
/// `tracker_hosts` 全空 →「已在做种」预筛失效 → 全部内容按未覆盖逐站检索。
/// 取当前 tracker 只是近似：一条种子挂多站时只报一个，但跨站辅种在下载器里
/// 本来就是多条种子（不同 hash 合并成同一段内容），并集照样能覆盖到各站。
fn tracker_hosts_of(t: &Value) -> Vec<String> {
    if let Some(arr) = t.get("tracker_hosts").and_then(|v| v.as_array()) {
        let hosts: Vec<String> = arr
            .iter()
            .filter_map(|h| h.as_str())
            .map(|h| h.to_lowercase())
            .collect();
        if !hosts.is_empty() {
            return hosts;
        }
    }
    if let Some(list) = t.get("trackers").and_then(|v| v.as_str()) {
        let hosts: Vec<String> = list.split('|').filter_map(host_of_announce).collect();
        if !hosts.is_empty() {
            return hosts;
        }
    }
    t.get("tracker")
        .and_then(|v| v.as_str())
        .and_then(host_of_announce)
        .into_iter()
        .collect()
}

/// 一条 `/torrents/info` 记录 → `LocalTorrent`。字段名两版兼容（`infohash_v2` 空串
/// 表示不是 v2 种子，必须落成 `None`，否则空 hash 会进身份表把所有内容串在一起）。
pub fn parse_torrent(t: &Value) -> Option<LocalTorrent> {
    let hash = t.get("hash")?.as_str()?.to_lowercase();
    if hash.is_empty() {
        return None;
    }
    let hash_v2 = t
        .get("infohash_v2")
        .and_then(|v| v.as_str())
        .filter(|h| !h.is_empty());
    Some(LocalTorrent::new(
        &hash,
        hash_v2,
        t.get("name").and_then(|v| v.as_str()).unwrap_or(""),
        t.get("save_path").and_then(|v| v.as_str()).unwrap_or(""),
        t.get("category").and_then(|v| v.as_str()),
        t.get("tags")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        t.get("size").and_then(|v| v.as_i64()).unwrap_or(0),
        t.get("progress").and_then(|v| v.as_f64()).unwrap_or(0.0),
        tracker_hosts_of(t),
        t.get("added_on").and_then(|v| v.as_i64()).unwrap_or(0),
    ))
}

/// 解析 `/torrents/files` 回包：单 hash 是 `[file,…]`，多 hash 是 `[[file,…],…]`。
/// 两种形态都要认，嵌套形态逐层摊平。
fn parse_files_value(obj: &Value) -> Option<Vec<(String, i64)>> {
    let arr = obj.as_array()?;
    let nested = arr.first().is_some_and(|v| v.is_array());
    let mut out = Vec::new();
    let mut push = |f: &Value| {
        if let Some(path) = f.get("name").and_then(|v| v.as_str()) {
            let size = f.get("size").and_then(|v| v.as_i64()).unwrap_or(0);
            out.push((path.to_string(), size));
        }
    };
    if nested {
        for inner in arr.iter().filter_map(|v| v.as_array()) {
            for f in inner {
                push(f);
            }
        }
    } else {
        for f in arr {
            push(f);
        }
    }
    Some(out)
}

/// tracker host 是否属于 `hosts` 里的站点。口径与“已在做种预筛”一致（`sites::host_matches`）：
/// `www.` 只是页面前缀，站点自己任意子域上的 tracker 都算本站。
///
/// 这里原来写成“相等或以 `.host` 结尾”，站点地址带 `www.` 时（`www.pthome.net`）
/// 永远匹配不上 `t.pthome.net` 这类 tracker，只能靠内置别名兜底；选 announce 和
/// 校验 `overrides.announceURL` 用的是同一套，改一处要同时改另一处。
pub fn announce_host_matches(tracker_host: &str, hosts: &[String]) -> bool {
    let tracker_host = tracker_host.to_lowercase();
    hosts
        .iter()
        .any(|w| crate::sites::host_matches(&tracker_host, &w.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    /// 极简 qB Web API 假服务器：一次只处理一条连接，够这些串行 API 用例用。
    /// 返回 `(HTTP 状态, 响应正文)`；请求记录放在 `seen` 里供断言。
    fn mock_qb<F>(seen: Arc<Mutex<Vec<String>>>, handler: F) -> String
    where
        F: Fn(&str, &str) -> (u16, String) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock qb");
        let addr = listener.local_addr().expect("mock qb addr");
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let Ok(stream) = stream.try_clone() else {
                    continue;
                };
                let _ = handle_mock_connection(stream, &seen, &handler);
            }
        });
        format!("http://{addr}")
    }

    fn handle_mock_connection<F>(
        stream: TcpStream,
        seen: &Arc<Mutex<Vec<String>>>,
        handler: &F,
    ) -> std::io::Result<()>
    where
        F: Fn(&str, &str) -> (u16, String),
    {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut request_line = String::new();
        reader.read_line(&mut request_line)?;
        let mut method = String::new();
        let mut path = String::new();
        let mut parts = request_line.split_whitespace();
        if let (Some(m), Some(p)) = (parts.next(), parts.next()) {
            method = m.to_string();
            path = p.to_string();
        }
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
        }
        if content_length > 0 {
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body)?;
        }
        seen.lock().unwrap().push(format!("{method} {path}"));
        let (status, body) = handler(&method, &path);
        let reason = match status {
            200 => "OK",
            204 => "No Content",
            400 => "Bad Request",
            404 => "Not Found",
            409 => "Conflict",
            _ => "Internal Server Error",
        };
        let mut response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
            body.len()
        );
        response.push_str(&body);
        let mut writer = stream;
        writer.write_all(response.as_bytes())?;
        writer.flush()
    }

    fn test_client(base: &str) -> QBittorrent {
        QBittorrent::new(base, "admin", "pass").expect("client")
    }

    fn minimal_torrent() -> Vec<u8> {
        // 只为拿到非空 info hash；分片内容不参与 add 的 409 判定。
        b"d4:infod6:lengthi1e4:name4:test12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee"
            .to_vec()
    }

    #[test]
    fn login_accepts_204_and_rejects_wrong_password() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen.clone(), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                // qB 5.2+：已有会话时回 204，不要求正文是 Ok.
                (204, String::new())
            } else {
                (200, "v5.3.0".into())
            }
        });
        let qb = test_client(&base);
        let version = qb.test_connection().expect("204 也算登录成功");
        assert!(version.contains("v5.3.0"), "{version}");
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|p| p.contains("auth/login"))
                .count(),
            1,
            "登录成功后不应重复登录"
        );

        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen, |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                (200, "Fails.".into())
            } else {
                (200, "v5.3.0".into())
            }
        });
        let err = test_client(&base)
            .test_connection()
            .unwrap_err()
            .to_string();
        assert!(err.contains("登录失败"), "{err}");
        assert!(err.contains("HTTP 200"), "{err}");
    }

    #[test]
    fn version_accepts_json_and_rejects_empty_body() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen, |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                (200, "Ok.".into())
            } else if path.starts_with("/api/v2/app/version") {
                (200, r#"{"version":"v4.6.2"}"#.into())
            } else {
                (404, String::new())
            }
        });
        let version = test_client(&base)
            .test_connection()
            .expect("JSON 版本可解析");
        assert!(version.contains("v4.6.2"), "{version}");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen, |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                (200, "Ok.".into())
            } else {
                (200, String::new())
            }
        });
        let err = test_client(&base)
            .test_connection()
            .unwrap_err()
            .to_string();
        assert!(err.contains("回包是空的"), "{err}");
    }

    #[test]
    fn list_torrents_skips_records_without_hash_and_keeps_rich_fields() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen, |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            if path.starts_with("/api/v2/torrents/info") {
                return (
                    200,
                    json!([
                        {
                            "hash": "ABCDEF",
                            "name": "Show.S01.1080p",
                            "save_path": "I:/Download",
                            "category": "tv",
                            "tags": "keep, boxsend",
                            "size": 123456,
                            "progress": 0.75,
                            "tracker": "https://tracker.hdsky.me/announce.php?passkey=x",
                            "added_on": 1700000000
                        },
                        {"name": "no-hash-must-be-skipped"},
                        {"hash": "", "name": "empty-hash-must-be-skipped"}
                    ])
                    .to_string(),
                );
            }
            (404, String::new())
        });
        let list = test_client(&base).list_torrents().expect("解析清单");
        assert_eq!(list.len(), 1);
        let t = &list[0];
        assert_eq!(t.hash, "abcdef");
        assert_eq!(t.name, "Show.S01.1080p");
        assert_eq!(t.total_size, 123456);
        assert_eq!(t.progress, 0.75);
        assert_eq!(t.tags, vec!["keep", "boxsend"]);
        assert_eq!(t.tracker_hosts, vec!["tracker.hdsky.me"]);
        assert_eq!(t.added_on, 1_700_000_000);
    }

    #[test]
    fn existing_hashes_pages_past_the_50000_item_limit() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen.clone(), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            if !path.starts_with("/api/v2/torrents/info") {
                return (404, String::new());
            }
            let offset = path
                .split("offset=")
                .nth(1)
                .and_then(|v| v.split('&').next())
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            // 第一页整 50,000 条，第二页只剩 1 条；总计 50,001。
            let (start, end) = if offset == 0 {
                (0, 50_000)
            } else if offset == 50_000 {
                (50_000, 50_001)
            } else {
                (50_001, 50_001)
            };
            let page: Vec<Value> = (start..end)
                .map(|i| json!({"hash": format!("h{i:05}"), "tracker": "https://tracker.example/announce"}))
                .collect();
            (200, Value::Array(page).to_string())
        });
        let hashes = test_client(&base).existing_hashes().expect("分页取全量");
        assert_eq!(hashes.len(), 50_001);
        assert!(hashes.contains("h00000"));
        assert!(hashes.contains("h50000"));
        let paths = seen.lock().unwrap().clone();
        assert!(
            paths.iter().any(|p| p.contains("offset=0")),
            "缺少第一页请求：{paths:?}"
        );
        assert!(
            paths.iter().any(|p| p.contains("offset=50000")),
            "缺少第二页请求：{paths:?}"
        );
    }

    #[test]
    fn files_falls_back_to_hash_and_remembers_the_working_param() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen.clone(), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            if path.contains("hashes=") {
                return (400, String::new());
            }
            if path.contains("hash=") {
                return (
                    200,
                    json!([{"name": "Pack/a.mkv", "size": 100}]).to_string(),
                );
            }
            (404, String::new())
        });
        let qb = test_client(&base);
        let first = qb.torrent_files("abc").expect("hash 参数回退");
        assert_eq!(first, vec![("Pack/a.mkv".to_string(), 100)]);
        // 第一次先撞 hashes 再试 hash；第二次因为记住了参数，只发一次 hash。
        let before = seen.lock().unwrap().len();
        let second = qb.torrent_files("def").expect("复用记住的参数");
        assert_eq!(second, first);
        let after = seen.lock().unwrap().len();
        assert_eq!(after - before, 1, "第二次不应再撞 hashes");
        assert!(seen.lock().unwrap().last().unwrap().contains("hash=def"));
    }

    #[test]
    fn tracker_info_keeps_status_and_turns_it_into_an_honest_note() {
        let text = r#"[
            {"url":"https://tracker.example/announce.php?passkey=x","status":2,"msg":"","num_seeds":3,"num_leeches":14,"num_downloaded":20},
            {"url":"** [DHT] **","status":0}
        ]"#;
        let infos = parse_tracker_entries(text).expect("解析 tracker 快照");
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].status, 2);
        assert_eq!(infos[0].num_seeds, 3);
        assert!(tracker_status_note(&infos).contains("tracker 已回包"));
        assert!(tracker_status_note(&infos).contains("做种 3 / 下载 14"));
        assert!(tracker_confirms_seeding(&infos));

        let pending = vec![TrackerInfo {
            url: "https://tracker.example/announce".into(),
            status: 1,
            message: String::new(),
            num_seeds: -1,
            num_leeches: -1,
            num_downloaded: -1,
        }];
        assert_eq!(tracker_status_note(&pending), "tracker 还没回包");
        assert!(!tracker_confirms_seeding(&pending));

        let broken = vec![TrackerInfo {
            url: "https://tracker.example/announce".into(),
            status: 4,
            message: "stream truncated".into(),
            num_seeds: -1,
            num_leeches: -1,
            num_downloaded: -1,
        }];
        assert_eq!(
            tracker_status_note(&broken),
            "tracker 报错：stream truncated"
        );
    }

    #[test]
    fn tracker_endpoint_status_2_overrides_top_level_not_contacted() {
        // qB 5.3 实测：顶层 status=1，但 endpoints 里有一条 status=2 已经回包
        let text = r#"[
            {"url":"https://tracker.example/announce.php?passkey=x","status":1,
             "num_seeds":-1,"num_leeches":-1,"num_downloaded":-1,
             "endpoints":[
                {"status":4,"msg":"timed out","num_seeds":-1,"num_leeches":-1},
                {"status":2,"msg":"","num_seeds":1,"num_leeches":14,"num_downloaded":20}
             ]}
        ]"#;
        let infos = parse_tracker_entries(text).expect("解析 tracker endpoints");
        assert_eq!(infos[0].status, 2);
        assert_eq!(infos[0].num_seeds, 1);
        assert_eq!(infos[0].num_leeches, 14);
        assert!(tracker_confirms_seeding(&infos));
        assert!(tracker_status_note(&infos).contains("做种 1 / 下载 14"));
    }

    #[test]
    fn trackers_falls_back_to_infohash_and_filters_non_http_entries() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen.clone(), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            if path.contains("/torrents/trackers?hash=") {
                return (400, String::new());
            }
            if path.contains("/torrents/trackers?infohash=") {
                return (
                    200,
                    json!([
                        {"url": "https://tracker.hdsky.me/announce.php?passkey=x"},
                        {"url": "http://tracker2.example/announce"},
                        {"url": "** [DHT] **"},
                        {"url": "udp://tracker.example:80/announce"}
                    ])
                    .to_string(),
                );
            }
            (404, String::new())
        });
        let urls = test_client(&base).trackers("abc").expect("infohash 回退");
        assert_eq!(
            urls,
            vec![
                "https://tracker.hdsky.me/announce.php?passkey=x",
                "http://tracker2.example/announce"
            ]
        );
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains("/torrents/trackers?hash=abc")));
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains("/torrents/trackers?infohash=abc")));
    }

    #[test]
    fn add_conflict_marks_already_without_touching_existing_torrent() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen.clone(), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            if path.starts_with("/api/v2/torrents/add") {
                return (409, String::new());
            }
            (404, String::new())
        });
        let res = test_client(&base)
            .add_torrent_new_only(
                &minimal_torrent(),
                "x.torrent",
                "I:/Download",
                None,
                true,
                0,
                true,
                None,
            )
            .expect("409 是已存在，不是失败");
        assert!(res.already);
        assert!(!res.id.is_empty());
        assert!(res.note.contains("同 hash"), "{}", res.note);
        assert_non_mutating_calls(&seen.lock().unwrap(), "/api/v2/torrents/");
    }

    #[test]
    fn add_zero_success_count_is_an_error_not_a_silent_accept() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen, |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            if path.starts_with("/api/v2/torrents/add") {
                return (200, r#"{"success_count":0,"failure_count":1}"#.into());
            }
            (404, String::new())
        });
        let err = test_client(&base)
            .add_torrent_new_only(
                b"not-a-torrent",
                "x.torrent",
                "I:/Download",
                None,
                true,
                0,
                true,
                None,
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("success_count"), "{err}");
    }

    #[test]
    fn add_missing_count_or_non_json_body_is_an_error() {
        for (body, expected) in [
            (r#"{"failure_count":0}"#, "success_count"),
            ("<html>login</html>", "JSON"),
        ] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let body = body.to_string();
            let base = mock_qb(seen, move |_, path| {
                if path.starts_with("/api/v2/auth/login") {
                    return (200, "Ok.".into());
                }
                if path.starts_with("/api/v2/torrents/add") {
                    return (200, body.clone());
                }
                (404, String::new())
            });
            let err = test_client(&base)
                .add_torrent_new_only(
                    b"not-a-torrent",
                    "x.torrent",
                    "I:/Download",
                    None,
                    true,
                    0,
                    true,
                    None,
                )
                .unwrap_err()
                .to_string();
            assert!(err.contains(expected), "{err}");
        }
    }

    #[test]
    fn export_rejects_non_bittorrent_body() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen, |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            if path.starts_with("/api/v2/torrents/export") {
                return (200, "this is not a torrent".into());
            }
            (404, String::new())
        });
        let err = test_client(&base)
            .export_torrent("abc")
            .unwrap_err()
            .to_string();
        assert!(err.contains("不是有效 .torrent"), "{err}");
    }

    #[test]
    fn stop_falls_back_to_legacy_pause_on_404() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen.clone(), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            if path.ends_with("/torrents/stop") {
                return (404, String::new());
            }
            if path.ends_with("/torrents/pause") {
                return (200, String::new());
            }
            (404, String::new())
        });
        test_client(&base)
            .set_paused(&["abc"], true)
            .expect("旧版 pause 兜底");
        let log = seen.lock().unwrap();
        assert!(log.iter().any(|p| p.ends_with("/torrents/stop")));
        assert!(log.iter().any(|p| p.ends_with("/torrents/pause")));
    }

    #[test]
    fn all_tracker_urls_collects_non_empty_trackers() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen.clone(), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            if path.starts_with("/api/v2/torrents/info") {
                return (
                    200,
                    json!([
                        {"hash": "a", "tracker": "https://a.example/announce"},
                        {"hash": "b", "tracker": ""},
                        {"hash": "c"},
                        {"hash": "d", "tracker": "https://d.example/announce"}
                    ])
                    .to_string(),
                );
            }
            (404, String::new())
        });
        let urls = test_client(&base).all_tracker_urls().expect("读 tracker");
        assert_eq!(
            urls,
            vec![
                "https://a.example/announce".to_string(),
                "https://d.example/announce".to_string()
            ]
        );
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains("/torrents/info?limit=50000")));
    }

    #[test]
    fn torrent_names_handles_empty_bad_json_and_missing_size() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen.clone(), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            if path.starts_with("/api/v2/torrents/info") {
                return (
                    200,
                    json!([
                        {"hash": "a", "name": "Pack", "size": 100},
                        {"hash": "b", "name": "NoSize"},
                        {"hash": "c", "size": 50}
                    ])
                    .to_string(),
                );
            }
            (404, String::new())
        });
        let qb = test_client(&base);
        assert!(qb.torrent_names(&[]).is_empty(), "空输入不该发请求");
        assert!(seen.lock().unwrap().is_empty(), "空输入却发了请求");
        let names = qb.torrent_names(&["a".into(), "b".into()]);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], ("Pack".into(), 100));
        assert_eq!(names[1], ("NoSize".into(), 0));

        let bad = mock_qb(Arc::new(Mutex::new(Vec::new())), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            (200, "not-json".into())
        });
        assert!(
            test_client(&bad).torrent_names(&["a".into()]).is_empty(),
            "坏 JSON 只返回空表，不 panic"
        );
    }

    #[test]
    fn torrent_content_path_filters_empty_values() {
        let good = mock_qb(Arc::new(Mutex::new(Vec::new())), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            (
                200,
                json!([{"hash":"a","content_path":"/downloads/Pack"}]).to_string(),
            )
        });
        assert_eq!(
            test_client(&good).torrent_content_path("a").as_deref(),
            Some("/downloads/Pack")
        );

        let empty = mock_qb(Arc::new(Mutex::new(Vec::new())), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            (200, json!([{"hash":"a","content_path":""}]).to_string())
        });
        assert_eq!(test_client(&empty).torrent_content_path("a"), None);
    }

    #[test]
    fn set_download_limit_and_remove_torrent_use_expected_endpoints() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen.clone(), |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            (200, "Ok.".into())
        });
        let qb = test_client(&base);
        qb.set_download_limit("abc", 1024).expect("限速接口");
        qb.remove_torrent("abc", true).expect("删除接口");
        let log = seen.lock().unwrap();
        assert!(log
            .iter()
            .any(|p| p.ends_with("/torrents/setDownloadLimit")));
        assert!(log.iter().any(|p| p.ends_with("/torrents/delete")));
        assert!(!log.iter().any(|p| p.contains("/torrents/add")));
    }

    #[test]
    fn torrent_files_many_skips_failures_and_reports_progress() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_qb(seen, |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (200, "Ok.".into());
            }
            if path.starts_with("/api/v2/torrents/files") {
                if path.contains("h1") || path.contains("h3") {
                    return (200, r#"[{"name":"Pack/a.mkv","size":10}]"#.into());
                }
                return (400, String::new());
            }
            (404, String::new())
        });
        let hashes = vec!["h1".to_string(), "h2".to_string(), "h3".to_string()];
        let mut progress = Vec::new();
        let files = test_client(&base).torrent_files_many(&hashes, 2, |done, total, ok| {
            progress.push((done, total, ok))
        });
        assert_eq!(files.len(), 2, "失败的 h2 不能进结果");
        assert_eq!(files["h1"], vec![("Pack/a.mkv".into(), 10)]);
        assert_eq!(files["h3"], vec![("Pack/a.mkv".into(), 10)]);
        assert_eq!(progress.len(), 3, "每条都要回调一次");
        assert_eq!(progress.iter().filter(|p| !p.2).count(), 1);
        assert_eq!(progress.last().map(|p| (p.0, p.1)), Some((3, 3)));
    }

    fn assert_non_mutating_calls(log: &[String], prefix: &str) {
        // 同 hash 已存在时只允许一次 add 探测和读接口；任何删除/改 tracker/改限速都不能出现。
        let mut add_calls = 0;
        for call in log {
            if !call.contains(prefix) {
                continue;
            }
            if call.contains("/add") {
                add_calls += 1;
                continue;
            }
            assert!(
                call.contains("/info") || call.contains("/files") || call.contains("/trackers"),
                "不该出现写接口：{call}"
            );
        }
        assert_eq!(add_calls, 1, "409 只该探测一次 add");
    }

    #[test]
    fn announce_host_matching_ignores_www_and_accepts_site_subdomains() {
        let hosts = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // 站点地址带 www. 时，站点自己子域上的 tracker 也算本站
        assert!(announce_host_matches(
            "t.pthome.net",
            &hosts(&["www.pthome.net"])
        ));
        assert!(announce_host_matches(
            "pttracker6.tjupt.org",
            &hosts(&["www.tjupt.org"])
        ));
        assert!(announce_host_matches(
            "tracker.luckpt.de",
            &hosts(&["luckpt.de", "tracker.luckpt.de"])
        ));
        // 别的站的子域不算，反过来站点域也不算 tracker 子域
        assert!(!announce_host_matches(
            "tracker.pterclub.com",
            &hosts(&["www.pterclub.net"])
        ));
        assert!(!announce_host_matches(
            "pterclub.com",
            &hosts(&["tracker.pterclub.com"])
        ));
        assert!(!announce_host_matches("tracker.a.com", &hosts(&[])));
        // 配置里大小写不敏感
        assert!(announce_host_matches(
            "t.PTHOME.net",
            &hosts(&["WWW.Pthome.Net"])
        ));
    }

    #[test]
    fn announce_for_hosts_recognizes_already_seeded_tracker() {
        // qB 里已挂着该站独立主机 tracker（正在做种）：按合并后的 host 清单能取到 announce，
        // 不会把已做种内容重复算成缺口。
        let seen = Arc::new(Mutex::new(Vec::new()));
        let tracker = "https://a.x-anime.cc/announce?passkey=abc";
        let base = mock_qb(seen.clone(), move |_, path| {
            if path.starts_with("/api/v2/auth/login") {
                return (204, String::new());
            }
            if path.starts_with("/api/v2/torrents/info") {
                return (200, json!([{ "tracker": tracker }]).to_string());
            }
            (404, String::new())
        });
        let qb = test_client(&base);
        let hosts = vec!["mua.xloli.cc".to_string(), "a.x-anime.cc".to_string()];
        let got = qb.announce_for_hosts(&hosts);
        assert_eq!(got.as_deref(), Some(tracker));
    }

    #[test]
    fn add_form_uses_seed_mode_for_skip_checking() {
        let f = add_form_fields(r"I:\Download", None, true, 0, false, Some("Original"));
        let get = |k: &str| f.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("skip_checking"), Some("true"));
        assert_eq!(get("seedMode"), Some("true"));
        assert_eq!(get("started"), Some("true"));
        // 引擎加种统一带 Boxsend 标签（下载器里可筛）
        assert_eq!(get("tags"), Some(BOXSEND_TAG));
        assert_eq!(get("stopped"), Some("false"));
        assert_eq!(get("contentLayout"), Some("Original"));
    }

    #[test]
    fn single_tracker_field_is_used_as_host() {
        // 实机 qB 5.3 形态：只有 tracker（当前那条 announce），没有 tracker_hosts/trackers
        let t = json!({
            "hash": "D4502B049F04AFFC70F6D2C76483717F6F709E01",
            "infohash_v2": "",
            "name": "Some.Release.1080p-GROUP",
            "save_path": "I:/Download",
            "category": "",
            "tags": "",
            "size": 105,
            "progress": 1.0,
            "tracker": "https://tracker.hdsky.me/announce.php?passkey=abc"
        });
        let lt = parse_torrent(&t).expect("能解析");
        assert_eq!(lt.hash, "d4502b049f04affc70f6d2c76483717f6f709e01");
        assert_eq!(lt.hash_v2, None, "空串不是 v2 hash");
        assert_eq!(lt.tracker_hosts, vec!["tracker.hdsky.me"]);
    }

    #[test]
    fn richer_fields_win_over_single_tracker() {
        // 新版若回数组就用数组（多站并集才准）；老版 trackers 串次之
        let arr = json!({
            "hash": "aa", "size": 1, "progress": 1.0,
            "tracker_hosts": ["HDSky.ME", "pterclub.net"],
            "tracker": "https://unused.example/announce"
        });
        assert_eq!(
            parse_torrent(&arr).unwrap().tracker_hosts,
            vec!["hdsky.me", "pterclub.net"]
        );
        let pipe = json!({
            "hash": "bb", "size": 1, "progress": 1.0,
            "trackers": "https://tracker.pterclub.net/announce|udp://x.example:80/announce",
            "tracker": "https://unused.example/announce"
        });
        assert_eq!(
            parse_torrent(&pipe).unwrap().tracker_hosts,
            vec!["tracker.pterclub.net", "x.example"]
        );
    }

    #[test]
    fn pseudo_trackers_are_ignored() {
        // 无 HTTP tracker 的种子：qB 的占位符解析不出 host → 空列表（后续按无 tracker 处理）
        let t = json!({
            "hash": "cc", "size": 1, "progress": 1.0, "tracker": "** [DHT] **"
        });
        assert!(parse_torrent(&t).unwrap().tracker_hosts.is_empty());
    }

    #[test]
    fn record_without_hash_is_skipped() {
        assert!(parse_torrent(&json!({"name": "x"})).is_none());
    }
}
