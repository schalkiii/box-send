//! 视觉令牌：颜色 / 尺寸 / 字号节奏的唯一真源。
//! 视图里禁止再写散装 magic number（rgb(0x...) / px(13.) 之类），要改配色或节奏只动这里。

use gpui::{px, Pixels};

// ============ 颜色（GitHub dark 系；文本对 #161b22 的对比度：#e6edf3 13.4:1、#9aa4af 6.6:1、#6e7681 3.6:1）============
/// 窗口底色
pub(crate) const BG: u32 = 0x0d1117;
/// 卡片 / 面板
pub(crate) const SURFACE: u32 = 0x161b22;
/// 控制条 / 次级块 / 次级按钮
pub(crate) const SURFACE_ALT: u32 = 0x1c2128;
/// 悬停态底 / 表头底
pub(crate) const SURFACE_HOVER: u32 = 0x21262d;
/// 边框
pub(crate) const BORDER: u32 = 0x30363d;
/// 细分隔线（行间）
pub(crate) const BORDER_SUBTLE: u32 = 0x21262d;
/// 正文
pub(crate) const TEXT: u32 = 0xe6edf3;
/// 次要说明
pub(crate) const TEXT_DIM: u32 = 0x9aa4af;
/// 三级提示（弱化，仅用于非关键信息）
pub(crate) const TEXT_FAINT: u32 = 0x6e7681;
/// 强调色（标题 / 链接 / 进度条）
pub(crate) const ACCENT: u32 = 0x58a6ff;
/// 强调底色（选中态 / 主 tab）
pub(crate) const ACCENT_BG: u32 = 0x1f6feb;
/// 成功色（做种 / 已覆盖）
pub(crate) const SUCCESS: u32 = 0x3fb950;
/// 成功底色（主操作按钮）
pub(crate) const SUCCESS_BG: u32 = 0x238636;
/// 成功悬停
pub(crate) const SUCCESS_HOVER: u32 = 0x2ea043;
/// 警告色（试运行 / 校验中）
pub(crate) const WARN: u32 = 0xd29922;
/// 警告底色
pub(crate) const WARN_BG: u32 = 0x9e6a03;
/// 危险色（失败 / 缺失）
pub(crate) const DANGER: u32 = 0xf85149;
/// 危险底色（真发布 / 删除）
pub(crate) const DANGER_BG: u32 = 0xda3633;
/// 危险悬停
pub(crate) const DANGER_HOVER: u32 = 0xe5534b;

// ============ 节奏（间距）============
pub(crate) const S1: Pixels = px(4.);
pub(crate) const S2: Pixels = px(8.);
pub(crate) const S3: Pixels = px(12.);
pub(crate) const S4: Pixels = px(16.);
pub(crate) const S5: Pixels = px(24.);

// ============ 组件尺寸 ============
/// 顶部标题栏高度
pub(crate) const HEADER_H: Pixels = px(54.);
/// tab / 动作工具栏高度
pub(crate) const TOOLBAR_H: Pixels = px(46.);
/// 底部状态栏高度
pub(crate) const STATUS_H: Pixels = px(28.);
/// 表格行高（固定，长列表靠滚动而不是撑高）
pub(crate) const ROW_H: Pixels = px(30.);
/// 表头行高
pub(crate) const HEAD_H: Pixels = px(28.);
/// 草稿卡片高度
pub(crate) const CARD_H: Pixels = px(78.);
/// 按钮高度（≥28 才够点）
pub(crate) const BTN_H: Pixels = px(30.);
/// 小 chip 高度
pub(crate) const CHIP_H: Pixels = px(24.);
/// 日志面板宽度
pub(crate) const LOG_W: Pixels = px(340.);
/// 日志行高
pub(crate) const LOG_ROW_H: Pixels = px(19.);
/// 窗口最小尺寸（6 列表 + 页头/KPI/控制条 + 至少 8 行表格刚好放得下；再小就必然重叠，直接挡住）
pub(crate) const MIN_WIN_W: Pixels = px(1060.);
pub(crate) const MIN_WIN_H: Pixels = px(720.);

// ============ 表格列宽（表头与数据行共用同一组常量，避免错列 / 溢出）============
// 概览表 6 列：站点 / 已有 / 缺口 / 覆盖率 / 草稿 / 已跳过
pub(crate) const COL_SITE: Pixels = px(168.);
pub(crate) const COL_NUM: Pixels = px(72.);
pub(crate) const COL_RATIO: Pixels = px(130.);
/// 站点 chip 选择区最大高度（4 行整：24×4 + 间距 4×3，超出内部滚动，不再把表格顶出去）
pub(crate) const CHIP_ZONE_MAX_H: Pixels = px(108.);
