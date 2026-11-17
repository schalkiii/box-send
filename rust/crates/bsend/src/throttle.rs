//! 每站节流 + 轮次预算。Swift 版是「全流水线一个节奏 + 每站 8s+3s 抖动 + 整轮 200 次上限」，
//! Rust 版把站拆成并行 worker 后，节流必须内聚成对象：每站自己的节奏、每类动作自己的预算，
//! 跨线程共享（`Mutex` 内部状态），站点返回限流标记还能把该站间隔翻倍退避。

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rand::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BudgetKind {
    /// 目标站检索（辅种臂：搜目标站 + 下载目标站 .torrent 判定）
    Search,
    /// 源站请求（源种定位：搜源站 + 下载源站 .torrent）
    Source,
    /// 转种上传（推送到目标站；v1 草稿模式不动它，预算只在放开上传后生效）
    Upload,
}

impl BudgetKind {
    pub fn label(&self) -> &'static str {
        match self {
            BudgetKind::Search => "search",
            BudgetKind::Source => "source",
            BudgetKind::Upload => "upload",
        }
    }
}

struct SitePace {
    /// 上次请求的（计划）发出时刻。等待未睡满时会落在未来——下次领号自然把剩余时间算进去
    last: Option<Instant>,
    /// 限流退避倍数（1 = 正常；每次被限流翻倍，封顶 30）
    backoff: u32,
    /// 该站本轮的最低间隔（被限流页警告过就抬到 60 秒）。
    /// hdsky 实机：HTTP 200 + 「请求过于频繁，限制访问间隔 60 秒/次」——8 秒节奏只会反复撞墙。
    min_interval: Duration,
}

impl SitePace {
    fn new() -> Self {
        Self {
            last: None,
            backoff: 1,
            min_interval: Duration::ZERO,
        }
    }
}

/// 限流退避封顶：240s = 8s × 30
const MAX_BACKOFF: u32 = 30;

/// 收到限流信号后该站本轮的最低请求间隔的缺省值（PT 站常见的「60 秒/次」）
const DEFAULT_RATE_LIMIT_FLOOR: Duration = Duration::from_secs(60);

/// 纯函数：距上次请求 `elapsed` 后还要等多久。`last=None`（首访）立即放行。
fn next_wait(has_last: bool, elapsed: Duration, interval: Duration, backoff: u32) -> Duration {
    if !has_last {
        return Duration::ZERO;
    }
    (interval * backoff).saturating_sub(elapsed)
}

pub struct SiteThrottle {
    interval: Duration,
    jitter: Duration,
    /// 逐站覆盖的请求间隔（站点明说 60 秒/次的，别用全局 8 秒去撞）
    site_intervals: HashMap<String, Duration>,
    /// 被限流后该站本轮的最低间隔
    rate_limit_floor: Duration,
    pace: Mutex<HashMap<String, SitePace>>,
    used: Mutex<HashMap<BudgetKind, i32>>,
    max: HashMap<BudgetKind, i32>,
}

impl SiteThrottle {
    pub fn new(
        interval: Duration,
        jitter: Duration,
        search_budget: i32,
        source_budget: i32,
        upload_budget: i32,
    ) -> Self {
        Self::with_rate_limit_floor(
            interval,
            jitter,
            search_budget,
            source_budget,
            upload_budget,
            DEFAULT_RATE_LIMIT_FLOOR,
        )
    }

    /// 带「限流最低间隔」的构造（配置可调；测试把它压到 1s 免得白等）
    #[allow(clippy::too_many_arguments)]
    pub fn with_rate_limit_floor(
        interval: Duration,
        jitter: Duration,
        search_budget: i32,
        source_budget: i32,
        upload_budget: i32,
        rate_limit_floor: Duration,
    ) -> Self {
        Self {
            interval,
            site_intervals: HashMap::new(),
            jitter,
            rate_limit_floor,
            pace: Mutex::new(HashMap::new()),
            used: Mutex::new(HashMap::new()),
            max: [
                (BudgetKind::Search, search_budget),
                (BudgetKind::Source, source_budget),
                (BudgetKind::Upload, upload_budget),
            ]
            .into_iter()
            .collect(),
        }
    }

    /// 逐站覆盖请求间隔（链式构造：全局默认 + 少数站单独放宽）
    pub fn with_site_intervals(mut self, map: HashMap<String, Duration>) -> Self {
        self.site_intervals = map;
        self
    }

    /// 预算余量（纯查询，报告用）
    pub fn budget_left(&self, kind: BudgetKind) -> i32 {
        let used = self.used.lock().unwrap().get(&kind).copied().unwrap_or(0);
        self.max[&kind] - used
    }

    pub fn used(&self, kind: BudgetKind) -> i32 {
        self.used.lock().unwrap().get(&kind).copied().unwrap_or(0)
    }

    pub fn budget_max(&self, kind: BudgetKind) -> i32 {
        self.max[&kind]
    }

    /// 站点报了限流（429 / 503 / 限流提示页）：该站下次请求间隔翻倍，并把本轮最低间隔
    /// 抬到 60 秒（站点明说「限制访问间隔 60 秒/次」时，8 秒节奏只会反复撞墙）。
    pub fn note_rate_limited(&self, site_id: &str) {
        let mut pace = self.pace.lock().unwrap();
        let p = pace
            .entry(site_id.to_string())
            .or_insert_with(SitePace::new);
        p.backoff = (p.backoff * 2).min(MAX_BACKOFF);
        p.min_interval = p.min_interval.max(self.rate_limit_floor);
    }

    /// 站点一切正常时恢复原节奏（一次成功的请求就该松开退避与最低间隔）
    pub fn note_ok(&self, site_id: &str) {
        let mut pace = self.pace.lock().unwrap();
        if let Some(p) = pace.get_mut(site_id) {
            p.backoff = 1;
            p.min_interval = Duration::ZERO;
        }
    }

    /// 领号：预算够 → 计算该站下一次允许时刻距现在的等待时长，记账（预算 -1、刷新该站时刻），
    /// 返回 `Some(wait)`；预算用完返回 `None`。真正睡多长时间由调用方决定（测试可以不睡）。
    pub fn acquire_dur(&self, site_id: &str, kind: BudgetKind) -> Option<Duration> {
        {
            // 预算以「动作为单位」而不以「站点为单位」：一轮辅种里同一个站可能被
            // 检索臂和源种臂各碰一次，两边都该被计数。
            let mut used = self.used.lock().unwrap();
            let u = used.entry(kind).or_insert(0);
            if *u >= self.max[&kind] {
                return None;
            }
            *u += 1;
        }
        let now = Instant::now();
        let mut pace = self.pace.lock().unwrap();
        let p = match pace.entry(site_id.to_string()) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(SitePace::new()),
        };
        let wait = match p.last {
            None => Duration::ZERO,
            Some(last) => {
                let elapsed = now.saturating_duration_since(last);
                // 退避倍数与「该站本轮最低间隔」取较大者；站间隔按该站自己的算
                let base = self
                    .site_intervals
                    .get(site_id)
                    .copied()
                    .unwrap_or(self.interval);
                next_wait(true, elapsed, base, p.backoff)
                    .max(p.min_interval.saturating_sub(elapsed))
            }
        };
        // 抖动在等待之上再加：让并行 worker 的节奏彼此错开
        let jitter = if self.jitter.is_zero() {
            Duration::ZERO
        } else {
            let ms = self.jitter.as_millis() as u64;
            Duration::from_millis(rand::thread_rng().gen_range(0..=ms))
        };
        p.last = Some(now + wait);
        Some(wait + jitter)
    }

    /// 领号并睡到允许时刻（生产路径）
    pub fn acquire(&self, site_id: &str, kind: BudgetKind) -> bool {
        match self.acquire_dur(site_id, kind) {
            Some(d) => {
                if !d.is_zero() {
                    std::thread::sleep(d);
                }
                true
            }
            None => false,
        }
    }

    /// 追加预算（人工确认后继续跑剩余计划时用）
    pub fn add_budget(&self, kind: BudgetKind, n: i32) {
        let mut used = self.used.lock().unwrap();
        let u = used.entry(kind).or_insert(0);
        *u = (*u - n).max(0);
    }

    /// 测试专用：把某站的「上次请求时刻」拨到 `ago` 之前
    #[cfg(test)]
    pub(crate) fn force_last(&self, site_id: &str, ago: Duration) {
        let mut pace = self.pace.lock().unwrap();
        let p = pace
            .entry(site_id.to_string())
            .or_insert_with(SitePace::new);
        p.last = Some(Instant::now() - ago);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_jitter() -> SiteThrottle {
        // 小预算：预算类测试的道具
        SiteThrottle::new(Duration::from_secs(8), Duration::ZERO, 3, 2, 1)
    }

    fn paced() -> SiteThrottle {
        // 大预算：节奏/退避类测试不担心配额
        SiteThrottle::new(Duration::from_secs(8), Duration::ZERO, 100, 100, 100)
    }

    /// force_last 与 acquire 之间隔着几微秒的真实墙钟，elapsed 会比拨的钟多一点点，
    /// 所以等待时长只能断「接近」，不能断相等
    fn assert_near(d: Duration, expected_secs: u64) {
        let e = Duration::from_secs(expected_secs);
        let tol = Duration::from_millis(200);
        assert!(
            d >= e.saturating_sub(tol) && d <= e + tol,
            "got {d:?}, expected ~{e:?}"
        );
    }

    #[test]
    fn next_wait_math_is_exact() {
        let d = Duration::from_secs;
        // 首访立即放行
        assert_eq!(next_wait(false, d(0), d(8), 1), Duration::ZERO);
        // 上次请求在 100s 前：早过间隔，立即放行
        assert_eq!(next_wait(true, d(100), d(8), 1), Duration::ZERO);
        // 上次在 3s 前：8s 间隔还差 5s
        assert_eq!(next_wait(true, d(3), d(8), 1), d(5));
        // 退避 ×4：需要 32s，还差 29s
        assert_eq!(next_wait(true, d(3), d(8), 4), d(29));
        // elapsed 超出也不会溢出成负数（saturating）
        assert_eq!(next_wait(true, d(999), d(8), 30), Duration::ZERO);
    }

    #[test]
    fn budget_decrements_and_runs_out() {
        let t = no_jitter();
        assert!(t.acquire_dur("hdsky", BudgetKind::Search).is_some());
        assert!(t.acquire_dur("hdsky", BudgetKind::Search).is_some());
        assert!(t.acquire_dur("pter", BudgetKind::Search).is_some());
        // search 预算 3 已用完，别的站也领不到
        assert!(t.acquire_dur("hdhome", BudgetKind::Search).is_none());
        // 但其它类别的预算还在
        assert!(t.acquire_dur("hdsky", BudgetKind::Source).is_some());
        assert_eq!(t.used(BudgetKind::Search), 3);
        assert_eq!(t.budget_left(BudgetKind::Search), 0);
        assert_eq!(t.budget_left(BudgetKind::Source), 1);
    }

    #[test]
    fn add_budget_extends_round() {
        let t = no_jitter();
        for _ in 0..3 {
            t.acquire_dur("a", BudgetKind::Search).unwrap();
        }
        assert!(t.acquire_dur("a", BudgetKind::Search).is_none());
        t.add_budget(BudgetKind::Search, 2);
        assert_eq!(t.budget_left(BudgetKind::Search), 2);
        assert!(t.acquire_dur("a", BudgetKind::Search).is_some());
    }

    #[test]
    fn first_call_is_immediate_and_paces_afterwards() {
        let t = paced();
        // 同站第一次立即放行
        assert_eq!(
            t.acquire_dur("hdsky", BudgetKind::Source).unwrap(),
            Duration::ZERO
        );
        // 第二次要等满间隔（不真睡；用拨钟避免墙钟误差）
        t.force_last("hdsky", Duration::from_secs(1));
        assert_near(t.acquire_dur("hdsky", BudgetKind::Source).unwrap(), 7);
        // 距上次 100s：立即放行
        t.force_last("hdsky", Duration::from_secs(100));
        assert_eq!(
            t.acquire_dur("hdsky", BudgetKind::Source).unwrap(),
            Duration::ZERO
        );
        // 另一个站不受影响
        assert_eq!(
            t.acquire_dur("pter", BudgetKind::Upload).unwrap(),
            Duration::ZERO
        );
    }

    #[test]
    fn rate_limit_backoff_doubles_with_cap() {
        let t = paced();
        t.acquire_dur("hdsky", BudgetKind::Search).unwrap();
        t.note_rate_limited("hdsky"); // ×2，且本轮最低间隔抬到 60s
        t.force_last("hdsky", Duration::from_secs(1));
        // max(8×2 − 1, 60 − 1) = 59：站点说 60 秒/次，就别按 15 秒去撞墙
        assert_near(t.acquire_dur("hdsky", BudgetKind::Search).unwrap(), 59);
        // 连着限流翻到封顶 30 倍
        t.note_rate_limited("hdsky");
        t.note_rate_limited("hdsky");
        t.note_rate_limited("hdsky");
        t.note_rate_limited("hdsky");
        t.force_last("hdsky", Duration::from_secs(1));
        assert_near(t.acquire_dur("hdsky", BudgetKind::Search).unwrap(), 239); // 8×30 − 1
                                                                               // 一次成功请求恢复原节奏
        t.note_ok("hdsky");
        t.force_last("hdsky", Duration::from_secs(1));
        assert_near(t.acquire_dur("hdsky", BudgetKind::Search).unwrap(), 7);
    }
}
