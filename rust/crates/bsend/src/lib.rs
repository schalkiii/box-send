//! BoxSend 辅种引擎（Rust 版）：内容索引 → 源种定位 → 转种/辅种一体化。
//!
//! 与 Swift 版 `BoxSendKit` 的关系：判定口径（`matcher`）、bencode 语义（`bencode`）、
//! qB 字段兼容（`qbittorrent`）、NexusPHP 检索启发式（`sites::nexusphp`）全部逐函数对齐；
//! 编排层（`coverage`）按 `COVERAGE-PLAN.md` 重写：内容级索引、站点 worker 并发、
//! 转种只出草稿。设计文档见仓库根目录 `COVERAGE-PLAN.md` 与 `RUST-ANALYSIS.md`。

pub mod bencode;
pub mod config;
pub mod cookiecloud;
pub mod coverage;
pub mod index;
pub mod layout;
pub mod manifest;
pub mod matcher;
pub mod mediainfo;
pub mod models;
pub mod qbittorrent;
pub mod report;
pub mod reseed;
pub mod resolver;
pub mod sites;
pub mod throttle;

pub use config::{Config, CookieEntry, CoverageSettings, DownloaderSettings, ReseedMode, SiteConf};
pub use cookiecloud::{CookieCloud, CookieCloudSettings};
pub use coverage::{run_round, DraftItem, Event, RoundOptions, RoundReport};
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
