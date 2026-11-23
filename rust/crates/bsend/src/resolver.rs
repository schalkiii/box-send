//! 源种定位（方案第五节）：对一段内容，在源站名单里找回一条可用于转种的源种。
//!
//! 优先级：1) 已在做种的源站（trackerHosts 里有它的 announce host——本地就挂着那站的
//! 种子，只在那一个站检索，1–3 次请求）；2) 源站名单按序检索。判定复用 `matcher::judge`：
//! 下载候选 .torrent，A 级（同 hash）或 B 级（manifest 全等）才算定位成功；C 级名字像但
//! 清单对不上 → 不采，换下一站。失败记 `source.missAt` 负缓存（TTL 更长，默认 30 天：
//! 源站新发种比目标站慢，没必要常回去问）。

use anyhow::Result;

use crate::matcher;
use crate::models::{JudgeLevel, TorrentFingerprint};
use crate::sites::nexusphp::{NexusPhp, SearchOutcome, TorrentPage};
use crate::throttle::{BudgetKind, SiteThrottle};

/// 站点客户端的最小面（NexusPHP 适配器实现；测试挂 mock）。
/// `search` 的 `gate` 是「每次真实 HTTP 请求前领一次号」：查询词阶梯每级都是一发请求，
/// 预算与站间隔必须按请求记，不能按 `search` 调用记。
pub trait SiteClient {
    fn site_id(&self) -> &str;
    /// 站内检索；`relaxed` = 宽松查询词兜底
    fn search(
        &self,
        name: &str,
        relaxed: bool,
        gate: &mut dyn FnMut() -> bool,
    ) -> Result<SearchOutcome>;
    fn fetch_torrent_page(&self, detail_url: &str) -> Result<TorrentPage>;
    fn download_torrent(&self, page: &TorrentPage, referer: &str) -> Result<Vec<u8>>;
}

impl SiteClient for NexusPhp {
    fn site_id(&self) -> &str {
        &self.site_id
    }

    fn search(
        &self,
        name: &str,
        relaxed: bool,
        gate: &mut dyn FnMut() -> bool,
    ) -> Result<SearchOutcome> {
        // 源种定位没有副标题/IMDb（那俩来自源站详情页，这里正是在找它）
        NexusPhp::search(self, name, "", relaxed, "", gate)
    }

    fn fetch_torrent_page(&self, detail_url: &str) -> Result<TorrentPage> {
        NexusPhp::fetch_torrent_page(self, detail_url)
    }

    fn download_torrent(&self, page: &TorrentPage, referer: &str) -> Result<Vec<u8>> {
        NexusPhp::download_torrent(self, page, referer)
    }
}

/// 一个源站候选：客户端 + 它的 announce host 列表（判断「本地已在那个站做种」）。
/// 用列表而不是单个 host：站点页面域名与 announce 域名未必一致（实机 luckpt/chdbits）。
pub struct SourceSite<'a> {
    pub client: &'a dyn SiteClient,
    pub announce_hosts: Vec<String>,
}

/// 单站尝试的记录（报告用）
#[derive(Debug, Clone)]
pub struct SiteAttempt {
    pub site_id: String,
    pub note: String,
}

#[derive(Debug)]
pub enum ResolveOutcome {
    /// 定位成功：源站 id + 详情页 + 发布名 + .torrent 字节 + 判定级别
    Resolved {
        site_id: String,
        detail_url: String,
        name: String,
        data: Vec<u8>,
        level: JudgeLevel,
        note: String,
    },
    /// 全部源站都没有。`clean = false` 表示有站检索报错（cookie 失效、500 一类）——
    /// 那不能记「站内没有」负缓存，否则整站被钉住 30 天不再问
    Miss {
        attempts: Vec<SiteAttempt>,
        clean: bool,
    },
    /// 源站预算用完，剩下的站下次再试
    BudgetExhausted { attempts: Vec<SiteAttempt> },
}

/// 定位源种。每个站点请求（检索/宽松检索/详情页/下载）都过一次 `throttle`
/// （源站类预算，与 Swift 辅种的记账粒度一致），站内保持间隔。
pub fn resolve_source(
    name: &str,
    tracker_hosts: &[String],
    local_hashes: &[String],
    local: &TorrentFingerprint,
    sites: &[SourceSite<'_>],
    relaxed_retry: bool,
    throttle: &SiteThrottle,
) -> ResolveOutcome {
    let mut attempts: Vec<SiteAttempt> = Vec::new();
    let mut clean = true;
    // 优先级：已在做种的源站排前面（其余保持给定顺序）
    let mut ordered: Vec<&SourceSite<'_>> = sites.iter().collect();
    ordered.sort_by_key(|s| {
        !tracker_hosts.iter().any(|h| {
            s.announce_hosts
                .iter()
                .any(|site_host| crate::sites::host_matches(h, site_host))
        })
    });

    for site in ordered {
        let site_id = site.client.site_id().to_string();
        // gate 自己持有站 id（move），调用点上就不用为了它挡住 site_id 的所有权
        let gate_site = site_id.clone();
        let mut gate = move || throttle.acquire(&gate_site, BudgetKind::Source);
        // 严格查询词先行（阶梯每级一发请求，gate 逐发领号）
        let mut found = match site.client.search(name, false, &mut gate) {
            Ok(SearchOutcome::Hit(h)) => Some(h),
            Ok(SearchOutcome::Miss) => None,
            Ok(SearchOutcome::BudgetExhausted) => {
                attempts.push(SiteAttempt {
                    site_id,
                    note: "源站预算用完".to_string(),
                });
                return ResolveOutcome::BudgetExhausted { attempts };
            }
            Err(e) => {
                clean = false;
                if crate::sites::nexusphp::error_is_rate_limited(&e) {
                    throttle.note_rate_limited(&site_id);
                }
                attempts.push(SiteAttempt {
                    site_id,
                    note: format!("站内检索失败：{e:#}"),
                });
                continue;
            }
        };
        // 严格没命中且允许宽松兜底 → 再走一轮（同样逐发领号）
        if found.is_none() && relaxed_retry {
            match site.client.search(name, true, &mut gate) {
                Ok(SearchOutcome::Hit(h)) => found = Some(h),
                Ok(SearchOutcome::Miss) => {}
                Ok(SearchOutcome::BudgetExhausted) => {
                    attempts.push(SiteAttempt {
                        site_id,
                        note: "源站预算用完".to_string(),
                    });
                    return ResolveOutcome::BudgetExhausted { attempts };
                }
                Err(e) => {
                    clean = false;
                    if crate::sites::nexusphp::error_is_rate_limited(&e) {
                        throttle.note_rate_limited(&site_id);
                    }
                    attempts.push(SiteAttempt {
                        site_id,
                        note: format!("宽松检索失败：{e:#}"),
                    });
                    continue;
                }
            }
        }
        let Some(hit) = found else {
            attempts.push(SiteAttempt {
                site_id: site_id.clone(),
                note: "站内没有".to_string(),
            });
            throttle.note_ok(&site_id);
            continue;
        };
        // 命中 → 取详情页 → 下载 .torrent → 判定
        if !throttle.acquire(&site_id, BudgetKind::Source) {
            attempts.push(SiteAttempt {
                site_id,
                note: "源站预算用完".to_string(),
            });
            return ResolveOutcome::BudgetExhausted { attempts };
        }
        let page = match site.client.fetch_torrent_page(&hit.detail_url) {
            Ok(p) => p,
            Err(e) => {
                attempts.push(SiteAttempt {
                    site_id,
                    note: format!("详情页读不下来：{e:#}"),
                });
                continue;
            }
        };
        if !throttle.acquire(&site_id, BudgetKind::Source) {
            attempts.push(SiteAttempt {
                site_id,
                note: "源站预算用完".to_string(),
            });
            return ResolveOutcome::BudgetExhausted { attempts };
        }
        let data = match site.client.download_torrent(&page, &hit.detail_url) {
            Ok(d) => d,
            Err(e) => {
                attempts.push(SiteAttempt {
                    site_id,
                    note: format!("下载源种失败：{e:#}"),
                });
                continue;
            }
        };
        throttle.note_ok(&site_id);
        let (level, note) = matcher::judge(local_hashes, local, &data, &page.name);
        match level {
            JudgeLevel::SameHash { .. } | JudgeLevel::SameFiles => {
                return ResolveOutcome::Resolved {
                    site_id,
                    detail_url: hit.detail_url,
                    name: page.name,
                    data,
                    level,
                    note,
                };
            }
            // C 级（清单对不上）不采：名字像不算数，换下一站
            _ => attempts.push(SiteAttempt {
                site_id,
                note: format!("候选清单对不上：{note}"),
            }),
        }
    }
    ResolveOutcome::Miss { attempts, clean }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode;
    use crate::bencode::tests::{enc, BVal};
    use crate::sites::nexusphp::SearchHit;
    use std::cell::RefCell;
    use std::time::Duration;

    /// mock 站：可编程的检索/下载行为
    struct MockSite {
        id: &'static str,
        search_result: RefCell<Result<Option<SearchHit>, String>>,
        torrent_data: RefCell<Vec<u8>>,
    }

    impl MockSite {
        fn no_hit(id: &'static str) -> Self {
            Self::with_torrent(id, Vec::new())
        }

        fn with_torrent(id: &'static str, data: Vec<u8>) -> Self {
            Self {
                id,
                search_result: RefCell::new(Ok(None)),
                torrent_data: RefCell::new(data),
            }
        }
    }

    impl SiteClient for MockSite {
        fn site_id(&self) -> &str {
            self.id
        }

        fn search(
            &self,
            _name: &str,
            _relaxed: bool,
            gate: &mut dyn FnMut() -> bool,
        ) -> Result<SearchOutcome> {
            // 一个 mock 站一次检索 = 一发请求（真实站会按查询词阶梯发多发，由 gate 计数）
            if !gate() {
                return Ok(SearchOutcome::BudgetExhausted);
            }
            let r = self.search_result.borrow();
            match &*r {
                Ok(None) => Ok(SearchOutcome::Miss),
                Ok(Some(h)) => Ok(SearchOutcome::Hit(h.clone())),
                Err(e) => Err(anyhow::anyhow!("{e}")),
            }
        }

        fn fetch_torrent_page(&self, _detail_url: &str) -> Result<TorrentPage> {
            Ok(TorrentPage {
                name: format!("{}.1080p-GROUP", self.id),
                download_url: format!("https://{}/download.php?id=1", self.id),
                size_bytes: None,
                uploader_uid: None,
                uploader_name: None,
            })
        }

        fn download_torrent(&self, _page: &TorrentPage, _referer: &str) -> Result<Vec<u8>> {
            let d = self.torrent_data.borrow().clone();
            if d.is_empty() {
                Err(anyhow::anyhow!("cookie 失效"))
            } else {
                Ok(d)
            }
        }
    }

    fn site_torrent(files: &[(&str, i64)], root: &str) -> Vec<u8> {
        let file_dicts: Vec<BVal> = files
            .iter()
            .map(|(p, s)| {
                let segs: Vec<BVal> = p.split('/').map(BVal::s).collect();
                BVal::d(vec![("length", BVal::I(*s)), ("path", BVal::L(segs))])
            })
            .collect();
        enc(&BVal::d(vec![(
            "info",
            BVal::d(vec![
                ("files", BVal::L(file_dicts)),
                ("name", BVal::s(root)),
                ("piece length", BVal::I(262144)),
                ("pieces", BVal::s("pppppppppppppppppppp")),
            ]),
        )]))
    }

    fn throttle() -> SiteThrottle {
        SiteThrottle::new(Duration::ZERO, Duration::ZERO, 100, 10, 10)
    }

    #[test]
    fn resolves_on_a_level_and_b_level() {
        let local_files = vec![("Pack/a.mkv".to_string(), 100)];
        let local = TorrentFingerprint::from_local_files(&local_files);
        // A 站：站内没有。B 站：同 hash 种子
        let a = MockSite::no_hit("a.example");
        let hit = SearchHit {
            detail_url: "https://b.example/details.php?id=9".to_string(),
            title: "x".to_string(),
        };
        let st = site_torrent(&[("a.mkv", 100)], "Pack");
        let b = MockSite {
            id: "b.example",
            search_result: RefCell::new(Ok(Some(hit))),
            torrent_data: RefCell::new(st.clone()),
        };
        let hash = bencode::info_hash_v1(&st).unwrap();
        let sites = [
            SourceSite {
                client: &a,
                announce_hosts: vec!["a.example".into()],
            },
            SourceSite {
                client: &b,
                announce_hosts: vec!["b.example".into()],
            },
        ];
        match resolve_source("Show", &[], &[hash], &local, &sites, true, &throttle()) {
            ResolveOutcome::Resolved {
                site_id,
                detail_url,
                level,
                ..
            } => {
                assert_eq!(site_id, "b.example");
                assert_eq!(detail_url, "https://b.example/details.php?id=9");
                assert!(matches!(level, JudgeLevel::SameHash { .. }));
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn seeding_source_site_searched_first() {
        // 两站都能给出同 hash 种子；trackerHosts 里有 b → b 必须先被问（a 的检索计数为 0）
        let local_files = vec![("Pack/a.mkv".to_string(), 100)];
        let local = TorrentFingerprint::from_local_files(&local_files);
        let st = site_torrent(&[("a.mkv", 100)], "Pack");
        let hash = bencode::info_hash_v1(&st).unwrap();
        let mk = |id: &'static str| MockSite {
            id,
            search_result: RefCell::new(Ok(Some(SearchHit {
                detail_url: format!("https://{id}/details.php?id=1"),
                title: "x".to_string(),
            }))),
            torrent_data: RefCell::new(st.clone()),
        };
        let a = mk("a.example");
        let b = mk("b.example");
        let sites = [
            SourceSite {
                client: &a,
                announce_hosts: vec!["a.example".into()],
            },
            SourceSite {
                client: &b,
                announce_hosts: vec!["b.example".into()],
            },
        ];
        match resolve_source(
            "Show",
            &["b.example".to_string()],
            &[hash],
            &local,
            &sites,
            true,
            &throttle(),
        ) {
            ResolveOutcome::Resolved { site_id, .. } => assert_eq!(site_id, "b.example"),
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn name_only_candidate_rejected_then_miss() {
        // 唯一候选是 C 级（清单对不上）→ 不采 → Miss
        let local = TorrentFingerprint::from_local_files(&[("Pack/a.mkv".to_string(), 100)]);
        let a = MockSite {
            id: "a.example",
            search_result: RefCell::new(Ok(Some(SearchHit {
                detail_url: "https://a.example/details.php?id=1".to_string(),
                title: "x".to_string(),
            }))),
            torrent_data: RefCell::new(site_torrent(&[("a.mkv", 999)], "Pack")),
        };
        let sites = [SourceSite {
            client: &a,
            announce_hosts: vec!["a.example".into()],
        }];
        match resolve_source(
            "Show",
            &[],
            &["deadbeef".to_string()],
            &local,
            &sites,
            true,
            &throttle(),
        ) {
            ResolveOutcome::Miss { attempts, clean } => {
                assert!(clean);
                assert!(
                    attempts[0].note.contains("清单对不上"),
                    "note = {}",
                    attempts[0].note
                );
            }
            other => panic!("expected Miss, got {other:?}"),
        }
    }

    #[test]
    fn search_error_is_not_a_clean_miss() {
        // 检索报错（cookie 失效一类）不能记「站内没有」负缓存
        let local = TorrentFingerprint::from_local_files(&[("Pack/a.mkv".to_string(), 100)]);
        let a = MockSite {
            id: "a.example",
            search_result: RefCell::new(Err("cookie 失效".to_string())),
            torrent_data: RefCell::new(Vec::new()),
        };
        let sites = [SourceSite {
            client: &a,
            announce_hosts: vec!["a.example".into()],
        }];
        match resolve_source(
            "Show",
            &[],
            &["h".to_string()],
            &local,
            &sites,
            true,
            &throttle(),
        ) {
            ResolveOutcome::Miss { attempts, clean } => {
                assert!(!clean, "报错的 miss 不是 clean");
                assert!(
                    attempts[0].note.contains("检索失败"),
                    "note = {}",
                    attempts[0].note
                );
            }
            other => panic!("expected Miss, got {other:?}"),
        }
    }

    #[test]
    fn rate_limited_error_backs_off_the_site() {
        // 站点回 429：不算 clean miss，且该站在 throttle 上要能看到间隔翻倍
        let local = TorrentFingerprint::from_local_files(&[("Pack/a.mkv".to_string(), 100)]);
        let a = MockSite {
            id: "a.example",
            search_result: RefCell::new(Err(
                "HTTP 429 检索 https://a.example/torrents.php：Too Many Requests".to_string(),
            )),
            torrent_data: RefCell::new(Vec::new()),
        };
        let sites = [SourceSite {
            client: &a,
            announce_hosts: vec!["a.example".into()],
        }];
        let t = SiteThrottle::new(Duration::from_secs(8), Duration::ZERO, 100, 10, 10);
        match resolve_source("Show", &[], &["h".to_string()], &local, &sites, true, &t) {
            ResolveOutcome::Miss { clean, .. } => assert!(!clean),
            other => panic!("expected Miss, got {other:?}"),
        }
        // 退避已生效：拨钟到 1s 前，下次领号至少等 60s − 1s（限流后的最低间隔），
        // 没接线的话只有 7s
        t.force_last("a.example", Duration::from_secs(1));
        let wait = t.acquire_dur("a.example", BudgetKind::Source).unwrap();
        let expect = Duration::from_secs(59);
        let tol = Duration::from_millis(200);
        assert!(
            wait >= expect.saturating_sub(tol) && wait <= expect + tol,
            "got {wait:?}, expected ~{expect:?}"
        );
    }

    #[test]
    fn budget_exhaustion_stops_the_walk() {
        let local = TorrentFingerprint::from_local_files(&[("Pack/a.mkv".to_string(), 100)]);
        let a = MockSite::no_hit("a.example");
        let b = MockSite::no_hit("b.example");
        let sites = [
            SourceSite {
                client: &a,
                announce_hosts: vec!["a.example".into()],
            },
            SourceSite {
                client: &b,
                announce_hosts: vec!["b.example".into()],
            },
        ];
        // 源站预算 2：a 严格+宽松两发用完（记「站内没有」），b 一进门就收工
        let t = SiteThrottle::new(Duration::ZERO, Duration::ZERO, 100, 2, 10);
        match resolve_source("Show", &[], &["h".to_string()], &local, &sites, true, &t) {
            ResolveOutcome::BudgetExhausted { attempts } => {
                assert_eq!(attempts.len(), 2);
                assert_eq!(attempts[0].site_id, "a.example");
                assert_eq!(attempts[0].note, "站内没有");
                assert_eq!(attempts[1].site_id, "b.example");
            }
            other => panic!("expected BudgetExhausted, got {other:?}"),
        }
    }
}
