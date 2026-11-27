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
    /// 该站完整 tracker 地址（含个人 passkey）。第一次在该站发种前必须配置：
    /// 此时 qB 里还没有任何该站种子，无法从现有 tracker 里采集 passkey。
    pub announce_url: Option<String>,
    /// 上传页把海报放在独立字段时，字段名（如 ptlgs 的 `cover`）。
    /// 不是所有站都把图片放进简介，独立字段要从简介图片里抽一张填过去。
    pub poster_field: Option<String>,
    /// 上传页把截图放在独立字段时，字段名（如 ptlgs 的 `screenshots`）。
    /// 提交值一行一个 URL，不能只靠简介里的图。
    pub screenshot_field: Option<String>,
    /// 该站专用代理地址（形如 `http://127.0.0.1:7890`）；不填或留空 = 直连，行为不变。
    /// 只影响这一个站的 HTTP 客户端，不影响 qB、CookieCloud 和其他站。
    pub proxy: Option<String>,
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
    /// 站点已有同名「其他版本」时照样发（默认 true）：分集/全集、不同压制组都发；
    /// 画质高低交给站点自己的发布规则，工具不自行比较。
    pub allow_other_version: bool,
    /// 内容类型黑名单：标题命中即跳过（教育、成人等不参与自动发种）
    pub skip_keywords: Vec<String>,
    pub verify_wait_sec: u64,
    pub verify_poll_sec: u64,
    /// 站内严格查重没命中时换宽松查询词再问一次
    pub relaxed_retry_search: bool,
    /// 名字检索兜底：内容名没命中时，再试最大的成员种子名（合集/目录名与站点标题
    /// 不一致的实机漏检，如 Coraline）。只多一次检索，仍受检索预算与站间隔约束。
    pub member_name_fallback: bool,
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
    /// 发布后不做自检（默认 false）：跳过标题/MediaInfo/链接回读核对
    pub skip_post_check: bool,
    /// 加入下载器时 skip_checking=true（默认 false）：跳过哈希校验
    pub skip_hash_check: bool,
    /// 同一内容 + 同一站点的失败重试冷却小时数（0 = 不冷却，默认 0）。
    /// 手动作业下重试由人决定，冷却只会挡住刚修好问题的验证；需要防连击时
    /// 在 `assist.backoffHours` 填一个小时间数。
    pub backoff_hours: u32,
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
            allow_other_version: true,
            skip_keywords: Vec::new(),
            verify_wait_sec: 600,
            verify_poll_sec: 10,
            relaxed_retry_search: true,
            member_name_fallback: true,
            paused_sites: Vec::new(),
            reseed_paused_sites: Vec::new(),
            media_info_dll: None,
            pieces_hash: PiecesHashMode::Auto,
            reseed_rules: std::collections::BTreeMap::new(),
            auto_reseed_sites: Vec::new(),
            reseed_publish_enabled: false,
            skip_post_check: false,
            skip_hash_check: false,
            backoff_hours: 0,
            archive_days: 30,
            max_concurrent_sites: 4,
            rate_limit_floor_sec: 60,
            site_intervals: HashMap::new(),
            scan_concurrency: 6,
            reseed_mode: ReseedMode::Draft,
        }
    }
}

impl CoverageSettings {
    /// 取某站生效的发种规则。图数门槛不读配置：要图的站按 `gates::image_floor_detail`
    /// 硬编码（总图数 + 海报/截图拆分），别处配了 `requireImages` 也以这里为准。
    pub fn rules_for(&self, site: &str) -> crate::gates::SiteRules {
        let mut rules = self.reseed_rules.get(site).cloned().unwrap_or_default();
        let floor = crate::gates::image_floor_detail(site);
        rules.require_images = floor.map(|f| f.min_total);
        rules.require_poster = floor.filter(|f| f.min_poster > 0).map(|f| f.min_poster);
        rules.require_screenshots = floor
            .filter(|f| f.min_screenshots > 0)
            .map(|f| f.min_screenshots);
        if rules.category_rules.is_empty() {
            rules.category_rules = crate::gates::category_rules_default(site);
        }
        rules
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
    /// 启用但被引擎排除的站点（id + 原因）：框架不支持/缺 url/白名单外/暂停。
    /// CLI/GUI 用它点名，避免用户以为所有启用站都参与了。
    pub skipped_sites: Vec<(String, String)>,
}

impl Config {
    /// 一行「被排除站点」提示；没有则 None。
    pub fn skipped_notice(&self) -> Option<String> {
        if self.skipped_sites.is_empty() {
            return None;
        }
        let list: Vec<String> = self
            .skipped_sites
            .iter()
            .map(|(id, why)| format!("{id}（{why}）"))
            .collect();
        Some(format!("这些启用站被排除，未参与本轮：{}", list.join("、")))
    }

    /// 按站点 id 取站点配置（参与本轮的站点列表里查）。
    pub fn site_conf(&self, id: &str) -> Option<&SiteConf> {
        self.sites.iter().find(|s| s.id == id)
    }

    /// 覆盖记录里的详情链接域名必须属于目标站。站点不在配置里、或链接域名
    /// 不匹配该站任一别名 → Err；调用方不得把这条记录写进覆盖索引。
    /// 空链接由调用方判断（None 不校验）。
    pub fn validate_coverage_url(&self, site_id: &str, url: &str) -> Result<()> {
        let Some(conf) = self.site_conf(site_id) else {
            anyhow::bail!("站点 {site_id} 不在配置里，无法校验详情链接域名");
        };
        let Some(host) = url_host(url) else {
            anyhow::bail!("详情链接解析不出域名：{url}");
        };
        if conf.matches_tracker(&host) {
            Ok(())
        } else {
            anyhow::bail!(
                "详情链接域名 {host} 不属于站点 {site_id}（本站别名：{}）；拒绝写入覆盖记录",
                conf.announce_hosts.join("/")
            );
        }
    }
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
            upload_budget: (v_i64(&assist, "uploadBudget", d.upload_budget as i64).max(0) as i32),
            miss_ttl_days: v_i64(&assist, "missTTLDays", d.miss_ttl_days),
            source_miss_ttl_days: v_i64(&assist, "sourceMissTTLDays", d.source_miss_ttl_days)
                .max(0),
            auto_accept_same_files: v_bool(
                &assist,
                "autoAcceptSameFiles",
                d.auto_accept_same_files,
            ),
            remove_on_verify_fail: v_bool(&assist, "removeOnVerifyFail", d.remove_on_verify_fail),
            min_size_gb: v_f64(&assist, "minSizeGB", d.min_size_gb),
            max_size_gb: v_f64(&assist, "maxSizeGB", d.max_size_gb),
            categories: v_str_list(&assist, "categories"),
            allow_other_version: v_bool(&assist, "allowOtherVersion", d.allow_other_version),
            skip_keywords: v_str_list(&assist, "skipKeywords"),
            // 缺省 0：不冷却。负数钳成 0，避免无意义的大值把重试推到很久以后
            backoff_hours: v_i64(&assist, "backoffHours", d.backoff_hours as i64).max(0) as u32,
            // 负值钳成 0：as u64 回绕成巨值会让 push_tail 的轮询 sleep 数年，整轮无声挂死
            verify_wait_sec: v_i64(&assist, "verifyWaitSec", d.verify_wait_sec as i64).max(0)
                as u64,
            verify_poll_sec: v_i64(&assist, "verifyPollSec", d.verify_poll_sec as i64).max(0)
                as u64,
            relaxed_retry_search: v_bool(&assist, "relaxedRetrySearch", d.relaxed_retry_search),
            member_name_fallback: v_bool(&assist, "memberNameFallback", d.member_name_fallback),
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
            skip_post_check: v_bool(&assist, "skipPostCheck", d.skip_post_check),
            skip_hash_check: v_bool(&assist, "skipHashCheck", d.skip_hash_check),
            archive_days: v_i64(&assist, "archiveDays", d.archive_days).max(0),
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
        let mut skipped_sites: Vec<(String, String)> = Vec::new();
        for s in &raw_sites {
            if !v_bool(s, "enabled", false) {
                continue;
            }
            let Some(id) = v_str(s, "id") else { continue };
            if let Some(w) = &whitelist {
                if !w.contains(&id) {
                    skipped_sites.push((id, "不在 assist.siteIDs 白名单".into()));
                    continue;
                }
            }
            if coverage.paused_sites.iter().any(|p| p == &id) {
                skipped_sites.push((id, "assist.pausedSites 暂停".into()));
                continue;
            }
            let Some(url) = v_str(s, "url") else {
                skipped_sites.push((id, "缺 url".into()));
                continue;
            };
            let framework = Framework::parse(&v_str(s, "framework").unwrap_or_default());
            if !framework.search_capable() {
                skipped_sites.push((id, format!("框架 {framework:?} 不支持检索")));
                continue;
            }
            let search_url = s
                .get("overrides")
                .and_then(|o| v_str(o, "searchURL"))
                .filter(|u| !u.is_empty());
            let announce_url = s
                .get("overrides")
                .and_then(|o| v_str(o, "announceURL"))
                .filter(|u| !u.is_empty());
            // 页面 host + 内置别名 + 配置里追加的 overrides.announceHosts + announceURL 自带主机。
            // 配了 announceURL 时把它自己的 host 一并并入清单：独立主机的 tracker（如 iloli 的
            // a.x-anime.cc）不会再被守卫误拦，已在做种的种子也不会被算成缺口。
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
            if let Some(h) = announce_url.as_deref().and_then(url_host) {
                announce_hosts.push(h);
            }
            announce_hosts.retain(|h| !h.is_empty());
            announce_hosts.dedup();
            let poster_field = s
                .get("overrides")
                .and_then(|o| v_str(o, "posterField"))
                .filter(|v| !v.is_empty());
            let screenshot_field = s
                .get("overrides")
                .and_then(|o| v_str(o, "screenshotField"))
                .filter(|v| !v.is_empty());
            let proxy = s
                .get("overrides")
                .and_then(|o| v_str(o, "proxy"))
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
            sites.push(SiteConf {
                announce_hosts,
                announce_url,
                poster_field,
                screenshot_field,
                proxy,
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
            skipped_sites,
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
    /// 或 host 是 jar 键的子域；空值 cookie 只在无过期时间时携带；已过期的一律不发）
    pub fn cookie_header_for(&self, host: &str) -> Option<String> {
        let host = host.to_lowercase();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let mut pairs: Vec<String> = Vec::new();
        for (key, list) in &self.cookies {
            let key = key.to_lowercase();
            if !(host == key || host.ends_with(&format!(".{key}"))) {
                continue;
            }
            for c in list {
                // 过期 cookie 照发只会撞登录墙，白耗请求且账面原因误导
                if c.expiration_date.is_some_and(|e| e > 0.0 && e < now) {
                    continue;
                }
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
        let body = serde_json::to_string_pretty(&self.cookies).context("cookie 序列化失败")?;
        crate::atomic_write(&path, body.as_bytes())?;
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    /// 每个测试线程一个稳定目录：同一测试内的多次写入共享，测试之间不共享。
    fn test_dir() -> PathBuf {
        thread_local! {
            static DIR: PathBuf = {
                let n = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
                let name = std::thread::current()
                    .name()
                    .unwrap_or("unnamed")
                    .replace("::", "-")
                    .replace(['/', '\\'], "-");
                std::env::temp_dir().join(format!("bsend-cfg-{}-{name}-{n}", std::process::id()))
            };
        }
        DIR.with(Clone::clone)
    }

    fn write_temp(name: &str, content: &str) -> PathBuf {
        let dir = test_dir();
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
         "overrides": {"searchURL": "torrents.php?search={name}&search_area=0",
                       "posterField": "url_poster", "screenshotField": "url_vimages"}},
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
            skipped_sites: Vec::new(),
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
        assert_eq!(cfg.sites[0].poster_field.as_deref(), Some("url_poster"));
        assert_eq!(
            cfg.sites[0].screenshot_field.as_deref(),
            Some("url_vimages")
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
    fn proxy_override_parses_or_stays_direct() {
        let json = r#"{
          "dataDir": "/tmp/bsend-proxy",
          "sourceSites": [
            {"id": "gtk", "name": "GTK", "url": "https://pt.gtkpw.xyz/", "framework": "NexusPHP", "enabled": true,
             "overrides": {"proxy": "http://127.0.0.1:7890"}},
            {"id": "empty", "name": "空", "url": "https://empty.example/", "framework": "NexusPHP", "enabled": true,
             "overrides": {"proxy": "  "}},
            {"id": "plain", "name": "直连", "url": "https://plain.example/", "framework": "NexusPHP", "enabled": true}
          ],
          "downloader": {"id": "main", "type": "qbittorrent", "url": "http://127.0.0.1:8080",
                         "username": "u", "password": "p", "skipChecking": true},
          "assist": {"siteIDs": ["gtk", "empty", "plain"]}
        }"#;
        let p = write_temp("boxsend-proxy.json", json);
        let cfg = Config::load(&p).unwrap();
        let by: std::collections::HashMap<_, _> = cfg
            .sites
            .iter()
            .map(|s| (s.id.as_str(), s.proxy.as_deref()))
            .collect();
        assert_eq!(by["gtk"], Some("http://127.0.0.1:7890"));
        assert_eq!(by["empty"], None, "空串/纯空白按直连处理");
        assert_eq!(by["plain"], None, "没配 proxy 保持直连");
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

    // 同一测试内还要再按 tag 分目录时用这个；跨测试已经由 test_dir 隔离。
    fn write_temp_isolated(tag: &str, name: &str, content: &str) -> PathBuf {
        let dir = test_dir().join(tag);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        p
    }

    #[test]
    fn temp_dirs_are_isolated_between_test_threads() {
        let here = test_dir();
        let other = std::thread::Builder::new()
            .name("config-isolation-probe".into())
            .spawn(test_dir)
            .unwrap()
            .join()
            .unwrap();
        assert_ne!(here, other, "不同测试线程不能共用配置临时目录");
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
    fn announce_url_is_loaded_for_first_upload() {
        // 第一次向新站发种时 qB 里没有该站 tracker，只能靠配置里的完整地址
        let p = write_temp_isolated(
            "announce-url",
            "boxsend.json",
            r#"{
          "sourceSites": [
            {"id": "hdsky", "name": "天空", "url": "https://hdsky.me/", "framework": "NexusPHP", "enabled": true,
             "overrides": {"announceURL": "https://tracker.hdsky.me/announce.php?passkey=abc"}},
            {"id": "pter", "name": "猫站", "url": "https://pterclub.com/", "framework": "NexusPHP", "enabled": true}
          ],
          "downloader": {"url": "http://x"}
        }"#,
        );
        let cfg = Config::load(&p).unwrap();
        let sky = cfg.sites.iter().find(|s| s.id == "hdsky").unwrap();
        assert_eq!(
            sky.announce_url.as_deref(),
            Some("https://tracker.hdsky.me/announce.php?passkey=abc")
        );
        // 没配的站保持空，运行时才会回退到从 qB 现有 tracker 里找
        let pter = cfg.sites.iter().find(|s| s.id == "pter").unwrap();
        assert!(pter.announce_url.is_none());
    }

    #[test]
    fn announce_url_host_merged_into_allowed_hosts() {
        // 配了 announceURL 且主机与页面域名不同：页面 host 和 announce host 都进清单，
        // 上传守卫能放行，已在做种的种子也不会被算成缺口。
        let p = write_temp_isolated(
            "announce-host-merge",
            "boxsend.json",
            r#"{
          "sourceSites": [
            {"id": "iloli", "name": "萝莉", "url": "https://mua.xloli.cc/", "framework": "NexusPHP", "enabled": true,
             "overrides": {"announceURL": "https://a.x-anime.cc/announce?passkey=abc"}}
          ],
          "downloader": {"url": "http://x"}
        }"#,
        );
        let cfg = Config::load(&p).unwrap();
        let s = cfg.sites.iter().find(|x| x.id == "iloli").unwrap();
        assert_eq!(
            s.announce_hosts,
            vec!["mua.xloli.cc".to_string(), "a.x-anime.cc".to_string()]
        );
        assert!(s.matches_tracker("a.x-anime.cc"));
        assert!(s.matches_tracker("mua.xloli.cc"));
        assert!(!s.matches_tracker("evil.example"));
    }

    #[test]
    fn announce_hosts_unchanged_without_announce_url() {
        // 没配 announceURL：清单与改动前逐项一致（该 id 无内置别名，只有页面 host）。
        let p = write_temp_isolated(
            "announce-host-plain",
            "boxsend.json",
            r#"{
          "sourceSites": [
            {"id": "hdsky", "name": "天空", "url": "https://hdsky.me/", "framework": "NexusPHP", "enabled": true}
          ],
          "downloader": {"url": "http://x"}
        }"#,
        );
        let cfg = Config::load(&p).unwrap();
        let s = cfg.sites.iter().find(|x| x.id == "hdsky").unwrap();
        assert_eq!(s.announce_hosts, vec!["hdsky.me".to_string()]);
        assert!(s.announce_url.is_none());
    }

    #[test]
    fn announce_url_manual_hosts_and_aliases_merge_without_dup() {
        // announceURL + 手写 announceHosts + 内置别名：三份合并、重复项只留一条。
        let p = write_temp_isolated(
            "announce-host-three-way",
            "boxsend.json",
            r#"{
          "sourceSites": [
            {"id": "multi", "name": "合并", "url": "https://m.example/", "framework": "NexusPHP", "enabled": true,
             "overrides": {"announceURL": "https://a.example/announce?passkey=x", "announceHosts": ["b.example", "c.example", "a.example"]}}
          ],
          "downloader": {"url": "http://x"}
        }"#,
        );
        let cfg = Config::load(&p).unwrap();
        let s = cfg.sites.iter().find(|x| x.id == "multi").unwrap();
        assert_eq!(
            s.announce_hosts,
            vec!["m.example", "b.example", "c.example", "a.example"]
        );
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

    #[test]
    fn negative_verify_timings_are_clamped() {
        // verifyWaitSec/verifyPollSec 负值不能 as u64 回绕成巨值（轮询 sleep 数年）
        let p = write_temp_isolated(
            "negverify",
            "boxsend.json",
            r#"{
          "sourceSites": [
            {"id": "nx", "name": "正常", "url": "https://nx.example/", "framework": "NexusPHP", "enabled": true}
          ],
          "downloader": {"url": "http://x"},
          "assist": {"verifyWaitSec": -10, "verifyPollSec": -3}
        }"#,
        );
        let cfg = Config::load(&p).unwrap();
        assert_eq!(cfg.coverage.verify_wait_sec, 0);
        assert_eq!(cfg.coverage.verify_poll_sec, 0);
    }

    #[test]
    fn skipped_sites_are_reported_with_reason() {
        // 启用但被框架/字段排除的站必须点名，不能让用户以为它们参与了
        let p = write_temp_isolated(
            "skipped",
            "boxsend.json",
            r#"{
          "sourceSites": [
            {"id": "u3", "name": "U3", "url": "https://u3.example/", "framework": "Unit3D", "enabled": true},
            {"id": "nourl", "name": "NoUrl", "framework": "NexusPHP", "enabled": true},
            {"id": "nx", "name": "NX", "url": "https://nx.example/", "framework": "NexusPHP", "enabled": true}
          ],
          "downloader": {"url": "http://x"}
        }"#,
        );
        let cfg = Config::load(&p).unwrap();
        assert_eq!(cfg.sites.len(), 1, "只有 nx 可用");
        let notice = cfg.skipped_notice().expect("应有排除提示");
        assert!(
            notice.contains("u3") && notice.contains("Unit3D"),
            "{notice}"
        );
        assert!(
            notice.contains("nourl") && notice.contains("缺 url"),
            "{notice}"
        );
    }

    #[test]
    fn image_floor_is_hardcoded_and_config_cannot_override() {
        // 2026-11-24：要图的站至少 3 张，改成代码硬规则；配置里写多少都不算。2026-11-25 加 ptlgs 4 张。
        let mut settings = crate::config::CoverageSettings::default();
        assert_eq!(settings.rules_for("qilin").require_images, Some(3));
        assert_eq!(settings.rules_for("ubits").require_images, Some(3));
        assert_eq!(settings.rules_for("tey").require_images, Some(3));
        assert_eq!(settings.rules_for("cangbao").require_images, Some(3));
        // 熊猫站规明说 4 张，保留更严的门槛
        assert_eq!(settings.rules_for("panda").require_images, Some(4));
        // ptlgs 要求 1 张海报 + 至少 3 张截图，总图数按 4 张
        assert_eq!(settings.rules_for("ptlgs").require_images, Some(4));
        // 不要求图的站没有这道门禁
        assert_eq!(settings.rules_for("sbpt").require_images, None);

        settings.reseed_rules.insert(
            "qilin".into(),
            crate::gates::SiteRules {
                require_images: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(
            settings.rules_for("qilin").require_images,
            Some(3),
            "配置写的 2 张必须被硬规则覆盖"
        );
    }
}
