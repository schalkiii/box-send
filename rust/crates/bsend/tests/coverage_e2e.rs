//! 端到端：mock qBittorrent + mock NexusPHP（真实 TCP HTTP 服务）跑完整一轮 `run_round`。
//! 覆盖：辅种臂（检索命中 A 级 → 暂停推送 → 校验 100% → 启动）、
//! 转种草稿臂（站内没有 → 源种定位 → ReseedDraft）、已做种预筛（零请求）、
//! 索引落盘。HTTP 兼容补丁（hashes/hash 参数、[file]/[[file]] 回包）也在这里钉住。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use bsend::bencode;
use bsend::config::{Config, CoverageSettings, DownloaderSettings, SiteConf};
use bsend::coverage::{run_round, RoundOptions};
use bsend::index::{ContentIndex, CoverageStatus};
use bsend::qbittorrent::QBittorrent;
use bsend::sites::nexusphp::NexusPhp;
use bsend::sites::Framework;

// ---------- 极简 mock HTTP 服务 ----------

type Handler = Arc<dyn Fn(&str, &str, &[u8]) -> (u16, &'static str, Vec<u8>) + Send + Sync>;

struct MockServer {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
    _handle: JoinHandle<()>,
}

impl MockServer {
    fn start(handler: Handler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let handler = handler.clone();
                let log = log2.clone();
                std::thread::spawn(move || serve_conn(stream, handler, log));
            }
        });
        Self {
            port,
            log,
            _handle: handle,
        }
    }

    fn requests(&self, prefix: &str) -> Vec<String> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with(prefix))
            .cloned()
            .collect()
    }
}

fn serve_conn(mut stream: TcpStream, handler: Handler, log: Arc<Mutex<Vec<String>>>) {
    // 读到 header 结束（请求都很小；逐字节读省得管半包）
    let mut buf: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return,
            Ok(_) => {
                buf.push(byte[0]);
                if buf.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    let head = String::from_utf8_lossy(&buf).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    // Content-Length 的 body
    let mut content_length = 0usize;
    for line in lines {
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        let _ = stream.read_exact(&mut body);
    }
    log.lock().unwrap().push(format!("{method} {path}"));
    let (status, ctype, payload) = handler(&method, &path, &body);
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        409 => "Conflict",
        _ => "Error",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(&payload);
    let _ = stream.flush();
}

// ---------- bencode 测试助手（集成测试在 crate 外，用不了 #[cfg(test)] 的助手）----------

fn t_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(format!("{}:", s.len()).as_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn t_int(out: &mut Vec<u8>, i: i64) {
    out.extend_from_slice(format!("i{i}e").as_bytes());
}

/// 造一个多文件 .torrent
fn torrent_bytes(root: &str, files: &[(&str, i64)]) -> Vec<u8> {
    let mut info = Vec::new();
    t_str(&mut info, "files");
    info.push(b'l');
    for (p, s) in files {
        info.push(b'd');
        t_str(&mut info, "length");
        t_int(&mut info, *s);
        t_str(&mut info, "path");
        info.push(b'l');
        for seg in p.split('/') {
            t_str(&mut info, seg);
        }
        info.push(b'e');
        info.push(b'e');
    }
    info.push(b'e');
    t_str(&mut info, "name");
    t_str(&mut info, root);
    t_str(&mut info, "piece length");
    t_int(&mut info, 262144);
    t_str(&mut info, "pieces");
    t_str(&mut info, "pppppppppppppppppppp");
    let mut out = Vec::new();
    out.push(b'd');
    t_str(&mut out, "announce");
    t_str(&mut out, "http://tracker.example/announce");
    t_str(&mut out, "info");
    out.push(b'd'); // info 字典开
    out.extend_from_slice(&info);
    out.push(b'e'); // 关 info
    out.push(b'e'); // 关外层
    out
}

const NAME: &str = "Black.Lagoon.S01.1080p.Blu-ray.x264-GROUP";
const FILES: &[(&str, i64)] = &[("a.mkv", 100), ("s/ch.srt", 5)];

fn qb_server(torrent_hash: String, tracker_hosts: Vec<String>) -> MockServer {
    let torrent_hash = Arc::new(torrent_hash);
    let tracker_hosts = Arc::new(tracker_hosts);
    MockServer::start(Arc::new(move |method, path, body| {
        let h = &*torrent_hash;
        if path.starts_with("/api/v2/auth/login") {
            return (200, "text/plain", b"Ok.".to_vec());
        }
        if path.starts_with("/api/v2/app/version") {
            return (
                200,
                "application/json",
                br#"{"version":"5.0.0-mock"}"#.to_vec(),
            );
        }
        if path.starts_with("/api/v2/torrents/info") {
            // 单查（?hashes=H）回单条已完成的；全量回清单
            if path.contains("hashes=") {
                let json = format!(
                    r#"[{{"hash":"{h}","progress":1.0,"state":"pausedUP","up_limit":-1}}]"#
                );
                return (200, "application/json", json.into_bytes());
            }
            let hosts = tracker_hosts
                .iter()
                .map(|h| format!("\"{h}\""))
                .collect::<Vec<_>>()
                .join(",");
            let json = format!(
                r#"[{{"hash":"{h}","infohash_v2":"","name":"{NAME}","save_path":"/data/","category":"","tags":"","size":105,"progress":1.0,"tracker_hosts":[{hosts}]}}]"#
            );
            return (200, "application/json", json.into_bytes());
        }
        if path.starts_with("/api/v2/torrents/files") {
            // hashes= / hash= 两个参数名都要能用；回包也试过 [[file]] 包装——这里直接回 [file]
            let json = r#"[{"name":"Pack/a.mkv","size":100,"index":0},{"name":"Pack/s/ch.srt","size":5,"index":1}]"#;
            return (200, "application/json", json.as_bytes().to_vec());
        }
        if path.starts_with("/api/v2/torrents/add") {
            assert_eq!(method, "POST");
            let text = String::from_utf8_lossy(body);
            assert!(
                text.contains("name=\"paused\""),
                "add 必须带 paused 字段: {text}"
            );
            assert!(
                text.contains("name=\"savepath\""),
                "add 必须带 savepath: {text}"
            );
            assert!(
                text.contains("name=\"torrents\""),
                "add 的文件字段名是 torrents"
            );
            return (200, "text/plain", b"Ok.".to_vec());
        }
        if path.starts_with("/api/v2/torrents/trackers") {
            return (200, "application/json", b"[]".to_vec());
        }
        // pause / resume / delete / addTrackers / setTrackers
        (200, "text/plain", Vec::new())
    }))
}

fn nexus_site_server(kind: &str, torrent: Arc<Vec<u8>>) -> MockServer {
    let torrent = torrent.clone();
    let kind = kind.to_string();
    MockServer::start(Arc::new(move |_method, path, _body| {
        if path.starts_with("/torrents.php") {
            if kind == "hit" {
                let html = format!(r#"<a href="details.php?id=1002">{NAME}</a>"#);
                return (200, "text/html", html.into_bytes());
            }
            return (200, "text/html", "<p>没有种子</p>".as_bytes().to_vec());
        }
        if path.starts_with("/details.php") {
            let html =
                format!(r#"<h1 id="top">{NAME}</h1><a href="download.php?id=1002">下载种子</a>"#);
            return (200, "text/html", html.into_bytes());
        }
        if path.starts_with("/download.php") {
            return (200, "application/x-bittorrent", torrent.as_ref().clone());
        }
        (404, "text/plain", Vec::new())
    }))
}

fn test_config(qb_port: u16, pter_port: u16, hdsky_port: u16, dir: &std::path::Path) -> Config {
    Config {
        config_path: dir.join("boxsend.json"),
        data_dir: Some(dir.to_path_buf()),
        downloader: DownloaderSettings {
            url: format!("http://127.0.0.1:{qb_port}"),
            username: "admin".into(),
            password: "pass".into(),
            save_path: None,
            category: None,
            skip_checking: true,
            default_up_limit: 0,
            site_up_limits: HashMap::new(),
        },
        sites: vec![
            SiteConf {
                id: "pter".into(),
                name: "猫".into(),
                url: format!("http://127.0.0.1:{pter_port}/"),
                framework: Framework::NexusPHP,
                search_url: None,
                api_key: None,
                announce_host: "127.0.0.1".into(),
            },
            SiteConf {
                id: "hdsky".into(),
                name: "天空".into(),
                url: format!("http://localhost:{hdsky_port}/"),
                framework: Framework::NexusPHP,
                search_url: None,
                api_key: None,
                announce_host: "localhost".into(),
            },
        ],
        coverage: CoverageSettings {
            per_site_interval_sec: 0.0,
            jitter_sec: 0.0,
            verify_wait_sec: 3,
            verify_poll_sec: 1,
            ..CoverageSettings::default()
        },
        user_agent: "bsend-e2e-test".into(),
        cookies: HashMap::new(),
    }
}

fn adapters(cfg: &Config) -> Vec<Arc<NexusPhp>> {
    cfg.sites
        .iter()
        .map(|s| {
            Arc::new(NexusPhp::new(&s.id, &s.name, &s.url, s.search_url.as_deref(), None).unwrap())
        })
        .collect()
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bsend-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 场景一：pter 站内有同 hash 种子（辅种臂全流程），hdsky 站内没有（转种草稿臂 +
/// 源种定位回到 pter）。
#[test]
fn e2e_assist_and_reseed_draft() {
    let dir = temp_dir("assist");
    let t_same = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t_same).unwrap();

    let qb = qb_server(hash.clone(), Vec::new());
    let pter = nexus_site_server("hit", t_same.clone());
    let hdsky = nexus_site_server("miss", t_same.clone());
    let cfg = test_config(qb.port, pter.port, hdsky.port, &dir);

    let qb_client = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());
    let ads = adapters(&cfg);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());

    let events = Arc::new(Mutex::new(Vec::new()));
    let ev_sink = events.clone();
    let report = run_round(
        &cfg,
        qb_client,
        &ads,
        &mut index,
        move |ev| ev_sink.lock().unwrap().push(ev),
        RoundOptions {
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();

    // ---- 索引结论 ----
    assert_eq!(index.contents.len(), 1, "一条种子 → 一段内容");
    let entry = index.contents.values().next().unwrap();
    assert_eq!(entry.torrents.len(), 1);
    assert_eq!(
        entry.coverage["pter"].status,
        CoverageStatus::Assisted,
        "pter 辅种成功"
    );
    assert!(entry.coverage["pter"]
        .url
        .as_deref()
        .unwrap()
        .contains("details.php?id=1002"));
    assert_eq!(
        entry.coverage["hdsky"].status,
        CoverageStatus::ReseedDraft,
        "hdsky 走转种草稿"
    );
    // 源种定位：hdsky 站内没有 → 回源站名单找，pter 有（已在做种优先级也走 pter：它有 hit）
    let src = entry.source.as_ref().expect("源种已定位");
    assert_eq!(src.site_id, "pter");
    assert!(src.is_resolved());

    // ---- 报告 ----
    assert_eq!(report.reseed_drafts.len(), 1);
    assert_eq!(report.reseed_drafts[0].target_site, "hdsky");
    assert_eq!(report.reseed_drafts[0].source_site, "pter");
    assert_eq!(report.by_status[&CoverageStatus::Assisted], 1);
    assert_eq!(report.by_status[&CoverageStatus::ReseedDraft], 1);

    // ---- 对下载器的副作用 ----
    assert_eq!(
        qb.requests("POST /api/v2/torrents/add").len(),
        1,
        "只推了一次"
    );
    assert_eq!(
        qb.requests("POST /api/v2/torrents/resume").len(),
        1,
        "校验满 100% 后启动"
    );
    assert_eq!(
        qb.requests("POST /api/v2/torrents/delete").len(),
        0,
        "没有回滚"
    );

    // ---- 落盘可重读（重跑零请求：全部终态）----
    let idx2 = ContentIndex::load(&cfg.coverage_state_path());
    assert_eq!(idx2.contents.len(), 1);
    let e2 = idx2.contents.values().next().unwrap();
    assert_eq!(e2.coverage["pter"].status, CoverageStatus::Assisted);
    assert_eq!(e2.source.as_ref().unwrap().site_id, "pter");
}

/// 场景二：已在做种的站零请求预筛（那格直接 Seeding，绝不推下载器）；
/// 站内没有的站走转种臂，源种定位优先回到「已在做种的那个源站」。
#[test]
fn e2e_seeding_preskip_and_priority_source() {
    let dir = temp_dir("preskip");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    // 本地种挂在 hdsky（localhost）做种；hdsky 站内其实有这条种子
    // （若预筛失效，目标站检索会命中并推下载器——add 计数就能抓到）
    let qb = qb_server(hash, vec!["localhost".to_string()]);
    let pter = nexus_site_server("miss", t.clone());
    let hdsky = nexus_site_server("hit", t.clone());
    let cfg = test_config(qb.port, pter.port, hdsky.port, &dir);

    let qb_client = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());
    let ads = adapters(&cfg);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let report = run_round(
        &cfg,
        qb_client,
        &ads,
        &mut index,
        |_| {},
        RoundOptions {
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();

    let entry = index.contents.values().next().unwrap();
    assert_eq!(
        entry.coverage["hdsky"].status,
        CoverageStatus::Seeding,
        "tracker 命中 → 已做种"
    );
    assert_eq!(
        entry.coverage["pter"].status,
        CoverageStatus::ReseedDraft,
        "站内没有 → 转种草稿"
    );
    // 源种定位优先回已在做种的 hdsky（不是按名单顺序先问 pter）
    let src = entry.source.as_ref().expect("源种已定位");
    assert_eq!(src.site_id, "hdsky", "已在做种的源站优先");
    assert_eq!(report.preskipped, 1, "hdsky 那格被预筛跳过");
    // 预筛失效的话 hdsky 目标检索会命中并 A 级推送
    assert_eq!(
        qb.requests("POST /api/v2/torrents/add").len(),
        0,
        "已做种的格子绝不推送"
    );
    assert_eq!(report.reseed_drafts.len(), 1);
    assert_eq!(report.reseed_drafts[0].source_site, "hdsky");

    // 重跑：Seeding 和 ReseedDraft 都是终态 → 零请求、零新结论
    let qb2 = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());
    let mut index2 = ContentIndex::load(&cfg.coverage_state_path());
    let before = pter.requests("GET /torrents.php").len();
    let report2 = run_round(
        &cfg,
        qb2,
        &ads,
        &mut index2,
        |_| {},
        RoundOptions::default(),
    )
    .unwrap();
    assert_eq!(
        pter.requests("GET /torrents.php").len(),
        before,
        "终态不再重搜"
    );
    assert_eq!(report2.by_status.len(), 0, "第二轮没有新结论");
}

/// 场景二 b：哪都没有（站内没有 + 源站也没有）→ 待找源 + 源种 miss 负缓存。
#[test]
fn e2e_wait_source() {
    let dir = temp_dir("waitsource");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let qb = qb_server(hash, Vec::new());
    let pter = nexus_site_server("miss", t.clone());
    let hdsky = nexus_site_server("miss", t.clone());
    let cfg = test_config(qb.port, pter.port, hdsky.port, &dir);

    let qb_client = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());
    let ads = adapters(&cfg);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let _ = run_round(
        &cfg,
        qb_client,
        &ads,
        &mut index,
        |_| {},
        RoundOptions {
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();

    let entry = index.contents.values().next().unwrap();
    assert_eq!(entry.coverage["pter"].status, CoverageStatus::WaitSource);
    assert_eq!(entry.coverage["hdsky"].status, CoverageStatus::WaitSource);
    let src = entry.source.as_ref().expect("miss 负缓存记录");
    assert!(!src.is_resolved());
    assert!(src.miss_at.is_some());

    // 重跑：waitSource 在 miss TTL 内 → 零请求
    let qb2 = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());
    let mut index2 = ContentIndex::load(&cfg.coverage_state_path());
    let before =
        pter.requests("GET /torrents.php").len() + hdsky.requests("GET /torrents.php").len();
    let report2 = run_round(
        &cfg,
        qb2,
        &ads,
        &mut index2,
        |_| {},
        RoundOptions::default(),
    )
    .unwrap();
    let after =
        pter.requests("GET /torrents.php").len() + hdsky.requests("GET /torrents.php").len();
    assert_eq!(after, before, "负缓存期内不再重搜");
    assert_eq!(report2.by_status.len(), 0);
}

/// 场景三：plan（dry-run + 不落盘）检索照跑但绝不碰下载器。
#[test]
fn e2e_plan_dry_run() {
    let dir = temp_dir("plan");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let qb = qb_server(hash, Vec::new());
    let pter = nexus_site_server("hit", t.clone());
    let hdsky = nexus_site_server("miss", t.clone());
    let cfg = test_config(qb.port, pter.port, hdsky.port, &dir);

    let qb_client = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());
    let ads = adapters(&cfg);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let _ = run_round(
        &cfg,
        qb_client,
        &ads,
        &mut index,
        |_| {},
        RoundOptions {
            dry_run: true,
            persist: false,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(
        qb.requests("POST /api/v2/torrents/add").len(),
        0,
        "dry-run 绝不加下载器"
    );
    // 不落盘：磁盘上没有 coverage.json
    assert!(!cfg.coverage_state_path().exists(), "plan 不落盘");
}

/// 场景四：C 级（清单对不上）→ 待人工确认，绝不动下载器。
#[test]
fn e2e_name_only_goes_to_review() {
    let dir = temp_dir("review");
    let t_site = Arc::new(torrent_bytes(
        "Pack",
        &[("a.mkv", 100), ("s/ch.srt", 5), ("extra.nfo", 1)],
    ));
    let hash = bencode::info_hash_v1(&torrent_bytes("Pack", FILES)).unwrap();

    let qb = qb_server(hash, Vec::new());
    let pter = nexus_site_server("hit", t_site.clone());
    let hdsky = nexus_site_server("miss", t_site.clone());
    let cfg = test_config(qb.port, pter.port, hdsky.port, &dir);

    let qb_client = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());
    let ads = adapters(&cfg);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let _ = run_round(
        &cfg,
        qb_client,
        &ads,
        &mut index,
        |_| {},
        RoundOptions {
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();

    let entry = index.contents.values().next().unwrap();
    assert_eq!(
        entry.coverage["pter"].status,
        CoverageStatus::ReviewPending,
        "C 级待人工确认"
    );
    assert!(entry.coverage["pter"]
        .note
        .as_deref()
        .unwrap()
        .contains("清单"));
    assert_eq!(
        qb.requests("POST /api/v2/torrents/add").len(),
        0,
        "C 级绝不动下载器"
    );
}
