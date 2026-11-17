//! qBittorrent Web API (v2) 客户端。与 Swift 版 `Downloader/QBittorrent.swift` 逐段对齐，
//! 保留它的全部兼容性补丁（同一条注释里写明为什么）：
//! - 登录 200+"Ok." 或 204（qB 5.2+ 已有会话）都算成功；
//! - `/torrents/files` 参数名 `hashes`→`hash` 两版都试，响应可能是 `[file]` 或 `[[file]]`；
//! - `/torrents/trackers` 参数名 `hash`→`infohash` 两版都试（qB 5.x 只认前者）；
//! - add 回 409 = 种子已在下载器（跨站转种同 hash 太常见），要补挂目标站 tracker 才算落地；
//! - addTrackers 带 `hash`+`hashes` 两份参数，老版本回退 setTrackers 整表重写。

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
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
}

/// 推种结果：`id` 是 v1 info hash（.torrent 解不出时为空），`note` 给人看（已存在/补挂/限速未生效…）
#[derive(Debug, Clone)]
pub struct AddTorrentResult {
    pub id: String,
    pub note: String,
}

impl QBittorrent {
    pub fn new(base_url: &str, username: &str, password: &str) -> Result<Self> {
        let client = Client::builder()
            .cookie_store(true)
            .timeout(Duration::from_secs(60))
            .build()
            .context("构建 HTTP 客户端失败")?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            username: username.to_string(),
            password: password.to_string(),
        })
    }

    fn login(&self) -> Result<()> {
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
            return Ok(());
        }
        let head: String = body.chars().take(80).collect();
        Err(anyhow!(
            "qBittorrent 登录失败 (HTTP {status} {head})，检查 url/账号/密码"
        ))
    }

    pub fn test_connection(&self) -> Result<String> {
        self.login()?;
        let resp = self.get_json("/api/v2/app/version")?;
        let version = resp
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("未知版本");
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

    /// 种子清单。`tracker_hosts`（qB 4.6+）或 `trackers`（老版本，`|` 分隔的 announce 列表）
    /// 任一能用就用；都拿不到的种子 tracker 列表为空（不阻断，后续按无 tracker 处理）。
    pub fn list_torrents(&self) -> Result<Vec<LocalTorrent>> {
        self.login()?;
        let arr = self
            .get_json("/api/v2/torrents/info")?
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow!("torrents/info 回包不是数组"))?;
        Ok(arr
            .into_iter()
            .filter_map(|t| {
                let hash = t.get("hash")?.as_str()?.to_lowercase();
                let hosts: Vec<String> =
                    if let Some(th) = t.get("tracker_hosts").and_then(|v| v.as_array()) {
                        th.iter()
                            .filter_map(|h| h.as_str())
                            .map(|h| h.to_lowercase())
                            .collect()
                    } else if let Some(tr) = t.get("trackers").and_then(|v| v.as_str()) {
                        tr.split('|')
                            .filter_map(|u| url::Url::parse(u).ok())
                            .filter_map(|u| u.host_str().map(|h| h.to_lowercase()))
                            .collect()
                    } else {
                        Vec::new()
                    };
                Some(LocalTorrent::new(
                    &hash,
                    t.get("infohash_v2").and_then(|v| v.as_str()),
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
                    hosts,
                ))
            })
            .collect())
    }

    /// 文件清单。参数名两版都试（同 trackers 端点的老坑：qB 5.x 只认 hash）
    pub fn torrent_files(&self, hash: &str) -> Result<Vec<(String, i64)>> {
        self.login()?;
        for param in ["hashes", "hash"] {
            let Ok(resp) = self
                .client
                .get(format!(
                    "{}/api/v2/torrents/files?{}={}",
                    self.base_url, param, hash
                ))
                .headers(self.referer_headers())
                .send()
            else {
                continue;
            };
            if resp.status().as_u16() != 200 {
                continue;
            }
            let Ok(text) = resp.text() else { continue };
            let Ok(obj) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            // 多 hash 查询时 qB 回 [[file,…],…]；单 hash 直接回 [file,…]
            let list: Vec<Value> = if let Some(arr) = obj.as_array() {
                arr.clone()
            } else if let Some(outer) = obj.as_array().and_then(|a| a.first().cloned()) {
                match outer.as_array() {
                    Some(inner) => inner.clone(),
                    None => continue,
                }
            } else {
                continue;
            };
            let files: Vec<(String, i64)> = list
                .into_iter()
                .filter_map(|f| {
                    let path = f.get("name")?.as_str()?.to_string();
                    let size = f.get("size").and_then(|v| v.as_i64()).unwrap_or(0);
                    Some((path, size))
                })
                .collect();
            if !files.is_empty() {
                return Ok(files);
            }
        }
        Err(anyhow!("读不到 {hash} 的文件清单"))
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
        if resp.status().as_u16() != 200 {
            return None;
        }
        let text = resp.text().ok()?;
        let Value::Array(arr) = serde_json::from_str(&text).ok()? else {
            return None;
        };
        arr.into_iter().next()
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

    pub fn set_paused(&self, hashes: &[&str], paused: bool) -> Result<()> {
        self.login()?;
        let path = if paused { "pause" } else { "resume" };
        self.post_form_ignoring_errors(
            &format!("/api/v2/torrents/{path}"),
            &[("hashes", hashes.join("|"))],
        );
        Ok(())
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

    /// 推种。`paused=true` 是辅种安全网：先暂停挂上，校验通过再恢复（Swift `AssistPipeline.push` 同款）
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
    ) -> Result<AddTorrentResult> {
        self.login()?;

        let mut form = reqwest::blocking::multipart::Form::new()
            .text("savepath", save_path.to_string())
            .text("category", category.unwrap_or("").to_string())
            .text(
                "skip_checking",
                if skip_checking { "true" } else { "false" }.to_string(),
            )
            .text("upLimit", up_limit.to_string())
            // 4.3+：显式启动标志，避免下载器默认「添加后暂停」导致推送了却不跑
            .text("started", if paused { "false" } else { "true" }.to_string());
        if paused {
            // qB 4.4+ 的 torrents/add 直接收 paused；辅种靠它把「先暂停等校验」落到实处
            form = form.text("paused", "true".to_string());
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
            // 5.2+: 未新增任何种子时返回 409 Conflict（典型场景：种子已存在于下载器）
            // 视为已添加，保证重复执行幂等；明确提示「已存在」
            return Ok(self.verify(&hash, up_limit, true, data));
        }
        if status != 200 {
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
        Ok(self.verify(&hash, up_limit, false, data))
    }

    /// 添加后回查：确认种子存在、限速是否生效（限速端点被隧道挡住时给用户可见提示）
    fn verify(
        &self,
        hash: &str,
        up_limit: i64,
        already: bool,
        torrent_data: &[u8],
    ) -> AddTorrentResult {
        if already {
            if hash.is_empty() {
                return AddTorrentResult {
                    id: String::new(),
                    note: "已存在于下载器".into(),
                };
            }
            let note = self.attach_site_tracker(hash, torrent_data);
            return AddTorrentResult {
                id: hash.to_string(),
                note,
            };
        }
        if hash.is_empty() {
            return AddTorrentResult {
                id: String::new(),
                note: String::new(),
            };
        }
        let Some(t) = self.torrent_info(hash) else {
            return AddTorrentResult {
                id: hash.to_string(),
                note: String::new(),
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
        }
    }

    /// 同 hash 的种子已在下载器（跨站转种太常见：很多站不改 info 段，源站和目标站的 .torrent
    /// 算出来是同一个 hash，qB 直接回 409 什么也不加）。这时目标站的 tracker 根本没挂上，
    /// 站内等于没做种——把本站 announce 补进那条种子，推送才算真落地。
    fn attach_site_tracker(&self, hash: &str, torrent_data: &[u8]) -> String {
        let announce = bencode::announce_urls(torrent_data);
        if announce.is_empty() {
            return "已存在于下载器（种子文件里没有 tracker，无法补挂）".to_string();
        }
        let hosts: Vec<String> = announce
            .iter()
            .filter_map(|u| url::Url::parse(u).ok())
            .filter_map(|u| u.host_str().map(|h| h.to_string()))
            .collect();
        let hosts_text = hosts.join("、");
        let Some(existing) = self.tracker_urls(hash) else {
            return format!(
                "已存在于下载器，本站 tracker（{hosts_text}）补挂失败：读不到该种子现有的 tracker"
            );
        };
        // 同一 host 视为同一个 tracker（换 passkey 重推不该重复挂）
        let host_of = |u: &str| {
            url::Url::parse(u)
                .ok()
                .and_then(|p| p.host_str().map(|h| h.to_lowercase()))
                .unwrap_or_else(|| u.to_lowercase())
        };
        let existing_hosts: Vec<String> = existing.iter().map(|u| host_of(u)).collect();
        let missing: Vec<String> = announce
            .iter()
            .filter(|a| !existing_hosts.contains(&host_of(a)))
            .cloned()
            .collect();
        if missing.is_empty() {
            return "已存在于下载器，本站 tracker 已在种子中，跨种已生效".to_string();
        }
        match self.patch_trackers(hash, &existing, &missing) {
            Some(err) => format!("已存在于下载器，本站 tracker（{hosts_text}）补挂失败：{err}"),
            None => format!("已存在于下载器，已补挂本站 tracker（{hosts_text}），跨种已生效"),
        }
    }

    /// 读现有 tracker 用的参数名：qB 5.x（WebAPI 2.11+）只认 hash，
    /// 用旧的 hashes 会回 400「缺少必需参数：hash」；4.x 用 infohash，两个都试一次
    fn tracker_urls(&self, hash: &str) -> Option<Vec<String>> {
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
            if resp.status().as_u16() != 200 {
                continue;
            }
            let Ok(text) = resp.text() else { continue };
            let Ok(Value::Array(arr)) = serde_json::from_str(&text) else {
                continue;
            };
            return Some(
                arr.into_iter()
                    .filter_map(|t| t.get("url").and_then(|v| v.as_str()).map(|s| s.to_string()))
                    .collect(),
            );
        }
        None
    }

    /// tracker 改写接口的参数：hash（qB 5.x）+ hashes（4.x），多出来的那一份会被忽略
    fn tracker_patch_fields(hash: &str, add: &[String]) -> Vec<(&'static str, String)> {
        vec![
            ("hash", hash.to_string()),
            ("hashes", hash.to_string()),
            ("urls", add.join("\n")),
        ]
    }

    /// 4.3+ 用 addTrackers（只追加）；老版本只有 setTrackers（整表重写，得把原有的带上）
    fn patch_trackers(&self, hash: &str, existing: &[String], add: &[String]) -> Option<String> {
        let fields = Self::tracker_patch_fields(hash, add);
        let resp = self
            .client
            .post(format!("{}/api/v2/torrents/addTrackers", self.base_url))
            .headers(self.referer_headers())
            .form(&fields)
            .send();
        if let Ok(resp) = resp {
            let s = resp.status().as_u16();
            if s == 200 || s == 204 {
                return None;
            }
            if !(s == 404 || s == 400 || s == 405) {
                return Some(format!("HTTP {s}"));
            }
        } else {
            return Some("补挂请求没通".to_string());
        }
        // qB 5.x 已删掉 setTrackers（404），只有 4.x 会走到这里
        let mut merged = existing.to_vec();
        merged.extend_from_slice(add);
        let fields = Self::tracker_patch_fields(hash, &merged);
        let Ok(resp) = self
            .client
            .post(format!("{}/api/v2/torrents/setTrackers", self.base_url))
            .headers(self.referer_headers())
            .form(&fields)
            .send()
        else {
            return Some("补挂请求没通（setTrackers）".to_string());
        };
        let s = resp.status().as_u16();
        if s == 200 || s == 204 {
            None
        } else {
            Some(format!("HTTP {s}"))
        }
    }
}

/// 站点 upLimit 的查询表（boxsend.json 的 siteUpLimits；Rust 版 v1 由 coverage 配置直接给每站限速）
pub type SiteUpLimits = HashMap<String, i64>;
