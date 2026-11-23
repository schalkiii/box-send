//! BoxSend 辅种引擎（Rust 版）：内容索引 → 源种定位 → 转种/辅种一体化。
//!
//! 与 Swift 版 `BoxSendKit` 的关系：判定口径（`matcher`）、bencode 语义（`bencode`）、
//! qB 字段兼容（`qbittorrent`）、NexusPHP 检索启发式（`sites::nexusphp`）全部逐函数对齐；
//! 编排层（`coverage`）：内容级索引、站点 worker 并发、
//! 辅种真推下载器、转种出草稿并经 `autopost` 门禁发布。
//! 设计与审计文档见 `doc/ARCHITECTURE.md` 与 `doc/ROADMAP.md`。

pub mod autopost;
pub mod bencode;
pub mod config;
pub mod cookiecloud;
pub mod coverage;
pub mod gates;
pub mod index;
pub mod layout;
pub mod lock;
pub mod manifest;
pub mod matcher;
pub mod mediainfo;
pub mod models;
pub mod piecesapi;
pub mod qbittorrent;
pub mod report;
pub mod reseed;
pub mod resolver;
pub mod sites;
pub mod throttle;
pub mod torrent_rewrite;

pub use config::{Config, CookieEntry, CoverageSettings, DownloaderSettings, ReseedMode, SiteConf};
pub use cookiecloud::{CookieCloud, CookieCloudSettings};
pub use coverage::{run_round, DraftItem, Event, RoundOptions, RoundReport};
pub use gates::{GateInput, GateResult, SiteRules, Verdict};
pub use index::{
    ContentEntry, ContentIndex, CoverageStatus, ScanDelta, SiteCoverage, SourceRecord, TorrentMeta,
};
pub use layout::AddLayout;
pub use models::{JudgeLevel, Layout, LocalTorrent, TorrentFingerprint};
pub use qbittorrent::{AddTorrentResult, QBittorrent};
pub use report::CoverageReport;
pub use resolver::{resolve_source, ResolveOutcome, SiteClient, SourceSite};
pub use sites::{builtin_sites, url_host, Framework, SiteDef};
pub use throttle::{BudgetKind, SiteThrottle};

/// 原子写：先写 `.tmp` 再 rename（进程中断/磁盘满时旧文件保持完好）。
/// 索引/配置已有各自的实现；pieces-cache、formcheck/gatecheck 等台账统一走这里。
pub fn atomic_write(path: &std::path::Path, body: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).with_context(|| format!("写不进 {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("落盘 {} 失败", path.display()))?;
    Ok(())
}
