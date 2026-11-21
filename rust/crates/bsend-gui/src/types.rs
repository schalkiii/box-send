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
    pub(crate) count: usize,
    pub(crate) color: u32,
}

#[derive(Clone)]
pub(crate) struct SiteRow {
    pub(crate) site: String,
    pub(crate) cells: usize,
    pub(crate) covered: usize,
    pub(crate) pending: usize,
    pub(crate) ratio: f64,
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
