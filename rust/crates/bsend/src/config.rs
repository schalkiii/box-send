//! 配置加载：直接吃 GUI 版的 `boxsend.json` + `cookies.json`，不引入第二份配置格式
//! （同一台机器上 Swift 版和 Rust 版可以共用一套配置）。
//! 字段名与 Swift `Models/Config.swift` 一致；只认铺种需要的那些，其余字段忽略。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::sites::{nexusphp::DEFAULT_UA, url_host, Framework};

/// 转种臂模式：v1 只出草稿报告（列「这段内容可以在哪些站发、预计参数」），不真上传。
/// 上传涉及目标站表单适配（148 站每家一套），是 Rust 版 R4 阶段的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReseedMode {
    Off,
    #[default]
    Draft,
}

/// 参与铺种的站点（已按 enabled/白名单/框架过滤）
#[derive(Debug, Clone)]
pub struct SiteConf {
    pub id: String,
    pub name: String,
    pub url: String,
    pub framework: Framework,
    /// overrides.searchURL（带 {name}/{imdb} 占位）
    pub search_url: Option<String>,
    pub api_key: Option<String>,
    /// 站点 announce 的 host（小写）＝判断「这份文件已经在哪个站做种」的依据
    pub announce_host: String,
}

#[derive(Debug, Clone)]
pub struct DownloaderSettings {
    pub url: String,
    pub username: String,
    pub password: String,
    pub save_path: Option<String>,
    pub category: Option<String>,
    pub skip_checking: bool,
    pub default_up_limit: i64,
    /// 按源站点 id 的上传限速（bytes/s）
    pub site_up_limits: HashMap<String, i64>,
}

/// 铺种节流与安全网参数。字段名跟 boxsend.json 的 assist 段一致，缺省值也一致。
#[derive(Debug, Clone)]
pub struct CoverageSettings {
    /// 参与辅种的站点白名单（空 = 全部启用的源站）
    pub site_ids: Vec<String>,
    pub per_site_interval_sec: f64,
    pub jitter_sec: f64,
    /// 目标站检索预算（对应 Swift roundBudget）
    pub search_budget: i32,
    /// 源站检索预算（源种定位）
    pub source_budget: i32,
    /// 转种上传预算（v1 草稿模式不消耗）
    pub upload_budget: i32,
    /// 「站内没检索到」记几天，期内不再重搜
    pub miss_ttl_days: i64,
    /// 源种定位 miss 的记录天数（比辅种 miss 长：源站认错代价高、变化慢）
    pub source_miss_ttl_days: i64,
    pub auto_accept_same_files: bool,
    pub remove_on_verify_fail: bool,
    pub min_size_gb: f64,
    pub max_size_gb: f64,
    pub categories: Vec<String>,
    pub verify_wait_sec: u64,
    pub verify_poll_sec: u64,
    /// 站内严格查重没命中时换宽松查询词再问一次
    pub relaxed_retry_search: bool,
    /// 暂停的站点：这轮不去敲它们的门
    pub paused_sites: Vec<String>,
    /// 索引归档：下载器里消失超过这么多天的内容从索引里清掉
    pub archive_days: i64,
    /// 并行站 worker 数（Swift 版全局串行；Rust 版按站并行，默认 4）
    pub max_concurrent_sites: usize,
    pub reseed_mode: ReseedMode,
}

impl Default for CoverageSettings {
    fn default() -> Self {
        Self {
            site_ids: Vec::new(),
            per_site_interval_sec: 8.0,
            jitter_sec: 3.0,
            search_budget: 200,
            source_budget: 200,
            upload_budget: 10,
            miss_ttl_days: 7,
            source_miss_ttl_days: 30,
            auto_accept_same_files: true,
            remove_on_verify_fail: true,
            min_size_gb: 0.0,
            max_size_gb: 0.0,
            categories: Vec::new(),
            verify_wait_sec: 600,
            verify_poll_sec: 10,
            relaxed_retry_search: true,
            paused_sites: Vec::new(),
            archive_days: 30,
            max_concurrent_sites: 4,
            reseed_mode: ReseedMode::Draft,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieEntry {
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub expiration_date: Option<f64>,
}

/// 一次加载后的完整配置
pub struct Config {
    pub config_path: PathBuf,
    pub data_dir: Option<PathBuf>,
    pub downloader: DownloaderSettings,
    /// 参与铺种的站点（已过滤：enabled + 白名单 + v1 支持检索的框架）
    pub sites: Vec<SiteConf>,
    pub coverage: CoverageSettings,
    pub user_agent: String,
    /// cookies.json：host → cookie 列表（pub 供测试与工具直接组装）
    pub cookies: HashMap<String, Vec<CookieEntry>>,
}

// boxsend.json 的键是驼峰（GUI 版 Swift Codable 默认）。这里用 serde_json::Value 动态取：
// 配置里大量字段铺种不关心，逐字段声明结构体反而要跟着上游演进改代码。

fn v_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn v_f64(v: &Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(|x| x.as_f64()).unwrap_or(default)
}

fn v_i64(v: &Value, key: &str, default: i64) -> i64 {
    v.get(key).and_then(|x| x.as_i64()).unwrap_or(default)
}

fn v_bool(v: &Value, key: &str, default: bool) -> bool {
    v.get(key).and_then(|x| x.as_bool()).unwrap_or(default)
}

fn v_str_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(|x| x.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

impl Config {
    /// 从 boxsend.json 加载（GUI 版 Swift Codable 写出的驼峰键，直接兼容）。
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("读不到配置 {}", path.display()))?;
        let root: Value = serde_json::from_str(&text)
            .with_context(|| format!("{} 不是合法 JSON", path.display()))?;

        let data_dir = v_str(&root, "dataDir")
            .or_else(|| v_str(&root, "data_dir"))
            .map(PathBuf::from);
        let user_agent = v_str(&root, "userAgent")
            .or_else(|| v_str(&root, "user_agent"))
            .unwrap_or_else(|| DEFAULT_UA.to_string());

        // ---- 下载器：辅种那台（assistDownloaderID → downloaders 里找；找不到退回主下载器）----
        let main = root.get("downloader").cloned().unwrap_or(Value::Null);
        let assist_id =
            v_str(&root, "assistDownloaderID").or_else(|| v_str(&root, "assist_downloader_id"));
        let mut chosen = main.clone();
        if let Some(id) = &assist_id {
            if id != "main" {
                if let Some(list) = root.get("downloaders").and_then(|v| v.as_array()) {
                    if let Some(hit) = list
                        .iter()
                        .find(|d| v_str(d, "id").as_deref() == Some(id.as_str()))
                    {
                        chosen = hit.clone();
                    }
                }
            }
        }
        let downloader = DownloaderSettings {
            url: v_str(&chosen, "url")
                .ok_or_else(|| anyhow!("配置里没有下载器 url（downloader.url）"))?,
            username: v_str(&chosen, "username").unwrap_or_default(),
            password: v_str(&chosen, "password").unwrap_or_default(),
            save_path: v_str(&chosen, "savePath"),
            category: v_str(&chosen, "category"),
            skip_checking: v_bool(&chosen, "skipChecking", true),
            default_up_limit: v_i64(&chosen, "defaultUpLimit", 0),
            site_up_limits: chosen
                .get("siteUpLimits")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_i64().map(|i| (k.clone(), i)))
                        .collect()
                })
                .unwrap_or_default(),
        };

        // ---- assist 段（全部字段都有缺省，老配置没有这段也能跑）----
        let assist = root.get("assist").cloned().unwrap_or(Value::Null);
        let d = CoverageSettings::default();
        let coverage = CoverageSettings {
            site_ids: v_str_list(&assist, "siteIDs"),
            per_site_interval_sec: v_f64(&assist, "perSiteIntervalSec", d.per_site_interval_sec),
            jitter_sec: v_f64(&assist, "jitterSec", d.jitter_sec),
            search_budget: v_i64(&assist, "roundBudget", d.search_budget as i64) as i32,
            source_budget: v_i64(&assist, "roundBudget", d.source_budget as i64) as i32,
            upload_budget: d.upload_budget,
            miss_ttl_days: v_i64(&assist, "missTTLDays", d.miss_ttl_days),
            source_miss_ttl_days: d.source_miss_ttl_days,
            auto_accept_same_files: v_bool(
                &assist,
                "autoAcceptSameFiles",
                d.auto_accept_same_files,
            ),
            remove_on_verify_fail: v_bool(&assist, "removeOnVerifyFail", d.remove_on_verify_fail),
            min_size_gb: v_f64(&assist, "minSizeGB", d.min_size_gb),
            max_size_gb: v_f64(&assist, "maxSizeGB", d.max_size_gb),
            categories: v_str_list(&assist, "categories"),
            verify_wait_sec: v_i64(&assist, "verifyWaitSec", d.verify_wait_sec as i64) as u64,
            verify_poll_sec: v_i64(&assist, "verifyPollSec", d.verify_poll_sec as i64) as u64,
            relaxed_retry_search: v_bool(&assist, "relaxedRetrySearch", d.relaxed_retry_search),
            paused_sites: v_str_list(&assist, "pausedSites"),
            archive_days: d.archive_days,
            max_concurrent_sites: d.max_concurrent_sites,
            reseed_mode: d.reseed_mode,
        };

        // ---- 站点：enabled 的 sourceSites；辅种白名单再筛一道；v1 只留支持检索的框架 ----
        let raw_sites = root
            .get("sourceSites")
            .and_then(|v| v.as_array())
            .cloned()
            .ok_or_else(|| anyhow!("配置里没有 sourceSites 数组"))?;
        let whitelist: Option<std::collections::HashSet<String>> = if coverage.site_ids.is_empty() {
            None
        } else {
            Some(coverage.site_ids.iter().cloned().collect())
        };
        let mut sites = Vec::new();
        for s in &raw_sites {
            if !v_bool(s, "enabled", false) {
                continue;
            }
            let Some(id) = v_str(s, "id") else { continue };
            if let Some(w) = &whitelist {
                if !w.contains(&id) {
                    continue;
                }
            }
            if coverage.paused_sites.iter().any(|p| p == &id) {
                continue;
            }
            let Some(url) = v_str(s, "url") else { continue };
            let framework = Framework::parse(&v_str(s, "framework").unwrap_or_default());
            if !framework.search_capable() {
                continue;
            }
            let search_url = s
                .get("overrides")
                .and_then(|o| v_str(o, "searchURL"))
                .filter(|u| !u.is_empty());
            sites.push(SiteConf {
                announce_host: url_host(&url).unwrap_or_default(),
                id,
                name: v_str(s, "name").unwrap_or_default(),
                url,
                framework,
                search_url,
                api_key: v_str(s, "apiKey").filter(|k| !k.is_empty()),
            });
        }

        // ---- cookies.json：配置同目录优先，其次 dataDir ----
        let cookies = Self::load_cookies(path, data_dir.as_deref());

        Ok(Self {
            config_path: path.to_path_buf(),
            data_dir,
            downloader,
            sites,
            coverage,
            user_agent,
            cookies,
        })
    }

    fn load_cookies(
        config_path: &Path,
        data_dir: Option<&Path>,
    ) -> HashMap<String, Vec<CookieEntry>> {
        let candidates = [
            Some(
                config_path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("cookies.json"),
            ),
            data_dir.map(|d| d.join("cookies.json")),
        ];
        for c in candidates.into_iter().flatten() {
            if let Ok(text) = std::fs::read_to_string(&c) {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    // 兼容 PT-depiler 备份的外层包裹 { "cookies": { host: [...] } }
                    let body = v.get("cookies").unwrap_or(&v);
                    if let Some(map) = body.as_object() {
                        let parsed: HashMap<String, Vec<CookieEntry>> = map
                            .iter()
                            .filter_map(|(host, arr)| {
                                let list =
                                    serde_json::from_value::<Vec<CookieEntry>>(arr.clone()).ok()?;
                                Some((host.clone(), list))
                            })
                            .collect();
                        return parsed;
                    }
                }
            }
        }
        HashMap::new()
    }

    /// 站点 host 的 Cookie 头（Swift CookieStore.cookieHeader 语义：host 等于 jar 键，
    /// 或 host 是 jar 键的子域；空值 cookie 只在无过期时间时携带）
    pub fn cookie_header_for(&self, host: &str) -> Option<String> {
        let host = host.to_lowercase();
        let mut pairs: Vec<String> = Vec::new();
        for (key, list) in &self.cookies {
            let key = key.to_lowercase();
            if !(host == key || host.ends_with(&format!(".{key}"))) {
                continue;
            }
            for c in list {
                if !c.value.is_empty() || c.expiration_date.is_none() {
                    pairs.push(format!("{}={}", c.name, c.value));
                }
            }
        }
        if pairs.is_empty() {
            None
        } else {
            Some(pairs.join("; "))
        }
    }

    /// 铺种索引的持久化路径（dataDir/coverage.json；没有 dataDir 就放配置旁边）
    pub fn coverage_state_path(&self) -> PathBuf {
        self.data_dir
            .as_deref()
            .unwrap_or_else(|| self.config_path.parent().unwrap_or(Path::new(".")))
            .join("coverage.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bsend-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        p
    }

    const BOXSEND: &str = r#"{
      "dataDir": "/tmp/bsend-data",
      "sourceSites": [
        {"id": "hdsky", "name": "天空", "url": "https://hdsky.me/", "framework": "NexusPHP", "enabled": true,
         "overrides": {"searchURL": "torrents.php?search={name}&search_area=0"}},
        {"id": "pter", "name": "猫", "url": "https://pterclub.net/", "framework": "NexusPHP", "enabled": true},
        {"id": "mteam", "name": "馒头", "url": "https://kp.m-team.cc/", "framework": "Unit3D", "enabled": true},
        {"id": "hdfans", "name": "红豆饭", "url": "http://hdfans.org/", "framework": "NexusPHP", "enabled": false}
      ],
      "downloader": {"id": "main", "type": "qbittorrent", "url": "http://127.0.0.1:8080",
                     "username": "admin", "password": "pass", "skipChecking": true, "defaultUpLimit": 0},
      "downloaders": [
        {"id": "vps", "type": "qbittorrent", "url": "http://vps:8080", "username": "u", "password": "p",
         "savePath": "/data", "category": "reseed"}
      ],
      "assistDownloaderID": "vps",
      "assist": {"siteIDs": ["hdsky", "pter"], "perSiteIntervalSec": 5, "roundBudget": 120,
                 "missTTLDays": 3, "pausedSites": ["pter"]}
    }"#;

    #[test]
    fn loads_boxsend_json_compat() {
        let p = write_temp("boxsend.json", BOXSEND);
        let cfg = Config::load(&p).unwrap();
        // 站点过滤：enabled + 白名单 + NexusPHP-only + 排除 pausedSites
        // （pter 在 pausedSites 里，这轮不参与）
        assert_eq!(cfg.sites.len(), 1);
        assert_eq!(cfg.sites[0].id, "hdsky");
        assert_eq!(cfg.sites[0].announce_host, "hdsky.me");
        assert_eq!(
            cfg.sites[0].search_url.as_deref(),
            Some("torrents.php?search={name}&search_area=0")
        );
        // 下载器解析到 assistDownloaderID 指的那台
        assert_eq!(cfg.downloader.url, "http://vps:8080");
        assert_eq!(cfg.downloader.save_path.as_deref(), Some("/data"));
        assert_eq!(cfg.downloader.category.as_deref(), Some("reseed"));
        // assist 段缺省值
        assert_eq!(cfg.coverage.per_site_interval_sec, 5.0);
        assert_eq!(cfg.coverage.search_budget, 120);
        assert_eq!(cfg.coverage.miss_ttl_days, 3);
        assert_eq!(cfg.coverage.jitter_sec, 3.0);
        assert!(cfg.coverage.auto_accept_same_files);
        assert_eq!(cfg.coverage.reseed_mode, ReseedMode::Draft);
        // 索引路径跟 dataDir
        assert_eq!(
            cfg.coverage_state_path(),
            PathBuf::from("/tmp/bsend-data/coverage.json")
        );
    }

    #[test]
    fn cookie_header_matching() {
        // cookies.json 与 boxsend.json 同目录（write_temp 已经写到同一个测试目录）
        let p = write_temp("boxsend2.json", BOXSEND);
        let cookies = r#"{
          "hdsky.me": [
            {"name": "c_secure_uid", "value": "123", "domain": ".hdsky.me"},
            {"name": "expired", "value": "", "domain": ".hdsky.me", "expirationDate": 100.0}
          ],
          "other.site": [{"name": "x", "value": "y"}]
        }"#;
        write_temp("cookies.json", cookies);
        let cfg = Config::load(&p).unwrap();
        let header = cfg.cookie_header_for("hdsky.me").unwrap();
        assert!(header.contains("c_secure_uid=123"), "header = {header}");
        // 空值且带过期时间的 cookie 不带
        assert!(!header.contains("expired"), "header = {header}");
        // 子域匹配（jar 键是父域）
        assert!(cfg.cookie_header_for("www.hdsky.me").is_some());
        assert!(cfg.cookie_header_for("other.site").is_some());
        assert!(cfg.cookie_header_for("nowhere.example").is_none());
    }

    #[test]
    fn missing_source_sites_is_error() {
        let p = write_temp("bad.json", r#"{"downloader": {"url": "http://x"}}"#);
        assert!(Config::load(&p).is_err());
    }
}
