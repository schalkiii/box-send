//! GUI 的视图状态与事件类型（纯数据，无逻辑）。
//! 拆出 main.rs 只为可读性：字段语义与内联时期完全一致。

use bsend::report::DraftRow;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Overview,
    Drafts,
    Autopost,
    Config,
    Diag,
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Job {
    Scan,
    Prime,
    Round,
    Autopost,
    TestQb,
    PullCookies,
    Refresh,
}

#[derive(Clone)]
pub(crate) struct StatusChip {
    pub(crate) label: &'static str,
    /// 该结论的格子数（图例汇总展示）
    pub(crate) count: usize,
    pub(crate) color: u32,
}

#[derive(Clone)]
pub(crate) struct SiteRow {
    pub(crate) site: String,
    /// qB 真值口径：qB 里带该站 tracker 的内容数（`SiteStats::qb_covered`）
    pub(crate) have: usize,
    /// 缺口 = 全库内容数 − 已有（`SiteStats::qb_missing`）
    pub(crate) gap: usize,
    /// 覆盖率 = 已有 / 全库内容数（`SiteStats::qb_ratio`）
    pub(crate) ratio: f64,
    /// 该站待发草稿数（按目标站展开的草稿条数）
    pub(crate) drafts: usize,
    /// 终态跳过（内容黑名单等，`CoverageStatus::Skipped`）的格子数
    pub(crate) skipped: usize,
    /// 该站各结论的分布（图例与状态分布提示用）
    pub(crate) chips: Vec<StatusChip>,
}

#[derive(Clone)]
pub(crate) struct Snapshot {
    pub(crate) stats: String,
    pub(crate) sites: Vec<SiteRow>,
    pub(crate) drafts: Vec<DraftRow>,
}

pub(crate) enum GuiEvent {
    Log(String),
    Snapshot(Snapshot),
    Busy(bool),
    /// 跑一轮时每格结论（含跳过）计一次，供进度显示
    CellDone,
    /// CookieCloud 拉取合并后通知视图热重载配置
    ReloadConfig,
}
