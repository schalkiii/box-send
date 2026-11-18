//! pieces-hash 批量查询通道（P1）。
//!
//! 站点若实现 `POST {base}/api/pieces-hash`（body `{"passkey", "pieces_hash":[…]}`，
//! 回包 `{"ret":0,"data":{<pieces_hash>: <torrent_id>}}`），一次请求即可确认上百段内容
//! 是否在站内——比按名字检索（每内容 3–7 请求）快两个数量级。
//!
//! 本模块只负责「通道」：passkey 采集与缓存、能力表、pieces_hash 计算缓存、批量查询、
//! 下载直链。是否走这条通道由 `coverage::process_item` 决定（不支持/失败回退名字检索）。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::bencode;
use crate::qbittorrent::QBittorrent;

/// 站点对 pieces-hash 接口的支持状态（学习得到，落盘缓存）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Supported,
    Unsupported,
    Unknown,
}

impl Capability {
    fn as_str(self) -> &'static str {
        match self {
            Capability::Supported => "ok",
            Capability::Unsupported => "unsupported",
            Capability::Unknown => "unknown",
        }
    }
    fn from_str(s: &str) -> Self {
        match s {
            "ok" => Capability::Supported,
            "unsupported" => Capability::Unsupported,
            _ => Capability::Unknown,
        }
    }
}

/// 一次批量查询的结论
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryOutcome {
    /// 命中：(pieces_hash, torrent_id)
    Hit(Vec<(String, i64)>),
    /// 接口正常但一个都没命中
    Miss,
    /// 站点没有这个接口（记能力表）
    Unsupported,
    /// 鉴权失败（passkey 缺失/不对，记能力表为 Unknown 并提示补 passkey）
    Auth,
    /// 其他错误（不记能力，下一轮重试）
    Error(String),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    /// tracker host（小写，去 www）→ passkey
    #[serde(default)]
    passkeys: BTreeMap<String, String>,
    /// 站点 id → ok / unsupported / unknown
    #[serde(default)]
    capabilities: BTreeMap<String, String>,
    /// info hash（小写）→ pieces hash
    #[serde(default)]
    pieces: BTreeMap<String, String>,
}

/// pieces-hash 通道的本地状态（passkey / 能力表 / pieces 缓存），JSON 落盘
pub struct PiecesApi {
    path: PathBuf,
    cache: CacheFile,
}

impl PiecesApi {
    /// 从 dataDir 载入（不存在则空表）
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("pieces-cache.json");
        let cache = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<CacheFile>(&s).ok())
            .unwrap_or_default();
        Self { path, cache }
    }

    /// 落盘（只读场景可跳过；失败不致命）
    pub fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let text = serde_json::to_string_pretty(&self.cache)?;
        std::fs::write(&self.path, text).with_context(|| format!("写 {} 失败", self.path.display()))
    }

    /// 取站点 passkey：精确 host → 去掉 www./pt. 前缀 → 同根域（tracker 常是
    /// `pt.example.com` 而页面是 `example.com`，两边必须能对上）
    pub fn passkey(&self, host: &str) -> Option<&str> {
        let h = normalize_host(host);
        if let Some(v) = self.cache.passkeys.get(&h) {
            return Some(v.as_str());
        }
        let hs = strip_prefixes(&h);
        for (k, v) in &self.cache.passkeys {
            if strip_prefixes(k) == hs || base_domain(k) == base_domain(&h) {
                return Some(v.as_str());
            }
        }
        None
    }

    pub fn set_passkey(&mut self, host: &str, passkey: &str) {
        if !passkey.is_empty() {
            self.cache
                .passkeys
                .insert(normalize_host(host), passkey.to_string());
        }
    }

    pub fn capability(&self, site_id: &str) -> Capability {
        self.cache
            .capabilities
            .get(site_id)
            .map(|s| Capability::from_str(s))
            .unwrap_or(Capability::Unknown)
    }

    pub fn mark(&mut self, site_id: &str, cap: Capability) {
        self.cache
            .capabilities
            .insert(site_id.to_string(), cap.as_str().to_string());
    }

    /// 取某段内容的 pieces_hash：缓存 → 让下载器导出 .torrent 现算
    pub fn pieces_hash_for(&mut self, qb: &QBittorrent, hash: &str) -> Option<String> {
        let key = hash.to_lowercase();
        if let Some(v) = self.cache.pieces.get(&key) {
            return Some(v.clone());
        }
        let data = qb.export_torrent(&key).ok()?;
        let ph = bencode::pieces_hash(&data)?;
        self.cache.pieces.insert(key, ph.clone());
        Some(ph)
    }

    /// 采集 passkey：扫下载器里所有种子当前的 tracker URL（host → passkey）。
    /// 只补缺失的 host，已有的不覆盖（避免被别的号串掉）。
    pub fn harvest_passkeys(&mut self, qb: &QBittorrent) -> usize {
        use std::collections::btree_map::Entry;
        let urls = qb.all_tracker_urls().unwrap_or_default();
        let mut added = 0usize;
        for u in urls {
            if let Some((host, pk)) = parse_passkey(&u) {
                if let Entry::Vacant(slot) = self.cache.passkeys.entry(host) {
                    slot.insert(pk);
                    added += 1;
                }
            }
        }
        added
    }

    /// 批量查询（≤100 个/批由调用方控制）。`base` 是站点根 URL。
    pub fn query(&mut self, site_id: &str, base: &str, hashes: &[String]) -> QueryOutcome {
        if hashes.is_empty() {
            return QueryOutcome::Miss;
        }
        let host = url_host(base);
        let Some(pk) = self.passkey(&host).map(|s| s.to_string()) else {
            return QueryOutcome::Auth;
        };
        let url = format!("{}/api/pieces-hash", base.trim_end_matches('/'));
        let body = serde_json::json!({ "passkey": pk, "pieces_hash": hashes });
        let client = match http_client() {
            Ok(c) => c,
            Err(e) => return QueryOutcome::Error(format!("{e:#}")),
        };
        let body = match serde_json::to_string(&body) {
            Ok(b) => b,
            Err(e) => return QueryOutcome::Error(format!("请求体序列化失败：{e}")),
        };
        let resp = client
            .post(&url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Origin", base.trim_end_matches('/'))
            .header("Referer", format!("{}/", base.trim_end_matches('/')))
            .body(body)
            .send();
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                // 站点侧 404 说明没有这个接口（部分站整页 404）
                return QueryOutcome::Error(format!("请求没通：{e}"));
            }
        };
        let status = resp.status().as_u16();
        let text = resp.text().unwrap_or_default();
        let outcome = parse_query_response(status, &text);
        match &outcome {
            QueryOutcome::Unsupported => self.mark(site_id, Capability::Unsupported),
            QueryOutcome::Hit(_) | QueryOutcome::Miss => self.mark(site_id, Capability::Supported),
            _ => {}
        }
        outcome
    }
}

/// 下载直链（NexusPHP 通用形态；HDArea 等直链站点 v1 不覆盖）
pub fn download_url(base: &str, torrent_id: i64, passkey: &str) -> String {
    format!(
        "{}/download.php?id={torrent_id}&passkey={passkey}",
        base.trim_end_matches('/')
    )
}

/// 从 tracker URL 取 (host, passkey)
pub fn parse_passkey(tracker_url: &str) -> Option<(String, String)> {
    let query = tracker_url.split_once('?')?.1;
    let mut pk = None;
    for kv in query.split('&') {
        if let Some(v) = kv.strip_prefix("passkey=") {
            pk = Some(v.to_string());
        }
    }
    let host = tracker_url
        .split("://")
        .nth(1)?
        .split('/')
        .next()?
        .split(':')
        .next()?;
    let pk = pk?;
    if pk.is_empty() {
        return None;
    }
    Some((normalize_host(host), pk))
}

/// 去掉 www./pt. 前缀（大小写不敏感）
fn strip_prefixes(h: &str) -> String {
    h.trim_start_matches("www.")
        .trim_start_matches("pt.")
        .to_string()
}

/// 根域（最后两段）
fn base_domain(h: &str) -> String {
    let p: Vec<&str> = h.split('.').collect();
    if p.len() >= 2 {
        p[p.len() - 2..].join(".")
    } else {
        h.to_string()
    }
}

fn normalize_host(h: &str) -> String {
    h.trim()
        .to_lowercase()
        .trim_start_matches("www.")
        .to_string()
}

fn url_host(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .trim_start_matches("www.")
        .to_string()
}

fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(crate::sites::nexusphp::DEFAULT_UA)
        .no_proxy()
        .timeout(Duration::from_secs(20))
        .build()
        .context("HTTP 客户端构造失败")
}

/// 回包解析（纯函数，单测友好）：
/// 200 + ret=0 → Hit/Miss；401 → Auth；404/405 → Unsupported；其他 → Error
pub fn parse_query_response(status: u16, body: &str) -> QueryOutcome {
    if status == 401 || status == 403 {
        return QueryOutcome::Auth;
    }
    if status == 404 || status == 405 {
        return QueryOutcome::Unsupported;
    }
    if status != 200 {
        return QueryOutcome::Error(format!("HTTP {status}: {}", head(body, 120)));
    }
    let Ok(obj) = serde_json::from_str::<serde_json::Value>(body) else {
        return QueryOutcome::Error(format!("回包不是 JSON：{}", head(body, 120)));
    };
    if obj.get("ret").and_then(|v| v.as_i64()) != Some(0) {
        return QueryOutcome::Error(format!("ret 非 0：{}", head(body, 120)));
    }
    let mut hits = Vec::new();
    if let Some(map) = obj.get("data").and_then(|v| v.as_object()) {
        for (ph, id) in map {
            if let Some(id) = id.as_i64() {
                hits.push((ph.to_lowercase(), id));
            }
        }
    }
    if hits.is_empty() {
        QueryOutcome::Miss
    } else {
        QueryOutcome::Hit(hits)
    }
}

fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_parsing_covers_all_shapes() {
        let ok = r#"{"ret":0,"msg":"torrent.querybypieceshash","data":{"AA1":15627}}"#;
        assert_eq!(
            parse_query_response(200, ok),
            QueryOutcome::Hit(vec![("aa1".into(), 15627)])
        );
        assert_eq!(
            parse_query_response(200, r#"{"ret":0,"data":{}}"#),
            QueryOutcome::Miss
        );
        assert_eq!(parse_query_response(401, ""), QueryOutcome::Auth);
        assert_eq!(
            parse_query_response(404, "not found"),
            QueryOutcome::Unsupported
        );
        assert!(matches!(
            parse_query_response(200, "<html>"),
            QueryOutcome::Error(_)
        ));
        assert!(matches!(
            parse_query_response(500, "boom"),
            QueryOutcome::Error(_)
        ));
    }

    #[test]
    fn passkey_is_parsed_from_tracker_url() {
        assert_eq!(
            parse_passkey("https://pt.example.test/announce.php?passkey=abc123&x=1"),
            Some(("pt.example.test".into(), "abc123".into()))
        );
        assert_eq!(parse_passkey("https://pt.example.test/announce.php"), None);
    }

    #[test]
    fn cache_roundtrips_and_marks_capability() {
        let dir = std::env::temp_dir().join(format!("bsend-pieces-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        {
            let mut api = PiecesApi::load(&dir);
            api.set_passkey("PT.Example.test", "k");
            api.mark("pter", Capability::Supported);
            api.save().unwrap();
        }
        let api = PiecesApi::load(&dir);
        assert_eq!(api.passkey("pt.example.test"), Some("k"));
        assert_eq!(api.capability("pter"), Capability::Supported);
        assert_eq!(api.capability("nope"), Capability::Unknown);
        // 未支持的站点不该被当成支持
        assert_eq!(api.capability("hdsky"), Capability::Unknown);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn passkey_falls_back_to_same_base_domain() {
        let dir = std::env::temp_dir().join(format!("bsend-pk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        {
            let mut api = PiecesApi::load(&dir);
            api.set_passkey("pt.luckpt.de", "k1");
            api.save().unwrap();
        }
        let api = PiecesApi::load(&dir);
        assert_eq!(api.passkey("luckpt.de"), Some("k1"));
        assert_eq!(api.passkey("www.luckpt.de"), Some("k1"));
        assert_eq!(api.passkey("other.test"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_url_is_nexusphp_style() {
        assert_eq!(
            download_url("https://pt.example.test/", 42, "pk"),
            "https://pt.example.test/download.php?id=42&passkey=pk"
        );
    }

    #[test]
    fn query_without_passkey_reports_auth() {
        let mut api = PiecesApi::load(&std::env::temp_dir());
        let out = api.query("x", "https://pt.example.test/", &["a".repeat(40)]);
        assert_eq!(out, QueryOutcome::Auth);
    }
}
