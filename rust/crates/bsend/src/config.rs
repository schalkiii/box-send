//! 配置加载：直接吃 GUI 版的 `boxsend.json` + `cookies.json`，不引入第二份配置格式
//! （同一台机器上 Swift 版和 Rust 版可以共用一套配置）。
//! 字段名与 Swift `Models/Config.swift` 一致；只认辅种需要的那些，其余字段忽略。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cookiecloud::CookieCloudSettings;
use crate::sites::{extra_announce_hosts, nexusphp::DEFAULT_UA, url_host, Framework};

/// 转种臂模式：v1 只出草稿报告（列「这段内容可以在哪些站发、预计参数」），不真上传。
/// 上传涉及目标站表单适配（148 站每家一套），是 Rust 版 R4 阶段的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReseedMode {
    Off,
    #[default]
    Draft,
}

/// 参与辅种的站点（已按 enabled/白名单/框架过滤）
#[derive(Debug, Clone)]
pub struct SiteConf {
    pub id: String,
    pub name: String,
    pub url: String,
    pub framework: Framework,
    /// overrides.searchURL（带 {name}/{imdb} 占位）
    pub search_url: Option<String>,
    pub api_key: Option<String>,
    /// 站点 announce 的 host 列表（小写，首个是页面 host）＝判断「这份文件已经在哪个站做种」
    pub announce_hosts: Vec<String>,
}

impl SiteConf {
    /// tracker host 是否属于这个站（任一别名命中即可；子域算命中）
    pub fn matches_tracker(&self, tracker_host: &str) -> bool {
        self.announce_hosts
            .iter()
            .any(|h| crate::sites::host_matches(tracker_host, h))
    }

    /// 主 host（没配别名时就是它）
    pub fn primary_host(&self) -> &str {
        self.announce_hosts
            .first()
            .map(|s| s.as_str())
            .unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub struct DownloaderSettings {
    pub url: String,
    pub username: String,
    pub password: String,
    pub save_path: Option<String>,
    pub category: Option<String>,
    pub default_up_limit: i64,
    /// 按源站点 id 的上传限速（bytes/s）
    pub site_up_limits: HashMap<String, i64>,
}

/// 辅种节流与安全网参数。字段名跟 boxsend.json 的 assist 段一致，缺省值也一致。
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
    /// 暂不参与**转种/发种**的站点（仍参与辅种）：账号无发布权限、上传界面 v1 不支持的站。
    /// 配置写法：`assist.reseedPausedSites: ["hdsky", …]`
    pub reseed_paused_sites: Vec<String>,
    /// MediaInfo.dll 路径（源站没 MediaInfo 时从本地文件生成用）；空 = 自动发现
    pub media_info_dll: Option<String>,
    /// pieces-hash 批量通道（auto 默认；off 关闭，回退名字检索）
    pub pieces_hash: PiecesHashMode,
    /// 站点发种规范（`assist.reseedRules.<siteId>`）；缺省全保守
    pub reseed_rules: std::collections::BTreeMap<String, crate::gates::SiteRules>,
    /// 自动发种白名单（空 = 不自动发，仅演练）
    pub auto_reseed_sites: Vec<String>,
    /// 真实发种总开关（缺省 false；所有 run_reseed 路径都过这道闸，演练不受影响）
    pub reseed_publish_enabled: bool,
    /// 索引归档：下载器里消失超过这么多天的内容从索引里清掉
    pub archive_days: i64,
    /// 并行站 worker 数（Swift 版全局串行；Rust 版按站并行，默认 4）
    pub max_concurrent_sites: usize,
    /// 站点报限流后的最低请求间隔（秒，默认 60：hdsky 明说「限制访问间隔 60 秒/次」）
    pub rate_limit_floor_sec: u64,
    /// 逐站间隔覆盖（秒）：有些站要求 60 秒/次（hdsky），全局 8 秒会一开始就撞墙。
    /// 配置写法：`assist.siteIntervals: {"hdsky": 65}`
    pub site_intervals: HashMap<String, f64>,
    /// 首扫描读文件清单的并发数（qB 一次一条请求，4 万条串行太慢）。
    /// Rust 侧扩展：config 里可选 `assist.scanConcurrency`。
    pub scan_concurrency: usize,
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
            reseed_paused_sites: Vec::new(),
            media_info_dll: None,
            pieces_hash: PiecesHashMode::Auto,
            reseed_rules: std::collections::BTreeMap::new(),
            auto_reseed_sites: Vec::new(),
            reseed_publish_enabled: false,
            archive_days: 30,
            max_concurrent_sites: 4,
            rate_limit_floor_sec: 60,
            site_intervals: HashMap::new(),
            scan_concurrency: 6,
            reseed_mode: ReseedMode::Draft,
        }
    }
}

/// 真实发种总闸（默认关）；CLI 与库入口都过这里
pub fn ensure_publish_enabled(cfg: &Config) -> Result<()> {
    if !cfg.coverage.reseed_publish_enabled {
        anyhow::bail!("真实发种总开关关闭（assist.publishEnabled=false）；演练用 `reseed --draft N` 或 `gatecheck`");
    }
    Ok(())
}

/// pieces-hash 批量通道开关（`assist.piecesHash`：auto/off）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiecesHashMode {
    /// 站点支持就走（能力表学习，失败自动回退名字检索）
    Auto,
    /// 关闭（只用名字检索）
    Off,
}

impl PiecesHashMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "0" | "no" => PiecesHashMode::Off,
            _ => PiecesHashMode::Auto,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieEntry {
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiration_date: Option<f64>,
}

/// 一次加载后的完整配置
pub struct Config {
    pub config_path: PathBuf,
    pub data_dir: Option<PathBuf>,
    pub downloader: DownloaderSettings,
    /// 参与辅种的站点（已过滤：enabled + 白名单 + v1 支持检索的框架）
    pub sites: Vec<SiteConf>,
    pub coverage: CoverageSettings,
    pub user_agent: String,
    /// cookies.json：host → cookie 列表（pub 供测试与工具直接组装）
    pub cookies: HashMap<String, Vec<CookieEntry>>,
    /// boxsend.json 的 cookieCloud 段（没配就是 None）
    pub cookie_cloud: Option<CookieCloudSettings>,
}

// boxsend.json 的键是驼峰（GUI 版 Swift Codable 默认）。这里用 serde_json::Value 动态取：
// 配置里大量字段辅种不关心，逐字段声明结构体反而要跟着上游演进改代码。

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
            // 负值钳成 0：负预算会让 acquire 永远失败、整轮静默空转
            search_budget: (v_i64(&assist, "roundBudget", d.search_budget as i64) as i32).max(0),
            source_budget: (v_i64(&assist, "roundBudget", d.source_budget as i64) as i32).max(0),
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
            reseed_paused_sites: v_str_list(&assist, "reseedPausedSites"),
            media_info_dll: assist
                .get("mediaInfoDll")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            pieces_hash: assist
                .get("piecesHash")
                .and_then(|v| v.as_str())
                .map(PiecesHashMode::parse)
                .unwrap_or(PiecesHashMode::Auto),
            reseed_rules: assist
                .get("reseedRules")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| {
                            serde_json::from_value(v.clone())
                                .ok()
                                .map(|r| (k.clone(), r))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            auto_reseed_sites: v_str_list(&assist, "autoReseedSites"),
            reseed_publish_enabled: v_bool(&assist, "publishEnabled", d.reseed_publish_enabled),
            archive_days: d.archive_days,
            max_concurrent_sites: (v_i64(
                &assist,
                "maxConcurrentSites",
                d.max_concurrent_sites as i64,
            ) as usize)
                .clamp(1, 32),
            scan_concurrency: (v_i64(&assist, "scanConcurrency", d.scan_concurrency as i64)
                as usize)
                .clamp(1, 32),
            rate_limit_floor_sec: (v_i64(
                &assist,
                "rateLimitFloorSec",
                d.rate_limit_floor_sec as i64,
            )
            .max(0) as u64),
            site_intervals: assist
                .get("siteIntervals")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_f64().map(|sec| (k.clone(), sec.max(0.0))))
                        .collect()
                })
                .unwrap_or_default(),
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
            // 页面 host + 内置别名 + 配置里追加的 overrides.announceHosts
            let mut announce_hosts: Vec<String> = vec![url_host(&url).unwrap_or_default()];
            announce_hosts.extend(extra_announce_hosts(&id).iter().map(|h| h.to_string()));
            if let Some(list) = s
                .get("overrides")
                .and_then(|o| o.get("announceHosts"))
                .and_then(|v| v.as_array())
            {
                announce_hosts.extend(
                    list.iter()
                        .filter_map(|x| x.as_str())
                        .map(|x| x.to_lowercase()),
                );
            }
            announce_hosts.retain(|h| !h.is_empty());
            announce_hosts.dedup();
            sites.push(SiteConf {
                announce_hosts,
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

        // ---- CookieCloud 段（GUI 的「CookieCloud」设置；没填就是 None）----
        let cc = root.get("cookieCloud").cloned().unwrap_or(Value::Null);
        let cookie_cloud = {
            let settings = CookieCloudSettings {
                host: v_str(&cc, "host").unwrap_or_default(),
                key: v_str(&cc, "key").unwrap_or_default(),
                password: v_str(&cc, "password").unwrap_or_default(),
            };
            settings.is_configured().then_some(settings)
        };

        Ok(Self {
            config_path: path.to_path_buf(),
            data_dir,
            downloader,
            sites,
            coverage,
            user_agent,
            cookies,
            cookie_cloud,
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

    /// 合并一批 cookie（CookieCloud 拉下来的）：按 host 覆盖，未涉及的 host 保留
    pub fn merge_cookies(&mut self, incoming: HashMap<String, Vec<CookieEntry>>) -> usize {
        let n = incoming.len();
        for (host, list) in incoming {
            self.cookies.insert(host, list);
        }
        n
    }

    /// cookies.json 的路径（与 GUI 共用同一份：配置同目录优先，其次 dataDir）
    pub fn cookies_path(&self) -> PathBuf {
        let by_config = self
            .config_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("cookies.json");
        if by_config.exists() || self.data_dir.is_none() {
            return by_config;
        }
        self.data_dir
            .as_deref()
            .unwrap_or(Path::new("."))
            .join("cookies.json")
    }

    /// 原子写 cookies.json（GUI 读这份；先备份旧的成 cookies.json.bak）
    pub fn save_cookies(&self) -> Result<PathBuf> {
        let path = self.cookies_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        if path.exists() {
            let _ = std::fs::copy(&path, path.with_extension("json.bak"));
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(&self.cookies).context("cookie 序列化失败")?;
        std::fs::write(&tmp, body.as_bytes())
            .with_context(|| format!("写不进 {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("落盘 {} 失败", path.display()))?;
        Ok(path)
    }

    /// 辅种索引的持久化路径（dataDir/coverage.json；没有 dataDir 就放配置旁边）
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
    fn publish_is_off_by_default() {
        let cfg = Config {
            config_path: std::env::temp_dir().join("no-such-boxsend.json"),
            data_dir: None,
            downloader: crate::config::DownloaderSettings {
                url: String::new(),
                username: String::new(),
                password: String::new(),
                save_path: None,
                category: None,
                default_up_limit: 0,
                site_up_limits: std::collections::HashMap::new(),
            },
            sites: vec![],
            coverage: crate::config::CoverageSettings::default(),
            user_agent: "x".into(),
            cookies: std::collections::HashMap::new(),
            cookie_cloud: None,
        };
        assert!(!cfg.coverage.reseed_publish_enabled);
    }

    #[test]
    fn loads_boxsend_json_compat() {
        let p = write_temp("boxsend.json", BOXSEND);
        let cfg = Config::load(&p).unwrap();
        // 站点过滤：enabled + 白名单 + NexusPHP-only + 排除 pausedSites
        // （pter 在 pausedSites 里，这轮不参与）
        assert_eq!(cfg.sites.len(), 1);
        assert_eq!(cfg.sites[0].id, "hdsky");
        assert_eq!(cfg.sites[0].primary_host(), "hdsky.me");
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

    // cookies.json 相关断言要用独立目录：write_temp 所有测试共用一个目录，
    // 谁写了 cookies.json 别的测试也会加载到
    fn write_temp_isolated(tag: &str, name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bsend-cfg-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        p
    }

    #[test]
    fn missing_source_sites_is_error() {
        let p = write_temp("bad.json", r#"{"downloader": {"url": "http://x"}}"#);
        assert!(Config::load(&p).is_err());
    }

    #[test]
    fn cookies_json_accepts_pt_depiler_wrapper() {
        let p = write_temp_isolated("wrap", "boxsend.json", BOXSEND);
        // PT-depiler 备份格式：外层多包一层 {"cookies": {...}}，要能解开
        let cookies = r#"{
          "cookies": {
            "hdsky.me": [{"name": "c_secure_uid", "value": "123", "domain": ".hdsky.me"}]
          }
        }"#;
        write_temp_isolated("wrap", "cookies.json", cookies);
        let cfg = Config::load(&p).unwrap();
        assert_eq!(
            cfg.cookie_header_for("hdsky.me").as_deref(),
            Some("c_secure_uid=123")
        );
    }

    #[test]
    fn non_search_frameworks_are_filtered() {
        // 缺 framework / "Other" / Unit3D 都不进 sites：v1 检索臂只认 NexusPHP
        let p = write_temp_isolated(
            "framework",
            "boxsend.json",
            r#"{
          "sourceSites": [
            {"id": "nofw", "name": "没写框架", "url": "https://nofw.example/", "enabled": true},
            {"id": "other", "name": "其他", "url": "https://other.example/", "framework": "Other", "enabled": true},
            {"id": "u3d", "name": "Unit3D", "url": "https://u3d.example/", "framework": "Unit3D", "enabled": true},
            {"id": "nx", "name": "正常", "url": "https://nx.example/", "framework": "NexusPHP", "enabled": true}
          ],
          "downloader": {"url": "http://x"}
        }"#,
        );
        let cfg = Config::load(&p).unwrap();
        let ids: Vec<&str> = cfg.sites.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["nx"]);
    }

    #[test]
    fn announce_aliases_are_loaded() {
        // 页面域名 ≠ announce 域名（实机 luckpt / chdbits），别名要进 announce_hosts；
        // 配置里 overrides.announceHosts 还能追加
        let p = write_temp_isolated(
            "alias",
            "boxsend.json",
            r#"{
          "sourceSites": [
            {"id": "luckpt", "name": "幸运", "url": "https://pt.luckpt.de/", "framework": "NexusPHP", "enabled": true},
            {"id": "chdbits", "name": "彩虹岛", "url": "https://ptchdbits.co/", "framework": "NexusPHP", "enabled": true,
             "overrides": {"announceHosts": ["ChdBits.ORG"]}},
            {"id": "hdsky", "name": "天空", "url": "https://hdsky.me/", "framework": "NexusPHP", "enabled": true}
          ],
          "downloader": {"url": "http://x"}
        }"#,
        );
        let cfg = Config::load(&p).unwrap();
        let by_id = |id: &str| cfg.sites.iter().find(|s| s.id == id).unwrap();
        let luck = by_id("luckpt");
        assert!(luck.matches_tracker("tracker.luckpt.de"), "别名要能命中");
        assert!(luck.matches_tracker("pt.luckpt.de"));
        assert!(!luck.matches_tracker("luckpt.de.evil.example"));
        let cd = by_id("chdbits");
        assert!(cd.matches_tracker("chdbits.xyz"));
        assert!(cd.matches_tracker("chdbits.org"), "配置追加的别名也要生效");
        // 没有别名的站：仍然只认页面 host 与子域
        let sky = by_id("hdsky");
        assert!(sky.matches_tracker("tracker.hdsky.me"));
        assert!(!sky.matches_tracker("chdbits.xyz"));
    }

    #[test]
    fn cookie_cloud_settings_are_parsed() {
        // GUI 的「CookieCloud」设置：host / key / password 齐了才算配好
        let p = write_temp_isolated(
            "cc",
            "boxsend.json",
            r#"{
          "sourceSites": [
            {"id": "nx", "name": "站", "url": "https://nx.example/", "framework": "NexusPHP", "enabled": true}
          ],
          "downloader": {"url": "http://x"},
          "cookieCloud": {"host": "http://127.0.0.1:8082/cookie", "key": "k1", "password": "p1", "pollMinutes": 30}
        }"#,
        );
        let cfg = Config::load(&p).unwrap();
        let cc = cfg.cookie_cloud.as_ref().expect("cookieCloud 段要读进来");
        assert_eq!(cc.host, "http://127.0.0.1:8082/cookie");
        assert_eq!(cc.key, "k1");
        assert!(cc.is_configured());
        // 缺 key/host 的不算配好（别拿空配置去发请求）
        let p2 = write_temp_isolated(
            "cc2",
            "boxsend.json",
            r#"{
          "sourceSites": [
            {"id": "nx", "name": "站", "url": "https://nx.example/", "framework": "NexusPHP", "enabled": true}
          ],
          "downloader": {"url": "http://x"},
          "cookieCloud": {"host": "", "key": "", "password": "p1"}
        }"#,
        );
        assert!(Config::load(&p2).unwrap().cookie_cloud.is_none());
    }

    #[test]
    fn negative_round_budget_is_clamped() {
        // 配置笔误（负预算）不能变成「整轮静默空转」
        let p = write_temp_isolated(
            "negbudget",
            "boxsend.json",
            r#"{
          "sourceSites": [
            {"id": "nx", "name": "正常", "url": "https://nx.example/", "framework": "NexusPHP", "enabled": true}
          ],
          "downloader": {"url": "http://x"},
          "assist": {"roundBudget": -5}
        }"#,
        );
        let cfg = Config::load(&p).unwrap();
        assert_eq!(cfg.coverage.search_budget, 0);
        assert_eq!(cfg.coverage.source_budget, 0);
    }
}
