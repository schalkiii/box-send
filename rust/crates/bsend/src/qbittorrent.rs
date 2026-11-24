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
use crate::models::LocalTorrent;

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
        let arr = self
            .get_json("/api/v2/torrents/info")?
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow!("torrents/info 回包不是数组"))?;
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
                    let next = { queue.lock().unwrap().pop_front() };
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
            let preferred = *self.files_param.lock().unwrap();
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
                    *self.files_param.lock().unwrap() = Some(param);
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
        let arr = self
            .get_json("/api/v2/torrents/info?limit=50000")?
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow!("torrents/info 回包不是数组"))?;
        Ok(arr
            .iter()
            .filter_map(|t| t.get("hash")?.as_str())
            .map(|h| h.to_lowercase())
            .filter(|h| !h.is_empty())
            .collect())
    }

    /// 一条种子当前的 http(s) tracker URL 列表。qB 的 trackers 接口会带
    /// `** [DHT] **` / `** [PeX] **` / `** [LSD] **` 伪条目，按 scheme 过滤后再计数。
    pub fn trackers(&self, hash: &str) -> Result<Vec<String>> {
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
            let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            return Ok(arr
                .into_iter()
                .filter_map(|t| t.get("url").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .filter(|u| {
                    let u = u.trim();
                    u.starts_with("http://") || u.starts_with("https://")
                })
                .collect());
        }
        Err(anyhow!("读不到 {hash} 的 tracker 列表"))
    }

    /// 一条种子的 http(s) tracker 条数（单 tracker 策略挂种后必须 == 1）。
    pub fn tracker_count(&self, hash: &str) -> Result<usize> {
        Ok(self.trackers(hash)?.len())
    }

    /// 从下载器现有种子里，按站点 announce host 取一条完整 announce URL（带 passkey）。
    /// 用于把源种 announce 改写为目标站唯一一条；本地没有该站任何种子时返回 `None`。
    pub fn announce_for_hosts(&self, hosts: &[String]) -> Option<String> {
        self.login().ok()?;
        let arr = self
            .get_json("/api/v2/torrents/info?limit=50000")
            .ok()?
            .as_array()
            .cloned()?;
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
        let resp = self
            .client
            .get(format!(
                "{}/api/v2/torrents/info?limit=50000",
                self.base_url
            ))
            .headers(self.referer_headers())
            .send()
            .context("GET torrents/info 没通")?;
        let status = resp.status().as_u16();
        if status != 200 {
            self.session_died(status);
            bail!("HTTP {status} 拉种子清单");
        }
        let text = resp.text().context("读种子清单失败")?;
        let list: Value = serde_json::from_str(&text).context("种子清单不是 JSON")?;
        let mut out = Vec::new();
        if let Some(arr) = list.as_array() {
            // 触顶即告警：静默截断会让 passkey/tracker 集不全，辅种源定位悄悄缺站
            if arr.len() >= 50_000 {
                eprintln!("警告：torrents/info 达到 50000 条上限，tracker 采集可能不完整");
            }
            for t in arr {
                if let Some(u) = t.get("tracker").and_then(|v| v.as_str()) {
                    if !u.is_empty() {
                        out.push(u.to_string());
                    }
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
        // 5.2+ 成功时返回 JSON（success_count 等），旧版返回 "Ok."
        if body.trim() != "Ok." {
            if let Ok(obj) = serde_json::from_str::<Value>(&body) {
                let sc = obj
                    .get("success_count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(1);
                let fc = obj
                    .get("failure_count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if sc == 0 && fc > 0 {
                    return Err(anyhow!("HTTP 200 POST torrents/add: {body}"));
                }
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
